//! Sample tables (ISO/IEC 14496-12, 8.6 and 8.7), kept packed as stored.
//!
//! Tables are read into memory only after their entry counts are checked
//! against the box payload and the table budget, so a huge count with a tiny
//! table is rejected without allocating. Validation walks run-length entries,
//! never individual samples.

use crate::bytes::{be_u32_at, be_u64_at};
use crate::error::LimitKind;
use crate::packets::StoredSample;
use crate::{Error, FourCc, Limits};

/// Which table a box holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableKind {
    Stts,
    Ctts,
    Stsc,
    Stsz,
    Stco,
    Co64,
    Stss,
}

impl TableKind {
    pub(crate) fn from_fourcc(kind: FourCc) -> Option<Self> {
        Some(match &kind.0 {
            b"stts" => Self::Stts,
            b"ctts" => Self::Ctts,
            b"stsc" => Self::Stsc,
            b"stsz" => Self::Stsz,
            b"stco" => Self::Stco,
            b"co64" => Self::Co64,
            b"stss" => Self::Stss,
            _ => return None,
        })
    }

    /// Bytes per entry.
    fn width(self) -> u64 {
        match self {
            Self::Stts | Self::Ctts | Self::Co64 => 8,
            Self::Stsc => 12,
            Self::Stsz | Self::Stco | Self::Stss => 4,
        }
    }

    /// Bytes of fixed fields, version and flags included.
    fn fixed(self) -> u64 {
        match self {
            Self::Stsz => 12,
            _ => 8,
        }
    }
}

/// One table as stored: version, entry count and packed entries.
#[derive(Clone, Debug, Default)]
pub(crate) struct Table {
    pub(crate) version: u8,
    /// `ctts` only: whether the sample offsets are signed. ISO/IEC 14496-12
    /// 8.6.1.3 makes them signed in version 1 and unsigned in version 0; the
    /// QuickTime File Format ("Composition offset atom") makes them signed
    /// in its version 0, so a QuickTime file's are signed too
    /// (`docs/design/probe-duration.md`).
    pub(crate) signed: bool,
    pub(crate) count: u32,
    pub(crate) entries: Vec<u8>,
    /// `sample_size` of `stsz` (0 when sizes are listed).
    pub(crate) constant: u32,
}

impl Table {
    /// The sample offset of the first `ctts` entry, as stored.
    pub(crate) fn first_offset(&self) -> Option<u32> {
        if self.count == 0 {
            return None;
        }
        self.u32(0, 4, 8)
    }

    /// Field `field` (byte offset) of entry `entry` of `width` bytes.
    pub(crate) fn field(&self, entry: u32, field: usize, width: usize) -> Option<u32> {
        self.u32(entry, field, width)
    }

    #[inline]
    fn u32(&self, entry: u32, field: usize, width: usize) -> Option<u32> {
        be_u32_at(
            &self.entries,
            (entry as usize).checked_mul(width)?.checked_add(field)?,
        )
    }
}

/// What a sample table box costs before it is read.
pub(crate) struct TableHeader {
    pub(crate) kind: TableKind,
    pub(crate) version: u8,
    pub(crate) count: u32,
    pub(crate) constant: u32,
    /// Bytes of entries to read after the fixed fields.
    pub(crate) entry_bytes: u64,
}

/// Parses the fixed fields of a table box (`head` holds at least the first
/// `fixed + 4` payload bytes when available) and checks that the entries fit
/// in the payload and the budget.
pub(crate) fn table_header(
    kind: TableKind,
    head: &[u8],
    payload_len: u64,
    offset: u64,
    budget_left: u64,
) -> Result<TableHeader, Error> {
    let fixed = kind.fixed();
    let invalid = |reason| Error::InvalidBox {
        offset,
        kind: FourCc(*b"stbl"),
        reason,
    };
    if payload_len < fixed {
        return Err(Error::Truncated { offset });
    }
    let version = *head.first().ok_or(Error::Truncated { offset })?;
    let (constant, count) = if kind == TableKind::Stsz {
        (
            be_u32_at(head, 4).ok_or(Error::Truncated { offset })?,
            be_u32_at(head, 8).ok_or(Error::Truncated { offset })?,
        )
    } else {
        (0, be_u32_at(head, 4).ok_or(Error::Truncated { offset })?)
    };
    let listed = !(kind == TableKind::Stsz && constant != 0);
    let entry_bytes = if listed {
        u64::from(count)
            .checked_mul(kind.width())
            .ok_or(Error::Overflow { offset })?
    } else {
        0
    };
    if entry_bytes > payload_len.saturating_sub(fixed) {
        return Err(invalid("entry count exceeds the box"));
    }
    if entry_bytes > budget_left {
        return Err(Error::LimitExceeded {
            limit: LimitKind::TableBytes,
            offset,
        });
    }
    if version > 1 || (version == 1 && kind != TableKind::Ctts) {
        return Err(invalid("unsupported table version"));
    }
    Ok(TableHeader {
        kind,
        version,
        count,
        constant,
        entry_bytes,
    })
}

/// All tables of one track.
#[derive(Clone, Debug, Default)]
pub(crate) struct SampleTables {
    pub(crate) stts: Option<Table>,
    pub(crate) ctts: Option<Table>,
    pub(crate) stsc: Option<Table>,
    pub(crate) stsz: Option<Table>,
    pub(crate) chunks: Option<(Table, usize)>,
    pub(crate) stss: Option<Table>,
}

impl SampleTables {
    pub(crate) fn set(&mut self, header: &TableHeader, entries: Vec<u8>) -> bool {
        let table = Table {
            version: header.version,
            signed: header.version == 1,
            count: header.count,
            entries,
            constant: header.constant,
        };
        let slot = match header.kind {
            TableKind::Stts => &mut self.stts,
            TableKind::Ctts => &mut self.ctts,
            TableKind::Stsc => &mut self.stsc,
            TableKind::Stsz => &mut self.stsz,
            TableKind::Stss => &mut self.stss,
            TableKind::Stco | TableKind::Co64 => {
                let width = if header.kind == TableKind::Co64 { 8 } else { 4 };
                if self.chunks.is_some() {
                    return false;
                }
                self.chunks = Some((table, width));
                return true;
            }
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(table);
        true
    }

    /// Applies the QuickTime convention for uncompressed audio (QuickTime
    /// File Format, sound sample descriptions version 0 and 1): a constant
    /// sample size of 1 does not give the bytes of a sample. A sample is one
    /// audio frame of `frame_bytes` bytes, and `stsz` counts either the
    /// frames (as many as `stts`) or the bytes (`stts` frames times
    /// `frame_bytes`). Either way the table becomes one frame of
    /// `frame_bytes` per `stts` sample; any other count is left as stored,
    /// for [`SampleTables::validate`] to judge. `work` counts entry visits.
    pub(crate) fn uncompressed_audio(
        &mut self,
        frame_bytes: u32,
        work: &mut Work,
    ) -> Result<(), &'static str> {
        let (Some(stts), Some(stsz)) = (self.stts.as_ref(), self.stsz.as_mut()) else {
            return Ok(());
        };
        if stsz.constant != 1 || frame_bytes <= 1 {
            return Ok(());
        }
        let mut frames = 0u64;
        for i in 0..stts.count {
            work.add(1).map_err(|_| "work")?;
            let count = stts.u32(i, 0, 8).ok_or("stts")?;
            frames = frames
                .checked_add(u64::from(count))
                .ok_or("stts overflow")?;
        }
        let listed = u64::from(stsz.count);
        let as_bytes = frames.checked_mul(u64::from(frame_bytes));
        if listed == frames || Some(listed) == as_bytes {
            if let Ok(count) = u32::try_from(frames) {
                stsz.count = count;
                stsz.constant = frame_bytes;
            }
        }
        Ok(())
    }

    pub(crate) fn sample_count(&self) -> u32 {
        self.stsz.as_ref().map_or(0, |t| t.count)
    }

    /// Checks that the tables agree with each other and returns the sum of
    /// the decode durations. `work` counts entry visits.
    pub(crate) fn validate(
        &self,
        descriptions: u32,
        offset: u64,
        work: &mut Work,
    ) -> Result<u64, &'static str> {
        let samples = u64::from(self.sample_count());
        let stts = self.stts.as_ref().ok_or("missing stts")?;
        let stsc = self.stsc.as_ref().ok_or("missing stsc")?;
        let (chunks, _) = self.chunks.as_ref().ok_or("missing chunk offsets")?;
        if self.stsz.is_none() {
            return Err("missing stsz");
        }
        let mut total = 0u64;
        let mut duration = 0u64;
        for i in 0..stts.count {
            work.add(1).map_err(|_| "work")?;
            let count = stts.u32(i, 0, 8).ok_or("stts")?;
            let delta = stts.u32(i, 4, 8).ok_or("stts")?;
            total = total.checked_add(u64::from(count)).ok_or("stts overflow")?;
            duration = u64::from(count)
                .checked_mul(u64::from(delta))
                .and_then(|d| duration.checked_add(d))
                .ok_or("stts overflow")?;
        }
        if total != samples {
            return Err("stts sample count differs from stsz");
        }
        if i64::try_from(duration).is_err() {
            return Err("stts overflow");
        }
        if let Some(ctts) = &self.ctts {
            let mut total = 0u64;
            for i in 0..ctts.count {
                work.add(1).map_err(|_| "work")?;
                total = total
                    .checked_add(u64::from(ctts.u32(i, 0, 8).ok_or("ctts")?))
                    .ok_or("ctts overflow")?;
            }
            if total != samples {
                return Err("ctts sample count differs from stsz");
            }
        }
        // Samples covered by stsc over all chunks.
        let chunk_count = u64::from(chunks.count);
        let mut covered = 0u64;
        let mut previous_first = 0u64;
        for i in 0..stsc.count {
            work.add(1).map_err(|_| "work")?;
            let first = u64::from(stsc.u32(i, 0, 12).ok_or("stsc")?);
            let per_chunk = u64::from(stsc.u32(i, 4, 12).ok_or("stsc")?);
            let description = stsc.u32(i, 8, 12).ok_or("stsc")?;
            if first <= previous_first || first > chunk_count || (i == 0 && first != 1) {
                return Err("stsc chunk numbers out of order or range");
            }
            if description == 0 || description > descriptions {
                return Err("stsc sample description index out of range");
            }
            let next = if i + 1 < stsc.count {
                u64::from(stsc.u32(i + 1, 0, 12).ok_or("stsc")?)
            } else {
                chunk_count.checked_add(1).ok_or("stsc")?
            };
            let run = next.saturating_sub(first);
            covered = run
                .checked_mul(per_chunk)
                .and_then(|n| covered.checked_add(n))
                .ok_or("stsc overflow")?;
            previous_first = first;
        }
        if covered != samples || (stsc.count == 0 && chunk_count != 0) {
            return Err("stsc does not cover the samples");
        }
        if let Some(stss) = &self.stss {
            let mut previous = 0u32;
            for i in 0..stss.count {
                work.add(1).map_err(|_| "work")?;
                let sample = stss.u32(i, 0, 4).ok_or("stss")?;
                if sample <= previous || u64::from(sample) > samples {
                    return Err("stss out of order or range");
                }
                previous = sample;
            }
        }
        let _ = offset;
        Ok(duration)
    }
}

/// A shared work counter for one request.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Work {
    pub(crate) used: u64,
    pub(crate) max: u64,
}

impl Work {
    pub(crate) fn new(limits: &Limits) -> Self {
        Self {
            used: 0,
            max: limits.max_sample_work,
        }
    }

    #[inline]
    pub(crate) fn add(&mut self, units: u64) -> Result<(), Error> {
        self.used = self.used.saturating_add(units);
        if self.used > self.max {
            return Err(Error::LimitExceeded {
                limit: LimitKind::SampleWork,
                offset: 0,
            });
        }
        Ok(())
    }
}

/// One sample read from the tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RawSample {
    /// Offset of the first sample.
    pub(crate) offset: u64,
    /// Size of each sample.
    pub(crate) size: u32,
    /// Decode time of the first sample.
    pub(crate) dts: i64,
    pub(crate) composition_offset: i64,
    /// Decode duration of each sample.
    pub(crate) duration: u32,
    pub(crate) keyframe: bool,
    /// Number of consecutive samples described (at least 1): back to back in
    /// one chunk or fragment run, with equal size, duration, composition
    /// offset and sync flag.
    pub(crate) count: u32,
}

/// Walks the samples of validated tables in decode order.
#[derive(Clone, Debug)]
pub(crate) struct Cursor {
    sample: u32,
    samples: u32,
    stts_entry: u32,
    stts_left: u32,
    ctts_entry: u32,
    ctts_left: u32,
    stsc_entry: u32,
    chunk: u32,
    chunk_left: u32,
    position: u64,
    stss_entry: u32,
    dts: i64,
    /// Cached `stts` delta, `ctts` offset and next sync sample number of the
    /// current entries, so a step reads the tables only when an entry ends.
    stts_delta: u32,
    ctts_value: i64,
    next_sync: Option<u32>,
    sync_loaded: bool,
}

impl Cursor {
    pub(crate) fn new(tables: &SampleTables) -> Self {
        Self {
            sample: 0,
            samples: tables.sample_count(),
            stts_entry: 0,
            stts_left: 0,
            ctts_entry: 0,
            ctts_left: 0,
            stsc_entry: 0,
            chunk: 0,
            chunk_left: 0,
            position: 0,
            stss_entry: 0,
            dts: 0,
            stts_delta: 0,
            ctts_value: 0,
            next_sync: None,
            sync_loaded: false,
        }
    }

    /// Decode time after the last sample.
    pub(crate) fn dts(&self) -> i64 {
        self.dts
    }

    /// Returns the next run of at most `max` samples (at least one), or
    /// `Ok(None)` after the last sample. Advancing over a run is arithmetic
    /// on the run-length tables; only variable sample sizes are read one by
    /// one.
    // Always inlined: it has two callers since #248's HLS index listed
    // its samples in batches (`Packets::next_runs` and so `fill`), and
    // left to itself the compiler then stops inlining it into
    // `Packets::next_raw`, which costs every packet-by-packet caller a
    // call per sample (3.3% of `clip`'s execution points in the
    // WebAssembly build, 0.9% of its instructions).
    #[inline(always)]
    pub(crate) fn next_run(
        &mut self,
        tables: &SampleTables,
        max: u32,
        work: &mut Work,
    ) -> Result<Option<RawSample>, Error> {
        if self.sample >= self.samples {
            return Ok(None);
        }
        let (Some(stts), Some(stsc), Some((chunks, width)), Some(stsz)) =
            (&tables.stts, &tables.stsc, &tables.chunks, &tables.stsz)
        else {
            return Err(inconsistent());
        };
        // Chunk and offset.
        while self.chunk_left == 0 {
            let chunk_number = self.chunk.checked_add(1).ok_or_else(inconsistent)?;
            // Move to the stsc run that covers this chunk.
            while self.stsc_entry + 1 < stsc.count
                && stsc
                    .u32(self.stsc_entry + 1, 0, 12)
                    .ok_or_else(inconsistent)?
                    <= chunk_number
            {
                self.stsc_entry += 1;
            }
            self.chunk_left = stsc.u32(self.stsc_entry, 4, 12).ok_or_else(inconsistent)?;
            let index = self.chunk as usize;
            self.position = if *width == 8 {
                be_u64_at(
                    &chunks.entries,
                    index.checked_mul(8).ok_or_else(inconsistent)?,
                )
            } else {
                be_u32_at(
                    &chunks.entries,
                    index.checked_mul(4).ok_or_else(inconsistent)?,
                )
                .map(u64::from)
            }
            .ok_or_else(inconsistent)?;
            self.chunk = chunk_number;
        }
        // Decode duration entry.
        while self.stts_left == 0 {
            self.stts_left = stts.u32(self.stts_entry, 0, 8).ok_or_else(inconsistent)?;
            self.stts_delta = stts.u32(self.stts_entry, 4, 8).ok_or_else(inconsistent)?;
            self.stts_entry += 1;
        }
        let duration = self.stts_delta;
        // Composition offset entry.
        let composition_offset = match &tables.ctts {
            Some(ctts) => {
                while self.ctts_left == 0 {
                    self.ctts_left = ctts.u32(self.ctts_entry, 0, 8).ok_or_else(inconsistent)?;
                    let raw = ctts.u32(self.ctts_entry, 4, 8).ok_or_else(inconsistent)?;
                    self.ctts_value = if ctts.signed {
                        i64::from(raw as i32)
                    } else {
                        i64::from(raw)
                    };
                    self.ctts_entry += 1;
                }
                self.ctts_value
            }
            None => 0,
        };
        // Longest run the run-length tables allow.
        let mut count = max
            .max(1)
            .min(self.chunk_left)
            .min(self.stts_left)
            .min(self.samples - self.sample);
        if tables.ctts.is_some() {
            count = count.min(self.ctts_left);
        }
        // Sync samples (1-based numbers): a sync sample stands alone; a run of
        // non-sync samples ends before the next sync sample.
        let number = self.sample + 1;
        let keyframe = match &tables.stss {
            None => true,
            Some(stss) => {
                // Advance the cached next sync sample past samples already read.
                if !self.sync_loaded || self.next_sync.is_some_and(|listed| listed < number) {
                    self.next_sync = None;
                    while self.stss_entry < stss.count {
                        let listed = stss.u32(self.stss_entry, 0, 4).ok_or_else(inconsistent)?;
                        if listed < number {
                            self.stss_entry += 1;
                        } else {
                            self.next_sync = Some(listed);
                            break;
                        }
                    }
                    self.sync_loaded = true;
                }
                match self.next_sync {
                    Some(listed) if listed == number => {
                        count = 1;
                        true
                    }
                    Some(listed) => {
                        count = count.min(listed - number);
                        false
                    }
                    None => false,
                }
            }
        };
        // Sample size: constant, or equal consecutive table entries.
        let size = if stsz.constant != 0 {
            stsz.constant
        } else {
            let first = stsz.u32(self.sample, 0, 4).ok_or_else(inconsistent)?;
            let mut equal = 1;
            while equal < count
                && stsz
                    .u32(self.sample + equal, 0, 4)
                    .ok_or_else(inconsistent)?
                    == first
            {
                equal += 1;
            }
            // Each size entry read beyond the first is a visit.
            work.add(u64::from(equal - 1))?;
            count = equal;
            first
        };
        let offset = self.position;
        let bytes = u64::from(size)
            .checked_mul(u64::from(count))
            .ok_or(Error::Overflow { offset })?;
        self.position = offset
            .checked_add(bytes)
            .ok_or(Error::Overflow { offset })?;
        let ticks = i64::from(duration)
            .checked_mul(i64::from(count))
            .ok_or(Error::Overflow { offset })?;
        let dts = self.dts;
        self.dts = dts.checked_add(ticks).ok_or(Error::Overflow { offset })?;
        self.chunk_left -= count;
        self.stts_left -= count;
        if tables.ctts.is_some() {
            self.ctts_left -= count;
        }
        self.sample += count;
        Ok(Some(RawSample {
            offset,
            size,
            dts,
            composition_offset,
            duration,
            keyframe,
            count,
        }))
    }
}

impl Cursor {
    /// Runs as [`next_run`](Self::next_run) returns them, handed to `emit`
    /// until it has taken `limit` or the samples end; returns how many it
    /// took. After each general step, the samples that stay inside the
    /// current chunk, decode-duration entry and composition-offset entry,
    /// before the next sync sample, are stepped in a tight loop that reads
    /// only their sizes (#30). Work is counted as `Packets::next_run` counts
    /// it and in the same order, one unit for each run before its step, so a
    /// budget that runs out stops both at the same run with the same work.
    pub(crate) fn fill(
        &mut self,
        tables: &SampleTables,
        max: u32,
        work: &mut Work,
        limit: usize,
        mut emit: impl FnMut(RawSample) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let max = max.max(1);
        let mut taken = 0;
        while taken < limit {
            work.add(1)?;
            let Some(first) = self.next_run(tables, max, work)? else {
                break;
            };
            emit(first)?;
            taken += 1;
            let Some(stsz) = &tables.stsz else {
                continue;
            };
            if stsz.constant != 0 {
                // Constant sizes: `next_run` already takes whole runs.
                continue;
            }
            let ctts = tables.ctts.is_some();
            let composition_offset = if ctts { self.ctts_value } else { 0 };
            while taken < limit
                && self.sample < self.samples
                && self.chunk_left > 0
                && self.stts_left > 0
                && (!ctts || self.ctts_left > 0)
            {
                let number = self.sample + 1;
                let mut count = max
                    .min(self.chunk_left)
                    .min(self.stts_left)
                    .min(self.samples - self.sample);
                if ctts {
                    count = count.min(self.ctts_left);
                }
                // The sync flag as `next_run` decides it: a listed sync
                // sample stands alone; a run of other samples ends before
                // the next listed one. A cached entry already passed is left
                // to the general step.
                let mut keyframe = tables.stss.is_none();
                let mut at_sync = false;
                if tables.stss.is_some() {
                    match self.next_sync {
                        Some(listed) if listed == number => {
                            count = 1;
                            keyframe = true;
                            at_sync = true;
                        }
                        Some(listed) if listed < number => break,
                        Some(listed) => count = count.min(listed - number),
                        None if !self.sync_loaded => break,
                        None => {}
                    }
                }
                work.add(1)?;
                let size = stsz.u32(self.sample, 0, 4).ok_or_else(inconsistent)?;
                let mut equal = 1;
                while equal < count
                    && stsz
                        .u32(self.sample + equal, 0, 4)
                        .ok_or_else(inconsistent)?
                        == size
                {
                    equal += 1;
                }
                work.add(u64::from(equal - 1))?;
                let count = equal;
                let offset = self.position;
                let bytes = u64::from(size)
                    .checked_mul(u64::from(count))
                    .ok_or(Error::Overflow { offset })?;
                self.position = offset
                    .checked_add(bytes)
                    .ok_or(Error::Overflow { offset })?;
                let ticks = i64::from(self.stts_delta)
                    .checked_mul(i64::from(count))
                    .ok_or(Error::Overflow { offset })?;
                let dts = self.dts;
                self.dts = dts.checked_add(ticks).ok_or(Error::Overflow { offset })?;
                self.chunk_left -= count;
                self.stts_left -= count;
                if ctts {
                    self.ctts_left -= count;
                }
                self.sample += count;
                if at_sync {
                    // The next listed sync sample, as the general step finds
                    // it: entries are read in order.
                    if let Some(stss) = &tables.stss {
                        self.stss_entry += 1;
                        self.next_sync = if self.stss_entry < stss.count {
                            Some(stss.u32(self.stss_entry, 0, 4).ok_or_else(inconsistent)?)
                        } else {
                            None
                        };
                    }
                }
                emit(RawSample {
                    offset,
                    size,
                    dts,
                    composition_offset,
                    duration: self.stts_delta,
                    keyframe,
                    count,
                })?;
                taken += 1;
            }
        }
        Ok(taken)
    }
}

/// What every sample is checked against: the packet limit and the file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub(crate) max_packet_bytes: u64,
    pub(crate) file_len: u64,
}

impl Cursor {
    /// Visits every remaining sample of the tables in decode order, each as
    /// stored, counting the track's sample numbers from `number`.
    ///
    /// The same samples as the runs of [`fill`](Self::fill) expanded, with
    /// the same checks, the same errors and the same work: one visit per
    /// sample, charged before the sample is read. A caller that indexes
    /// every sample wants no run: once the general step has settled the
    /// chunk, the decode-duration entry, the composition-offset entry and
    /// the next sync sample, the samples until the next of those are a span
    /// whose sizes alone are read, and nothing about a run is built or
    /// compared (#308).
    pub(crate) fn visit<E: From<Error>>(
        &mut self,
        tables: &SampleTables,
        bounds: Bounds,
        work: &mut Work,
        number: &mut u64,
        visit: &mut impl FnMut(StoredSample) -> Result<(), E>,
    ) -> Result<(), E> {
        let (Some(stts), Some(stsc), Some((chunks, width)), Some(stsz)) =
            (&tables.stts, &tables.stsc, &tables.chunks, &tables.stsz)
        else {
            return self.visit_run_by_run(tables, bounds, work, number, visit);
        };
        if stsz.constant != 0 {
            // Constant sizes: a run is the whole of one chunk's arithmetic
            // and costs nothing to expand, so the general step serves.
            return self.visit_run_by_run(tables, bounds, work, number, visit);
        }
        let ctts = tables.ctts.is_some();
        while self.sample < self.samples {
            // Chunk and offset, as the general step reads them.
            while self.chunk_left == 0 {
                let chunk_number = self.chunk.checked_add(1).ok_or_else(inconsistent)?;
                while self.stsc_entry + 1 < stsc.count
                    && stsc
                        .u32(self.stsc_entry + 1, 0, 12)
                        .ok_or_else(inconsistent)?
                        <= chunk_number
                {
                    self.stsc_entry += 1;
                }
                self.chunk_left = stsc.u32(self.stsc_entry, 4, 12).ok_or_else(inconsistent)?;
                let index = self.chunk as usize;
                self.position = if *width == 8 {
                    be_u64_at(
                        &chunks.entries,
                        index.checked_mul(8).ok_or_else(inconsistent)?,
                    )
                } else {
                    be_u32_at(
                        &chunks.entries,
                        index.checked_mul(4).ok_or_else(inconsistent)?,
                    )
                    .map(u64::from)
                }
                .ok_or_else(inconsistent)?;
                self.chunk = chunk_number;
            }
            while self.stts_left == 0 {
                self.stts_left = stts.u32(self.stts_entry, 0, 8).ok_or_else(inconsistent)?;
                self.stts_delta = stts.u32(self.stts_entry, 4, 8).ok_or_else(inconsistent)?;
                self.stts_entry += 1;
            }
            if let Some(table) = &tables.ctts {
                while self.ctts_left == 0 {
                    self.ctts_left = table.u32(self.ctts_entry, 0, 8).ok_or_else(inconsistent)?;
                    let raw = table.u32(self.ctts_entry, 4, 8).ok_or_else(inconsistent)?;
                    self.ctts_value = if table.signed {
                        i64::from(raw as i32)
                    } else {
                        i64::from(raw)
                    };
                    self.ctts_entry += 1;
                }
            }
            // The samples until the next of those entries ends: inside the
            // span every value but the size is settled.
            let mut span = self
                .chunk_left
                .min(self.stts_left)
                .min(self.samples - self.sample);
            if ctts {
                span = span.min(self.ctts_left);
            }
            // Sync samples (1-based numbers), as the general step reads
            // them: a listed sync sample stands alone, and a span of other
            // samples ends before the next listed one.
            let at = self.sample + 1;
            let keyframe = match &tables.stss {
                None => true,
                Some(stss) => {
                    if !self.sync_loaded || self.next_sync.is_some_and(|listed| listed < at) {
                        self.next_sync = None;
                        while self.stss_entry < stss.count {
                            let listed =
                                stss.u32(self.stss_entry, 0, 4).ok_or_else(inconsistent)?;
                            if listed < at {
                                self.stss_entry += 1;
                            } else {
                                self.next_sync = Some(listed);
                                break;
                            }
                        }
                        self.sync_loaded = true;
                    }
                    match self.next_sync {
                        Some(listed) if listed == at => {
                            span = 1;
                            true
                        }
                        Some(listed) => {
                            span = span.min(listed - at);
                            false
                        }
                        None => false,
                    }
                }
            };
            self.visit_span(stsz, span, keyframe, ctts, bounds, work, number, visit)?;
        }
        // The step that finds the end, which the general step charges too.
        Ok(work.add(1)?)
    }

    /// Every remaining sample through the general step, one sample per run:
    /// for tables whose sizes are constant, and for tables the general step
    /// rejects.
    fn visit_run_by_run<E: From<Error>>(
        &mut self,
        tables: &SampleTables,
        bounds: Bounds,
        work: &mut Work,
        number: &mut u64,
        visit: &mut impl FnMut(StoredSample) -> Result<(), E>,
    ) -> Result<(), E> {
        loop {
            work.add(1)?;
            let Some(raw) = self.next_run(tables, 1, work)? else {
                return Ok(());
            };
            let sample = *number;
            *number = number.saturating_add(1);
            check(&raw, bounds)?;
            visit(StoredSample {
                sample,
                offset: raw.offset,
                size: raw.size,
                dts: raw.dts,
                duration: raw.duration,
                composition_offset: raw.composition_offset,
                keyframe: raw.keyframe,
            })?;
        }
    }

    /// Visits the `span` samples that share the settled entries: their sizes
    /// are read from one slice of the table when it holds them all, and one
    /// by one when it does not, which reports the table's own error.
    #[allow(clippy::too_many_arguments)]
    fn visit_span<E: From<Error>>(
        &mut self,
        stsz: &Table,
        span: u32,
        keyframe: bool,
        ctts: bool,
        bounds: Bounds,
        work: &mut Work,
        number: &mut u64,
        visit: &mut impl FnMut(StoredSample) -> Result<(), E>,
    ) -> Result<(), E> {
        let composition_offset = if ctts { self.ctts_value } else { 0 };
        let duration = self.stts_delta;
        let step = i64::from(duration);
        let (mut offset, mut dts, mut sample) = (self.position, self.dts, *number);
        let mut done = 0u32;
        let mut failed = None;
        let sizes = (self.sample as usize)
            .checked_mul(4)
            .zip((span as usize).checked_mul(4))
            .and_then(|(start, len)| stsz.entries.get(start..start.checked_add(len)?));
        // One step over a size, whatever it was read from: the checks and
        // their order are the general step's, then `checked_run`'s.
        let mut one = |size: u32, offset: &mut u64, dts: &mut i64| -> Result<(), E> {
            work.add(1)?;
            let at = *offset;
            let (Some(end), Some(then)) = (at.checked_add(u64::from(size)), dts.checked_add(step))
            else {
                return Err(Error::Overflow { offset: at }.into());
            };
            if u64::from(size) > bounds.max_packet_bytes {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::PacketBytes,
                    offset: at,
                }
                .into());
            }
            if end > bounds.file_len {
                return Err(Error::Truncated { offset: at }.into());
            }
            let (this, was) = (sample, *dts);
            sample = sample.saturating_add(1);
            done += 1;
            *offset = end;
            *dts = then;
            visit(StoredSample {
                sample: this,
                offset: at,
                size,
                dts: was,
                duration,
                composition_offset,
                keyframe,
            })
        };
        match sizes {
            Some(sizes) => {
                for entry in sizes.chunks_exact(4) {
                    // `chunks_exact` gives four bytes; a short one would
                    // mean the slice above was not the span's.
                    let read = entry
                        .first_chunk::<4>()
                        .map(|bytes| u32::from_be_bytes(*bytes))
                        .ok_or_else(|| E::from(inconsistent()));
                    match read.and_then(|size| one(size, &mut offset, &mut dts)) {
                        Ok(()) => {}
                        Err(error) => {
                            failed = Some(error);
                            break;
                        }
                    }
                }
            }
            None => {
                for i in 0..span {
                    let read = stsz
                        .u32(self.sample + i, 0, 4)
                        .ok_or_else(|| E::from(inconsistent()));
                    match read.and_then(|size| one(size, &mut offset, &mut dts)) {
                        Ok(()) => {}
                        Err(error) => {
                            failed = Some(error);
                            break;
                        }
                    }
                }
            }
        }
        self.position = offset;
        self.dts = dts;
        self.sample += done;
        self.chunk_left -= done;
        self.stts_left -= done;
        if ctts {
            self.ctts_left -= done;
        }
        *number = sample;
        match failed {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// A single sample as `checked_run` checks a run of one: within the packet
/// limit and inside the file.
fn check(raw: &RawSample, bounds: Bounds) -> Result<(), Error> {
    let offset = raw.offset;
    if u64::from(raw.size) > bounds.max_packet_bytes {
        return Err(Error::LimitExceeded {
            limit: LimitKind::PacketBytes,
            offset,
        });
    }
    let end = offset
        .checked_add(u64::from(raw.size))
        .ok_or(Error::Overflow { offset })?;
    if end > bounds.file_len {
        return Err(Error::Truncated { offset });
    }
    Ok(())
}

/// The error for tables that disagree with each other during iteration.
/// Where a positioned [`Cursor`] stands.
#[derive(Debug)]
pub(crate) enum Position {
    /// At the sample asked for.
    At(Cursor),
    /// Per-sample iteration would have stopped with an error before that
    /// sample (a sample over `max_packet_bytes`, past the end of the file, or
    /// at an offset that overflows), so there is nothing after it.
    Stopped,
}

impl Cursor {
    /// A cursor at sample `start` (0-based; the sample count for the end),
    /// in the state [`next_run`](Self::next_run) would leave after the
    /// samples before it, computed by arithmetic on the run-length tables
    /// (`docs/design/seek.md`). The samples before `start` are checked as
    /// per-sample iteration checks them, chunk by chunk: each chunk's end
    /// within the file and each size within `max_packet_bytes`, which reads
    /// the listed sizes of `stsz` once. `work` counts table entries visited.
    /// The tables must have passed [`SampleTables::validate`].
    pub(crate) fn at(
        tables: &SampleTables,
        start: u32,
        file_len: u64,
        max_packet_bytes: u64,
        work: &mut Work,
    ) -> Result<Position, Error> {
        let (Some(stts), Some(stsc), Some((chunks, width)), Some(stsz)) =
            (&tables.stts, &tables.stsc, &tables.chunks, &tables.stsz)
        else {
            return Err(inconsistent());
        };
        let mut cursor = Self::new(tables);
        let start = start.min(cursor.samples);
        // Decode times: the stts entry that holds `start`.
        let mut before = 0u32;
        let mut dts = 0i64;
        for entry in 0..stts.count {
            work.add(1)?;
            let count = stts.u32(entry, 0, 8).ok_or_else(inconsistent)?;
            let delta = stts.u32(entry, 4, 8).ok_or_else(inconsistent)?;
            let into = start - before;
            if into < count {
                if into > 0 {
                    cursor.stts_entry = entry + 1;
                    cursor.stts_left = count - into;
                    cursor.stts_delta = delta;
                } else {
                    cursor.stts_entry = entry;
                }
                dts = i64::from(into)
                    .checked_mul(i64::from(delta))
                    .and_then(|d| dts.checked_add(d))
                    .ok_or_else(inconsistent)?;
                break;
            }
            dts = i64::from(count)
                .checked_mul(i64::from(delta))
                .and_then(|d| dts.checked_add(d))
                .ok_or_else(inconsistent)?;
            before += count;
            cursor.stts_entry = entry + 1;
        }
        cursor.dts = dts;
        // Composition offsets, the same way.
        if let Some(ctts) = &tables.ctts {
            let mut before = 0u32;
            for entry in 0..ctts.count {
                work.add(1)?;
                let count = ctts.u32(entry, 0, 8).ok_or_else(inconsistent)?;
                let into = start - before;
                if into < count {
                    if into > 0 {
                        let raw = ctts.u32(entry, 4, 8).ok_or_else(inconsistent)?;
                        cursor.ctts_entry = entry + 1;
                        cursor.ctts_left = count - into;
                        cursor.ctts_value = if ctts.signed {
                            i64::from(raw as i32)
                        } else {
                            i64::from(raw)
                        };
                    } else {
                        cursor.ctts_entry = entry;
                    }
                    break;
                }
                before += count;
                cursor.ctts_entry = entry + 1;
            }
        }
        if start == 0 {
            return Ok(Position::At(Self::new(tables)));
        }
        // Chunks: each chunk wholly before `start` is checked whole, the one
        // that holds `start` up to it.
        let chunk_count = chunks.count;
        let mut first_sample = 0u32;
        for entry in 0..stsc.count {
            work.add(1)?;
            let first = stsc.u32(entry, 0, 12).ok_or_else(inconsistent)?;
            let per_chunk = stsc.u32(entry, 4, 12).ok_or_else(inconsistent)?;
            let next = if entry + 1 < stsc.count {
                stsc.u32(entry + 1, 0, 12).ok_or_else(inconsistent)?
            } else {
                chunk_count.checked_add(1).ok_or_else(inconsistent)?
            };
            for chunk in first..next {
                if first_sample == start && start < cursor.samples {
                    // `start` opens a later chunk: the cursor stands where
                    // the previous chunk ended and loads the next as
                    // iteration would (chunks without samples included).
                    cursor.stsc_entry = entry;
                    cursor.chunk = chunk - 1;
                    cursor.chunk_left = 0;
                    return cursor.finish(tables, start, work);
                }
                work.add(1)?;
                let offset = chunk_offset(chunks, *width, chunk as usize - 1)?;
                let take = per_chunk.min(start - first_sample);
                let Some(position) = checked_sizes(
                    stsz,
                    first_sample,
                    take,
                    offset,
                    file_len,
                    max_packet_bytes,
                    work,
                )?
                else {
                    return Ok(Position::Stopped);
                };
                first_sample += take;
                if take < per_chunk || first_sample == cursor.samples {
                    // `start` is inside this chunk, or the samples end here.
                    cursor.stsc_entry = entry;
                    cursor.chunk = chunk;
                    cursor.chunk_left = per_chunk - take;
                    cursor.position = position;
                    return cursor.finish(tables, start, work);
                }
            }
        }
        Err(inconsistent())
    }

    /// Sets the sample number and the sync-sample state for `start`.
    fn finish(
        mut self,
        tables: &SampleTables,
        start: u32,
        work: &mut Work,
    ) -> Result<Position, Error> {
        self.sample = start;
        if let Some(stss) = &tables.stss {
            // The first listed sync sample at or after `start` (1-based
            // numbers, strictly increasing after validation).
            let number = start.saturating_add(1);
            let (mut low, mut high) = (0u32, stss.count);
            while low < high {
                work.add(1)?;
                let middle = low + (high - low) / 2;
                if stss.u32(middle, 0, 4).ok_or_else(inconsistent)? < number {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            self.stss_entry = low;
            self.next_sync = if low < stss.count {
                Some(stss.u32(low, 0, 4).ok_or_else(inconsistent)?)
            } else {
                None
            };
            self.sync_loaded = true;
        }
        Ok(Position::At(self))
    }
}

/// Offset of chunk `index` (0-based).
fn chunk_offset(chunks: &Table, width: usize, index: usize) -> Result<u64, Error> {
    if width == 8 {
        be_u64_at(
            &chunks.entries,
            index.checked_mul(8).ok_or_else(inconsistent)?,
        )
    } else {
        be_u32_at(
            &chunks.entries,
            index.checked_mul(4).ok_or_else(inconsistent)?,
        )
        .map(u64::from)
    }
    .ok_or_else(inconsistent)
}

/// The position after `count` samples from sample `first` (0-based) laid
/// back to back from `offset`, each checked as per-sample iteration checks
/// it; `None` where that iteration would stop with an error.
fn checked_sizes(
    stsz: &Table,
    first: u32,
    count: u32,
    offset: u64,
    file_len: u64,
    max_packet_bytes: u64,
    work: &mut Work,
) -> Result<Option<u64>, Error> {
    let mut position = offset;
    if count == 0 {
        // Iteration never reads where a chunk without samples lies.
        return Ok(Some(position));
    }
    if stsz.constant != 0 {
        if u64::from(stsz.constant) > max_packet_bytes {
            return Ok(None);
        }
        let Some(end) = u64::from(stsz.constant)
            .checked_mul(u64::from(count))
            .and_then(|bytes| position.checked_add(bytes))
        else {
            return Ok(None);
        };
        if end > file_len {
            return Ok(None);
        }
        return Ok(Some(end));
    }
    work.add(u64::from(count))?;
    for sample in first..first.saturating_add(count) {
        let size = stsz.u32(sample, 0, 4).ok_or_else(inconsistent)?;
        if u64::from(size) > max_packet_bytes {
            return Ok(None);
        }
        let Some(end) = position.checked_add(u64::from(size)) else {
            return Ok(None);
        };
        if end > file_len {
            return Ok(None);
        }
        position = end;
    }
    Ok(Some(position))
}

fn inconsistent() -> Error {
    Error::InvalidBox {
        offset: 0,
        kind: FourCc(*b"stbl"),
        reason: "inconsistent sample tables",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(frames: &[(u32, u32)], stsz_count: u32, constant: u32) -> SampleTables {
        let entries = frames
            .iter()
            .flat_map(|(count, delta)| [count.to_be_bytes(), delta.to_be_bytes()].concat())
            .collect();
        SampleTables {
            stts: Some(Table {
                version: 0,
                signed: false,
                count: u32::try_from(frames.len()).unwrap(),
                entries,
                constant: 0,
            }),
            stsz: Some(Table {
                version: 0,
                signed: false,
                count: stsz_count,
                entries: Vec::new(),
                constant,
            }),
            ..SampleTables::default()
        }
    }

    fn sizes(t: &SampleTables) -> (u32, u32) {
        let stsz = t.stsz.as_ref().unwrap();
        (stsz.count, stsz.constant)
    }

    #[test]
    fn uncompressed_audio_frames() {
        let mut work = Work::new(&Limits::default());
        // QuickTime version 0, 8-bit stereo: stsz counts frames of size 1.
        let mut frames = tables(&[(1000, 1), (500, 1)], 1500, 1);
        frames.uncompressed_audio(2, &mut work).unwrap();
        assert_eq!(sizes(&frames), (1500, 2));
        // QuickTime version 1, 16-bit stereo: stsz counts bytes.
        let mut bytes = tables(&[(1500, 1)], 6000, 1);
        bytes.uncompressed_audio(4, &mut work).unwrap();
        assert_eq!(sizes(&bytes), (1500, 4));
        // Any other count, a real constant size, or 1-byte frames: unchanged.
        for (count, constant, frame) in [(1499, 1, 2), (1500, 2, 4), (1500, 1, 1), (1500, 0, 2)] {
            let mut other = tables(&[(1500, 1)], count, constant);
            other.uncompressed_audio(frame, &mut work).unwrap();
            assert_eq!(sizes(&other), (count, constant));
        }
        // The stts walk counts as work.
        let mut tight = Work { used: 0, max: 1 };
        let mut two = tables(&[(1, 1), (1, 1)], 2, 1);
        assert_eq!(two.uncompressed_audio(2, &mut tight), Err("work"));
    }
}
