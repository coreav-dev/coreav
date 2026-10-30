//! Movie fragments (ISO/IEC 14496-12, 8.8): one `moof` at a time.
//!
//! A fragment's payload is held in memory (bounded by `max_box_bytes`) while
//! its samples are listed; per-sample values are decoded lazily from it, so a
//! run that declares billions of samples costs nothing until they are
//! visited, and every visit counts against the work budget.
//!
//! # What backs a declared sample count
//!
//! A `trun` states how many samples it has, and every per-sample value may
//! come from the `trex` and `tfhd` defaults, so the count alone can claim a
//! fragment far larger than the file. Every count read from a file must be
//! checked against the bytes available, as the sample tables of a
//! non-fragmented movie are (`tables::table_header` refuses an entry count
//! that exceeds its box, and `SampleTables::validate` refuses a sample count
//! the other tables do not back). A `trun`'s count is checked the same way,
//! against the three things that can back a sample:
//!
//! 1. its per-sample fields must fit in the `trun` box
//!    (`count * stride <= remaining`);
//! 2. its media data must lie in the file: the run's data offset plus the
//!    sum of its sample sizes (per-sample, or `count` times the default
//!    size) must not pass the end of the file ([`traf_data_end`]);
//! 3. a run that declares samples whose size comes from a default of zero,
//!    and that state no size of their own, pays nothing anywhere: those
//!    samples cost no byte of the `trun` and no byte of media, so neither
//!    check above bounds what it declares (a sample of zero bytes fits in
//!    any file, however many of them there are). It is refused outright. A
//!    `trun` of no samples declares nothing and is read.
//!
//! Without them a kilobyte could declare 2^32 − 1 samples and a scan would
//! report years of video from it. Memory stayed bounded throughout, which is
//! why this was a validation gap and not a limits one
//! (`docs/measurements/0004-limit-bounds.md`). FFmpeg 7.0.2 refuses the
//! 2^32 − 1 file of the issue when it reads the header, and refuses what
//! check 3 refuses.
//!
//! What check 3 does **not** refuse is a run that writes a size of zero per
//! sample (`sample-size-present`), or writes zero for some of its samples:
//! those samples cost four bytes each in the `trun`, so check 1 bounds the
//! run at the box limit — the same price a `stsz` entry pays in a
//! non-fragmented movie, where a table of zero sizes is read (by FFmpeg
//! too). FFmpeg's fragment reader is stricter than its own sample tables
//! here and refuses any zero-sized sample in a `trun`; following it would
//! mean refusing a fragment that holds one empty sample beside full ones,
//! which this crate reads elsewhere. The residue is measured and pinned
//! (`trun_zero_sample_sizes.mp4` in the hostile suite).
//!
//! The checks judge the `trun`s of the track being read (`trex.track_id`),
//! not every `traf` of the `moof`: a scan visits each track of the file in
//! turn, so each is still judged, while a malformed `traf` of one track no
//! longer takes the readable tracks of the file with it.
//!
//! A track whose media is in another file (`dref`) is
//! not read here at all. Its offsets are offsets into that file, which this
//! crate never opens and whose length it does not know, so nothing here can
//! back what its `trun`s declare — and a declaration nothing backs must not
//! be reported, which is this whole section. It gets no packets either way
//! (`packets::Packets`), so [`crate::Movie::scan_fragments`] leaves its
//! samples uncounted and its duration unknown, as it was before the scan
//! (no duration is read from the headers of a fragmented movie). The same
//! holds for a stream the allowlist dropped, whose `dref` was never read:
//! a fragment of it this file cannot back costs that stream its timing, and
//! nothing else.

use crate::bytes::{Bytes, be_u32_at};
use crate::header::{Parsed, parse_box_header, read_header, read_payload};
use crate::packets::{Budget, SampleRun};
use crate::tables::{RawSample, Work};
use crate::{Error, FourCc, LimitKind, Limits, Source};
use core::cell::Cell;

/// Track defaults from `trex`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trex {
    pub(crate) track_id: u32,
    /// `default_sample_description_index` (1-based), which a `tfhd` may
    /// override.
    pub(crate) description: u32,
    pub(crate) duration: u32,
    pub(crate) size: u32,
    pub(crate) flags: u32,
}

pub(crate) fn parse_trex(payload: &[u8], offset: u64) -> Result<Trex, Error> {
    let mut b = Bytes::new(payload, offset);
    b.version_flags()?;
    let track_id = b.u32()?;
    let description = b.u32()?;
    Ok(Trex {
        track_id,
        description,
        duration: b.u32()?,
        size: b.u32()?,
        flags: b.u32()?,
    })
}

const NON_SYNC: u32 = 0x0001_0000;
/// Bytes charged per held [`SampleRun`] against `max_table_bytes`.
const RUN_BYTES: u64 = 48;

/// One `trun` of the selected track.
#[derive(Clone, Debug)]
struct Run {
    /// Absolute offset of the `trun` payload, for errors.
    at: u64,
    entries_at: usize,
    count: u32,
    stride: usize,
    flags: u32,
    version: u8,
    first_flags: Option<u32>,
    /// Absolute data start, when the run says where its data is.
    data_start: Option<u64>,
    /// Decode time of the first sample, when the `traf` has a `tfdt` and
    /// this is its first run.
    tfdt: Option<i64>,
    defaults: Trex,
}

/// The samples of one track in one fragment.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fragment {
    payload: Vec<u8>,
    runs: Vec<Run>,
    run: usize,
    index: u32,
    /// The sample description index of the last sample returned.
    description: u32,
}

fn invalid(offset: u64, reason: &'static str) -> Error {
    Error::InvalidBox {
        offset,
        kind: FourCc(*b"moof"),
        reason,
    }
}

/// Iterates the child boxes of `data`, which starts at absolute `offset`.
fn children(
    data: &[u8],
    offset: u64,
    boxes: &Cell<u64>,
    mut visit: impl FnMut(FourCc, &[u8], u64, usize) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut at = 0usize;
    while at < data.len() {
        let rest = data.get(at..).unwrap_or_default();
        let absolute = offset.saturating_add(at as u64);
        let header = match parse_box_header(rest, absolute, rest.len() as u64)? {
            Parsed::Done(header) => header,
            Parsed::NeedMore(_) => return Err(Error::Truncated { offset: absolute }),
        };
        boxes.set(boxes.get().saturating_add(1));
        let start = at.saturating_add(usize::from(header.header_len));
        let end =
            at.saturating_add(usize::try_from(header.size).map_err(|_| invalid(absolute, "size"))?);
        let payload = data
            .get(start..end)
            .ok_or(Error::Truncated { offset: absolute })?;
        visit(
            header.kind,
            payload,
            absolute.saturating_add(u64::from(header.header_len)),
            start,
        )?;
        at = end;
    }
    Ok(())
}

/// Bytes of media data one run spans: the sum of its per-sample sizes, or
/// `count` times the default size. `None` when no size is known, which a
/// `traf` whose track has no `trex` and no `tfhd` default size leaves open.
fn run_bytes(payload: &[u8], run: &Run, default_size_known: bool) -> Result<Option<u64>, Error> {
    let overflow = Error::Overflow { offset: run.at };
    if run.flags & 0x200 != 0 {
        // The size follows the duration when both are present.
        let field = if run.flags & 0x100 != 0 { 4 } else { 0 };
        let mut total = 0u64;
        for i in 0..run.count as usize {
            let at = run
                .entries_at
                .saturating_add(i.saturating_mul(run.stride))
                .saturating_add(field);
            let size = be_u32_at(payload, at).ok_or(Error::Truncated { offset: run.at })?;
            total = total.checked_add(u64::from(size)).ok_or(overflow.clone())?;
        }
        return Ok(Some(total));
    }
    if default_size_known {
        return Ok(Some(
            u64::from(run.defaults.size)
                .checked_mul(u64::from(run.count))
                .ok_or(overflow)?,
        ));
    }
    Ok(None)
}

/// Where the data of one track fragment ends: each run starts at its
/// explicit offset or where the previous run ended, and spans
/// [`run_bytes`]. `None` when any run's size or start is not known, even if
/// a later run states its own offset, so that a following `traf` with an
/// implicit base offset still fails rather than take a guess.
///
/// With `bound`, every run whose start and size are both known is checked
/// against it: a run whose samples do not fit in the file is refused where
/// the fragment is read, rather than reported as the samples it claims and
/// refused later, one sample at a time, by
/// [`crate::packets::checked_run`], which this reports the same way
/// ([`Error::Truncated`]: the bytes the run names are not there). The
/// caller leaves `bound` out for the track fragments it is not reading and
/// for a track whose media is in another file (see this module's
/// documentation). A run whose start or size is unknown is not checked, and
/// does not stop the runs after it from being checked.
fn traf_data_end(
    payload: &[u8],
    base: Option<u64>,
    runs: &[Run],
    default_size_known: bool,
    bound: Option<u64>,
) -> Result<Option<u64>, Error> {
    let mut cursor = base;
    let mut known = true;
    for run in runs {
        let start = run.data_start.or(cursor);
        let bytes = run_bytes(payload, run, default_size_known)?;
        cursor = match (start, bytes) {
            (Some(start), Some(bytes)) => {
                let end = start
                    .checked_add(bytes)
                    .ok_or(Error::Overflow { offset: run.at })?;
                if bound.is_some_and(|file_len| end > file_len) {
                    return Err(Error::Truncated { offset: start });
                }
                Some(end)
            }
            _ => {
                known = false;
                None
            }
        };
    }
    Ok(cursor.filter(|_| known))
}

impl Fragment {
    /// The sample description index (1-based) of the last sample
    /// [`Fragment::next_run`] returned: the `tfhd`'s, else the `trex`'s.
    pub(crate) fn description(&self) -> u32 {
        self.description
    }

    /// Parses a `moof` payload (`payload` starts at absolute
    /// `payload_offset`; the box itself starts at `moof_offset`) and keeps the
    /// runs of `trex.track_id`. `all_trex` holds the defaults of every track,
    /// needed to find where a preceding track fragment's data ends.
    ///
    /// `file_len` bounds the runs of `trex.track_id`, and only those (this
    /// module's documentation, checks 2 and 3). A caller that cannot judge
    /// a track by this file's length — one whose media is in another file —
    /// does not read its fragments at all.
    pub(crate) fn parse(
        payload: Vec<u8>,
        moof_offset: u64,
        payload_offset: u64,
        file_len: u64,
        trex: Trex,
        all_trex: &[Trex],
        boxes: &Cell<u64>,
    ) -> Result<Self, Error> {
        let mut runs = Vec::new();
        // End of the data of the preceding track fragment (ISO/IEC 14496-12
        // 8.8.7): `None` before the first `traf`, `Some(None)` when it could
        // not be determined.
        let mut previous_traf_end: Option<Option<u64>> = None;
        let data = payload.as_slice();
        children(
            data,
            payload_offset,
            boxes,
            |kind, traf, traf_offset, traf_at| {
                if kind != FourCc(*b"traf") {
                    return Ok(());
                }
                let mut defaults = None;
                let mut base = None;
                let mut tfdt = None;
                let mut traf_runs: Vec<Run> = Vec::new();
                let mut first_box = true;
                let mut size_known = false;
                children(
                    traf,
                    traf_offset,
                    boxes,
                    |kind, body, body_offset, body_at| {
                        let body_index = traf_at.saturating_add(body_at);
                        match &kind.0 {
                            b"tfhd" => {
                                if !first_box {
                                    return Err(invalid(body_offset, "tfhd is not first"));
                                }
                                let mut b = Bytes::new(body, body_offset);
                                let (_, flags) = b.version_flags()?;
                                let track_id = b.u32()?;
                                let known = all_trex.iter().find(|t| t.track_id == track_id);
                                let mut d = known.copied().unwrap_or(Trex {
                                    track_id,
                                    ..Trex::default()
                                });
                                size_known = known.is_some();
                                if flags & 0x1 != 0 {
                                    base = Some(b.u64()?);
                                } else if flags & 0x2_0000 != 0 {
                                    base = Some(moof_offset);
                                } else {
                                    // The first track fragment starts at the
                                    // moof; a later one where the preceding
                                    // track fragment's data ends.
                                    base = Some(match previous_traf_end {
                                        None => moof_offset,
                                        Some(Some(end)) => end,
                                        Some(None) => {
                                            return Err(invalid(
                                                body_offset,
                                                "implicit base offset cannot be determined",
                                            ));
                                        }
                                    });
                                }
                                if flags & 0x2 != 0 {
                                    d.description = b.u32()?;
                                }
                                if flags & 0x8 != 0 {
                                    d.duration = b.u32()?;
                                }
                                if flags & 0x10 != 0 {
                                    d.size = b.u32()?;
                                    size_known = true;
                                }
                                if flags & 0x20 != 0 {
                                    d.flags = b.u32()?;
                                }
                                defaults = Some(d);
                            }
                            b"tfdt" => {
                                let mut b = Bytes::new(body, body_offset);
                                let (version, _) = b.version_flags()?;
                                let time = if version == 1 {
                                    b.u64()?
                                } else {
                                    u64::from(b.u32()?)
                                };
                                tfdt = Some(i64::try_from(time).map_err(|_| Error::Overflow {
                                    offset: body_offset,
                                })?);
                            }
                            b"trun" => {
                                let d = defaults.ok_or(invalid(body_offset, "trun before tfhd"))?;
                                let mut b = Bytes::new(body, body_offset);
                                let (version, flags) = b.version_flags()?;
                                let count = b.u32()?;
                                let data_start = if flags & 0x1 != 0 {
                                    let relative = i64::from(b.i32()?);
                                    let start = base.ok_or(invalid(body_offset, "no base"))?;
                                    Some(start.checked_add_signed(relative).ok_or(
                                        Error::Overflow {
                                            offset: body_offset,
                                        },
                                    )?)
                                } else if traf_runs.is_empty() {
                                    base
                                } else {
                                    None
                                };
                                let first_flags = if flags & 0x4 != 0 {
                                    Some(b.u32()?)
                                } else {
                                    None
                                };
                                let stride = [0x100u32, 0x200, 0x400, 0x800]
                                    .iter()
                                    .filter(|bit| flags & **bit != 0)
                                    .count()
                                    .saturating_mul(4);
                                let needed = (count as usize).checked_mul(stride).ok_or(
                                    Error::Overflow {
                                        offset: body_offset,
                                    },
                                )?;
                                if needed > b.remaining() {
                                    return Err(invalid(
                                        body_offset,
                                        "trun entries exceed the box",
                                    ));
                                }
                                // Samples with no size at all (check 3): no
                                // per-sample sizes and a default of zero.
                                // Such a run carries no data, so nothing
                                // bounds what it declares, and the file it
                                // describes has no media in it.
                                if d.track_id == trex.track_id
                                    && count > 0
                                    && flags & 0x200 == 0
                                    && d.size == 0
                                {
                                    return Err(invalid(body_offset, "trun samples have no size"));
                                }
                                let entries_at = body_index
                                    .saturating_add(body.len().saturating_sub(b.remaining()));
                                traf_runs.push(Run {
                                    at: body_offset,
                                    entries_at,
                                    count,
                                    stride,
                                    flags,
                                    version,
                                    first_flags,
                                    data_start,
                                    tfdt: None,
                                    defaults: d,
                                });
                            }
                            _ => {}
                        }
                        first_box = false;
                        Ok(())
                    },
                )?;
                let Some(d) = defaults else {
                    return Err(invalid(traf_offset, "traf without tfhd"));
                };
                // Only the runs of the track being read are bounded by the
                // file's length (this module's documentation).
                let bound = (d.track_id == trex.track_id).then_some(file_len);
                previous_traf_end = Some(traf_data_end(data, base, &traf_runs, size_known, bound)?);
                if d.track_id == trex.track_id {
                    if let Some(first) = traf_runs.first_mut() {
                        first.tfdt = tfdt;
                    }
                    runs.extend(traf_runs);
                }
                Ok(())
            },
        )?;
        Ok(Self {
            payload,
            runs,
            run: 0,
            index: 0,
            description: 0,
        })
    }

    /// Decode time of the first sample, from the track fragment's `tfdt`.
    pub(crate) fn base_decode_time(&self) -> Option<i64> {
        self.runs.first().and_then(|run| run.tfdt)
    }

    /// Sum of the sample durations of the selected track in this fragment.
    pub(crate) fn duration(&self, work: &mut Work) -> Result<(u64, u64, Option<i64>), Error> {
        let mut total = 0u64;
        let mut samples = 0u64;
        let mut start = None;
        for run in &self.runs {
            if start.is_none() {
                start = run.tfdt;
            }
            samples = samples.saturating_add(u64::from(run.count));
            if run.flags & 0x100 == 0 {
                total = u64::from(run.count)
                    .checked_mul(u64::from(run.defaults.duration))
                    .and_then(|d| total.checked_add(d))
                    .ok_or(Error::Overflow { offset: 0 })?;
                continue;
            }
            work.add(u64::from(run.count))?;
            for i in 0..run.count {
                let at = run
                    .entries_at
                    .saturating_add((i as usize).saturating_mul(run.stride));
                let value = be_u32_at(&self.payload, at).ok_or(Error::Truncated { offset: 0 })?;
                total = total
                    .checked_add(u64::from(value))
                    .ok_or(Error::Overflow { offset: 0 })?;
            }
        }
        Ok((total, samples, start))
    }

    /// Values of sample `index` of `run`: (duration, size, flags, composition offset).
    fn sample_fields(&self, run: &Run, index: u32) -> Result<(u32, u32, u32, i64), Error> {
        let mut at = run
            .entries_at
            .saturating_add((index as usize).saturating_mul(run.stride));
        let mut field = |bit: u32| -> Result<Option<u32>, Error> {
            if run.flags & bit == 0 {
                return Ok(None);
            }
            let value = be_u32_at(&self.payload, at).ok_or(Error::Truncated { offset: 0 })?;
            at = at.saturating_add(4);
            Ok(Some(value))
        };
        let duration = field(0x100)?.unwrap_or(run.defaults.duration);
        let size = field(0x200)?.unwrap_or(run.defaults.size);
        let flags = match field(0x400)? {
            Some(flags) => flags,
            None if index == 0 => run.first_flags.unwrap_or(run.defaults.flags),
            None => run.defaults.flags,
        };
        let composition_offset = match field(0x800)? {
            Some(raw) if run.version == 1 => i64::from(raw as i32),
            Some(raw) => i64::from(raw),
            None => 0,
        };
        Ok((duration, size, flags, composition_offset))
    }

    /// Next run of at most `max` samples (at least one) with equal size,
    /// duration, flags and composition offset within one `trun`; `position`
    /// and `dts` carry the running data position and decode time of the track
    /// across runs and fragments.
    pub(crate) fn next_run(
        &mut self,
        position: &mut u64,
        dts: &mut i64,
        work: &mut Work,
        max: u32,
    ) -> Result<Option<RawSample>, Error> {
        loop {
            let Some(run) = self.runs.get(self.run).cloned() else {
                return Ok(None);
            };
            if self.index >= run.count {
                self.run = self.run.saturating_add(1);
                self.index = 0;
                continue;
            }
            if self.index == 0 {
                if let Some(start) = run.data_start {
                    *position = start;
                }
                if let Some(time) = run.tfdt {
                    *dts = time;
                }
            }
            work.add(1)?;
            let first = self.sample_fields(&run, self.index)?;
            let limit = max.max(1).min(run.count - self.index);
            let count = if run.stride == 0 {
                // No per-sample fields: every sample equals the defaults, except
                // that the first may carry its own flags. Computed, not scanned,
                // so a run declaring billions of samples costs one step.
                let first_differs = self.index == 0
                    && run
                        .first_flags
                        .is_some_and(|flags| flags != run.defaults.flags);
                if first_differs { 1 } else { limit }
            } else {
                // Per-sample fields: compare entries, each one counted as work.
                let mut count = 1;
                while count < limit && self.sample_fields(&run, self.index + count)? == first {
                    count += 1;
                }
                work.add(u64::from(count - 1))?;
                count
            };
            let (duration, size, flags, composition_offset) = first;
            let offset = *position;
            let bytes = u64::from(size)
                .checked_mul(u64::from(count))
                .ok_or(Error::Overflow { offset })?;
            *position = offset
                .checked_add(bytes)
                .ok_or(Error::Overflow { offset })?;
            let ticks = i64::from(duration)
                .checked_mul(i64::from(count))
                .ok_or(Error::Overflow { offset })?;
            let sample_dts = *dts;
            *dts = dts.checked_add(ticks).ok_or(Error::Overflow { offset })?;
            self.index += count;
            self.description = run.defaults.description;
            return Ok(Some(RawSample {
                offset,
                size,
                dts: sample_dts,
                composition_offset,
                duration,
                keyframe: flags & NON_SYNC == 0,
                count,
            }));
        }
    }
}

/// Track defaults from `trex` (ISO/IEC 14496-12 8.8.3), as listing one
/// movie fragment needs them when the movie box is not at hand: the HLS
/// index built from a segment index records them and reads a fragment per
/// request (`docs/design/hls.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackDefaults {
    /// The track the defaults belong to.
    pub track_id: u32,
    /// `default_sample_duration`.
    pub duration: u32,
    /// `default_sample_size`.
    pub size: u32,
    /// `default_sample_flags`.
    pub flags: u32,
}

/// One track's samples in one movie fragment.
#[derive(Clone, Debug)]
pub struct FragmentSamples {
    /// Decode time of the first sample, from the track fragment's `tfdt`;
    /// `None` without one, when only the fragments before it say where the
    /// track's decode time stands.
    pub base_decode_time: Option<i64>,
    /// The runs, in decode order.
    pub runs: Vec<SampleRun>,
}

/// Lists the samples of `defaults.track_id` in the subsegment at `at` of
/// `len` bytes, without a movie box: every movie fragment it holds, in
/// order, skipping the boxes between them (`styp`, `emsg`, a segment
/// index of its own). A subsegment is one or more fragments with their
/// media data (ISO/IEC 14496-12 8.16.1). For a caller that recorded where
/// each subsegment lies.
///
/// `file_len` bounds every sample's byte range, `limits` each box, each
/// sample and the number of fragments, and the work is charged to
/// `budget`.
pub fn subsegment_samples<S: Source>(
    source: &mut S,
    at: u64,
    len: u64,
    file_len: u64,
    defaults: TrackDefaults,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<FragmentSamples, Error> {
    let end = at.checked_add(len).ok_or(Error::Overflow { offset: at })?;
    if end > file_len {
        return Err(Error::Truncated { offset: at });
    }
    let trex = Trex {
        track_id: defaults.track_id,
        description: 1,
        duration: defaults.duration,
        size: defaults.size,
        flags: defaults.flags,
    };
    // The runs are held, so they count against the table limit before
    // they are built, as every other table of this crate does.
    let max_runs = usize::try_from(limits.max_table_bytes / RUN_BYTES).unwrap_or(usize::MAX);
    let mut out = FragmentSamples {
        base_decode_time: None,
        runs: Vec::new(),
    };
    let mut work = Work::new(limits);
    let (mut position, mut dts, mut sample) = (0u64, 0i64, 0u64);
    let (mut next, mut fragments) = (at, 0u32);
    while next < end {
        let header = read_header(source, next, end)?;
        work.add(1)?;
        next = header.end();
        if header.kind != FourCc(*b"moof") {
            continue;
        }
        fragments = fragments.saturating_add(1);
        if fragments > limits.max_fragments {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Fragments,
                offset: header.offset,
            });
        }
        let payload = read_payload(source, &header, limits.max_box_bytes, LimitKind::BoxBytes)?;
        let boxes = Cell::new(0u64);
        let mut fragment = Fragment::parse(
            payload,
            header.offset,
            header.payload_offset(),
            file_len,
            trex,
            core::slice::from_ref(&trex),
            &boxes,
        )?;
        work.add(boxes.get())?;
        if out.base_decode_time.is_none() && out.runs.is_empty() {
            out.base_decode_time = fragment.base_decode_time();
        }
        while let Some(raw) = fragment.next_run(&mut position, &mut dts, &mut work, u32::MAX)? {
            if out.runs.len() >= max_runs {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::TableBytes,
                    offset: header.offset,
                });
            }
            out.runs.push(crate::packets::checked_run(
                raw,
                limits.max_packet_bytes,
                file_len,
                &mut sample,
            )?);
        }
    }
    budget.charge(work.used)?;
    if fragments == 0 {
        return Err(Error::InvalidBox {
            offset: at,
            kind: FourCc(*b"moof"),
            reason: "a subsegment without a movie fragment",
        });
    }
    Ok(out)
}
