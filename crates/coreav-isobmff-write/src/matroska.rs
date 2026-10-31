//! Matroska and WebM to MP4 by stream copy (`docs/design/mkv-remux.md`).
//!
//! The source is read by `coreav-mkv` under its limits and allowlist; the
//! output is written by the shared [`Muxer`] in two passes. The first pass
//! lists every packet once and keeps a small record per kept sample; the
//! second copies the samples in the writer's chunk order through one
//! [`CopyWindow`]. Nothing is decoded.

use crate::audio;
use crate::mux::{
    CopyWindow, Edit, FileType, MovieHeader, Muxer, RawBox, Sample, TrackHeader, TrackMedia,
    audio_sample_entry, esds, visual_sample_entry,
};
use crate::replace::{
    Replacement, Replacements, audio_edits, roll_boxes, validate, validate_audio,
};
use crate::{ExtractError, OnUnsupported, TrackDropReason};
use core::fmt;
use coreav_isobmff::{FourCc, Profile};
use coreav_mkv::{Allowlist, Codec, Encoding, Segment, SliceSource, Source, Track, TrackKind};
use std::io::Write;

/// Bytes charged per kept sample against
/// [`coreav_isobmff::Limits::max_table_bytes`]: its record, kept between the
/// passes, and the per-sample vectors of the time computation.
const SAMPLE_BYTES: u64 = core::mem::size_of::<Record>() as u64 + 64;

/// Longest parameter set kept from an Annex B keyframe (its length is a
/// 16-bit field of the `avcC`).
const MAX_PARAMETER_SET: u64 = 0xFFFF;

/// Most NAL units in one Annex B sample.
const MAX_NAL_UNITS: usize = 1 << 16;

/// Largest time, in nanoseconds (about 146 years), kept from the source; a
/// stream with a time beyond it is dropped, so that no time computation
/// overflows.
const MAX_TIME_NS: u64 = 1 << 62;

/// Movie and video timescale when a Segment Tick does not divide a second.
const FALLBACK_TIMESCALE: u32 = 90_000;

/// Largest first frame of an AC-3 or E-AC-3 stream read to configure its
/// sample entry: an E-AC-3 sample of eight independent substreams, each with
/// eight dependent ones, in syncframes of up to 4,096 bytes.
const MAX_FIRST_FRAME: u32 = 8 * 9 * 4096;

/// Bytes of a FLAC frame header read to find its block size: the longest
/// header up to the block size (RFC 9639 9.1).
const FLAC_HEAD: usize = 13;

/// Why a Matroska remux failed. Nothing about the source's bytes is
/// included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatroskaRemuxError {
    /// The source could not be read, is malformed, exceeds a demux limit,
    /// or has nothing MP4 can carry (`coreav_mkv::Error::Unsupported`).
    Source(coreav_mkv::Error),
    /// The output exceeds a writer limit or cannot be represented.
    Output(coreav_isobmff::Error),
    /// Writing the output failed; the output is incomplete.
    Write(std::io::ErrorKind),
    /// The source returned different bytes in the copy pass; the output is
    /// incomplete.
    SourceChanged,
    /// A video stream cannot be copied, and
    /// [`MatroskaRemuxOptions::drop_video`] is not set: the output would
    /// silently lose its picture. Nothing is written.
    VideoNotCopied {
        /// Position of the stream among all `TrackEntry` elements.
        stream_index: usize,
        /// Why it cannot be copied.
        reason: &'static str,
    },
}

impl fmt::Display for MatroskaRemuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(f),
            Self::Output(error) => error.fmt(f),
            Self::Write(kind) => write!(f, "write error: {kind}"),
            Self::SourceChanged => f.write_str("the source changed during the remux"),
            Self::VideoNotCopied {
                stream_index,
                reason,
            } => write!(
                f,
                "video stream {stream_index} cannot be copied to MP4: {reason}"
            ),
        }
    }
}

impl std::error::Error for MatroskaRemuxError {}

impl From<coreav_mkv::Error> for MatroskaRemuxError {
    fn from(error: coreav_mkv::Error) -> Self {
        Self::Source(error)
    }
}

impl From<coreav_isobmff::Error> for MatroskaRemuxError {
    fn from(error: coreav_isobmff::Error) -> Self {
        match error {
            // Reads go through the Matroska source; report them as such.
            coreav_isobmff::Error::Io(kind) => Self::Source(coreav_mkv::Error::Io(kind)),
            coreav_isobmff::Error::Truncated { offset } => {
                Self::Source(coreav_mkv::Error::Truncated { offset })
            }
            other => Self::Output(other),
        }
    }
}

impl From<ExtractError> for MatroskaRemuxError {
    fn from(error: ExtractError) -> Self {
        match error {
            ExtractError::Source(error) => error.into(),
            ExtractError::Write(kind) => Self::Write(kind),
            ExtractError::SourceChanged => Self::SourceChanged,
            // Only HLS indexes produce this; the remux never reads one.
            ExtractError::InvalidIndex { reason } => {
                Self::Output(coreav_isobmff::Error::Unsupported { reason })
            }
        }
    }
}

impl From<std::io::Error> for MatroskaRemuxError {
    fn from(error: std::io::Error) -> Self {
        Self::Write(error.kind())
    }
}

/// One stream of the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatroskaRemuxTrack {
    /// Position of the track among all `TrackEntry` elements of the source.
    pub stream_index: usize,
    /// `TrackNumber` in the source.
    pub track_number: u64,
    /// `track_ID` in the output, numbered from 1 in output order.
    pub track_id: u32,
    /// The track's kind (video or audio).
    pub kind: TrackKind,
    /// The track's codec.
    pub codec: Codec,
    /// Samples copied: every frame of the source track.
    pub samples: u32,
    /// Media timescale of the output track.
    pub timescale: u32,
    /// Sum of the decode durations, in media ticks.
    pub duration: u64,
    /// The samples were rewritten from Annex B start codes to NAL unit
    /// lengths (H.264 without `CodecPrivate`).
    pub annex_b: bool,
    /// The stream's samples are the replacement's of
    /// [`remux_matroska_replacing`], not the source's; `codec` is then the
    /// replacement's.
    pub replaced: bool,
}

/// A stream of the source that is not in the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatroskaDroppedTrack {
    /// Position of the track among all `TrackEntry` elements of the source.
    pub stream_index: usize,
    /// `TrackNumber` in the source.
    pub track_number: u64,
    /// The track's codec.
    pub codec: Codec,
    /// Why.
    pub reason: TrackDropReason,
}

/// A `BlockAdditions` element of a kept stream that the output does not
/// carry: MP4 has no place for per-frame additions such as a VP8 or VP9
/// alpha plane, so the frames are copied without them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedAdditions {
    /// The kept stream the element belongs to.
    pub stream_index: usize,
    /// Absolute offset of the element in the source.
    pub offset: u64,
    /// Size of the element, header included.
    pub size: u64,
}

/// What [`remux_matroska`] copies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatroskaRemuxOptions {
    /// Stream indexes to keep (`Track::stream_index`), in the output's
    /// order, a stream named twice written as two tracks; `None` for every
    /// stream that can be copied, in source order. A named stream that
    /// cannot be copied fails the remux; streams not named are left out.
    pub streams: Option<Vec<usize>>,
    /// What happens to a stream that cannot be copied:
    /// [`OnUnsupported::Drop`] leaves audio, subtitle and data streams out
    /// and lists them, [`OnUnsupported::Fail`] fails on any.
    pub on_unsupported: OnUnsupported,
    /// Also drop a video stream that cannot be copied, or that the
    /// allowlist leaves out, when no streams are named. Without it such a
    /// stream fails the remux ([`MatroskaRemuxError::VideoNotCopied`]), so
    /// that a video never silently becomes an audio-only file.
    pub drop_video: bool,
}

/// Most dropped `BlockAdditions` elements listed in a summary.
pub const MAX_LISTED_ADDITIONS: usize = 256;

/// What [`remux_matroska`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatroskaRemuxSummary {
    /// The kept streams, in output order (`MatroskaRemuxOptions::streams`).
    pub tracks: Vec<MatroskaRemuxTrack>,
    /// The streams of the source that are not in the output, in source
    /// order.
    pub dropped_tracks: Vec<MatroskaDroppedTrack>,
    /// `BlockAdditions` of kept streams that are not in the output, in file
    /// order: at most [`MAX_LISTED_ADDITIONS`].
    pub dropped_additions: Vec<DroppedAdditions>,
    /// Dropped `BlockAdditions` beyond those listed.
    pub dropped_additions_not_listed: u64,
    /// The source ends inside an element at this offset (a recording cut
    /// off): every complete frame before it is copied.
    pub truncated_at: Option<u64>,
    /// Bytes written to the output.
    pub bytes_written: u64,
}

/// What a kept track becomes.
struct Plan {
    track: usize,
    codec: Codec,
    media: Media,
    /// Media timescale.
    timescale: u32,
    /// Frame quantum for audio durations, in media ticks.
    quantum: u64,
    /// H.264 without `CodecPrivate`: Annex B samples.
    annex_b: bool,
    /// Annex B: a later keyframe has other parameter sets than the first
    /// (a resolution change), so the sample entry is `avc3`, which tells
    /// readers to take them from the samples.
    avc3: bool,
    /// Header-stripping prefix written before each sample.
    prefix: Vec<u8>,
    samples: Records,
    /// Parameter sets from the first keyframe (Annex B), in order.
    parameter_sets: Vec<Vec<u8>>,
    /// The `dac3` or `dec3` payload, built from the first frame of an
    /// AC-3 or E-AC-3 stream.
    audio_config: Vec<u8>,
    /// Apple Lossless: the tag of the one channel element every packet
    /// holds, 0 for mono or 1 for a stereo pair.
    alac_element: u32,
    /// Why the stream cannot be copied after all, found while listing it.
    failure: Option<&'static str>,
    /// `DiscardPadding` of the last frame, in nanoseconds: not presented.
    end_padding_ns: i64,
    /// The sample entry, built after the listing.
    entry: Vec<u8>,
    /// The output times, computed after the listing.
    timing: Option<Timing>,
    /// The stream's samples are a replacement's: none of the source's
    /// frames is listed or copied.
    replaced: Option<Replaced>,
}

impl Plan {
    /// Drops the stream with `reason`, and its records.
    fn fail(&mut self, reason: &'static str) {
        self.failure = Some(reason);
        self.samples = Records::default();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Media {
    Video,
    Audio,
}

/// The records of a stream's samples, in chunks of [`RECORD_CHUNK`]: a
/// growing list never copies the records it holds, so memory peaks at the
/// records themselves rather than at a reallocation's old and new buffers
/// together, which for a long audio track (hundreds of thousands of
/// frames) is three times as much, and the WebAssembly build's memory
/// keeps its peak.
#[derive(Default)]
struct Records {
    chunks: Vec<Vec<Record>>,
    len: usize,
}

/// Records per chunk of [`Records`] (1 MiB).
const RECORD_CHUNK: usize = 1 << 14;

impl Records {
    fn push(&mut self, record: Record) {
        match self.chunks.last_mut() {
            Some(chunk) if chunk.len() < RECORD_CHUNK => chunk.push(record),
            _ => {
                // The first chunk grows as a vector does, so that a short
                // stream takes no more than its records; the later ones
                // are allocated whole.
                let mut chunk = if self.chunks.is_empty() {
                    Vec::new()
                } else {
                    Vec::with_capacity(RECORD_CHUNK)
                };
                chunk.push(record);
                self.chunks.push(chunk);
            }
        }
        self.len += 1;
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn get(&self, index: usize) -> Option<&Record> {
        self.chunks
            .get(index / RECORD_CHUNK)
            .and_then(|chunk| chunk.get(index % RECORD_CHUNK))
    }

    fn first(&self) -> Option<&Record> {
        self.get(0)
    }

    fn last(&self) -> Option<&Record> {
        self.chunks.last().and_then(|chunk| chunk.last())
    }

    /// The records in order; its exact length lets a `collect` allocate
    /// once.
    fn iter(&self) -> RecordsIter<'_> {
        RecordsIter {
            inner: self.chunks.iter().flatten(),
            left: self.len,
        }
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut Record> {
        self.chunks.iter_mut().flatten()
    }
}

/// [`Records::iter`].
struct RecordsIter<'a> {
    inner: core::iter::Flatten<core::slice::Iter<'a, Vec<Record>>>,
    left: usize,
}

impl<'a> Iterator for RecordsIter<'a> {
    type Item = &'a Record;

    fn next(&mut self) -> Option<&'a Record> {
        let record = self.inner.next()?;
        self.left = self.left.saturating_sub(1);
        Some(record)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl ExactSizeIterator for RecordsIter<'_> {}

/// One kept sample between the passes. Times are within
/// [`MAX_TIME_NS`].
#[derive(Clone, Copy)]
struct Record {
    offset: u64,
    stored: u32,
    /// Output size.
    size: u32,
    /// Presentation time in nanoseconds; `None` for a later laced frame
    /// without `DefaultDuration`.
    pts_ns: Option<i64>,
    /// The block's time in nanoseconds, before `CodecDelay`.
    block_ns: i64,
    block_duration_ns: Option<i64>,
    /// Audio: the frame's duration in media ticks, from the codec (the Opus
    /// TOC byte, the FLAC frame header, the Apple Lossless element header;
    /// one AAC frame, 1,536 samples of AC-3 or E-AC-3); 0 when unknown.
    frame: u32,
    /// `frame` is a count the frame itself states (an Apple Lossless
    /// packet's), which the source's times do not change.
    stated: bool,
    keyframe: bool,
}

/// Rewrites a Matroska or WebM file as an MP4 file by stream copy, the
/// movie box first, into `out`; see the module documentation.
///
/// `demux` bounds the Matroska reading, `mux` the output tables (and the
/// per-sample records kept between the passes). Streams MP4 cannot carry,
/// or this remux cannot copy, are dropped and listed, or fail the remux with
/// [`OnUnsupported::Fail`]; a video stream fails it unless
/// [`MatroskaRemuxOptions::drop_video`] is set; a file with nothing to copy
/// fails.
pub fn remux_matroska<S: Source, W: Write>(
    source: S,
    demux: &coreav_mkv::Limits,
    mux: &coreav_isobmff::Limits,
    allowlist: &Allowlist,
    options: &MatroskaRemuxOptions,
    out: W,
) -> Result<MatroskaRemuxSummary, MatroskaRemuxError> {
    let none = Replacements::<SliceSource<'static>> {
        video: None,
        audio: None,
    };
    remux(source, demux, mux, allowlist, options, none, out)
}

/// [`remux_matroska`] with one video stream of the source, one audio
/// stream, or one of each, replaced by the samples of `replacements`
/// (`docs/design/mkv-remux.md`, "Replaced streams"); every other stream is
/// written exactly as [`remux_matroska`] writes it. A replaced audio stream
/// is written as [`AudioReplacement`] describes, with its media timescale
/// as a factor of the movie timescale (their least common multiple, when it
/// fits 32 bits), so that its edits are exact; the rest of this description
/// is the video stream's.
///
/// The replaced stream keeps the source track's place in the track order,
/// its language, enabled flag and rotation; it takes the replacement's
/// sample entry, presentation width and height (`tkhd`, with the rotation's
/// translation computed from them) and samples. None of the source
/// stream's frames is read, so its codec need not be one MP4 carries (VP8,
/// for example), and the `drop_video` rule does not apply to it.
///
/// Sample times are Segment Ticks on the segment's timeline, as
/// `coreav_mkv::Packet::pts` gives them ([`Replacement`]); the track must
/// have a `TrackTimestampScale` of 1.0. The media timescale is the one
/// [`remux_matroska`] gives video (Segment Ticks per second, or 90,000
/// with times rounded to it when a tick does not divide a second), and the
/// edit list is built as for a copied video stream: an empty edit up to
/// the first presented frame's time, then the media from it to the end of
/// the last presented frame (the largest presentation time plus the last
/// sample's duration). So a frame with presentation time `P` is presented
/// at segment time `P`, as [`remux_matroska`] presents a copied frame with
/// that timestamp; decode times only set the composition offsets.
///
/// Fails before writing anything when a replacement is invalid
/// ([`Replacement`], [`AudioReplacement`]), when a `stream_index` is not a
/// stream of its kind the segment keeps (or `options.streams` leaves it
/// out), when two decode times meet after rounding to 90,000, or when
/// nothing of the video replacement would be presented; later failures are
/// those of [`remux_matroska`], and a read error of a replacement's data
/// (reported as a source error).
pub fn remux_matroska_replacing<S: Source, R: Source, W: Write>(
    source: S,
    demux: &coreav_mkv::Limits,
    mux: &coreav_isobmff::Limits,
    allowlist: &Allowlist,
    options: &MatroskaRemuxOptions,
    replacements: Replacements<'_, R>,
    out: W,
) -> Result<MatroskaRemuxSummary, MatroskaRemuxError> {
    remux(source, demux, mux, allowlist, options, replacements, out)
}

/// Which replacement a stream's samples are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Replaced {
    Video,
    Audio,
}

/// The Matroska remux, with or without replaced streams.
fn remux<S: Source, R: Source, W: Write>(
    mut source: S,
    demux: &coreav_mkv::Limits,
    mux: &coreav_isobmff::Limits,
    allowlist: &Allowlist,
    options: &MatroskaRemuxOptions,
    replacements: Replacements<'_, R>,
    mut out: W,
) -> Result<MatroskaRemuxSummary, MatroskaRemuxError> {
    let Replacements {
        video: mut replacement,
        mut audio,
    } = replacements;
    if options.streams.as_ref().is_some_and(|wanted| {
        u64::try_from(wanted.len()).unwrap_or(u64::MAX) > u64::from(mux.max_tracks)
    }) {
        return Err(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::Tracks,
            offset: 0,
        }
        .into());
    }
    let replaced_codec = match replacement.as_mut() {
        Some(r) => Some(validate(r, mux)?),
        None => None,
    };
    let audio_codec = match audio.as_mut() {
        Some(a) => Some(validate_audio(a, mux)?),
        None => None,
    };
    let segment = coreav_mkv::probe(&mut source, demux, allowlist)?;
    if let Some(r) = &replacement {
        check_replaced(&segment, r.stream_index, TrackKind::Video, options)?;
    }
    if let Some(a) = &audio {
        check_replaced(&segment, a.stream_index, TrackKind::Audio, options)?;
        if replacement
            .as_ref()
            .is_some_and(|r| r.stream_index == a.stream_index)
        {
            return Err(coreav_mkv::Error::Unsupported {
                reason: "one stream replaced twice",
            }
            .into());
        }
    }
    if (replacement.is_some() || audio.is_some())
        && options.streams.as_ref().is_some_and(|wanted| {
            wanted
                .iter()
                .enumerate()
                .any(|(n, i)| wanted.iter().skip(n.saturating_add(1)).any(|j| j == i))
        })
    {
        return Err(coreav_mkv::Error::Unsupported {
            reason: "a stream named twice beside a replaced stream",
        }
        .into());
    }
    // A video stream that would be lost fails the remux once every stream
    // is planned, unless the caller named the streams or asked for it to be
    // dropped: the first such stream, and its reason (`None` when the
    // allowlist left it out).
    let guard_video = !options.drop_video && options.streams.is_none();
    let mut lost_video: Option<(usize, Option<&'static str>)> = None;
    let mut lose = |stream_index: usize, reason: Option<&'static str>| {
        if guard_video && lost_video.is_none_or(|(first, _)| stream_index < first) {
            lost_video = Some((stream_index, reason));
        }
    };
    for d in &segment.dropped {
        if d.kind == TrackKind::Video {
            lose(d.stream_index, None);
        }
    }
    let mut dropped: Vec<MatroskaDroppedTrack> = segment
        .dropped
        .iter()
        .map(|d| MatroskaDroppedTrack {
            stream_index: d.stream_index,
            track_number: d.number,
            codec: d.codec,
            reason: TrackDropReason::CodecNotAllowed,
        })
        .collect();
    let mut plans = Vec::new();
    for track in &segment.tracks {
        let named = options
            .streams
            .as_ref()
            .map(|s| s.contains(&track.stream_index));
        if named == Some(false) {
            dropped.push(drop_of(track, TrackDropReason::NotSelected));
            continue;
        }
        if let (Some(r), Some(codec)) = (&replacement, replaced_codec) {
            if r.stream_index == track.stream_index {
                plans.push(replaced_plan(track, codec.matroska()));
                continue;
            }
        }
        if let (Some(a), Some(codec)) = (&audio, audio_codec) {
            if a.stream_index == track.stream_index {
                plans.push(audio_plan(track, codec.matroska(), a.timescale));
                continue;
            }
        }
        match plan(track) {
            Ok(plan) => plans.push(plan),
            Err(reason) => {
                if named == Some(true) || options.on_unsupported == OnUnsupported::Fail {
                    return Err(coreav_mkv::Error::Unsupported { reason }.into());
                }
                if track.kind == TrackKind::Video {
                    lose(track.stream_index, Some(reason));
                }
                dropped.push(drop_of(track, TrackDropReason::Unsupported { reason }));
            }
        }
    }
    let missing = options.streams.as_ref().is_some_and(|s| {
        s.iter()
            .any(|i| !segment.tracks.iter().any(|t| t.stream_index == *i))
    });
    if missing {
        return Err(coreav_mkv::Error::Unsupported {
            reason: "a selected stream does not exist or is not allowed",
        }
        .into());
    }
    dropped.sort_by_key(|d| d.stream_index);

    let mut additions = Vec::new();
    // Per track index: the BlockAdditions seen.
    let mut additions_seen = vec![0u64; segment.tracks.len()];
    // The replacements' samples are charged as the source's would be.
    let replaced_samples = replacement
        .as_ref()
        .map_or(0, |r| u64::try_from(r.samples.len()).unwrap_or(u64::MAX))
        .saturating_add(
            audio
                .as_ref()
                .map_or(0, |a| u64::try_from(a.samples.len()).unwrap_or(u64::MAX)),
        );
    let truncated_at = list_samples(
        &segment,
        &mut source,
        demux,
        mux,
        &mut plans,
        &mut additions,
        &mut additions_seen,
        replaced_samples,
    )?;
    // A stream that turned out to have a problem, no samples or no
    // parameter sets cannot be written.
    let mut kept = Vec::new();
    for mut plan in plans {
        let Some(track) = segment.tracks.get(plan.track) else {
            continue;
        };
        match (plan.replaced, &replacement, &audio) {
            (Some(Replaced::Video), Some(r), _) => {
                plan.entry = r.sample_entry.to_vec();
                plan.timing = Some(replaced_timing(&segment, plan.timescale, r)?);
                kept.push(plan);
                continue;
            }
            (Some(Replaced::Audio), _, Some(a)) => {
                plan.entry = a.sample_entry.to_vec();
                plan.timing = Some(Timing {
                    samples: a.samples.iter().map(|s| (s.duration, 0)).collect(),
                    start_ns: 0,
                    media_time: i64::from(a.priming),
                    end: i128::from(a.priming) + i128::from(a.presented),
                });
                kept.push(plan);
                continue;
            }
            (Some(_), _, _) => continue,
            (None, _, _) => {}
        }
        // A stream without frames loses nothing.
        let empty = plan.failure.is_none() && plan.samples.is_empty();
        let mut reason = if let Some(failure) = plan.failure {
            Some(failure)
        } else if plan.samples.is_empty() && truncated_at.is_some() {
            Some("the file ends before the stream's first frame")
        } else if plan.samples.is_empty() {
            Some("the stream has no frames")
        } else if plan.annex_b && plan.parameter_sets.is_empty() {
            Some("the first H.264 keyframe has no parameter sets")
        } else if plan.codec == Codec::Vp9 && track.chroma_subsampling.is_none() {
            Some("VP9 whose first key frame header could not be read")
        } else {
            None
        };
        if reason.is_none() {
            match sample_entry(track, &plan).and_then(|entry| Ok((entry, timing(track, &plan)?))) {
                Ok((entry, times)) => {
                    plan.entry = entry;
                    plan.timing = Some(times);
                }
                Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason: why })) => {
                    reason = Some(why);
                }
                Err(error) => return Err(error),
            }
        }
        match reason {
            Some(reason) => {
                if options.on_unsupported == OnUnsupported::Fail || options.streams.is_some() {
                    return Err(coreav_mkv::Error::Unsupported { reason }.into());
                }
                if track.kind == TrackKind::Video && !empty {
                    lose(track.stream_index, Some(reason));
                }
                dropped.push(drop_of(track, TrackDropReason::Unsupported { reason }));
            }
            None => kept.push(plan),
        }
    }
    dropped.sort_by_key(|d| d.stream_index);
    if kept.is_empty() {
        if let Some(offset) = truncated_at {
            return Err(coreav_mkv::Error::Truncated { offset }.into());
        }
        // Nothing to write: the lost video stream's reason first, else the
        // first dropped stream's.
        let reason = match (lost_video, dropped.first().map(|d| d.reason)) {
            (Some((_, Some(reason))), _) | (_, Some(TrackDropReason::Unsupported { reason })) => {
                reason
            }
            _ => "no stream can be copied to MP4",
        };
        return Err(coreav_mkv::Error::Unsupported { reason }.into());
    }
    if let Some((stream_index, reason)) = lost_video {
        // Other streams could be written, but not the picture.
        return Err(MatroskaRemuxError::VideoNotCopied {
            stream_index,
            reason: reason.unwrap_or("its codec is not on the allowlist"),
        });
    }

    let movie_timescale = match &audio {
        Some(a) => crate::remux::lcm(video_timescale(&segment), a.timescale)
            .unwrap_or(video_timescale(&segment)),
        None => video_timescale(&segment),
    };
    let mut muxer = Muxer::new(
        FileType {
            major: FourCc(*b"isom"),
            minor: 0x200,
            compatible: vec![FourCc(*b"isom"), FourCc(*b"iso2"), FourCc(*b"mp41")],
        },
        MovieHeader {
            timescale: movie_timescale,
            creation_time: creation_time(&segment),
            boxes: Vec::new(),
        },
        mux,
    )?;
    // The output's tracks, in the order named: a stream named twice is
    // written twice.
    let order: Vec<usize> = match &options.streams {
        None => (0..kept.len()).collect(),
        Some(wanted) => wanted
            .iter()
            .filter_map(|i| {
                kept.iter().position(|plan| {
                    segment
                        .tracks
                        .get(plan.track)
                        .is_some_and(|t| t.stream_index == *i)
                })
            })
            .collect(),
    };
    let mut tracks = Vec::new();
    let mut written = Vec::new();
    for k in order {
        let Some(plan) = kept.get(k) else {
            continue;
        };
        let track = segment
            .tracks
            .get(plan.track)
            .ok_or(coreav_mkv::Error::Unsupported {
                reason: "track index out of range",
            })?;
        let Some(timing) = plan.timing.as_ref() else {
            continue;
        };
        let replaced = replacement
            .as_ref()
            .filter(|_| plan.replaced == Some(Replaced::Video));
        let audio = audio
            .as_ref()
            .filter(|_| plan.replaced == Some(Replaced::Audio));
        let header = TrackHeader {
            media: match (plan.media, replaced) {
                (Media::Video, Some(r)) => replaced_video_media(track, r.width, r.height),
                (Media::Video, None) => video_media(track),
                (Media::Audio, _) => TrackMedia::Audio { volume: 0x0100 },
            },
            timescale: plan.timescale,
            language: track_language(&track.language, track.language_iso639.as_deref()),
            creation_time: creation_time(&segment),
            media_creation_time: creation_time(&segment),
            flags: if track.enabled { 3 } else { 2 },
            layer: 0,
            alternate_group: 0,
            edits: Vec::new(),
            duration: None,
            media_duration: None,
            sample_entry: plan.entry.clone(),
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        };
        let id = muxer.add_track(header)?;
        written.push(k);
        if let Some(r) = replaced {
            for (sample, (duration, offset)) in r.samples.iter().zip(&timing.samples) {
                muxer.push_sample(
                    id,
                    Sample {
                        size: sample.size,
                        duration: *duration,
                        sync: sample.sync,
                        composition_offset: *offset,
                    },
                )?;
            }
        }
        if let Some(a) = audio {
            for sample in a.samples {
                muxer.push_sample(
                    id,
                    Sample {
                        size: sample.size,
                        duration: sample.duration,
                        sync: true,
                        composition_offset: 0,
                    },
                )?;
            }
        }
        for (record, (duration, offset)) in plan.samples.iter().zip(&timing.samples) {
            muxer.push_sample(
                id,
                Sample {
                    size: record.size,
                    duration: *duration,
                    sync: record.keyframe,
                    composition_offset: *offset,
                },
            )?;
        }
        muxer.finish_track(id);
        let media_duration = muxer.duration(id).unwrap_or(0);
        match audio {
            Some(a) => {
                muxer.set_edits(id, audio_edits(a, movie_timescale)?)?;
                let samples = muxer.samples(id).unwrap_or(0);
                muxer.set_sample_table_boxes(id, roll_boxes(samples, a.roll))?;
            }
            None => muxer.set_edits(
                id,
                edits(timing, media_duration, plan.timescale, movie_timescale)?,
            )?,
        }
        tracks.push(MatroskaRemuxTrack {
            stream_index: track.stream_index,
            track_number: track.number,
            track_id: u32::try_from(tracks.len() + 1).unwrap_or(u32::MAX),
            kind: track.kind,
            codec: plan.codec,
            samples: muxer.samples(id).unwrap_or(0),
            timescale: plan.timescale,
            duration: media_duration,
            annex_b: plan.annex_b,
            replaced: plan.replaced.is_some(),
        });
    }

    let head = muxer.write_head(&mut out)?;
    let mut window = CopyWindow::new();
    // The replacements' samples, each read through a window of their own
    // from the offset after the ones before.
    let mut replaced = replacement.map(|r| {
        let sizes = r.samples.iter().map(|s| s.size).collect::<Vec<_>>();
        (r.data, sizes, CopyWindow::new(), 0u64)
    });
    let mut audio = audio.map(|a| {
        let sizes = a.samples.iter().map(|s| s.size).collect::<Vec<_>>();
        (a.data, sizes, CopyWindow::new(), 0u64)
    });
    let mut next = vec![0usize; written.len()];
    let mut nals = Vec::new();
    muxer.for_each_chunk(|index, count| -> Result<(), MatroskaRemuxError> {
        let plan = written.get(index).and_then(|k| kept.get(*k));
        let (Some(plan), Some(position)) = (plan, next.get_mut(index)) else {
            return Err(MatroskaRemuxError::SourceChanged);
        };
        for _ in 0..count {
            if let Some(which) = plan.replaced {
                let feed = match which {
                    Replaced::Video => replaced.as_mut(),
                    Replaced::Audio => audio.as_mut(),
                };
                let Some((data, sizes, window, offset)) = feed else {
                    return Err(MatroskaRemuxError::SourceChanged);
                };
                let size = *sizes
                    .get(*position)
                    .ok_or(MatroskaRemuxError::SourceChanged)?;
                *position += 1;
                window.copy(data, *offset, size, &mut out)?;
                *offset = offset
                    .checked_add(u64::from(size))
                    .ok_or(MatroskaRemuxError::SourceChanged)?;
                continue;
            }
            let record = plan
                .samples
                .get(*position)
                .ok_or(MatroskaRemuxError::SourceChanged)?;
            *position += 1;
            if plan.annex_b {
                nals.clear();
                annex_b_units(
                    &mut source,
                    record.offset,
                    u64::from(record.stored),
                    &mut nals,
                )?;
                let mut written = 0u64;
                for (at, len) in &nals {
                    let len32 =
                        u32::try_from(*len).map_err(|_| MatroskaRemuxError::SourceChanged)?;
                    out.write_all(&len32.to_be_bytes())?;
                    window.copy(&mut source, *at, len32, &mut out)?;
                    written += 4 + len;
                }
                if written != u64::from(record.size) {
                    return Err(MatroskaRemuxError::SourceChanged);
                }
            } else {
                out.write_all(&plan.prefix)?;
                window.copy(&mut source, record.offset, record.stored, &mut out)?;
            }
        }
        Ok(())
    })?;
    out.flush()?;
    drop((replaced, audio));
    // Additions of streams dropped after the listing are not reported; the
    // kept streams' ones are listed in file order up to the limit, and the
    // rest counted.
    additions.retain(|a| tracks.iter().any(|t| t.stream_index == a.stream_index));
    additions.truncate(MAX_LISTED_ADDITIONS);
    let additions_seen: u64 = segment
        .tracks
        .iter()
        .zip(&additions_seen)
        .filter(|(t, _)| tracks.iter().any(|k| k.stream_index == t.stream_index))
        .map(|(_, n)| *n)
        .sum();
    let additions_not_listed = additions_seen.saturating_sub(additions.len() as u64);
    Ok(MatroskaRemuxSummary {
        tracks,
        dropped_tracks: dropped,
        dropped_additions: additions,
        dropped_additions_not_listed: additions_not_listed,
        truncated_at,
        bytes_written: head.total(),
    })
}

fn drop_of(track: &Track, reason: TrackDropReason) -> MatroskaDroppedTrack {
    MatroskaDroppedTrack {
        stream_index: track.stream_index,
        track_number: track.number,
        codec: track.codec,
        reason,
    }
}

/// Whether and how a track can be copied, before its samples are read.
fn plan(track: &Track) -> Result<Plan, &'static str> {
    let media = match track.kind {
        TrackKind::Video => Media::Video,
        TrackKind::Audio => Media::Audio,
        TrackKind::Subtitle => return Err("subtitle streams are not copied to MP4"),
        TrackKind::Data => return Err("data streams are not copied to MP4"),
    };
    let prefix = match &track.encoding {
        Encoding::None => Vec::new(),
        Encoding::HeaderStripping(prefix) => prefix.clone(),
        Encoding::Encrypted => return Err("encrypted streams are not copied"),
        Encoding::Zlib => return Err("zlib-compressed streams are not copied"),
        Encoding::Unsupported => return Err("the stream's frames are compressed"),
    };
    if !track.timestamp_scale_one {
        return Err("TrackTimestampScale other than 1.0 is not supported");
    }
    let mut alac_element = 0;
    let (timescale, quantum) = match (media, track.codec) {
        (Media::Video, Codec::H264 | Codec::Hevc | Codec::Av1 | Codec::Vp9) => (0, 1),
        (Media::Audio, Codec::Opus) => (48_000, 120),
        (Media::Audio, Codec::Flac) => {
            let (_, info) = track
                .codec_private
                .as_deref()
                .and_then(audio::dfla_from_codec_private)
                .ok_or("FLAC without its STREAMINFO in CodecPrivate")?;
            (info.sample_rate, 1)
        }
        (Media::Audio, Codec::Alac) => {
            let (_, config) = track
                .codec_private
                .as_deref()
                .and_then(audio::alac_from_codec_private)
                .ok_or("ALAC without its magic cookie in CodecPrivate")?;
            // Only a packet's first element header is read for its sample
            // count, so a packet must hold that one element.
            alac_element = match config.channels {
                1 => 0,
                2 => 1,
                _ => {
                    return Err("Apple Lossless from Matroska with more than two channels: \
                         a packet's later channel elements are not read");
                }
            };
            (config.sample_rate, u64::from(config.frame_length))
        }
        // The first frame's header gives the sample rate (`list_samples`).
        (Media::Audio, Codec::Ac3 | Codec::Eac3) => (
            track.audio.and_then(|a| a.sample_rate()).unwrap_or(48_000),
            u64::from(audio::AC3_FRAME),
        ),
        (Media::Audio, Codec::Aac) => {
            let rate = track
                .audio
                .and_then(|a| a.sample_rate())
                .filter(|r| *r > 0 && *r <= 65_535)
                .ok_or("AAC sample rate is not an integer up to 65535 Hz")?;
            (rate, aac_frame_length(track))
        }
        // VP8 in MP4 (the binding's `vp08`) does not play in Safari, while
        // the WebM plays in Chrome, Firefox and Safari.
        (_, Codec::Vp8) => {
            return Err(
                "VP8 in MP4 does not play in Safari; keep the WebM, which plays in Chrome, \
                 Firefox and Safari, or re-encode to VP9, H.264 or AV1 outside CoreAV",
            );
        }
        (_, Codec::Vorbis) => return Err("MP4 has no standard sample entry for Vorbis"),
        (_, codec) => return Err(unsupported_codec(codec)),
    };
    // An empty CodecPrivate is no CodecPrivate.
    let private = track.codec_private.as_deref().filter(|p| !p.is_empty());
    match track.codec {
        // configurationVersion 1 and the fixed fields (ISO/IEC 14496-15
        // 5.3.3.1 and 8.3.3.1; AV1 in ISOBMFF 2.3.3: marker and version).
        Codec::H264 if private.is_some_and(|p| p.len() < 7 || p.first() != Some(&1)) => {
            return Err("H.264 CodecPrivate is not an AVCDecoderConfigurationRecord");
        }
        Codec::Hevc if private.is_none_or(|p| p.len() < 23 || p.first() != Some(&1)) => {
            return Err("HEVC without an HEVCDecoderConfigurationRecord");
        }
        Codec::Av1 if private.is_none_or(|p| p.len() < 4 || p.first() != Some(&0x81)) => {
            return Err("AV1 without an AV1CodecConfigurationRecord");
        }
        Codec::Opus if opus_head(track).is_none() => {
            return Err("Opus without a valid OpusHead in CodecPrivate");
        }
        Codec::Aac if aac_config(track).is_none() => {
            return Err("AAC without an AudioSpecificConfig");
        }

        _ => {}
    }
    let annex_b = track.codec == Codec::H264 && private.is_none();
    if annex_b && !prefix.is_empty() {
        return Err("header stripping on an H.264 stream without CodecPrivate");
    }
    Ok(Plan {
        track: track.index,
        codec: track.codec,
        media,
        timescale,
        quantum,
        annex_b,
        avc3: false,
        prefix,
        samples: Records::default(),
        parameter_sets: Vec::new(),
        audio_config: Vec::new(),
        alac_element,
        failure: None,
        end_padding_ns: 0,
        entry: Vec::new(),
        timing: None,
        replaced: None,
    })
}

/// The plan of the replaced stream: a video stream whose samples come from
/// the replacement.
fn replaced_plan(track: &Track, codec: Codec) -> Plan {
    Plan {
        track: track.index,
        codec,
        media: Media::Video,
        timescale: 0,
        quantum: 1,
        annex_b: false,
        avc3: false,
        prefix: Vec::new(),
        samples: Records::default(),
        parameter_sets: Vec::new(),
        audio_config: Vec::new(),
        alac_element: 0,
        failure: None,
        end_padding_ns: 0,
        entry: Vec::new(),
        timing: None,
        replaced: Some(Replaced::Video),
    }
}

/// The plan of the replaced audio stream: its samples come from the
/// replacement, at its media timescale.
fn audio_plan(track: &Track, codec: Codec, timescale: u32) -> Plan {
    Plan {
        track: track.index,
        codec,
        media: Media::Audio,
        timescale,
        quantum: 1,
        annex_b: false,
        avc3: false,
        prefix: Vec::new(),
        samples: Records::default(),
        parameter_sets: Vec::new(),
        audio_config: Vec::new(),
        alac_element: 0,
        failure: None,
        end_padding_ns: 0,
        entry: Vec::new(),
        timing: None,
        replaced: Some(Replaced::Audio),
    }
}

/// Checks that `stream_index` names a track of `kind` of `segment` that a
/// replacement can take the place of: one the segment keeps (the
/// allowlist, else `CodecNotAllowed` as for MP4 sources), among
/// `options.streams` when they are named; a video track with a
/// `TrackTimestampScale` of 1.0.
fn check_replaced(
    segment: &Segment,
    stream_index: usize,
    kind: TrackKind,
    options: &MatroskaRemuxOptions,
) -> Result<(), MatroskaRemuxError> {
    let unsupported =
        |reason| MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason });
    let Some(track) = segment
        .tracks
        .iter()
        .find(|t| t.stream_index == stream_index)
    else {
        return Err(
            match segment
                .dropped
                .iter()
                .find(|d| d.stream_index == stream_index)
            {
                Some(d) => MatroskaRemuxError::Source(coreav_mkv::Error::CodecNotAllowed {
                    track_number: d.number,
                    codec: d.codec,
                }),
                None => unsupported("the replaced stream does not exist"),
            },
        );
    };
    if track.kind != kind {
        return Err(unsupported(if kind == TrackKind::Video {
            "the replaced stream is not a video stream"
        } else {
            "the replaced stream is not an audio stream"
        }));
    }
    if options
        .streams
        .as_ref()
        .is_some_and(|s| !s.contains(&stream_index))
    {
        return Err(unsupported(
            "the replaced stream is not among the selected streams",
        ));
    }
    if kind == TrackKind::Video && !track.timestamp_scale_one {
        return Err(unsupported(
            "TrackTimestampScale other than 1.0 is not supported",
        ));
    }
    Ok(())
}

/// The output times of the replaced stream, as [`stream_timing`] computes
/// them for a copied video stream, from the replacement's decode and
/// presentation times (Segment Ticks) instead of sorted block times.
fn replaced_timing<R>(
    segment: &Segment,
    timescale: u32,
    replacement: &Replacement<'_, R>,
) -> Result<Timing, MatroskaRemuxError> {
    let bad = |reason| MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason });
    let scale = i128::from(segment.timestamp_scale);
    let to_ticks = |v: i64| -> Result<i128, MatroskaRemuxError> {
        let ns = i128::from(v).saturating_mul(scale);
        if ns.unsigned_abs() > u128::from(MAX_TIME_NS) {
            return Err(bad("a time beyond the range this remux keeps (146 years)"));
        }
        Ok(ticks(ns, timescale))
    };
    let samples = replacement.samples;
    let mut times: Vec<(i128, i128)> = Vec::new();
    times
        .try_reserve_exact(samples.len())
        .map_err(|_| coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::TableBytes,
            offset: 0,
        })?;
    for sample in samples {
        let (dts, pts) = (
            to_ticks(sample.decode_time)?,
            to_ticks(sample.presentation_time)?,
        );
        // Rounding to 90,000 can make two decode times meet.
        if times.last().is_some_and(|(previous, _)| dts <= *previous) || pts < dts {
            return Err(bad(
                "the replacement's times collide at the output timescale",
            ));
        }
        times.push((dts, pts));
    }
    let last_duration = ticks(
        i128::from(replacement.last_duration).saturating_mul(scale),
        timescale,
    )
    .max(1);
    let u32_of = |v: i128| u32::try_from(v).map_err(|_| bad("a sample lasts too long for MP4"));
    let mut out = Vec::with_capacity(times.len());
    for (i, (d, p)) in times.iter().enumerate() {
        let duration = match times.get(i + 1) {
            Some((next, _)) => next - d,
            None => last_duration,
        };
        let offset = u32::try_from(p - d)
            .map(i64::from)
            .map_err(|_| bad("a composition offset too large for MP4"))?;
        out.push((u32_of(duration)?, offset));
    }
    let first_pts = times.iter().map(|(_, p)| *p).min().unwrap_or(0);
    let last_pts = times.iter().map(|(_, p)| *p).max().unwrap_or(0);
    let first_dts = times.first().map_or(0, |(d, _)| *d);
    let timing = Timing {
        samples: out,
        start_ns: round_div(first_pts * 1_000_000_000, i128::from(timescale)),
        media_time: i64::try_from(first_pts - first_dts).map_err(|_| bad("media time overflow"))?,
        end: last_pts - first_dts + last_duration,
    };
    let mut first = i128::from(timing.media_time);
    if timing.start_ns < 0 {
        first += ticks(-timing.start_ns, timescale);
    }
    if timing.end <= first {
        return Err(bad("nothing of the replacement is presented"));
    }
    Ok(timing)
}

fn unsupported_codec(codec: Codec) -> &'static str {
    match codec {
        Codec::Mp3 => "MP3 is not copied to MP4 by this remux",
        Codec::Pcm => "PCM audio is not copied to MP4",
        Codec::ProRes => "ProRes has no MP4 sample entry",
        _ => "MP4 cannot carry this codec by stream copy",
    }
}

/// The video timescale and the movie timescale: Segment Ticks per second,
/// or 90,000 when a tick does not divide a second.
fn video_timescale(segment: &Segment) -> u32 {
    let scale = segment.timestamp_scale;
    if 1_000_000_000 % scale == 0 {
        u32::try_from(1_000_000_000 / scale).unwrap_or(FALLBACK_TIMESCALE)
    } else {
        FALLBACK_TIMESCALE
    }
}

/// The first pass: every packet once, keeping a record per kept sample.
#[allow(clippy::too_many_arguments)]
fn list_samples<S: Source>(
    segment: &Segment,
    source: &mut S,
    demux: &coreav_mkv::Limits,
    mux: &coreav_isobmff::Limits,
    plans: &mut [Plan],
    additions: &mut Vec<DroppedAdditions>,
    seen: &mut [u64],
    replaced_samples: u64,
) -> Result<Option<u64>, MatroskaRemuxError> {
    let video = video_timescale(segment);
    for plan in plans.iter_mut() {
        if plan.media == Media::Video {
            plan.timescale = video;
        }
    }
    let mut by_track = vec![None; segment.tracks.len()];
    // A replaced stream's frames are skipped like those of a dropped one.
    for (index, plan) in plans
        .iter()
        .enumerate()
        .filter(|(_, p)| p.replaced.is_none())
    {
        if let Some(slot) = by_track.get_mut(plan.track) {
            *slot = Some(index);
        }
    }
    let scale = i128::from(segment.timestamp_scale);
    let in_range = |ns: i128| {
        i64::try_from(ns)
            .ok()
            .filter(|v| v.unsigned_abs() <= MAX_TIME_NS)
    };
    let mut kept = replaced_samples;
    if kept.saturating_mul(SAMPLE_BYTES) > mux.max_table_bytes {
        return Err(coreav_isobmff::Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::TableBytes,
            offset: 0,
        }
        .into());
    }
    let mut truncated_at = None;
    let mut packets = segment.packets(&mut *source, demux);
    while let Some(packet) = packets.next() {
        let packet = match packet {
            Ok(packet) => packet,
            // The packet iterator ends with this after every complete
            // frame of a cut-off file.
            Err(coreav_mkv::Error::Truncated { offset }) => {
                truncated_at = Some(offset);
                break;
            }
            Err(error) => return Err(error.into()),
        };
        if let (Some((offset, size)), Some(track)) =
            (packet.block_additions, segment.tracks.get(packet.track))
        {
            if let (Some(_), Some(count)) = (
                by_track.get(packet.track).copied().flatten(),
                seen.get_mut(packet.track),
            ) {
                // Each track lists up to the summary's limit, so that a
                // stream dropped later cannot use up the kept ones' share.
                *count += 1;
                if *count <= MAX_LISTED_ADDITIONS as u64 {
                    additions.push(DroppedAdditions {
                        stream_index: track.stream_index,
                        offset,
                        size,
                    });
                }
            }
        }
        let Some(plan) = by_track
            .get(packet.track)
            .copied()
            .flatten()
            .and_then(|i| plans.get_mut(i))
        else {
            continue;
        };
        if plan.failure.is_some() {
            continue;
        }
        kept += 1;
        if kept.saturating_mul(SAMPLE_BYTES) > mux.max_table_bytes {
            return Err(coreav_isobmff::Error::LimitExceeded {
                limit: coreav_isobmff::LimitKind::TableBytes,
                offset: packet.offset,
            }
            .into());
        }
        let (Ok(stored), Ok(size)) = (
            u32::try_from(packet.stored_size),
            u32::try_from(packet.size()),
        ) else {
            plan.fail("a frame larger than 4 GiB");
            continue;
        };
        let pts_ns = match packet.pts_ns(segment) {
            Some(ns) => match in_range(ns) {
                Some(ns) => Some(ns),
                None => {
                    plan.fail("a time beyond the range this remux keeps (146 years)");
                    continue;
                }
            },
            None if plan.media == Media::Video => {
                plan.fail("a video frame has no timestamp");
                continue;
            }
            None => None,
        };
        let Some(block_ns) = scale
            .checked_mul(i128::from(packet.block_timestamp))
            .and_then(in_range)
        else {
            plan.fail("a time beyond the range this remux keeps (146 years)");
            continue;
        };
        let block_duration_ns = packet
            .block_duration
            .and_then(|d| scale.checked_mul(i128::from(d)))
            .and_then(in_range);
        plan.end_padding_ns = packet.discard_padding.unwrap_or(0).max(0);
        // Frame durations from the frame's own header (the Opus TOC byte,
        // RFC 6716 3.1; the FLAC frame header, RFC 9639 9.1.1; the Apple
        // Lossless element header), and the
        // AC-3 or E-AC-3 configuration from the first frame, read as the
        // listing passes the frame rather than in a pass of their own,
        // which over a network would read the file a second time.
        let source = packets.source_mut();
        let mut stated = false;
        let frame = match plan.codec {
            Codec::Opus => {
                let mut head = [0u8; 2];
                let filled = frame_head(source, &plan.prefix, packet.offset, stored, &mut head)?;
                opus_samples(&head, filled).unwrap_or(0)
            }
            Codec::Flac => {
                let mut head = [0u8; FLAC_HEAD];
                let filled = frame_head(source, &plan.prefix, packet.offset, stored, &mut head)?;
                head.get(..filled)
                    .and_then(audio::flac_block_size)
                    .unwrap_or(0)
            }
            Codec::Alac => {
                let mut head = [0u8; audio::ALAC_HEAD];
                let filled = frame_head(source, &plan.prefix, packet.offset, stored, &mut head)?;
                let frame_length = u32::try_from(plan.quantum).unwrap_or(0);
                match head
                    .get(..filled)
                    .and_then(|h| audio::alac_samples(h, plan.alac_element, frame_length))
                {
                    Some((samples, own)) => {
                        stated = own;
                        samples
                    }
                    None => {
                        plan.fail("an Apple Lossless packet without a valid element header");
                        continue;
                    }
                }
            }
            Codec::Ac3 | Codec::Eac3 => {
                if plan.audio_config.is_empty() {
                    if size > MAX_FIRST_FRAME {
                        plan.fail("an AC-3 or E-AC-3 frame larger than 288 KiB");
                        continue;
                    }
                    let mut first = vec![0u8; usize::try_from(size).unwrap_or(0)];
                    frame_head(source, &plan.prefix, packet.offset, stored, &mut first)?;
                    let built = if plan.codec == Codec::Ac3 {
                        audio::dac3(&first)
                            .filter(|(_, _, bytes)| bytes.is_none_or(|b| b == first.len()))
                            .map(|(config, rate, _)| (config, rate))
                            .ok_or("AC-3 frames that are not one syncframe each")
                    } else {
                        audio::dec3(&first)
                    };
                    match built {
                        Ok((config, rate)) => {
                            plan.audio_config = config;
                            plan.timescale = rate;
                        }
                        Err(reason) => {
                            plan.fail(reason);
                            continue;
                        }
                    }
                }
                audio::AC3_FRAME
            }
            _ => u32::try_from(plan.quantum).unwrap_or(0),
        };
        plan.samples.push(Record {
            offset: packet.offset,
            stored,
            size,
            pts_ns,
            block_ns,
            block_duration_ns,
            frame,
            stated,
            keyframe: packet.keyframe,
        });
    }
    drop(packets);
    // Annex B samples: their output size, the first keyframe's parameter
    // sets, and whether a later keyframe changes them.
    let mut nals = Vec::new();
    for plan in plans
        .iter_mut()
        .filter(|p| p.annex_b && p.failure.is_none())
    {
        let mut seen_keyframe = false;
        let mut failure = None;
        for record in plan.samples.iter_mut() {
            nals.clear();
            match annex_b_units(source, record.offset, u64::from(record.stored), &mut nals) {
                Ok(()) => {}
                Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason })) => {
                    failure = Some(reason);
                    break;
                }
                Err(error) => return Err(error),
            }
            if record.keyframe {
                let sets = parameter_sets(source, &nals)?;
                if !seen_keyframe {
                    plan.parameter_sets = sets;
                    seen_keyframe = true;
                } else if !sets.is_empty() && sets != plan.parameter_sets {
                    plan.avc3 = true;
                }
            }
            match nals
                .iter()
                .try_fold(0u64, |sum, (_, len)| sum.checked_add(4 + len))
                .and_then(|s| u32::try_from(s).ok())
            {
                Some(size) => record.size = size,
                None => {
                    failure = Some("a frame larger than 4 GiB");
                    break;
                }
            }
        }
        if let Some(reason) = failure {
            plan.fail(reason);
        }
    }
    Ok(truncated_at)
}

/// The sequence (at most 31) and picture (at most 255) parameter sets
/// among `units`, in order.
fn parameter_sets<S: Source>(
    source: &mut S,
    units: &[(u64, u64)],
) -> Result<Vec<Vec<u8>>, MatroskaRemuxError> {
    let mut sets: Vec<Vec<u8>> = Vec::new();
    let (mut sps, mut pps) = (0, 0);
    for (at, len) in units {
        let mut first = [0u8; 1];
        source.read_exact_at(*at, &mut first)?;
        let kind = first[0] & 0x1f;
        let room = match kind {
            7 => sps < 31,
            8 => pps < 255,
            _ => false,
        };
        if room && *len <= MAX_PARAMETER_SET {
            let mut unit = vec![0u8; usize::try_from(*len).unwrap_or(0)];
            source.read_exact_at(*at, &mut unit)?;
            sets.push(unit);
            if kind == 7 {
                sps += 1;
            } else {
                pps += 1;
            }
        }
    }
    Ok(sets)
}

/// Fills `head` with the first bytes of a frame as a decoder sees them:
/// the track's header-stripping prefix, then the stored bytes at `offset`.
/// Returns how many bytes it filled, fewer than `head` holds when the
/// frame is shorter.
fn frame_head<S: Source>(
    source: &mut S,
    prefix: &[u8],
    offset: u64,
    stored: u32,
    head: &mut [u8],
) -> Result<usize, MatroskaRemuxError> {
    let mut filled = 0;
    for (slot, byte) in head.iter_mut().zip(prefix) {
        *slot = *byte;
        filled += 1;
    }
    let wanted = (head.len() - filled).min(usize::try_from(stored).unwrap_or(0));
    if let Some(rest) = head.get_mut(filled..filled + wanted) {
        source.read_exact_at(offset, rest)?;
        filled += wanted;
    }
    Ok(filled)
}

/// The samples at 48 kHz in an Opus packet whose first `len` bytes (at
/// most 2) are `head`: the frame size of its TOC configuration times its
/// frame count (RFC 6716 3.1 and 3.2.5); `None` for a malformed packet.
fn opus_samples(head: &[u8; 2], len: usize) -> Option<u32> {
    let [toc, second] = *head;
    if len == 0 {
        return None;
    }
    let config = toc >> 3;
    let sizes: [u32; 4] = match config {
        0..=11 => [480, 960, 1920, 2880],
        12..=15 => [480, 960, 480, 960],
        _ => [120, 240, 480, 960],
    };
    let frame = sizes.get(usize::from(config & 3)).copied()?;
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ if len >= 2 => u32::from(second & 0x3F),
        _ => return None,
    };
    let total = frame * count;
    (count > 0 && total <= 5760).then_some(total)
}

/// The NAL units of an Annex B sample: (absolute offset, length) of each,
/// found by reading the sample through a 64 KiB buffer. The sample must
/// start with a start code.
fn annex_b_units<S: Source>(
    source: &mut S,
    offset: u64,
    size: u64,
    units: &mut Vec<(u64, u64)>,
) -> Result<(), MatroskaRemuxError> {
    const CHUNK: u64 = 64 << 10;
    let not_annex_b = coreav_mkv::Error::Unsupported {
        reason: "an H.264 frame without CodecPrivate is not an Annex B byte stream",
    };
    let mut buf = vec![0u8; usize::try_from(CHUNK.min(size)).unwrap_or(0)];
    let mut zeros = 0u64;
    let mut start: Option<u64> = None;
    let mut done = 0u64;
    while done < size {
        let len = CHUNK.min(size - done);
        let chunk = buf
            .get_mut(..usize::try_from(len).unwrap_or(0))
            .ok_or(MatroskaRemuxError::SourceChanged)?;
        source.read_exact_at(offset + done, chunk)?;
        for (i, byte) in chunk.iter().enumerate() {
            let at = done + i as u64;
            if *byte == 1 && zeros >= 2 {
                // A start code ends here; the unit before it ends before
                // its zeros.
                let code_start = at - zeros;
                if units.len() >= MAX_NAL_UNITS {
                    return Err(coreav_mkv::Error::Unsupported {
                        reason: "an H.264 frame with more than 65,536 NAL units",
                    }
                    .into());
                }
                match start {
                    Some(s) if code_start > s => units.push((offset + s, code_start - s)),
                    Some(_) => {}
                    None if code_start == 0 => {}
                    None => return Err(not_annex_b.clone().into()),
                }
                start = Some(at + 1);
                zeros = 0;
            } else if *byte == 0 {
                zeros += 1;
            } else {
                if start.is_none() {
                    return Err(not_annex_b.clone().into());
                }
                zeros = 0;
            }
        }
        done += len;
    }
    let s = start.ok_or(not_annex_b)?;
    let end = size - zeros;
    if end > s {
        if units.len() >= MAX_NAL_UNITS {
            return Err(coreav_mkv::Error::Unsupported {
                reason: "an H.264 frame with more than 65,536 NAL units",
            }
            .into());
        }
        units.push((offset + s, end - s));
    }
    Ok(())
}

/// The output times of a track.
struct Timing {
    /// (duration, composition offset) per sample, in media ticks.
    samples: Vec<(u32, i64)>,
    /// Segment time (nanoseconds) at which the media edit starts.
    start_ns: i128,
    /// Media time of the first presented instant, in media ticks.
    media_time: i64,
    /// Media time at which the presentation ends, in media ticks.
    end: i128,
}

fn round_div(num: i128, den: i128) -> i128 {
    if den == 0 {
        return 0;
    }
    let half = den / 2;
    if num >= 0 {
        (num + half) / den
    } else {
        -((-num + half) / den)
    }
}

/// Nanoseconds in media ticks. Callers keep `ns` within a few times
/// [`MAX_TIME_NS`], so the product fits.
fn ticks(ns: i128, timescale: u32) -> i128 {
    round_div(ns.saturating_mul(i128::from(timescale)), 1_000_000_000)
}

/// The output times of a stream; a stream of which nothing would be
/// presented (all priming, trimming or before time 0) cannot be written,
/// because an empty edit list would present all of it.
fn timing(track: &Track, plan: &Plan) -> Result<Timing, MatroskaRemuxError> {
    let timing = stream_timing(track, plan)?;
    let mut first = i128::from(timing.media_time);
    if timing.start_ns < 0 {
        first += ticks(-timing.start_ns, plan.timescale);
    }
    if timing.end <= first {
        return Err(MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported {
            reason: "nothing of the stream is presented",
        }));
    }
    Ok(timing)
}

fn stream_timing(track: &Track, plan: &Plan) -> Result<Timing, MatroskaRemuxError> {
    let bad = |reason| MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason });
    let ts = plan.timescale;
    let u32_of = |v: i128| u32::try_from(v).map_err(|_| bad("a sample lasts too long for MP4"));
    match plan.media {
        Media::Video => {
            let pts: Vec<i128> = plan
                .samples
                .iter()
                .map(|r| r.pts_ns.map(|ns| ticks(i128::from(ns), ts)))
                .collect::<Option<_>>()
                .ok_or_else(|| bad("a video frame has no timestamp"))?;
            let mut sorted = pts.clone();
            sorted.sort_unstable();
            // Decode times must increase: a repeated presentation time
            // decodes one tick after the previous frame.
            let mut dts: Vec<i128> = Vec::with_capacity(sorted.len());
            for s in &sorted {
                let d = match dts.last() {
                    Some(&previous) if *s <= previous => previous + 1,
                    _ => *s,
                };
                dts.push(d);
            }
            let shift = dts
                .iter()
                .zip(&pts)
                .map(|(d, p)| d - p)
                .max()
                .unwrap_or(0)
                .max(0);
            for d in &mut dts {
                *d -= shift;
            }
            let fallback = plan
                .samples
                .last()
                .and_then(|r| r.block_duration_ns)
                .map(i128::from)
                .or(track.default_duration.map(i128::from))
                .map(|ns| ticks(ns, ts));
            let mut samples = Vec::with_capacity(pts.len());
            for (i, (d, p)) in dts.iter().zip(&pts).enumerate() {
                let duration = match dts.get(i + 1) {
                    Some(next) => next - d,
                    None => fallback
                        .or_else(|| (i > 0).then(|| d - dts.get(i - 1).copied().unwrap_or(*d)))
                        .unwrap_or(1)
                        .max(1),
                };
                // ctts version 0 holds an unsigned 32-bit offset.
                let offset = u32::try_from(p - d)
                    .map(i64::from)
                    .map_err(|_| bad("a composition offset too large for MP4"))?;
                samples.push((u32_of(duration)?, offset));
            }
            let first_pts = sorted.first().copied().unwrap_or(0);
            let last_pts = sorted.last().copied().unwrap_or(0);
            let first_dts = dts.first().copied().unwrap_or(0);
            let last_duration = samples.last().map_or(0, |(d, _)| i128::from(*d));
            let start_ns = round_div(first_pts * 1_000_000_000, i128::from(ts));
            let media_time =
                i64::try_from(first_pts - first_dts).map_err(|_| bad("media time overflow"))?;
            Ok(Timing {
                samples,
                start_ns,
                media_time,
                // The last presented frame lasts as long as the last one
                // decoded.
                end: last_pts - first_dts + last_duration,
            })
        }
        Media::Audio => audio_timing(track, plan),
    }
}

/// Audio times: each frame lasts what it decodes to (see [`Record::frame`]),
/// unless the next frame's time is more than half a frame away
/// from where this one ends (a pause, a lost frame, timestamps that do not
/// follow the frames): then it lasts up to that time, rounded to the
/// codec's quantum, at least one quantum, as Matroska's times are the
/// presentation. Frames whose duration is unknown last up to the next
/// frame's time, rounded the same way.
fn audio_timing(track: &Track, plan: &Plan) -> Result<Timing, MatroskaRemuxError> {
    let bad = |reason| MatroskaRemuxError::Source(coreav_mkv::Error::Unsupported { reason });
    let ts = plan.timescale;
    let q = i128::from(plan.quantum.max(1));
    let n = plan.samples.len();
    // Targets in media ticks of each frame's block time (before
    // CodecDelay); frames without one are spread evenly up to the next
    // frame that has one, or after the last one at the frames' own
    // durations.
    let delay = ticks(i128::from(track.codec_delay), ts);
    let mut targets: Vec<Option<i128>> = plan
        .samples
        .iter()
        .map(|r| r.pts_ns.map(|ns| ticks(i128::from(ns), ts) + delay))
        .collect();
    // Only the source's own times can move a frame.
    let timed: Vec<bool> = targets.iter().map(Option::is_some).collect();
    let mut i = 0;
    while i < n {
        if targets.get(i).copied().flatten().is_some() {
            i += 1;
            continue;
        }
        let prev = (0..i)
            .rev()
            .find_map(|k| targets.get(k).copied().flatten().map(|t| (k, t)));
        let next = (i..n).find_map(|k| targets.get(k).copied().flatten().map(|t| (k, t)));
        let value = match (prev, next) {
            (Some((a, ta)), Some((b, tb))) => {
                ta + (tb - ta) * i128::try_from(i - a).unwrap_or(0)
                    / i128::try_from(b - a).unwrap_or(1).max(1)
            }
            (Some((_, ta)), None) => {
                let frame = plan
                    .samples
                    .get(i.saturating_sub(1))
                    .map(|r| i128::from(r.frame))
                    .filter(|f| *f > 0)
                    .unwrap_or(q);
                ta + frame
            }
            _ => ticks(
                plan.samples.get(i).map_or(0, |r| i128::from(r.block_ns)),
                ts,
            ),
        };
        if let Some(slot) = targets.get_mut(i) {
            *slot = Some(value);
        }
        i += 1;
    }
    let targets: Vec<i128> = targets.into_iter().map(Option::unwrap_or_default).collect();
    let mut time = targets.first().copied().unwrap_or(0);
    let mut samples = Vec::with_capacity(n);
    let mut previous = q;
    let mut total = 0i128;
    for (k, record) in plan.samples.iter().enumerate() {
        let frame = (record.frame > 0).then(|| i128::from(record.frame));
        let to_next = |next: i128| round_div(next - time, q).max(1) * q;
        let duration = match (frame, targets.get(k + 1)) {
            (Some(f), Some(&next))
                if !record.stated
                    && timed.get(k + 1) == Some(&true)
                    && (next - (time + f)).abs() * 2 > f =>
            {
                to_next(next)
            }
            (Some(f), _) => f,
            (None, Some(&next)) => to_next(next),
            // The last frame: its BlockDuration, else the frame before it.
            (None, None) => record.block_duration_ns.map_or(previous, |ns| {
                round_div(ticks(i128::from(ns), ts), q).max(1) * q
            }),
        };
        previous = duration;
        time += duration;
        total += duration;
        samples.push((
            u32::try_from(duration).map_err(|_| bad("a sample lasts too long for MP4"))?,
            0,
        ));
    }
    // CodecDelay worth of samples are priming and not presented (RFC 9559
    // 11.2), nor is the last frame's DiscardPadding.
    let block0 = plan.samples.first().map_or(0, |r| i128::from(r.block_ns));
    // At most the last frame is padding.
    let padding = ticks(i128::from(plan.end_padding_ns), ts).min(previous);
    Ok(Timing {
        samples,
        start_ns: block0,
        media_time: i64::try_from(delay).map_err(|_| bad("codec delay overflow"))?,
        end: total - padding,
    })
}

/// The edit list: an empty edit up to the track's start on the segment's
/// timeline, then the media from its first presented instant to the end of
/// its presentation. Left out when it would only restate the media.
fn edits(
    timing: &Timing,
    media_duration: u64,
    timescale: u32,
    movie_timescale: u32,
) -> Result<Vec<Edit>, coreav_isobmff::Error> {
    let overflow = || coreav_isobmff::Error::Overflow { offset: 0 };
    let mut media_time = i128::from(timing.media_time);
    let mut start = timing.start_ns;
    if start < 0 {
        // Presented before the segment's time 0: skip that part of the
        // media.
        media_time += ticks(-start, timescale);
        start = 0;
    }
    let presented = timing.end - media_time;
    if presented <= 0 {
        return Ok(Vec::new());
    }
    let mut list = Vec::new();
    let empty = ticks(start, movie_timescale);
    if empty > 0 {
        list.push(Edit {
            duration: u64::try_from(empty).map_err(|_| overflow())?,
            media_time: -1,
            rate: Edit::UNIT_RATE,
        });
    }
    let duration = round_div(
        presented.saturating_mul(i128::from(movie_timescale)),
        i128::from(timescale),
    );
    list.push(Edit {
        duration: u64::try_from(duration).map_err(|_| overflow())?,
        media_time: i64::try_from(media_time).map_err(|_| overflow())?,
        rate: Edit::UNIT_RATE,
    });
    if list.len() == 1 && media_time == 0 && timing.end == i128::from(media_duration) {
        return Ok(Vec::new());
    }
    Ok(list)
}

fn video_media(track: &Track) -> TrackMedia {
    const ONE: i32 = 0x0001_0000;
    let video = track.video.unwrap_or_default();
    let width = video.pixel_width.unwrap_or(0);
    let height = video.pixel_height.unwrap_or(0);
    let (dw, dh) = match video.display_unit {
        0 => (
            video.display_width.unwrap_or(width),
            video.display_height.unwrap_or(height),
        ),
        _ => (width, height),
    };
    // 16.16 fixed point; the translation keeps the rotated picture in the
    // positive quadrant (ISO/IEC 14496-12 6.2.2).
    let fixed = |v: u64| u32::try_from(v.min(0xFFFF) << 16).unwrap_or(0);
    let shift = |v: u64| i32::try_from(v.min(0x7FFF) << 16).unwrap_or(0);
    let (a, b, c, d, x, y) = match video.rotation() {
        Some(90) => (0, ONE, -ONE, 0, shift(dh), 0),
        Some(180) => (-ONE, 0, 0, -ONE, shift(dw), shift(dh)),
        Some(270) => (0, -ONE, ONE, 0, 0, shift(dw)),
        _ => (ONE, 0, 0, ONE, 0, 0),
    };
    TrackMedia::Video {
        matrix: [a, b, 0, c, d, 0, x, y, 0x4000_0000],
        width: fixed(dw),
        height: fixed(dh),
    }
}

/// [`video_media`] of the replaced stream: the track's rotation, with the
/// presentation size the replacement gives (16.16 fixed point), from which
/// the rotation's translation is computed.
fn replaced_video_media(track: &Track, width: u32, height: u32) -> TrackMedia {
    const ONE: i32 = 0x0001_0000;
    let video = track.video.unwrap_or_default();
    let shift = |v: u32| i32::try_from(v.min(0x7FFF_0000)).unwrap_or(0);
    let (a, b, c, d, x, y) = match video.rotation() {
        Some(90) => (0, ONE, -ONE, 0, shift(height), 0),
        Some(180) => (-ONE, 0, 0, -ONE, shift(width), shift(height)),
        Some(270) => (0, -ONE, ONE, 0, 0, shift(width)),
        _ => (ONE, 0, 0, ONE, 0, 0),
    };
    TrackMedia::Video {
        matrix: [a, b, 0, c, d, 0, x, y, 0x4000_0000],
        width,
        height,
    }
}

/// The track's language as an ISO 639-2/T code: the primary subtag of
/// `LanguageBCP47` when it has three letters, else `Language`; `und` when
/// neither gives one.
fn track_language(language_tag: &str, legacy: Option<&str>) -> [u8; 3] {
    let primary = language_tag.split('-').next().unwrap_or("");
    match (language(primary), legacy) {
        (code, _) if &code != b"und" => code,
        (_, Some(legacy)) => language(legacy),
        (code, None) => code,
    }
}

/// An ISO 639-2/T code: Matroska's 639-2 bibliographic codes mapped to
/// their terminologic forms; `und` for anything that is not three letters.
fn language(language: &str) -> [u8; 3] {
    let code = match language {
        "alb" => "sqi",
        "arm" => "hye",
        "baq" => "eus",
        "bur" => "mya",
        "chi" => "zho",
        "cze" => "ces",
        "dut" => "nld",
        "fre" => "fra",
        "geo" => "kat",
        "ger" => "deu",
        "gre" => "ell",
        "ice" => "isl",
        "mac" => "mkd",
        "mao" => "mri",
        "may" => "msa",
        "per" => "fas",
        "rum" => "ron",
        "slo" => "slk",
        "tib" => "bod",
        "wel" => "cym",
        other => other,
    };
    match code.as_bytes() {
        [a, b, c] if [a, b, c].iter().all(|x| x.is_ascii_lowercase()) => [*a, *b, *c],
        _ => *b"und",
    }
}

/// `DateUTC` as seconds since 1904-01-01; 0 when absent.
fn creation_time(segment: &Segment) -> u64 {
    segment
        .date_utc
        .map(|ns| ns.div_euclid(1_000_000_000) + 3_061_152_000)
        .and_then(|s| u64::try_from(s).ok())
        .unwrap_or(0)
}

fn sample_entry(track: &Track, plan: &Plan) -> Result<Vec<u8>, MatroskaRemuxError> {
    let video = track.video.unwrap_or_default();
    let dim = |v: Option<u64>| v.and_then(|v| u16::try_from(v).ok()).unwrap_or(0);
    let (w, h) = (dim(video.pixel_width), dim(video.pixel_height));
    let mut children = Vec::new();
    let fourcc = match track.codec {
        Codec::H264 => {
            let record = match track.codec_private.as_ref().filter(|p| !p.is_empty()) {
                Some(private) => private.clone(),
                None => avc_record(&plan.parameter_sets).ok_or(coreav_mkv::Error::Unsupported {
                    reason: "the first H.264 keyframe has no usable parameter sets",
                })?,
            };
            children.push(raw(b"avcC", record));
            FourCc(if plan.avc3 { *b"avc3" } else { *b"avc1" })
        }
        Codec::Hevc => {
            children.push(raw(
                b"hvcC",
                track.codec_private.clone().unwrap_or_default(),
            ));
            FourCc(*b"hev1")
        }
        Codec::Av1 => {
            children.push(raw(
                b"av1C",
                track.codec_private.clone().unwrap_or_default(),
            ));
            FourCc(*b"av01")
        }
        Codec::Vp9 => {
            children.push(raw(
                b"vpcC",
                vpcc(track).ok_or(coreav_mkv::Error::Unsupported {
                    reason: "VP9 4:4:0 has no chroma subsampling value in vpcC",
                })?,
            ));
            FourCc(*b"vp09")
        }
        Codec::Opus => {
            let dops = opus_head(track).ok_or(coreav_mkv::Error::Unsupported {
                reason: "Opus without a valid OpusHead in CodecPrivate",
            })?;
            let channels = u16::from(*dops.get(1).unwrap_or(&2));
            return Ok(audio_sample_entry(
                FourCc(*b"Opus"),
                channels,
                16,
                48_000,
                &[raw(b"dOps", dops)],
            )?);
        }
        Codec::Flac => {
            let (dfla, info) = track
                .codec_private
                .as_deref()
                .and_then(audio::dfla_from_codec_private)
                .ok_or(coreav_mkv::Error::Unsupported {
                    reason: "FLAC without its STREAMINFO in CodecPrivate",
                })?;
            return Ok(audio_sample_entry(
                FourCc(*b"fLaC"),
                info.channels,
                info.bits,
                audio::entry_rate(info.sample_rate),
                &[raw(b"dfLa", dfla)],
            )?);
        }
        Codec::Alac => {
            let (cookie, config) = track
                .codec_private
                .as_deref()
                .and_then(audio::alac_from_codec_private)
                .ok_or(coreav_mkv::Error::Unsupported {
                    reason: "ALAC without its magic cookie in CodecPrivate",
                })?;
            return Ok(audio_sample_entry(
                FourCc(*b"alac"),
                config.channels,
                config.bits,
                audio::entry_rate(config.sample_rate),
                &[raw(b"alac", cookie)],
            )?);
        }
        Codec::Ac3 | Codec::Eac3 => {
            let (fourcc, kind) = if track.codec == Codec::Ac3 {
                (b"ac-3", b"dac3")
            } else {
                (b"ec-3", b"dec3")
            };
            // Annex F has readers ignore the entry's channel count and take
            // the configuration's.
            let channels = track
                .audio
                .map_or(2, |a| u16::try_from(a.channels).unwrap_or(2));
            return Ok(audio_sample_entry(
                FourCc(*fourcc),
                channels,
                16,
                plan.timescale,
                &[raw(kind, plan.audio_config.clone())],
            )?);
        }
        Codec::Aac => {
            let config = aac_config(track).ok_or(coreav_mkv::Error::Unsupported {
                reason: "AAC without an AudioSpecificConfig",
            })?;
            let channels = track
                .audio
                .map_or(2, |a| u16::try_from(a.channels).unwrap_or(2));
            return Ok(audio_sample_entry(
                FourCc(*b"mp4a"),
                channels,
                16,
                plan.timescale,
                &[raw(b"esds", esds(&config))],
            )?);
        }
        _ => {
            return Err(coreav_mkv::Error::Unsupported {
                reason: unsupported_codec(track.codec),
            }
            .into());
        }
    };
    // Colour as Matroska states it, a range alone included, for the codecs
    // whose configuration record may not (VP9's vpcC carries it).
    let color = track.color;
    if track.codec != Codec::Vp9
        && (color.primaries.is_some()
            || color.transfer.is_some()
            || color.matrix.is_some()
            || color.full_range.is_some())
    {
        let mut nclx = b"nclx".to_vec();
        for code in [color.primaries, color.transfer, color.matrix] {
            nclx.extend_from_slice(&code.unwrap_or(2).to_be_bytes());
        }
        nclx.push(if color.full_range == Some(true) {
            0x80
        } else {
            0
        });
        children.push(raw(b"colr", nclx));
    }
    if let (Some((dw, dh)), true) = (video.display_aspect(), w > 0 && h > 0) {
        let h_spacing = u64::from(h).saturating_mul(dw);
        let v_spacing = u64::from(w).saturating_mul(dh);
        let g = gcd(h_spacing, v_spacing);
        if h_spacing != v_spacing && g > 0 {
            let (hs, vs) = (h_spacing / g, v_spacing / g);
            if let (Ok(hs), Ok(vs)) = (u32::try_from(hs), u32::try_from(vs)) {
                let mut pasp = hs.to_be_bytes().to_vec();
                pasp.extend_from_slice(&vs.to_be_bytes());
                children.push(raw(b"pasp", pasp));
            }
        }
    }
    Ok(visual_sample_entry(
        fourcc, w, h, &[0; 32], 0x0018, &children,
    )?)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn raw(kind: &[u8; 4], payload: Vec<u8>) -> RawBox {
    RawBox {
        kind: FourCc(*kind),
        payload,
    }
}

/// An `AVCDecoderConfigurationRecord` (ISO/IEC 14496-15 5.3.3.1) of the
/// sequence and picture parameter sets of the first keyframe, 4-byte NAL
/// unit lengths.
fn avc_record(units: &[Vec<u8>]) -> Option<Vec<u8>> {
    let sps: Vec<&Vec<u8>> = units
        .iter()
        .filter(|u| u.first().is_some_and(|h| h & 0x1f == 7))
        .collect();
    let pps: Vec<&Vec<u8>> = units
        .iter()
        .filter(|u| u.first().is_some_and(|h| h & 0x1f == 8))
        .collect();
    let first = sps.first()?;
    let [_, profile, compatibility, level, ..] = *first.as_slice() else {
        return None;
    };
    if pps.is_empty() || sps.len() > 31 || pps.len() > 255 {
        return None;
    }
    let mut record = vec![
        1,
        profile,
        compatibility,
        level,
        0xFF,
        0xE0 | u8::try_from(sps.len()).ok()?,
    ];
    for unit in &sps {
        record.extend_from_slice(&u16::try_from(unit.len()).ok()?.to_be_bytes());
        record.extend_from_slice(unit);
    }
    record.push(u8::try_from(pps.len()).ok()?);
    for unit in &pps {
        record.extend_from_slice(&u16::try_from(unit.len()).ok()?.to_be_bytes());
        record.extend_from_slice(unit);
    }
    // High profiles carry the chroma format and bit depths (5.3.3.1.2).
    if matches!(profile, 100 | 110 | 122 | 144) {
        let (chroma, luma_depth, chroma_depth) = sps_format(first)?;
        record.push(0xFC | chroma);
        record.push(0xF8 | luma_depth);
        record.push(0xF8 | chroma_depth);
        record.push(0);
    }
    Some(record)
}

/// `chroma_format_idc`, `bit_depth_luma_minus8` and
/// `bit_depth_chroma_minus8` of a high-profile sequence parameter set (ITU-T
/// H.264 7.3.2.1.1).
fn sps_format(sps: &[u8]) -> Option<(u8, u8, u8)> {
    let mut rbsp = Vec::with_capacity(sps.len());
    let mut zeros = 0;
    for &byte in sps.get(1..)? {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        rbsp.push(byte);
    }
    let mut bits = Bits {
        data: &rbsp,
        at: 24,
    };
    bits.ue()?;
    let chroma = bits.ue().filter(|c| *c <= 3)?;
    if chroma == 3 {
        bits.bit()?;
    }
    let luma = bits.ue()?;
    let chroma_depth = bits.ue()?;
    Some((
        u8::try_from(chroma).ok()?,
        u8::try_from(luma).ok().filter(|v| *v < 8)?,
        u8::try_from(chroma_depth).ok().filter(|v| *v < 8)?,
    ))
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(u32::from(bit))
    }

    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let mut value = 1u64;
        for _ in 0..zeros {
            value = (value << 1) | u64::from(self.bit()?);
        }
        u32::try_from(value - 1).ok()
    }
}

/// A `vpcC` payload (VP codec ISO media binding, version 1); `None` for
/// 4:4:0, which the binding cannot state.
fn vpcc(track: &Track) -> Option<Vec<u8>> {
    let (profile, level, depth) = match track.profile {
        Some(Profile::Vp9 {
            profile,
            level,
            bit_depth,
        }) => (profile, level, bit_depth),
        _ => (track.frame_profile.unwrap_or(0), 0, 8),
    };
    let depth = if depth == 0 { 8 } else { depth };
    // 0: 4:2:0 with the MPEG-2 chroma siting most encoders use (neither
    // Matroska's track header nor the VP9 header states it), 2: 4:2:2,
    // 3: 4:4:4.
    let subsampling = match track.chroma_subsampling {
        Some((1, 0)) => 2,
        Some((0, 0)) => 3,
        Some((0, 1)) => return None,
        _ => 0,
    };
    let color = track.color;
    let full = u8::from(color.full_range == Some(true));
    let code = |c: Option<u16>| u8::try_from(c.unwrap_or(2)).unwrap_or(2);
    Some(vec![
        1,
        0,
        0,
        0,
        profile,
        level,
        (depth << 4) | (subsampling << 1) | full,
        code(color.primaries),
        code(color.transfer),
        code(color.matrix),
        0,
        0,
    ])
}

/// A `dOps` payload (Opus in ISOBMFF 4.3.2) from the `OpusHead` of
/// `CodecPrivate` (RFC 7845 5.1): the same fields, big-endian.
fn opus_head(track: &Track) -> Option<Vec<u8>> {
    let head = track.codec_private.as_deref()?;
    let body = head.strip_prefix(b"OpusHead")?;
    let [
        _version,
        channels,
        p0,
        p1,
        r0,
        r1,
        r2,
        r3,
        g0,
        g1,
        family,
        ref rest @ ..,
    ] = *body
    else {
        return None;
    };
    if channels == 0 {
        return None;
    }
    let mut dops = vec![0, channels];
    dops.extend_from_slice(&u16::from_le_bytes([p0, p1]).to_be_bytes());
    dops.extend_from_slice(&u32::from_le_bytes([r0, r1, r2, r3]).to_be_bytes());
    dops.extend_from_slice(&i16::from_le_bytes([g0, g1]).to_be_bytes());
    dops.push(family);
    if family != 0 {
        let table = rest.get(..2 + usize::from(channels))?;
        dops.extend_from_slice(table);
    }
    Some(dops)
}

/// The `AudioSpecificConfig` of an AAC track: `CodecPrivate`, or one built
/// from a legacy codec ID's profile, the sample rate and the channels.
fn aac_config(track: &Track) -> Option<Vec<u8>> {
    if let Some(private) = &track.codec_private {
        return (!private.is_empty()).then(|| private.clone());
    }
    let Some(Profile::Aac {
        object_type, sbr, ..
    }) = track.profile
    else {
        return None;
    };
    let audio = track.audio?;
    let rate = audio.sample_rate()?;
    let channels = u8::try_from(audio.channels)
        .ok()
        .filter(|c| (1..=7).contains(c))?;
    let index = |rate: u32| {
        [
            96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
            8_000, 7_350,
        ]
        .iter()
        .position(|r| *r == rate)
        .and_then(|i| u8::try_from(i).ok())
    };
    let core = index(rate)?;
    let mut bits: Vec<(u32, u32)> = Vec::new();
    if sbr {
        let output = audio
            .output_sampling_frequency
            .filter(|f| f.fract() == 0.0 && *f > 0.0 && *f < 200_000.0)
            .map_or(rate * 2, |f| f as u32);
        bits.push((5, 5));
        bits.push((u32::from(core), 4));
        bits.push((u32::from(channels), 4));
        bits.push((u32::from(index(output)?), 4));
        bits.push((u32::from(object_type), 5));
    } else {
        bits.push((u32::from(object_type), 5));
        bits.push((u32::from(core), 4));
        bits.push((u32::from(channels), 4));
    }
    bits.push((0, 3)); // frameLengthFlag, dependsOnCoreCoder, extensionFlag
    let mut out = Vec::new();
    let (mut acc, mut n) = (0u64, 0u32);
    for (value, width) in bits {
        acc = (acc << width) | u64::from(value);
        n += width;
        while n >= 8 {
            out.push(u8::try_from((acc >> (n - 8)) & 0xFF).unwrap_or(0));
            n -= 8;
        }
    }
    if n > 0 {
        out.push(u8::try_from((acc << (8 - n)) & 0xFF).unwrap_or(0));
    }
    Some(out)
}

/// The AAC frame length in samples: 960 when the `AudioSpecificConfig` of
/// an object type 1 to 4 sets `frameLengthFlag`, else 1,024.
fn aac_frame_length(track: &Track) -> u64 {
    // audioObjectType (5 bits), samplingFrequencyIndex (4), then
    // channelConfiguration (4) and frameLengthFlag; an escaped object type
    // or an explicit frequency moves the flag, and those keep 1,024.
    let Some(&[a, b, ..]) = track.codec_private.as_deref() else {
        return 1024;
    };
    let object_type = a >> 3;
    let index = ((a & 7) << 1) | (b >> 7);
    if (1..=4).contains(&object_type) && index != 15 && (b >> 2) & 1 == 1 {
        960
    } else {
        1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages() {
        assert_eq!(&language("ger"), b"deu");
        assert_eq!(&language("eng"), b"eng");
        assert_eq!(&language("en-US"), b"und");
        assert_eq!(&language("ENG"), b"und");
        assert_eq!(&track_language("de", None), b"und");
        assert_eq!(&track_language("de", Some("ger")), b"deu");
        assert_eq!(&track_language("fra-CA", Some("eng")), b"fra");
    }

    #[test]
    fn edit_lists() {
        // Video starting at 0 with a reorder shift of 2 ticks: frames
        // presented from media time 2 to 102.
        let t = Timing {
            samples: Vec::new(),
            start_ns: 0,
            media_time: 2,
            end: 102,
        };
        let e = edits(&t, 100, 1000, 1000).unwrap();
        assert_eq!(
            e,
            vec![Edit {
                duration: 100,
                media_time: 2,
                rate: Edit::UNIT_RATE
            }]
        );
        // Audio starting 40 ms in, with 312 priming samples at 48 kHz and
        // 120 samples of padding at the end.
        let t = Timing {
            samples: Vec::new(),
            start_ns: 40_000_000,
            media_time: 312,
            end: 48_000 - 120,
        };
        let e = edits(&t, 48_000, 48_000, 1000).unwrap();
        assert_eq!(
            e[0],
            Edit {
                duration: 40,
                media_time: -1,
                rate: Edit::UNIT_RATE
            }
        );
        assert_eq!(e[1].media_time, 312);
        assert_eq!(e[1].duration, 991);
        // Nothing to say: no edit list.
        let t = Timing {
            samples: Vec::new(),
            start_ns: 0,
            media_time: 0,
            end: 100,
        };
        assert!(edits(&t, 100, 1000, 1000).unwrap().is_empty());
        // Padding at the end alone needs an edit.
        let t = Timing {
            samples: Vec::new(),
            start_ns: 0,
            media_time: 0,
            end: 90,
        };
        assert_eq!(edits(&t, 100, 1000, 1000).unwrap()[0].duration, 90);
        // Starting before time 0 skips into the media.
        let t = Timing {
            samples: Vec::new(),
            start_ns: -7_000_000,
            media_time: 0,
            end: 1000,
        };
        assert_eq!(edits(&t, 1000, 1000, 1000).unwrap()[0].media_time, 7);
    }

    #[test]
    fn opus_packet_durations() {
        // SILK 20 ms, one frame; CELT 2.5 ms, two frames; code 3 with 3
        // frames of 20 ms; a code 3 packet without its count byte.
        assert_eq!(opus_samples(&[0x08, 0], 1), Some(960));
        assert_eq!(opus_samples(&[0x81, 0], 1), Some(240));
        assert_eq!(opus_samples(&[0xFB, 0x03], 2), Some(2880));
        assert_eq!(opus_samples(&[0xFB, 0], 1), None);
        assert_eq!(opus_samples(&[0xFB, 0x00], 2), None);
        // More than 120 ms.
        assert_eq!(opus_samples(&[0x1B, 0x03], 2), None);
        assert_eq!(opus_samples(&[0, 0], 0), None);
    }

    #[test]
    fn descriptors() {
        let e = esds(&[0x12, 0x10]);
        assert_eq!(&e[..5], &[0, 0, 0, 0, 0x03]);
        assert!(
            e.windows(6)
                .any(|w| w == [0x05, 0x80, 0x80, 0x80, 0x02, 0x12])
        );
        assert_eq!(round_div(-5, 2), -3);
        assert_eq!(round_div(5, 2), 3);
        assert_eq!(ticks(6_500_000, 48_000), 312);
    }
}
