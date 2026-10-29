//! `coreav ffmpeg`: `ffmpeg`'s stream-copy command lines over CoreAV's
//! writers (`docs/design/compat-cli.md`, "The command").
//!
//! [`args::parse`] reads the command line and refuses what is outside the
//! vocabulary. [`plan`] reads the input's streams as `coreav ffprobe` lists
//! them, selects the streams as `ffmpeg` does (`-map`, or one video, one
//! audio and one subtitle stream chosen automatically, less `-vn`, `-an`,
//! `-sn`, `-dn`), and picks the writer: a remux, a clip, a split or an
//! audio extraction. [`run`] writes the output. Whatever a writer does not
//! implement is a [`NotNative`] refusal, found before any output is
//! written.

pub mod args;
#[cfg(feature = "transcode")]
pub mod encode;

use crate::error::Failure;
use crate::ffprobe::args::Kind2;
use crate::ffprobe::model::Value;
use crate::ffprobe::{Report, Stream, StreamKind};
use crate::output::{self, WriteFailure};
use crate::split::Pattern;
use args::{Codec, Command, Demuxer, Muxer, Overwrite, Spec};
use coreav_isobmff::{FileSource, Rational};
use coreav_isobmff_write::{
    AudioChoice, ClipOptions, MatroskaRemuxOptions, OnUnsupported, RemuxOptions, SplitAt,
    SplitOptions, TrackChoice, VideoChoice,
};
use std::path::{Path, PathBuf};

/// What a command that is not native names: the option, with its value
/// when the value is what is refused, or a property of the command or of
/// its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// An option as given (`-vf`, `-c:v`), and the refused value.
    Option(String, Option<String>),
    /// A fixed description that holds no user data.
    Property(&'static str),
}

/// A command `coreav ffmpeg` does not run itself (exit status 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotNative {
    /// What is not native.
    pub reason: Reason,
    /// The message for standard error.
    pub message: String,
}

/// A command that is not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not native: exit 4.
    NotNative(NotNative),
    /// Malformed, as `ffmpeg` would also fail it: exit 2.
    Usage(String),
}

impl Refusal {
    /// A refusal for a property of the command or of its input.
    pub fn not_native(property: &'static str) -> Self {
        Self::NotNative(NotNative {
            reason: Reason::Property(property),
            message: format!("not supported by coreav ffmpeg: {property}"),
        })
    }
}

/// Why a command stopped.
pub enum Stop {
    /// It was refused before the input was read, or once its streams were.
    Refused(Refusal),
    /// The input was rejected or could not be read, or the output could not
    /// be written: exit 1, or 3 when `WriteFailure::is_io`.
    Failed(WriteFailure),
}

impl From<Refusal> for Stop {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<WriteFailure> for Stop {
    fn from(failure: WriteFailure) -> Self {
        match not_native_reason(&failure) {
            Some(reason) => Self::Refused(Refusal::not_native(reason)),
            None => Self::Failed(failure),
        }
    }
}

/// What [`plan`] chose: the writer and its options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// The whole input, these streams (`Track::stream_index`), as MP4.
    Remux {
        /// The streams, in input order.
        streams: Vec<usize>,
        /// The input is Matroska or WebM.
        matroska: bool,
    },
    /// One AAC stream as M4A.
    ExtractAudio {
        /// The stream.
        stream: usize,
    },
    /// A keyframe-aligned clip.
    Clip(ClipOptions),
    /// Segments, written to the names of the output pattern.
    Split(SplitOptions),
    /// The video stream encoded with libx264 and these audio streams copied,
    /// as MP4 (`docs/design/transcode.md`, "Through `coreav ffmpeg`").
    Transcode {
        /// The video stream.
        video: usize,
        /// The audio streams, in input order.
        audio: Vec<usize>,
        /// Whether the decoder reorders the video's frames (B-frames).
        reordered: bool,
    },
}

/// The container families CoreAV reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Container {
    Isobmff,
    Matroska,
}

/// Parses, plans and runs a command line. Returns the summary its writer's
/// own command prints (`coreav remux`, `clip`, `split`, `extract-audio`),
/// which `coreav ffmpeg` does not print; the fixture tests check outputs
/// with it.
pub fn run(args: &[String]) -> Result<serde_json::Value, Stop> {
    let command = args::parse(args)?;
    let input = Path::new(&command.input);
    let size = std::fs::metadata(input)
        .map_err(|e| WriteFailure::Rejected(Failure::io(e)))?
        .len();
    let mut file = FileSource::open(input).map_err(WriteFailure::from)?;
    let force = command.demuxer.map(|d| match d {
        Demuxer::Isobmff => crate::ffprobe::args::Container::Isobmff,
        Demuxer::Matroska => crate::ffprobe::args::Container::Matroska,
    });
    let report = crate::ffprobe::probe(&mut file, size, &command.input, force)
        .map_err(WriteFailure::Rejected)?
        .report;
    let job = plan(&command, &report)?;
    write(&command, &job)
}

/// The writer and options for `command` on the input `report` describes.
pub fn plan(command: &Command, report: &Report) -> Result<Job, Refusal> {
    let container = match format_name(report) {
        Some("mov,mp4,m4a,3gp,3g2,mj2") => Container::Isobmff,
        Some("matroska,webm") => Container::Matroska,
        Some("mpegts") => {
            return Err(Refusal::not_native(
                "copying from an MPEG-TS input (no writer of CoreAV reads one)",
            ));
        }
        _ => return Err(Refusal::not_native("an input of this container")),
    };
    let streams = &report.streams;
    let selected = select(command, streams)?;
    let kind = |i: usize| streams.get(i).map(|s| s.kind);
    let of = |k: StreamKind| {
        selected
            .iter()
            .copied()
            .filter(move |i| kind(*i) == Some(k))
    };
    let one_video_one_audio = || {
        let videos: Vec<usize> = of(StreamKind::Video).collect();
        let audios: Vec<usize> = of(StreamKind::Audio).collect();
        match (videos.as_slice(), audios.as_slice()) {
            _ if videos.len() + audios.len() != selected.len() => Err(Refusal::not_native(
                "a clip or a segment of other streams than one video and one audio stream",
            )),
            ([video], [audio]) => Ok((*video, AudioChoice::Stream(*audio))),
            ([video], []) => Ok((*video, AudioChoice::None)),
            ([], _) => Err(Refusal::not_native("a clip or a segment without video")),
            _ => Err(Refusal::not_native(
                "a clip or a segment of more than one video or audio stream",
            )),
        }
    };
    let trims = command.input_seek.is_some()
        || command.input_duration.is_some()
        || command.input_until.is_some()
        || command.seek.is_some()
        || command.duration.is_some()
        || command.until.is_some();
    if command.strip_metadata && command.muxer != Muxer::Ipod {
        return Err(Refusal::not_native(
            "-map_metadata -1 on another output than an M4A file",
        ));
    }
    let output: Vec<(usize, &Stream)> = selected
        .iter()
        .filter_map(|i| streams.get(*i).map(|s| (*i, s)))
        .collect();
    let encoded: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|i| codec(command, &output, *i) == Some(Codec::Libx264))
        .collect();
    if !encoded.is_empty() {
        return transcode_job(command, report, &output, &encoded, trims);
    }
    if (args::Encoding {
        threads: None,
        ..command.encoding.clone()
    }) != args::Encoding::default()
    {
        return Err(Refusal::not_native(
            "encoder options while every selected stream is copied",
        ));
    }
    if command.muxer == Muxer::Segment {
        if trims {
            return Err(Refusal::not_native(
                "a segmented output that also seeks or trims",
            ));
        }
        if container != Container::Isobmff {
            return Err(Refusal::not_native("segmenting a Matroska or WebM input"));
        }
        if Pattern::parse(&command.output).is_none() {
            return Err(Refusal::not_native("a segment name without one %d or %0Nd"));
        }
        let (video, audio) = one_video_one_audio()?;
        return Ok(Job::Split(SplitOptions {
            video: VideoChoice::Stream(video),
            audio,
            at: SplitAt::Every(command.segment_time),
        }));
    }
    if trims {
        if container != Container::Isobmff {
            return Err(Refusal::not_native("clipping a Matroska or WebM input"));
        }
        if command.muxer != Muxer::Mp4 {
            return Err(Refusal::not_native("a clip written as M4A"));
        }
        let (video, audio) = one_video_one_audio()?;
        let (start, end) = clip_range(command, input_start(report))
            .ok_or(Refusal::not_native("a time CoreAV cannot represent"))?;
        return Ok(Job::Clip(ClipOptions {
            video: VideoChoice::Stream(video),
            audio,
            start,
            end,
        }));
    }
    if command.muxer == Muxer::Ipod {
        if container != Container::Isobmff {
            return Err(Refusal::not_native(
                "an M4A output from a Matroska or WebM input",
            ));
        }
        let [stream] = selected.as_slice() else {
            return Err(Refusal::not_native(
                "an M4A output of other than one audio stream",
            ));
        };
        let Some(chosen) = streams.get(*stream).filter(|s| s.kind == StreamKind::Audio) else {
            return Err(Refusal::not_native(
                "an M4A output of other than one audio stream",
            ));
        };
        if command.strip_metadata
            && tag(chosen, "language").is_some_and(|language| language != "und")
        {
            return Err(Refusal::not_native(
                "-map_metadata -1 on a stream with a language",
            ));
        }
        return Ok(Job::ExtractAudio { stream: *stream });
    }
    Ok(Job::Remux {
        streams: selected,
        matroska: container == Container::Matroska,
    })
}

/// The job of a command that encodes: one video stream with libx264, the
/// other selected streams audio, copied, into one MP4 file.
fn transcode_job(
    command: &Command,
    report: &Report,
    output: &[(usize, &Stream)],
    encoded: &[usize],
    trims: bool,
) -> Result<Job, Refusal> {
    let [video] = encoded else {
        return Err(Refusal::not_native("encoding more than one stream"));
    };
    if !output
        .iter()
        .any(|(i, s)| i == video && s.kind == StreamKind::Video && !s.attached_pic)
    {
        return Err(Refusal::not_native(
            "encoding a stream other than video with libx264",
        ));
    }
    if command.muxer != Muxer::Mp4 {
        return Err(Refusal::not_native(
            "an encoded output other than one MP4 file",
        ));
    }
    if trims {
        return Err(Refusal::not_native("seeking or trimming while encoding"));
    }
    if !report.chapters.is_empty() || report.streams.iter().any(|s| tag(s, "timecode").is_some()) {
        return Err(Refusal::not_native(
            "an input with chapters or a timecode (ffmpeg writes them as tracks of their own)",
        ));
    }
    if !command.encoding.passthrough
        && output
            .iter()
            .any(|(i, s)| i == video && int(s, "start_pts") != 0)
    {
        return Err(Refusal::not_native(
            "a video that does not start at time 0, without -fps_mode passthrough \
             (ffmpeg would repeat its first frame to fill the start)",
        ));
    }
    let others: Vec<(usize, &Stream)> =
        output.iter().filter(|(i, _)| i != video).copied().collect();
    if others.iter().any(|(i, _)| i < video) {
        return Err(Refusal::not_native(
            "audio before the video in the input (ffmpeg writes the video first; the transcode keeps the input's order)",
        ));
    }
    if others.iter().any(|(_, s)| s.kind != StreamKind::Audio) {
        return Err(Refusal::not_native(
            "copying a stream other than audio beside the encoded video",
        ));
    }
    Ok(Job::Transcode {
        video: *video,
        audio: others.iter().map(|(i, _)| *i).collect(),
        reordered: output
            .iter()
            .any(|(i, s)| i == video && int(s, "has_b_frames") > 0),
    })
}

/// The codec option that decides stream `index` of `output`: the last one
/// that names it, as in `ffmpeg` (`-c:v libx264 -c copy` copies the video).
fn codec(command: &Command, output: &[(usize, &Stream)], index: usize) -> Option<Codec> {
    command
        .codecs
        .iter()
        .rev()
        .find(|(spec, _)| matching(spec, output).contains(&index))
        .map(|(_, codec)| *codec)
}

/// Where a clip starts and ends on the input's presentation timeline, in
/// `ffmpeg`'s meaning of the options: every time counts from `origin`,
/// the input's start time; an input `-ss` shifts the output's time 0 to
/// it, so an output `-to` counts from there; an input `-t` or `-to` stops
/// reading the input; an output `-t` counts from the output `-ss`; `-t`
/// wins over `-to` given with it. A start before the timeline's 0 (a
/// negative start time) is 0.
fn clip_range(command: &Command, origin: Rational) -> Option<(Rational, Option<Rational>)> {
    let zero = Rational::from_int(0);
    let shift = origin.checked_add(command.input_seek.unwrap_or(zero))?;
    let start = shift.checked_add(command.seek.unwrap_or(zero))?;
    let input_end = match (command.input_duration, command.input_until) {
        (Some(duration), _) => Some(shift.checked_add(duration)?),
        (None, Some(until)) => Some(origin.checked_add(until)?),
        (None, None) => None,
    };
    let output_end = match (command.duration, command.until) {
        (Some(duration), _) => Some(start.checked_add(duration)?),
        (None, Some(until)) => Some(shift.checked_add(until)?),
        (None, None) => None,
    };
    let end = match (input_end, output_end) {
        (Some(a), Some(b)) => Some(if a.checked_lt(b)? { a } else { b }),
        (a, b) => a.or(b),
    };
    let start = if start.checked_lt(zero)? { zero } else { start };
    Some((start, end))
}

/// The input's start time as `ffprobe` prints it (whole microseconds),
/// from which `ffmpeg` counts `-ss`, `-t` and `-to`; 0 when not known.
fn input_start(report: &Report) -> Rational {
    report
        .format
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (&"start_time", Value::Time(seconds)) => {
                Rational::new(i128::from((seconds * 1e6).round() as i64), 1_000_000)
            }
            _ => None,
        })
        .unwrap_or(Rational::from_int(0))
}

fn format_name(report: &Report) -> Option<&str> {
    report
        .format
        .iter()
        .find_map(|(key, value)| match (key, value) {
            (&"format_name", Value::Str(name)) => Some(name.as_str()),
            _ => None,
        })
}

fn int(stream: &Stream, key: &str) -> i64 {
    stream
        .entries
        .iter()
        .find_map(|(k, v)| match v {
            Value::Int(n) if *k == key => Some(*n),
            _ => None,
        })
        .unwrap_or(0)
}

fn is_default(stream: &Stream) -> bool {
    stream
        .disposition
        .iter()
        .any(|(k, v)| *k == "default" && matches!(v, Value::Int(1)))
}

fn tag<'a>(stream: &'a Stream, key: &str) -> Option<&'a str> {
    stream
        .tags
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn is_kind(kind: Kind2, stream: &Stream) -> bool {
    match kind {
        Kind2::Video => stream.kind == StreamKind::Video,
        Kind2::VideoNotPicture => stream.kind == StreamKind::Video && !stream.attached_pic,
        Kind2::Audio => stream.kind == StreamKind::Audio,
        Kind2::Subtitle => stream.kind == StreamKind::Subtitle,
        Kind2::Data => stream.kind == StreamKind::Data,
        Kind2::Attachment => stream.kind == StreamKind::Attachment,
    }
}

/// The indexes of `streams` that `spec` names, in order. `position` is
/// where each of them sits among `streams`.
fn matching(spec: &Spec, streams: &[(usize, &Stream)]) -> Vec<usize> {
    let mut kind_seen = 0;
    let mut out = Vec::new();
    for (position, (index, stream)) in streams.iter().enumerate() {
        let hit = match spec {
            Spec::All => true,
            Spec::Index(n) => *n == position,
            Spec::Kind(kind, n) => {
                if !is_kind(*kind, stream) {
                    false
                } else {
                    let ordinal = kind_seen;
                    kind_seen += 1;
                    n.is_none_or(|n| n == ordinal)
                }
            }
            Spec::Language(value) => match (tag(stream, "language"), value) {
                (Some(language), Some(value)) => language == value,
                (Some(_), None) => true,
                (None, _) => false,
            },
        };
        if hit {
            out.push(*index);
        }
    }
    out
}

/// Subtitle codecs that are text, of which `ffmpeg` selects the first
/// automatically for an MP4 output when a codec option names the subtitle
/// type alone (`-c:s`, `-scodec`; neither `-c` nor `-c:s:0` does).
const TEXT_SUBTITLES: [&str; 7] = ["mov_text", "subrip", "srt", "ass", "ssa", "webvtt", "text"];

/// The streams `ffmpeg` would write, in order: `-map`, or the automatic
/// selection (the video and the audio stream with the default disposition
/// first, then the largest picture or the most channels, the first of
/// equals), less the disabled types. Each must be covered by a `copy`
/// codec option, appear once and keep the input's order.
fn select(command: &Command, streams: &[Stream]) -> Result<Vec<usize>, Refusal> {
    let all: Vec<(usize, &Stream)> = streams.iter().enumerate().collect();
    let disabled = |s: &Stream| match s.kind {
        StreamKind::Video => command.no_video,
        StreamKind::Audio => command.no_audio,
        StreamKind::Subtitle => command.no_subtitle,
        StreamKind::Data => command.no_data,
        StreamKind::Attachment => false,
    };
    let mut selected: Vec<usize> = Vec::new();
    if command.maps.is_empty() {
        let best = |kind: StreamKind, score: &dyn Fn(&Stream) -> i64| {
            all.iter()
                .filter(|(_, s)| s.kind == kind && !disabled(s))
                .fold(None::<(usize, (bool, i64))>, |best, (i, s)| {
                    let value = (is_default(s), score(s));
                    match best {
                        Some((_, top)) if top >= value => best,
                        _ => Some((*i, value)),
                    }
                })
                .map(|(i, _)| i)
        };
        selected.extend(best(StreamKind::Video, &|s| {
            int(s, "width").saturating_mul(int(s, "height"))
        }));
        selected.extend(best(StreamKind::Audio, &|s| int(s, "channels")));
        let subtitle = all
            .iter()
            .filter(|_| {
                command
                    .codecs
                    .iter()
                    .any(|(spec, _)| *spec == Spec::Kind(Kind2::Subtitle, None))
            })
            .find(|(_, s)| s.kind == StreamKind::Subtitle && !disabled(s));
        if let Some((i, s)) = subtitle {
            let codec = s.entries.iter().find_map(|(k, v)| match v {
                Value::Str(name) if *k == "codec_name" => Some(name.as_str()),
                _ => None,
            });
            if codec.is_none_or(|c| TEXT_SUBTITLES.contains(&c)) {
                selected.push(*i);
            }
        }
    } else {
        for map in &command.maps {
            let hits = matching(&map.spec, &all);
            if map.remove {
                selected.retain(|i| !hits.contains(i));
            } else if hits.is_empty() {
                return Err(Refusal::Usage(
                    "A -map matches no streams; to ignore it, add a trailing '?' to the map."
                        .to_owned(),
                ));
            } else {
                selected.extend(hits);
            }
        }
        selected.retain(|i| streams.get(*i).is_some_and(|s| !disabled(s)));
    }
    if selected.is_empty() {
        return Err(Refusal::Usage(
            "Output file does not contain any stream".to_owned(),
        ));
    }
    if selected.windows(2).any(|w| matches!(w, [a, b] if a >= b)) {
        return Err(Refusal::not_native(
            "a selection that maps a stream twice or out of the input's order",
        ));
    }
    let output: Vec<(usize, &Stream)> = selected
        .iter()
        .filter_map(|i| streams.get(*i).map(|s| (*i, s)))
        .collect();
    if selected
        .iter()
        .any(|i| codec(command, &output, *i).is_none())
    {
        return Err(Refusal::not_native(
            "a selected stream that no -c copy option covers (it would be encoded)",
        ));
    }
    if output.iter().any(|(_, s)| s.kind == StreamKind::Subtitle) {
        return Err(Refusal::not_native("copying a subtitle stream"));
    }
    Ok(selected)
}

/// The reason a writer gives for a source it does not implement, as
/// opposed to one it rejects.
fn not_native_reason(failure: &WriteFailure) -> Option<&'static str> {
    match failure {
        WriteFailure::Rejected(Failure::Library(coreav_isobmff::Error::Unsupported { reason }))
        | WriteFailure::Rejected(Failure::Matroska(coreav_mkv::Error::Unsupported { reason })) => {
            Some(reason)
        }
        WriteFailure::Rejected(Failure::Library(coreav_isobmff::Error::CodecNotAllowed {
            ..
        }))
        | WriteFailure::Rejected(Failure::Matroska(coreav_mkv::Error::CodecNotAllowed {
            ..
        })) => Some("a codec that CoreAV does not copy"),
        WriteFailure::Rejected(Failure::Unsupported(_)) => {
            Some("a stream that CoreAV cannot copy to MP4")
        }
        _ => None,
    }
}

/// Whether an existing output may be replaced, as `-y` and `-n` say.
fn may_replace(command: &Command, path: &Path) -> Result<bool, Refusal> {
    let exists = std::fs::symlink_metadata(path).is_ok();
    match command.overwrite {
        Overwrite::Yes => Ok(true),
        Overwrite::No if exists => Err(Refusal::Usage(format!(
            "File '{}' already exists. Exiting.",
            path.display()
        ))),
        Overwrite::Ask if exists => Err(Refusal::not_native(
            "asking whether to overwrite an existing output (pass -y or -n)",
        )),
        Overwrite::No | Overwrite::Ask => Ok(false),
    }
}

pub(crate) fn check_output(command: &Command, path: &Path) -> Result<bool, Refusal> {
    let force = may_replace(command, path)?;
    match output::refuse(Path::new(&command.input), path, force) {
        Some(message) => Err(Refusal::Usage(message)),
        None => Ok(force),
    }
}

/// Writes the output of `job`: to a temporary file next to each output,
/// moved into place only once the whole job succeeded.
fn write(command: &Command, job: &Job) -> Result<serde_json::Value, Stop> {
    let input = Path::new(&command.input);
    let (allowlist, mkv_allowlist) = crate::matroska::allowlists(None, false).map_err(|_| {
        Refusal::Usage("the default codec allowlists could not be built".to_owned())
    })?;
    if let Job::Split(options) = job {
        return split(command, options, &allowlist);
    }
    if let Job::Transcode {
        video,
        audio,
        reordered,
    } = job
    {
        #[cfg(feature = "transcode")]
        return encode::write(command, *video, audio, *reordered);
        #[cfg(not(feature = "transcode"))]
        {
            let _ = (video, audio, reordered);
            return Err(
                Refusal::not_native("encoding, in a build without a decoder backend").into(),
            );
        }
    }
    let path = PathBuf::from(&command.output);
    let force = check_output(command, &path)?;
    let document = output::to_file(&path, force, |out| match job {
        Job::Remux {
            streams,
            matroska: false,
        } => {
            let options = RemuxOptions {
                streams: Some(streams.clone()),
                on_unsupported: OnUnsupported::Fail,
                ..RemuxOptions::default()
            };
            crate::remux::run(input, &options, &allowlist, out)
                .map(|summary| crate::remux::document(&summary, &options))
        }
        Job::Remux {
            streams,
            matroska: true,
        } => {
            let options = MatroskaRemuxOptions {
                streams: Some(streams.clone()),
                on_unsupported: OnUnsupported::Fail,
                drop_video: false,
            };
            crate::remux::run_matroska(input, &options, &mkv_allowlist, out)
                .map(|summary| crate::remux::matroska_document(&summary))
        }
        Job::ExtractAudio { stream } => {
            crate::extract::run(input, TrackChoice::Stream(*stream), &allowlist, out)
                .map(|summary| crate::extract::document(&summary))
        }
        Job::Clip(options) => crate::clip::run(input, options, &allowlist, out)
            .map(|summary| crate::clip::document(&summary)),
        Job::Split(_) | Job::Transcode { .. } => Ok(serde_json::Value::Null),
    })?;
    Ok(document)
}

/// Writes the segments of a split, each to a temporary file, and moves them
/// to their names once all are written (as `coreav split` does).
fn split(
    command: &Command,
    options: &SplitOptions,
    allowlist: &coreav_isobmff::Allowlist,
) -> Result<serde_json::Value, Stop> {
    let Some(pattern) = Pattern::parse(&command.output) else {
        return Err(Refusal::not_native("a segment name without one %d or %0Nd").into());
    };
    let mut written: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let mut refused = None;
    let open = |index: usize| {
        let name = PathBuf::from(pattern.name(index));
        let force = match check_output(command, &name) {
            Ok(force) => force,
            Err(refusal) => {
                refused = Some(refusal);
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
            }
        };
        let temporary = output::tagged_temporary_path(&name, &format!("-{index}"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        written.push((temporary, name, force));
        Ok(std::io::BufWriter::new(file))
    };
    let summary = match crate::split::run(Path::new(&command.input), options, allowlist, open) {
        Ok(summary) => summary,
        Err(failure) => {
            for (temporary, _, _) in &written {
                let _ = std::fs::remove_file(temporary);
            }
            return Err(match refused {
                Some(refusal) => refusal.into(),
                None => failure.into(),
            });
        }
    };
    for (n, (temporary, name, force)) in written.iter().enumerate() {
        if let Err(error) = output::place(temporary, name, *force) {
            for (rest, _, _) in written.iter().skip(n) {
                let _ = std::fs::remove_file(rest);
            }
            return Err(Stop::Failed(WriteFailure::output(&error)));
        }
    }
    let names: Vec<String> = written
        .iter()
        .map(|(_, name, _)| name.display().to_string())
        .collect();
    let mut document = Vec::new();
    crate::split::write_document(&mut document, &summary, &names)
        .map_err(|error| Stop::Failed(WriteFailure::output(&error)))?;
    serde_json::from_slice(&document)
        .map_err(|error| Stop::Failed(WriteFailure::output(&std::io::Error::other(error))))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// The range of `command` on an input whose start time is
    /// `origin_tenths` tenths of a second, in tenths.
    fn range(command: &str, origin_tenths: i128) -> (i128, Option<i128>) {
        let args: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
        let command = args::parse(&args).unwrap();
        let origin = Rational::new(origin_tenths, 10).unwrap();
        let tenths = |r: Rational| {
            let scaled = r.num.checked_mul(10).unwrap();
            assert_eq!(scaled % i128::from(r.den), 0, "{r:?}");
            scaled / i128::from(r.den)
        };
        let (start, end) = clip_range(&command, origin).unwrap();
        (tenths(start), end.map(tenths))
    }

    #[test]
    fn clip_times_count_from_the_input_start() {
        // Each checked against the pinned ffmpeg on an all-intra video at
        // one frame a second whose first frame is presented at 5 s: the
        // copied frames are those of the range.
        let at_5 = |command| range(command, 50);
        assert_eq!(
            at_5("-ss 6 -i in.mp4 -t 1 -c copy out.mp4"),
            (110, Some(120))
        );
        assert_eq!(
            at_5("-i in.mp4 -ss 6 -t 1 -c copy out.mp4"),
            (110, Some(120))
        );
        assert_eq!(
            at_5("-ss 2 -i in.mp4 -ss 3 -t 1 -c copy out.mp4"),
            (100, Some(110))
        );
        assert_eq!(
            at_5("-ss 6 -i in.mp4 -to 2 -c copy out.mp4"),
            (110, Some(130))
        );
        assert_eq!(at_5("-i in.mp4 -to 2 -c copy out.mp4"), (50, Some(70)));
        assert_eq!(
            at_5("-ss 1 -i in.mp4 -to 3 -c copy out.mp4"),
            (60, Some(90))
        );
        assert_eq!(at_5("-to 3 -i in.mp4 -c copy out.mp4"), (50, Some(80)));
        assert_eq!(at_5("-t 4 -i in.mp4 -to 2 -c copy out.mp4"), (50, Some(70)));
        assert_eq!(
            range("-ss 6 -i in.mp4 -t 1 -c copy out.mp4", 0),
            (60, Some(70))
        );
        // A negative start time: the clip starts at the timeline's 0.
        assert_eq!(range("-i in.mp4 -t 1 -c copy out.mp4", -1), (0, Some(9)));
    }
}
