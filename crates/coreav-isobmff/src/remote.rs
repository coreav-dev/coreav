//! Probe and index over range reads.
//!
//! [`RangeReader`] sits in front of a remote [`Source`] (HTTP ranges, object
//! storage): it counts and caps requested bytes and requests and keeps the
//! windows it fetched. After [`crate::probe`] through it, [`plan_index`]
//! decides from probe results how the fragments can be indexed, as one typed
//! [`IndexRoute`]. Packets are then listed through the same reader; the box
//! walk stays authoritative, so a wrong hint costs reads, never results.

use crate::error::LimitKind;
use crate::header::read_header;
use crate::{Error, FourCc, MAX_READ, Movie, Source};
use std::collections::BTreeMap;

/// Read policy of the remote index path. Every value is configurable; the
/// defaults ([`RangePolicy::default`]) are the initial values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangePolicy {
    /// Byte bar in basis points of the object length: requested bytes of
    /// probe plus index (default 5%).
    pub max_read_share_bps: u32,
    /// Largest speculative window per fragment start (default 4 KiB).
    pub max_window: u32,
    /// Smallest speculative window per fragment start.
    pub min_window: u32,
    /// Cumulative speculative bytes of one plan (default 4 MiB).
    pub max_speculative_bytes: u64,
    /// Fragments a file without a usable segment index may have and still be
    /// walked one by one.
    pub max_unindexed_fragments: u32,
    /// Bytes of fetched windows the reader keeps.
    pub max_held_bytes: u64,
}

impl Default for RangePolicy {
    fn default() -> Self {
        Self {
            max_read_share_bps: 500,
            max_window: 4 << 10,
            min_window: 512,
            max_speculative_bytes: 4 << 20,
            max_unindexed_fragments: 8,
            max_held_bytes: 8 << 20,
        }
    }
}

impl RangePolicy {
    /// The byte budget for an object of `len` bytes:
    /// `floor(len × max_read_share_bps / 10,000)`.
    pub fn byte_budget(&self, len: u64) -> u64 {
        let budget = u128::from(len).saturating_mul(u128::from(self.max_read_share_bps)) / 10_000;
        u64::try_from(budget).unwrap_or(u64::MAX)
    }
}

/// A byte range to fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    /// Absolute offset of the first byte.
    pub offset: u64,
    /// Length in bytes, at most [`RangePolicy::max_window`].
    pub len: u32,
}

impl ByteRange {
    /// Offset just past the range.
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(u64::from(self.len))
    }
}

/// The windows to fetch before listing the packets of a fragmented file whose
/// segment index locates every fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentPlan {
    /// One window per subsegment start not already held, in file order.
    pub windows: Vec<ByteRange>,
    /// Distinct subsegments named by the segment indexes.
    pub subsegments: u64,
    /// Bytes requested before the plan (probe).
    pub requested: u64,
    /// The byte budget of the request ([`RangePolicy::byte_budget`]).
    pub byte_budget: u64,
}

impl FragmentPlan {
    /// Sum of the window lengths.
    pub fn window_bytes(&self) -> u64 {
        self.windows
            .iter()
            .fold(0u64, |sum, w| sum.saturating_add(u64::from(w.len)))
    }
}

/// Why the segment indexes found by probe cannot locate the fragments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexStatus {
    /// No segment index comes before the first fragment.
    Absent,
    /// A reference points to another segment index after the first fragment;
    /// those are not followed.
    Hierarchical,
    /// The referenced ranges do not start at the first fragment.
    WrongStart,
    /// The referenced ranges overlap or leave a gap.
    NotContiguous,
    /// The first subsegment is not exactly one fragment, so one window per
    /// subsegment would not find every `moof`.
    SeveralFragmentsPerSubsegment,
    /// More fragments follow the referenced ranges (for example one segment
    /// index per segment).
    Incomplete,
}

/// Why a file is routed to download-then-index, outside the H7 claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadReason {
    /// No usable segment index, and more fragments than
    /// [`RangePolicy::max_unindexed_fragments`] by the estimate.
    TooManyUnindexedFragments {
        /// Estimated number of fragments.
        estimated_fragments: u64,
        /// Why the segment index could not be used.
        index: IndexStatus,
    },
    /// Probe plus the planned windows, or plus one 4 KiB fill per estimated
    /// fragment of a sequential walk, would exceed the byte budget.
    ByteBudget {
        /// Bytes that would be requested.
        planned: u64,
        /// The byte budget.
        allowed: u64,
    },
    /// The planned windows would exceed the speculative budget.
    SpeculativeBudget {
        /// Bytes of the planned windows.
        planned: u64,
        /// [`RangePolicy::max_speculative_bytes`].
        allowed: u64,
    },
    /// The reader cannot keep the planned windows and the reads they imply
    /// ([`RangePolicy::max_held_bytes`]); dropped windows would be read again.
    HeldBudget {
        /// Bytes the plan needs kept.
        planned: u64,
        /// Bytes the reader can still keep.
        allowed: u64,
    },
}

/// How to build the index of a probed file over range reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IndexRoute {
    /// No movie fragments: the movie box describes every sample.
    MovieBox,
    /// The segment index locates every fragment: fetch the plan's windows
    /// (concurrently if possible), then list packets through the reader.
    SegmentIndex(FragmentPlan),
    /// Few fragments without a usable segment index: list packets through the
    /// reader, which walks them one by one under a request cap.
    Sequential {
        /// Estimated number of fragments.
        estimated_fragments: u64,
        /// Why the segment index could not be used.
        index: IndexStatus,
    },
    /// Download the whole object, then index it locally. Outside the H7
    /// claim.
    DownloadThenIndex(DownloadReason),
}

const FIRST_FILL: usize = 4 << 10;

/// A counting, budgeted, caching reader in front of a remote source.
///
/// Every request to the inner source and every requested byte, speculative
/// read-ahead included, is counted. A miss fills a window that starts at
/// 4 KiB and doubles while reads move forward, never above [`MAX_READ`].
/// Fetched windows are kept up to a byte bound and serve later reads of the
/// same bytes. Going over a byte or request cap is an
/// [`Error::LimitExceeded`] error before the request is sent.
#[derive(Debug)]
pub struct RangeReader<S> {
    inner: S,
    len: Option<u64>,
    windows: BTreeMap<u64, Vec<u8>>,
    held: u64,
    max_held: u64,
    requests: u64,
    bytes: u64,
    max_bytes: Option<u64>,
    max_requests: Option<u64>,
    last_end: Option<u64>,
    next_fill: usize,
    largest_read: usize,
}

impl<S: Source> RangeReader<S> {
    /// Wraps a remote source, keeping at most `policy.max_held_bytes` of
    /// windows. No cap is set.
    pub fn new(inner: S, policy: &RangePolicy) -> Self {
        Self {
            inner,
            len: None,
            windows: BTreeMap::new(),
            held: 0,
            max_held: policy.max_held_bytes,
            requests: 0,
            bytes: 0,
            max_bytes: None,
            max_requests: None,
            last_end: None,
            next_fill: FIRST_FILL,
            largest_read: 0,
        }
    }

    /// Requests sent to the inner source, inserted windows included.
    pub fn requests(&self) -> u64 {
        self.requests
    }

    /// Bytes requested from the inner source, inserted windows included.
    pub fn bytes_requested(&self) -> u64 {
        self.bytes
    }

    /// The largest single request so far.
    pub fn largest_read(&self) -> usize {
        self.largest_read
    }

    /// Bytes of windows currently kept.
    pub fn held_bytes(&self) -> u64 {
        self.held
    }

    /// Caps the total requested bytes (`None`: no cap).
    pub fn set_max_bytes(&mut self, max: Option<u64>) {
        self.max_bytes = max;
    }

    /// Caps the total number of requests (`None`: no cap).
    pub fn set_max_requests(&mut self, max: Option<u64>) {
        self.max_requests = max;
    }

    /// Returns the inner source.
    pub fn into_inner(self) -> S {
        self.inner
    }

    /// Whether the kept windows hold every byte of `range`.
    pub fn holds(&self, range: ByteRange) -> bool {
        self.held_through(
            range.offset,
            usize::try_from(range.len).unwrap_or(usize::MAX),
        )
    }

    /// Adds a window fetched by the caller (for example by a concurrent
    /// client), counted as one request of `data.len()` bytes. Fails without
    /// keeping it when the window is outside the object or over a cap.
    pub fn insert(&mut self, offset: u64, data: Vec<u8>) -> Result<(), Error> {
        let len = self.len()?;
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(Error::Overflow { offset })?;
        if end > len {
            return Err(Error::Truncated { offset });
        }
        self.charge(offset, data.len())?;
        self.keep(offset, data);
        Ok(())
    }

    /// Fetches every range from the inner source, one request each, in order.
    pub fn fetch(&mut self, ranges: &[ByteRange]) -> Result<(), Error> {
        for range in ranges {
            if self.holds(*range) {
                continue;
            }
            let size = usize::try_from(range.len).map_err(|_| Error::LimitExceeded {
                limit: LimitKind::BoxBytes,
                offset: range.offset,
            })?;
            let data = self.request(range.offset, size)?;
            self.keep(range.offset, data);
        }
        Ok(())
    }

    /// Drops kept windows that end at or before `offset`.
    pub fn release_before(&mut self, offset: u64) {
        self.windows
            .retain(|start, data| start.saturating_add(data.len() as u64) > offset);
        self.held = self
            .windows
            .values()
            .fold(0u64, |sum, w| sum.saturating_add(w.len() as u64));
    }

    /// The held bytes from `offset` to the end of the window that covers it.
    /// No kept window contains another (see `keep`), so ends grow with
    /// starts and the window with the latest start at or before `offset` is
    /// the only one that can reach furthest.
    fn held_from(&self, offset: u64) -> Option<&[u8]> {
        let (start, data) = self.windows.range(..=offset).next_back()?;
        let skip = usize::try_from(offset.checked_sub(*start)?).ok()?;
        data.get(skip..).filter(|rest| !rest.is_empty())
    }

    /// Whether held windows, possibly several, cover `len` bytes at `offset`.
    fn held_through(&self, offset: u64, len: usize) -> bool {
        let end = offset.saturating_add(len as u64);
        let mut at = offset;
        while at < end {
            match self.held_from(at) {
                Some(rest) => at = at.saturating_add(rest.len() as u64),
                None => return false,
            }
        }
        true
    }

    fn charge(&mut self, offset: u64, size: usize) -> Result<(), Error> {
        let requests = self.requests.saturating_add(1);
        if self.max_requests.is_some_and(|max| requests > max) {
            return Err(Error::LimitExceeded {
                limit: LimitKind::ReadRequests,
                offset,
            });
        }
        let bytes = self.bytes.saturating_add(size as u64);
        if self.max_bytes.is_some_and(|max| bytes > max) {
            return Err(Error::LimitExceeded {
                limit: LimitKind::ReadBytes,
                offset,
            });
        }
        self.requests = requests;
        self.bytes = bytes;
        self.largest_read = self.largest_read.max(size);
        Ok(())
    }

    /// One counted request of `size` (at most [`MAX_READ`]) bytes.
    fn request(&mut self, offset: u64, size: usize) -> Result<Vec<u8>, Error> {
        if size > MAX_READ {
            return Err(Error::LimitExceeded {
                limit: LimitKind::BoxBytes,
                offset,
            });
        }
        self.charge(offset, size)?;
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| Error::LimitExceeded {
                limit: LimitKind::BoxBytes,
                offset,
            })?;
        data.resize(size, 0);
        self.inner.read_exact_at(offset, &mut data)?;
        self.last_end = offset.checked_add(size as u64);
        Ok(data)
    }

    /// Keeps a window unless the byte bound is reached. A window inside an
    /// existing one is dropped and existing windows inside it are replaced,
    /// so no kept window contains another.
    fn keep(&mut self, offset: u64, data: Vec<u8>) {
        let size = data.len() as u64;
        let end = offset.saturating_add(size);
        if data.is_empty() || self.held_through(offset, data.len()) {
            return;
        }
        if self
            .windows
            .range(..=offset)
            .next_back()
            .is_some_and(|(start, w)| start.saturating_add(w.len() as u64) >= end)
        {
            return;
        }
        let inside: Vec<u64> = self
            .windows
            .range(offset..end)
            .filter(|(start, w)| start.saturating_add(w.len() as u64) <= end)
            .map(|(start, _)| *start)
            .collect();
        let freed = inside.iter().fold(0u64, |sum, start| {
            sum.saturating_add(self.windows.get(start).map_or(0, |w| w.len() as u64))
        });
        if self.held.saturating_sub(freed).saturating_add(size) > self.max_held {
            return;
        }
        for start in inside {
            self.windows.remove(&start);
        }
        self.held = self.held.saturating_sub(freed).saturating_add(size);
        self.windows.insert(offset, data);
    }

    /// Bytes the reader can still keep.
    pub fn held_room(&self) -> u64 {
        self.max_held.saturating_sub(self.held)
    }

    /// A miss: one window of read-ahead when it fits, else exact reads.
    fn miss(&mut self, offset: u64, buf: &mut [u8], total: u64) -> Result<(), Error> {
        if buf.len() > MAX_READ {
            // Larger than any window: exact reads of at most MAX_READ, not kept.
            let mut done = 0usize;
            for chunk in buf.chunks_mut(MAX_READ) {
                let at = offset
                    .checked_add(done as u64)
                    .ok_or(Error::Overflow { offset })?;
                let data = self.request(at, chunk.len())?;
                chunk.copy_from_slice(data.get(..chunk.len()).unwrap_or_default());
                done = done.saturating_add(chunk.len());
            }
            return Ok(());
        }
        self.next_fill = if self.last_end == Some(offset) {
            self.next_fill.saturating_mul(2).min(MAX_READ)
        } else {
            FIRST_FILL
        };
        let available = usize::try_from(total.saturating_sub(offset)).unwrap_or(usize::MAX);
        let mut size = self.next_fill.max(buf.len()).min(available);
        // Speculative bytes never push a request over the byte cap.
        if let Some(max) = self.max_bytes {
            let room = usize::try_from(max.saturating_sub(self.bytes)).unwrap_or(usize::MAX);
            if size > room {
                size = buf.len().max(room.min(size));
            }
        }
        let data = self.request(offset, size)?;
        let prefix = data.get(..buf.len()).ok_or(Error::Truncated { offset })?;
        buf.copy_from_slice(prefix);
        self.keep(offset, data);
        Ok(())
    }
}

impl<S: Source> Source for RangeReader<S> {
    fn len(&mut self) -> Result<u64, Error> {
        if let Some(len) = self.len {
            return Ok(len);
        }
        let len = self.inner.len()?;
        self.len = Some(len);
        Ok(len)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        self.inner.will_read(offset, len);
    }

    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        self.inner.will_read_regions(regions);
    }

    fn wants_regions(&mut self) -> bool {
        self.inner.wants_regions()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let total = self.len()?;
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::Truncated { offset })?;
        if end > total {
            return Err(Error::Truncated { offset });
        }
        // Copy what is held; only the rest from the first gap is requested.
        let mut done = 0usize;
        while done < buf.len() {
            let at = offset.saturating_add(done as u64);
            let rest = buf.get_mut(done..).unwrap_or_default();
            let Some(held) = self.held_from(at) else {
                return self.miss(at, rest, total);
            };
            let count = held.len().min(rest.len());
            let target = rest
                .get_mut(..count)
                .ok_or(Error::Truncated { offset: at })?;
            let source = held.get(..count).ok_or(Error::Truncated { offset: at })?;
            target.copy_from_slice(source);
            done = done.saturating_add(count);
        }
        Ok(())
    }
}

/// Box types that belong to the fragment sequence of a segment.
const FRAGMENT_BOXES: [&[u8; 4]; 7] = [
    b"moof", b"mdat", b"sidx", b"styp", b"ssix", b"prft", b"emsg",
];

/// Decides how to index a probed movie over range reads and
/// caps `reader` for the walk that follows.
///
/// `movie` must come from [`crate::probe`] over `reader` (or a source with
/// the same bytes). Reads at most a few box headers, normally already held.
/// A segment index that does not locate every fragment is a wrong hint, not
/// an error: the file is routed as if it had none.
pub fn plan_index<S: Source>(
    movie: &Movie,
    reader: &mut RangeReader<S>,
    policy: &RangePolicy,
) -> Result<IndexRoute, Error> {
    let Some(first) = movie.first_fragment.filter(|_| movie.has_fragments()) else {
        return Ok(IndexRoute::MovieBox);
    };
    let len = movie.file_len;
    let budget = policy.byte_budget(len);
    let moof = read_header(reader, first, len)?;
    // The first fragment's span: its `moof` and the media data that follows.
    let mut span = moof.size;
    if moof.end() < len {
        let next = read_header(reader, moof.end(), len)?;
        if next.kind == FourCc(*b"mdat") {
            span = span.saturating_add(next.size);
        }
    }
    let status = match located_subsegments(movie, reader, first, span, len)? {
        Ok(subsegments) => {
            return plan_windows(&subsegments, moof.size, reader, policy, first, budget);
        }
        Err(status) => status,
    };
    // Fragment estimate.
    let remaining = len.saturating_sub(first);
    let estimated_fragments = remaining.div_ceil(span.max(1));
    if estimated_fragments > u64::from(policy.max_unindexed_fragments) {
        return Ok(IndexRoute::DownloadThenIndex(
            DownloadReason::TooManyUnindexedFragments {
                estimated_fragments,
                index: status,
            },
        ));
    }
    // Each fragment of the walk costs about one first fill, or its `moof`.
    let planned = estimated_fragments
        .saturating_mul((FIRST_FILL as u64).max(moof.size.saturating_add(16)))
        .saturating_add(reader.bytes_requested());
    if planned > budget {
        return Ok(IndexRoute::DownloadThenIndex(DownloadReason::ByteBudget {
            planned,
            allowed: budget,
        }));
    }
    let extra = u64::from(policy.max_unindexed_fragments)
        .saturating_mul(2)
        .saturating_add(2);
    reader.release_before(first);
    reader.set_max_bytes(Some(budget));
    reader.set_max_requests(Some(reader.requests().saturating_add(extra)));
    Ok(IndexRoute::Sequential {
        estimated_fragments,
        index: status,
    })
}

/// The distinct subsegments `(offset, size)` in file order when the segment
/// indexes locate every fragment, else why not.
fn located_subsegments<S: Source>(
    movie: &Movie,
    reader: &mut RangeReader<S>,
    first: u64,
    span: u64,
    len: u64,
) -> Result<Result<Vec<(u64, u64)>, IndexStatus>, Error> {
    let all = movie.segment_indexes.iter().flat_map(|i| &i.references);
    if all.clone().next().is_none() {
        return Ok(Err(IndexStatus::Absent));
    }
    // A reference to another index before the first fragment names a box
    // probe has already read, so its references are already here.
    if all
        .clone()
        .any(|r| r.is_index && r.offset.saturating_add(u64::from(r.size)) > first)
    {
        return Ok(Err(IndexStatus::Hierarchical));
    }
    let references = all.filter(|r| !r.is_index);
    let count = references.clone().count();
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    ranges
        .try_reserve_exact(count)
        .map_err(|_| Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            offset: first,
        })?;
    ranges.extend(references.map(|r| (r.offset, u64::from(r.size))));
    ranges.sort_unstable();
    ranges.dedup();
    let mut expected = first;
    for (offset, size) in &ranges {
        if *offset != expected {
            return Ok(Err(if expected == first {
                IndexStatus::WrongStart
            } else {
                IndexStatus::NotContiguous
            }));
        }
        expected = offset
            .checked_add(*size)
            .ok_or(Error::Overflow { offset: *offset })?;
    }
    // One window per subsegment finds only its first `moof`.
    if ranges.first().is_some_and(|(_, size)| *size != span) {
        return Ok(Err(IndexStatus::SeveralFragmentsPerSubsegment));
    }
    if expected < len {
        match read_header(reader, expected, len) {
            Ok(next) if FRAGMENT_BOXES.iter().any(|k| **k == next.kind.0) => {
                return Ok(Err(IndexStatus::Incomplete));
            }
            Ok(_) => {}
            // The hint ends inside a box: a wrong hint, not a malformed file.
            Err(Error::Truncated { .. } | Error::InvalidBox { .. }) => {
                return Ok(Err(IndexStatus::NotContiguous));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Ok(ranges))
}

fn plan_windows<S: Source>(
    subsegments: &[(u64, u64)],
    moof_size: u64,
    reader: &mut RangeReader<S>,
    policy: &RangePolicy,
    first: u64,
    budget: u64,
) -> Result<IndexRoute, Error> {
    // Twice the first moof plus a media data header,
    // in 512-byte steps, between min_window and max_window.
    let window = moof_size
        .saturating_mul(2)
        .saturating_add(16)
        .div_ceil(512)
        .saturating_mul(512)
        .clamp(
            u64::from(policy.min_window.min(policy.max_window)),
            u64::from(policy.max_window),
        );
    let wanted = |(offset, size): &(u64, u64)| ByteRange {
        offset: *offset,
        len: u32::try_from(window.min(*size)).unwrap_or(u32::MAX),
    };
    // Windows before the first fragment are not read again.
    reader.release_before(first);
    // A `moof` larger than its window costs a second read of the rest; the
    // first `moof` stands for all of them in the byte prediction.
    let needed = moof_size.saturating_add(16);
    let mut total = 0u64;
    let mut predicted = 0u64;
    let mut count = 0usize;
    for (range, size) in subsegments.iter().map(|s| (wanted(s), s.1)) {
        let whole = ByteRange {
            offset: range.offset,
            len: u32::try_from(needed.min(size)).unwrap_or(u32::MAX),
        };
        if range.len > 0 && !reader.holds(range) {
            total = total.saturating_add(u64::from(range.len));
            count = count.saturating_add(1);
        }
        if !reader.holds(whole) {
            let cost = u64::from(range.len).max(u64::from(whole.len));
            predicted = predicted.saturating_add(cost);
        }
    }
    if total > policy.max_speculative_bytes {
        return Ok(IndexRoute::DownloadThenIndex(
            DownloadReason::SpeculativeBudget {
                planned: total,
                allowed: policy.max_speculative_bytes,
            },
        ));
    }
    let room = reader.held_room();
    if predicted.max(total) > room {
        return Ok(IndexRoute::DownloadThenIndex(DownloadReason::HeldBudget {
            planned: predicted.max(total),
            allowed: room,
        }));
    }
    let requested = reader.bytes_requested();
    let planned = requested.saturating_add(predicted.max(total));
    if planned > budget {
        return Ok(IndexRoute::DownloadThenIndex(DownloadReason::ByteBudget {
            planned,
            allowed: budget,
        }));
    }
    let mut windows = Vec::new();
    windows
        .try_reserve_exact(count)
        .map_err(|_| Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            offset: first,
        })?;
    windows.extend(
        subsegments
            .iter()
            .map(wanted)
            .filter(|range| range.len > 0 && !reader.holds(*range)),
    );
    reader.set_max_bytes(Some(budget));
    Ok(IndexRoute::SegmentIndex(FragmentPlan {
        windows,
        subsegments: subsegments.len() as u64,
        requested,
        byte_budget: budget,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    fn reader(data: &[u8]) -> RangeReader<SliceSource<'_>> {
        RangeReader::new(SliceSource::new(data), &RangePolicy::default())
    }

    #[test]
    fn misses_fill_windows_and_hits_are_free() {
        let data: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        let mut r = reader(&data);
        let mut buf = [0u8; 10];
        r.read_exact_at(100, &mut buf).unwrap();
        assert_eq!(buf[0], 100);
        assert_eq!((r.requests(), r.bytes_requested()), (1, 4096));
        r.read_exact_at(200, &mut buf).unwrap();
        assert_eq!(r.requests(), 1);
        // Forward from the window end: the fill doubles.
        r.read_exact_at(4196, &mut buf).unwrap();
        assert_eq!((r.requests(), r.bytes_requested()), (2, 4096 + 8192));
        // A jump starts again at 4 KiB, cut at the end of the object.
        r.read_exact_at(19_990, &mut buf).unwrap();
        assert_eq!(r.bytes_requested(), 4096 + 8192 + 10);
        assert!(r.read_exact_at(19_995, &mut buf).is_err());
        assert_eq!(r.requests(), 3);
    }

    #[test]
    fn caps_are_checked_before_the_request() {
        let data = vec![7u8; 100_000];
        let mut r = reader(&data);
        r.set_max_bytes(Some(5_000));
        let mut buf = [0u8; 100];
        r.read_exact_at(0, &mut buf).unwrap();
        // Read-ahead shrinks to fit the cap; the exact bytes still fit.
        r.read_exact_at(50_000, &mut buf).unwrap();
        assert!(r.bytes_requested() <= 5_000);
        let error = r.read_exact_at(90_000, &mut [0u8; 4_000]).unwrap_err();
        assert!(matches!(
            error,
            Error::LimitExceeded {
                limit: LimitKind::ReadBytes,
                ..
            }
        ));
        r.set_max_requests(Some(r.requests()));
        let error = r.read_exact_at(70_000, &mut buf).unwrap_err();
        assert!(matches!(
            error,
            Error::LimitExceeded {
                limit: LimitKind::ReadRequests,
                ..
            }
        ));
    }

    #[test]
    fn large_reads_are_split_and_not_kept() {
        let data = vec![1u8; 3 * MAX_READ];
        let mut r = reader(&data);
        let mut buf = vec![0u8; 2 * MAX_READ + 5];
        r.read_exact_at(1, &mut buf).unwrap();
        assert_eq!(r.requests(), 3);
        assert_eq!(r.largest_read(), MAX_READ);
        assert_eq!(r.held_bytes(), 0);
    }

    #[test]
    fn held_bytes_are_bounded_and_inserts_are_counted() {
        let data = vec![0u8; 50_000];
        let policy = RangePolicy {
            max_held_bytes: 5_000,
            ..RangePolicy::default()
        };
        let mut r = RangeReader::new(SliceSource::new(&data), &policy);
        r.read_exact_at(0, &mut [0u8; 8]).unwrap();
        r.read_exact_at(20_000, &mut [0u8; 8]).unwrap();
        assert_eq!(r.held_bytes(), 4096);
        r.insert(30_000, vec![0u8; 100]).unwrap();
        assert!(r.holds(ByteRange {
            offset: 30_010,
            len: 90
        }));
        assert!(!r.holds(ByteRange {
            offset: 30_010,
            len: 91
        }));
        assert_eq!(r.requests(), 3);
        assert!(r.insert(49_990, vec![0u8; 11]).is_err());
        assert_eq!(r.requests(), 3);
        r.release_before(4_000);
        assert_eq!(r.held_bytes(), 4196);
        r.release_before(25_000);
        assert_eq!(r.held_bytes(), 100);
    }

    #[test]
    fn overlapping_windows_keep_cached_bytes_reachable() {
        let data: Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
        let mut r = reader(&data);
        r.read_exact_at(0, &mut [0u8; 8]).unwrap();
        // A window inside a held one is not kept, and costs no later read.
        r.insert(1_000, data[1_000..1_100].to_vec()).unwrap();
        let mut buf = [0u8; 200];
        r.read_exact_at(1_000, &mut buf).unwrap();
        assert_eq!(&buf[..], &data[1_000..1_200]);
        assert_eq!((r.requests(), r.held_bytes()), (2, 4096));
        // A larger window replaces the windows inside it.
        r.insert(5_000, data[5_000..5_100].to_vec()).unwrap();
        r.insert(4_096, data[4_096..6_000].to_vec()).unwrap();
        assert_eq!(r.held_bytes(), 4096 + 1904);
        // A read across two windows is stitched without a request.
        let mut buf = vec![0u8; 3_000];
        r.read_exact_at(3_000, &mut buf).unwrap();
        assert_eq!(&buf[..], &data[3_000..6_000]);
        assert_eq!(r.requests(), 4);
        // A read with a held prefix requests only the rest.
        let before = r.bytes_requested();
        let mut buf = vec![0u8; 1_000];
        r.read_exact_at(5_500, &mut buf).unwrap();
        assert_eq!(&buf[..], &data[5_500..6_500]);
        assert_eq!(r.requests(), 5);
        assert_eq!(r.bytes_requested() - before, 4096);
    }

    #[test]
    fn byte_budget_is_five_percent_rounded_down() {
        let policy = RangePolicy::default();
        assert_eq!(policy.byte_budget(1_000_000), 50_000);
        assert_eq!(policy.byte_budget(39), 1);
        assert_eq!(policy.byte_budget(u64::MAX), u64::MAX / 20);
    }
}
