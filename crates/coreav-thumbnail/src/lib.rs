//! Thumbnails (`docs/design/thumbnails.md`): the frame a time asks for in
//! an MP4, MOV, Matroska or WebM file, decoded through CoreAV's decode
//! boundary (`coreav-decode`), turned and made square-pixelled for display,
//! resized and encoded as JPEG, PNG or lossless WebP.
//!
//! By default the image is of the latest keyframe at or before the time,
//! so one packet is decoded; with [`ThumbnailOptions::exact`] it is of the
//! first frame at or after the time, as `ffmpeg -ss` gives. Times are on
//! the file's presentation timeline from its start, as for `ffmpeg -ss`.
//!
//! [`video_stream`] gives the same video stream whole, every packet in
//! decode order, for `coreav transcode` (`docs/design/transcode.md`).
//!
//! The caller passes the decoder backends; `coreav-worker` provides the
//! sandboxed FFmpeg fallback and dav1d. With none, nothing is decoded:
//!
//! ```
//! use coreav_isobmff::{Rational, SliceSource};
//! use coreav_thumbnail::{Error, ThumbnailOptions, thumbnail};
//!
//! let options = ThumbnailOptions::at(Rational::from_int(10));
//! let not_a_movie = b"RIFF\x24\x00\x00\x00AVI LIST";
//! let result = thumbnail(SliceSource::new(not_a_movie), &options, &[]);
//! assert!(matches!(result, Err(Error::Isobmff(_))));
//! ```

#![forbid(unsafe_code)]

mod color;
mod image;
mod jpeg;
mod mkv;
mod mp4;
mod png;
mod select;
mod skip;

use core::fmt;

use coreav_decode::{Backend, Codec, Decoder, DecoderInfo, Packet, Policy, Skipped, StreamConfig};
use coreav_frame::{ChromaUpsampling, Filter, Frame, FrameError, Picture};
use coreav_isobmff::{Rational, Source};

pub use image::{BuiltinEncoder, ImageEncoder, ImageFormat};
use select::{Choice, Sample};

/// Largest side of a thumbnail.
pub const MAX_SIDE: u32 = 16_384;

/// What to make.
#[derive(Clone, Debug)]
pub struct ThumbnailOptions {
    /// Seconds from the file's start time.
    pub at: Rational,
    /// The first frame at or after `at` instead of the keyframe at or
    /// before it.
    pub exact: bool,
    /// Width of the image; with `height` absent, the height follows the
    /// display aspect ratio.
    pub width: Option<u32>,
    /// Height of the image.
    pub height: Option<u32>,
    /// Image format.
    pub format: ImageFormat,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    /// Resampling filter.
    pub filter: Filter,
    /// How chroma is upsampled; FFmpeg's way by default.
    pub chroma: ChromaUpsampling,
    /// The stream index (among all of the file's streams) of the video
    /// stream; `None` for FFmpeg's default: the video stream with the
    /// highest resolution, the first of those on a tie.
    pub stream: Option<usize>,
    /// Limits of the MP4/MOV parser.
    pub isobmff_limits: coreav_isobmff::Limits,
    /// Limits of the Matroska parser.
    pub matroska_limits: coreav_mkv::Limits,
    /// Which codecs may be decoded, by which backends.
    pub policy: Policy,
    /// Decode limits.
    pub decode_limits: coreav_decode::Limits,
    /// Packets before the wanted frame that no later picture depends on
    /// are not sent to the decoder (H.264 and HEVC; `docs/design/
    /// thumbnails.md`, "Frames nothing depends on"). On by default; off,
    /// every packet from the keyframe is decoded, which gives the same
    /// image.
    pub skip_disposable: bool,
    /// MP4 and MOV: the packets around `at` are found in the sample tables
    /// and only they are listed, instead of every packet of the stream
    /// (`docs/design/seek.md`). On by default; off, every packet is listed,
    /// which chooses the same frame.
    pub seek: bool,
}

impl ThumbnailOptions {
    /// The defaults for a thumbnail at `at` seconds: keyframe mode, the
    /// display size, JPEG at quality 91, bicubic, the first video stream,
    /// the default limits and policy.
    pub fn at(at: Rational) -> Self {
        Self {
            at,
            exact: false,
            width: None,
            height: None,
            format: ImageFormat::Jpeg,
            // The operating point of CoreAV's earlier encoder at 90
            // (docs/design/thumbnails.md, "JPEG encoder").
            quality: 91,
            filter: Filter::Bicubic,
            chroma: ChromaUpsampling::FfmpegCompatible,
            stream: None,
            isobmff_limits: coreav_isobmff::Limits::default(),
            matroska_limits: coreav_mkv::Limits::default(),
            policy: Policy::default(),
            decode_limits: coreav_decode::Limits::default(),
            skip_disposable: true,
            seek: true,
        }
    }
}

/// A thumbnail, and how it was made.
#[derive(Clone, Debug)]
pub struct Thumbnail {
    /// The encoded image.
    pub image: Vec<u8>,
    /// Its format.
    pub format: ImageFormat,
    /// Its width.
    pub width: u32,
    /// Its height.
    pub height: u32,
    /// The video stream's index among the file's streams.
    pub stream_index: usize,
    /// Its codec.
    pub codec: Codec,
    /// The frame's presentation time in seconds from the file's start.
    pub time: Rational,
    /// The frame is the keyframe decoding started from.
    pub keyframe: bool,
    /// The presentation time in seconds from the file's start of the
    /// keyframe decoding started from.
    pub decode_start: Rational,
    /// Byte offset and size of the frame's packet in the file.
    pub packet: (u64, u64),
    /// Recoverable problems met.
    pub warnings: Vec<String>,
    /// Packets sent to the decoder.
    pub packets_decoded: u32,
    /// The decoder that ran.
    pub decoder: DecoderInfo,
    /// Backends tried before it.
    pub skipped: Vec<Skipped>,
}

/// Why no thumbnail was made.
#[derive(Debug)]
pub enum Error {
    /// The MP4/MOV parser rejected the file or could not read it.
    Isobmff(coreav_isobmff::Error),
    /// The Matroska parser rejected the file or could not read it.
    Matroska(coreav_mkv::Error),
    /// Decoding failed, or no backend could decode the stream.
    Decode(coreav_decode::Error),
    /// A frame operation failed.
    Frame(FrameError),
    /// The file has no video stream (or none with the index asked for).
    NoVideo,
    /// The time is at or after the end of the video stream.
    TimeOutOfRange,
    /// The file cannot be thumbnailed as asked.
    Unsupported(String),
    /// The image could not be encoded.
    Encode(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Isobmff(e) => write!(f, "{e}"),
            Self::Matroska(e) => write!(f, "{e}"),
            Self::Decode(e) => write!(f, "decoding: {e}"),
            Self::Frame(e) => write!(f, "frame: {e}"),
            Self::NoVideo => f.write_str("no video stream"),
            Self::TimeOutOfRange => f.write_str("the time is at or after the end of the video"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Encode(what) => write!(f, "encoding the image: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<coreav_isobmff::Error> for Error {
    fn from(e: coreav_isobmff::Error) -> Self {
        Self::Isobmff(e)
    }
}

impl From<coreav_mkv::Error> for Error {
    fn from(e: coreav_mkv::Error) -> Self {
        Self::Matroska(e)
    }
}

impl From<coreav_decode::Error> for Error {
    fn from(e: coreav_decode::Error) -> Self {
        Self::Decode(e)
    }
}

impl From<FrameError> for Error {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

/// One packet of the video stream, where its bytes are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PacketRef {
    pub offset: u64,
    pub size: u64,
    /// The pts given to the decoder, in the stream's time scale.
    pub pts: i64,
    pub dts: Option<i64>,
    pub keyframe: bool,
}

/// How a sync sample is checked against its coded data
/// (`docs/design/cut-points.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CutRule {
    /// H.264: the frame starts decoding by itself.
    H264 {
        format: coreav_isobmff::access::AccessFormat,
        max_num_ref_frames: Option<u32>,
    },
    /// H.264 stored as a byte stream (ITU-T H.264 Annex B), which a
    /// Matroska track written without a configuration record carries: the
    /// frame starts decoding by itself, read from the packet's bytes.
    H264ByteStream,
    /// HEVC: the first coded slice is an IRAP picture.
    Hevc { length_size: u8 },
    /// AV1: a sequence header, then a key frame shown at once.
    Av1,
    /// The container's flag.
    Container,
}

/// What the container glue found: the stream, its packets and how to
/// show its frames.
pub(crate) struct Plan {
    /// Recoverable problems, for the summary.
    pub warnings: Vec<String>,
    pub stream_index: usize,
    pub stream: StreamConfig,
    pub quarter_turns: u8,
    /// The width and height of a pixel, when pixels are not square.
    pub pixel_aspect: Option<(u64, u64)>,
    /// The file's start time in seconds.
    pub start: Rational,
    pub samples: Vec<Sample>,
    pub packets: Vec<PacketRef>,
    /// Bytes every packet starts with that the file does not store
    /// (Matroska header stripping).
    pub prefix: Vec<u8>,
    pub rule: CutRule,
    /// Whether a sync flag is stated, per packet index (MP4 tables without
    /// `stss`); a frame whose coded data cannot be read keeps a stated
    /// flag.
    pub stated: Vec<bool>,
    /// When `packets` are only those around the time: bounds on the packets
    /// left out, which say whether the choice made from them is the one
    /// every packet gives (`docs/design/seek.md`).
    pub window: Option<Window>,
    /// The stream's first keyframe when `packets` may not hold it.
    pub first_keyframe: Option<PacketRef>,
    /// The container, which sets the units of the timestamps.
    pub container: Container,
    /// Per packet index: its duration in the stream's units, when known
    /// (MP4: the decode duration; Matroska: `BlockDuration` or
    /// `DefaultDuration`).
    pub durations: Vec<Option<u64>>,
    /// Seconds per unit of the packets' timestamps.
    pub time_base: Rational,
    /// Where the container presents the stream.
    pub presentation: Presentation,
}

/// Where the container presents a video stream (an MP4 edit list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Presentation {
    /// Everything: no edit list (MP4), or Matroska.
    All,
    /// The media edit of an edit list of the simple model.
    Window(PresentationWindow),
    /// An edit list outside the simple model, for this reason.
    Unsupported(&'static str),
}

/// Bounds, in seconds on the file's timeline, on the presentation times of
/// the packets a window leaves out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    /// No packet before the window is presented later than this; `None`
    /// when the window starts at the first packet.
    pub before: Option<Rational>,
    /// No packet after the window is presented earlier than this; `None`
    /// when it ends at the last packet.
    pub after: Option<Rational>,
}

/// The container glue: the video stream and its packets, around
/// `options.at`, or the first `first` packets.
fn plan<S: Source>(
    source: &mut S,
    options: &ThumbnailOptions,
    first: Option<usize>,
) -> Result<Plan, Error> {
    let mut magic = [0u8; 4];
    let len = source.len()?;
    let matroska = len >= 4 && {
        source.read_exact_at(0, &mut magic)?;
        coreav_mkv::sniff(&magic)
    };
    let mut plan = if matroska {
        mkv::plan(source, options, first)?
    } else {
        mp4::plan(source, options, first)?
    };
    // The colour a decoder applies: the coded data's or the container's,
    // per codec, as FFmpeg's decoders do.
    let header = first_keyframe_header(source, &plan)?;
    plan.stream.color = color::applied(
        plan.stream.codec,
        plan.stream.config.as_ref(),
        &header,
        plan.stream.color,
    );
    Ok(plan)
}

/// The first bytes of the stream's first keyframe (empty without one).
fn first_keyframe_header<S: Source>(source: &mut S, plan: &Plan) -> Result<Vec<u8>, Error> {
    let Some(packet) = plan
        .first_keyframe
        .as_ref()
        .or_else(|| plan.packets.iter().find(|p| p.keyframe))
    else {
        return Ok(Vec::new());
    };
    let mut out = plan.prefix.clone();
    out.truncate(color::HEADER_BYTES);
    let want = color::HEADER_BYTES.saturating_sub(out.len());
    let len = usize::try_from(packet.size).unwrap_or(usize::MAX).min(want);
    source.read_append(packet.offset, len, &mut out)?;
    Ok(out)
}

/// One packet with its bytes, for an oracle.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OraclePacket {
    /// The bytes, as a decoder takes them.
    pub data: Vec<u8>,
    /// Presentation timestamp, in the stream's time scale.
    pub pts: i64,
    /// Decode timestamp.
    pub dts: Option<i64>,
    /// Sync flag.
    pub keyframe: bool,
}

/// The video stream's index and configuration and its first `limit`
/// packets in decode order, as [`thumbnail`] would give them to a decoder:
/// for the frame-agreement oracle (`docs/design/decode.md`, bar 1).
#[doc(hidden)]
pub fn first_packets<S: Source>(
    mut source: S,
    options: &ThumbnailOptions,
    limit: usize,
) -> Result<(usize, StreamConfig, Vec<OraclePacket>), Error> {
    let plan = plan(&mut source, options, Some(limit))?;
    let mut out = Vec::with_capacity(plan.packets.len());
    for (index, p) in plan.packets.iter().enumerate() {
        out.push(OraclePacket {
            data: read_packet(
                &mut source,
                &plan,
                index,
                options.decode_limits.max_packet_bytes,
            )?,
            pts: p.pts,
            dts: p.dts,
            keyframe: p.keyframe,
        });
    }
    Ok((plan.stream_index, plan.stream, out))
}

/// The container a [`VideoStream`] was read from, which sets the units of
/// its timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    /// MP4, MOV and other ISO base media files: media ticks of the track's
    /// media timescale (`mdhd`), on its media timeline (composition and
    /// decode times as the sample tables give them).
    Mp4,
    /// Matroska and WebM: Segment Ticks (`TimestampScale` units), the
    /// blocks' timestamps.
    Matroska,
}

/// The media edit of an MP4 edit list of the simple model (at most one
/// empty edit, then one media edit at rate 1): the media times it
/// presents, in media ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationWindow {
    /// The first media time presented (the edit's `media_time`).
    pub start: i64,
    /// The end of the edit's media (exclusive), its duration rounded up to
    /// a whole media tick; `None` for an edit that lasts to the end of the
    /// media.
    pub end: Option<i64>,
}

/// One packet of a [`VideoStream`], in decode order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamPacket {
    /// Byte offset of its stored bytes in the file.
    pub offset: u64,
    /// Size of its stored bytes; [`VideoStream::read_packet`] adds the
    /// stripped header of a Matroska track.
    pub size: u64,
    /// Presentation timestamp in the stream's units ([`Container`]): MP4,
    /// the composition time in media ticks (`coreav_isobmff::Packet::cts`);
    /// Matroska, the block's timestamp in Segment Ticks
    /// (`coreav_mkv::Packet::pts`).
    pub pts: i64,
    /// Decode timestamp in the same units, when the container has one.
    pub dts: Option<i64>,
    /// Duration in the same units, when known: MP4, the decode duration;
    /// Matroska, `BlockDuration`, else `DefaultDuration` rounded to a tick.
    pub duration: Option<u64>,
    /// A sync sample (MP4) or a keyframe (Matroska).
    pub keyframe: bool,
    /// The container presents the frame: MP4, the edit list presents part
    /// of it (`coreav_isobmff::Packet::presented`); Matroska, the frame is
    /// not invisible.
    pub presented: bool,
}

/// The video stream [`thumbnail`] decodes, whole: what a decoder needs to
/// decode every frame of it, in decode order (`docs/design/transcode.md`).
#[derive(Clone, Debug)]
pub struct VideoStream {
    /// The container, which sets the units of the timestamps.
    pub container: Container,
    /// The stream's position among all streams of the file.
    pub stream_index: usize,
    /// The decoder's configuration. Its colour description is the one a
    /// decoder applies (the coded data's or the container's, per codec),
    /// as thumbnails apply it.
    pub stream: StreamConfig,
    /// The width and height of a pixel, when the container states one.
    pub pixel_aspect: Option<(u64, u64)>,
    /// Quarter turns counterclockwise the container asks for at display.
    pub quarter_turns: u8,
    /// Seconds per unit of the timestamps: `1 / timescale` (MP4), or
    /// `TimestampScale` nanoseconds (Matroska).
    pub time_base: Rational,
    /// Where an MP4 edit list presents the stream; `None` without an edit
    /// list and for Matroska. Edit lists outside the simple model are
    /// refused by [`video_stream`].
    pub presentation: Option<PresentationWindow>,
    /// The packets in decode order.
    pub packets: Vec<StreamPacket>,
    /// Recoverable problems met, for example composition offsets of a
    /// version 0 `ctts` read as signed (the packets' times are then not
    /// the sample tables').
    pub warnings: Vec<String>,
    /// Bytes every packet starts with that the file does not store.
    prefix: Vec<u8>,
}

impl VideoStream {
    /// The bytes every packet starts with that the file does not store
    /// (Matroska header stripping); empty otherwise.
    pub fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    /// Reads packet `index`'s bytes as a decoder takes them, the stripped
    /// header first; a packet of more than `max` bytes with it is a
    /// `Limit` error.
    pub fn read_packet<S: Source>(
        &self,
        source: &mut S,
        index: usize,
        max: u64,
    ) -> Result<Vec<u8>, Error> {
        let packet = self
            .packets
            .get(index)
            .ok_or_else(|| Error::Unsupported("no such packet".into()))?;
        read_bytes(source, packet.offset, packet.size, &self.prefix, max)
    }
}

/// The whole video stream [`thumbnail`] would decode, chosen as it chooses
/// (`options.stream`, else FFmpeg's default), with every packet in decode
/// order (`docs/design/transcode.md`). Of `options`, only the container
/// limits, the stream and the policy's allowlist (Matroska's long-tail
/// codecs) are used. An MP4 edit list outside the simple model fails with
/// [`Error::Unsupported`].
pub fn video_stream<S: Source>(
    mut source: S,
    options: &ThumbnailOptions,
) -> Result<VideoStream, Error> {
    // With a count, every packet is listed: no window around a time.
    let plan = plan(&mut source, options, Some(usize::MAX))?;
    let presentation = match plan.presentation {
        Presentation::Unsupported(why) => {
            return Err(Error::Unsupported(format!("the video stream has {why}")));
        }
        Presentation::Window(window) => Some(window),
        Presentation::All => None,
    };
    let packets = plan
        .packets
        .iter()
        .enumerate()
        .map(|(i, p)| StreamPacket {
            offset: p.offset,
            size: p.size,
            pts: p.pts,
            dts: p.dts,
            duration: plan.durations.get(i).copied().flatten(),
            keyframe: p.keyframe,
            presented: plan.samples.get(i).is_none_or(|s| s.presented),
        })
        .collect();
    Ok(VideoStream {
        container: plan.container,
        stream_index: plan.stream_index,
        stream: plan.stream,
        pixel_aspect: plan.pixel_aspect,
        quarter_turns: plan.quarter_turns,
        time_base: plan.time_base,
        presentation,
        packets,
        warnings: plan.warnings,
        prefix: plan.prefix,
    })
}

/// The plan and the packets `options.at` asks for. A choice from a window
/// stands only when the packets left out could not change it; otherwise,
/// and on any error, every packet is listed and chosen from again, so the
/// result is always the full listing's (`docs/design/seek.md`).
fn planned<S: Source>(source: &mut S, options: &ThumbnailOptions) -> Result<(Plan, Choice), Error> {
    let mut plan = plan(source, options, None)?;
    let at = plan
        .start
        .checked_add(options.at)
        .ok_or_else(|| Error::Unsupported("time out of range".into()))?;
    let mut choice = choose(source, &plan, at, options.exact);
    // A source that fails to read fails the plan; the full listing below
    // is for a window that cannot choose, not for a failed read (#594).
    if let Err(error @ Error::Isobmff(coreav_isobmff::Error::Io(_))) = choice {
        return Err(error);
    }
    let holds = choice
        .as_ref()
        .is_ok_and(|c| window_holds(&plan, *c, at, options.exact));
    if !holds && plan.window.is_some() {
        let every = ThumbnailOptions {
            seek: false,
            ..options.clone()
        };
        plan = self::plan(source, &every, None)?;
        choice = choose(source, &plan, at, options.exact);
    }
    Ok((plan, choice?))
}

/// What [`thumbnail`] would decode, without decoding: for the check that a
/// window chooses as every packet does (`scripts/seek_check.py`).
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OracleChoice {
    /// Byte offsets of the first packet decoded and of the frame's packet.
    pub start: u64,
    pub target: u64,
    /// The frame's presentation time in seconds on the file's timeline.
    pub time: Option<Rational>,
    /// Packets from the first decoded to the frame's, in decode order.
    pub packets: usize,
    /// The plan listed only a window of the packets.
    pub windowed: bool,
    /// The byte offset of the stream's first keyframe.
    pub first_keyframe: Option<u64>,
}

/// [`OracleChoice`] for `options`.
#[doc(hidden)]
pub fn oracle_choice<S: Source>(
    mut source: S,
    options: &ThumbnailOptions,
) -> Result<OracleChoice, Error> {
    let (plan, choice) = planned(&mut source, options)?;
    let offset = |i: usize| plan.packets.get(i).map(|p| p.offset).unwrap_or(u64::MAX);
    Ok(OracleChoice {
        start: offset(choice.start),
        target: offset(choice.target),
        time: plan.samples.get(choice.target).and_then(|s| s.time),
        packets: choice.target.saturating_sub(choice.start).saturating_add(1),
        windowed: plan.window.is_some(),
        first_keyframe: plan
            .first_keyframe
            .as_ref()
            .or_else(|| plan.packets.iter().find(|p| p.keyframe))
            .map(|p| p.offset),
    })
}

/// The packets `at` asks for (`select`).
fn choose<S: Source>(
    source: &mut S,
    plan: &Plan,
    at: Rational,
    exact: bool,
) -> Result<Choice, Error> {
    let mut cut_point = |index: usize| cut_point(source, plan, index);
    // The first cut point after the frame is a last resort, taken only
    // from a listing of every packet: a packet before a window might be a
    // cut point, and `planned` lists every packet when a window's choice
    // does not hold.
    let forward = plan.window.is_none();
    if exact {
        select::exact(&plan.samples, at, forward, &mut cut_point)
    } else {
        select::keyframe(&plan.samples, at, forward, &mut cut_point)
    }
}

/// Whether `choice`, made from a window of the packets, is the one every
/// packet gives (`docs/design/seek.md`, "Why the choice is the same"). True
/// without a window.
fn window_holds(plan: &Plan, choice: Choice, at: Rational, exact: bool) -> bool {
    let Some(window) = plan.window else {
        return true;
    };
    let lt = |a: Rational, b: Rational| a.checked_lt(b).unwrap_or(false);
    let le = |a: Rational, b: Rational| b.checked_lt(a) == Some(false);
    let Some(target) = plan.samples.get(choice.target) else {
        return false;
    };
    let Some(time) = target.time else {
        return false;
    };
    if !exact {
        if choice.start == choice.target && select::offered_by(target, at) {
            // The latest keyframe at or before `at`: nothing before the
            // window is presented after it, nothing after the window at or
            // before `at`.
            return window.before.is_none_or(|b| le(b, time))
                && window.after.is_none_or(|a| lt(at, a));
        }
        // Keyframe mode fell back to the frame at the time, because no
        // keyframe of the window qualified; one before the window might.
        if window.before.is_some() {
            return false;
        }
    }
    // The first frame at or after `at`: everything before the window is
    // presented before `at`, nothing after it earlier than the frame.
    window.before.is_none_or(|b| lt(b, at)) && window.after.is_none_or(|a| le(time, a))
}

/// Makes a thumbnail of `source` (`docs/design/thumbnails.md`).
pub fn thumbnail<S: Source>(
    mut source: S,
    options: &ThumbnailOptions,
    backends: &[&dyn Backend],
) -> Result<Thumbnail, Error> {
    let (plan, choice) = planned(&mut source, options)?;
    let (frame, decoder, skipped, decoded) = decode(&mut source, &plan, choice, options, backends)?;
    let rgb = render(&frame, &plan, options)?;
    let encoder = BuiltinEncoder {
        format: options.format,
        quality: options.quality,
    };
    let image = encoder.encode(&rgb)?;
    let time = plan
        .samples
        .get(choice.target)
        .and_then(|s| s.time)
        .and_then(|t| t.checked_add(negate(plan.start)?))
        .unwrap_or(options.at);
    Ok(Thumbnail {
        image,
        format: options.format,
        width: rgb.width(),
        height: rgb.height(),
        stream_index: plan.stream_index,
        codec: plan.stream.codec,
        time,
        keyframe: choice.start == choice.target,
        decode_start: plan
            .samples
            .get(choice.start)
            .and_then(|s| s.time)
            .and_then(|t| t.checked_add(negate(plan.start)?))
            .unwrap_or(time),
        packet: plan
            .packets
            .get(choice.target)
            .map_or((0, 0), |p| (p.offset, p.size)),
        warnings: plan.warnings.clone(),
        packets_decoded: decoded,
        decoder,
        skipped,
    })
}

fn negate(r: Rational) -> Option<Rational> {
    Rational::new(r.num.checked_neg()?, r.den)
}

/// Reads packet `index`'s bytes, the stripped header first.
fn read_packet<S: Source>(
    source: &mut S,
    plan: &Plan,
    index: usize,
    max: u64,
) -> Result<Vec<u8>, Error> {
    let packet = plan
        .packets
        .get(index)
        .ok_or_else(|| Error::Unsupported("no such packet".into()))?;
    read_bytes(source, packet.offset, packet.size, &plan.prefix, max)
}

/// `prefix` and then the `size` bytes at `offset`; more than `max` bytes
/// together is a `Limit` error.
fn read_bytes<S: Source>(
    source: &mut S,
    offset: u64,
    size: u64,
    prefix: &[u8],
    max: u64,
) -> Result<Vec<u8>, Error> {
    let total = size.saturating_add(prefix.len() as u64);
    if total > max {
        return Err(Error::Decode(coreav_decode::Error::new(
            coreav_decode::ErrorKind::Limit,
            "packet exceeds the decode limits",
        )));
    }
    let size = usize::try_from(size).map_err(|_| Error::Unsupported("packet too large".into()))?;
    let mut data = Vec::new();
    data.try_reserve_exact(size.saturating_add(prefix.len()))
        .map_err(|_| Error::Unsupported("out of memory for a packet".into()))?;
    data.extend_from_slice(prefix);
    source.read_append(offset, size, &mut data)?;
    Ok(data)
}

/// Whether sync sample `index` starts decoding.
fn cut_point<S: Source>(source: &mut S, plan: &Plan, index: usize) -> Result<bool, Error> {
    let stated = plan.stated.get(index).copied().unwrap_or(true);
    let Some(packet) = plan.packets.get(index) else {
        return Ok(false);
    };
    starts_decoding(source, plan.rule, &plan.prefix, packet, stated)
}

/// Whether the frame of `packet` starts decoding under `rule`
/// (`docs/design/cut-points.md`); `stated` is the container's word, which
/// a frame whose coded data cannot be read keeps.
pub(crate) fn starts_decoding<S: Source>(
    source: &mut S,
    rule: CutRule,
    prefix: &[u8],
    packet: &PacketRef,
    stated: bool,
) -> Result<bool, Error> {
    let mut work = 0u32;
    let mut charge = || {
        work += 1;
        if work > 100_000 {
            Err(coreav_isobmff::Error::LimitExceeded {
                limit: coreav_isobmff::LimitKind::SampleWork,
                offset: packet.offset,
            })
        } else {
            Ok(())
        }
    };
    let read = match rule {
        CutRule::H264ByteStream => {
            // A byte stream's units are the stretches between its start
            // codes, so they are read from the packet's bytes rather than
            // walked through the source a unit at a time: its first
            // [`BYTE_STREAM_BYTES`], after the stripped prefix, which
            // belongs to them (and which bounds the read, so no limit
            // applies). Without a configuration record no
            // `max_num_ref_frames` is known, so an intra reference
            // picture of a one-reference stream is not taken for a cut
            // point; an IDR picture and an exact recovery point are, and
            // both are in those bytes.
            let size = packet.size.min(BYTE_STREAM_BYTES);
            let data = read_bytes(source, packet.offset, size, prefix, u64::MAX)?;
            coreav_isobmff::access::annex_b_frame_access(&data, size == packet.size)
                .map(|frame| frame.starts_decoding(None))
        }
        // The stored packet lacks the bytes the source holds (Matroska
        // header stripping), so the frame cannot be walked through it.
        _ if !prefix.is_empty() => return Ok(stated),
        CutRule::H264 {
            format,
            max_num_ref_frames,
        } => coreav_isobmff::access::read_starts_decoding(
            format,
            source,
            packet.offset,
            packet.size,
            max_num_ref_frames,
            &mut charge,
        )?,
        CutRule::Av1 => {
            coreav_isobmff::access::read_av1_sync(source, packet.offset, packet.size, &mut charge)?
        }
        CutRule::Hevc { length_size } => hevc_irap(source, packet, length_size)?,
        CutRule::Container => return Ok(true),
    };
    Ok(read.unwrap_or(stated))
}

/// Whether the first coded slice of an HEVC sample is an IRAP picture
/// (`nal_unit_type` 16 to 23, ITU-T H.265 7.4.2.2); `None` when the NAL
/// units cannot be walked within the first 64 units.
fn hevc_irap<S: Source>(
    source: &mut S,
    packet: &PacketRef,
    length_size: u8,
) -> Result<Option<bool>, Error> {
    let n = u64::from(length_size);
    if !(1..=4).contains(&n) {
        return Ok(None);
    }
    let mut at = 0u64;
    for _ in 0..64 {
        if at.saturating_add(n + 1) > packet.size {
            return Ok(None);
        }
        let mut head = [0u8; 5];
        let len_bytes = usize::from(length_size);
        let head = head
            .get_mut(..len_bytes + 1)
            .ok_or_else(|| Error::Unsupported("length size".into()))?;
        let offset = packet
            .offset
            .checked_add(at)
            .ok_or_else(|| Error::Unsupported("packet offset".into()))?;
        source.read_exact_at(offset, head)?;
        let (length, nal) = head.split_at(len_bytes);
        let length = length.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
        let kind = nal.first().map_or(63, |b| (b >> 1) & 0x3F);
        if kind <= 31 {
            return Ok(Some((16..=23).contains(&kind)));
        }
        at = at.saturating_add(n).saturating_add(length);
    }
    Ok(None)
}

/// Most NAL units of a packet read for its highest TemporalId.
const MAX_UNITS: usize = 4096;

/// The highest TemporalId of the slices of packet `index` (HEVC), from the
/// units' length prefixes and headers alone; the whole packet only when
/// the stored packet lacks a prefix (Matroska header stripping). `None`
/// when a unit cannot be read, or past [`MAX_UNITS`].
fn packet_temporal_id<S: Source>(
    source: &mut S,
    plan: &Plan,
    index: usize,
    rule: skip::Rule,
    options: &ThumbnailOptions,
) -> Result<Option<u8>, Error> {
    if !plan.prefix.is_empty() {
        let data = read_packet(source, plan, index, options.decode_limits.max_packet_bytes)?;
        return Ok(rule.temporal_id(&data));
    }
    let packet = plan
        .packets
        .get(index)
        .ok_or_else(|| Error::Unsupported("no such packet".into()))?;
    let length = rule.length();
    let n = u64::try_from(length).unwrap_or(u64::MAX);
    let mut top = 0u8;
    let mut at = 0u64;
    for _ in 0..MAX_UNITS {
        if at == packet.size {
            return Ok(Some(top));
        }
        // The length prefix and the two-byte NAL unit header.
        let mut head = [0u8; 6];
        let Some(head) = head.get_mut(..length + 2) else {
            return Ok(None);
        };
        if at.saturating_add(n + 2) > packet.size {
            return Ok(None);
        }
        let offset = packet
            .offset
            .checked_add(at)
            .ok_or_else(|| Error::Unsupported("packet offset".into()))?;
        source.read_exact_at(offset, head)?;
        let (size, nal) = head.split_at(length);
        let size = size.iter().fold(0u64, |v, b| (v << 8) | u64::from(*b));
        if let [first, second] = nal {
            if (first >> 1) & 0x3F < 32 {
                top = top.max((second & 7).saturating_sub(1));
            }
        }
        at = at.saturating_add(n).saturating_add(size);
        if at > packet.size {
            return Ok(None);
        }
    }
    Ok(None)
}

/// Bytes of a byte-stream packet read for its cut point: the parameter
/// sets, any SEI messages ([`coreav_isobmff::access`] reads at most 4,096
/// bytes of one) and the first slice header of a frame fit in them. A
/// frame whose first slice lies past them cannot be read, so it keeps the
/// container's word.
const BYTE_STREAM_BYTES: u64 = 8_192;

/// Packets after the target a second decode sends at most, past the
/// reordering and the reference frames a decoder may hold (the seek
/// window's margin).
const HELD_BACK: usize = 32;

/// Decodes packets `choice.start..=choice.target` and returns the target's
/// frame.
///
/// A decoder may hold a frame back until later packets arrive and drop it
/// at the end of the stream: FFmpeg's H.264 decoder does so for pictures
/// after an open GOP's I picture (a recovery point, ITU-T H.264 D.2.8),
/// which decoded from it are the frames a decode from the start gives.
/// When the target's frame does not come, the packets are decoded again
/// with those after the target, in decode order up to the next keyframe and
/// at most [`HELD_BACK`] of them, within what is left of the timeout.
fn decode<S: Source>(
    source: &mut S,
    plan: &Plan,
    choice: Choice,
    options: &ThumbnailOptions,
    backends: &[&dyn Backend],
) -> Result<(Frame, DecoderInfo, Vec<Skipped>, u32), Error> {
    let started = std::time::Instant::now();
    let limits = &options.decode_limits;
    let (found, info, skipped, decoded) = decode_through(
        source,
        plan,
        choice,
        choice.target,
        options,
        limits,
        backends,
    )?;
    if let Some(frame) = found {
        return Ok((frame, info, skipped, decoded));
    }
    let next_keyframe = plan
        .packets
        .iter()
        .enumerate()
        .skip(choice.target.saturating_add(1))
        .find(|(_, p)| p.keyframe)
        .map_or(plan.packets.len(), |(i, _)| i);
    let last = next_keyframe
        .saturating_sub(1)
        .min(choice.target.saturating_add(HELD_BACK));
    // Both passes share the timeout.
    let left = limits.timeout.saturating_sub(started.elapsed());
    let found = if last > choice.target && !left.is_zero() {
        let limits = coreav_decode::Limits {
            timeout: left,
            ..limits.clone()
        };
        let (found, info, skipped, more) =
            decode_through(source, plan, choice, last, options, &limits, backends)?;
        found.map(|frame| (frame, info, skipped, decoded.saturating_add(more)))
    } else {
        None
    };
    found.ok_or_else(|| {
        Error::Decode(coreav_decode::Error::new(
            coreav_decode::ErrorKind::InvalidData,
            "the decoder returned no frame for the packet",
        ))
    })
}

/// Decodes packets `choice.start..=last` (`last` at or after the target),
/// discarding every frame but the target's, which it returns when the
/// decoder gives it.
#[allow(clippy::too_many_arguments)]
fn decode_through<S: Source>(
    source: &mut S,
    plan: &Plan,
    choice: Choice,
    last: usize,
    options: &ThumbnailOptions,
    limits: &coreav_decode::Limits,
    backends: &[&dyn Backend],
) -> Result<(Option<Frame>, DecoderInfo, Vec<Skipped>, u32), Error> {
    let opened = coreav_decode::open(&plan.stream, &options.policy, limits, backends)?;
    let mut decoder: Box<dyn Decoder> = opened.decoder;
    let target_pts = plan
        .packets
        .get(choice.target)
        .map(|p| p.pts)
        .ok_or_else(|| Error::Unsupported("no such packet".into()))?;
    let mut found = None;
    let mut decoded = 0u32;
    let mut skip = options
        .skip_disposable
        .then(|| skip::Rule::of(plan.stream.codec, plan.stream.config.as_ref()))
        .flatten();
    // HEVC: the highest TemporalId of the packets to decode, read from them
    // first; one that cannot be read turns skipping off.
    let mut top = 0u8;
    if let Some(rule) = skip.filter(|r| r.needs_temporal_ids()) {
        for index in choice.start..=last {
            match packet_temporal_id(source, plan, index, rule, options)? {
                Some(id) => top = top.max(id),
                None => {
                    skip = None;
                    break;
                }
            }
        }
    }
    for index in choice.start..=last {
        let data = read_packet(source, plan, index, options.decode_limits.max_packet_bytes)?;
        let packet = plan
            .packets
            .get(index)
            .ok_or_else(|| Error::Unsupported("no such packet".into()))?;
        // A picture other than the target that nothing depends on need not
        // be decoded at all.
        if index != choice.start
            && index != choice.target
            && packet.pts != target_pts
            && skip.is_some_and(|rule| rule.disposable(&data, top))
        {
            continue;
        }
        decoder.send_packet(&Packet {
            data: &data,
            pts: Some(packet.pts),
            dts: packet.dts,
            keyframe: packet.keyframe,
            // Only the target's frame is wanted.
            discard: index != choice.target && packet.pts != target_pts,
        })?;
        decoded = decoded.saturating_add(1);
        while let Some(frame) = decoder.receive_frame()? {
            if frame.pts == Some(target_pts) {
                found = Some(frame);
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    if found.is_none() {
        decoder.send_eof()?;
        // Frames come in presentation order: stop at the target, without
        // waiting for the decoder to drain.
        while let Some(frame) = decoder.receive_frame()? {
            if frame.pts == Some(target_pts) {
                found = Some(frame);
                break;
            }
        }
    }
    let info = decoder.info().clone();
    Ok((found, info, opened.skipped, decoded))
}

/// Turns, squares and resizes the frame into an RGB picture.
fn render(frame: &Frame, plan: &Plan, options: &ThumbnailOptions) -> Result<Picture, Error> {
    let picture = frame.to_cpu()?;
    let (pw, ph) = (u64::from(picture.width()), u64::from(picture.height()));
    let (dw, dh) = match plan.pixel_aspect {
        // Stretch the width to make pixels square.
        Some((h, v)) if h > 0 && v > 0 && h != v => (scale(pw, h, v).max(1), ph),
        _ => (pw, ph),
    };
    let turned = plan.quarter_turns % 2 == 1;
    let (vw, vh) = if turned { (dh, dw) } else { (dw, dh) };
    let (tw, th) = match (options.width, options.height) {
        (Some(w), Some(h)) => (u64::from(w), u64::from(h)),
        (Some(w), None) => (u64::from(w), scale(u64::from(w), vh, vw).max(1)),
        (None, Some(h)) => (scale(u64::from(h), vw, vh).max(1), u64::from(h)),
        (None, None) => (vw, vh),
    };
    let side = |v: u64| {
        u32::try_from(v)
            .ok()
            .filter(|v| (1..=MAX_SIDE).contains(v))
            .ok_or_else(|| Error::Unsupported(format!("image size {v} outside 1 to {MAX_SIDE}")))
    };
    let (tw, th) = (side(tw)?, side(th)?);
    let turns = plan.quarter_turns % 4;
    if turns == 0 {
        return Ok(picture.resize_to_rgb(tw, th, options.filter, &frame.color, options.chroma)?);
    }
    // Turn the decoded picture first, as FFmpeg's autorotation does, so
    // that chroma is resampled in the image's own orientation; a quarter
    // turn of 4:2:2 has no Y'CbCr format, so that one turns the RGB result.
    // So does full chroma upsampling, which honours the chroma siting: the
    // siting describes the picture as decoded, not turned.
    let turn_first = options.chroma != ChromaUpsampling::Full;
    let rotated = if turn_first {
        picture.rotate(turns)
    } else {
        Err(FrameError::Unsupported("convert first"))
    };
    match rotated {
        Ok(turned_picture) => Ok(turned_picture.resize_to_rgb(
            tw,
            th,
            options.filter,
            &frame.color,
            options.chroma,
        )?),
        Err(FrameError::Unsupported(_)) => {
            let (rw, rh) = if turned { (th, tw) } else { (tw, th) };
            let rgb =
                picture.resize_to_rgb(rw, rh, options.filter, &frame.color, options.chroma)?;
            Ok(rgb.rotate(turns)?)
        }
        Err(e) => Err(e.into()),
    }
}

/// `a × num / den`, rounded, without overflow (the factors come from the
/// file); saturates at `u64::MAX`, which the size check then refuses.
fn scale(a: u64, num: u64, den: u64) -> u64 {
    let (a, num, den) = (u128::from(a), u128::from(num), u128::from(den));
    let product = a.saturating_mul(num);
    let value = product
        .saturating_add(den / 2)
        .checked_div(den)
        .unwrap_or(product);
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::scale;

    #[test]
    fn sizes_from_the_file_cannot_overflow() {
        assert_eq!(scale(320, 9, 16), 180);
        assert_eq!(scale(5, 1, 2), 3);
        // A display width of 2^58 and a requested height of 256.
        assert_eq!(scale(256, 1 << 58, 64), 1 << 60);
        assert_eq!(scale(u64::MAX, u64::MAX, 1), u64::MAX);
        assert_eq!(scale(7, 3, 0), 21);
    }
}

#[cfg(test)]
mod skip_tests {
    use std::sync::{Arc, Mutex};

    use coreav_decode::{
        BackendKind, CodecConfig, ConfigKind, Error as DecodeError, Isolation, Limits,
        StreamTraits, Support,
    };
    use coreav_frame::{ColorInfo, PixelFormat, Pixels};
    use coreav_isobmff::SliceSource;

    use super::*;

    /// A backend that records the pts of every packet it is sent and
    /// returns a frame for each.
    struct Recorder(Arc<Mutex<Vec<i64>>>);

    struct RecordingDecoder {
        sent: Arc<Mutex<Vec<i64>>>,
        ready: Vec<i64>,
        info: DecoderInfo,
    }

    impl Backend for Recorder {
        fn kind(&self) -> BackendKind {
            BackendKind::Fallback
        }
        fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
            Support::Yes
        }
        fn open(&self, _: &StreamConfig, _: &Limits) -> Result<Box<dyn Decoder>, DecodeError> {
            Ok(Box::new(RecordingDecoder {
                sent: Arc::clone(&self.0),
                ready: Vec::new(),
                info: DecoderInfo {
                    backend: BackendKind::Fallback,
                    isolation: Isolation::InProcess,
                    decoder: "recorder".into(),
                },
            }))
        }
    }

    impl Decoder for RecordingDecoder {
        fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), DecodeError> {
            let pts = packet.pts.unwrap_or(-1);
            self.sent.lock().expect("lock").push(pts);
            self.ready.push(pts);
            Ok(())
        }
        fn send_eof(&mut self) -> Result<(), DecodeError> {
            Ok(())
        }
        fn receive_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
            if self.ready.is_empty() {
                return Ok(None);
            }
            let pts = self.ready.remove(0);
            let picture = Picture::zeroed(2, 2, PixelFormat::YUV420P).expect("picture");
            Ok(Some(Frame {
                pts: Some(pts),
                color: ColorInfo::default(),
                pixels: Pixels::Cpu(picture),
            }))
        }
        fn info(&self) -> &DecoderInfo {
            &self.info
        }
    }

    /// One length-prefixed (4 bytes) HEVC unit of `kind` at TemporalId
    /// `temporal`.
    fn unit(kind: u8, temporal: u8) -> Vec<u8> {
        vec![0, 0, 0, 3, kind << 1, temporal + 1, 0xAF]
    }

    #[test]
    fn hevc_temporal_layers_come_from_the_packets_not_hvcc() {
        // hvcC declares one temporal layer, which the stream exceeds: a
        // TemporalId 1 picture is decoded, so the TRAIL_N picture at
        // TemporalId 0, which it may reference, must be sent; the TRAIL_N
        // picture at TemporalId 1 is not.
        let packets: Vec<Vec<u8>> = vec![
            unit(19, 0), // IDR_W_RADL
            unit(0, 0),  // TRAIL_N, TemporalId 0
            unit(0, 1),  // TRAIL_N, TemporalId 1
            unit(1, 1),  // TRAIL_R, TemporalId 1: the target
        ];
        let mut file = Vec::new();
        let mut refs = Vec::new();
        for (i, p) in packets.iter().enumerate() {
            refs.push(PacketRef {
                offset: file.len() as u64,
                size: p.len() as u64,
                pts: i as i64,
                dts: Some(i as i64),
                keyframe: i == 0,
            });
            file.extend_from_slice(p);
        }
        let mut hvcc = vec![0u8; 23];
        hvcc[21] = (1 << 3) | 3;
        let plan = Plan {
            warnings: Vec::new(),
            stream_index: 0,
            stream: StreamConfig {
                config: Some(CodecConfig {
                    kind: ConfigKind::Hvcc,
                    data: hvcc,
                }),
                ..StreamConfig::new(Codec::Hevc)
            },
            quarter_turns: 0,
            pixel_aspect: None,
            start: Rational::from_int(0),
            samples: Vec::new(),
            packets: refs,
            prefix: Vec::new(),
            rule: CutRule::Hevc { length_size: 4 },
            stated: vec![true; 4],
            window: None,
            first_keyframe: None,
            container: Container::Mp4,
            durations: vec![None; 4],
            time_base: Rational::from_int(1),
            presentation: Presentation::All,
        };
        let sent = Arc::new(Mutex::new(Vec::new()));
        let backend = Recorder(Arc::clone(&sent));
        let backends: [&dyn Backend; 1] = [&backend];
        let options = ThumbnailOptions::at(Rational::from_int(0));
        let mut source = SliceSource::new(&file);
        let choice = Choice {
            start: 0,
            target: 3,
        };
        decode(&mut source, &plan, choice, &options, &backends).expect("decoded");
        assert_eq!(*sent.lock().expect("lock"), vec![0, 1, 3]);
        // Without TemporalId 1 pictures, TRAIL_N at TemporalId 0 is the top
        // layer, which nothing decoded references: skipped.
        sent.lock().expect("lock").clear();
        let low: Vec<Vec<u8>> = vec![unit(19, 0), unit(0, 0), unit(1, 0)];
        let mut low_file = Vec::new();
        let mut low_refs = Vec::new();
        for (i, p) in low.iter().enumerate() {
            low_refs.push(PacketRef {
                offset: low_file.len() as u64,
                size: p.len() as u64,
                pts: i as i64,
                dts: Some(i as i64),
                keyframe: i == 0,
            });
            low_file.extend_from_slice(p);
        }
        let low_plan = Plan {
            packets: low_refs,
            stated: vec![true; 3],
            window: None,
            first_keyframe: None,
            ..plan
        };
        let mut low_source = SliceSource::new(&low_file);
        let choice = Choice {
            start: 0,
            target: 2,
        };
        decode(&mut low_source, &low_plan, choice, &options, &backends).expect("decoded");
        assert_eq!(*sent.lock().expect("lock"), vec![0, 2]);
    }

    /// A backend whose frames come only once `hold` more packets follow
    /// them, and whose held frames are dropped at the end of the stream, as
    /// FFmpeg's H.264 decoder does after an open GOP's I picture.
    struct Holding {
        sent: Arc<Mutex<Vec<i64>>>,
        hold: usize,
    }

    struct HoldingDecoder {
        sent: Arc<Mutex<Vec<i64>>>,
        hold: usize,
        held: Vec<i64>,
        info: DecoderInfo,
    }

    impl Backend for Holding {
        fn kind(&self) -> BackendKind {
            BackendKind::Fallback
        }
        fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
            Support::Yes
        }
        fn open(&self, _: &StreamConfig, _: &Limits) -> Result<Box<dyn Decoder>, DecodeError> {
            Ok(Box::new(HoldingDecoder {
                sent: Arc::clone(&self.sent),
                hold: self.hold,
                held: Vec::new(),
                info: DecoderInfo {
                    backend: BackendKind::Fallback,
                    isolation: Isolation::InProcess,
                    decoder: "holding".into(),
                },
            }))
        }
    }

    impl Decoder for HoldingDecoder {
        fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), DecodeError> {
            let pts = packet.pts.unwrap_or(-1);
            self.sent.lock().expect("lock").push(pts);
            self.held.push(pts);
            Ok(())
        }
        fn send_eof(&mut self) -> Result<(), DecodeError> {
            self.held.clear();
            Ok(())
        }
        fn receive_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
            if self.held.len() <= self.hold {
                return Ok(None);
            }
            let pts = self.held.remove(0);
            let picture = Picture::zeroed(2, 2, PixelFormat::YUV420P).expect("picture");
            Ok(Some(Frame {
                pts: Some(pts),
                color: ColorInfo::default(),
                pixels: Pixels::Cpu(picture),
            }))
        }
        fn info(&self) -> &DecoderInfo {
            &self.info
        }
    }

    #[test]
    fn a_frame_held_back_is_decoded_again_with_the_packets_after_it() {
        // Two GOPs of HEVC reference pictures (IDR, then TRAIL_R), keyframes
        // at 0 and 6; the target is packet 2.
        let mut file = Vec::new();
        let mut refs = Vec::new();
        for i in 0..8usize {
            let p = unit(if i % 6 == 0 { 19 } else { 1 }, 0);
            refs.push(PacketRef {
                offset: file.len() as u64,
                size: p.len() as u64,
                pts: i as i64,
                dts: Some(i as i64),
                keyframe: i % 6 == 0,
            });
            file.extend_from_slice(&p);
        }
        let mut hvcc = vec![0u8; 23];
        hvcc[21] = 3;
        let plan = Plan {
            warnings: Vec::new(),
            stream_index: 0,
            stream: StreamConfig {
                config: Some(CodecConfig {
                    kind: ConfigKind::Hvcc,
                    data: hvcc,
                }),
                ..StreamConfig::new(Codec::Hevc)
            },
            quarter_turns: 0,
            pixel_aspect: None,
            start: Rational::from_int(0),
            samples: Vec::new(),
            packets: refs,
            prefix: Vec::new(),
            rule: CutRule::Hevc { length_size: 4 },
            stated: vec![true; 8],
            window: None,
            first_keyframe: None,
            container: Container::Mp4,
            durations: vec![None; 8],
            time_base: Rational::from_int(1),
            presentation: Presentation::All,
        };
        let options = ThumbnailOptions::at(Rational::from_int(0));
        let choice = Choice {
            start: 0,
            target: 2,
        };
        // Held for two packets: the first pass ends at the target and loses
        // its frame; the second sends packets 3 and 4 and stops there.
        let sent = Arc::new(Mutex::new(Vec::new()));
        let backend = Holding {
            sent: Arc::clone(&sent),
            hold: 2,
        };
        let backends: [&dyn Backend; 1] = [&backend];
        let mut source = SliceSource::new(&file);
        let (frame, _, _, decoded) =
            decode(&mut source, &plan, choice, &options, &backends).expect("decoded");
        assert_eq!(frame.pts, Some(2));
        assert_eq!(*sent.lock().expect("lock"), vec![0, 1, 2, 0, 1, 2, 3, 4]);
        assert_eq!(decoded, 8);
        // A decoder that holds nothing back is sent the packets once.
        sent.lock().expect("lock").clear();
        let backend = Holding {
            sent: Arc::clone(&sent),
            hold: 0,
        };
        let backends: [&dyn Backend; 1] = [&backend];
        decode(&mut source, &plan, choice, &options, &backends).expect("decoded");
        assert_eq!(*sent.lock().expect("lock"), vec![0, 1, 2]);
        // Held for longer than the GOP lasts: the next keyframe bounds the
        // second pass, and the error is the decoder's as before.
        sent.lock().expect("lock").clear();
        let backend = Holding {
            sent: Arc::clone(&sent),
            hold: 4,
        };
        let backends: [&dyn Backend; 1] = [&backend];
        assert!(matches!(
            decode(&mut source, &plan, choice, &options, &backends),
            Err(Error::Decode(_))
        ));
        assert_eq!(*sent.lock().expect("lock"), vec![0, 1, 2, 0, 1, 2, 3, 4, 5]);
    }
}
