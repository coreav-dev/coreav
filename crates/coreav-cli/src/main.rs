//! The `coreav` command.
//!
//! `coreav probe FILE` detects the container from the first bytes and prints
//! CoreAV's probe output schema: `coreav.probe/1` for MP4, MOV and other ISO
//! base media files (`docs/schema/probe-v1.md`), `coreav.probe-matroska/1`
//! for Matroska and WebM (`docs/schema/probe-matroska-v1.md`) and
//! `coreav.probe-mpegts/1` for MPEG transport streams
//! (`docs/schema/probe-mpegts-v1.md`).
//! `--ffprobe-compat` prints the subset of `ffprobe -of json -show_format
//! -show_streams` that is checked against `ffprobe`. `coreav clip --start
//! TIME INPUT OUTPUT` cuts a keyframe-aligned clip of an MP4/MOV file by
//! stream copy and prints its summary (`coreav.clip/1`,
//! `docs/schema/clip-v1.md`). `coreav split --every TIME INPUT
//! PATTERN` splits an MP4/MOV file at keyframes into segments by stream copy
//! and prints their summary (`coreav.split/1`, `docs/schema/split-v1.md`;
//! `docs/design/split.md`). `coreav extract-audio INPUT
//! OUTPUT` copies an AAC track of an MP4/MOV file into an M4A file and
//! prints its summary (`coreav.extract-audio/1`,
//! `docs/schema/extract-audio-v1.md`). `coreav remux INPUT
//! OUTPUT` rewrites a whole MP4/MOV file as MP4 with the movie box first, by
//! stream copy, and prints its summary (`coreav.remux/1`,
//! `docs/schema/remux-v1.md`); `coreav sanitize INPUT OUTPUT` does the same
//! keeping only allowlisted boxes (`docs/design/sanitize.md`). `coreav
//! extract-subtitles INPUT OUTPUT` writes a text subtitle track as WebVTT
//! and prints its summary (`coreav.extract-subtitles/1`), and `coreav
//! chapters FILE` prints the QuickTime chapter list (`coreav.chapters/1`;
//! `docs/design/text.md`). A rejected file prints a `coreav.error/1` object
//! on stderr. `coreav check --policy POLICY FILE` checks a file against an
//! upload policy (`coreav.policy/1`) and prints the verdict
//! (`coreav.verdict/1`, `docs/schema/verdict-v1.md`) on stdout, for a
//! rejected file too.
//! `coreav hls` builds an HLS index of a file and cuts its playlists and
//! CMAF segments from it (`docs/design/hls.md`).
//!
//! Exit codes: 0 success (for `check`, the file is accepted), 1 the file was
//! rejected, 2 usage error (for `check`, also an invalid policy), 3 I/O
//! error, 4 refused: a `coreav ffprobe` or `coreav ffmpeg` command that is
//! not native, or what strict-safe mode refuses (`strict_safe`), 141
//! standard output closed by its reader.
//!
//! Every write to standard output is checked: the JSON documents, the
//! summaries, the output written to `-`, and `--help` and `--version`. A
//! command whose write to standard output failed never exits 0. (A
//! standard output closed before the command starts, `>&-`, is not a
//! failure: the Rust runtime opens `/dev/null` in its place, as for any
//! Rust program, so the output is discarded as with `>/dev/null`.) A write
//! that fails because the reader closed the pipe (`coreav probe f | head`)
//! ends the command with status 141, what a shell reports for a process
//! ended by `SIGPIPE` (128 + 13): Rust ignores `SIGPIPE`, so the write fails
//! with `EPIPE` instead, and the command exits as if the signal had ended
//! it, without a message. Restoring `SIGPIPE`'s default action instead
//! would need a C call (`unsafe`, which this crate forbids) and would
//! also change it for the Python process that runs the same code. Any other write error (a full disk, `/dev/full`)
//! is an I/O error: exit 3 with a `coreav.error/1` object on standard
//! error. When the output itself goes to standard output (`-`), the summary
//! goes to standard error, and a summary that cannot be written there ends
//! the command with exit 3 (141 for a closed pipe), without a message.
//! Errors written to standard error are not checked: there is nowhere left
//! to report them, and the exit code already says the command failed.
//! The output written to `-` goes through a duplicate of the standard
//! output descriptor, past `Stdout`'s line buffering; the documents and
//! messages keep `Stdout` (`docs/design/cli-stdout.md`).

mod compat_cmd;
mod fallback_cmd;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use coreav_cli::output::{self, WriteFailure};
#[cfg(feature = "net")]
use coreav_cli::remote::{self, RemoteSource};
use coreav_cli::{
    check, clip, compat, decode_audio, error::Failure, extract, hls, join, matroska, mcp, mpegts,
    names, native, probe, remux, split, strict_safe::StrictSafe, text, thumbnail,
};
use coreav_isobmff::BufferedSource;
use coreav_isobmff_write::hls::{HlsIndex, HlsOptions, Resource};
use coreav_isobmff_write::{
    AudioChoice, ClipOptions, JoinAudio, JoinOptions, MatroskaRemuxOptions, Metadata,
    OnUnsupported, RemuxOptions, SplitAt, SplitOptions, TrackChoice, VideoChoice,
};
use coreav_policy::Policy;
#[cfg(not(feature = "net"))]
use offline::RemoteSource;
use serde_json::Value;

/// Memory-safe media probing.
#[derive(Parser)]
#[command(name = "coreav", version, about, long_about = None)]
struct Cli {
    /// For services that read untrusted input: CoreAV never runs an ffmpeg
    /// or ffprobe program, which is FFmpeg's C code, as COREAV_STRICT_SAFE=1
    /// does. `coreav ffprobe` and `coreav ffmpeg` refuse a command that is
    /// not native (exit status 4) instead of running it with FFmpeg, and a
    /// thumbnail or transcode never decodes or encodes with FFmpeg: a video
    /// only FFmpeg would decode, and an encode without an x264 program, are
    /// refused (exit status 4).
    #[arg(long)]
    strict_safe: bool,
    #[command(subcommand)]
    command: Command,
}

/// A subcommand's options, built only when the subcommand runs (or its
/// help is shown): clap's derive builds every subcommand's options before
/// it parses anything, so each one would cost every start, `coreav
/// --version` included (docs/design/benchmarks.md, start-up). The
/// subcommand's name and summary stay in the tree, for `coreav --help`.
struct Deferred<T>(T);

impl<T: Args> Args for Deferred<T> {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.defer(T::augment_args)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        cmd.defer(T::augment_args_for_update)
    }
}

impl<T: FromArgMatches> FromArgMatches for Deferred<T> {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        T::from_arg_matches(matches).map(Self)
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        self.0.update_from_arg_matches(matches)
    }
}

impl<T> std::ops::Deref for Deferred<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// `sanitize`'s options, deferred as [`Deferred`]'s are: they are remux's,
/// and sanitize reads MP4/MOV only, so the options that concern Matroska
/// and WebM say so or are hidden.
struct Sanitize(SanitizeArgs);

impl Sanitize {
    fn options(cmd: clap::Command) -> clap::Command {
        SanitizeArgs::augment_args(cmd)
            .mut_arg("streams", |a| {
                a.help(
                    "Stream indexes to keep, comma separated (default: every stream MP4 can carry)",
                )
            })
            .mut_arg("drop_unsupported", |a| a.hide(true))
    }
}

impl Args for Sanitize {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        cmd.defer(Self::options)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        cmd.defer(Self::options)
    }
}

impl FromArgMatches for Sanitize {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        SanitizeArgs::from_arg_matches(matches).map(Self)
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        self.0.update_from_arg_matches(matches)
    }
}

impl std::ops::Deref for Sanitize {
    type Target = SanitizeArgs;

    fn deref(&self) -> &SanitizeArgs {
        &self.0
    }
}

#[derive(Subcommand)]
enum Command {
    /// Describe an MP4, MOV, Matroska or WebM file as JSON.
    Probe(Deferred<ProbeArgs>),
    /// Cut a keyframe-aligned clip of an MP4/MOV file to MP4, without
    /// re-encoding.
    Clip(Deferred<ClipArgs>),
    /// Split an MP4/MOV file at keyframes into MP4 segments, without
    /// re-encoding. Together the segments hold every packet of the kept
    /// streams exactly once.
    Split(Deferred<SplitArgs>),
    /// Check an MP4, MOV, Matroska or WebM file against an upload policy
    /// and print the verdict as JSON.
    Check(Deferred<CheckArgs>),
    /// Copy the AAC audio track of an MP4/MOV file into an M4A file,
    /// without re-encoding.
    ExtractAudio(Deferred<ExtractAudioArgs>),
    /// Decode an audio track of an MP4/MOV, Matroska or WebM file to a WAV
    /// file (FLAC, MP3).
    DecodeAudio(Deferred<DecodeAudioArgs>),
    /// Rewrite an MP4/MOV, Matroska or WebM file as MP4 with the movie
    /// header first (faststart), without re-encoding.
    Remux(Deferred<RemuxArgs>),
    /// Write the text subtitles (mov_text: 3GPP timed text or QuickTime
    /// text) of an MP4/MOV file as WebVTT.
    ExtractSubtitles(Deferred<ExtractSubtitlesArgs>),
    /// Print the QuickTime chapter list of an MP4/MOV file as JSON.
    Chapters(Deferred<ChaptersArgs>),
    /// Rewrite an MP4/MOV file as MP4 without its metadata (location,
    /// device, dates), keeping only allowlisted boxes, without re-encoding.
    Sanitize(Sanitize),
    /// Join MP4/MOV files that share a codec configuration into one MP4,
    /// without re-encoding.
    Join(Deferred<JoinArgs>),
    /// Serve these commands as Model Context Protocol tools over standard
    /// input and output, for agents (docs/design/mcp.md).
    Mcp(Deferred<McpArgs>),
    /// Serve an MP4/MOV file as HLS with fragmented MP4 (CMAF) segments,
    /// without re-encoding.
    #[command(subcommand)]
    Hls(HlsCommand),
    /// Write a thumbnail of an MP4, MOV, Matroska or WebM file: the
    /// keyframe at or before a time (or with --exact the frame at it),
    /// resized and encoded as JPEG, PNG or WebP.
    // Only in builds with a decoder backend, which can make one; its
    // arguments would cost every other command's start-up
    // (docs/design/benchmarks.md).
    #[cfg(feature = "thumbnail")]
    Thumbnail(Deferred<ThumbnailArgs>),
    /// Re-encode the video of an MP4, MOV, Matroska or WebM file to H.264
    /// in MP4 and copy its audio (H.264 sources supported, other source
    /// codecs experimental). The encoder is an x264 or ffmpeg program the
    /// operator installs.
    // Only in builds with a decoder backend (docs/design/transcode.md).
    #[cfg(feature = "transcode")]
    Transcode(Deferred<TranscodeArgs>),
    /// Experimental. Runs an ffprobe command line: ffprobe's own options, a
    /// supported subset. With --strict-safe (COREAV_STRICT_SAFE=1), for
    /// untrusted input, anything else is refused with exit status 4;
    /// without it, anything else runs the ffprobe program on PATH, FFmpeg's
    /// C code (docs/design/compat-cli.md). Values can differ from ffprobe's for
    /// MP4/MOV and MPEG-TS; Matroska/WebM agree on 98% or more of files for
    /// 99 of 101 keys (docs/measurements/compat-ffprobe.md). `coreav` run
    /// as `ffprobe` does the same.
    #[command(disable_help_flag = true)]
    Ffprobe(Deferred<ToolArgs>),
    /// Runs an ffmpeg stream-copy command line (-c copy) with CoreAV's own
    /// writers: remuxing to MP4, clipping, segmenting and audio extraction
    /// to M4A, with ffmpeg's stream selection. With --strict-safe
    /// (COREAV_STRICT_SAFE=1), for untrusted input, anything else is refused
    /// with exit status 4; without it, anything else runs the ffmpeg program
    /// on PATH, FFmpeg's C code (docs/design/compat-cli.md). `coreav` run as
    /// `ffmpeg` does the same.
    #[command(disable_help_flag = true)]
    Ffmpeg(Deferred<ToolArgs>),
}

/// A tool's own arguments, passed through untouched.
#[derive(Args)]
struct ToolArgs {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        num_args = 0..,
        value_name = "ARGS"
    )]
    args: Vec<OsString>,
}

#[cfg(feature = "transcode")]
#[derive(Args)]
#[command(
    about = "Re-encode the video to H.264 in MP4 and copy the audio",
    long_about = "Re-encode the video of an MP4, MOV, Matroska or WebM file to H.264 in MP4 \
and copy its audio, as `ffmpeg -i IN -c:v libx264 -c:a copy OUT.mp4` does \
(docs/design/transcode.md).\n\n\
With an H.264 source it is supported: the options and the coreav.transcode/1 \
summary are stable. Other source codecs (HEVC, AV1, VP9, VP8, ProRes) are \
experimental: measured slower than FFmpeg's command and with known defects \
(docs/measurements/transcode.md). The encoder is a separate program the operator installs: \
an x264 program, or an ffmpeg program built with libx264; CoreAV runs it as a \
resource-limited process and gives it only raw frames."
)]
struct TranscodeArgs {
    /// The file (or http(s) URL) to transcode.
    input: PathBuf,
    /// Where to write the MP4 file: a new file (not the input; an existing
    /// one only with --force).
    output: PathBuf,
    /// Constant rate factor, 0 to 51 (default 23).
    #[arg(long, value_name = "Q", conflicts_with = "bitrate",
          value_parser = parse_crf)]
    crf: Option<f32>,
    /// Average bit rate in kbit/s, one pass, instead of --crf.
    #[arg(long, value_name = "KBPS",
          value_parser = clap::value_parser!(u32).range(1..=2_000_000))]
    bitrate: Option<u32>,
    /// x264's preset, fastest to slowest: ultrafast, superfast, veryfast,
    /// faster, fast, medium, slow, slower, veryslow, placebo.
    #[arg(long, value_name = "PRESET", default_value = "medium",
          value_parser = coreav_transcode::encoder::PRESETS)]
    preset: String,
    /// Output size as FFmpeg's scale filter takes it: W:H, each side in
    /// pixels, -1 (from the other side, keeping the aspect ratio) or -2
    /// (the same, even). Default: the source's size.
    #[arg(long, value_name = "W:H", allow_hyphen_values = true)]
    scale: Option<String>,
    /// `copy` every audio stream MP4 can carry (the others are dropped and
    /// listed), or `none`.
    #[arg(long, value_name = "MODE", default_value = "copy", value_parser = ["copy", "none"])]
    audio: String,
    /// The encoder program: `x264`, `ffmpeg` (with libx264), or `auto`:
    /// x264 when there is one, else ffmpeg. With --strict-safe, `ffmpeg`
    /// is refused and `auto` is x264 only.
    #[arg(long, value_name = "ENCODER", default_value = "auto",
          value_parser = ["auto", "x264", "ffmpeg"])]
    encoder: String,
    /// The x264 program (default: COREAV_X264, else x264 on PATH).
    #[arg(long, value_name = "PATH")]
    x264: Option<PathBuf>,
    /// The ffmpeg program (default: COREAV_FFMPEG, else ffmpeg on PATH).
    #[arg(long, value_name = "PATH")]
    ffmpeg: Option<PathBuf>,
    /// Encoder threads (default: the encoder's own choice).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=128))]
    threads: Option<u32>,
    /// Seconds the whole transcode may take before it is stopped.
    #[arg(long, value_name = "SECONDS", default_value_t = 3600,
          value_parser = clap::value_parser!(u64).range(1..=604_800))]
    timeout: u64,
    /// Overwrite OUTPUT if it exists.
    #[arg(long)]
    force: bool,
}

/// `--crf`: a number from 0 to 51.
#[cfg(feature = "transcode")]
fn parse_crf(text: &str) -> Result<f32, String> {
    text.parse::<f32>()
        .ok()
        .filter(|q| (0.0..=51.0).contains(q))
        .ok_or_else(|| format!("'{text}' is not a number from 0 to 51"))
}

#[cfg(feature = "thumbnail")]
#[derive(Args)]
struct ThumbnailArgs {
    /// The file (or http(s) URL) to take the thumbnail of.
    input: PathBuf,
    /// Where to write the image: a new file, or `-` for standard output.
    #[arg(short, long, value_name = "FILE")]
    output: String,
    /// The time, from the file's start as for `ffmpeg -ss`: seconds (`12`,
    /// `12.5`, `12.5s`), milliseconds (`1500ms`) or `[HH:]MM:SS[.fff]`.
    #[arg(long, value_name = "TIME", default_value = "0")]
    at: String,
    /// The first frame at or after the time (decoding from the keyframe
    /// before it) instead of the keyframe at or before it.
    #[arg(long)]
    exact: bool,
    /// Image width; the height follows the display aspect ratio unless
    /// --height is given too.
    #[arg(long, value_name = "PIXELS", value_parser = clap::value_parser!(u32).range(1..=16_384))]
    width: Option<u32>,
    /// Image height.
    #[arg(long, value_name = "PIXELS", value_parser = clap::value_parser!(u32).range(1..=16_384))]
    height: Option<u32>,
    /// `jpeg`, `png` or `webp` (lossless); default: from the output's
    /// extension, else `jpeg`.
    #[arg(long, value_name = "FORMAT", value_parser = ["jpeg", "jpg", "png", "webp"])]
    format: Option<String>,
    /// JPEG quality, 1 to 100.
    #[arg(long, value_name = "Q", default_value_t = 91,
          value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,
    /// The video stream index (default: the first video stream).
    #[arg(long, value_name = "STREAM")]
    stream: Option<usize>,
    /// Codecs that may be decoded, comma separated, by `ffprobe`'s names
    /// (default: h264, hevc, av1, vp9, vp8, prores).
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Decoder threads (default: the decoder's own choice); fewer when the
    /// frames would not fit the decoder's memory budget.
    #[arg(long, value_name = "N", default_value_t = 0)]
    threads: u32,
    /// Seconds decoding may take before it is stopped.
    #[arg(long, value_name = "SECONDS", default_value_t = 30,
          value_parser = clap::value_parser!(u64).range(1..=86_400))]
    timeout: u64,
    /// Overwrite OUTPUT if it exists.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct McpArgs {
    /// A directory the tools may read and write in; repeat it for more.
    /// Paths outside every root are refused. Default: the current
    /// directory.
    #[arg(long, value_name = "DIR")]
    root: Vec<PathBuf>,
    /// Seconds one tool call may run before it is stopped.
    #[arg(long, value_name = "SECONDS", default_value_t = 600,
          value_parser = clap::value_parser!(u64).range(1..=86_400))]
    timeout: u64,
    /// Which URLs the tools read: `public` (https:// URLs of public
    /// addresses), `any` (as the command line, private addresses and
    /// http:// included) or `none`.
    #[arg(long, value_name = "WHICH", default_value = "public",
          value_parser = ["public", "any", "none"])]
    urls: String,
}

#[derive(Args)]
struct SanitizeArgs {
    #[command(flatten)]
    remux: RemuxArgs,
    /// What to keep, comma separated: `rotation`, `language`, `dates`
    /// (creation times), `gapless` (encoder delay and padding), `kinds`
    /// (track roles), or `none`.
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "FIELD",
        default_value = "rotation,language,gapless,kinds"
    )]
    keep: Vec<String>,
}

#[derive(Subcommand)]
enum HlsCommand {
    /// Build the index that playlists and segments are cut from.
    Index(Deferred<HlsIndexArgs>),
    /// Write one resource (`master.m3u8`, `video.m3u8`, `video-init.mp4`,
    /// `video-0.m4s`, ...) from an index and its source.
    Get(Deferred<HlsGetArgs>),
    /// Build the index and write every resource into a directory.
    Package(Deferred<HlsPackageArgs>),
}

#[derive(Args)]
struct HlsTrackArgs {
    /// Target segment duration in seconds.
    #[arg(long, value_name = "TIME", default_value = "6")]
    segment_duration: String,
    /// The video stream index, or `none` for audio alone (default: the
    /// first video stream).
    #[arg(long, value_name = "STREAM|none")]
    video: Option<String>,
    /// The audio stream index, or `none` (default: the first audio stream).
    #[arg(long, value_name = "STREAM|none")]
    audio: Option<String>,
    /// Codecs to accept, comma separated, as for `coreav probe`.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject the whole file when a stream's codec is not allowed.
    #[arg(long)]
    strict: bool,
    /// Text stored in the index to identify the source version (for
    /// example its entity tag), at most 255 bytes.
    #[arg(long, value_name = "TEXT", default_value = "")]
    tag: String,
}

#[derive(Args)]
struct HlsIndexArgs {
    /// The file to index.
    input: PathBuf,
    /// Where to write the index: a new file, or `-` for standard output.
    index: String,
    #[command(flatten)]
    tracks: HlsTrackArgs,
    /// Overwrite INDEX if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct HlsGetArgs {
    /// The index built by `coreav hls index`.
    index: PathBuf,
    /// The file the index was built from.
    input: PathBuf,
    /// The resource name.
    name: String,
    /// Where to write it: a new file, or `-` for standard output.
    output: String,
    /// Overwrite OUTPUT if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct HlsPackageArgs {
    /// The file to package.
    input: PathBuf,
    /// A new or empty directory for the index and the resources (not `-`:
    /// a package is several files; `./-` names a directory called `-`).
    dir: PathBuf,
    #[command(flatten)]
    tracks: HlsTrackArgs,
}

#[derive(Args)]
struct RemuxArgs {
    /// The file to rewrite.
    input: PathBuf,
    /// Where to write the MP4 file: a new file, or `-` for standard output.
    output: String,
    /// Stream indexes to keep, comma separated (default: every stream MP4
    /// can carry; for Matroska and WebM, a video stream it cannot carry
    /// fails the remux unless --drop-unsupported is given).
    #[arg(long, value_delimiter = ',', value_name = "STREAM")]
    streams: Option<Vec<usize>>,
    /// Fail instead of dropping a stream that MP4 cannot carry.
    #[arg(long, conflicts_with = "drop_unsupported")]
    fail_on_unsupported: bool,
    /// Matroska and WebM sources: also drop a video stream that MP4 cannot
    /// carry (or that the allowlist leaves out), writing the other streams;
    /// by default that fails, so a video never silently becomes an
    /// audio-only file. MP4 sources drop such streams anyway.
    #[arg(long)]
    drop_unsupported: bool,
    /// Codecs to accept, comma separated, as for `coreav probe`.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject the whole file when a stream's codec is not allowed.
    #[arg(long)]
    strict: bool,
    /// Overwrite OUTPUT if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct SplitArgs {
    /// The file to split.
    input: PathBuf,
    /// Where to write the segments: a name with `%d`, or `%03d` for three
    /// digits, replaced by the segment number from 0 (for example
    /// `seg-%03d.mp4`).
    output: String,
    /// Cut at the latest keyframe at or before every multiple of this many
    /// seconds (for example `10`).
    #[arg(
        long,
        value_name = "SECONDS",
        required_unless_present = "at",
        conflicts_with = "at"
    )]
    every: Option<String>,
    /// Cut at the latest keyframe at or before each of these times, in
    /// increasing seconds, comma separated (for example `10,25.5`).
    #[arg(long, value_name = "TIMES")]
    at: Option<String>,
    /// The video stream index (default: the first video stream).
    #[arg(long, value_name = "STREAM")]
    video: Option<usize>,
    /// The audio stream index, or `none` (default: the first audio stream).
    #[arg(long, value_name = "STREAM|none")]
    audio: Option<String>,
    /// Codecs to accept, comma separated, as for `coreav probe`.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject the whole file when a stream's codec is not allowed.
    #[arg(long)]
    strict: bool,
    /// Overwrite segment files that exist (a symbolic link is replaced, not
    /// written through; a new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct ExtractSubtitlesArgs {
    /// The file to read.
    input: PathBuf,
    /// Where to write the WebVTT file: a new file, or `-` for standard
    /// output.
    output: String,
    /// The subtitle stream index (default: the first text stream that is
    /// not a chapter track).
    #[arg(long, value_name = "STREAM")]
    stream: Option<usize>,
    /// Overwrite OUTPUT if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct ChaptersArgs {
    /// The file to read.
    file: PathBuf,
}

#[derive(Args)]
struct JoinArgs {
    /// The files to join, in order.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Where to write the joined file: a new file, or `-` for standard
    /// output.
    #[arg(long, short = 'o', value_name = "OUTPUT")]
    output: String,
    /// `none` to join the video only (default: the first audio stream of
    /// every input).
    #[arg(long, value_name = "none")]
    audio: Option<String>,
    /// Codecs to accept, comma separated, as for `coreav probe`.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject an input when a stream's codec is not allowed.
    #[arg(long)]
    strict: bool,
    /// Overwrite OUTPUT if it exists.
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct CheckArgs {
    /// The policy: a `coreav.policy/1` JSON file (docs/schema/policy-v1.md).
    #[arg(long, value_name = "POLICY")]
    policy: PathBuf,
    /// The file to check.
    file: PathBuf,
}

#[derive(Args)]
struct ExtractAudioArgs {
    /// The file to read.
    input: PathBuf,
    /// Where to write the M4A file: a new file, or `-` for standard output.
    output: String,
    /// The audio stream index (default: the first audio stream).
    #[arg(long, value_name = "STREAM")]
    stream: Option<usize>,
    /// Codecs to accept, comma separated, as for `coreav probe`.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject the whole file when a stream's codec is not allowed.
    #[arg(long)]
    strict: bool,
    /// Overwrite OUTPUT if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct DecodeAudioArgs {
    /// The file to read.
    input: PathBuf,
    /// Where to write the WAV file: a new file, or `-` for standard output.
    output: String,
    /// The audio stream index (default: the first audio stream whose codec
    /// CoreAV decodes).
    #[arg(long, value_name = "STREAM")]
    stream: Option<usize>,
    /// Threads that decode in parallel; 0 for one per CPU, at most 16.
    #[arg(long, value_name = "N", default_value_t = 0,
          value_parser = clap::value_parser!(u16).range(0..=16))]
    threads: u16,
    /// Sample format of the output: `s16` (16-bit integers, as ffmpeg
    /// writes WAV by default), `s24`, `s32` or `f32` (32-bit float).
    #[arg(long, value_name = "FORMAT", default_value = "s16",
          value_parser = ["s16", "s24", "s32", "f32"])]
    format: String,
    /// Sample rate of the output in Hz, 1000 to 768000 (default: the
    /// stream's), for example 16000 for speech recognition. Another rate
    /// is resampled.
    #[arg(long, value_name = "HZ",
          value_parser = clap::value_parser!(u32).range(1000..=768_000))]
    rate: Option<u32>,
    /// Channels of the output (default: the stream's): 1 or 2 downmix
    /// other layouts (ITU-R BS.775); mono becomes stereo at -3 dB.
    #[arg(long, value_name = "N",
          value_parser = clap::value_parser!(u16).range(1..=32))]
    channels: Option<u16>,
    /// Overwrite OUTPUT if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct ClipArgs {
    /// The file to clip.
    input: PathBuf,
    /// Where to write the clip: a new file, or `-` for standard output.
    output: String,
    /// Start in seconds (for example `12.5`). The clip starts at the latest
    /// keyframe at or before it.
    #[arg(long, value_name = "TIME")]
    start: String,
    /// End in seconds; the end of the video when absent.
    #[arg(long, value_name = "TIME")]
    end: Option<String>,
    /// The video stream index (default: the first video stream).
    #[arg(long, value_name = "STREAM")]
    video: Option<usize>,
    /// The audio stream index, or `none` (default: the first audio stream).
    #[arg(long, value_name = "STREAM|none")]
    audio: Option<String>,
    /// Codecs to accept, comma separated, as for `coreav probe`.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject the whole file when a stream's codec is not allowed.
    #[arg(long)]
    strict: bool,
    /// Overwrite OUTPUT if it exists (a symbolic link is replaced, not
    /// written through; the new file keeps the old one's permissions).
    #[arg(long)]
    force: bool,
}

#[derive(Args)]
struct ProbeArgs {
    /// The file to probe.
    file: PathBuf,
    /// Print the `ffprobe -of json -show_format -show_streams` shape,
    /// limited to fields checked against `ffprobe`.
    #[arg(long)]
    ffprobe_compat: bool,
    /// Read every movie fragment so that fragmented streams get their
    /// durations (the default with --ffprobe-compat).
    #[arg(long, conflicts_with = "no_scan_fragments")]
    scan_fragments: bool,
    /// Do not read movie fragments, even with --ffprobe-compat.
    #[arg(long)]
    no_scan_fragments: bool,
    /// Read the parameter sets and frame headers in every sample of each
    /// video stream for `video.scanned_size` (docs/design/frame-sizes.md).
    /// Reads the headers of every sample, not their coded data. For
    /// Matroska and WebM, the same pass also gives each stream's
    /// `duration`, `frame_count` and `bit_rate`.
    #[arg(long)]
    scan_frame_sizes: bool,
    /// Codecs to accept, comma separated (for example `h264,aac`); streams
    /// with any other codec are dropped. Default: every supported codec.
    /// Unsupported codecs (`other`) can never be allowed. Names of codecs a
    /// container cannot hold are ignored for that container.
    #[arg(long, value_delimiter = ',', value_name = "CODEC")]
    allow: Option<Vec<String>>,
    /// Reject the whole file when a stream's codec is not allowed, instead
    /// of dropping that stream.
    #[arg(long)]
    strict: bool,
}

/// Exit code for a file that was rejected.
const EXIT_REJECTED: u8 = 1;
/// Exit code for a usage error (also used by the argument parser).
const EXIT_USAGE: u8 = 2;
/// Exit code for an I/O error.
const EXIT_IO: u8 = 3;
/// Exit code for a standard output closed by its reader: 128 + `SIGPIPE`.
const EXIT_BROKEN_PIPE: u8 = 141;

fn main() -> ExitCode {
    // `coreav thumbnail` runs this program again as its sandboxed decode
    // worker (docs/design/decode.md), and `coreav transcode` as the
    // launcher of its encoder (docs/design/transcode.md), before any
    // argument parsing.
    if let Some(code) = thumbnail::run_worker() {
        return code;
    }
    #[cfg(target_os = "wasi")]
    if let Err(code) = enter_working_directory() {
        return code;
    }
    // Started through a link named `ffprobe` or `ffmpeg`: that tool's
    // command line, whole (docs/design/compat-cli.md, "Multicall").
    let args: Vec<OsString> = std::env::args_os().collect();
    match args.first().and_then(compat_cmd::multicall) {
        Some("ffprobe") => {
            return compat_cmd::run_ffprobe(
                args.get(1..).unwrap_or_default(),
                StrictSafe::new(false),
            );
        }
        Some(_) => {
            return compat_cmd::run_ffmpeg(
                args.get(1..).unwrap_or_default(),
                StrictSafe::new(false),
            );
        }
        None => {}
    }
    main_with(args, std::env::current_exe().ok())
}

/// Parses `args` (the program name first) and runs the command; `this` is
/// the command itself, which `coreav mcp` runs for each tool call. `--help`,
/// `--version` and usage errors are printed as the argument parser prints
/// them, with the write checked. The Python package's `coreav` command
/// calls this with its own arguments (`docs/design/python.md`).
pub(crate) fn main_with(
    args: impl IntoIterator<Item = OsString>,
    this: Option<PathBuf>,
) -> ExitCode {
    // The argument parser's tables and matches stay allocated until the
    // command has run, and are freed after it: freed before, their many
    // small blocks would be merged by the allocator at the command's first
    // large allocation, a cost that grows with the number of subcommands and
    // has nothing to do with the command's work (`docs/design/benchmarks.md`).
    let mut command = Cli::command();
    let parsed = match command.try_get_matches_from_mut(args) {
        Ok(mut matches) => match Cli::from_arg_matches_mut(&mut matches) {
            Ok(cli) => Ok((cli, matches)),
            // As the derived parser does: the error names the command.
            Err(error) => Err(error.format(&mut command)),
        },
        Err(error) => Err(error),
    };
    let code = match parsed {
        Ok((cli, matches)) => {
            let code = run(cli, this);
            drop(matches);
            code
        }
        Err(error) => match error.print() {
            // 0 for `--help` and `--version`, 2 for a usage error.
            Ok(()) => ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(EXIT_USAGE)),
            // A usage error that could not be printed is still one.
            Err(_) if error.use_stderr() => ExitCode::from(EXIT_USAGE),
            Err(io) => write_error(&io),
        },
    };
    drop(command);
    code
}

/// The exit for a failed write to standard output: [`EXIT_BROKEN_PIPE`]
/// without a message for a closed pipe, otherwise exit 3 with the error on
/// standard error.
fn write_error(error: &std::io::Error) -> ExitCode {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        return ExitCode::from(EXIT_BROKEN_PIPE);
    }
    let _ = writeln!(
        std::io::stderr(),
        "{}",
        WriteFailure::output(error).to_json()
    );
    ExitCode::from(EXIT_IO)
}

/// Prints `document` on standard output, flushed. Err is the exit for a
/// failed write ([`write_error`]).
fn print_document(document: &Value) -> Result<(), ExitCode> {
    let text = serde_json::to_string_pretty(document).unwrap_or_default();
    let mut out = std::io::stdout().lock();
    writeln!(out, "{text}")
        .and_then(|()| out.flush())
        .map_err(|error| write_error(&error))
}

/// Standard output for media bytes (an output written to `-`), buffered.
///
/// `std::io::Stdout` is line-buffered whatever it is connected to: every
/// write runs `memrchr` for a newline over the whole buffer, and a 0x0a
/// byte in the media flushes there. So the media go through a `File` on a
/// duplicate of the standard output descriptor (Unix) or handle (Windows),
/// which the standard library opens without `unsafe`. Where there is no
/// descriptor to duplicate (WASI, or a Windows process without a standard
/// output handle) the media go through `Stdout` as before. Rust's own
/// buffer is flushed first, so that nothing written through `Stdout`
/// earlier comes after the media; the JSON documents, messages and `--help`
/// keep using `Stdout`.
///
/// The buffer is std's default 8 KiB, as for a file output: a larger one
/// would copy the sample runs that now go to the kernel straight from the
/// source's buffer (`docs/design/cli-stdout.md`).
fn media_stdout() -> std::io::BufWriter<Box<dyn Write>> {
    let stdout = std::io::stdout();
    // An error here shows again on the first write of the media.
    let _ = stdout.lock().flush();
    #[cfg(unix)]
    let duplicate = {
        use std::os::fd::AsFd;
        stdout.as_fd().try_clone_to_owned().map(std::fs::File::from)
    };
    // A Windows process without standard output has a null handle, which
    // std duplicates into a handle that fails every write; `Stdout`
    // discards the bytes instead, so that case keeps `Stdout`.
    #[cfg(windows)]
    let duplicate = {
        use std::os::windows::io::{AsHandle, AsRawHandle};
        if stdout.as_raw_handle().is_null() {
            Err(std::io::ErrorKind::Unsupported.into())
        } else {
            stdout
                .as_handle()
                .try_clone_to_owned()
                .map(std::fs::File::from)
        }
    };
    #[cfg(not(any(unix, windows)))]
    let duplicate: std::io::Result<std::fs::File> = Err(std::io::ErrorKind::Unsupported.into());
    let inner: Box<dyn Write> = match duplicate {
        Ok(file) => Box::new(file),
        Err(_) => Box::new(stdout.lock()),
    };
    std::io::BufWriter::new(inner)
}

/// A writer that remembers whether a write or a flush failed because the
/// reader closed the pipe, so that the output written to `-` can end with
/// [`EXIT_BROKEN_PIPE`] however the library reports the failed write.
struct Watched<W> {
    inner: W,
    broken_pipe: bool,
}

impl<W: Write> Watched<W> {
    fn note<T>(&mut self, result: std::io::Result<T>) -> std::io::Result<T> {
        if let Err(error) = &result {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                self.broken_pipe = true;
            }
        }
        result
    }
}

impl<W: Write> Write for Watched<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let result = self.inner.write(buf);
        self.note(result)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let result = self.inner.flush();
        self.note(result)
    }
}

/// Runs the command `cli` describes.
fn run(cli: Cli, this: Option<PathBuf>) -> ExitCode {
    match cli.command {
        Command::Mcp(args) => run_mcp(&args, this),
        Command::Ffprobe(args) => {
            compat_cmd::run_ffprobe(&args.args, StrictSafe::new(cli.strict_safe))
        }
        Command::Ffmpeg(args) => {
            compat_cmd::run_ffmpeg(&args.args, StrictSafe::new(cli.strict_safe))
        }
        Command::Probe(args) => run_probe(&args),
        Command::Clip(args) => run_clip(&args),
        Command::Check(args) => run_check(&args),
        Command::ExtractAudio(args) => run_extract_audio(&args),
        Command::DecodeAudio(args) => run_decode_audio(&args),
        Command::Remux(args) => run_remux(&args, Metadata::Copy),
        Command::Sanitize(args) => match remux::parse_keep(&args.keep) {
            Ok(keep) => run_remux(&args.remux, Metadata::Sanitize(keep)),
            Err(unknown) => usage(&format!(
                "cannot keep '{unknown}': --keep takes {}, or none alone",
                remux::KEEP_NAMES.join(", ")
            )),
        },
        Command::Split(args) => run_split(&args),
        Command::ExtractSubtitles(args) => run_extract_subtitles(&args),
        Command::Chapters(args) => run_chapters(&args),
        Command::Join(args) => run_join(&args),
        Command::Hls(HlsCommand::Index(args)) => run_hls_index(&args),
        Command::Hls(HlsCommand::Get(args)) => run_hls_get(&args),
        Command::Hls(HlsCommand::Package(args)) => run_hls_package(&args),
        #[cfg(feature = "thumbnail")]
        Command::Thumbnail(args) => {
            run_thumbnail(&args, this.as_deref(), StrictSafe::new(cli.strict_safe))
        }
        #[cfg(feature = "transcode")]
        Command::Transcode(args) => {
            run_transcode(&args, this.as_deref(), StrictSafe::new(cli.strict_safe))
        }
    }
}

#[cfg(feature = "transcode")]
fn run_transcode(args: &TranscodeArgs, this: Option<&Path>, strict_safe: StrictSafe) -> ExitCode {
    use coreav_cli::transcode;
    let scale = match args.scale.as_deref().map(transcode::scale).transpose() {
        Ok(scale) => scale,
        Err(message) => return usage(&message),
    };
    let encoder = match strict_safe.encoder(&args.encoder, args.x264.as_deref()) {
        Ok(encoder) => encoder,
        Err(message) => {
            let _ = writeln!(
                std::io::stderr(),
                "{}",
                Failure::StrictSafe(message).to_json()
            );
            return ExitCode::from(compat_cmd::EXIT_UNSUPPORTED);
        }
    };
    let request = transcode::Request {
        crf: args.crf,
        bitrate: args.bitrate,
        preset: args.preset.clone(),
        scale,
        no_audio: args.audio == "none",
        audio_streams: None,
        pixel_format: None,
        one_size: false,
        encoder,
        x264: args.x264.clone(),
        ffmpeg: args.ffmpeg.clone(),
        threads: args.threads,
        timeout: args.timeout,
        strict_safe,
    };
    if args.output.as_os_str() == "-" {
        return usage("the output must be a file: the MP4 is written with its movie box first");
    }
    let Some(output) = args.output.to_str() else {
        return usage("the output path must be valid UTF-8");
    };
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    write_output(&args.input, output, args.force, |out| {
        match remote.as_mut() {
            None => coreav_isobmff::FileSource::open(&args.input)
                .map_err(|e| WriteFailure::Rejected(Failure::library(e)))
                .and_then(|file| {
                    transcode::make(&request, this, BufferedSource::new(file), &args.output, out)
                }),
            Some(source) => transcode::make(
                &request,
                this,
                BufferedSource::new(&mut *source),
                &args.output,
                out,
            )
            .map_err(|f| explain_write(source, f)),
        }
    })
}

#[cfg(feature = "thumbnail")]
fn run_thumbnail(args: &ThumbnailArgs, this: Option<&Path>, strict_safe: StrictSafe) -> ExitCode {
    let Some(at) = thumbnail::parse_at(&args.at) else {
        return usage("--at must be a time such as 12, 12.5s, 1500ms or 1:02.5");
    };
    if let Some(names) = &args.allow {
        if let Err(message) = thumbnail::check_allow(names) {
            return usage(&message);
        }
    }
    let request = thumbnail::Request {
        at,
        exact: args.exact,
        width: args.width,
        height: args.height,
        format: args.format.clone(),
        output: args.output.clone(),
        quality: args.quality,
        stream: args.stream,
        allow: args.allow.clone(),
        threads: args.threads,
        timeout: args.timeout,
        strict_safe,
    };
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    write_output(&args.input, &args.output, args.force, |out| {
        let (image, document) = match remote.as_mut() {
            None => coreav_isobmff::FileSource::open(&args.input)
                .map_err(|e| WriteFailure::Rejected(Failure::library(e)))
                .and_then(|file| thumbnail::make(&request, this, BufferedSource::new(file))),
            Some(source) => thumbnail::make(&request, this, BufferedSource::new(&mut *source))
                .map_err(|f| explain_write(source, f)),
        }?;
        out.write_all(&image)
            .map_err(|e| WriteFailure::output(&e))?;
        Ok(document)
    })
}

fn run_mcp(args: &McpArgs, this: Option<PathBuf>) -> ExitCode {
    // The server runs the command as a child process for each tool call;
    // a WASI module (the npm package's fallback) has no processes.
    if cfg!(target_os = "wasi") {
        return usage(
            "coreav mcp needs the native coreav command; this WebAssembly build cannot start processes",
        );
    }
    // Never a `coreav` found on PATH, which may be another program.
    let Some(this) = this else {
        let _ = writeln!(
            std::io::stderr(),
            "error: cannot find the path of the running coreav command"
        );
        return ExitCode::from(EXIT_IO);
    };
    let roots = if args.root.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.root.clone()
    };
    let config = mcp::Config {
        roots,
        command: this,
        timeout: std::time::Duration::from_secs(args.timeout),
        urls: match args.urls.as_str() {
            "any" => mcp::Urls::Any,
            "none" => mcp::Urls::None,
            _ => mcp::Urls::Public,
        },
    };
    let mut server = match mcp::Server::new(config) {
        Ok(server) => server,
        Err(message) => return usage(&message),
    };
    match mcp::serve(
        &mut server,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => write_error(&error),
    }
}

/// A WASI module starts in `/`. The npm launcher (`packages/cli`) preopens
/// the host's root as `/` and passes the caller's working directory in
/// `COREAV_CWD`, so that relative paths resolve as they do natively.
#[cfg(target_os = "wasi")]
fn enter_working_directory() -> Result<(), ExitCode> {
    let Some(dir) = std::env::var_os("COREAV_CWD") else {
        return Ok(());
    };
    // An I/O error (exit 3, `coreav.error/1` on stderr), as for any path
    // the command cannot reach.
    std::env::set_current_dir(dir).map_err(|error| fail(&Failure::io(error)))
}

fn usage(message: &str) -> ExitCode {
    let _ = writeln!(std::io::stderr(), "error: {message}");
    ExitCode::from(EXIT_USAGE)
}

fn run_clip(args: &ClipArgs) -> ExitCode {
    let allowlist = match probe::allowlist(args.allow.as_deref(), args.strict) {
        Ok(allowlist) => allowlist,
        Err(unknown) => {
            return usage(&format!(
                "unknown codec '{unknown}' in --allow; known: {}",
                names::ALLOW_NAMES.join(", ")
            ));
        }
    };
    let Some(start) = clip::parse_time(&args.start) else {
        return usage("--start must be decimal seconds, such as 12 or 12.5");
    };
    let end = match args.end.as_deref().map(clip::parse_time) {
        None => None,
        Some(Some(end)) => Some(end),
        Some(None) => return usage("--end must be decimal seconds, such as 12 or 12.5"),
    };
    let audio = match args.audio.as_deref() {
        None => AudioChoice::First,
        Some("none") => AudioChoice::None,
        Some(index) => match index.parse() {
            Ok(index) => AudioChoice::Stream(index),
            Err(_) => return usage("--audio must be a stream index or `none`"),
        },
    };
    let options = ClipOptions {
        video: args.video.map_or(VideoChoice::First, VideoChoice::Stream),
        audio,
        start,
        end,
    };
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    write_output(&args.input, &args.output, args.force, |out| {
        let summary = match remote.as_mut() {
            None => clip::run(&args.input, &options, &allowlist, out),
            Some(source) => {
                clip::run_source(BufferedSource::new(&mut *source), &options, &allowlist, out)
                    .map_err(|f| explain_write(source, f))
            }
        };
        summary.map(|s| clip::document(&s))
    })
}

fn run_extract_audio(args: &ExtractAudioArgs) -> ExitCode {
    let allowlist = match probe::allowlist(args.allow.as_deref(), args.strict) {
        Ok(allowlist) => allowlist,
        Err(unknown) => {
            return usage(&format!(
                "unknown codec '{unknown}' in --allow; known: {}",
                names::ALLOW_NAMES.join(", ")
            ));
        }
    };
    let choice = args
        .stream
        .map_or(TrackChoice::FirstAudio, TrackChoice::Stream);
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    write_output(&args.input, &args.output, args.force, |out| {
        let summary = match remote.as_mut() {
            None => extract::run(&args.input, choice, &allowlist, out),
            Some(source) => {
                extract::run_source(BufferedSource::new(&mut *source), choice, &allowlist, out)
                    .map_err(|f| explain_write(source, f))
            }
        };
        summary.map(|s| extract::document(&s))
    })
}

fn run_decode_audio(args: &DecodeAudioArgs) -> ExitCode {
    let options = coreav_audio::Options {
        stream: args.stream,
        format: coreav_audio::SampleFormat::from_name(&args.format)
            .unwrap_or(coreav_audio::SampleFormat::S16),
        threads: usize::from(args.threads),
        sample_rate: args.rate,
        channels: args.channels,
    };
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    write_output_patched(&args.input, &args.output, args.force, |out| {
        let mut counted = Counted {
            inner: out,
            bytes: 0,
        };
        let result = match remote.as_mut() {
            None => decode_audio::run(&args.input, &options, &mut counted),
            Some(source) => {
                decode_audio::run_source(BufferedSource::new(&mut *source), &options, &mut counted)
                    .map_err(|f| explain_write(source, f))
            }
        };
        let bytes = counted.bytes;
        result.map(|(summary, header)| (decode_audio::document(&summary, bytes), header))
    })
}

/// Counts the bytes written through it.
struct Counted<'a> {
    inner: &'a mut dyn Write,
    bytes: u64,
}

impl Write for Counted<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.bytes = self.bytes.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn run_extract_subtitles(args: &ExtractSubtitlesArgs) -> ExitCode {
    let choice = args.stream.map_or(
        coreav_text::TrackChoice::First,
        coreav_text::TrackChoice::Stream,
    );
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    write_output(&args.input, &args.output, args.force, |out| {
        let summary = match remote.as_mut() {
            None => text::run_subtitles(&args.input, choice, out),
            Some(source) => {
                text::run_subtitles_source(BufferedSource::new(&mut *source), choice, out)
                    .map_err(|f| explain_write(source, f))
            }
        };
        summary.map(|s| text::subtitles_document(&s))
    })
}

fn run_chapters(args: &ChaptersArgs) -> ExitCode {
    let chapters = match open_remote(&args.file) {
        Ok(None) => text::run_chapters(&args.file),
        Ok(Some(mut source)) => text::run_chapters_source(BufferedSource::new(&mut source))
            .map_err(|f| source.explain(f)),
        Err(exit) => return exit,
    };
    match chapters {
        Ok(chapters) => match print_document(&text::chapters_document(chapters.as_ref())) {
            Ok(()) => ExitCode::SUCCESS,
            Err(exit) => exit,
        },
        Err(failure) => fail(&failure),
    }
}

fn run_remux(args: &RemuxArgs, metadata: Metadata) -> ExitCode {
    let (allowlist, mkv_allowlist) = match matroska::allowlists(args.allow.as_deref(), args.strict)
    {
        Ok(lists) => lists,
        Err(unknown) => {
            let mut known = names::ALLOW_NAMES.to_vec();
            for name in matroska::allow_names() {
                if !known.contains(&name) {
                    known.push(name);
                }
            }
            return usage(&format!(
                "unknown codec '{unknown}' in --allow; known: {}",
                known.join(", ")
            ));
        }
    };
    let options = RemuxOptions {
        streams: args.streams.clone(),
        on_unsupported: if args.fail_on_unsupported {
            OnUnsupported::Fail
        } else {
            OnUnsupported::Drop
        },
        metadata,
    };
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    let detected = match remote.as_mut() {
        None => matroska::detect(&args.input),
        Some(source) => matroska::detect_source(source).map_err(|f| source.explain(f)),
    };
    let detected = match detected {
        Ok(detected) => detected,
        Err(failure) => return fail(&failure),
    };
    if detected && matches!(metadata, Metadata::Sanitize(_)) {
        return fail(&Failure::library(coreav_isobmff::Error::Unsupported {
            reason: "sanitizing reads MP4/MOV files; Matroska and WebM are not supported",
        }));
    }
    let mkv_options = MatroskaRemuxOptions {
        streams: options.streams.clone(),
        on_unsupported: options.on_unsupported,
        drop_video: args.drop_unsupported,
    };
    write_output(&args.input, &args.output, args.force, |out| {
        match (remote.as_mut(), detected) {
            (None, true) => {
                let summary = remux::run_matroska(&args.input, &mkv_options, &mkv_allowlist, out)?;
                Ok(remux::matroska_document(&summary))
            }
            (None, false) => {
                let summary = remux::run(&args.input, &options, &allowlist, out)?;
                Ok(remux::document(&summary, &options))
            }
            (Some(source), true) => {
                let summary = remux::run_matroska_source(
                    BufferedSource::new(&mut *source),
                    &mkv_options,
                    &mkv_allowlist,
                    out,
                )
                .map_err(|f| explain_write(source, f))?;
                Ok(remux::matroska_document(&summary))
            }
            (Some(source), false) => {
                let summary =
                    remux::run_source(BufferedSource::new(&mut *source), &options, &allowlist, out)
                        .map_err(|f| explain_write(source, f))?;
                Ok(remux::document(&summary, &options))
            }
        }
    })
}

/// Runs `write` on `output` (`-` for standard output) and prints the summary
/// it returns: on standard output, or on standard error when the output goes
/// to standard output. A file output goes through [`output::to_file`].
fn write_output(
    input: &Path,
    output: &str,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<Value, WriteFailure>,
) -> ExitCode {
    write_output_with(input, output, force, write, print_json)
}

/// Prints a summary as pretty-printed JSON.
fn print_json(summary: &Value, out: &mut dyn Write) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(summary).unwrap_or_default();
    writeln!(out, "{text}")
}

/// [`write_output`] with the summary printed by `print`.
fn write_output_with<T>(
    input: &Path,
    output: &str,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<T, WriteFailure>,
    print: impl FnOnce(&T, &mut dyn Write) -> std::io::Result<()>,
) -> ExitCode {
    write_output_core(
        input,
        output,
        force,
        |out| write(out).map(|v| (v, None)),
        print,
    )
}

/// [`write_output`] for an output whose first bytes are rewritten at its
/// end when it is a file (`output::to_file_patched`); standard output keeps
/// them as first written.
fn write_output_patched(
    input: &Path,
    output: &str,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<(Value, Option<Vec<u8>>), WriteFailure>,
) -> ExitCode {
    write_output_core(input, output, force, write, print_json)
}

/// The shared body: `write` returns the summary and, for a file, the bytes
/// to rewrite at its start.
fn write_output_core<T>(
    input: &Path,
    output: &str,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<(T, Option<Vec<u8>>), WriteFailure>,
    print: impl FnOnce(&T, &mut dyn Write) -> std::io::Result<()>,
) -> ExitCode {
    let to_stdout = output == "-";
    let mut broken_pipe = false;
    let result = if to_stdout {
        let mut out = Watched {
            inner: media_stdout(),
            broken_pipe: false,
        };
        let result = write(&mut out).and_then(|(summary, _)| match out.flush() {
            Ok(()) => Ok(summary),
            Err(error) => Err(WriteFailure::output(&error)),
        });
        broken_pipe = out.broken_pipe;
        result
    } else {
        let path = PathBuf::from(output);
        if let Some(message) = output::refuse(input, &path, force) {
            return usage(&message);
        }
        output::to_file_patched(&path, force, write)
    };
    match result {
        Ok(summary) if to_stdout => match print(&summary, &mut std::io::stderr().lock()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                ExitCode::from(EXIT_BROKEN_PIPE)
            }
            Err(_) => ExitCode::from(EXIT_IO),
        },
        Ok(summary) => {
            let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
            match print(&summary, &mut stdout).and_then(|()| stdout.flush()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => write_error(&error),
            }
        }
        // The reader closed the pipe: no message, as for the documents.
        Err(_) if broken_pipe => ExitCode::from(EXIT_BROKEN_PIPE),
        Err(failure) => {
            let code = if failure.is_io() {
                EXIT_IO
            } else if failure.is_strict_safe() {
                compat_cmd::EXIT_UNSUPPORTED
            } else {
                EXIT_REJECTED
            };
            let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
            ExitCode::from(code)
        }
    }
}

fn run_check(args: &CheckArgs) -> ExitCode {
    let text = match read_policy(&args.policy) {
        Ok(text) => text,
        Err(message) => return usage(&message),
    };
    let policy = match Policy::from_json(&text) {
        Ok(policy) => policy,
        Err(error) => {
            return usage(&format!(
                "invalid policy {}: {error}",
                args.policy.display()
            ));
        }
    };
    let verdict = match open_remote(&args.file) {
        Ok(None) => check::run(&args.file, &policy),
        Ok(Some(mut source)) => {
            let size = source.len_known();
            check::run_source(BufferedSource::new(&mut source), size, &policy)
                .map_err(|f| source.explain(f))
        }
        Err(exit) => return exit,
    };
    let verdict = match verdict {
        Ok(verdict) => verdict,
        Err(failure) => {
            let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
            return ExitCode::from(EXIT_IO);
        }
    };
    // A verdict that did not reach its reader must not look like an accept.
    if let Err(exit) = print_document(&verdict.to_json()) {
        return exit;
    }
    if verdict.accepted() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_REJECTED)
    }
}

/// Reads the policy file, refusing one larger than the policy limit
/// without reading it whole.
fn read_policy(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let limit = coreav_policy::MAX_POLICY_BYTES;
    let file = std::fs::File::open(path)
        .map_err(|e| format!("cannot read policy {}: {e}", path.display()))?;
    let mut text = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut text)
        .map_err(|e| format!("cannot read policy {}: {e}", path.display()))?;
    if text.len() > limit {
        return Err(format!(
            "policy {} is larger than {limit} bytes",
            path.display()
        ));
    }
    Ok(text)
}

/// A failure's error object on stderr and its exit code.
fn fail_clip(failure: &clip::ClipFailure) -> ExitCode {
    let code = if failure.is_io() {
        EXIT_IO
    } else {
        EXIT_REJECTED
    };
    let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
    ExitCode::from(code)
}

fn io_failure(error: std::io::Error) -> ExitCode {
    let failure = coreav_cli::error::Failure::io(error);
    let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
    ExitCode::from(EXIT_IO)
}

fn hls_options(args: &HlsTrackArgs) -> Result<(HlsOptions, coreav_isobmff::Allowlist), ExitCode> {
    let allowlist = probe::allowlist(args.allow.as_deref(), args.strict).map_err(|unknown| {
        usage(&format!(
            "unknown codec '{unknown}' in --allow; known: {}",
            names::ALLOW_NAMES.join(", ")
        ))
    })?;
    let segment_duration = clip::parse_time(&args.segment_duration)
        .filter(|d| d.num > 0)
        .ok_or_else(|| usage("--segment-duration must be decimal seconds above 0"))?;
    let video = match args.video.as_deref() {
        None => Some(VideoChoice::First),
        Some("none") => None,
        Some(index) => {
            Some(VideoChoice::Stream(index.parse().map_err(|_| {
                usage("--video must be a stream index or `none`")
            })?))
        }
    };
    let audio = match args.audio.as_deref() {
        None => AudioChoice::First,
        Some("none") => AudioChoice::None,
        Some(index) => AudioChoice::Stream(
            index
                .parse()
                .map_err(|_| usage("--audio must be a stream index or `none`"))?,
        ),
    };
    if video.is_none() && audio == AudioChoice::None {
        return Err(usage(
            "--video none and --audio none leave nothing to package",
        ));
    }
    if args.tag.len() > coreav_isobmff_write::hls::MAX_SOURCE_TAG {
        return Err(usage("--tag is longer than 255 bytes"));
    }
    let options = HlsOptions {
        video,
        audio,
        segment_duration,
        source_tag: args.tag.as_bytes().to_vec(),
    };
    Ok((options, allowlist))
}

/// Opens `path` for writing: a new file, or any file with `force`.
fn create(path: &std::path::Path, force: bool) -> Result<std::fs::File, ExitCode> {
    let mut open = std::fs::OpenOptions::new();
    open.write(true);
    if force {
        open.create(true).truncate(true);
    } else {
        open.create_new(true);
    }
    match open.open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(usage(&format!(
            "{} exists; pass --force to overwrite it",
            path.display()
        ))),
        Err(error) => Err(io_failure(error)),
    }
}

/// Prints the index's summary; a failed write is the command's exit
/// ([`write_error`]).
fn print_summary(index: &coreav_isobmff_write::hls::HlsIndex) -> ExitCode {
    match print_document(&hls::document(index)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => exit,
    }
}

/// Writes `path` through a new temporary file next to it, moved into place
/// only when `write` succeeds, so a failure never leaves a partial file or
/// destroys an existing one. Without `force` the move is a hard link, which
/// fails rather than replace a file that appeared meanwhile.
fn write_atomically(
    path: &Path,
    force: bool,
    write: impl FnOnce(std::fs::File) -> Result<(), ExitCode>,
) -> ExitCode {
    let exists = || {
        usage(&format!(
            "{} exists; pass --force to overwrite it",
            path.display()
        ))
    };
    if !force && path.exists() {
        return exists();
    }
    // `.{name}.coreav-{pid}.part`, kept within the file name limit.
    let temp = output::temporary_path(path);
    let file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
    {
        Ok(file) => file,
        Err(error) => return io_failure(error),
    };
    if let Err(code) = write(file) {
        let _ = std::fs::remove_file(&temp);
        return code;
    }
    match output::place(&temp, path, force) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                exists()
            } else {
                io_failure(error)
            }
        }
    }
}

fn run_hls_index(args: &HlsIndexArgs) -> ExitCode {
    let (options, allowlist) = match hls_options(&args.tracks) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    // The output path is checked before the index is built, as
    // `write_output` checks it before the other commands do their work.
    if args.index != "-" {
        if let Some(message) = output::refuse(&args.input, Path::new(&args.index), args.force) {
            return usage(&message);
        }
    }
    let bytes = match hls::index(&args.input, &options, &allowlist) {
        Ok(bytes) => bytes,
        Err(failure) => return fail_clip(&failure),
    };
    let index = match HlsIndex::open(
        coreav_isobmff::SliceSource::new(&bytes),
        &Default::default(),
    ) {
        Ok(index) => index,
        Err(error) => return fail_clip(&error.into()),
    };
    write_output(&args.input, &args.index, args.force, |out| {
        out.write_all(&bytes)
            .map_err(|error| WriteFailure::output(&error))?;
        Ok(hls::document(&index))
    })
}

fn run_hls_get(args: &HlsGetArgs) -> ExitCode {
    let Ok(resource) = args.name.parse::<Resource>() else {
        return usage(&format!(
            "unknown resource '{}'; expected master.m3u8, video.m3u8, audio.m3u8, \
             video-init.mp4, audio-init.mp4, video-N.m4s or audio-N.m4s",
            args.name
        ));
    };
    let to_stdout = args.output == "-";
    let path = PathBuf::from(&args.output);
    if !to_stdout
        && (output::same_file(&path, &args.input) || output::same_file(&path, &args.index))
    {
        return usage("OUTPUT is the INPUT or the INDEX file");
    }
    let index = match hls::open(&args.index) {
        Ok(index) => index,
        Err(failure) => return fail_clip(&failure),
    };
    // Everything that can fail before writing, before the output is opened.
    let prepared = match hls::prepare(&args.index, &index, &args.input, resource) {
        Ok(prepared) => prepared,
        Err(failure) => return fail_clip(&failure),
    };
    if to_stdout {
        let mut out = Watched {
            inner: media_stdout(),
            broken_pipe: false,
        };
        return match hls::emit(prepared, &mut out) {
            Ok(_) => ExitCode::SUCCESS,
            // The reader closed the pipe: no message, as for the documents.
            Err(_) if out.broken_pipe => ExitCode::from(EXIT_BROKEN_PIPE),
            Err(failure) => fail_clip(&failure),
        };
    }
    // A plain file, so that segment ranges are copied by the kernel.
    write_atomically(&path, args.force, |file| {
        hls::emit(prepared, file)
            .map(|_| ())
            .map_err(|f| fail_clip(&f))
    })
}

fn run_hls_package(args: &HlsPackageArgs) -> ExitCode {
    // `-` means standard output to every other writing command; a package
    // is a playlist and many segments, which it cannot hold (`./-` names a
    // directory called `-`).
    if args.dir.as_os_str() == "-" {
        return usage("the output must be a directory: a package is several files, not a stream");
    }
    let (options, allowlist) = match hls_options(&args.tracks) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };
    let existing = match std::fs::read_dir(&args.dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return usage(&format!("{} is not empty", args.dir.display()));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return io_failure(error),
    };
    // Built before anything is written: a rejected input leaves nothing.
    let bytes = match hls::index(&args.input, &options, &allowlist) {
        Ok(bytes) => bytes,
        Err(failure) => return fail_clip(&failure),
    };
    if !existing {
        if let Err(error) = std::fs::create_dir(&args.dir) {
            return io_failure(error);
        }
    }
    // Written in place; on failure the files written are removed, and the
    // directory too when this command created it.
    let mut written: Vec<PathBuf> = Vec::new();
    let mut result = (|| -> Result<HlsIndex, ExitCode> {
        let index_path = args.dir.join(hls::INDEX_NAME);
        let mut file = create(&index_path, false)?;
        written.push(index_path.clone());
        file.write_all(&bytes).map_err(io_failure)?;
        let index = hls::open(&index_path).map_err(|f| fail_clip(&f))?;
        for resource in hls::resources(&index) {
            let prepared = hls::prepare(&index_path, &index, &args.input, resource)
                .map_err(|f| fail_clip(&f))?;
            let path = args.dir.join(resource.to_string());
            let file = create(&path, false)?;
            written.push(path);
            hls::emit(prepared, file).map_err(|f| fail_clip(&f))?;
        }
        Ok(index)
    })();
    if result.is_err() {
        for path in &written {
            let _ = std::fs::remove_file(path);
        }
        if !existing {
            let _ = std::fs::remove_dir(&args.dir);
        }
    }
    match result.as_mut() {
        Ok(index) => print_summary(index),
        Err(code) => *code,
    }
}

/// What a probe read: an ISO base media file, a Matroska one or a
/// transport stream.
enum Probed {
    Iso(probe::Probed),
    Matroska(matroska::Probed),
    Mpegts(mpegts::Probed),
}

/// The allowlists of each container that `coreav probe` reads.
struct Allowlists {
    iso: coreav_isobmff::Allowlist,
    mkv: coreav_mkv::Allowlist,
    ts: coreav_mpegts::Allowlist,
}

/// Probes a file as the command always has. The container is told from the
/// file itself: the EBML header of Matroska, then ISO base media (its
/// first box names one of thirteen types), and a file that is neither is
/// offered to the transport stream reader, which finds its own packet size
/// and start. Transport streams come last because they have no header to
/// recognise: a stream starts wherever its sync byte begins to repeat at a
/// packet stride.
fn probe_file(
    path: &Path,
    lists: &Allowlists,
    scan: bool,
    frame_sizes: bool,
) -> Result<Probed, Failure> {
    if matroska::detect(path)? {
        return matroska::run_with(path, &lists.mkv, frame_sizes).map(Probed::Matroska);
    }
    match probe::run_with(path, &lists.iso, scan, frame_sizes) {
        Err(failure) if not_isobmff(&failure) => {
            or_isobmff_error(mpegts::run_with(path, &lists.ts, frame_sizes).map(Probed::Mpegts))
        }
        other => other.map(Probed::Iso),
    }
}

/// Whether the ISO base media reader did not recognise the file, so that
/// another container may read it.
fn not_isobmff(failure: &Failure) -> bool {
    matches!(failure, Failure::Library(coreav_isobmff::Error::NotIsobmff))
}

/// A file that neither reader recognised is reported as the ISO base media
/// reader reports it, the command's answer for a container it does not
/// read; any other transport stream rejection is its own.
fn or_isobmff_error(result: Result<Probed, Failure>) -> Result<Probed, Failure> {
    match result {
        Err(Failure::Mpegts(coreav_mpegts::Error::NotTransportStream)) => {
            Err(Failure::Library(coreav_isobmff::Error::NotIsobmff))
        }
        other => other,
    }
}

fn run_probe(args: &ProbeArgs) -> ExitCode {
    // Each container's list keeps the names it knows: `--allow vp8,h264`
    // allows H.264 in an MP4 file and both in a WebM file.
    let lists = match probe::allowlists(args.allow.as_deref(), args.strict) {
        Ok((iso, mkv, ts)) => Allowlists { iso, mkv, ts },
        Err(unknown) => {
            let mut known = names::ALLOW_NAMES.to_vec();
            for name in matroska::allow_names()
                .into_iter()
                .chain(mpegts::allow_names())
            {
                if !known.contains(&name) {
                    known.push(name);
                }
            }
            let _ = writeln!(
                std::io::stderr(),
                "error: unknown codec '{unknown}' in --allow; known: {}",
                known.join(", ")
            );
            return ExitCode::from(EXIT_USAGE);
        }
    };
    let scan = if args.ffprobe_compat {
        !args.no_scan_fragments
    } else {
        args.scan_fragments
    };
    let name = args.file.to_string_lossy();
    let mut remote = match open_remote(&args.file) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    let probed = match remote.as_mut() {
        None => probe_file(&args.file, &lists, scan, args.scan_frame_sizes),
        Some(source) => {
            let size = source.len_known();
            let mut buffered = BufferedSource::new(&mut *source);
            let probed = (|| {
                if matroska::detect_source(&mut buffered)? {
                    return matroska::run_source_with(
                        buffered,
                        size,
                        &lists.mkv,
                        args.scan_frame_sizes,
                    )
                    .map(Probed::Matroska);
                }
                // As `probe_file`: ISO base media, then transport streams.
                match probe::run_source_with(
                    &mut buffered,
                    size,
                    &lists.iso,
                    scan,
                    args.scan_frame_sizes,
                ) {
                    Err(failure)
                        if not_isobmff(&failure) && mpegts::detect_source(&mut buffered)? =>
                    {
                        mpegts::run_source_with(
                            &mut buffered,
                            size,
                            &lists.ts,
                            args.scan_frame_sizes,
                        )
                        .map(Probed::Mpegts)
                    }
                    other => other.map(Probed::Iso),
                }
            })();
            probed.map_err(|f| source.explain(f))
        }
    };
    let document = match probed {
        Ok(Probed::Matroska(probed)) if args.ffprobe_compat => {
            matroska::compat_document(&probed, &name)
        }
        Ok(Probed::Matroska(probed)) => matroska::document(&probed),
        Ok(Probed::Mpegts(probed)) if args.ffprobe_compat => {
            mpegts::compat_document(&probed, &name)
        }
        Ok(Probed::Mpegts(probed)) => mpegts::document(&probed),
        Ok(Probed::Iso(result)) if args.ffprobe_compat => compat::document(&result, &name),
        Ok(Probed::Iso(result)) => native::document(&result),
        Err(failure) => return fail(&failure),
    };
    match print_document(&document) {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => exit,
    }
}

/// Without the `net` feature, as the WASI build of the npm package is built
/// (WASI has no sockets): no input is read as a URL, so no `RemoteSource`
/// exists.
#[cfg(not(feature = "net"))]
mod offline {
    use coreav_cli::error::Failure;
    use coreav_isobmff::{Error, Source};

    /// A URL source, which this build never opens.
    pub enum RemoteSource {}

    impl RemoteSource {
        pub fn failure(&self) -> Option<&str> {
            match *self {}
        }

        pub fn len_known(&self) -> u64 {
            match *self {}
        }

        pub fn explain(&self, _: Failure) -> Failure {
            match *self {}
        }
    }

    impl Source for RemoteSource {
        fn len(&mut self) -> Result<u64, Error> {
            match *self {}
        }

        fn read_exact_at(&mut self, _: u64, _: &mut [u8]) -> Result<(), Error> {
            match *self {}
        }
    }
}

/// Without the `net` feature an input that names a URL (`scheme://...`,
/// the rule of `remote::is_url`) is a usage error, not a file name that is
/// not found.
#[cfg(not(feature = "net"))]
fn open_remote(input: &Path) -> Result<Option<RemoteSource>, ExitCode> {
    let text = input.to_string_lossy();
    let url = text.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if url {
        return Err(usage(
            "this build of coreav reads local files only, not URLs (the WebAssembly build has no network)",
        ));
    }
    Ok(None)
}

/// The input opened as a URL read by byte ranges, or `None` for a file.
#[cfg(feature = "net")]
fn open_remote(input: &Path) -> Result<Option<RemoteSource>, ExitCode> {
    let Some(url) = input.to_str().filter(|text| remote::is_url(text)) else {
        return Ok(None);
    };
    match remote::open(url, &remote::Options::from_env()) {
        Ok(source) => Ok(Some(source)),
        Err(failure) => Err(fail(&failure)),
    }
}

/// A write's failure with the remote reason, when a read of `source` failed.
fn explain_write(source: &RemoteSource, failure: WriteFailure) -> WriteFailure {
    match source.failure() {
        Some(reason) if failure.is_io() => {
            WriteFailure::Io(Failure::Remote(reason.to_owned()).to_json())
        }
        _ => failure,
    }
}

fn fail(failure: &Failure) -> ExitCode {
    let code = if failure.is_io() {
        EXIT_IO
    } else {
        EXIT_REJECTED
    };
    let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
    ExitCode::from(code)
}

fn run_split(args: &SplitArgs) -> ExitCode {
    let allowlist = match probe::allowlist(args.allow.as_deref(), args.strict) {
        Ok(allowlist) => allowlist,
        Err(unknown) => {
            return usage(&format!(
                "unknown codec '{unknown}' in --allow; known: {}",
                names::ALLOW_NAMES.join(", ")
            ));
        }
    };
    let at = match (&args.every, &args.at) {
        (Some(every), None) => match clip::parse_time(every) {
            Some(step) if step.num > 0 => SplitAt::Every(step),
            _ => return usage("--every must be decimal seconds above 0, such as 10 or 2.5"),
        },
        (None, Some(times)) => match split::parse_times(times) {
            Some(times)
                if times
                    .windows(2)
                    .all(|w| matches!(w, [a, b] if a.checked_lt(*b) == Some(true))) =>
            {
                SplitAt::Times(times)
            }
            Some(_) => return usage("--at times must increase"),
            None => {
                return usage("--at must be decimal seconds separated by commas, such as 10,25.5");
            }
        },
        _ => return usage("give --every or --at"),
    };
    let audio = match args.audio.as_deref() {
        None => AudioChoice::First,
        Some("none") => AudioChoice::None,
        Some(index) => match index.parse() {
            Ok(index) => AudioChoice::Stream(index),
            Err(_) => return usage("--audio must be a stream index or `none`"),
        },
    };
    let Some(pattern) = split::Pattern::parse(&args.output) else {
        return usage("OUTPUT must hold one %d, or %0Nd for N digits (1 to 9), and no other %");
    };
    let options = SplitOptions {
        video: args.video.map_or(VideoChoice::First, VideoChoice::Stream),
        audio,
        at,
    };
    // Each segment is written to a temporary file and every one is renamed
    // to its name only once the whole split succeeds, so a failed split
    // never touches an existing file and leaves no segment behind. Only a
    // failed rename (the temporary files sit next to their names) leaves
    // the segments renamed before it in place.
    let mut written: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut refused = None;
    let mut remote = match open_remote(&args.input) {
        Ok(remote) => remote,
        Err(exit) => return exit,
    };
    let open = |index: usize| {
        let name = PathBuf::from(pattern.name(index));
        if let Some(message) = output::refuse(&args.input, &name, args.force) {
            refused = Some(message);
            return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
        }
        let temporary = output::tagged_temporary_path(&name, &format!("-{index}"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        written.push((temporary, name));
        Ok(std::io::BufWriter::new(file))
    };
    let result = match remote.as_mut() {
        None => split::run(&args.input, &options, &allowlist, open),
        Some(source) => split::run_source(
            BufferedSource::new(&mut *source),
            &options,
            &allowlist,
            open,
        )
        .map_err(|f| explain_write(source, f)),
    };
    let summary = match result {
        Ok(summary) => summary,
        Err(failure) => {
            for (temporary, _) in &written {
                let _ = std::fs::remove_file(temporary);
            }
            if let Some(message) = refused {
                return usage(&message);
            }
            let code = if failure.is_io() {
                EXIT_IO
            } else {
                EXIT_REJECTED
            };
            let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
            return ExitCode::from(code);
        }
    };
    for (n, (temporary, name)) in written.iter().enumerate() {
        if let Err(error) = output::place(temporary, name, args.force) {
            for (rest, _) in written.iter().skip(n) {
                let _ = std::fs::remove_file(rest);
            }
            let failure = output::WriteFailure::output(&error);
            let _ = writeln!(std::io::stderr(), "{}", failure.to_json());
            return ExitCode::from(EXIT_IO);
        }
    }
    let names: Vec<String> = written
        .iter()
        .map(|(_, name)| name.to_string_lossy().into_owned())
        .collect();
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    match split::write_document(&mut stdout, &summary, &names).and_then(|()| stdout.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => write_error(&error),
    }
}

fn run_join(args: &JoinArgs) -> ExitCode {
    let allowlist = match probe::allowlist(args.allow.as_deref(), args.strict) {
        Ok(allowlist) => allowlist,
        Err(unknown) => {
            return usage(&format!(
                "unknown codec '{unknown}' in --allow; known: {}",
                names::ALLOW_NAMES.join(", ")
            ));
        }
    };
    let audio = match args.audio.as_deref() {
        None => JoinAudio::First,
        Some("none") => JoinAudio::None,
        Some(_) => return usage("--audio takes only `none`"),
    };
    let options = JoinOptions { audio };
    // The output may name none of the inputs; `write_output` checks the
    // first, and whether the output exists.
    if args.output != "-" {
        let path = PathBuf::from(&args.output);
        if let Some(message) = args
            .inputs
            .iter()
            .find_map(|input| output::refuse(input, &path, true))
        {
            return usage(&message);
        }
    }
    let first = args.inputs.first().map_or(Path::new(""), PathBuf::as_path);
    write_output_with(
        first,
        &args.output,
        args.force,
        |out| join::run(&args.inputs, &options, &allowlist, out).map_err(WriteFailure::from),
        |summary, out| join::write_document(out, summary, &args.inputs),
    )
}
