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

use crate::error::Failure;
use crate::ffprobe::args::Kind2;
use crate::ffprobe::model::Value;
use crate::ffprobe::{Report, Stream, StreamKind};
use crate::output::{self, WriteFailure};
use crate::split::Pattern;
use args::{Command, Demuxer, Muxer, Overwrite, Spec};
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
    /// `-h`, `-help`, `--help`: the usage text, exit 0.
    Help,
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

/// The usage text of `coreav ffmpeg -h`.
pub const USAGE: &str = "usage: coreav ffmpeg [OPTIONS] -i INPUT [OPTIONS] OUTPUT

Runs an ffmpeg stream-copy command line (-c copy) with CoreAV's own writers:
remuxing MP4/MOV, Matroska and WebM to MP4, clipping (-ss, -t, -to),
segmenting (-f segment) and audio extraction to M4A, with ffmpeg's stream
selection (-map, -vn, -an, -sn, -dn). A command outside that vocabulary is
refused by name with exit status 4. See docs/design/compat-cli.md.
";

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
}

/// The container families CoreAV reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Container {
    Isobmff,
    Matroska,
}

/// Parses, plans and runs a command line.
pub fn run(args: &[String]) -> Result<(), Stop> {
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
        let (start, end) =
            clip_range(command).ok_or(Refusal::not_native("a time CoreAV cannot represent"))?;
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

/// Where a clip starts and ends on the input's presentation timeline, in
/// `ffmpeg`'s meaning of the options: an input `-ss` shifts the output's
/// time 0 to it, so an output `-to` counts from there; an input `-t` or
/// `-to` stops reading the input; an output `-t` counts from the output
/// `-ss`; `-t` wins over `-to` given with it.
fn clip_range(command: &Command) -> Option<(Rational, Option<Rational>)> {
    let zero = Rational::from_int(0);
    let shift = command.input_seek.unwrap_or(zero);
    let start = shift.checked_add(command.seek.unwrap_or(zero))?;
    let input_end = match (command.input_duration, command.input_until) {
        (Some(duration), _) => Some(shift.checked_add(duration)?),
        (None, until) => until,
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
    Some((start, end))
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
                    kind_seen += 1;
                    n.is_none_or(|n| n + 1 == kind_seen)
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

/// Subtitle codecs that are text, which `ffmpeg` selects automatically for
/// an MP4 output (whose default subtitle encoder, `mov_text`, is one).
const TEXT_SUBTITLES: [&str; 7] = ["mov_text", "subrip", "srt", "ass", "ssa", "webvtt", "text"];

/// The streams `ffmpeg` would write, in order: `-map`, or the automatic
/// selection, less the disabled types. Each must be covered by a `copy`
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
                .fold(None::<(usize, i64)>, |best, (i, s)| {
                    let value = score(s);
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
    let copied: Vec<usize> = command
        .copied
        .iter()
        .flat_map(|spec| matching(spec, &output))
        .collect();
    if selected.iter().any(|i| !copied.contains(i)) {
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

fn check_output(command: &Command, path: &Path) -> Result<bool, Refusal> {
    let force = may_replace(command, path)?;
    match output::refuse(Path::new(&command.input), path, force) {
        Some(message) => Err(Refusal::Usage(message)),
        None => Ok(force),
    }
}

/// Writes the output of `job`: to a temporary file next to each output,
/// moved into place only once the whole job succeeded.
fn write(command: &Command, job: &Job) -> Result<(), Stop> {
    let input = Path::new(&command.input);
    let (allowlist, mkv_allowlist) = crate::matroska::allowlists(None, false).map_err(|_| {
        Refusal::Usage("the default codec allowlists could not be built".to_owned())
    })?;
    if let Job::Split(options) = job {
        return split(command, options, &allowlist);
    }
    let path = PathBuf::from(&command.output);
    let force = check_output(command, &path)?;
    output::to_file(&path, force, |out| match job {
        Job::Remux {
            streams,
            matroska: false,
        } => {
            let options = RemuxOptions {
                streams: Some(streams.clone()),
                on_unsupported: OnUnsupported::Fail,
                ..RemuxOptions::default()
            };
            crate::remux::run(input, &options, &allowlist, out).map(drop)
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
            crate::remux::run_matroska(input, &options, &mkv_allowlist, out).map(drop)
        }
        Job::ExtractAudio { stream } => {
            crate::extract::run(input, TrackChoice::Stream(*stream), &allowlist, out).map(drop)
        }
        Job::Clip(options) => crate::clip::run(input, options, &allowlist, out).map(drop),
        Job::Split(_) => Ok(()),
    })?;
    Ok(())
}

/// Writes the segments of a split, each to a temporary file, and moves them
/// to their names once all are written (as `coreav split` does).
fn split(
    command: &Command,
    options: &SplitOptions,
    allowlist: &coreav_isobmff::Allowlist,
) -> Result<(), Stop> {
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
    let result = crate::split::run(Path::new(&command.input), options, allowlist, open);
    if let Err(failure) = result {
        for (temporary, _, _) in &written {
            let _ = std::fs::remove_file(temporary);
        }
        return Err(match refused {
            Some(refusal) => refusal.into(),
            None => failure.into(),
        });
    }
    for (n, (temporary, name, force)) in written.iter().enumerate() {
        if let Err(error) = output::place(temporary, name, *force) {
            for (rest, _, _) in written.iter().skip(n) {
                let _ = std::fs::remove_file(rest);
            }
            return Err(Stop::Failed(WriteFailure::output(&error)));
        }
    }
    Ok(())
}
