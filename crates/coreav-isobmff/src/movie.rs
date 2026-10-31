//! Probing: the box tree, movie and track metadata.

use crate::bytes::Bytes;
use crate::codec::{EntryLayout, parse_sample_entry};
use crate::error::{LimitKind, Warning};
use crate::fragment::{Fragment, TrackDefaults, Trex, parse_trex};
use crate::gapless::{self, ItunesGapless};
use crate::header::{
    BoxHeader, MAX_HEADER_LEN, Parsed, parse_box_header, read_header, read_payload,
};
use crate::segment::{SegmentIndex, parse_sidx};
use crate::tables::{SampleTables, TableKind, Work, table_header};
use crate::{
    Allowlist, Codec, Error, FourCc, Limits, Rational, SampleDescription, SliceSource, Source,
};
use core::cell::Cell;

/// What a track carries, from its handler type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// `vide`.
    Video,
    /// `soun`.
    Audio,
    /// `sbtl`, `subt` or `text`.
    Subtitle,
    /// `tmcd`.
    Timecode,
    /// Anything else.
    Data,
}

/// The edit list of a track, classified by the simple edit model: at most
/// one empty edit followed by one media edit at rate 1.0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditList {
    /// No edit list: media time is presentation time.
    None,
    /// At most one empty edit followed by one media edit at rate 1.
    Simple {
        /// Duration of the leading empty edit, in movie ticks (0 if none).
        empty: u64,
        /// First media time presented, in media ticks.
        media_time: i64,
        /// Duration of the media edit, in movie ticks; 0 for an edit that
        /// lasts to the end of the media.
        segment: u64,
    },
    /// Empty edits and media edits at rate 1 whose media windows increase
    /// without overlap (`docs/design/edit-lists.md`), applied exactly from
    /// [`Track::edits`].
    Multiple,
    /// Any other shape; presentation times are unknown.
    Unsupported,
}

/// One entry of an edit list (ISO/IEC 14496-12 8.6.6), as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Duration in movie ticks.
    pub segment_duration: u64,
    /// First media time presented, in media ticks; -1 for an empty edit.
    pub media_time: i64,
    /// Rate, 16.16 fixed point as stored: `0x0001_0000` is 1, 0 a dwell.
    pub media_rate: i32,
}

impl Edit {
    /// The rate of an edit that plays the media at its own speed.
    pub const UNIT_RATE: i32 = 0x0001_0000;

    /// Bytes of [`Track::edits`] charged to `max_retained_bytes` per entry.
    pub(crate) const RETAINED_BYTES: usize = 24;

    fn is_empty_edit(&self) -> bool {
        self.media_time == -1
    }
}

/// Where a track's reported duration comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurationSource {
    /// The media header duration, capped by the media edit.
    MediaHeader,
    /// The media edit, because the media header says 0 in a fragmented movie.
    MediaEdit,
    /// The observed sample span: the sample tables of a non-fragmented track,
    /// or every fragment after [`Movie::scan_fragments`].
    Observed,
    /// Not declared, and fragments were not scanned.
    Unknown,
}

/// Why a stream was left out of the probe result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// The codec is not on the caller's [`Allowlist`].
    CodecNotAllowed,
}

/// A stream that probe left out (see [`DropReason`]). It is not in
/// [`Movie::tracks`], so none of its samples can be listed or read through
/// this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedStream {
    /// Position of the track among all tracks of the file.
    pub stream_index: usize,
    /// `track_ID` from the track header (0 when it came after the sample
    /// description).
    pub track_id: u32,
    /// Sample entry type of the disallowed description.
    pub fourcc: FourCc,
    /// The track's kind from its handler (`Data` for a handler this crate
    /// does not name, or none).
    pub kind: TrackKind,
    /// Media timescale from the media header; 0 when it was not read.
    pub timescale: u32,
    /// Media header duration in media ticks, as stored; 0 when unknown.
    pub mdhd_duration: u64,
    /// The track's edit list, classified as for kept tracks; `None` without
    /// one.
    pub edit_list: EditList,
    /// Why it was dropped.
    pub reason: DropReason,
    /// The sum of the durations of the stream's fragment samples, in media
    /// ticks, once [`Movie::scan_fragments`] has read the fragments; `None`
    /// before, or without fragments. Only the fragments' run headers and
    /// track defaults are read for it, never the samples.
    pub fragment_duration: Option<u64>,
    /// The base media decode time (`tfdt`) of its first fragment with
    /// samples, as for [`Track::fragment_start`].
    pub fragment_start: Option<i64>,
    /// The composition offset of the stream's first fragment sample.
    pub(crate) fragment_first_offset: Option<i64>,
}

/// When one stream is presented on the movie timeline
/// ([`Movie::presentation_spans`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationSpan {
    /// The stream's kind, from its handler.
    pub kind: TrackKind,
    /// Its media timescale.
    pub timescale: u32,
    /// Its first presented instant, in seconds.
    pub start: Rational,
    /// Its end, in seconds.
    pub end: Rational,
}

impl DroppedStream {
    /// When the stream is presented on the movie timeline, in seconds: from
    /// its leading empty edit, for its media edit capped by its media header
    /// duration (without the sample tables, which a dropped stream does not
    /// read). In a movie with fragments the media header describes only the
    /// movie box samples, so the media edit alone counts.
    /// `None` when its timing is unknown.
    pub fn presentation(
        &self,
        movie_timescale: u32,
        has_fragments: bool,
    ) -> Option<(Rational, Rational)> {
        if self.timescale == 0 || movie_timescale == 0 {
            return None;
        }
        // The media's length: the media header's without fragments; with
        // them, the scanned fragments' when the movie box holds
        // none of the stream's samples (a media header duration of 0), else
        // unknown, as its sample tables are not read.
        let header = if has_fragments {
            self.fragment_duration
                .filter(|_| self.mdhd_duration == 0)
                .map(|d| Rational::from_int(i128::from(d)))
        } else {
            (self.mdhd_duration > 0).then(|| Rational::from_int(i128::from(self.mdhd_duration)))
        };
        // The first sample's composition time: its fragment's `tfdt` plus
        // its offset.
        let first_offset = i128::from(self.fragment_start.unwrap_or(0))
            + i128::from(self.fragment_first_offset.unwrap_or(0));
        let (start, edit) = match self.edit_list {
            EditList::None => (first_offset, None),
            // An edit that lasts to the end of the media: the media from its
            // media time on, the part before the first sample's composition
            // offset not cut (as for kept streams).
            EditList::Simple {
                segment: 0,
                empty,
                media_time,
            } => {
                let skipped = (i128::from(media_time) - first_offset).max(0);
                let media = header?;
                let rest = Rational::from_int(media.num.checked_sub(skipped)?.max(0));
                return span(
                    ticks(empty, self.timescale, movie_timescale)?.round(),
                    rest,
                    self.timescale,
                );
            }
            EditList::Simple { empty, segment, .. } => (
                ticks(empty, self.timescale, movie_timescale)?.round(),
                ticks(segment, self.timescale, movie_timescale),
            ),
            // A dropped stream keeps no edit entries.
            EditList::Multiple | EditList::Unsupported => return None,
        };
        let duration = match (edit, header) {
            (Some(edit), Some(header)) if header.checked_lt(edit) == Some(true) => header,
            (Some(edit), _) => edit,
            (None, Some(header)) => header,
            (None, None) => return None,
        };
        span(start, duration, self.timescale)
    }
}

/// One track.
#[derive(Clone, Debug)]
pub struct Track {
    /// Position in [`Movie::tracks`]; the argument of [`Movie::packets`].
    pub index: usize,
    /// Position among all tracks of the file, dropped streams included.
    pub stream_index: usize,
    /// `track_ID` from the track header.
    pub track_id: u32,
    /// Kind, from the handler type.
    pub kind: TrackKind,
    /// Handler type.
    pub handler: FourCc,
    /// Handler name, lossily decoded: a counted string in a QuickTime file
    /// (the QuickTime File Format's component name), else the bytes up to
    /// the first NUL (ISO/IEC 14496-12 8.4.3, a null-terminated string).
    pub handler_name: Option<String>,
    /// ISO 639-2/T language code, `None` for QuickTime Macintosh codes.
    pub language: Option<[u8; 3]>,
    /// Media timescale (ticks per second).
    pub timescale: u32,
    /// Media header duration in media ticks (0 when unknown).
    pub mdhd_duration: u64,
    /// Sum of the decode durations of all samples in media ticks, including
    /// fragments when they were scanned.
    pub sample_duration: u64,
    /// Where [`Track::duration`] comes from.
    pub duration_source: DurationSource,
    /// Stream duration in media ticks; [`Track::duration_source`] tells where
    /// it comes from.
    pub duration: Option<Rational>,
    /// Edit list, classified by the simple edit model (see [`EditList`]) and
    /// `docs/design/edit-lists.md`.
    pub edit_list: EditList,
    /// The edit list's entries as stored, whatever its classification;
    /// empty without an edit list.
    pub edits: Vec<Edit>,
    /// Track header transformation matrix as stored.
    pub matrix: [i32; 9],
    /// Track header width and height, 16.16 fixed point as stored (0 when
    /// the box ends before them).
    pub presentation_size: (u32, u32),
    /// Clockwise rotation in degrees when the matrix is a pure quarter-turn
    /// rotation, `None` otherwise.
    pub rotation: Option<u16>,
    /// First sample description.
    pub description: SampleDescription,
    /// Number of sample descriptions.
    pub descriptions: u32,
    /// Samples in the movie box tables (fragments excluded).
    pub sample_count: u32,
    /// Samples in fragments, when they were scanned.
    pub fragment_samples: u64,
    /// The base media decode time (`tfdt`) of the track's first fragment
    /// that has samples, once [`Movie::scan_fragments`] has read the
    /// fragments; `None` before, or when that fragment has no `tfdt`.
    pub fragment_start: Option<i64>,
    /// The bytes of the samples of the track's first fragment that has
    /// samples, once [`Movie::scan_fragments`] has read the fragments;
    /// `None` before, without fragments, or when they cannot be counted.
    pub first_fragment_bytes: Option<u64>,
    /// The track's data is in another file; it has no packets.
    pub external_data: bool,
    /// The sample tables are consistent; packets are available.
    pub tables_valid: bool,
    /// Total bytes of every sample: the movie box samples, plus every
    /// fragment's samples after [`Movie::scan_fragments`]. `None` when the
    /// tables are invalid, or in a movie with fragments that were not scanned.
    pub sample_bytes: Option<u64>,
    /// Track header creation time, seconds since 1904-01-01 UTC as stored
    /// (see [`unix_time`]).
    pub creation_time: u64,
    /// Media header creation time, seconds since 1904-01-01 UTC as stored.
    pub media_creation_time: u64,
    /// Recoverable problems.
    pub warnings: Vec<Warning>,
    pub(crate) tables: SampleTables,
    /// Bytes of the movie box samples (`stsz`), `None` for invalid tables.
    pub(crate) stsz_bytes: Option<u64>,
    pub(crate) trex: Option<Trex>,
    /// The composition offset of the first fragment sample, after
    /// [`Movie::scan_fragments`].
    pub(crate) fragment_first_offset: Option<i64>,
    /// Sum of the `stts` decode durations (movie box samples only).
    pub(crate) stts_duration: u64,
    /// Whether the media handler (`hdlr`) names the QuickTime component
    /// type `mhlr`.
    pub(crate) quicktime_handler: bool,
    /// The handler name's bytes (at most 256), decoded into `handler_name`
    /// once the file is known to be QuickTime or not.
    pub(crate) handler_name_bytes: Vec<u8>,
}

impl Track {
    /// The track's `trex` defaults, when the movie box declares movie
    /// fragments for it: what listing one fragment on its own needs
    /// ([`crate::fragment_samples`]).
    pub fn fragment_defaults(&self) -> Option<TrackDefaults> {
        self.trex.map(|t| TrackDefaults {
            track_id: t.track_id,
            duration: t.duration,
            size: t.size,
            flags: t.flags,
        })
    }

    /// Whether the movie box samples' sync flags come from a sync sample
    /// table (`stss`). Without one, every movie box sample is a sync sample
    /// (ISO/IEC 14496-12 8.6.2): the container then states nothing about
    /// random access, and [`crate::Packet::keyframe`] is true throughout.
    /// Fragment samples carry their own flags.
    pub fn sync_table(&self) -> bool {
        self.tables.stss.is_some()
    }

    /// Entries of the sync sample table (`stss`), `None` without one. A
    /// table of no entry declares no sample a sync sample.
    pub fn sync_entries(&self) -> Option<u32> {
        self.tables.stss.as_ref().map(|t| t.count)
    }

    /// `sample_size` of `stsz` when every sample has that size.
    pub fn constant_sample_size(&self) -> Option<u32> {
        self.tables
            .stsz
            .as_ref()
            .map(|t| t.constant)
            .filter(|size| *size != 0)
    }

    /// Version of the composition offset table, when present (1 means
    /// signed offsets).
    pub fn ctts_version(&self) -> Option<u8> {
        self.tables.ctts.as_ref().map(|t| t.version)
    }

    /// Whether the composition offsets are read as signed: a version 1
    /// `ctts`, or any `ctts` of a QuickTime file (major brand `qt  `, or no
    /// `ftyp` and a media handler of component type `mhlr`), whose format
    /// defines them signed. `None` without a `ctts`.
    pub fn ctts_signed(&self) -> Option<bool> {
        self.tables.ctts.as_ref().map(|t| t.signed)
    }

    /// Leading empty edits converted to media ticks (exact): where the
    /// first media edit starts on the presentation timeline.
    pub fn presentation_offset(&self, movie_timescale: u32) -> Option<Rational> {
        match self.edit_list {
            EditList::None => Some(Rational::from_int(0)),
            EditList::Simple { empty, .. } => ticks(empty, self.timescale, movie_timescale),
            EditList::Multiple => ticks(self.leading_empty()?, self.timescale, movie_timescale),
            EditList::Unsupported => None,
        }
    }

    /// Sum of the entries from the first media edit through the last one,
    /// empty edits between them included, in movie ticks.
    fn media_edit_span(&self) -> Option<u64> {
        let first = self.edits.iter().position(|e| !e.is_empty_edit())?;
        let last = self.edits.iter().rposition(|e| !e.is_empty_edit())?;
        self.edits
            .get(first..=last)?
            .iter()
            .try_fold(0u64, |sum, e| sum.checked_add(e.segment_duration))
    }

    /// Sum of the empty edits before the first media edit, in movie ticks.
    fn leading_empty(&self) -> Option<u64> {
        self.edits
            .iter()
            .take_while(|e| e.is_empty_edit())
            .try_fold(0u64, |sum, e| sum.checked_add(e.segment_duration))
    }

    /// Exact bit rate in bits per second: [`Track::sample_bytes`] × 8 divided
    /// by the stream duration ([`Track::duration`]) in seconds.
    /// `None` when either is unknown, the duration is 0, or the value is not
    /// representable.
    pub fn bit_rate(&self) -> Option<Rational> {
        let bytes = self.sample_bytes?;
        let duration = self.duration?;
        if duration.num <= 0 {
            return None;
        }
        // bytes × 8 / (num / (den × timescale)) = bytes × 8 × den × timescale / num
        let num = i128::from(bytes)
            .checked_mul(8)?
            .checked_mul(i128::from(duration.den))?
            .checked_mul(i128::from(self.timescale))?;
        Rational::new(num, u64::try_from(duration.num).ok()?)
    }

    /// When the stream is presented on the movie timeline, in seconds, from
    /// its first presented instant for its duration ([`Track::duration`]).
    /// The first instant is the leading empty edit, rounded to the stream's
    /// own ticks (its samples are presented at media ticks); without an
    /// edit list, the composition time of the first sample (the
    /// presentation timeline is the composition timeline). With several
    /// edits, the stream ends where its last media edit ends: the empty
    /// edits between media edits are inside the span, though not in the
    /// duration (`docs/design/edit-lists.md`, decision 5). `None` when the
    /// start or the duration is unknown.
    pub fn presentation(&self, movie_timescale: u32) -> Option<(Rational, Rational)> {
        if self.edit_list == EditList::Multiple {
            let start = ticks(self.leading_empty()?, self.timescale, movie_timescale)?.round();
            // From the first media edit through the last, less what the
            // duration's cap by the media header takes off the media edits.
            let shown = ticks(self.media_edit_span()?, self.timescale, movie_timescale)?;
            let capped =
                negate(self.media_edit_duration(movie_timescale)?)?.checked_add(self.duration?)?;
            return span(start, shown.checked_add(capped)?, self.timescale);
        }
        let start = match self.edit_list {
            EditList::None => i128::from(self.first_composition_offset()),
            EditList::Simple { empty, .. } => {
                ticks(empty, self.timescale, movie_timescale)?.round()
            }
            EditList::Multiple => {
                ticks(self.leading_empty()?, self.timescale, movie_timescale)?.round()
            }
            EditList::Unsupported => return None,
        };
        span(start, self.duration?, self.timescale)
    }

    /// The composition time of the first sample in decode order: its
    /// `ctts` offset (the movie box samples start at decode time 0), or for
    /// a track whose samples are all in fragments, once scanned, the first
    /// fragment's `tfdt` plus its first sample's offset; 0 without one.
    fn first_composition_offset(&self) -> i64 {
        if self.sample_count == 0 {
            return self
                .fragment_start
                .unwrap_or(0)
                .saturating_add(self.fragment_first_offset.unwrap_or(0));
        }
        let Some(ctts) = &self.tables.ctts else {
            return 0;
        };
        match ctts.first_offset() {
            Some(raw) if ctts.signed => i64::from(i32::from_be_bytes(raw.to_be_bytes())),
            Some(raw) => i64::from(raw),
            None => 0,
        }
    }

    /// Media edit duration converted to media ticks (exact), the media
    /// edits' durations added up for [`EditList::Multiple`]; `None` without
    /// a media edit, or for one that lasts to the end of the media (a
    /// duration of 0).
    pub fn media_edit_duration(&self, movie_timescale: u32) -> Option<Rational> {
        match self.edit_list {
            EditList::Simple { segment, .. } if segment > 0 => {
                ticks(segment, self.timescale, movie_timescale)
            }
            EditList::Multiple => {
                let total = self
                    .edits
                    .iter()
                    .filter(|e| !e.is_empty_edit())
                    .try_fold(0u64, |sum, e| sum.checked_add(e.segment_duration))?;
                ticks(total, self.timescale, movie_timescale)
            }
            _ => None,
        }
    }
}

fn is_av(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Video | TrackKind::Audio)
}

/// The first start and last end `ffprobe`'s format duration spans, from
/// `(start, end, is audio or video)` per stream: the audio and video ones,
/// then each other stream whose end is less than `second` after theirs;
/// every stream without audio or video.
fn format_bounds<T: Copy>(
    spans: &[(T, T, bool)],
    second: T,
    add: impl Fn(T, T) -> Option<T>,
    lt: impl Fn(T, T) -> bool,
) -> Option<(T, T)> {
    let widen = |bounds: Option<(T, T)>, (start, end): (T, T)| {
        Some(match bounds {
            Some((first, last)) => (
                if lt(start, first) { start } else { first },
                if lt(last, end) { end } else { last },
            ),
            None => (start, end),
        })
    };
    let mut bounds = None;
    for (start, end, av) in spans {
        if *av {
            bounds = widen(bounds, (*start, *end));
        }
    }
    let Some((_, av_end)) = bounds else {
        return spans
            .iter()
            .fold(None, |b, (start, end, _)| widen(b, (*start, *end)));
    };
    let limit = add(av_end, second)?;
    for (start, end, av) in spans {
        if !*av && lt(*end, limit) {
            bounds = widen(bounds, (*start, *end));
        }
    }
    bounds
}

/// The latest end minus the earliest start of `spans`; `None` without one.
fn span_of<'a>(spans: impl Iterator<Item = &'a PresentationSpan>) -> Option<Rational> {
    let mut first: Option<Rational> = None;
    let mut last: Option<Rational> = None;
    for s in spans {
        if first.is_none_or(|f| s.start.checked_lt(f) == Some(true)) {
            first = Some(s.start);
        }
        if last.is_none_or(|l| l.checked_lt(s.end) == Some(true)) {
            last = Some(s.end);
        }
    }
    let first = first?;
    last?.checked_add(Rational::new(first.num.checked_neg()?, first.den)?)
}

/// `(start, end)` in seconds of a stream presented from `start` media ticks
/// for `duration` media ticks.
fn span(start: i128, duration: Rational, timescale: u32) -> Option<(Rational, Rational)> {
    let seconds =
        |ticks: Rational| Rational::new(ticks.num, ticks.den.checked_mul(u64::from(timescale))?);
    let start = Rational::from_int(start);
    Some((seconds(start)?, seconds(start.checked_add(duration)?)?))
}

fn negate(value: Rational) -> Option<Rational> {
    Rational::new(value.num.checked_neg()?, value.den)
}

fn ticks(movie_ticks: u64, timescale: u32, movie_timescale: u32) -> Option<Rational> {
    let num = i128::from(movie_ticks).checked_mul(i128::from(timescale))?;
    Rational::new(num, u64::from(movie_timescale))
}

/// Seconds from 1904-01-01 to 1970-01-01, both 00:00:00 UTC.
const MAC_TO_UNIX: i64 = 2_082_844_800;

/// Converts a stored creation time (seconds since 1904-01-01 00:00:00 UTC,
/// ISO/IEC 14496-12 8.2.2) to seconds since the Unix epoch. Returns `None`
/// for 0, which writers use for "not set", and for values beyond `i64`.
pub fn unix_time(stored: u64) -> Option<i64> {
    if stored == 0 {
        return None;
    }
    i64::try_from(stored).ok()?.checked_sub(MAC_TO_UNIX)
}

/// A probed movie.
#[derive(Clone, Debug)]
pub struct Movie {
    /// `major_brand` of `ftyp`, when present.
    pub major_brand: Option<FourCc>,
    /// Movie timescale.
    pub timescale: u32,
    /// Movie header duration in movie ticks (0 when unknown).
    pub mvhd_duration: u64,
    /// Movie header transformation matrix as stored (the identity when the
    /// box ends before it), applied after each track's.
    pub matrix: [i32; 9],
    /// Movie header creation time, seconds since 1904-01-01 UTC as stored
    /// (see [`unix_time`]).
    pub creation_time: u64,
    /// Tracks in file order, without dropped streams.
    pub tracks: Vec<Track>,
    /// Streams left out because their codec is not allowed.
    pub dropped: Vec<DroppedStream>,
    /// The movie has fragments (`mvex`).
    pub fragmented: bool,
    /// `fragment_duration` of the movie extends header (`mehd`), in movie
    /// ticks, when present.
    pub fragment_duration: Option<u64>,
    /// Segment indexes (`sidx`) found before the first fragment.
    pub segment_indexes: Vec<SegmentIndex>,
    /// `track_ID`s that a track reference of type `chap` names: the
    /// QuickTime chapter tracks (Apple QuickTime File Format, "Chapter
    /// lists"). Read from every enabled track, dropped streams included, in
    /// file order without repeats, at most [`MAX_CHAPTER_REFERENCES`]; the
    /// first is the one QuickTime uses. An ID may name no track of the file.
    pub chapter_tracks: Vec<u32>,
    /// `track_ID`s that a `chap` track reference of any track names, the
    /// references of disabled tracks included, in file order without
    /// repeats, at most [`MAX_CHAPTER_REFERENCES`]. `ffprobe` lists a timed
    /// text track named here as a data stream, whichever track names it
    /// (`docs/known-divergences.md`, `KD-CHAPTER-TEXT`); a reference past
    /// the cap, or in a `tref` box this parser ignores, is not listed.
    /// QuickTime's chapters are [`Movie::chapter_tracks`].
    pub chapter_references: Vec<u32>,
    /// The movie box was compressed (a QuickTime compressed movie, `cmov`,
    /// docs/design/compressed-movie.md): its boxes were read from the
    /// inflated data, an error in them reports the `cmvd` box's offset, and
    /// [`Movie::itunes_gapless`] gives what probe read from them.
    pub compressed: bool,
    /// For a compressed movie, the gapless item of its inflated movie box,
    /// read while it was in memory.
    pub(crate) inflated_gapless: Option<Option<ItunesGapless>>,
    pub(crate) first_fragment: Option<u64>,
    pub(crate) file_len: u64,
    /// Fragment defaults of every track, dropped ones included.
    pub(crate) all_trex: Vec<Trex>,
    /// Movie-level `udta` and `meta` boxes, located but never read by probe;
    /// see [`Movie::itunes_gapless`].
    pub(crate) tag_boxes: Vec<BoxHeader>,
}

impl Movie {
    /// Whether the movie has fragments: an `mvex` box and at least one `moof`.
    /// Its header durations then describe only the movie box samples.
    pub fn has_fragments(&self) -> bool {
        self.fragmented && self.first_fragment.is_some()
    }

    /// Offset of the first movie fragment's `moof` box, when the movie has
    /// fragments: where the media a segment index names must begin.
    pub fn first_fragment(&self) -> Option<u64> {
        self.first_fragment
    }

    /// How many tracks the movie declares fragment defaults for
    /// (`mvex`/`trex`): how many a movie fragment may hold, and so
    /// whether a subsegment's bytes are one track's alone.
    pub fn fragment_tracks(&self) -> usize {
        self.all_trex.len()
    }

    /// The iTunes gapless item `iTunSMPB` of the movie, if it has a
    /// well-formed one. Reads only the
    /// movie's `udta` and `meta` boxes from `source` (the probed file),
    /// bounded by [`MAX_TAG_BOXES`](crate::MAX_TAG_BOXES) box headers and small payloads; anything
    /// malformed gives `Ok(None)`, never an error. A source that fails to
    /// read ([`Error::Io`]) is an error, so that a transient read error
    /// cannot change what a caller writes. The values come from an Apple
    /// convention, not a cited specification: check them against the sample
    /// table before relying on them. For a compressed movie
    /// ([`Movie::compressed`]) the item was read from the inflated movie box
    /// during probe, and `source` is not read.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when `source` fails to read.
    pub fn itunes_gapless<S: Source>(&self, source: S) -> Result<Option<ItunesGapless>, Error> {
        if let Some(found) = self.inflated_gapless {
            // A compressed movie's metadata was read from its inflated box.
            return Ok(found);
        }
        let mut source = source;
        gapless::read(&mut source, &self.tag_boxes)
    }

    /// Whether [`Movie::scan_fragments`] would tell more than probe: the
    /// movie has fragments, and a kept stream's duration or sample bytes, or
    /// a dropped stream's fragment duration (for its timing; when the movie
    /// box holds none of its samples), is still unknown.
    pub fn needs_fragment_scan(&self) -> bool {
        self.has_fragments()
            && (self
                .tracks
                .iter()
                .any(|t| t.duration_source == DurationSource::Unknown || t.sample_bytes.is_none())
                || self.dropped.iter().any(|d| {
                    d.track_id != 0 && d.mdhd_duration == 0 && d.fragment_duration.is_none()
                }))
    }

    /// When each stream whose timing is known is presented, kept and dropped
    /// streams alike, in file order of each list (kept streams first).
    pub fn presentation_spans(&self) -> Vec<PresentationSpan> {
        let kept = self.tracks.iter().filter_map(|t| {
            let (start, end) = t.presentation(self.timescale)?;
            Some(PresentationSpan {
                kind: t.kind,
                timescale: t.timescale,
                start,
                end,
            })
        });
        let dropped = self.dropped.iter().filter_map(|d| {
            let (start, end) = d.presentation(self.timescale, self.has_fragments())?;
            Some(PresentationSpan {
                kind: d.kind,
                timescale: d.timescale,
                start,
                end,
            })
        });
        kept.chain(dropped).collect()
    }

    /// The derived presentation duration in seconds: the latest stream end
    /// minus the earliest stream start, over every stream whose timing is
    /// known, the streams the allowlist dropped included. `None` when no
    /// stream's timing is known. [`Movie::duration_seconds`] is the declared
    /// duration; the two differ when the movie header does not match the
    /// streams (rounding to the movie timescale, a stale header) or when the
    /// earliest stream starts after 0.
    pub fn presentation_duration_seconds(&self) -> Option<Rational> {
        span_of(self.presentation_spans().iter())
    }

    /// The format duration as `ffprobe` reports it, in seconds: the span of
    /// the audio and video streams, extended by each subtitle, timecode or
    /// other data stream that ends less than a second after them (one that
    /// ends a second or more later is left out); every stream's span when
    /// there is no audio or video. Measured black-box
    /// (`docs/design/probe-duration.md`). [`Movie::presentation_duration_seconds`]
    /// is the span of every stream. `None` when no stream's timing is known.
    pub fn ffprobe_format_duration_seconds(&self) -> Option<Rational> {
        let spans = self.presentation_spans();
        let bounds: Vec<(Rational, Rational, bool)> = spans
            .iter()
            .map(|s| (s.start, s.end, is_av(s.kind)))
            .collect();
        let (first, last) = format_bounds(
            &bounds,
            Rational::from_int(1),
            |a, b| a.checked_add(b),
            |a, b| a.checked_lt(b) == Some(true),
        )?;
        last.checked_add(Rational::new(first.num.checked_neg()?, first.den)?)
    }

    /// [`Movie::ffprobe_format_duration_seconds`] in whole microseconds,
    /// counted as `ffprobe` counts it: each stream's duration in whole ticks
    /// of its timescale first (its `duration_ts`), then its start and that
    /// duration in microseconds on their own, rounded to the nearest (halves
    /// away from zero); then the same rule on those values.
    pub fn ffprobe_format_duration_micros(&self) -> Option<i128> {
        let us =
            |r: Rational| Rational::new(r.num.checked_mul(1_000_000)?, r.den).map(Rational::round);
        let mut bounds = Vec::new();
        for span in self.presentation_spans() {
            let scale = u64::from(span.timescale.max(1));
            let duration = span
                .start
                .num
                .checked_neg()
                .and_then(|num| Rational::new(num, span.start.den))
                .and_then(|negated| span.end.checked_add(negated))
                .and_then(|d| Rational::new(d.num.checked_mul(i128::from(scale))?, d.den))
                .and_then(|ticks| Rational::new(ticks.round(), scale));
            let (Some(start), Some(duration)) = (us(span.start), duration.and_then(us)) else {
                continue;
            };
            bounds.push((start, start.saturating_add(duration), is_av(span.kind)));
        }
        let (first, last) =
            format_bounds(&bounds, 1_000_000, |a, b| a.checked_add(b), |a, b| a < b)?;
        Some(last.saturating_sub(first))
    }

    /// Presentation duration in seconds: the movie header duration when the
    /// movie has no fragments, else the movie extends header (`mehd`), else
    /// the longest known track presentation.
    pub fn duration_seconds(&self) -> Option<Rational> {
        if self.mvhd_duration > 0 && !self.has_fragments() {
            return Rational::new(i128::from(self.mvhd_duration), u64::from(self.timescale));
        }
        if let Some(mehd) = self.fragment_duration.filter(|d| *d > 0) {
            return Rational::new(i128::from(mehd), u64::from(self.timescale));
        }
        let mut longest: Option<Rational> = None;
        for track in &self.tracks {
            let (Some(duration), Some(offset)) =
                (track.duration, track.presentation_offset(self.timescale))
            else {
                continue;
            };
            let end = duration.checked_add(offset)?;
            let seconds = Rational::new(end.num, end.den.checked_mul(u64::from(track.timescale))?)?;
            if longest.is_none_or(|l| l.checked_lt(seconds).unwrap_or(false)) {
                longest = Some(seconds);
            }
        }
        longest
    }

    /// Reads every movie fragment and reports each fragmented track's
    /// observed sample span as its duration. Probe never does this; call it
    /// when the duration of a
    /// fragmented track is [`DurationSource::Unknown`] and needed. Fragment
    /// count, box size and work are bounded by `limits`; a truncated or
    /// malformed fragment is an error. Calling it again gives the same result.
    pub fn scan_fragments<S: Source>(&mut self, source: S, limits: &Limits) -> Result<(), Error> {
        let Some(mut at) = self.first_fragment else {
            return Ok(());
        };
        let mut source = source;
        let mut work = Work::new(limits);
        let mut totals = vec![(0u64, 0u64); self.tracks.len()];
        let mut first_offsets: Vec<Option<i64>> = vec![None; self.tracks.len()];
        // The `tfdt` of each track's first fragment with samples (None when
        // that fragment has none), and whether it was seen.
        let mut starts: Vec<(bool, Option<i64>)> = vec![(false, None); self.tracks.len()];
        let mut bytes = vec![Some(0u64); self.tracks.len()];
        let mut first_bytes: Vec<Option<u64>> = vec![None; self.tracks.len()];
        // Dropped streams: only their fragment durations and first
        // composition offset, for their timing (none of their samples).
        let dropped_trex: Vec<Option<Trex>> = self
            .dropped
            .iter()
            .map(|d| {
                self.all_trex
                    .iter()
                    .find(|t| d.track_id != 0 && t.track_id == d.track_id)
                    .copied()
            })
            .collect();
        let mut dropped_totals: Vec<Option<u64>> = vec![None; self.dropped.len()];
        // Dropped streams whose fragments this file cannot back: their
        // timing is dropped with them, and later fragments do not revive it.
        let mut dropped_unusable = vec![false; self.dropped.len()];
        let mut dropped_offsets: Vec<Option<i64>> = vec![None; self.dropped.len()];
        let mut dropped_starts: Vec<(bool, Option<i64>)> = vec![(false, None); self.dropped.len()];
        let mut fragments = 0u32;
        let len = self.file_len;
        while at < len {
            let header = read_header(&mut source, at, len)?;
            work.add(1)?;
            if header.kind == FourCc(*b"moof") {
                fragments = fragments.saturating_add(1);
                if fragments > limits.max_fragments {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::Fragments,
                        offset: at,
                    });
                }
                let payload = read_payload(
                    &mut source,
                    &header,
                    limits.max_box_bytes,
                    LimitKind::BoxBytes,
                )?;
                let boxes = Cell::new(0u64);
                for (((((track, total), sum), first_offset), first_start), first_sum) in self
                    .tracks
                    .iter()
                    .zip(totals.iter_mut())
                    .zip(bytes.iter_mut())
                    .zip(first_offsets.iter_mut())
                    .zip(starts.iter_mut())
                    .zip(first_bytes.iter_mut())
                {
                    let Some(trex) = track.trex else { continue };
                    // A track whose media is in another file gets no
                    // packets, and nothing here can
                    // back what its fragments declare: its offsets are
                    // offsets into that file. Its samples are not counted
                    // and its duration stays unknown (`fragment.rs`, "What
                    // backs a declared sample count").
                    if track.external_data {
                        continue;
                    }
                    let mut fragment = Fragment::parse(
                        payload.clone(),
                        header.offset,
                        header.payload_offset(),
                        len,
                        trex,
                        &self.all_trex,
                        &boxes,
                    )?;
                    let (duration, samples, start) = fragment.duration(&mut work)?;
                    if samples > 0 && !first_start.0 {
                        *first_start = (true, start);
                    }
                    total.0 = total
                        .0
                        .checked_add(duration)
                        .ok_or(Error::Overflow { offset: at })?;
                    total.1 = total.1.saturating_add(samples);
                    // Sum the sample sizes run by run, with the
                    // work accounting of packet iteration. Walking runs also
                    // advances data positions and decode times; when those
                    // overflow (for example a `tfdt` near `i64::MAX`) the
                    // byte count is unknown, but the scan still succeeds.
                    let (mut position, mut dts) = (0u64, 0i64);
                    let before = *sum;
                    while let Some(total_bytes) = *sum {
                        let run =
                            match fragment.next_run(&mut position, &mut dts, &mut work, u32::MAX) {
                                Ok(Some(run)) => run,
                                Ok(None) => break,
                                Err(Error::Overflow { .. }) => {
                                    *sum = None;
                                    break;
                                }
                                Err(error) => return Err(error),
                            };
                        first_offset.get_or_insert(run.composition_offset);
                        let run_bytes = u64::from(run.size)
                            .checked_mul(u64::from(run.count))
                            .ok_or(Error::Overflow { offset: at })?;
                        *sum = Some(
                            total_bytes
                                .checked_add(run_bytes)
                                .ok_or(Error::Overflow { offset: at })?,
                        );
                    }
                    if samples > 0 && first_sum.is_none() {
                        *first_sum = before.zip(*sum).map(|(b, a)| a.saturating_sub(b));
                    }
                }
                for ((((trex, total), first_offset), first_start), unusable) in dropped_trex
                    .iter()
                    .zip(dropped_totals.iter_mut())
                    .zip(dropped_offsets.iter_mut())
                    .zip(dropped_starts.iter_mut())
                    .zip(dropped_unusable.iter_mut())
                {
                    let Some(trex) = trex else { continue };
                    if *unusable {
                        continue;
                    }
                    // A dropped stream is not exposed, so a fragment of it
                    // this file cannot back takes its timing with it and
                    // nothing else, as an unusable sample table invalidates
                    // only its track. That also covers a
                    // dropped stream whose media is in another file, whose
                    // offsets this file cannot judge: a stream the caller
                    // never sees must not report a duration nothing backs,
                    // and must not refuse the file either.
                    let outcome = (|| -> Result<(), Error> {
                        let mut fragment = Fragment::parse(
                            payload.clone(),
                            header.offset,
                            header.payload_offset(),
                            len,
                            *trex,
                            &self.all_trex,
                            &boxes,
                        )?;
                        let (duration, samples, start) = fragment.duration(&mut work)?;
                        if samples == 0 {
                            return Ok(());
                        }
                        if !first_start.0 {
                            *first_start = (true, start);
                        }
                        *total = Some(
                            total
                                .unwrap_or(0)
                                .checked_add(duration)
                                .ok_or(Error::Overflow { offset: at })?,
                        );
                        if first_offset.is_none() {
                            let (mut position, mut dts) = (0u64, 0i64);
                            if let Some(run) =
                                fragment.next_run(&mut position, &mut dts, &mut work, 1)?
                            {
                                *first_offset = Some(run.composition_offset);
                            }
                        }
                        Ok(())
                    })();
                    match outcome {
                        Ok(()) => {}
                        // A limit is the request's, not the stream's.
                        Err(error @ Error::LimitExceeded { .. }) => return Err(error),
                        Err(_) => {
                            *unusable = true;
                            *total = None;
                        }
                    }
                }
                work.add(boxes.get())?;
            }
            at = header.end();
        }
        for (((stream, total), first_offset), start) in self
            .dropped
            .iter_mut()
            .zip(dropped_totals)
            .zip(dropped_offsets)
            .zip(dropped_starts)
        {
            stream.fragment_duration = total;
            stream.fragment_first_offset = first_offset;
            stream.fragment_start = start.1;
        }
        for (((((track, (duration, samples)), sum), first_offset), start), first_sum) in self
            .tracks
            .iter_mut()
            .zip(totals)
            .zip(bytes)
            .zip(first_offsets)
            .zip(starts)
            .zip(first_bytes)
        {
            track.fragment_first_offset = first_offset;
            track.first_fragment_bytes = first_sum;
            track.fragment_start = start.1;
            track.sample_bytes = match (track.stsz_bytes, sum) {
                (Some(stsz), Some(fragments)) => Some(
                    stsz.checked_add(fragments)
                        .ok_or(Error::Overflow { offset: 0 })?,
                ),
                _ => None,
            };
            track.sample_duration = track
                .stts_duration
                .checked_add(duration)
                .ok_or(Error::Overflow { offset: 0 })?;
            track.fragment_samples = samples;
            // A track whose fragments were not read (external media, above)
            // has observed nothing: its duration stays what it was before
            // the scan, which for a fragmented movie is unknown.
            finish_duration(track, self.timescale, true, !track.external_data);
        }
        Ok(())
    }
}

struct Walker<'a, S> {
    source: S,
    limits: &'a Limits,
    boxes: u64,
    retained: u64,
    table_bytes: u64,
    work: Work,
}

impl<S: Source> Walker<'_, S> {
    fn header(&mut self, at: u64, end: u64) -> Result<BoxHeader, Error> {
        let room = end.checked_sub(at).ok_or(Error::Truncated { offset: at })?;
        let mut buf = [0u8; MAX_HEADER_LEN];
        let n = usize::try_from(room.min(MAX_HEADER_LEN as u64)).unwrap_or(MAX_HEADER_LEN);
        let slice = buf.get_mut(..n).ok_or(Error::Truncated { offset: at })?;
        self.source.read_exact_at(at, slice)?;
        let header = match parse_box_header(slice, at, room)? {
            Parsed::Done(header) => header,
            Parsed::NeedMore(_) => return Err(Error::Truncated { offset: at }),
        };
        self.count(1, at)?;
        Ok(header)
    }

    fn count(&mut self, boxes: u64, at: u64) -> Result<(), Error> {
        self.boxes = self.boxes.saturating_add(boxes);
        if self.boxes > u64::from(self.limits.max_total_boxes) {
            return Err(Error::LimitExceeded {
                limit: LimitKind::TotalBoxes,
                offset: at,
            });
        }
        Ok(())
    }

    fn read(&mut self, at: u64, len: u64, limit: LimitKind, max: u64) -> Result<Vec<u8>, Error> {
        if len > max {
            return Err(Error::LimitExceeded { limit, offset: at });
        }
        let size = usize::try_from(len).map_err(|_| Error::LimitExceeded { limit, offset: at })?;
        let mut data = Vec::new();
        data.try_reserve_exact(size)
            .map_err(|_| Error::LimitExceeded { limit, offset: at })?;
        // Read into the reserved capacity: the bytes are written once,
        // not zeroed first (a large sample table is megabytes).
        self.source.read_append(at, size, &mut data)?;
        Ok(data)
    }

    fn payload(&mut self, header: &BoxHeader) -> Result<Vec<u8>, Error> {
        self.read(
            header.payload_offset(),
            header.payload_len(),
            LimitKind::BoxBytes,
            self.limits.max_box_bytes,
        )
    }

    fn retain(&mut self, bytes: usize, at: u64) -> Result<(), Error> {
        self.retained = self.retained.saturating_add(bytes as u64);
        if self.retained > self.limits.max_retained_bytes {
            return Err(Error::LimitExceeded {
                limit: LimitKind::RetainedBytes,
                offset: at,
            });
        }
        Ok(())
    }

    fn children(
        &mut self,
        parent: &BoxHeader,
        depth: u32,
        visit: &mut dyn FnMut(&mut Self, BoxHeader, u32) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let depth = depth.saturating_add(1);
        if depth > self.limits.max_depth {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Depth,
                offset: parent.offset,
            });
        }
        let end = parent.end();
        let mut at = parent.payload_offset();
        // QuickTime allows a 32-bit zero terminator at the end of a list.
        while end.saturating_sub(at) >= 8 {
            let header = self.header(at, end)?;
            visit(self, header, depth)?;
            at = header.end();
        }
        Ok(())
    }
}

/// Movie-level `udta` and `meta` boxes remembered for tag reading.
const MAX_TAG_ROOTS: usize = 4;

/// Most `track_ID`s kept in [`Movie::chapter_tracks`]; later ones are
/// ignored.
pub const MAX_CHAPTER_REFERENCES: usize = 64;

/// Largest `tref` box read; a larger one is ignored. Room for 1,000 track
/// references.
const MAX_TREF_BYTES: u64 = 4096;

/// Box types that may start an ISO base media or QuickTime file.
const FIRST_BOXES: [&[u8; 4]; 13] = [
    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot", b"styp", b"uuid", b"sidx",
    b"moof", b"meta", b"junk",
];

/// Reads the movie structure and metadata of a file.
///
/// Fails if the input is not an ISO base media or QuickTime file, if it is
/// structurally invalid or truncated, or if a limit is exceeded. A track whose
/// codec is outside `allowlist` is dropped and reported in
/// [`Movie::dropped`], or rejects the file when the allowlist is strict.
/// Sample tables are validated; a track with inconsistent
/// tables stays in the result with `tables_valid` false.
pub fn probe<S: Source>(source: S, limits: &Limits, allowlist: &Allowlist) -> Result<Movie, Error> {
    let mut source = source;
    let len = source.len()?;
    let mut w = Walker {
        source,
        limits,
        boxes: 0,
        retained: 0,
        table_bytes: 0,
        work: Work::new(limits),
    };
    let mut first = [0u8; 8];
    if len < 8 {
        return Err(Error::NotIsobmff);
    }
    w.source.read_exact_at(0, &mut first)?;
    let kind = first.get(4..8).unwrap_or_default();
    if !FIRST_BOXES.iter().any(|k| k.as_slice() == kind) {
        return Err(Error::NotIsobmff);
    }

    let mut movie: Option<Movie> = None;
    let mut major_brand = None;
    let mut segment_indexes = Vec::new();
    let mut top_level = 0u32;
    let mut fragments = 0u32;
    let mut at = 0u64;
    while at < len {
        if len - at < 8 {
            return Err(Error::Truncated { offset: at });
        }
        let header = w.header(at, len)?;
        let is_fragment =
            header.kind == FourCc(*b"moof") || (header.kind == FourCc(*b"mdat") && fragments > 0);
        if is_fragment {
            if header.kind == FourCc(*b"moof") {
                fragments = fragments.saturating_add(1);
                if fragments > limits.max_fragments {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::Fragments,
                        offset: at,
                    });
                }
            }
        } else {
            top_level = top_level.saturating_add(1);
            if top_level > limits.max_top_level_boxes {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::TopLevelBoxes,
                    offset: at,
                });
            }
        }
        match &header.kind.0 {
            b"ftyp" if major_brand.is_none() => {
                let payload = w.payload(&header)?;
                major_brand = Bytes::new(&payload, header.payload_offset()).fourcc().ok();
            }
            b"moov" => {
                if movie.is_some() {
                    return Err(Error::InvalidBox {
                        offset: at,
                        kind: header.kind,
                        reason: "second movie box",
                    });
                }
                movie = Some(parse_moov(&mut w, &header, allowlist, false)?);
            }
            b"sidx" => {
                let payload = w.payload(&header)?;
                let index = parse_sidx(&payload, header.payload_offset(), header.end(), len)?;
                w.retain(index.references.len().saturating_mul(24), at)?;
                segment_indexes.push(index);
            }
            b"moof" => {
                let Some(m) = movie.as_mut() else {
                    return Err(Error::InvalidBox {
                        offset: at,
                        kind: header.kind,
                        reason: "fragment before the movie box",
                    });
                };
                // Fragments are read by packet iteration and by
                // `Movie::scan_fragments`, never by probe.
                m.first_fragment = Some(at);
                break;
            }
            _ => {}
        }
        at = header.end();
    }
    let mut movie = movie.ok_or(Error::InvalidBox {
        offset: 0,
        kind: FourCc(*b"moov"),
        reason: "no movie box",
    })?;
    movie.major_brand = major_brand;
    // A QuickTime file: major brand `qt  `, or, without `ftyp` (which the
    // QuickTime File Format makes optional), a media handler with
    // QuickTime's component type, which ISO/IEC 14496-12 leaves 0.
    let quicktime = match major_brand {
        Some(brand) => brand == FourCc(*b"qt  "),
        None => movie.tracks.iter().any(|t| t.quicktime_handler),
    };
    for track in &mut movie.tracks {
        track.handler_name =
            handler_name(&std::mem::take(&mut track.handler_name_bytes), quicktime);
    }
    if quicktime {
        // The QuickTime File Format's composition offsets are signed in
        // every version.
        for track in &mut movie.tracks {
            if let Some(ctts) = &mut track.tables.ctts {
                ctts.signed = true;
            }
        }
    }
    movie.segment_indexes = segment_indexes;
    movie.file_len = len;
    let has_fragments = movie.has_fragments();
    for track in &mut movie.tracks {
        if has_fragments {
            // Probe never reads fragments.
            track.sample_bytes = None;
        }
        finish_duration(track, movie.timescale, has_fragments, false);
    }
    Ok(movie)
}

/// Stream duration: the media header duration (or the observed sample span
/// when it is 0) capped by the media edit; in a movie with fragments, the
/// media edit until the fragments are scanned, then the observed sample span
/// capped by the media edit. `scanned` tells whether every fragment's samples
/// are included in `sample_duration`.
fn finish_duration(track: &mut Track, movie_timescale: u32, has_fragments: bool, scanned: bool) {
    let edit = track.media_edit_duration(movie_timescale);
    let (cap, source) = if has_fragments {
        if !scanned {
            // The header durations describe only the movie box samples.
            track.duration = edit;
            track.duration_source = if edit.is_some() {
                DurationSource::MediaEdit
            } else {
                DurationSource::Unknown
            };
            return;
        }
        (track.sample_duration, DurationSource::Observed)
    } else if track.mdhd_duration > 0 {
        (track.mdhd_duration, DurationSource::MediaHeader)
    } else {
        (track.sample_duration, DurationSource::Observed)
    };
    let mut cap = Rational::from_int(i128::from(cap));
    // A media edit that lasts to the end presents the media from its media
    // time on.
    if let EditList::Simple {
        segment: 0,
        media_time,
        ..
    } = track.edit_list
    {
        // The media's composition times start at the first sample's offset
        // (a constant B-frame shift that the media time skips adds nothing).
        let skipped = i128::from(media_time) - i128::from(track.first_composition_offset());
        cap = Rational::from_int(cap.num.saturating_sub(skipped.max(0)).max(0));
    }
    track.duration = match edit {
        Some(edit) if edit.checked_lt(cap) == Some(true) => Some(edit),
        _ => Some(cap),
    };
    track.duration_source = source;
    let conflict = Warning::DurationConflict {
        mdhd: track.mdhd_duration,
        stts: track.sample_duration,
    };
    if !has_fragments
        && track.tables_valid
        && track.mdhd_duration > 0
        && track.mdhd_duration != track.sample_duration
        && !track.warnings.contains(&conflict)
    {
        crate::error::warn(&mut track.warnings, conflict);
    }
}

fn invalid(header: &BoxHeader, reason: &'static str) -> Error {
    Error::InvalidBox {
        offset: header.offset,
        kind: header.kind,
        reason,
    }
}

/// Reads a movie box. `inflated` tells that it came out of a compressed
/// movie box, which may not hold another.
fn parse_moov<S: Source>(
    w: &mut Walker<'_, S>,
    moov: &BoxHeader,
    allowlist: &Allowlist,
    inflated: bool,
) -> Result<Movie, Error> {
    let mut mvhd: Option<(u32, u64, u64, [i32; 9])> = None;
    let mut cmov: Option<(BoxHeader, u32)> = None;
    let mut tracks: Vec<Track> = Vec::new();
    let mut trex: Vec<Trex> = Vec::new();
    let mut dropped: Vec<DroppedStream> = Vec::new();
    let mut fragmented = false;
    let mut mehd: Option<u64> = None;
    let mut tag_boxes: Vec<BoxHeader> = Vec::new();
    let mut chapter_tracks: Vec<u32> = Vec::new();
    let mut chapter_references: Vec<u32> = Vec::new();
    w.children(moov, 0, &mut |w, child, depth| {
        match &child.kind.0 {
            b"udta" | b"meta" if tag_boxes.len() < MAX_TAG_ROOTS => tag_boxes.push(child),
            b"cmov" if cmov.is_none() => cmov = Some((child, depth)),
            b"mvhd" => {
                let payload = w.payload(&child)?;
                let mut b = Bytes::new(&payload, child.payload_offset());
                let (version, created, timescale, duration) = media_times(&mut b)?;
                if timescale == 0 {
                    return Err(invalid(&child, "zero timescale"));
                }
                // Rate, volume and reserved bytes, then the matrix; a box
                // that ends before it keeps the identity.
                let mut matrix = IDENTITY_MATRIX;
                if b.skip(16).is_ok() {
                    for value in &mut matrix {
                        let Ok(v) = b.i32() else {
                            matrix = IDENTITY_MATRIX;
                            break;
                        };
                        *value = v;
                    }
                }
                mvhd = Some((
                    timescale,
                    unknown_duration(version, duration),
                    created,
                    matrix,
                ));
            }
            b"trak" => {
                let stream_index = tracks.len().saturating_add(dropped.len());
                if stream_index >= w.limits.max_tracks as usize {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::Tracks,
                        offset: child.offset,
                    });
                }
                let index = tracks.len();
                let mut timing = DroppedTiming::default();
                match parse_trak(
                    w,
                    &child,
                    depth,
                    (index, stream_index),
                    allowlist,
                    (&mut chapter_tracks, &mut chapter_references),
                    &mut timing,
                ) {
                    Ok(track) => tracks.push(track),
                    // Drop the stream unless the caller asked for
                    // strict mode; nothing more of it is parsed or exposed.
                    Err(Error::CodecNotAllowed { track_id, fourcc }) if !allowlist.is_strict() => {
                        dropped.push(DroppedStream {
                            stream_index,
                            track_id,
                            fourcc,
                            kind: timing.kind,
                            timescale: timing.timescale,
                            mdhd_duration: timing.mdhd_duration,
                            edit_list: timing.edit_list,
                            reason: DropReason::CodecNotAllowed,
                            fragment_duration: None,
                            fragment_start: None,
                            fragment_first_offset: None,
                        });
                    }
                    Err(error) => return Err(error),
                }
            }
            b"mvex" => {
                fragmented = true;
                w.children(&child, depth, &mut |w, entry, _| {
                    if entry.kind == FourCc(*b"trex") {
                        let payload = w.payload(&entry)?;
                        trex.push(parse_trex(&payload, entry.payload_offset())?);
                    } else if entry.kind == FourCc(*b"mehd") {
                        let payload = w.payload(&entry)?;
                        let mut b = Bytes::new(&payload, entry.payload_offset());
                        let (version, _) = b.version_flags()?;
                        mehd = Some(if version == 1 {
                            b.u64()?
                        } else {
                            u64::from(b.u32()?)
                        });
                    }
                    Ok(())
                })?;
            }
            _ => {}
        }
        Ok(())
    })?;
    if let (None, Some((cmov, depth))) = (mvhd, cmov) {
        if inflated {
            return Err(invalid(&cmov, "compressed movie inside a compressed movie"));
        }
        if !tracks.is_empty() || !dropped.is_empty() {
            return Err(invalid(&cmov, "compressed movie beside tracks"));
        }
        return parse_compressed_moov(w, &cmov, depth, allowlist);
    }
    let (timescale, mvhd_duration, creation_time, matrix) =
        mvhd.ok_or(invalid(moov, "missing movie header"))?;
    // `trex` may come before or after the tracks; resolve it now.
    for defaults in &trex {
        let Some(track) = tracks.iter_mut().find(|t| t.track_id == defaults.track_id) else {
            // Defaults of a dropped stream are ignored with it.
            if dropped
                .iter()
                .any(|d| d.track_id == defaults.track_id || d.track_id == 0)
            {
                continue;
            }
            return Err(invalid(moov, "trex for an unknown track"));
        };
        if track.trex.is_some() {
            return Err(invalid(moov, "duplicate trex"));
        }
        track.trex = Some(*defaults);
    }
    if !fragmented {
        // A media edit lasts to the end of the media only in a movie with
        // fragments (ISO/IEC 14496-12 8.6.6), where the media's length is not
        // known when the movie box is written; elsewhere a duration of 0 is
        // outside the simple edit model.
        let open = |edit: &mut EditList| {
            let open = matches!(edit, EditList::Simple { segment: 0, .. });
            if open {
                *edit = EditList::Unsupported;
            }
            open
        };
        for track in &mut tracks {
            open(&mut track.edit_list);
        }
        for stream in &mut dropped {
            open(&mut stream.edit_list);
        }
    }
    for track in &mut tracks {
        // Several edits are applied in movies without fragments only: the
        // model was measured there (docs/design/edit-lists.md, decision 1).
        if !fragmented
            && track.edit_list == EditList::Unsupported
            && several_edits_apply(&track.edits, track.timescale, timescale)
        {
            track.edit_list = EditList::Multiple;
        }
        if track.edit_list == EditList::Unsupported {
            crate::error::warn(&mut track.warnings, Warning::UnsupportedEditList);
        }
    }
    let ids: Vec<u32> = tracks
        .iter()
        .map(|t| t.track_id)
        .chain(dropped.iter().map(|d| d.track_id).filter(|id| *id != 0))
        .collect();
    for (i, id) in ids.iter().enumerate() {
        if ids.iter().skip(i + 1).any(|other| other == id) {
            return Err(invalid(moov, "duplicate track ID"));
        }
    }
    Ok(Movie {
        major_brand: None,
        timescale,
        mvhd_duration,
        creation_time,
        matrix,
        tracks,
        dropped,
        fragmented,
        fragment_duration: mehd,
        segment_indexes: Vec::new(),
        chapter_tracks,
        chapter_references,
        compressed: false,
        inflated_gapless: None,
        first_fragment: None,
        file_len: 0,
        all_trex: trex.clone(),
        tag_boxes,
    })
}

/// The one compression algorithm of compressed movie boxes this crate
/// inflates: zlib (RFC 1950 and 1951).
const ZLIB: FourCc = FourCc(*b"zlib");

/// Reads a QuickTime compressed movie box (`cmov`, Apple QuickTime File
/// Format, "Compressed movie resources"): its data compression box
/// (`dcom`) names the algorithm, its compressed movie data box (`cmvd`)
/// holds the inflated size (32 bits) and the compressed data, which inflate
/// to a whole movie box. That box is read like one in the file, under the
/// same limits and counters; the inflated size is bounded by
/// `max_movie_bytes` and by `max_movie_ratio` times the compressed size,
/// and must be the size the box declares.
fn parse_compressed_moov<S: Source>(
    w: &mut Walker<'_, S>,
    cmov: &BoxHeader,
    depth: u32,
    allowlist: &Allowlist,
) -> Result<Movie, Error> {
    let (inflated, cmvd) = inflate_compressed_moov(w, cmov, depth)?;
    let declared = inflated.len() as u64;
    let limits = w.limits;
    // The inflated box, read through its own walker that carries on the
    // file walker's counters. Its offsets are offsets in the inflated data,
    // so an error in it reports the `cmvd` box.
    let mut inner = Walker {
        source: SliceSource::new(&inflated),
        limits,
        boxes: w.boxes,
        retained: w.retained,
        table_bytes: w.table_bytes,
        work: w.work,
    };
    let in_cmvd = |error| at_box(error, cmvd.offset);
    let header = inner.header(0, declared).map_err(in_cmvd)?;
    if header.kind != FourCc(*b"moov") || header.end() != declared {
        return Err(invalid(&cmvd, "compressed movie data is not one movie box"));
    }
    let mut movie = parse_moov(&mut inner, &header, allowlist, true).map_err(in_cmvd)?;
    w.boxes = inner.boxes;
    w.retained = inner.retained;
    w.table_bytes = inner.table_bytes;
    w.work = inner.work;
    movie.compressed = true;
    // The movie-level metadata is in the inflated data, not in the file:
    // read what the file's would give now.
    movie.inflated_gapless = Some(
        gapless::read(&mut SliceSource::new(&inflated), &movie.tag_boxes)
            .ok()
            .flatten(),
    );
    movie.tag_boxes.clear();
    Ok(movie)
}

/// Inflates the compressed movie box `cmov` of `source` into the bytes of
/// the movie box it holds, under the checks and bounds of probe
/// ([`crate::metadata`] reads the movie's metadata from them). The bytes
/// are not checked to be one movie box.
pub(crate) fn inflate_movie_box<S: Source>(
    source: S,
    cmov: &BoxHeader,
    limits: &Limits,
) -> Result<Vec<u8>, Error> {
    let mut w = Walker {
        source,
        limits,
        boxes: 0,
        retained: 0,
        table_bytes: 0,
        work: Work::new(limits),
    };
    inflate_compressed_moov(&mut w, cmov, 1).map(|(inflated, _)| inflated)
}

/// Reads the algorithm and data boxes of `cmov` and inflates the data:
/// the inflated bytes and the `cmvd` box.
fn inflate_compressed_moov<S: Source>(
    w: &mut Walker<'_, S>,
    cmov: &BoxHeader,
    depth: u32,
) -> Result<(Vec<u8>, BoxHeader), Error> {
    let mut algorithm: Option<FourCc> = None;
    let mut data: Option<BoxHeader> = None;
    w.children(cmov, depth, &mut |w, child, _| {
        match &child.kind.0 {
            b"dcom" if algorithm.is_none() => {
                let payload = w.payload(&child)?;
                algorithm = Some(Bytes::new(&payload, child.payload_offset()).fourcc()?);
            }
            b"cmvd" if data.is_none() => data = Some(child),
            _ => {}
        }
        Ok(())
    })?;
    let algorithm = algorithm.ok_or(invalid(cmov, "compressed movie without dcom"))?;
    if algorithm != ZLIB {
        return Err(Error::Unsupported {
            reason: "compressed movie algorithm other than zlib",
        });
    }
    let cmvd = data.ok_or(invalid(cmov, "compressed movie without cmvd"))?;
    let compressed_len = cmvd
        .payload_len()
        .checked_sub(4)
        .ok_or(invalid(&cmvd, "compressed movie data without a size"))?;
    let at = cmvd.payload_offset();
    let mut size = [0u8; 4];
    w.source.read_exact_at(at, &mut size)?;
    let declared = u64::from(u32::from_be_bytes(size));
    let limits = w.limits;
    if declared > limits.max_movie_bytes {
        return Err(Error::LimitExceeded {
            limit: LimitKind::MovieBytes,
            offset: cmvd.offset,
        });
    }
    if declared > compressed_len.saturating_mul(u64::from(limits.max_movie_ratio)) {
        return Err(Error::LimitExceeded {
            limit: LimitKind::MovieRatio,
            offset: cmvd.offset,
        });
    }
    // zlib's bound on what its compressor writes for `declared` bytes
    // (`compressBound`); longer data cannot be one stream of that size.
    let bound = declared
        .saturating_add(declared >> 12)
        .saturating_add(declared >> 14)
        .saturating_add(declared >> 25)
        .saturating_add(13);
    if compressed_len > bound {
        return Err(invalid(
            &cmvd,
            "compressed movie data longer than its size allows",
        ));
    }
    let start = at.checked_add(4).ok_or(Error::Overflow { offset: at })?;
    let inflated = inflate(w, start, compressed_len, declared, &cmvd)?;
    Ok((inflated, cmvd))
}

/// Bytes of compressed movie data read from the file at once.
const INFLATE_CHUNK: u64 = 64 << 10;

/// Inflates the `len` bytes of zlib data at `start` into exactly `size`
/// bytes, reading the file in chunks of [`INFLATE_CHUNK`]: memory is the
/// output, reserved once, and one chunk. Data that ends early, runs past
/// `size`, or fails its check is an invalid `cmvd`.
fn inflate<S: Source>(
    w: &mut Walker<'_, S>,
    start: u64,
    len: u64,
    size: u64,
    cmvd: &BoxHeader,
) -> Result<Vec<u8>, Error> {
    use miniz_oxide::inflate::TINFLStatus;
    use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
    let too_large = || Error::LimitExceeded {
        limit: LimitKind::MovieBytes,
        offset: cmvd.offset,
    };
    let size = usize::try_from(size).map_err(|_| too_large())?;
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|_| too_large())?;
    out.resize(size, 0);
    let mut state = Box::<DecompressorOxide>::default();
    let mut chunk = Vec::new();
    let end = start
        .checked_add(len)
        .ok_or(Error::Overflow { offset: start })?;
    let (mut at, mut written) = (start, 0usize);
    loop {
        let take = end.saturating_sub(at).min(INFLATE_CHUNK);
        chunk.resize(usize::try_from(take).map_err(|_| too_large())?, 0);
        w.source.read_exact_at(at, &mut chunk)?;
        let more = at.saturating_add(take) < end;
        let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
            | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF
            | inflate_flags::TINFL_FLAG_COMPUTE_ADLER32
            | if more {
                inflate_flags::TINFL_FLAG_HAS_MORE_INPUT
            } else {
                0
            };
        let (status, read, wrote) = decompress(&mut state, &chunk, &mut out, written, flags);
        at = at.saturating_add(read as u64);
        written = written.saturating_add(wrote);
        match status {
            TINFLStatus::Done if written == size => return Ok(out),
            TINFLStatus::Done | TINFLStatus::HasMoreOutput => {
                return Err(invalid(
                    cmvd,
                    "compressed movie data inflates to another size",
                ));
            }
            TINFLStatus::NeedsMoreInput if more && (read > 0 || wrote > 0) => {}
            _ => return Err(invalid(cmvd, "compressed movie data does not inflate")),
        }
    }
}

/// `error` reported at `offset`: for an error inside a compressed movie
/// box, whose own offsets are offsets in the inflated data.
fn at_box(error: Error, offset: u64) -> Error {
    match error {
        Error::Truncated { .. } => Error::Truncated { offset },
        Error::InvalidBox { kind, reason, .. } => Error::InvalidBox {
            offset,
            kind,
            reason,
        },
        Error::LimitExceeded { limit, .. } => Error::LimitExceeded { limit, offset },
        Error::Overflow { .. } => Error::Overflow { offset },
        other => other,
    }
}

/// What a dropped stream's track box says about its kind and timing.
struct DroppedTiming {
    kind: TrackKind,
    timescale: u32,
    mdhd_duration: u64,
    edit_list: EditList,
}

impl Default for DroppedTiming {
    fn default() -> Self {
        Self {
            kind: TrackKind::Data,
            timescale: 0,
            mdhd_duration: 0,
            edit_list: EditList::None,
        }
    }
}

#[derive(Default)]
struct TrackParts {
    tkhd: Option<Tkhd>,
    edits: Option<(EditList, Vec<Edit>)>,
    mdhd: Option<(u32, u64, Option<[u8; 3]>, u64)>,
    handler: Option<(FourCc, Vec<u8>)>,
    quicktime_handler: bool,
    description: Option<(SampleDescription, u32, u16)>,
    self_contained: Option<Vec<bool>>,
    tables: SampleTables,
    table_problem: Option<&'static str>,
    warnings: Vec<Warning>,
}

fn parse_trak<S: Source>(
    w: &mut Walker<'_, S>,
    trak: &BoxHeader,
    depth: u32,
    (index, stream_index): (usize, usize),
    allowlist: &Allowlist,
    (chapter_tracks, chapter_references): (&mut Vec<u32>, &mut Vec<u32>),
    dropped: &mut DroppedTiming,
) -> Result<Track, Error> {
    let mut parts = TrackParts::default();
    // A stream the allowlist drops is not parsed further, but its chapter
    // reference is still read: the chapter track is a track of its own.
    let mut not_allowed: Option<Error> = None;
    let mut chapters: Vec<u32> = Vec::new();
    let walked = w.children(trak, depth, &mut |w, child, depth| {
        match &child.kind.0 {
            b"tref" => track_references(w, &child, &mut chapters)?,
            // The edit list is still read after a drop: it places the
            // dropped stream on the timeline.
            b"edts" => w.children(&child, depth, &mut |w, e, _| {
                if e.kind == FourCc(*b"elst") {
                    let (list, entries) = parse_elst(&w.payload(&e)?, e.payload_offset())?;
                    w.retain(entries.len().saturating_mul(Edit::RETAINED_BYTES), e.offset)?;
                    parts.edits = Some((list, entries));
                }
                Ok(())
            })?,
            _ if not_allowed.is_some() => {}
            b"tkhd" => parts.tkhd = Some(parse_tkhd(&w.payload(&child)?, child.payload_offset())?),
            b"mdia" => match parse_mdia(w, &child, depth, &mut parts, allowlist) {
                Err(Error::CodecNotAllowed { fourcc, .. }) => {
                    // `minf` is read after `hdlr` (see `parse_mdia`), so the
                    // handler is known.
                    if let Some((handler, _)) = &parts.handler {
                        dropped.kind = handler_kind(*handler);
                    }
                    let error = Error::CodecNotAllowed {
                        track_id: parts.tkhd.map_or(0, |t| t.track_id),
                        fourcc,
                    };
                    if allowlist.is_strict() {
                        return Err(error);
                    }
                    not_allowed = Some(error);
                }
                other => other?,
            },
            b"trak" => return Err(invalid(&child, "track inside a track")),
            _ => {}
        }
        Ok(())
    });
    // A dropped stream was never parsed past its media box: a defect after
    // it ends the search for references instead of rejecting the file. A
    // source that fails to read fails the probe either way.
    if not_allowed.is_none() || matches!(walked, Err(Error::Io(_))) {
        walked?;
    }
    // QuickTime File Format, "Chapter lists": the track holding the
    // reference must be enabled. A dropped stream whose `tkhd` came after
    // its sample description has no known flags; its reference is kept.
    let enabled = parts.tkhd.is_none_or(|t| t.enabled);
    for id in chapters {
        if enabled && chapter_tracks.len() < MAX_CHAPTER_REFERENCES && !chapter_tracks.contains(&id)
        {
            chapter_tracks.push(id);
        }
        if chapter_references.len() < MAX_CHAPTER_REFERENCES && !chapter_references.contains(&id) {
            chapter_references.push(id);
        }
    }
    if let Some(error) = not_allowed {
        if let Some((timescale, duration, ..)) = parts.mdhd {
            dropped.timescale = timescale;
            dropped.mdhd_duration = duration;
        }
        dropped.edit_list = parts.edits.map_or(EditList::None, |(list, _)| list);
        return Err(error);
    }
    let Tkhd {
        track_id,
        matrix,
        size: presentation_size,
        created: creation_time,
        ..
    } = parts.tkhd.ok_or(invalid(trak, "missing track header"))?;
    let (timescale, mdhd_duration, language, media_creation_time) =
        parts.mdhd.ok_or(invalid(trak, "missing media header"))?;
    let (handler, handler_name_bytes) = parts.handler.ok_or(invalid(trak, "missing handler"))?;
    let (description, descriptions, data_reference) = parts
        .description
        .ok_or(invalid(trak, "missing sample description"))?;
    let kind = handler_kind(handler);
    let external_data = parts
        .self_contained
        .as_ref()
        .and_then(|entries| {
            entries
                .get(usize::from(data_reference).checked_sub(1)?)
                .copied()
        })
        .is_some_and(|self_contained| !self_contained);
    let mut warnings = parts.warnings;
    if external_data {
        crate::error::warn(&mut warnings, Warning::ExternalData);
    }
    let mut sample_duration = 0u64;
    let mut problem = parts.table_problem;
    // One frame size for every sample: only with one sample description.
    if problem.is_none() && description.codec == Codec::Pcm && descriptions == 1 {
        if let Some(frame_bytes) = description.frame_bytes {
            match parts.tables.uncompressed_audio(frame_bytes, &mut w.work) {
                Ok(()) => {}
                Err("work") => {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::SampleWork,
                        offset: trak.offset,
                    });
                }
                Err(reason) => problem = Some(reason),
            }
        }
    }
    if problem.is_none() {
        match parts
            .tables
            .validate(descriptions, trak.offset, &mut w.work)
        {
            Ok(duration) => sample_duration = duration,
            Err("work") => {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::SampleWork,
                    offset: trak.offset,
                });
            }
            Err(reason) => problem = Some(reason),
        }
    }
    if let Some(reason) = problem {
        crate::error::warn(&mut warnings, Warning::InvalidSampleTable(reason));
    }
    let stsz_bytes = if problem.is_none() {
        stsz_total(&parts.tables, &mut w.work, trak.offset)?
    } else {
        None
    };
    // The warning for an unsupported list is given in `parse_moov`, once the
    // movie timescale tells whether several edits apply (`EditList::Multiple`).
    let (edit_list, edits) = parts.edits.unwrap_or((EditList::None, Vec::new()));
    Ok(Track {
        index,
        stream_index,
        track_id,
        kind,
        handler,
        // Decoded with the file's kind (`handler_name_bytes`).
        handler_name: None,
        language,
        timescale,
        mdhd_duration,
        sample_duration,
        duration_source: DurationSource::Unknown,
        stts_duration: sample_duration,
        duration: None,
        edit_list,
        edits,
        matrix,
        presentation_size,
        rotation: rotation(&matrix),
        description,
        descriptions,
        sample_count: parts.tables.sample_count(),
        fragment_samples: 0,
        fragment_start: None,
        first_fragment_bytes: None,
        external_data,
        tables_valid: problem.is_none(),
        sample_bytes: stsz_bytes,
        creation_time,
        media_creation_time,
        warnings,
        tables: parts.tables,
        stsz_bytes,
        trex: None,
        fragment_first_offset: None,
        quicktime_handler: parts.quicktime_handler,
        handler_name_bytes,
    })
}

/// Adds the `track_ID`s of the `chap` references in `tref` (ISO/IEC
/// 14496-12 8.3.3; the `chap` type is from Apple's QuickTime File Format) to
/// `chapter_tracks`, the references of one track. Chapters are optional: a
/// `tref` that is too large or malformed is ignored rather than rejecting
/// the file; a source that fails to read it fails the probe.
fn track_references<S: Source>(
    w: &mut Walker<'_, S>,
    tref: &BoxHeader,
    chapter_tracks: &mut Vec<u32>,
) -> Result<(), Error> {
    if tref.size > MAX_TREF_BYTES {
        return Ok(());
    }
    let Some(payload) = w.payload(tref).map(Some).or_else(crate::gapless::absent)? else {
        return Ok(());
    };
    let mut entries = Bytes::new(&payload, tref.payload_offset());
    while let (Ok(size), Ok(kind)) = (entries.u32(), entries.fourcc()) {
        let Some(ids) = (size as usize)
            .checked_sub(8)
            .and_then(|len| entries.take(len).ok())
        else {
            break;
        };
        if kind != FourCc(*b"chap") {
            continue;
        }
        // Apple allows 0 for an unused entry.
        for id in ids.chunks_exact(4) {
            let id = crate::bytes::be_u32_at(id, 0).unwrap_or(0);
            if id == 0 || chapter_tracks.contains(&id) {
                continue;
            }
            // At most 64 IDs per track and per movie: a fixed bound, so the
            // references are not charged to `max_retained_bytes`, whose room
            // the rest of the file needs as before.
            if chapter_tracks.len() >= MAX_CHAPTER_REFERENCES {
                return Ok(());
            }
            chapter_tracks.push(id);
        }
    }
    Ok(())
}

/// The track kind a handler type names.
fn handler_kind(handler: FourCc) -> TrackKind {
    match &handler.0 {
        b"vide" => TrackKind::Video,
        b"soun" => TrackKind::Audio,
        b"sbtl" | b"subt" | b"text" => TrackKind::Subtitle,
        b"tmcd" => TrackKind::Timecode,
        _ => TrackKind::Data,
    }
}

fn parse_mdia<S: Source>(
    w: &mut Walker<'_, S>,
    mdia: &BoxHeader,
    depth: u32,
    parts: &mut TrackParts,
    allowlist: &Allowlist,
) -> Result<(), Error> {
    // The sample entry layout depends on the handler, and the children of
    // `mdia` may come in any order, so `minf` is parsed after the others.
    let mut minf: Option<(BoxHeader, u32)> = None;
    w.children(mdia, depth, &mut |w, child, depth| {
        match &child.kind.0 {
            b"mdhd" => {
                let payload = w.payload(&child)?;
                let mut b = Bytes::new(&payload, child.payload_offset());
                let (version, created, timescale, duration) = media_times(&mut b)?;
                if timescale == 0 {
                    return Err(invalid(&child, "zero timescale"));
                }
                let duration = unknown_duration(version, duration);
                let packed = b.u16()?;
                let language = (packed >= 0x400).then(|| {
                    [10u16, 5, 0]
                        .map(|shift| (((packed >> shift) & 0x1f) as u8).saturating_add(0x60))
                });
                parts.mdhd = Some((timescale, duration, language, created));
            }
            b"hdlr" => {
                let payload = w.payload(&child)?;
                let mut b = Bytes::new(&payload, child.payload_offset());
                b.skip(4)?;
                // `pre_defined` in ISO/IEC 14496-12 8.4.3 (always 0); the
                // QuickTime File Format's component type, `mhlr` for a
                // media handler.
                parts.quicktime_handler = b.fourcc()? == FourCc(*b"mhlr");
                let handler = b.fourcc()?;
                b.skip(12).ok();
                let name = b.rest();
                let name = name.get(..name.len().min(256)).unwrap_or_default().to_vec();
                w.retain(name.len(), child.offset)?;
                parts.handler = Some((handler, name));
            }
            b"minf" => {
                if minf.is_some() {
                    return Err(invalid(&child, "second media information box"));
                }
                minf = Some((child, depth));
            }
            _ => {}
        }
        Ok(())
    })?;
    let Some((minf, depth)) = minf else {
        return Ok(());
    };
    w.children(&minf, depth, &mut |w, child, depth| {
        match &child.kind.0 {
            b"dinf" => w.children(&child, depth, &mut |w, d, _| {
                if d.kind == FourCc(*b"dref") {
                    parts.self_contained = Some(parse_dref(&w.payload(&d)?, d.payload_offset())?);
                }
                Ok(())
            })?,
            b"stbl" => parse_stbl(w, &child, depth, parts, allowlist)?,
            _ => {}
        }
        Ok(())
    })
}

fn parse_stbl<S: Source>(
    w: &mut Walker<'_, S>,
    stbl: &BoxHeader,
    depth: u32,
    parts: &mut TrackParts,
    allowlist: &Allowlist,
) -> Result<(), Error> {
    w.children(stbl, depth, &mut |w, child, _| {
        if child.kind == FourCc(*b"stsd") {
            let layout = match parts.handler.as_ref().map(|h| h.0.0) {
                Some(kind) if &kind == b"vide" => EntryLayout::Visual,
                Some(kind) if &kind == b"soun" => EntryLayout::Audio,
                _ => EntryLayout::Other,
            };
            let payload = w.payload(&child)?;
            let parsed = parse_stsd(
                &payload,
                child.payload_offset(),
                layout,
                allowlist,
                &mut parts.warnings,
            )?;
            if parts.description.is_some() {
                return Err(invalid(&child, "second sample description box"));
            }
            let kept = parsed
                .0
                .codec_config
                .iter()
                .chain(&parsed.0.entry_boxes)
                .fold(0usize, |sum, c| sum.saturating_add(c.data.len()));
            w.retain(kept, child.offset)?;
            parts.description = Some(parsed);
            return Ok(());
        }
        if child.kind == FourCc(*b"stz2") {
            parts
                .table_problem
                .get_or_insert("compact sample sizes (stz2) are not supported");
            return Ok(());
        }
        let Some(kind) = TableKind::from_fourcc(child.kind) else {
            return Ok(());
        };
        let head_len = child.payload_len().min(16);
        let head = w.read(child.payload_offset(), head_len, LimitKind::BoxBytes, 16)?;
        let budget = w.limits.max_table_bytes.saturating_sub(w.table_bytes);
        let header = table_header(kind, &head, child.payload_len(), child.offset, budget)?;
        let fixed = if kind == TableKind::Stsz { 12 } else { 8 };
        let entries = w.read(
            child.payload_offset().saturating_add(fixed),
            header.entry_bytes,
            LimitKind::TableBytes,
            budget,
        )?;
        w.table_bytes = w.table_bytes.saturating_add(header.entry_bytes);
        if !parts.tables.set(&header, entries) {
            return Err(invalid(&child, "duplicate sample table"));
        }
        Ok(())
    })
}

fn parse_stsd(
    payload: &[u8],
    offset: u64,
    layout: EntryLayout,
    allowlist: &Allowlist,
    warnings: &mut Vec<Warning>,
) -> Result<(SampleDescription, u32, u16), Error> {
    let mut b = Bytes::new(payload, offset);
    b.version_flags()?;
    let count = b.u32()?;
    if count == 0 {
        return Err(Error::InvalidBox {
            offset,
            kind: FourCc(*b"stsd"),
            reason: "no sample descriptions",
        });
    }
    let mut first = None;
    for _ in 0..count {
        let entry_offset = b.offset();
        let size = b.clone().u32()?;
        if size < 16 {
            return Err(Error::InvalidBox {
                offset: entry_offset,
                kind: FourCc(*b"stsd"),
                reason: "sample entry too small",
            });
        }
        let entry = b.take(size as usize)?;
        let description = parse_sample_entry(entry, entry_offset, layout, warnings)?;
        if !allowlist.allows(description.codec) {
            return Err(Error::CodecNotAllowed {
                track_id: 0,
                fourcc: description.fourcc,
            });
        }
        if first.is_none() {
            let data_reference = crate::bytes::be_u16_at(entry, 14).unwrap_or(1);
            first = Some((description, data_reference));
        }
    }
    if count > 1 {
        crate::error::warn(warnings, Warning::MultipleSampleEntries);
    }
    let (description, data_reference) = first.ok_or(Error::Truncated { offset })?;
    Ok((description, count, data_reference))
}

#[derive(Clone, Copy)]
struct Tkhd {
    track_id: u32,
    /// `track_enabled` (flag 1).
    enabled: bool,
    matrix: [i32; 9],
    size: (u32, u32),
    created: u64,
}

/// The identity transformation matrix (ISO/IEC 14496-12 8.2.2.3).
pub const IDENTITY_MATRIX: [i32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

/// Version, creation time, timescale and duration of `mvhd` or `mdhd`
/// (ISO/IEC 14496-12 8.2.2 and 8.4.2); the modification time is skipped.
fn media_times(b: &mut Bytes<'_>) -> Result<(u8, u64, u32, u64), Error> {
    let (version, _) = b.version_flags()?;
    if version == 1 {
        let created = b.u64()?;
        b.skip(8)?;
        Ok((version, created, b.u32()?, b.u64()?))
    } else {
        let created = u64::from(b.u32()?);
        b.skip(4)?;
        Ok((version, created, b.u32()?, u64::from(b.u32()?)))
    }
}

/// All-ones durations mean "unknown" and are reported as 0.
fn unknown_duration(version: u8, duration: u64) -> u64 {
    if duration == u64::MAX || (version == 0 && duration == u64::from(u32::MAX)) {
        0
    } else {
        duration
    }
}

/// Sum of the `stsz` sample sizes: one step for a constant
/// size, one work visit per listed entry. `None` when a listed entry is
/// missing; overflow is an error.
fn stsz_total(tables: &SampleTables, work: &mut Work, offset: u64) -> Result<Option<u64>, Error> {
    let Some(stsz) = tables.stsz.as_ref() else {
        return Ok(None);
    };
    if stsz.constant != 0 {
        return u64::from(stsz.constant)
            .checked_mul(u64::from(stsz.count))
            .map(Some)
            .ok_or(Error::Overflow { offset });
    }
    work.add(u64::from(stsz.count))?;
    if stsz.entries.len() as u64 != u64::from(stsz.count).saturating_mul(4) {
        return Ok(None);
    }
    Ok(Some(sum_be_u32(&stsz.entries)))
}

/// The sum of the big-endian `u32` values in `entries` (whole entries; a
/// trailing partial one is ignored). At most 2^32 - 1 values of at most
/// 2^32 - 1: the sum stays below 2^64, so it needs no check per entry.
///
/// Each entry is read as a little-endian word, and its four bytes are summed
/// in separate 16-bit lanes of two 32-bit accumulators, 256 entries at a
/// time (256 × 255 fits 16 bits), then weighted by their places: no byte
/// swap per entry, which WebAssembly has no instruction for (tables of
/// a million entries).
fn sum_be_u32(entries: &[u8]) -> u64 {
    let mut total = 0u64;
    for block in entries.chunks(4 * 256) {
        // Bytes 0 and 2 of each entry (the high byte and the third), and
        // bytes 1 and 3, each pair in the low and high halves of a word.
        let (mut even, mut odd) = (0u32, 0u32);
        for entry in block.chunks_exact(4) {
            let word = <[u8; 4]>::try_from(entry).map_or(0, u32::from_le_bytes);
            even = even.wrapping_add(word & 0x00FF_00FF);
            odd = odd.wrapping_add((word >> 8) & 0x00FF_00FF);
        }
        let b0 = u64::from(even & 0xFFFF);
        let b2 = u64::from(even >> 16);
        let b1 = u64::from(odd & 0xFFFF);
        let b3 = u64::from(odd >> 16);
        total = total
            .wrapping_add(b0 << 24)
            .wrapping_add(b1 << 16)
            .wrapping_add(b2 << 8)
            .wrapping_add(b3);
    }
    total
}

fn parse_tkhd(payload: &[u8], offset: u64) -> Result<Tkhd, Error> {
    let mut b = Bytes::new(payload, offset);
    let (version, flags) = b.version_flags()?;
    let (created, track_id) = if version == 1 {
        let created = b.u64()?;
        b.skip(8)?;
        let id = b.u32()?;
        b.skip(12)?;
        (created, id)
    } else {
        let created = u64::from(b.u32()?);
        b.skip(4)?;
        let id = b.u32()?;
        b.skip(8)?;
        (created, id)
    };
    b.skip(16)?;
    let mut matrix = [0i32; 9];
    for value in &mut matrix {
        *value = b.i32()?;
    }
    // Width and height close the box; a box without them still parses, as
    // it did before they were read.
    let width = b.u32().unwrap_or(0);
    let height = b.u32().unwrap_or(0);
    Ok(Tkhd {
        track_id,
        enabled: flags & 1 == 1,
        matrix,
        size: (width, height),
        created,
    })
}

/// Whether an edit list outside the simple model is applied exactly
/// (`docs/design/edit-lists.md`, decision 1): every entry an empty edit or
/// a media edit at rate 1 with a duration, and the media edits' windows
/// `[media_time, media_time + duration)`, the duration converted exactly
/// from the movie timescale, in increasing order without overlap.
fn several_edits_apply(edits: &[Edit], timescale: u32, movie_timescale: u32) -> bool {
    if edits.is_empty() || timescale == 0 || movie_timescale == 0 {
        return false;
    }
    // Every entry's start on the presentation timeline must be representable.
    if edits
        .iter()
        .try_fold(0u64, |sum, e| sum.checked_add(e.segment_duration))
        .is_none()
    {
        return false;
    }
    // A window's end compared with the next media time, both scaled by the
    // movie timescale: media_time × M + duration × T.
    let (t, m) = (i128::from(timescale), i128::from(movie_timescale));
    let mut previous_end: Option<i128> = None;
    for edit in edits {
        if edit.is_empty_edit() {
            continue;
        }
        if edit.media_time < 0 || edit.media_rate != Edit::UNIT_RATE || edit.segment_duration == 0 {
            return false;
        }
        let start = i128::from(edit.media_time) * m;
        if previous_end.is_some_and(|end| start < end) {
            return false;
        }
        previous_end = Some(start + i128::from(edit.segment_duration) * t);
    }
    previous_end.is_some()
}

fn parse_elst(payload: &[u8], offset: u64) -> Result<(EditList, Vec<Edit>), Error> {
    let mut b = Bytes::new(payload, offset);
    let (version, _) = b.version_flags()?;
    let count = b.u32()?;
    let width: u64 = if version == 1 { 20 } else { 12 };
    if u64::from(count).saturating_mul(width) > b.remaining() as u64 {
        return Err(Error::InvalidBox {
            offset,
            kind: FourCc(*b"elst"),
            reason: "entry count exceeds the box",
        });
    }
    // The count was checked against the box, so the entries are no more
    // than the payload already read; the caller charges them as retained.
    let mut edits = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for _ in 0..count {
        let (segment_duration, media_time) = if version == 1 {
            (b.u64()?, b.i64()?)
        } else {
            (u64::from(b.u32()?), i64::from(b.i32()?))
        };
        let media_rate = b.i32()?;
        edits.push(Edit {
            segment_duration,
            media_time,
            media_rate,
        });
    }
    let list = match edits.as_slice() {
        [] => EditList::None,
        // A media edit with a duration of 0 lasts to the end of the media
        // (in a movie with fragments; `parse_moov` checks that).
        [media] if media.media_time >= 0 && media.media_rate == Edit::UNIT_RATE => {
            EditList::Simple {
                empty: 0,
                media_time: media.media_time,
                segment: media.segment_duration,
            }
        }
        [empty, media]
            if empty.is_empty_edit()
                && media.media_time >= 0
                && media.media_rate == Edit::UNIT_RATE =>
        {
            EditList::Simple {
                empty: empty.segment_duration,
                media_time: media.media_time,
                segment: media.segment_duration,
            }
        }
        _ => EditList::Unsupported,
    };
    Ok((list, edits))
}

fn parse_dref(payload: &[u8], offset: u64) -> Result<Vec<bool>, Error> {
    let mut b = Bytes::new(payload, offset);
    b.version_flags()?;
    let count = b.u32()?;
    let mut entries = Vec::new();
    for _ in 0..count {
        let size = b.u32()?;
        b.fourcc()?;
        let body = b.take(
            (size as usize)
                .checked_sub(8)
                .ok_or(Error::Truncated { offset })?,
        )?;
        let flags = Bytes::new(body, 0)
            .version_flags()
            .map(|(_, f)| f)
            .unwrap_or(0);
        entries.push(flags & 1 == 1);
    }
    Ok(entries)
}

/// The `hdlr` name: QuickTime stores a counted string (a length byte equal
/// to the rest of the field), ISO/IEC 14496-12 a null-terminated one, whose
/// first byte is text even when it happens to equal the length.
fn handler_name(raw: &[u8], quicktime: bool) -> Option<String> {
    let text = match raw.split_first() {
        Some((&len, tail)) if quicktime && usize::from(len) == tail.len() && len > 0 => tail,
        _ => raw.split(|b| *b == 0).next().unwrap_or_default(),
    };
    (!text.is_empty()).then(|| String::from_utf8_lossy(text).into_owned())
}

fn rotation(m: &[i32; 9]) -> Option<u16> {
    const ONE: i32 = 0x0001_0000;
    const MINUS_ONE: i32 = -ONE;
    let [a, b, u, c, d, v, _, _, w] = *m;
    if u != 0 || v != 0 || w != 0x4000_0000 {
        return None;
    }
    match (a, b, c, d) {
        (ONE, 0, 0, ONE) => Some(0),
        (0, ONE, MINUS_ONE, 0) => Some(90),
        (MINUS_ONE, 0, 0, MINUS_ONE) => Some(180),
        (0, MINUS_ONE, ONE, 0) => Some(270),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    /// A counted string only in QuickTime files; in ISO files the first byte
    /// is text, even when it equals the length of the rest.
    #[test]
    fn handler_names_follow_the_files_kind() {
        use super::handler_name;
        let counted = b"\x0bGoPro H.265";
        assert_eq!(handler_name(counted, true).as_deref(), Some("GoPro H.265"));
        assert_eq!(
            handler_name(counted, false).as_deref(),
            Some("\u{b}GoPro H.265")
        );
        // A count that spans padding NULs: ISO stops at the first NUL.
        let padded = b"\x10DJI.AVC\0\0\0\0\0\0\0\0\0";
        assert_eq!(
            handler_name(padded, false).as_deref(),
            Some("\u{10}DJI.AVC")
        );
        // A QuickTime name whose first byte is not its length is read up to
        // the NUL, as before.
        assert_eq!(
            handler_name(b"VideoHandler\0", true).as_deref(),
            Some("VideoHandler")
        );
        assert_eq!(
            handler_name(b"VideoHandler\0", false).as_deref(),
            Some("VideoHandler")
        );
        assert_eq!(handler_name(b"\0", false), None);
        assert_eq!(handler_name(b"", true), None);
    }

    /// The lane sum equals the plain big-endian sum, around block bounds
    /// and with every byte at its largest.
    #[test]
    fn stsz_lane_sum_matches_the_plain_sum() {
        use super::sum_be_u32;
        let plain = |entries: &[u8]| -> u64 {
            entries
                .chunks_exact(4)
                .map(|e| u64::from(u32::from_be_bytes([e[0], e[1], e[2], e[3]])))
                .sum()
        };
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for count in [0usize, 1, 255, 256, 257, 511, 512, 1000, 4097] {
            let random: Vec<u8> = (0..count * 4).map(|_| next() as u8).collect();
            assert_eq!(sum_be_u32(&random), plain(&random), "{count} random");
            let full = vec![0xFF; count * 4];
            assert_eq!(sum_be_u32(&full), plain(&full), "{count} at the largest");
            // A trailing partial entry is ignored.
            let mut partial = random.clone();
            partial.extend_from_slice(&[1, 2, 3]);
            assert_eq!(sum_be_u32(&partial), plain(&random), "{count} with a tail");
        }
    }
    use super::{Edit, several_edits_apply};

    fn media(segment_duration: u64, media_time: i64) -> Edit {
        Edit {
            segment_duration,
            media_time,
            media_rate: Edit::UNIT_RATE,
        }
    }

    #[test]
    fn several_edits_need_representable_starts() {
        assert!(several_edits_apply(
            &[media(40, 0), media(40, 1024)],
            12_800,
            1000
        ));
        // The second edit's start on the presentation timeline does not fit
        // in 64 bits, so its packets could not be mapped.
        let half = 1u64 << 63;
        assert!(!several_edits_apply(
            &[media(half, 0), media(half, 6_000_000_000_000_000_000)],
            1000,
            1000
        ));
    }
}
