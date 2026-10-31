//! The operations, independent of the target: options in, a JSON document
//! out, the output written front to back to any [`Write`].
//!
//! The WebAssembly bindings call these with a source and a sink backed by
//! JavaScript; the native build calls them with files and buffers, which is
//! how tests and the measurement's reference engine get the native output of
//! the same code. Documents and error objects come from `coreav-cli`, so each
//! operation here prints what the matching `coreav` command prints.

use coreav_cli::clip::{ClipFailure, parse_time};
use coreav_cli::error::Failure as CliFailure;
use coreav_cli::output::WriteFailure;
use coreav_cli::{check, compat, extract, matroska, names, native, remux, text};
use coreav_isobmff::{Allowlist, Limits, Rational, SliceSource, Source};
use coreav_isobmff_write::hls::{HlsIndex, HlsOptions, MAX_SOURCE_TAG, Resource, build_index};
use coreav_isobmff_write::{
    AudioChoice, ClipOptions, ExtractError, MatroskaRemuxOptions, Metadata, OnUnsupported,
    RemuxOptions, TrackChoice, VideoChoice, clip_mp4, extract_audio, remux_matroska, remux_mp4,
};
use coreav_policy::Policy;
use coreav_text::{TextLimits, extract_subtitles, read_chapters};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::Write;

/// Bytes of output the module hands to JavaScript at a time. Each chunk
/// becomes a new `Uint8Array` in the output stream; fewer, larger chunks
/// cost fewer stream operations, and 256 KiB keeps the copy cheap.
pub const WRITE_CHUNK: usize = 256 << 10;

/// The operations, by the name the bindings receive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// `probe`: the `coreav.probe/1` document (or `coreav.probe-matroska/1`
    /// for Matroska and WebM), or the `--ffprobe-compat` shape.
    Probe,
    /// `check`: the `coreav.verdict/1` verdict of a `coreav.policy/1`
    /// policy (`coreav check`).
    Check,
    /// `chapters`: the `coreav.chapters/1` chapter list.
    Chapters,
    /// `extractAudio`: one AAC track to M4A
    /// (`docs/design/extract-audio-cli.md`).
    ExtractAudio,
    /// `clip`: a keyframe-aligned clip to MP4 (`docs/schema/clip-v1.md`).
    Clip,
    /// `hlsIndex`: the HLS index of a file (`docs/design/hls.md`), written
    /// as the output; the summary is `coreav.hls-index/2`.
    HlsIndex,
    /// `hlsGet`: one HLS resource cut from an index ([`run_indexed`]).
    HlsGet,
    /// `extractSubtitles`: a 3GPP timed text track to WebVTT.
    ExtractSubtitles,
    /// `remux`: MP4/MOV, Matroska or WebM to MP4 with the movie box first.
    Remux,
    /// `sanitize`: a remux that keeps only allowlisted metadata.
    Sanitize,
}

impl Operation {
    /// The operation called `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "probe" => Some(Self::Probe),
            "check" => Some(Self::Check),
            "chapters" => Some(Self::Chapters),
            "extractAudio" => Some(Self::ExtractAudio),
            "clip" => Some(Self::Clip),
            "hlsIndex" => Some(Self::HlsIndex),
            "hlsGet" => Some(Self::HlsGet),
            "extractSubtitles" => Some(Self::ExtractSubtitles),
            "remux" => Some(Self::Remux),
            "sanitize" => Some(Self::Sanitize),
            _ => None,
        }
    }

    /// Whether the operation writes an output stream.
    pub fn writes_output(self) -> bool {
        !matches!(self, Self::Probe | Self::Check | Self::Chapters)
    }
}

/// Why an operation produced no document.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The options are invalid; nothing was read. The package throws a
    /// `TypeError` with the message.
    Usage(String),
    /// The `coreav.error/1` object, as `coreav` prints it on stderr.
    Error(Value),
}

impl Failure {
    /// The JSON text the bindings reject with: `{"usage": message}` or the
    /// error object.
    pub fn to_json(&self) -> String {
        match self {
            Self::Usage(message) => json!({ "usage": message }).to_string(),
            Self::Error(value) => value.to_string(),
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }
}

impl From<CliFailure> for Failure {
    fn from(failure: CliFailure) -> Self {
        Self::Error(failure.to_json())
    }
}

/// Options every operation takes. Each operation's options repeat these
/// fields rather than flattening them, because `serde` cannot combine
/// `flatten` with `deny_unknown_fields`.
#[derive(Clone, Copy, Debug)]
struct Common<'a> {
    /// Codec names to accept, as `--allow`.
    allow: Option<&'a [String]>,
    /// As `--strict`.
    strict: bool,
    /// Overrides of single limits.
    limits: Option<LimitOverrides>,
}

/// The fields of [`Common`], for each options struct.
macro_rules! common {
    ($options:expr) => {
        Common {
            allow: $options.allow.as_deref(),
            strict: $options.strict,
            limits: $options.limits,
        }
    };
}

/// Overrides of [`Limits`] fields; absent fields keep [`edge_limits`].
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LimitOverrides {
    max_top_level_boxes: Option<u32>,
    max_depth: Option<u32>,
    max_total_boxes: Option<u32>,
    max_box_bytes: Option<u64>,
    max_retained_bytes: Option<u64>,
    max_table_bytes: Option<u64>,
    max_packet_bytes: Option<u64>,
    max_sample_work: Option<u64>,
    max_fragments: Option<u32>,
    max_tracks: Option<u32>,
    max_movie_bytes: Option<u64>,
    max_movie_ratio: Option<u32>,
}

/// The library's limits, which the native `coreav` command
/// uses too, with `max_table_bytes` lowered to [`EDGE_TABLE_BYTES`] so that
/// one operation's tables fit 128 MB of memory beside the reader's buffers. A
/// caller raises or lowers any of them through the `limits` option.
pub fn edge_limits() -> Limits {
    Limits {
        max_table_bytes: EDGE_TABLE_BYTES,
        ..Limits::default()
    }
}

/// The WebAssembly build's `max_table_bytes`: 64 MiB, where the library's is
/// 256 MiB. A Matroska remux charges about 128 bytes per sample against it,
/// and its module memory stays below the charge; the largest held-out remux
/// charged 60.7 MiB and used 57.4 MiB.
pub const EDGE_TABLE_BYTES: u64 = 64 << 20;

/// [`edge_limits`] with a caller's overrides, for the operations that take
/// no allowlist (`chapters`, `extractSubtitles`).
fn edge_limits_with(overrides: Option<LimitOverrides>) -> Limits {
    Common {
        allow: None,
        strict: false,
        limits: overrides,
    }
    .limits()
}

impl Common<'_> {
    fn allowlist(&self) -> Result<Allowlist, Failure> {
        coreav_cli::probe::allowlist(self.allow, self.strict).map_err(|unknown| {
            Failure::usage(format!(
                "unknown codec '{unknown}' in allow; known: {}",
                names::ALLOW_NAMES.join(", ")
            ))
        })
    }

    /// The ISO base media and Matroska allowlists, as `coreav probe` and
    /// `coreav remux` build them: each container keeps the names it knows.
    fn allowlists(&self) -> Result<(Allowlist, coreav_mkv::Allowlist), Failure> {
        matroska::allowlists(self.allow, self.strict).map_err(|unknown| {
            let mut known = names::ALLOW_NAMES.to_vec();
            for name in matroska::allow_names() {
                if !known.contains(&name) {
                    known.push(name);
                }
            }
            Failure::usage(format!(
                "unknown codec '{unknown}' in allow; known: {}",
                known.join(", ")
            ))
        })
    }

    fn limits(&self) -> Limits {
        let mut limits = edge_limits();
        let Some(o) = self.limits else {
            return limits;
        };
        let set32 = |field: &mut u32, value: Option<u32>| *field = value.unwrap_or(*field);
        let set64 = |field: &mut u64, value: Option<u64>| *field = value.unwrap_or(*field);
        set32(&mut limits.max_top_level_boxes, o.max_top_level_boxes);
        set32(&mut limits.max_depth, o.max_depth);
        set32(&mut limits.max_total_boxes, o.max_total_boxes);
        set64(&mut limits.max_box_bytes, o.max_box_bytes);
        set64(&mut limits.max_retained_bytes, o.max_retained_bytes);
        set64(&mut limits.max_table_bytes, o.max_table_bytes);
        set64(&mut limits.max_packet_bytes, o.max_packet_bytes);
        set64(&mut limits.max_sample_work, o.max_sample_work);
        set32(&mut limits.max_fragments, o.max_fragments);
        set32(&mut limits.max_tracks, o.max_tracks);
        set64(&mut limits.max_movie_bytes, o.max_movie_bytes);
        set32(&mut limits.max_movie_ratio, o.max_movie_ratio);
        limits
    }
}

/// `probe` options.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProbeOptions {
    allow: Option<Vec<String>>,
    #[serde(default)]
    strict: bool,
    limits: Option<LimitOverrides>,
    /// `native` (default) or `ffprobe-compat`.
    format: Option<String>,
    /// As `--scan-fragments` and `--no-scan-fragments`; the default follows
    /// the format, as in the command.
    scan_fragments: Option<bool>,
    /// As `--scan-frame-sizes`.
    #[serde(default)]
    scan_frame_sizes: bool,
    /// The `format.filename` of the compat shape.
    filename: Option<String>,
}

/// `extractAudio` options.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExtractOptions {
    allow: Option<Vec<String>>,
    #[serde(default)]
    strict: bool,
    limits: Option<LimitOverrides>,
    /// The stream index of the track; the first audio track when absent.
    stream: Option<usize>,
}

/// `clip` options, as the flags of `coreav clip`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClipArgs {
    allow: Option<Vec<String>>,
    #[serde(default)]
    strict: bool,
    limits: Option<LimitOverrides>,
    /// Decimal seconds.
    start: Option<String>,
    /// Decimal seconds; the end of the video when absent.
    end: Option<String>,
    /// The video stream index.
    video: Option<usize>,
    /// The audio stream index, or `none`.
    audio: Option<AudioArg>,
}

/// `check` options: the `coreav.policy/1` policy, as a JSON object or as
/// its JSON text (read as `coreav check` reads a policy file: within its
/// size limit, every integer exact), and limits for MP4 and MOV inputs.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CheckOptions {
    policy: Option<Value>,
    limits: Option<LimitOverrides>,
}

/// `chapters` options.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChapterOptions {
    limits: Option<LimitOverrides>,
}

/// `extractSubtitles` options, as `coreav extract-subtitles`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubtitleOptions {
    /// The stream index; the first text stream that is not a chapter track
    /// when absent.
    stream: Option<usize>,
    limits: Option<LimitOverrides>,
}

/// `remux` and `sanitize` options, as the flags of `coreav remux` and
/// `coreav sanitize`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemuxArgs {
    allow: Option<Vec<String>>,
    #[serde(default)]
    strict: bool,
    limits: Option<LimitOverrides>,
    /// Stream indexes to keep.
    streams: Option<Vec<usize>>,
    /// Fail instead of dropping a stream MP4 cannot carry.
    #[serde(default)]
    fail_on_unsupported: bool,
    /// Matroska and WebM: also drop a video stream MP4 cannot carry.
    #[serde(default)]
    drop_unsupported: bool,
    /// `sanitize` only: what to keep (`rotation`, `language`, `dates`,
    /// `gapless`, `kinds`, or `none`).
    keep: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum AudioArg {
    Stream(usize),
    Name(String),
}

/// `hlsIndex` options, as the flags of `coreav hls index`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HlsIndexArgs {
    allow: Option<Vec<String>>,
    #[serde(default)]
    strict: bool,
    limits: Option<LimitOverrides>,
    /// Target segment duration, decimal seconds; 6 when absent.
    segment_duration: Option<String>,
    /// The video stream index, or `none`; the first video stream when
    /// absent.
    video: Option<AudioArg>,
    /// The audio stream index, or `none`; the first audio stream when
    /// absent.
    audio: Option<AudioArg>,
    /// Text stored in the index to identify the source's version (its
    /// entity tag, say), at most 255 bytes.
    #[serde(default)]
    tag: String,
}

/// `hlsGet` options.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HlsGetArgs {
    limits: Option<LimitOverrides>,
    /// The resource: `master.m3u8`, `video.m3u8`, `video-init.mp4`,
    /// `video-3.m4s`, and the same for `audio`.
    name: Option<String>,
}

fn parse<'a, T: Deserialize<'a>>(options: &'a str) -> Result<T, Failure> {
    let text = if options.trim().is_empty() {
        "{}"
    } else {
        options
    };
    serde_json::from_str(text).map_err(|error| Failure::usage(format!("invalid options: {error}")))
}

/// Runs an operation that returns a document only.
pub fn run_document<S: Source>(name: &str, source: S, options: &str) -> Result<String, Failure> {
    let document = match Operation::from_name(name) {
        Some(Operation::Probe) => probe_document(source, &parse(options)?)?,
        Some(Operation::Check) => check_document(source, &parse(options)?)?,
        Some(Operation::Chapters) => chapters_document(source, &parse(options)?)?,
        Some(_) => return Err(Failure::usage(format!("{name} writes an output stream"))),
        None => return Err(Failure::usage(format!("unknown operation {name}"))),
    };
    Ok(document.to_string())
}

/// Runs an operation that writes an output stream to `out` and returns its
/// summary. `out` is flushed before the summary is returned.
pub fn run_stream<S: Source, W: Write>(
    name: &str,
    source: S,
    options: &str,
    out: W,
) -> Result<String, Failure> {
    let summary = match Operation::from_name(name) {
        Some(Operation::ExtractAudio) => extract_document(source, &parse(options)?, out)?,
        Some(Operation::Clip) => clip_document(source, &parse(options)?, out)?,
        Some(Operation::HlsIndex) => hls_index_document(source, &parse(options)?, out)?,
        Some(Operation::HlsGet) => {
            return Err(Failure::usage(format!("{name} needs an index")));
        }
        Some(Operation::ExtractSubtitles) => subtitles_document(source, &parse(options)?, out)?,
        Some(Operation::Remux) => remux_document(source, &parse(options)?, false, out)?,
        Some(Operation::Sanitize) => remux_document(source, &parse(options)?, true, out)?,
        Some(Operation::Probe | Operation::Check | Operation::Chapters) => {
            return Err(Failure::usage(format!("{name} writes no output stream")));
        }
        None => return Err(Failure::usage(format!("unknown operation {name}"))),
    };
    Ok(summary.to_string())
}

/// Runs an operation that reads an index as well as the source (`hlsGet`)
/// and writes an output stream to `out`; returns its summary, `{"bytes":
/// N}`. Playlists and init segments are cut from the index alone, without
/// reading `source`.
pub fn run_indexed<S: Source, W: Write>(
    name: &str,
    index: &[u8],
    source: S,
    options: &str,
    out: W,
) -> Result<String, Failure> {
    match Operation::from_name(name) {
        Some(Operation::HlsGet) => Ok(hls_get(index, source, &parse(options)?, out)?.to_string()),
        Some(_) => Err(Failure::usage(format!("{name} takes no index"))),
        None => Err(Failure::usage(format!("unknown operation {name}"))),
    }
}

/// Whether the source starts like a Matroska or WebM file, as `coreav`
/// decides it.
fn is_matroska<S: Source>(source: &mut S) -> Result<bool, Failure> {
    let size = source.len().map_err(CliFailure::library)?;
    let mut prefix = [0u8; 4];
    let length = usize::try_from(size.min(4)).map_err(|_| Failure::usage("size"))?;
    let head = prefix.get_mut(..length).unwrap_or_default();
    source.read_exact_at(0, head).map_err(CliFailure::library)?;
    Ok(coreav_mkv::sniff(prefix.get(..length).unwrap_or_default()))
}

fn probe_document<S: Source>(mut source: S, options: &ProbeOptions) -> Result<Value, Failure> {
    let compat = match options.format.as_deref() {
        None | Some("native") => false,
        Some("ffprobe-compat") => true,
        Some(other) => {
            return Err(Failure::usage(format!(
                "unknown format '{other}'; known: native, ffprobe-compat"
            )));
        }
    };
    let (allowlist, mkv_allowlist) = common!(options).allowlists()?;
    let limits = common!(options).limits();
    let filename = options.filename.as_deref().unwrap_or("-");
    let size = source.len().map_err(CliFailure::library)?;
    if is_matroska(&mut source)? {
        // As `coreav probe`, which probes Matroska with its library defaults
        // and scans its blocks for `--scan-frame-sizes`.
        let probed =
            matroska::run_source_with(&mut source, size, &mkv_allowlist, options.scan_frame_sizes)?;
        return Ok(if compat {
            matroska::compat_document(&probed, filename)
        } else {
            matroska::document(&probed)
        });
    }
    let scan = options.scan_fragments.unwrap_or(compat);
    let probed = coreav_cli::probe::run_source_limited(
        &mut source,
        size,
        &allowlist,
        scan,
        options.scan_frame_sizes,
        &limits,
    )?;
    Ok(if compat {
        compat::document(&probed, filename)
    } else {
        native::document(&probed)
    })
}

fn check_document<S: Source>(mut source: S, options: &CheckOptions) -> Result<Value, Failure> {
    let policy = options
        .policy
        .as_ref()
        .ok_or_else(|| Failure::usage("check needs a policy"))?;
    let policy = match policy {
        Value::String(text) => Policy::from_json(text.as_bytes()),
        value => Policy::from_value(value),
    }
    .map_err(|error| Failure::usage(format!("invalid policy: {error}")))?;
    let limits = Common {
        allow: None,
        strict: false,
        limits: options.limits,
    }
    .limits();
    let size = source.len().map_err(CliFailure::library)?;
    // A file the probe rejects gets a reject verdict; only a source that
    // cannot be read is an error, as `coreav check` exits 3.
    let verdict = check::run_source_limited(&mut source, size, &policy, &limits)?;
    Ok(verdict.to_json())
}

fn chapters_document<S: Source>(source: S, options: &ChapterOptions) -> Result<Value, Failure> {
    let limits = edge_limits_with(options.limits);
    let chapters = read_chapters(source, &limits, &TextLimits::default())
        .map_err(|error| Failure::Error(WriteFailure::from(error).to_json()))?;
    Ok(text::chapters_document(chapters.as_ref()))
}

fn subtitles_document<S: Source, W: Write>(
    source: S,
    options: &SubtitleOptions,
    mut out: W,
) -> Result<Value, Failure> {
    let choice = options.stream.map_or(
        coreav_text::TrackChoice::First,
        coreav_text::TrackChoice::Stream,
    );
    let to_failure = |failure: WriteFailure| Failure::Error(failure.to_json());
    let limits = edge_limits_with(options.limits);
    let summary = extract_subtitles(source, &limits, &TextLimits::default(), choice, &mut out)
        .map_err(|error| to_failure(WriteFailure::from(error)))?;
    out.flush()
        .map_err(|error| to_failure(WriteFailure::output(&error)))?;
    Ok(text::subtitles_document(&summary))
}

fn remux_document<S: Source, W: Write>(
    mut source: S,
    options: &RemuxArgs,
    sanitize: bool,
    mut out: W,
) -> Result<Value, Failure> {
    let (allowlist, mkv_allowlist) = common!(options).allowlists()?;
    let metadata = if sanitize {
        let keep = options.keep.clone().unwrap_or_else(|| {
            ["rotation", "language", "gapless", "kinds"]
                .map(str::to_owned)
                .to_vec()
        });
        let keep = remux::parse_keep(&keep).map_err(|unknown| {
            Failure::usage(format!(
                "unknown field '{unknown}' in keep; known: {}, or none",
                remux::KEEP_NAMES.join(", ")
            ))
        })?;
        Metadata::Sanitize(keep)
    } else {
        if options.keep.is_some() {
            return Err(Failure::usage("keep is a sanitize option"));
        }
        Metadata::Copy
    };
    if options.fail_on_unsupported && options.drop_unsupported {
        return Err(Failure::usage(
            "failOnUnsupported and dropUnsupported exclude each other",
        ));
    }
    let on_unsupported = if options.fail_on_unsupported {
        OnUnsupported::Fail
    } else {
        OnUnsupported::Drop
    };
    let limits = common!(options).limits();
    let to_failure = |failure: ClipFailure| Failure::Error(failure.to_json());
    let document = if is_matroska(&mut source)? {
        if sanitize {
            return Err(CliFailure::library(coreav_isobmff::Error::Unsupported {
                reason: "sanitizing reads MP4/MOV files; Matroska and WebM are not supported",
            })
            .into());
        }
        let mkv_options = MatroskaRemuxOptions {
            streams: options.streams.clone(),
            on_unsupported,
            drop_video: options.drop_unsupported,
        };
        let summary = remux_matroska(
            source,
            &coreav_mkv::Limits::default(),
            &limits,
            &mkv_allowlist,
            &mkv_options,
            &mut out,
        )
        .map_err(|error| to_failure(ClipFailure::from(error)))?;
        remux::matroska_document(&summary)
    } else {
        let remux_options = RemuxOptions {
            streams: options.streams.clone(),
            on_unsupported,
            metadata,
            left_out_covers: Vec::new(),
        };
        let summary = remux_mp4(source, &limits, &allowlist, &remux_options, &mut out)
            .map_err(|error| to_failure(ClipFailure::from(error)))?;
        remux::document(&summary, &remux_options)
    };
    out.flush()
        .map_err(|error| to_failure(WriteFailure::output(&error)))?;
    Ok(document)
}

/// The error object of a failed extraction or clip, as `coreav` prints it.
fn write_failure(error: ExtractError) -> Failure {
    Failure::Error(WriteFailure::from(error).to_json())
}

fn extract_document<S: Source, W: Write>(
    source: S,
    options: &ExtractOptions,
    mut out: W,
) -> Result<Value, Failure> {
    let allowlist = common!(options).allowlist()?;
    let choice = options
        .stream
        .map_or(TrackChoice::FirstAudio, TrackChoice::Stream);
    let summary = extract_audio(
        source,
        &common!(options).limits(),
        &allowlist,
        choice,
        &mut out,
    )
    .map_err(write_failure)?;
    out.flush()
        .map_err(|error| write_failure(ExtractError::from(error)))?;
    Ok(extract::document(&summary))
}

fn clip_document<S: Source, W: Write>(
    source: S,
    options: &ClipArgs,
    mut out: W,
) -> Result<Value, Failure> {
    let allowlist = common!(options).allowlist()?;
    let start = options
        .start
        .as_deref()
        .ok_or_else(|| Failure::usage("clip needs a start"))?;
    let start = parse_time(start)
        .ok_or_else(|| Failure::usage("start must be decimal seconds, such as 12 or 12.5"))?;
    let end = match options.end.as_deref() {
        None => None,
        Some(text) => Some(
            parse_time(text)
                .ok_or_else(|| Failure::usage("end must be decimal seconds, such as 12 or 12.5"))?,
        ),
    };
    let audio = match &options.audio {
        None => AudioChoice::First,
        Some(AudioArg::Stream(index)) => AudioChoice::Stream(*index),
        Some(AudioArg::Name(name)) if name == "none" => AudioChoice::None,
        Some(AudioArg::Name(_)) => {
            return Err(Failure::usage("audio must be a stream index or \"none\""));
        }
    };
    let clip = ClipOptions {
        video: options
            .video
            .map_or(VideoChoice::First, VideoChoice::Stream),
        audio,
        start,
        end,
    };
    let summary = clip_mp4(
        source,
        &common!(options).limits(),
        &allowlist,
        &clip,
        &mut out,
    )
    .map_err(write_failure)?;
    out.flush()
        .map_err(|error| write_failure(ExtractError::from(error)))?;
    Ok(coreav_cli::clip::document(&summary))
}

/// A stream choice of `hlsIndex`: the first stream when absent, none for
/// `"none"`.
fn stream_choice(arg: Option<&AudioArg>, name: &str) -> Result<Option<Option<usize>>, Failure> {
    match arg {
        None => Ok(Some(None)),
        Some(AudioArg::Stream(index)) => Ok(Some(Some(*index))),
        Some(AudioArg::Name(text)) if text == "none" => Ok(None),
        Some(AudioArg::Name(_)) => Err(Failure::usage(format!(
            "{name} must be a stream index or \"none\""
        ))),
    }
}

fn hls_index_document<S: Source, W: Write>(
    source: S,
    options: &HlsIndexArgs,
    mut out: W,
) -> Result<Value, Failure> {
    let allowlist = common!(options).allowlist()?;
    let limits = common!(options).limits();
    let segment_duration = match options.segment_duration.as_deref() {
        None => Rational::from_int(6),
        Some(text) => parse_time(text).filter(|d| d.num > 0).ok_or_else(|| {
            Failure::usage("segmentDuration must be decimal seconds above 0, such as 6 or 2.5")
        })?,
    };
    let video = stream_choice(options.video.as_ref(), "video")?
        .map(|stream| stream.map_or(VideoChoice::First, VideoChoice::Stream));
    let audio = match stream_choice(options.audio.as_ref(), "audio")? {
        None => AudioChoice::None,
        Some(None) => AudioChoice::First,
        Some(Some(index)) => AudioChoice::Stream(index),
    };
    if video.is_none() && audio == AudioChoice::None {
        return Err(Failure::usage(
            "video \"none\" and audio \"none\" leave nothing to package",
        ));
    }
    if options.tag.len() > MAX_SOURCE_TAG {
        return Err(Failure::usage("tag is longer than 255 bytes"));
    }
    let hls = HlsOptions {
        video,
        audio,
        segment_duration,
        source_tag: options.tag.as_bytes().to_vec(),
    };
    let bytes = build_index(source, &limits, &allowlist, &hls).map_err(write_failure)?;
    let index = HlsIndex::open(SliceSource::new(&bytes), &limits).map_err(write_failure)?;
    out.write_all(&bytes)
        .and_then(|()| out.flush())
        .map_err(|error| write_failure(ExtractError::from(error)))?;
    Ok(coreav_cli::hls::document(&index))
}

fn hls_get<S: Source, W: Write>(
    index_bytes: &[u8],
    mut source: S,
    options: &HlsGetArgs,
    mut out: W,
) -> Result<Value, Failure> {
    let name = options
        .name
        .as_deref()
        .ok_or_else(|| Failure::usage("hlsGet needs a name"))?;
    let resource: Resource = name.parse().map_err(|()| {
        Failure::usage(format!(
            "unknown resource '{name}'; expected master.m3u8, video.m3u8, audio.m3u8, \
             video-init.mp4, audio-init.mp4, video-N.m4s or audio-N.m4s"
        ))
    })?;
    let limits = HlsGetArgs::limits(options);
    let index = HlsIndex::open(SliceSource::new(index_bytes), &limits).map_err(write_failure)?;
    let missing = || write_failure(coreav_isobmff::Error::NoSuchTrack.into());
    let bytes = match resource {
        Resource::Master => Some(index.master_playlist().into_bytes()),
        Resource::Playlist(rendition) => Some(
            index
                .media_playlist(rendition)
                .ok_or_else(missing)?
                .into_bytes(),
        ),
        Resource::Init(rendition) => Some(index.init_segment(rendition).map_err(write_failure)?),
        Resource::Segment(..) => None,
    };
    let written = match (bytes, resource) {
        (Some(bytes), _) => {
            out.write_all(&bytes)
                .and_then(|()| out.flush())
                .map_err(|error| write_failure(ExtractError::from(error)))?;
            u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        }
        (None, Resource::Segment(rendition, number)) => {
            let segment = index
                .media_segment(
                    rendition,
                    number,
                    SliceSource::new(index_bytes),
                    &mut source,
                    &limits,
                )
                .map_err(write_failure)?;
            // Checked before anything is written, as `coreav hls get` does.
            let size = source.len().map_err(CliFailure::library)?;
            if size != segment.source_len() {
                return Err(Failure::Error(coreav_cli::hls::source_mismatch()));
            }
            // The extent the ranges span, so that a remote source starts
            // reading it at once (`Source::will_read`), when the ranges fill
            // at least half of it: an audio segment's samples lie between
            // the video's, and reading through all of it would fetch many
            // times the segment.
            if let (Some(first), Some(last)) = (segment.ranges().first(), segment.ranges().last()) {
                let extent = last
                    .offset
                    .saturating_add(last.len)
                    .saturating_sub(first.offset);
                let held = segment
                    .ranges()
                    .iter()
                    .fold(0u64, |sum, range| sum.saturating_add(range.len));
                if held.saturating_mul(2) >= extent {
                    source.will_read(first.offset, extent);
                }
            }
            segment
                .write(&mut source, &mut out)
                .map_err(write_failure)?
        }
        (None, _) => return Err(missing()),
    };
    Ok(json!({ "bytes": written }))
}

impl HlsGetArgs {
    fn limits(&self) -> Limits {
        Common {
            allow: None,
            strict: false,
            limits: self.limits,
        }
        .limits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_isobmff::SliceSource;

    fn usage(result: Result<String, Failure>) -> String {
        match result {
            Err(Failure::Usage(message)) => message,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn options_are_checked_before_reading() {
        let empty = SliceSource::new(b"");
        assert!(usage(run_document("probe", empty, r#"{"bogus": 1}"#)).contains("bogus"));
        assert!(usage(run_document("probe", empty, r#"{"allow": ["nope"]}"#)).contains("nope"));
        assert!(usage(run_document("probe", empty, r#"{"format": "xml"}"#)).contains("xml"));
        assert!(usage(run_document("clip", empty, "{}")).contains("output stream"));
        assert!(usage(run_document("transcode", empty, "{}")).contains("unknown"));
        let mut out = Vec::new();
        assert!(usage(run_stream("clip", empty, "{}", &mut out)).contains("start"));
        assert!(
            usage(run_stream("clip", empty, r#"{"start": "1e3"}"#, &mut out)).contains("start")
        );
        assert!(
            usage(run_stream(
                "clip",
                empty,
                r#"{"start": "1", "audio": "all"}"#,
                &mut out
            ))
            .contains("audio")
        );
        assert!(usage(run_stream("probe", empty, "{}", &mut out)).contains("no output"));
        assert!(out.is_empty());
    }

    #[test]
    fn hls_options_are_checked_before_reading() {
        let empty = SliceSource::new(b"");
        let mut out = Vec::new();
        let index =
            |options: &str, out: &mut Vec<u8>| usage(run_stream("hlsIndex", empty, options, out));
        assert!(index(r#"{"segmentDuration": "0"}"#, &mut out).contains("segmentDuration"));
        assert!(index(r#"{"segmentDuration": "-1"}"#, &mut out).contains("segmentDuration"));
        assert!(index(r#"{"video": "all"}"#, &mut out).contains("video"));
        assert!(index(r#"{"video": "none", "audio": "none"}"#, &mut out).contains("nothing"));
        let long = format!(r#"{{"tag": "{}"}}"#, "x".repeat(256));
        assert!(index(&long, &mut out).contains("tag"));
        assert!(index(r#"{"bogus": 1}"#, &mut out).contains("bogus"));
        assert!(usage(run_stream("hlsGet", empty, "{}", &mut out)).contains("index"));
        let get = |options: &str, out: &mut Vec<u8>| {
            usage(run_indexed("hlsGet", b"", empty, options, out))
        };
        assert!(get("{}", &mut out).contains("name"));
        assert!(get(r#"{"name": "video-01.m4s"}"#, &mut out).contains("unknown resource"));
        assert!(usage(run_indexed("clip", b"", empty, "{}", &mut out)).contains("no index"));
        assert!(out.is_empty());
    }

    #[test]
    fn a_damaged_index_is_an_error_object() {
        let mut out = Vec::new();
        let Err(Failure::Error(error)) = run_indexed(
            "hlsGet",
            b"CAVHLSIX\x00\x02",
            SliceSource::new(b""),
            r#"{"name": "master.m3u8"}"#,
            &mut out,
        ) else {
            panic!("expected a rejection");
        };
        assert_eq!(error["kind"], "invalid_index");
        assert!(out.is_empty());
    }

    #[test]
    fn rejections_are_error_objects() {
        let not_a_movie = SliceSource::new(b"RIFF\x24\x00\x00\x00AVI LIST");
        let Err(Failure::Error(error)) = run_document("probe", not_a_movie, "") else {
            panic!("expected a rejection");
        };
        assert_eq!(error["schema"], "coreav.error/1");
        assert_eq!(error["kind"], "unsupported_container");
        let mut out = Vec::new();
        let Err(Failure::Error(error)) = run_stream("extractAudio", not_a_movie, "{}", &mut out)
        else {
            panic!("expected a rejection");
        };
        assert_eq!(error["kind"], "unsupported_container");
        assert!(out.is_empty());
    }

    #[test]
    fn limit_overrides_apply_to_single_fields() {
        let options: ProbeOptions =
            parse(r#"{"limits": {"maxTableBytes": 1000, "maxTracks": 2}}"#).unwrap();
        let limits = common!(options).limits();
        assert_eq!(limits.max_table_bytes, 1000);
        assert_eq!(limits.max_tracks, 2);
        assert_eq!(limits.max_depth, edge_limits().max_depth);
        // Without overrides: the library's limits, tables lowered for 128 MB
        // of memory.
        let limits = common!(parse::<ProbeOptions>("{}").unwrap()).limits();
        assert_eq!(limits.max_table_bytes, EDGE_TABLE_BYTES);
        assert_eq!(limits.max_depth, Limits::default().max_depth);
    }

    #[test]
    fn chapters_and_subtitles_take_the_edge_table_bound() {
        // Both read sample tables, so neither may fall back to the
        // library's 256 MiB (`edge_limits`).
        let options: ChapterOptions = parse("{}").unwrap();
        assert_eq!(
            edge_limits_with(options.limits).max_table_bytes,
            EDGE_TABLE_BYTES
        );
        let options: SubtitleOptions = parse(r#"{"stream": 1}"#).unwrap();
        assert_eq!(
            edge_limits_with(options.limits).max_table_bytes,
            EDGE_TABLE_BYTES
        );
        // And a caller may lower it further.
        let options: ChapterOptions = parse(r#"{"limits": {"maxTableBytes": 1000}}"#).unwrap();
        assert_eq!(edge_limits_with(options.limits).max_table_bytes, 1000);
    }

    #[test]
    fn compressed_movie_limits_can_be_lowered() {
        let options: ProbeOptions =
            parse(r#"{"limits": {"maxMovieBytes": 4096, "maxMovieRatio": 8}}"#).unwrap();
        let limits = common!(options).limits();
        assert_eq!((limits.max_movie_bytes, limits.max_movie_ratio), (4096, 8));
    }

    #[test]
    fn failures_serialize_for_the_bindings() {
        assert_eq!(
            Failure::Usage("x".into()).to_json(),
            r#"{"usage":"x"}"#.to_owned()
        );
    }
}
