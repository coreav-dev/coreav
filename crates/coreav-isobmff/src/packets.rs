//! Packet iteration: one packet per sample; samples are never merged or split.

use crate::error::LimitKind;
use crate::fragment::Fragment;
use crate::header::{read_header, read_payload};
use crate::movie::{EditList, Movie, Track, TrackKind};
use crate::tables::{Bounds, Cursor, Position, RawSample, Work};
use crate::{Error, FourCc, Limits, Rational, Source};
use core::cell::Cell;

/// Work budget shared by all packet iterations of one request.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    work: Work,
    /// Samples listed to advise a remote source of header regions, over
    /// every track of the request (`Movie::scan_frame_sizes`): advice is
    /// given only while this stays under its own small cap, apart from
    /// `work`, so that it never changes what the scan itself may read.
    pub(crate) advised: usize,
}

impl Budget {
    /// A fresh budget of `limits.max_sample_work` visits.
    pub fn new(limits: &Limits) -> Self {
        Self {
            work: Work::new(limits),
            advised: 0,
        }
    }

    /// Visits used so far.
    pub fn used(&self) -> u64 {
        self.work.used
    }

    /// Counts `units` more visits, failing when the limit is passed.
    pub(crate) fn charge(&mut self, units: u64) -> Result<(), Error> {
        self.work.add(units)
    }
}

/// One sample of a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Zero-based sample number in the track, fragments included.
    pub sample: u64,
    /// Absolute byte offset of the sample data.
    pub offset: u64,
    /// Size in bytes.
    pub size: u32,
    /// Decode time in media ticks, as stored.
    pub dts: i64,
    /// Composition time in media ticks: decode time plus the stored
    /// composition offset, never shifted.
    pub cts: i64,
    /// Decode duration in media ticks.
    pub duration: u32,
    /// Sync sample.
    pub keyframe: bool,
    /// Presentation time after the edit list, `None` when the edit list is
    /// unsupported.
    pub pts: Option<Rational>,
    /// Decode time on the presentation timeline, `None` when the edit list is
    /// unsupported.
    pub presentation_dts: Option<Rational>,
    /// Whether the edit list presents any part of the sample.
    pub presented: bool,
}

/// Consecutive samples of one track that lie back to back in one chunk or one
/// fragment run and share size, decode duration, composition offset and sync
/// flag. A run is described once and expanded only on demand,
/// so constant-size tracks such as PCM audio are indexed without visiting
/// every sample. Each sample is still one packet.
///
/// For a QuickTime compressed audio track whose sample tables count single
/// audio samples (`stsz` of 1, [`crate::SampleDescription::sound_packet`]), offsets
/// and sizes are in the tables' units, one byte a sample from the chunk's
/// offset, and need not lie inside the file: the bytes are the compressed
/// frames from the chunk's offset, which the iteration checks are inside
/// the file (`docs/design/sample-tables.md`). The same holds for such a
/// track's [`Packet`]s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleRun {
    /// Zero-based number of the first sample in the track.
    pub first_sample: u64,
    /// Number of samples, at least 1.
    pub count: u32,
    /// Absolute byte offset of the first sample.
    pub offset: u64,
    /// Size of each sample in bytes.
    pub sample_size: u32,
    /// Decode time of the first sample in media ticks, as stored.
    pub dts: i64,
    /// Decode duration of each sample in media ticks.
    pub sample_duration: u32,
    /// Composition offset of each sample, as stored.
    pub composition_offset: i64,
    /// Whether every sample of the run is a sync sample.
    pub keyframe: bool,
}

impl SampleRun {
    /// Byte offset of sample `i` of the run, `None` past the run.
    pub fn sample_offset(&self, i: u32) -> Option<u64> {
        if i >= self.count {
            return None;
        }
        u64::from(self.sample_size)
            .checked_mul(u64::from(i))
            .and_then(|delta| self.offset.checked_add(delta))
    }

    /// Decode time of sample `i` of the run, `None` past the run.
    pub fn sample_dts(&self, i: u32) -> Option<i64> {
        if i >= self.count {
            return None;
        }
        i64::from(self.sample_duration)
            .checked_mul(i64::from(i))
            .and_then(|delta| self.dts.checked_add(delta))
    }

    /// Total bytes of the run.
    pub fn byte_len(&self) -> u64 {
        u64::from(self.sample_size).saturating_mul(u64::from(self.count))
    }
}

/// One sample of a track as the sample tables store it: a [`Packet`]
/// without the presentation times an edit list gives it, and a
/// [`SampleRun`] of one sample without its count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredSample {
    /// Zero-based sample number in the track, fragments included.
    pub sample: u64,
    /// Absolute byte offset of the sample data.
    pub offset: u64,
    /// Size in bytes.
    pub size: u32,
    /// Decode time in media ticks, as stored.
    pub dts: i64,
    /// Decode duration in media ticks.
    pub duration: u32,
    /// Composition offset, as stored.
    pub composition_offset: i64,
    /// Sync sample.
    pub keyframe: bool,
}

/// Iterator over the packets of one track.
pub struct Packets<'a, S> {
    track: &'a Track,
    movie: &'a Movie,
    source: S,
    limits: Limits,
    budget: &'a mut Budget,
    cursor: Option<Cursor>,
    fragment: Option<Fragment>,
    next_box: Option<u64>,
    fragments_seen: u32,
    position: u64,
    dts: i64,
    sample: u64,
    timeline: Timeline,
    window: Window,
    /// The media edits of an [`EditList::Multiple`] list, in media order.
    spans: Vec<Span>,
    /// The end the byte ranges of the sample tables are checked against:
    /// the file's length, except for a QuickTime compressed audio track
    /// whose samples are single audio samples (`sound_packet` with an
    /// `stsz` of 1). The tables count one byte a sample there, which does
    /// not say where the bytes are; they are the compressed frames from
    /// each chunk's offset, all checked inside the file when the iteration
    /// is made (`SampleTables::frames_inside`), and the tables' own
    /// ranges are then not checked (`u64::MAX`).
    table_end: u64,
    failed: bool,
}

impl Movie {
    /// Lists the packets of track `index`. Tracks with external data or
    /// invalid sample tables have no packets.
    pub fn packets<'a, S: Source>(
        &'a self,
        index: usize,
        source: S,
        limits: &Limits,
        budget: &'a mut Budget,
    ) -> Result<Packets<'a, S>, Error> {
        let track = self.tracks.get(index).ok_or(Error::NoSuchTrack)?;
        let usable = track.tables_valid && !track.external_data;
        let (mut timeline, window) = Timeline::for_track(track, self.timescale);
        let spans = if timeline == Timeline::Spans {
            Span::for_track(track, self.timescale).unwrap_or_else(|| {
                timeline = Timeline::Unknown;
                Vec::new()
            })
        } else {
            Vec::new()
        };
        // The windows are a copy of the stored entries, which probe charged
        // to the retained bytes; building them is charged here, one visit
        // each (docs/design/edit-list-writers.md, common decision 2).
        budget.charge(u64::try_from(spans.len()).unwrap_or(u64::MAX))?;
        let framed = match track.description.sound_packet {
            // One frame size for every chunk: only with one sample description.
            Some(frame) if usable && track.trex.is_none() && track.descriptions == 1 => {
                track.tables.frames_inside(
                    frame,
                    self.file_len,
                    limits.max_packet_bytes,
                    &mut budget.work,
                )?
            }
            _ => false,
        };
        Ok(Packets {
            track,
            movie: self,
            source,
            limits: *limits,
            budget,
            cursor: usable.then(|| Cursor::new(&track.tables)),
            fragment: None,
            next_box: if usable && track.trex.is_some() {
                self.first_fragment
            } else {
                None
            },
            fragments_seen: 0,
            position: 0,
            dts: 0,
            sample: 0,
            timeline,
            window,
            spans,
            table_end: if framed { u64::MAX } else { self.file_len },
            failed: false,
        })
    }
}

impl Movie {
    /// Lists the packets of track `index` from sample `start` (0-based):
    /// exactly what [`packets`](Self::packets) lists after skipping `start`
    /// packets, nothing when that listing would stop with an error before
    /// sample `start`. The position in the sample tables is computed by
    /// arithmetic on their run-length entries, and the samples before it
    /// are checked chunk by chunk as iteration checks them; samples of
    /// movie fragments before it are skipped run by run
    /// (`docs/design/seek.md`). The work budget is charged for the table
    /// entries visited, which is less than listing the samples charges.
    pub fn packets_from<'a, S: Source>(
        &'a self,
        index: usize,
        start: u64,
        source: S,
        limits: &Limits,
        budget: &'a mut Budget,
    ) -> Result<Packets<'a, S>, Error> {
        let mut packets = self.packets(index, source, limits, budget)?;
        packets.skip_to(start)?;
        Ok(packets)
    }
}

impl<S: Source> Packets<'_, S> {
    /// Moves a fresh iteration to sample `start` for
    /// [`Movie::packets_from`].
    fn skip_to(&mut self, start: u64) -> Result<(), Error> {
        let mut left = start;
        if self.cursor.is_some() {
            let tables = &self.track.tables;
            let in_tables = u32::try_from(start)
                .unwrap_or(u32::MAX)
                .min(tables.sample_count());
            match Cursor::at(
                tables,
                in_tables,
                self.table_end,
                self.limits.max_packet_bytes,
                &mut self.budget.work,
            )? {
                // A composition time can overflow only within one 32-bit
                // offset of the largest decode time; there the samples are
                // listed one by one, so the overflow stops it in place.
                Position::At(cursor) if cursor.dts() > i64::MAX - (1 << 32) => {
                    for _ in 0..in_tables {
                        match self.next() {
                            Some(Ok(_)) => {}
                            _ => {
                                self.failed = true;
                                return Ok(());
                            }
                        }
                    }
                }
                Position::At(cursor) => {
                    self.cursor = Some(cursor);
                    self.sample = u64::from(in_tables);
                }
                Position::Stopped => {
                    self.failed = true;
                    return Ok(());
                }
            }
            left = start - u64::from(in_tables);
        }
        // Samples of movie fragments, run by run with each run's checks.
        while left > 0 {
            let max = u32::try_from(left).unwrap_or(u32::MAX);
            let run = match self.next_raw(max) {
                // Each sample's composition time is checked as a packet's is.
                Ok(Some(raw)) if !composition_fits(&raw) => {
                    Err(Error::Overflow { offset: raw.offset })
                }
                Ok(Some(raw)) => self.run(raw),
                Ok(None) => return Ok(()),
                Err(error) => Err(error),
            };
            match run {
                Ok(run) => left = left.saturating_sub(u64::from(run.count)),
                Err(_) => {
                    self.failed = true;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Next run of at most `max` samples from the tables, then fragments.
    #[inline]
    fn next_raw(&mut self, max: u32) -> Result<Option<RawSample>, Error> {
        if let Some(cursor) = self.cursor.as_mut() {
            self.budget.work.add(1)?;
            if let Some(sample) = cursor.next_run(&self.track.tables, max, &mut self.budget.work)? {
                return Ok(Some(sample));
            }
            self.dts = cursor.dts();
            self.cursor = None;
        }
        loop {
            if let Some(fragment) = self.fragment.as_mut() {
                if let Some(sample) = fragment.next_run(
                    &mut self.position,
                    &mut self.dts,
                    &mut self.budget.work,
                    max,
                )? {
                    // Every sample of a subtitle track is a sync sample,
                    // whatever a fragment's flags say: `ffprobe` keys each
                    // one where a muxer marks empty text samples non-sync
                    // (`fragmented_text.mp4`).
                    let keyframe = sample.keyframe || self.track.kind == TrackKind::Subtitle;
                    return Ok(Some(RawSample { keyframe, ..sample }));
                }
                self.fragment = None;
            }
            let Some(at) = self.next_box else {
                return Ok(None);
            };
            let len = self.movie.file_len;
            if at >= len {
                self.next_box = None;
                return Ok(None);
            }
            let header = read_header(&mut self.source, at, len)?;
            self.budget.work.add(1)?;
            self.next_box = Some(header.end());
            if header.kind != FourCc(*b"moof") {
                continue;
            }
            self.fragments_seen = self.fragments_seen.saturating_add(1);
            if self.fragments_seen > self.limits.max_fragments {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::Fragments,
                    offset: at,
                });
            }
            let payload = read_payload(
                &mut self.source,
                &header,
                self.limits.max_box_bytes,
                LimitKind::BoxBytes,
            )?;
            let Some(trex) = self.track.trex else {
                return Ok(None);
            };
            let boxes = Cell::new(0u64);
            let fragment = Fragment::parse(
                payload,
                header.offset,
                header.payload_offset(),
                len,
                trex,
                &self.movie.all_trex,
                &boxes,
            )?;
            self.budget.work.add(boxes.get())?;
            self.fragment = Some(fragment);
        }
    }

    /// Charges `units` visits to this iteration's budget, for per-sample
    /// work the caller does itself: a caller that takes maximal runs
    /// ([`next_runs`](Self::next_runs)) and expands every sample charges
    /// one visit each, as iterating packet by packet does.
    pub fn charge(&mut self, units: u64) -> Result<(), Error> {
        self.budget.charge(units)
    }

    /// The sample description index (1-based, ISO/IEC 14496-12 8.8.7) of
    /// the last sample returned when it came from a movie fragment: the
    /// track fragment header's, else the track extends box's. `None` for a
    /// sample of the movie box's tables, or before any sample.
    pub fn fragment_description(&self) -> Option<u32> {
        if self.cursor.is_some() {
            return None;
        }
        self.fragment
            .as_ref()
            .map(Fragment::description)
            .filter(|d| *d > 0)
    }

    /// The sample description index (1-based, ISO/IEC 14496-12 8.7.4) of
    /// the last sample returned: the one the sample to chunk box gives its
    /// chunk, or for a sample of a movie fragment
    /// [`fragment_description`](Self::fragment_description). `None` for a
    /// fragment that states none.
    pub fn description(&self) -> Option<u32> {
        match &self.cursor {
            Some(cursor) => cursor.description(&self.track.tables),
            None => self.fragment_description(),
        }
    }

    /// The source the packets are read from, so a caller can read each
    /// packet's bytes between calls without opening a second source
    /// (the M4A writer copies samples while iterating). Reads
    /// through it do not move the iterator.
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    /// Next run of at most `max` samples, or `None` after the
    /// last sample. Runs and single packets ([`Iterator::next`]) can be mixed;
    /// both advance the same position. Each run's byte range is checked
    /// against the file and `max_packet_bytes` applies to each sample.
    #[inline]
    pub fn next_run(&mut self, max: u32) -> Option<Result<SampleRun, Error>> {
        if self.failed {
            return None;
        }
        let result = match self.next_raw(max) {
            Ok(Some(raw)) => self.run(raw),
            Ok(None) => return None,
            Err(error) => Err(error),
        };
        if result.is_err() {
            self.failed = true;
        }
        Some(result)
    }

    /// Appends up to `limit` runs of at most `max` samples each to `out`,
    /// the same runs [`next_run`](Self::next_run) returns one by one, and
    /// returns how many it appended: 0 after the last sample. For callers
    /// that index every sample: runs from the sample tables are read
    /// in one loop, without the per-run calls, while every check of
    /// `next_run` still applies to each run; runs of movie fragments come
    /// one per call of `next_run`. After an error, every later call returns
    /// `Ok(0)`, as `next_run` returns `None`.
    pub fn next_runs(
        &mut self,
        max: u32,
        out: &mut Vec<SampleRun>,
        limit: usize,
    ) -> Result<usize, Error> {
        if self.failed || limit == 0 {
            return Ok(0);
        }
        let start = out.len();
        if let Some(mut cursor) = self.cursor.take() {
            let max_packet_bytes = self.limits.max_packet_bytes;
            let file_len = self.table_end;
            let sample = &mut self.sample;
            let result = cursor.fill(
                &self.track.tables,
                max,
                &mut self.budget.work,
                limit,
                |raw| {
                    out.push(checked_run(raw, max_packet_bytes, file_len, sample)?);
                    Ok(())
                },
            );
            match result {
                Err(error) => {
                    self.failed = true;
                    return Err(error);
                }
                Ok(taken) if taken == limit => {
                    self.cursor = Some(cursor);
                    return Ok(taken);
                }
                Ok(taken) => {
                    // The samples of the tables ended.
                    self.dts = cursor.dts();
                    if taken > 0 {
                        return Ok(taken);
                    }
                }
            }
        }
        while out.len().saturating_sub(start) < limit {
            match self.next_run(max) {
                Some(Ok(run)) => out.push(run),
                Some(Err(error)) => return Err(error),
                None => break,
            }
        }
        Ok(out.len().saturating_sub(start))
    }

    /// Visits every remaining sample of the track in decode order, each as
    /// the tables store it, and returns how many it visited. For a caller
    /// that indexes every sample: the samples come without one
    /// record per sample in memory, so the walk and the caller's own record
    /// are one loop, and the sizes of a chunk are read in one pass of the
    /// table. Every check of [`next_run`](Self::next_run) applies to each
    /// sample.
    ///
    /// One visit per sample is charged. The samples of the sample tables
    /// are charged as iterating packet by packet charges them, one before
    /// each sample is read. A movie fragment is taken a run at a time and
    /// its samples are charged before any of them is expanded, so a run
    /// that declares more samples than the budget allows costs the budget
    /// and not the loop; a budget that runs out
    /// therefore stops such a track at a run boundary rather than at a
    /// sample, and the run's own step is charged beside its samples.
    ///
    /// After it returns the iteration is at the end; after an error every
    /// later call returns `Ok(0)`, as [`next_run`](Self::next_run) returns
    /// `None`.
    pub fn for_each_sample<E: From<Error>>(
        &mut self,
        mut visit: impl FnMut(StoredSample) -> Result<(), E>,
    ) -> Result<u64, E> {
        if self.failed {
            return Ok(0);
        }
        let before = self.sample;
        let mut seen = 0u64;
        if let Some(mut cursor) = self.cursor.take() {
            let bounds = Bounds {
                max_packet_bytes: self.limits.max_packet_bytes,
                file_len: self.table_end,
            };
            let result = cursor.visit(
                &self.track.tables,
                bounds,
                &mut self.budget.work,
                &mut self.sample,
                &mut visit,
            );
            seen = self.sample.saturating_sub(before);
            if let Err(error) = result {
                self.failed = true;
                return Err(error);
            }
            // The samples of the tables ended.
            self.dts = cursor.dts();
        }
        // Fragments: a whole run at a time, expanded here. Every sample
        // of the run is visited, so every sample is charged before any of
        // them is — a run that declares billions of samples costs one
        // step and the budget, not the loop.
        loop {
            match self.next_run(u32::MAX) {
                Some(Ok(run)) => {
                    if let Err(error) = self.charge(u64::from(run.count)) {
                        self.failed = true;
                        return Err(error.into());
                    }
                    for i in 0..run.count {
                        let (Some(offset), Some(dts)) = (run.sample_offset(i), run.sample_dts(i))
                        else {
                            self.failed = true;
                            return Err(Error::Overflow { offset: run.offset }.into());
                        };
                        let sample = StoredSample {
                            sample: run.first_sample.saturating_add(u64::from(i)),
                            offset,
                            size: run.sample_size,
                            dts,
                            duration: run.sample_duration,
                            composition_offset: run.composition_offset,
                            keyframe: run.keyframe,
                        };
                        if let Err(error) = visit(sample) {
                            // The caller stopped the walk: later calls
                            // return `Ok(0)`, as they do after an error of
                            // our own.
                            self.failed = true;
                            return Err(error);
                        }
                    }
                    seen = seen.saturating_add(u64::from(run.count));
                }
                Some(Err(error)) => {
                    self.failed = true;
                    return Err(error.into());
                }
                None => break,
            }
        }
        Ok(seen)
    }

    #[inline]
    fn run(&mut self, raw: RawSample) -> Result<SampleRun, Error> {
        checked_run(
            raw,
            self.limits.max_packet_bytes,
            self.table_end,
            &mut self.sample,
        )
    }

    fn packet(&mut self, raw: RawSample) -> Result<Packet, Error> {
        let offset = raw.offset;
        if u64::from(raw.size) > self.limits.max_packet_bytes {
            return Err(Error::LimitExceeded {
                limit: LimitKind::PacketBytes,
                offset,
            });
        }
        let end = offset
            .checked_add(u64::from(raw.size))
            .ok_or(Error::Overflow { offset })?;
        if end > self.table_end {
            return Err(Error::Truncated { offset });
        }
        let cts = raw
            .dts
            .checked_add(raw.composition_offset)
            .ok_or(Error::Overflow { offset })?;
        let (pts, presentation_dts, presented) = if self.timeline == Timeline::Spans {
            match Span::owner(&self.spans, cts, raw.duration) {
                Some((span, presented)) => (span.map(cts), span.map(raw.dts), presented),
                None => (None, None, true),
            }
        } else {
            (
                self.timeline.map(cts),
                self.timeline.map(raw.dts),
                self.window.presents(cts, raw.duration),
            )
        };
        let sample = self.sample;
        self.sample = self.sample.saturating_add(1);
        Ok(Packet {
            sample,
            offset,
            size: raw.size,
            dts: raw.dts,
            cts,
            duration: raw.duration,
            keyframe: raw.keyframe,
            pts,
            presentation_dts,
            presented,
        })
    }
}

impl<S> Packets<'_, S> {
    /// The source and the work budget at once, for walks that read through
    /// one and charge the other.
    pub(crate) fn parts_mut(&mut self) -> (&mut S, &mut Budget) {
        (&mut self.source, self.budget)
    }
}

impl<S: Source> Iterator for Packets<'_, S> {
    type Item = Result<Packet, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let result = match self.next_raw(1) {
            Ok(Some(raw)) => self.packet(raw),
            Ok(None) => return None,
            Err(error) => Err(error),
        };
        if result.is_err() {
            self.failed = true;
        }
        Some(result)
    }
}

/// Media-to-presentation mapping of one track under the simple edit model
/// (at most one empty edit, then one media edit at rate 1.0), precomputed
/// once per iteration. Whole-tick shifts use
/// integer arithmetic; a fractional empty-edit offset keeps exact rationals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timeline {
    /// No edit list: media time is presentation time.
    Identity,
    /// `presentation = media + shift`, in whole ticks.
    Shift(i128),
    /// `presentation = media - media_time + offset`, offset fractional.
    Exact { media_time: i64, offset: Rational },
    /// Several edits (`EditList::Multiple`): each sample is mapped by its
    /// media edit, a [`Span`].
    Spans,
    /// Unsupported edit list: presentation times are unknown.
    Unknown,
}

/// One media edit of an [`EditList::Multiple`] list
/// (`docs/design/edit-lists.md`, decision 2).
#[derive(Clone, Copy, Debug)]
struct Span {
    /// First media time presented, in media ticks.
    start: i64,
    /// End of the window, in media ticks (exact).
    end: Rational,
    /// Where the media edit starts on the presentation timeline, less its
    /// media time: `presentation = media + shift`.
    shift: Rational,
}

impl Span {
    /// The media edits of the track's list in order, each with its window
    /// and shift; `None` when a value does not fit.
    fn for_track(track: &Track, movie_timescale: u32) -> Option<Vec<Self>> {
        let ticks = |movie_ticks: u64| {
            let num = i128::from(movie_ticks).checked_mul(i128::from(track.timescale))?;
            Rational::new(num, u64::from(movie_timescale))
        };
        let mut spans = Vec::new();
        // Where the current entry starts, in movie ticks.
        let mut at = 0u64;
        for edit in &track.edits {
            if edit.media_time != -1 {
                let start = Rational::from_int(i128::from(edit.media_time));
                let presented_at = ticks(at)?;
                spans.push(Self {
                    start: edit.media_time,
                    end: start.checked_add(ticks(edit.segment_duration)?)?,
                    shift: presented_at.checked_add(Rational::new(-start.num, 1)?)?,
                });
            }
            at = at.checked_add(edit.segment_duration)?;
        }
        Some(spans)
    }

    /// The media edit that maps the sample whose composition interval is
    /// `[cts, cts + duration)`, and whether any media edit presents it: the
    /// edit whose window holds `cts`; else the first window the interval
    /// overlaps; else, for a sample no window shows, the next media edit
    /// after it (the last one after all of them). `None` without any.
    fn owner(spans: &[Self], cts: i64, duration: u32) -> Option<(Self, bool)> {
        let at = Rational::from_int(i128::from(cts));
        let after = spans.partition_point(|s| s.start <= cts);
        if let Some(span) = after.checked_sub(1).and_then(|i| spans.get(i)) {
            if at.checked_lt(span.end) == Some(true) {
                return Some((*span, true));
            }
        }
        match spans.get(after) {
            Some(next) => {
                let last = i128::from(cts) + i128::from(duration);
                Some((*next, duration > 0 && i128::from(next.start) < last))
            }
            None => spans.last().map(|span| (*span, false)),
        }
    }

    fn map(self, media: i64) -> Option<Rational> {
        Rational::from_int(i128::from(media)).checked_add(self.shift)
    }
}

/// The media interval a simple edit presents, `[start, end)` in media ticks.
#[derive(Clone, Copy, Debug)]
enum Window {
    /// Everything is presented (no edit list or an unsupported one).
    All,
    Ticks {
        start: i128,
        end: i128,
    },
    Exact {
        start: Rational,
        end: Rational,
    },
}

impl Timeline {
    fn for_track(track: &Track, movie_timescale: u32) -> (Self, Window) {
        match track.edit_list {
            EditList::None => (Self::Identity, Window::All),
            EditList::Unsupported => (Self::Unknown, Window::All),
            EditList::Multiple => (Self::Spans, Window::All),
            EditList::Simple {
                media_time,
                segment: 0,
                ..
            } => {
                // Open-ended: presented from the media time to the end.
                let window = Window::Ticks {
                    start: i128::from(media_time),
                    end: i128::MAX,
                };
                (
                    Self::from_offset(track, movie_timescale, media_time),
                    window,
                )
            }
            EditList::Simple { media_time, .. } => {
                let start = Rational::from_int(i128::from(media_time));
                let window = track
                    .media_edit_duration(movie_timescale)
                    .and_then(|segment| start.checked_add(segment))
                    .map_or(Window::All, |end| {
                        if end.den == 1 {
                            Window::Ticks {
                                start: start.num,
                                end: end.num,
                            }
                        } else {
                            Window::Exact { start, end }
                        }
                    });
                (
                    Self::from_offset(track, movie_timescale, media_time),
                    window,
                )
            }
        }
    }

    /// Media time `media_time` presented at the track's leading empty edit.
    fn from_offset(track: &Track, movie_timescale: u32, media_time: i64) -> Self {
        match track.presentation_offset(movie_timescale) {
            Some(offset) if offset.den == 1 => offset
                .num
                .checked_sub(i128::from(media_time))
                .map_or(Self::Unknown, Self::Shift),
            Some(offset) => Self::Exact { media_time, offset },
            None => Self::Unknown,
        }
    }

    fn map(self, media: i64) -> Option<Rational> {
        match self {
            Self::Identity => Some(Rational::from_int(i128::from(media))),
            Self::Shift(shift) => i128::from(media).checked_add(shift).map(Rational::from_int),
            Self::Exact { media_time, offset } => {
                let shifted = i128::from(media).checked_sub(i128::from(media_time))?;
                Rational::from_int(shifted).checked_add(offset)
            }
            Self::Spans | Self::Unknown => None,
        }
    }
}

impl Window {
    /// Whether the composition interval `[cts, cts + duration)` overlaps the
    /// window; a zero-duration sample is presented when `start <= cts < end`.
    fn presents(self, cts: i64, duration: u32) -> bool {
        let first = i128::from(cts);
        let last = first + i128::from(duration);
        match self {
            Self::All => true,
            Self::Ticks { start, end } => {
                if duration > 0 {
                    first < end && start < last
                } else {
                    start <= first && first < end
                }
            }
            Self::Exact { start, end } => {
                let (first, last) = (Rational::from_int(first), Rational::from_int(last));
                let before_end = first.checked_lt(end).unwrap_or(false);
                if duration > 0 {
                    before_end && start.checked_lt(last).unwrap_or(false)
                } else {
                    before_end && !first.checked_lt(start).unwrap_or(true)
                }
            }
        }
    }
}

/// Whether every sample of `raw` has a composition time (decode time plus
/// offset) that does not overflow, as [`Packets::next`] requires.
fn composition_fits(raw: &RawSample) -> bool {
    let last = i64::from(raw.duration)
        .checked_mul(i64::from(raw.count.saturating_sub(1)))
        .and_then(|span| raw.dts.checked_add(span));
    [Some(raw.dts), last].into_iter().all(|dts| {
        dts.and_then(|d| d.checked_add(raw.composition_offset))
            .is_some()
    })
}

/// A run of samples from the tables, checked as every run is: each sample
/// within `max_packet_bytes`, the run's bytes inside the file. Counts its
/// samples into `sample`.
pub(crate) fn checked_run(
    raw: RawSample,
    max_packet_bytes: u64,
    file_len: u64,
    sample: &mut u64,
) -> Result<SampleRun, Error> {
    let offset = raw.offset;
    if u64::from(raw.size) > max_packet_bytes {
        return Err(Error::LimitExceeded {
            limit: LimitKind::PacketBytes,
            offset,
        });
    }
    let bytes = u64::from(raw.size)
        .checked_mul(u64::from(raw.count))
        .ok_or(Error::Overflow { offset })?;
    let end = offset
        .checked_add(bytes)
        .ok_or(Error::Overflow { offset })?;
    if end > file_len {
        return Err(Error::Truncated { offset });
    }
    let first_sample = *sample;
    *sample = sample.saturating_add(u64::from(raw.count));
    Ok(SampleRun {
        first_sample,
        count: raw.count,
        offset,
        sample_size: raw.size,
        dts: raw.dts,
        sample_duration: raw.duration,
        composition_offset: raw.composition_offset,
        keyframe: raw.keyframe,
    })
}
