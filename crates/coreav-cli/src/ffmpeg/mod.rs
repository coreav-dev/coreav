//! `coreav ffmpeg`: `ffmpeg`'s stream-copy command lines over CoreAV's
//! writers (`docs/design/compat-cli.md`, "The command").
//!
//! [`args::parse`] reads the command line and refuses what is outside the
//! vocabulary. [`plan`] reads the input's streams as `coreav ffprobe` lists
//! them, selects the streams as `ffmpeg` does (`-map`, or one video, one
//! audio and one subtitle stream chosen automatically, less `-vn`, `-an`,
//! `-sn`, `-dn`), and picks the writer: a remux, a clip (of audio alone
//! when no video is selected), a split or an audio extraction. [`run`]
//! writes the output. Whatever a writer does not implement is a
//! [`NotNative`] refusal, found before any output is written.

pub mod args;
#[cfg(feature = "transcode")]
pub mod encode;

use crate::error::Failure;
use crate::fallback::NotNative;
use crate::ffprobe::args::Kind2;
use crate::ffprobe::model::Value;
use crate::ffprobe::{Report, Stream, StreamKind};
use crate::output::{self, WriteFailure};
use crate::split::Pattern;
use crate::strict_safe::StrictSafe;
use args::{AudioEncoder, Codec, Command, Demuxer, Muxer, Overwrite, Spec};
use coreav_isobmff::{Allowlist, FileSource, Limits, Rational, Track};
use coreav_isobmff_write::{
    AudioChoice, AudioClipOptions, ClipOptions, JoinAudio, JoinOptions, JoinSummary,
    MatroskaRemuxOptions, OnUnsupported, RemuxOptions, SplitAt, SplitOptions, TrackChoice,
    VideoChoice,
};
use std::path::{Path, PathBuf};

/// A command that is not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not native: exit 4.
    NotNative(NotNative),
    /// Malformed, as `ffmpeg` would also fail it: exit 2.
    Usage(String),
    /// `-h`, `-?`, `-help` or `--help` with nothing after it: the usage
    /// text, exit 0. With a topic after it (`-h long`), the query is
    /// FFmpeg's, so it is [`Refusal::NotNative`].
    Help,
}

impl Refusal {
    /// A refusal for a property of the command or of its input.
    pub fn not_native(property: &'static str) -> Self {
        Self::NotNative(NotNative::property(
            property,
            format!("not supported by coreav ffmpeg: {property}"),
        ))
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
        /// The tracks, in input order.
        streams: Vec<usize>,
        /// The cover pictures that are not selected, as
        /// `RemuxOptions::left_out_covers` names them.
        left_out_covers: Vec<u64>,
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
    /// A clip of one AAC stream alone, presenting the range exactly.
    AudioClip(AudioClipOptions),
    /// Segments, written to the names of the output pattern.
    Split(SplitOptions),
    /// The video stream encoded with libx264 and these audio streams copied,
    /// or the one audio stream encoded with libopus or aac, as MP4
    /// (`docs/design/transcode.md`, "Through `coreav ffmpeg`").
    Transcode(TranscodeJob),
    /// The files of a concat list, joined in order.
    Join {
        /// The files.
        paths: Vec<PathBuf>,
        /// The video stream.
        video: usize,
        /// The audio kept.
        audio: JoinAudio,
    },
}

/// What [`Job::Transcode`] encodes and copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscodeJob {
    /// The video stream.
    pub video: usize,
    /// The audio streams, in input order.
    pub audio: Vec<usize>,
    /// The encoder of the audio stream, then the only one; `None` when the
    /// audio is copied.
    pub audio_encoder: Option<AudioEncoder>,
    /// Whether the decoder reorders the video's frames (B-frames).
    pub reordered: bool,
    /// The cover pictures that are not selected, as
    /// `RemuxOptions::left_out_covers` names them; a selected one is copied.
    pub left_out_covers: Vec<u64>,
}

/// The container families CoreAV reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Container {
    Isobmff,
    Matroska,
}

/// The usage text of `coreav ffmpeg -h`.
pub const USAGE: &str = "\
usage: coreav ffmpeg [OPTIONS] -i INPUT [OPTIONS] OUTPUT

Experimental until it meets its 0.1.0 bars, R1 to R4, on held-out files.

The common ffmpeg stream-copy command lines (-c copy), over CoreAV's own
writers, with ffmpeg's stream selection (-map, -vn, -an, -sn, -dn):
  a copy to .mp4 (or -f mp4) from MP4, MOV, Matroska or WebM: a remux
  a copy of one audio stream to .m4a (or -f ipod): an audio extraction
  -ss, -t or -to, before or after -i: a clip
  -f segment with MP4 segments: a split every -segment_time
  -f concat with an MP4 output: a join of the list's files
Builds with a decoder backend also encode the video with -c:v libx264.
Prints nothing on success.

For untrusted input, set COREAV_STRICT_SAFE=1: anything else is then
refused (exit status 4). Without it, anything else runs the ffmpeg
program on PATH instead (or the one COREAV_FALLBACK names), which is
FFmpeg's C code, and says so on standard error. Help on a topic
(-h long, -h full, -h encoder=NAME, ...) is handled the same way.
Documentation: https://coreav.dev/docs/commands/ffmpeg
See docs/design/compat-cli.md.
";

/// Parses, plans and runs a command line. Returns the summary its writer's
/// own command prints (`coreav remux`, `clip`, `split`, `extract-audio`,
/// `join`), which `coreav ffmpeg` does not print; the fixture tests check
/// outputs with it. `strict_safe` is `coreav --strict-safe`: an encode
/// then runs no `ffmpeg` program (`crate::strict_safe`), and its encoder
/// is decided from the command line, before the input is read.
#[cfg_attr(not(feature = "transcode"), allow(unused_variables))]
pub fn run(args: &[String], strict_safe: StrictSafe) -> Result<serde_json::Value, Stop> {
    let command = args::parse(args)?;
    #[cfg(feature = "transcode")]
    let encoder = encode::encoder(&command, strict_safe)?;
    let io = |e| WriteFailure::Rejected(Failure::io(e));
    let input = PathBuf::from(&command.input);
    std::fs::metadata(&input).map_err(io)?;
    let (list, force) = match command.demuxer {
        Some(Demuxer::Concat { safe }) => (Some(list(&input, safe)?), None),
        Some(Demuxer::Isobmff) => (None, Some(crate::ffprobe::args::Container::Isobmff)),
        Some(Demuxer::Matroska) => (None, Some(crate::ffprobe::args::Container::Matroska)),
        None => (None, None),
    };
    let probed = list.as_ref().and_then(|l| l.first()).unwrap_or(&input);
    let size = std::fs::metadata(probed).map_err(io)?.len();
    let mut file = FileSource::open(probed).map_err(WriteFailure::from)?;
    let name = probed.display().to_string();
    let report = crate::ffprobe::probe(&mut file, size, &name, force)
        .map_err(WriteFailure::Rejected)?
        .report;
    let job = plan(&command, &report, list)?;
    #[cfg(feature = "transcode")]
    if let (Job::Transcode(transcode), Some(encoder)) = (&job, encoder) {
        return encode::write(&command, transcode, encoder, strict_safe);
    }
    write(&command, &job)
}

/// The files a concat list names (`crate::concat::read`). A directive or a
/// path `ffmpeg` reads differently is not native; a list `ffmpeg` fails
/// too is rejected.
fn list(path: &Path, safe: bool) -> Result<Vec<PathBuf>, Stop> {
    use crate::concat::ListError;
    crate::concat::read(path, safe).map_err(|error| match error {
        ListError::Directive { known: true, .. }
        | ListError::Version { .. }
        | ListError::Protocol { .. }
        | ListError::TooLarge => Stop::Refused(Refusal::NotNative(NotNative::property(
            "a concat list other than local files",
            format!("not supported by coreav ffmpeg: {error}"),
        ))),
        ListError::Io(kind) => Stop::Failed(WriteFailure::Rejected(Failure::Io(kind))),
        ListError::Directive { known: false, .. }
        | ListError::Read(_)
        | ListError::NoPath { .. }
        | ListError::Unsafe { .. }
        | ListError::Empty => Stop::Failed(WriteFailure::Object {
            json: serde_json::json!({
                "schema": "coreav.error/1",
                "kind": "invalid_structure",
                "limit": null,
                "offset": null,
                "message": error.to_string(),
            }),
            io: false,
        }),
    })
}

/// The writer and options for `command` on the input `report` describes,
/// or on the first file of the concat list `list` when the input is one:
/// the streams of that file, as the concat demuxer exposes them, with
/// every disposition flag cleared (black box: `ffprobe` of the list).
pub fn plan(
    command: &Command,
    report: &Report,
    list: Option<Vec<PathBuf>>,
) -> Result<Job, Refusal> {
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
    let exposed: Vec<Stream>;
    let streams = if list.is_some() {
        exposed = report
            .streams
            .iter()
            .cloned()
            .map(|mut stream| {
                stream.attached_pic = false;
                for (_, value) in &mut stream.disposition {
                    *value = Value::Int(0);
                }
                stream
            })
            .collect();
        &exposed
    } else {
        &report.streams
    };
    let selected = select(command, streams)?;
    let cover = |i: usize| report.covers.iter().any(|(c, _)| *c == i);
    let kind = |i: usize| streams.get(i).map(|s| s.kind);
    let of = |k: StreamKind| {
        selected
            .iter()
            .copied()
            .filter(move |i| kind(*i) == Some(k))
    };
    let one_video_one_audio = || {
        let videos: Vec<usize> = of(StreamKind::Video).filter(|i| !cover(*i)).collect();
        let audios: Vec<usize> = of(StreamKind::Audio).collect();
        match (videos.as_slice(), audios.as_slice()) {
            _ if videos.len() + audios.len() != selected.len() => Err(Refusal::not_native(
                "a clip or a segment of other streams than one video and one audio stream",
            )),
            ([video], [audio]) => Ok((*video, AudioChoice::Stream(*audio))),
            ([video], []) => Ok((*video, AudioChoice::None)),
            ([], _) => Err(Refusal::not_native("a segment without video")),
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
    let audio_encoded: Vec<(usize, AudioEncoder)> = selected
        .iter()
        .filter_map(|i| match codec(command, &output, *i) {
            Some(Codec::Audio(encoder)) => Some((*i, encoder)),
            _ => None,
        })
        .collect();
    if !encoded.is_empty() {
        if list.is_some() {
            return Err(Refusal::not_native("encoding the files of a concat list"));
        }
        return transcode_job(command, report, &output, &encoded, &audio_encoded, trims);
    }
    if let Some((_, encoder)) = audio_encoded.first() {
        return Err(Refusal::not_native(match encoder {
            AudioEncoder::Libopus => {
                "encoding audio with libopus without encoding the video with libx264"
            }
            AudioEncoder::Aac => "encoding audio with aac without encoding the video with libx264",
        }));
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
    if let Some(paths) = list {
        if container != Container::Isobmff {
            return Err(Refusal::not_native("joining Matroska or WebM files"));
        }
        if trims || command.muxer != Muxer::Mp4 {
            return Err(Refusal::not_native(
                "a join that also seeks, trims, segments or writes M4A",
            ));
        }
        if !selected.iter().any(|i| kind(*i) == Some(StreamKind::Video)) {
            return Err(Refusal::not_native("a join without video"));
        }
        let (video, audio) = match selected.as_slice() {
            [v] if kind(*v) == Some(StreamKind::Video) => (*v, JoinAudio::None),
            [v, a]
                if kind(*v) == Some(StreamKind::Video) && kind(*a) == Some(StreamKind::Audio) =>
            {
                (*v, JoinAudio::Stream(*a))
            }
            _ => {
                return Err(Refusal::not_native(
                    "a join of other streams than one video and at most one audio stream",
                ));
            }
        };
        return Ok(Job::Join {
            paths,
            video,
            audio,
        });
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
        let (start, end) = clip_range(command, input_start(report))
            .ok_or(Refusal::not_native("a time CoreAV cannot represent"))?;
        if of(StreamKind::Video).next().is_none() {
            return match selected.as_slice() {
                [stream] if kind(*stream) == Some(StreamKind::Audio) => {
                    Ok(Job::AudioClip(AudioClipOptions {
                        audio: TrackChoice::Stream(*stream),
                        start,
                        end,
                    }))
                }
                _ => Err(Refusal::not_native(
                    "a clip without video of other than one audio stream",
                )),
            };
        }
        let (video, audio) = one_video_one_audio()?;
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
    let tracks: Vec<usize> = selected.iter().copied().filter(|i| !cover(*i)).collect();
    if tracks.is_empty() {
        return Err(Refusal::not_native(
            "cover pictures alone (ffmpeg writes an MP4 file without a track, which ISO/IEC 14496-12 8.3.1 does not allow)",
        ));
    }
    Ok(Job::Remux {
        left_out_covers: left_out_covers(report, &selected),
        streams: tracks,
        matroska: container == Container::Matroska,
    })
}

/// The cover pictures of `report` that `selected` leaves out, as
/// `RemuxOptions::left_out_covers` names them.
fn left_out_covers(report: &Report, selected: &[usize]) -> Vec<u64> {
    report
        .covers
        .iter()
        .filter(|(i, _)| !selected.contains(i))
        .map(|(_, at)| *at)
        .collect()
}

/// The job of a command that encodes: one video stream with libx264, the
/// other selected streams audio, copied, or one audio stream encoded with
/// libopus or aac (`audio`, the selected streams it names and their
/// encoders), into one MP4 file.
fn transcode_job(
    command: &Command,
    report: &Report,
    output: &[(usize, &Stream)],
    encoded: &[usize],
    audio: &[(usize, AudioEncoder)],
    trims: bool,
) -> Result<Job, Refusal> {
    let cover = |i: usize| report.covers.iter().any(|(c, _)| *c == i);
    if encoded
        .iter()
        .chain(audio.iter().map(|(i, _)| i))
        .any(|i| cover(*i))
    {
        return Err(Refusal::not_native("encoding a cover picture"));
    }
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
    let others: Vec<(usize, &Stream)> = output
        .iter()
        .filter(|(i, _)| i != video && !cover(*i))
        .copied()
        .collect();
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
    let audio_encoder = match (audio, others.as_slice()) {
        ([], _) => None,
        ([(index, encoder)], [(other, stream)]) if index == other => {
            audio_source(*encoder, stream)?;
            Some(*encoder)
        }
        ([(_, encoder)], _) => {
            return Err(Refusal::not_native(match encoder {
                AudioEncoder::Libopus => "copying audio beside the audio stream libopus encodes",
                AudioEncoder::Aac => "copying audio beside the audio stream aac encodes",
            }));
        }
        ([(_, encoder), ..], _) => {
            return Err(Refusal::not_native(match encoder {
                AudioEncoder::Libopus => "encoding more than one audio stream with libopus",
                AudioEncoder::Aac => "encoding more than one audio stream with aac",
            }));
        }
    };
    let selected: Vec<usize> = output.iter().map(|(i, _)| *i).collect();
    Ok(Job::Transcode(TranscodeJob {
        video: *video,
        audio: others.iter().map(|(i, _)| *i).collect(),
        audio_encoder,
        reordered: output
            .iter()
            .any(|(i, s)| i == video && int(s, "has_b_frames") > 0),
        left_out_covers: left_out_covers(report, &selected),
    }))
}

/// The refusals of an audio stream to encode with `encoder` where the
/// pipeline would not encode it as `ffmpeg` does: audio CoreAV does not
/// decode (it decodes FLAC and MP3), more than two channels (`ffmpeg`
/// writes a surround stream), a build without libopus, or for aac a sample
/// rate outside 8 to 64 kHz (the pipeline encodes AAC at the source's rate
/// and does not resample; `ffmpeg` encodes 88.2 and 96 kHz as they are).
fn audio_source(encoder: AudioEncoder, stream: &Stream) -> Result<(), Refusal> {
    let opus = encoder == AudioEncoder::Libopus;
    if opus && !cfg!(feature = "libopus") {
        return Err(Refusal::not_native(
            "encoding with libopus, in a build without the libopus feature",
        ));
    }
    let codec = stream.entries.iter().find_map(|(k, v)| match v {
        Value::Str(name) if *k == "codec_name" => Some(name.as_str()),
        _ => None,
    });
    if !matches!(codec, Some("flac" | "mp3")) {
        return Err(Refusal::not_native(if opus {
            "encoding audio other than FLAC or MP3 with libopus (CoreAV decodes only those)"
        } else {
            "encoding audio other than FLAC or MP3 with aac (CoreAV decodes only those)"
        }));
    }
    if !(1..=2).contains(&int(stream, "channels")) {
        return Err(Refusal::not_native(if opus {
            "encoding other than mono or stereo with libopus (ffmpeg writes a surround Opus stream)"
        } else {
            "encoding other than mono or stereo with aac (ffmpeg writes a surround AAC stream)"
        }));
    }
    let rate = stream.entries.iter().find_map(|(k, v)| match v {
        Value::Unit(hz, _) if *k == "sample_rate" => Some(*hz),
        _ => None,
    });
    if !opus && rate.is_none_or(|hz| !(8_000..=64_000).contains(&hz)) {
        return Err(Refusal::not_native(
            "encoding audio sampled outside 8 to 64 kHz with aac (CoreAV does not resample it)",
        ));
    }
    Ok(())
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
    micros(report, "start_time")
        .and_then(|us| Rational::new(us, 1_000_000))
        .unwrap_or(Rational::from_int(0))
}

/// A time of the format as `ffprobe` prints it, in whole microseconds.
fn micros(report: &Report, key: &str) -> Option<i128> {
    report.format.iter().find_map(|(name, value)| match value {
        Value::Time(seconds) if *name == key => Some(i128::from((seconds * 1e6).round() as i64)),
        _ => None,
    })
}

/// Where the concat demuxer of `ffmpeg` places each listed file, in whole
/// microseconds from where it places the first: at the end of the files
/// before it, each as long as its `duration`, less its own `start_time`
/// (black box: a file whose audio outlasts its video is followed at the
/// audio's end).
fn concat_offsets(paths: &[PathBuf]) -> Result<Vec<i128>, Stop> {
    let mut offsets = Vec::new();
    let mut end: i128 = 0;
    let mut first = None;
    for path in paths {
        let size = std::fs::metadata(path)
            .map_err(|e| WriteFailure::Rejected(Failure::io(e)))?
            .len();
        let mut file = FileSource::open(path).map_err(WriteFailure::from)?;
        let report = crate::ffprobe::probe(&mut file, size, &path.display().to_string(), None)
            .map_err(WriteFailure::Rejected)?
            .report;
        let (Some(start), Some(duration)) =
            (micros(&report, "start_time"), micros(&report, "duration"))
        else {
            return Err(Refusal::not_native("a concat list of a file of unknown duration").into());
        };
        let first = *first.get_or_insert(start);
        offsets.push(end.saturating_sub(start).saturating_add(first));
        end = end.saturating_add(duration);
    }
    Ok(offsets)
}

/// Whether the join placed every input where the concat demuxer does
/// ([`concat_offsets`]), to the microsecond.
fn placed_as_concat(summary: &JoinSummary, offsets: &[i128]) -> bool {
    let Some(first) = summary.inputs.first() else {
        return false;
    };
    let from_first = |offset: Rational| {
        Rational::new(first.offset.num.checked_neg()?, first.offset.den)
            .and_then(|minus| offset.checked_add(minus))
            .and_then(|r| Rational::new(r.num.checked_mul(1_000_000)?, r.den))
            .map(Rational::round)
    };
    summary.inputs.len() == offsets.len()
        && summary
            .inputs
            .iter()
            .zip(offsets)
            .all(|(input, expected)| from_first(input.offset) == Some(*expected))
}

/// Why a join is not native: a track it copies has more than one edit in
/// its file (an empty one included), which the demuxer reads by its own
/// edit model (docs/design/edit-list-writers.md) where the join keeps the
/// edits, or it places an input elsewhere than the concat demuxer
/// ([`placed_as_concat`]). The edits are counted in each file, as edits
/// that meet merge in the output.
fn concat_timing(
    paths: &[PathBuf],
    summary: &JoinSummary,
    offsets: &[i128],
) -> Result<Option<&'static str>, WriteFailure> {
    for (path, input) in paths.iter().zip(&summary.inputs) {
        let file = FileSource::open(path)?;
        let movie = coreav_isobmff::probe(file, &Limits::default(), &Allowlist::listing())
            .map_err(|e| WriteFailure::Rejected(Failure::library(e)))?;
        let copied = [Some(&input.video), input.audio.as_ref()];
        let edited = |track: &Track| {
            track.edits.len() > 1
                && copied
                    .iter()
                    .flatten()
                    .any(|c| c.track_id == track.track_id)
        };
        if movie.tracks.iter().any(edited) {
            return Ok(Some("a join of files whose tracks have several edits"));
        }
    }
    Ok((!placed_as_concat(summary, offsets))
        .then_some("a join of files whose streams end at different times"))
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
                .fold(None::<(usize, (bool, bool, i64))>, |best, (i, s)| {
                    let value = (!s.attached_pic, is_default(s), score(s));
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
/// moved into place only once the whole job succeeded. A transcode is
/// [`encode::write`]'s, in a build that has it.
fn write(command: &Command, job: &Job) -> Result<serde_json::Value, Stop> {
    let input = Path::new(&command.input);
    let (allowlist, mkv_allowlist) = crate::matroska::allowlists(None, false).map_err(|_| {
        Refusal::Usage("the default codec allowlists could not be built".to_owned())
    })?;
    if let Job::Split(options) = job {
        return split(command, options, &allowlist);
    }
    if let Job::Transcode(_) = job {
        return Err(Refusal::not_native("encoding, in a build without a decoder backend").into());
    }
    let path = PathBuf::from(&command.output);
    let force = check_output(command, &path)?;
    if let Job::Join { paths, .. } = job
        && paths.iter().any(|p| output::same_file(p, &path))
    {
        return Err(Refusal::Usage(format!(
            "{} is an input file; write the output to another path",
            path.display()
        ))
        .into());
    }
    let offsets = match job {
        Job::Join { paths, .. } => concat_offsets(paths)?,
        _ => Vec::new(),
    };
    let mut refused = None;
    let written = output::to_file(&path, force, |out| match job {
        Job::Remux {
            streams,
            left_out_covers,
            matroska: false,
        } => {
            let options = RemuxOptions {
                streams: Some(streams.clone()),
                on_unsupported: OnUnsupported::Fail,
                left_out_covers: left_out_covers.clone(),
                ..RemuxOptions::default()
            };
            crate::remux::run(input, &options, &allowlist, out)
                .map(|summary| crate::remux::document(&summary, &options))
        }
        Job::Remux {
            streams,
            matroska: true,
            ..
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
        Job::AudioClip(options) => crate::clip::run_audio(input, options, &allowlist, out)
            .map(|summary| crate::clip::audio_document(&summary)),
        Job::Join {
            paths,
            video,
            audio,
        } => {
            let options = JoinOptions {
                video: VideoChoice::Stream(*video),
                audio: *audio,
            };
            let summary =
                crate::join::run(paths, &options, &allowlist, out).map_err(|failure| {
                    refused = failure.not_native;
                    WriteFailure::from(failure)
                })?;
            if let Some(words) = concat_timing(paths, &summary, &offsets)? {
                refused = Some(words);
                return Err(WriteFailure::Rejected(Failure::Unsupported(
                    words.to_owned(),
                )));
            }
            let mut document = Vec::new();
            crate::join::write_document(&mut document, &summary, paths)
                .and_then(|()| Ok(serde_json::from_slice(&document)?))
                .map_err(|e| WriteFailure::Rejected(Failure::io(e)))
        }
        Job::Split(_) | Job::Transcode(_) => Ok(serde_json::Value::Null),
    });
    match (written, refused) {
        (Err(_), Some(words)) => Err(Refusal::not_native(words).into()),
        (written, _) => Ok(written?),
    }
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

    /// The audio streams `-c:a aac` encodes: FLAC or MP3, mono or stereo,
    /// sampled at 8 to 64 kHz.
    #[test]
    fn aac_takes_flac_or_mp3_mono_or_stereo_at_8_to_64_khz() {
        use crate::ffprobe::model::Unit;
        let stream = |codec: &str, channels: i64, rate: i64| Stream {
            kind: StreamKind::Audio,
            attached_pic: false,
            entries: vec![
                ("codec_name", Value::Str(codec.to_owned())),
                ("sample_rate", Value::Unit(rate, Unit::Hertz)),
                ("channels", Value::Int(channels)),
            ],
            disposition: Vec::new(),
            tags: Vec::new(),
            side_data: Vec::new(),
        };
        let refusal = |codec, channels, rate| match audio_source(
            AudioEncoder::Aac,
            &stream(codec, channels, rate),
        ) {
            Ok(()) => None,
            Err(Refusal::NotNative(n)) => Some(n.message),
            Err(other) => panic!("{other:?}"),
        };
        for (codec, channels, rate) in [
            ("flac", 2, 48_000),
            ("flac", 1, 8_000),
            ("mp3", 2, 44_100),
            ("flac", 2, 64_000),
        ] {
            assert_eq!(refusal(codec, channels, rate), None, "{codec} {rate}");
        }
        for (codec, channels, rate, says) in [
            ("aac", 2, 48_000, "other than FLAC or MP3 with aac"),
            ("opus", 2, 48_000, "other than FLAC or MP3 with aac"),
            ("flac", 6, 48_000, "other than mono or stereo with aac"),
            ("flac", 2, 96_000, "outside 8 to 64 kHz with aac"),
            ("flac", 2, 88_200, "outside 8 to 64 kHz with aac"),
            ("mp3", 1, 7_350, "outside 8 to 64 kHz with aac"),
        ] {
            let message = refusal(codec, channels, rate).unwrap_or_default();
            assert!(
                message.contains(says),
                "{codec} {channels} {rate}: {message}"
            );
        }
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
