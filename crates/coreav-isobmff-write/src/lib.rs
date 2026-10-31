//! Memory-safe writing of ISO base media files by stream copy.
//!
//! Three entry points, none of which decodes anything: [`extract_audio`]
//! copies one AAC track of an MP4/MOV file into a new M4A file,
//! [`clip_mp4`] cuts a keyframe-aligned clip of one video track and at most
//! one AAC track into a new MP4 file, [`clip_audio_mp4`] a clip of one AAC
//! track alone, and [`remux_mp4`] rewrites a whole MP4/MOV file as an MP4 file with the
//! movie box first (`docs/design/remux.md`), and [`remux_matroska`]
//! rewrites a Matroska or WebM file as MP4 (`docs/design/mkv-remux.md`);
//! [`remux_mp4_replacing`] and [`remux_matroska_replacing`] do the same
//! with one video stream's samples taken from a [`Replacement`] (the
//! output of `coreav transcode`'s encoder).
//! The [`mux`] module is the shared writer under both remuxes. The source is
//! untrusted; it is read through `coreav-isobmff` under the
//! caller's [`Limits`](coreav_isobmff::Limits), and malformed input produces an [`ExtractError`],
//! never a panic. The output is written front to back to any
//! [`std::io::Write`], so it can go to a pipe or an upload stream.
//!
//! ```
//! use coreav_isobmff::{Allowlist, Limits, SliceSource};
//! use coreav_isobmff_write::{TrackChoice, extract_audio};
//!
//! let not_a_movie = b"RIFF\x24\x00\x00\x00AVI LIST";
//! let mut out = Vec::new();
//! let result = extract_audio(
//!     SliceSource::new(not_a_movie),
//!     &Limits::default(),
//!     &Allowlist::default(),
//!     TrackChoice::FirstAudio,
//!     &mut out,
//! );
//! assert!(result.is_err());
//! assert!(out.is_empty());
//! ```

mod boxes;
mod clip;
mod copy;
mod cut;
pub mod hls;
mod join;
mod m4a;
mod matroska;
mod moov;
pub mod mux;
mod rational;
mod remux;
mod replace;
mod sanitize;
mod scan;
mod split;
mod tables;

pub use clip::{clip_audio_mp4, clip_mp4};
pub use join::join_mp4;
pub use m4a::extract_audio;
pub use matroska::{
    DroppedAdditions, MAX_LISTED_ADDITIONS, MatroskaDroppedTrack, MatroskaRemuxError,
    MatroskaRemuxOptions, MatroskaRemuxSummary, MatroskaRemuxTrack, remux_matroska,
    remux_matroska_replacing,
};
pub use remux::{remux_mp4, remux_mp4_replacing, sanitize_mp4};
pub use replace::{AudioReplacement, AudioSample, Replacement, ReplacementSample, Replacements};
pub use split::split_mp4;

use core::fmt;
use coreav_isobmff::{Codec, Error, FourCc, Rational, TrackKind};

/// Which track to extract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackChoice {
    /// The first audio track the allowlist keeps.
    FirstAudio,
    /// The track at this position among all tracks of the file, dropped
    /// streams included (`Track::stream_index`).
    Stream(usize),
}

/// What [`extract_audio`] wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    /// Position of the extracted track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the extracted track in the source.
    pub track_id: u32,
    /// Samples copied.
    pub samples: u32,
    /// Bytes written to the output.
    pub bytes_written: u64,
    /// Media timescale of the output track (that of the source track).
    pub timescale: u32,
    /// Sum of the decode durations, in media ticks.
    pub duration: u64,
}

/// Which video track a clip keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoChoice {
    /// The first video track the allowlist keeps.
    First,
    /// The track at this position among all tracks of the file, dropped
    /// streams included (`Track::stream_index`).
    Stream(usize),
}

/// Which audio track a clip keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioChoice {
    /// The first audio track the allowlist keeps, if any; the clip fails
    /// when it is not AAC.
    First,
    /// The track at this position among all tracks of the file.
    Stream(usize),
    /// No audio.
    None,
}

/// What [`clip_mp4`] cuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipOptions {
    /// The video track.
    pub video: VideoChoice,
    /// The audio track.
    pub audio: AudioChoice,
    /// Start on the source's presentation timeline, in seconds, at least 0.
    pub start: Rational,
    /// End in seconds, after `start`; `None` for the end of the video.
    pub end: Option<Rational>,
}

/// One track of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipTrack {
    /// Position of the track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the track in the source.
    pub track_id: u32,
    /// The track's codec.
    pub codec: Codec,
    /// Zero-based number of the first copied sample in the source track
    /// (`Packet::sample`); the copied samples follow it in order.
    pub first_sample: u64,
    /// Samples copied.
    pub samples: u32,
    /// Media timescale (that of the source track).
    pub timescale: u32,
    /// Sum of the copied decode durations, in media ticks.
    pub duration: u64,
}

/// Why a track of the source is not in a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftOutReason {
    /// Not the chosen video or audio track.
    NotSelected,
    /// The chosen audio track presents nothing inside the clip.
    NoSamplesInClip,
    /// Dropped by the allowlist.
    CodecNotAllowed,
}

/// A track of the source that is not in a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeftOut {
    /// Position of the track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the track in the source.
    pub track_id: u32,
    /// Why.
    pub reason: LeftOutReason,
}

/// What [`clip_audio_mp4`] cuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioClipOptions {
    /// The audio track: the first one the allowlist keeps, or the track at
    /// a position among all tracks of the file.
    pub audio: TrackChoice,
    /// Start on the source's presentation timeline, in seconds, at least 0.
    pub start: Rational,
    /// End in seconds, after `start`; `None` for the end of the track.
    pub end: Option<Rational>,
}

/// What [`clip_audio_mp4`] wrote. Times are exact, in seconds on the
/// source's presentation timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClipSummary {
    /// The audio track.
    pub audio: ClipTrack,
    /// The requested start: the output's time 0.
    pub start: Rational,
    /// The requested end, `None` for the end of the track.
    pub end: Option<Rational>,
    /// Where the clip ends: the end of the audio's edits as written.
    pub written_end: Rational,
    /// Bytes written to the output.
    pub bytes_written: u64,
    /// Tracks of the source that are not in the clip.
    pub left_out: Vec<LeftOut>,
}

/// What [`clip_mp4`] wrote. Times are exact, in seconds on the source's
/// presentation timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipSummary {
    /// The video track.
    pub video: ClipTrack,
    /// The audio track, if the clip has one.
    pub audio: Option<ClipTrack>,
    /// The requested start.
    pub start: Rational,
    /// Where the clip starts: the presentation time of its first sync
    /// sample; the output's time 0.
    pub snapped_start: Rational,
    /// The requested end, `None` for the end of the video.
    pub end: Option<Rational>,
    /// Where the clip ends: the end of the video edit as written, rounded
    /// up to a whole video tick.
    pub written_end: Rational,
    /// Bytes written to the output.
    pub bytes_written: u64,
    /// Tracks of the source that are not in the clip.
    pub left_out: Vec<LeftOut>,
    /// Child boxes of the video sample entry that the clip does not copy:
    /// all but the codec configuration, `pasp`, `clap`, `colr` and `btrt`.
    pub boxes_left_out: Vec<FourCc>,
}

/// What [`remux_mp4`] does with a stream it cannot copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnUnsupported {
    /// Leave the stream out and list it in the summary.
    #[default]
    Drop,
    /// Fail before writing anything.
    Fail,
}

/// What [`remux_mp4`] copies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemuxOptions {
    /// Stream indexes to keep (`Track::stream_index`); `None` for every
    /// stream that can be copied. A named stream that cannot be copied
    /// fails the remux.
    pub streams: Option<Vec<usize>>,
    /// What happens to a stream that cannot be copied.
    pub on_unsupported: OnUnsupported,
    /// What happens to metadata.
    pub metadata: Metadata,
}

/// What [`remux_mp4`] does with metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Metadata {
    /// Copy the movie's and the tracks' `udta` and `meta` boxes, the sample
    /// entries' child boxes and the header fields as stored
    /// (`docs/design/remux.md`).
    #[default]
    Copy,
    /// Sanitize (`docs/design/sanitize.md`): write only allowlisted boxes,
    /// so no `udta` or `meta` box, no vendor box and no compressor name;
    /// clear the header fields `Keep` does not keep; report everything
    /// left out.
    Sanitize(Keep),
}

/// What sanitization keeps besides the media: header fields, and two
/// metadata items that change how the file plays, written again from what
/// they say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keep {
    /// The track header's transformation matrix (the display rotation);
    /// otherwise the identity matrix is written.
    pub rotation: bool,
    /// The media header's language; otherwise `und` is written.
    pub language: bool,
    /// The creation and modification times of the movie, track and media
    /// headers; otherwise 0 ("not set") is written.
    pub creation_times: bool,
    /// Gapless playback information (the `iTunSMPB` item: encoder delay and
    /// padding, which players trim), written in its canonical form.
    pub gapless: bool,
    /// The tracks' `kind` boxes (ISO/IEC 14496-12 8.10.4: a track's role,
    /// such as a forced subtitle track), when the scheme and value are
    /// from the DASH role or HTML kind vocabularies.
    pub kinds: bool,
}

impl Keep {
    /// Keep none of them.
    pub const NOTHING: Self = Self {
        rotation: false,
        language: false,
        creation_times: false,
        gapless: false,
        kinds: false,
    };
}

impl Default for Keep {
    /// Rotation, language, gapless information and track kinds, which change
    /// how the file plays and say nothing about who made it or where; not
    /// the creation times.
    fn default() -> Self {
        Self {
            rotation: true,
            language: true,
            creation_times: false,
            gapless: true,
            kinds: true,
        }
    }
}

/// One stream of a remuxed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemuxTrack {
    /// Position of the track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the track in the source.
    pub source_track_id: u32,
    /// `track_ID` in the output: the kept tracks are numbered from 1 in
    /// source order.
    pub track_id: u32,
    /// The track's kind.
    pub kind: TrackKind,
    /// The track's codec.
    pub codec: Codec,
    /// Samples copied: all of the source track's; for the replaced stream
    /// of [`remux_mp4_replacing`], the replacement's.
    pub samples: u32,
    /// Media timescale (that of the source track).
    pub timescale: u32,
    /// Sum of the decode durations, in media ticks.
    pub duration: u64,
    /// The stream's samples are the replacement's of
    /// [`remux_mp4_replacing`], not the source's; `codec` is then the
    /// replacement's.
    pub replaced: bool,
}

/// Why a stream of the source is not in a remuxed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackDropReason {
    /// Not among [`RemuxOptions::streams`].
    NotSelected,
    /// Dropped by the allowlist.
    CodecNotAllowed,
    /// The stream cannot be copied to MP4, for this reason.
    Unsupported {
        /// Why, in English.
        reason: &'static str,
    },
}

/// A stream of the source that is not in a remuxed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedTrack {
    /// Position of the track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the track in the source.
    pub track_id: u32,
    /// Why.
    pub reason: TrackDropReason,
}

/// Why a box of the source is not in a remuxed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxDropReason {
    /// The remux does not carry boxes of this type here: the output
    /// rebuilds or leaves out what they describe (`docs/design/remux.md`).
    NotCarried,
    /// A metadata box beyond what [`Limits::max_retained_bytes`] leaves.
    ///
    /// [`Limits::max_retained_bytes`]: coreav_isobmff::Limits::max_retained_bytes
    TooLarge,
    /// Removed by sanitization: a metadata box or item, or a sample entry
    /// child outside the allowlist.
    Sanitized,
}

/// What a removed metadata item describes, from its type or key
/// (`docs/design/sanitize.md`); a hint for the summary, not a guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataKind {
    /// A place, such as `\xa9xyz`, `loci` or a `...location...` key.
    Location,
    /// The recording device or software, such as `\xa9mak`, `\xa9mod`,
    /// `\xa9swr`, `\xa9too` or a `make`, `model` or `software` key.
    Device,
    /// A date, such as `\xa9day` or a `creationdate` key.
    Date,
    /// Anything else.
    Other,
}

/// A box of the source that is not in a remuxed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedBox {
    /// The kept stream the box belongs to; `None` outside the tracks.
    pub stream_index: Option<usize>,
    /// Box types from the top level down to the box itself, such as
    /// `moov`, `trak`, `mdia`, `minf`, `stbl`, `sgpd`.
    pub path: Vec<FourCc>,
    /// Absolute offset of the box in the source.
    pub offset: u64,
    /// Size of the box, header included.
    pub size: u64,
    /// Why.
    pub reason: BoxDropReason,
    /// For a sanitized metadata item, what it describes.
    pub kind: Option<MetadataKind>,
    /// For a QuickTime metadata key (in a `keys` box), an item that refers
    /// to one, or a free-form item (`----`), the name, such as
    /// `com.apple.quicktime.location.ISO6709`: only when it reads as an
    /// identifier (letters, digits, `.`, `_`, `-`, at most 64 bytes), so
    /// that a name holding a value is not repeated.
    pub name: Option<String>,
}

/// A header field that sanitization cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearedField {
    /// Creation and modification times, written as 0.
    CreationTime,
    /// A transformation matrix other than the identity, written as the
    /// identity.
    Rotation,
    /// A language other than `und`, written as `und`.
    Language,
    /// A visual sample entry's compressor name, written empty.
    CompressorName,
    /// Parts of allowlisted sample entry boxes beyond their defined fields,
    /// left out: NAL units other than parameter sets in an H.264 or HEVC
    /// configuration (SEI, which encoders fill with their name and
    /// settings), bytes past a fixed-layout box's fields, unknown children of
    /// a spherical video box.
    SampleEntryExtras,
    /// The metadata source (the tool or camera) of a spherical video
    /// header, written empty.
    SphericalSource,
}

/// A header field of the source that sanitization cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cleared {
    /// The kept stream; `None` for the movie header.
    pub stream_index: Option<usize>,
    /// Which field.
    pub field: ClearedField,
}

/// What [`remux_mp4`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemuxSummary {
    /// The kept streams, in source order.
    pub tracks: Vec<RemuxTrack>,
    /// The streams of the source that are not in the output, in source
    /// order.
    pub dropped_tracks: Vec<DroppedTrack>,
    /// Boxes of the source that are not in the output, in file order (those
    /// of dropped streams are not listed): at most 256.
    pub dropped_boxes: Vec<DroppedBox>,
    /// Dropped boxes beyond those listed.
    pub dropped_boxes_not_listed: u64,
    /// Header fields that sanitization cleared, in source order.
    pub cleared: Vec<Cleared>,
    /// Whether the source's movie box already came before its media data.
    pub source_moov_first: bool,
    /// Bytes written to the output.
    pub bytes_written: u64,
}

/// Where [`split_mp4`] cuts. A cut is made at a clean cut point: a video
/// sync sample that every sample before it in decode order is presented
/// before, and every sample after it is presented after
/// (`docs/design/split.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SplitAt {
    /// At the latest clean cut point at or before each multiple of this
    /// many seconds (more than 0) on the source's presentation timeline.
    Every(Rational),
    /// At the latest clean cut point at or before each of these times, in
    /// seconds on the source's presentation timeline, increasing.
    Times(Vec<Rational>),
}

/// What [`split_mp4`] keeps and where it cuts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitOptions {
    /// The video track.
    pub video: VideoChoice,
    /// The audio track, chosen as for a clip.
    pub audio: AudioChoice,
    /// Where to cut.
    pub at: SplitAt,
}

/// One segment written by [`split_mp4`]. Times are exact, in seconds on the
/// source's presentation timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// The segment's time 0: where its video starts, or its audio when that
    /// starts earlier (only in the first segment).
    pub start: Rational,
    /// Where its video starts: the cut, or the video's start for the first
    /// segment.
    pub video_start: Rational,
    /// Where its video ends: the next cut, or the video's end rounded up to
    /// a whole video tick for the last segment.
    pub end: Rational,
    /// The video samples it holds.
    pub video: ClipTrack,
    /// The audio samples it holds, if any.
    pub audio: Option<ClipTrack>,
    /// Bytes written to its output.
    pub bytes_written: u64,
}

/// What [`split_mp4`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitSummary {
    /// The segments, in order. Together they hold every sample of the kept
    /// tracks exactly once.
    pub segments: Vec<Segment>,
    /// Tracks of the source that are not in the segments.
    pub left_out: Vec<LeftOut>,
    /// Child boxes of the video sample entry that are not copied.
    pub boxes_left_out: Vec<FourCc>,
}

/// Which audio [`join_mp4`] keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinAudio {
    /// The first audio track of every input, which must be AAC; the inputs
    /// must all have one or all have none.
    First,
    /// The audio stream at this index (`Track::stream_index`) of every
    /// input, which must be AAC, as the concat demuxer of `ffmpeg` pairs
    /// streams by index; a stream the allowlist leaves out fails with
    /// `Error::CodecNotAllowed` instead of being skipped.
    Stream(usize),
    /// No audio.
    None,
}

/// What [`join_mp4`] keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinOptions {
    /// The video: the first video track of every input the allowlist keeps
    /// (`VideoChoice::First`), or the stream at this index of every input
    /// (`VideoChoice::Stream`, which `coreav ffmpeg` uses as the concat
    /// demuxer pairs streams by index; a stream the allowlist leaves out
    /// fails with `Error::CodecNotAllowed` instead of being skipped).
    pub video: VideoChoice,
    /// The audio.
    pub audio: JoinAudio,
}

/// One input of a join. Times are exact, in seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinedInput {
    /// Its video track, every sample of it.
    pub video: ClipTrack,
    /// Its audio track, every sample of it, if the join has audio.
    pub audio: Option<ClipTrack>,
    /// Output time of the input's time 0: a time `t` on the input's own
    /// presentation timeline is presented at `t + offset` in the output.
    pub offset: Rational,
    /// Where its video starts and ends in the output; the next input's
    /// video starts at `video_end`.
    pub video_start: Rational,
    /// See `video_start`.
    pub video_end: Rational,
    /// Ticks added to the decode duration of its last video sample, so that
    /// none of its frames is presented in the next input's media time.
    pub extended: u64,
    /// Tracks of the input that are not in the output.
    pub left_out: Vec<LeftOut>,
}

/// What [`join_mp4`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinSummary {
    /// The inputs, in order.
    pub inputs: Vec<JoinedInput>,
    /// Entries of the output's video edit list, empty edits included.
    pub video_edits: usize,
    /// Entries of the output's audio edit list (0 without one or without
    /// audio).
    pub audio_edits: usize,
    /// Length of the output's video, in seconds.
    pub duration: Rational,
    /// Bytes written.
    pub bytes_written: u64,
    /// Child boxes of the video sample entry that are not copied.
    pub boxes_left_out: Vec<FourCc>,
}

/// A track of a join, for messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinTrack {
    /// The video track.
    Video,
    /// The audio track.
    Audio,
}

impl fmt::Display for JoinTrack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Video => "video",
            Self::Audio => "audio",
        })
    }
}

/// How an input differs from the first input, so that the two cannot share
/// one track (`docs/design/join.md`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// The input has an audio track and the first has none, or the reverse.
    AudioTrack {
        /// Whether this input has one.
        present: bool,
    },
    /// A different codec.
    Codec {
        /// The track.
        track: JoinTrack,
        /// The first input's codec.
        first: Codec,
        /// This input's codec.
        this: Codec,
    },
    /// A different sample entry type (such as `avc1` against `avc3`).
    EntryType {
        /// The track.
        track: JoinTrack,
        /// The first input's.
        first: FourCc,
        /// This input's.
        this: FourCc,
    },
    /// A different codec configuration (`avcC`, `hvcC`, `av1C`, `vpcC`, or
    /// the AAC decoder configuration in `esds`).
    Configuration {
        /// The track.
        track: JoinTrack,
        /// The configuration box.
        kind: FourCc,
    },
    /// A different coded size.
    CodedSize {
        /// The first input's width and height.
        first: (u16, u16),
        /// This input's.
        this: (u16, u16),
    },
    /// A different `pasp`, `clap` or `colr` box, or one that only one of
    /// the two has.
    EntryBox {
        /// The box type.
        kind: FourCc,
    },
    /// A different channel count.
    Channels {
        /// The first input's.
        first: u32,
        /// This input's.
        this: u32,
    },
    /// A different sample rate.
    SampleRate {
        /// The first input's.
        first: u32,
        /// This input's.
        this: u32,
    },
    /// A different media timescale.
    Timescale {
        /// The track.
        track: JoinTrack,
        /// The first input's.
        first: u32,
        /// This input's.
        this: u32,
    },
    /// A different transformation matrix (rotation) in the video's track
    /// header.
    Matrix,
    /// A different display size in the video's track header.
    DisplaySize {
        /// The first input's width and height, 16.16 fixed point.
        first: (u32, u32),
        /// This input's.
        this: (u32, u32),
    },
}

/// A codec's name in messages.
fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "H.264",
        Codec::Hevc => "HEVC",
        Codec::Av1 => "AV1",
        Codec::Vp9 => "VP9",
        Codec::ProRes => "ProRes",
        Codec::Aac => "AAC",
        Codec::Mp3 => "MP3",
        Codec::Opus => "Opus",
        Codec::Pcm => "PCM",
        Codec::TimedText => "timed text",
        Codec::Timecode => "timecode",
        _ => "another codec",
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fixed = |v: u32| f64::from(v) / 65_536.0;
        match self {
            Self::AudioTrack { present: true } => f.write_str(
                "it has an audio track and the first input has none; join the video only to drop it",
            ),
            Self::AudioTrack { present: false } => {
                f.write_str("it has no audio track and the first input has one")
            }
            Self::Codec { track, first, this } => write!(
                f,
                "its {track} codec is {}, the first input's is {}",
                codec_name(*this),
                codec_name(*first)
            ),
            Self::EntryType { track, first, this } => write!(
                f,
                "its {track} sample entry is {this}, the first input's is {first}"
            ),
            Self::Configuration { track, kind } => write!(
                f,
                "its {track} codec configuration ({kind}) differs from the first input's"
            ),
            Self::CodedSize { first, this } => write!(
                f,
                "its coded video size is {}x{}, the first input's is {}x{}",
                this.0, this.1, first.0, first.1
            ),
            Self::EntryBox { kind } => write!(
                f,
                "its video sample entry's {kind} box differs from the first input's"
            ),
            Self::Channels { first, this } => write!(
                f,
                "its audio has {this} channels, the first input's has {first}"
            ),
            Self::SampleRate { first, this } => write!(
                f,
                "its audio sample rate is {this} Hz, the first input's is {first} Hz"
            ),
            Self::Timescale { track, first, this } => write!(
                f,
                "its {track} timescale is {this}, the first input's is {first}"
            ),
            Self::Matrix => f.write_str(
                "its video's transformation matrix (rotation) differs from the first input's",
            ),
            Self::DisplaySize { first, this } => write!(
                f,
                "its video display size is {}x{}, the first input's is {}x{}",
                fixed(this.0),
                fixed(this.1),
                fixed(first.0),
                fixed(first.1)
            ),
        }
    }
}

/// Why [`join_mp4`] failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// No input was given.
    NoInputs,
    /// An input (0-based) cannot be read, or cannot be joined on its own:
    /// the errors a clip of it would give, and joining's own (a video that
    /// does not start with a sync sample, say).
    Input {
        /// The input.
        input: usize,
        /// Why.
        error: ExtractError,
    },
    /// An input does not match the first input.
    Mismatch {
        /// The input.
        input: usize,
        /// How.
        mismatch: Mismatch,
    },
    /// The output exceeds a limit or a field's range.
    Output(Error),
    /// Writing the output failed; the output is incomplete.
    Write(std::io::ErrorKind),
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoInputs => f.write_str("no input to join"),
            Self::Input { input, error } => write!(f, "input {input}: {error}"),
            Self::Mismatch { input, mismatch } => {
                write!(f, "input {input} cannot be joined to the first: {mismatch}")
            }
            Self::Output(error) => write!(f, "output: {error}"),
            Self::Write(kind) => write!(f, "write error: {kind}"),
        }
    }
}

impl std::error::Error for JoinError {}

impl From<Error> for JoinError {
    fn from(error: Error) -> Self {
        Self::Output(error)
    }
}

/// Why extraction failed. Nothing about the source's bytes is included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtractError {
    /// The source could not be read, is malformed, exceeds a limit, or has
    /// a track that cannot be extracted (`Error::Unsupported`,
    /// `Error::NoSuchTrack`, `Error::CodecNotAllowed`).
    Source(Error),
    /// Writing the output failed; the output is incomplete.
    Write(std::io::ErrorKind),
    /// The source returned different samples while they were copied than
    /// while the tables were built, or does not have the length an HLS
    /// index was built from; the output is incomplete.
    SourceChanged,
    /// An HLS index is malformed (`hls::HlsIndex`).
    InvalidIndex {
        /// What is wrong.
        reason: &'static str,
    },
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(f),
            Self::Write(kind) => write!(f, "write error: {kind}"),
            Self::SourceChanged => f.write_str("the source changed during extraction"),
            Self::InvalidIndex { reason } => write!(f, "invalid HLS index: {reason}"),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<Error> for ExtractError {
    fn from(error: Error) -> Self {
        Self::Source(error)
    }
}

impl From<std::io::Error> for ExtractError {
    fn from(error: std::io::Error) -> Self {
        Self::Write(error.kind())
    }
}
