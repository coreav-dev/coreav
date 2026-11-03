//! `ffmpeg`'s arguments, read as its documentation describes them: options
//! apply to the next file named, `-i` names an input, and any other bare
//! argument an output. Only the vocabulary of
//! `docs/design/compat-cli.md` ("Step 1's vocabulary") that a writer of
//! CoreAV implements is accepted; every other option is refused by name
//! before the input is opened.

use super::Refusal;
use crate::clip::parse_time;
use crate::fallback::NotNative;
use crate::ffprobe::args::Kind2;
use coreav_isobmff::Rational;

/// The container family `-f` names before the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demuxer {
    /// Explicit signed-16 little-endian audio with declared rate and channels.
    S16le(coreav_audio::raw::RawS16Format),
    /// `mov,mp4,m4a,3gp,3g2,mj2` and its aliases.
    Isobmff,
    /// `matroska,webm` and its aliases.
    Matroska,
    /// `concat`: the input is a list of files to join.
    Concat {
        /// The demuxer's `safe` option (on unless `-safe 0`).
        safe: bool,
    },
}

/// What the output is written as: `-f`, or the output's extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Muxer {
    /// `avi` (`.avi`), a single bounded RIFF file.
    Avi,
    /// `mp4` (`.mp4`).
    Mp4,
    /// `ipod` (`.m4a`): an audio-only M4A file.
    Ipod,
    /// `mp3` (`.mp3`): one Layer III stream.
    Mp3,
    /// Native FLAC (`.flac`).
    Flac,
    /// `segment`, writing MP4 segments.
    Segment,
    /// Matroska output, with H.264 and Vorbis defaults.
    Matroska,
    /// Audio-only automatic selection for .mka.
    MatroskaAudio,
    /// WebM with its restricted codec set.
    Webm,
    /// `wav` (`.wav`), with signed 16-bit PCM by default.
    Wav,
    /// Headerless signed 16-bit little-endian PCM (`-f s16le`).
    S16le,
    /// One encoded image per file.
    Image2,
    /// Encoded video packets without a container.
    Rawvideo,
    /// YUV4MPEG2 frames and their stream header.
    Yuv4mpeg,
}

impl Muxer {
    /// Whether this muxer writes decoded video rather than a remux.
    pub fn frames(self) -> bool {
        matches!(self, Self::Image2 | Self::Rawvideo | Self::Yuv4mpeg)
    }
    /// Matroska output profile, when selected.
    pub fn matroska(self) -> Option<coreav_mkv::DocType> {
        match self {
            Self::Matroska | Self::MatroskaAudio => Some(coreav_mkv::DocType::Matroska),
            Self::Webm => Some(coreav_mkv::DocType::Webm),
            _ => None,
        }
    }
}

/// The native video encoders for image and raw output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoEncoder {
    /// PNG.
    Png,
    /// Windows bitmap.
    Bmp,
    /// Unconverted decoded samples.
    Rawvideo,
}

/// Frame selection and image-file naming, independent of the encoder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameOptions {
    /// `-frames:v` or `-vframes`.
    pub limit: Option<u64>,
    /// `-update 1`.
    pub update: bool,
    /// `-r` after the input, in frames per second.
    pub rate: Option<Rational>,
}

/// A stream specifier: of `-map` (input streams) or of a codec option
/// (output streams).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spec {
    /// Every stream.
    All,
    /// The stream with this index.
    Index(usize),
    /// Every stream of a type, or the `n`-th of them.
    Kind(Kind2, Option<usize>),
    /// `m:language[:VALUE]`: streams with a `language` tag (of this value).
    Language(Option<String>),
}

/// One `-map`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Map {
    /// Which input file, starting at zero.
    pub file: usize,
    /// `-map -0:a`: removes from what the earlier `-map` options selected.
    pub remove: bool,
    /// Which streams of the input.
    pub spec: Spec,
}

/// What a codec option asks for its streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// The native lossless FLAC encoder.
    Flac,
    /// Audio encoders supported by the AVI and PCM writers.
    AviAudio(AviAudio),
    /// `copy`.
    Copy,
    /// `libx264`: the video re-encoded (`docs/design/transcode.md`,
    /// "Through `coreav ffmpeg`").
    Libx264,
    /// An encoder of one audio stream.
    Audio(AudioEncoder),
    /// A native image or raw video encoder.
    Video(VideoEncoder),
}

/// AVI and PCM audio output encoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AviAudio {
    /// The operator's external FFmpeg `libmp3lame` encoder.
    Mp3,
    /// Signed little-endian 16-bit PCM.
    PcmS16,
    /// Unsigned 8-bit PCM.
    PcmU8,
}

/// The audio encoders of `coreav ffmpeg` (`docs/design/transcode.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioEncoder {
    /// `libopus`: Opus (`docs/design/transcode.md`, "Opus audio").
    Libopus,
    /// `aac`, FFmpeg's built-in encoder: AAC-LC (`docs/design/transcode.md`,
    /// "AAC audio").
    Aac,
    /// `libmp3lame`, an operator-provided FFmpeg encoder process.
    Libmp3lame,
}

/// The encoder options of a command that encodes with `libx264`, as given;
/// [`crate::ffmpeg::encode`] reads their values. The last of a repeated
/// option wins, as in `ffmpeg`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Encoding {
    /// `-crf`.
    pub crf: Option<String>,
    /// `-b:v`.
    pub bitrate: Option<String>,
    /// `-preset`.
    pub preset: Option<String>,
    /// `-threads` after the input.
    pub threads: Option<String>,
    /// `-vf` or `-filter:v`.
    pub filter: Option<String>,
    /// `-s` after the input.
    pub size: Option<String>,
    /// `-pix_fmt`.
    pub pixel_format: Option<String>,
    /// `-b:a`, with `libopus` or `aac`.
    pub audio_bitrate: Option<String>,
    /// `-q:a` or `-qscale:a` for libmp3lame VBR.
    pub audio_quality: Option<String>,
    /// `-fps_mode` or `-vsync`: `passthrough` (or `0`).
    pub passthrough: bool,
}

/// What `-y` and `-n` say about an existing output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overwrite {
    /// Neither: `ffmpeg` asks on the terminal.
    #[default]
    Ask,
    /// `-y`.
    Yes,
    /// `-n`.
    No,
}

/// A command line `coreav ffmpeg` runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// The input file.
    pub input: String,
    /// The remaining input files, in command-line order.
    pub more_inputs: Vec<String>,
    /// `-f` before `-i`.
    pub demuxer: Option<Demuxer>,
    /// `-ss` before `-i`: input seeking.
    pub input_seek: Option<Rational>,
    /// `-t` before `-i`.
    pub input_duration: Option<Rational>,
    /// `-to` before `-i`.
    pub input_until: Option<Rational>,
    /// The output file, or the segment name pattern.
    pub output: String,
    /// How the output is written.
    pub muxer: Muxer,
    /// `-ss` after `-i`: output seeking.
    pub seek: Option<Rational>,
    /// `-t` after `-i`.
    pub duration: Option<Rational>,
    /// `-to` after `-i`.
    pub until: Option<Rational>,
    /// The `-map` options, in order.
    pub maps: Vec<Map>,
    /// The codec options, in order: for each stream the last one that
    /// names it decides.
    pub codecs: Vec<(Spec, Codec)>,
    /// Video and audio encoder options, checked by the selected writer.
    pub encoding: Encoding,
    /// `-ar` after the input, accepted for PCM only at the source rate.
    pub sample_rate: Option<u32>,
    /// `-ac` after the input, accepted for PCM only at the source count.
    pub channels: Option<u16>,
    /// Image and raw-video output options.
    pub frames: FrameOptions,
    /// `-noautorotate` before the input.
    pub noautorotate: bool,
    /// `-vn`.
    pub no_video: bool,
    /// `-an`.
    pub no_audio: bool,
    /// `-sn`.
    pub no_subtitle: bool,
    /// `-dn`.
    pub no_data: bool,
    /// `-map_metadata`.
    pub map_metadata: MapMetadata,
    /// `-segment_time`, for the segment muxer (2 seconds by default).
    pub segment_time: Rational,
    /// `-y`, `-n`.
    pub overwrite: Overwrite,
}

/// What `-map_metadata` asks of the output's metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapMetadata {
    /// No `-map_metadata`: `ffmpeg`'s default copy.
    Default,
    /// `-map_metadata -1`: no metadata of the input.
    Strip,
    /// `-map_metadata 0:s:N`: the file's metadata from the input's stream
    /// N, and no stream's own metadata.
    Stream(usize),
}

/// An option's value, or the flag itself.
#[derive(Clone, Copy)]
enum Arity {
    Flag,
    Value,
}

/// Options that may come anywhere and apply to the whole command.
const GLOBAL: [(&str, Arity); 7] = [
    ("-y", Arity::Flag),
    ("-n", Arity::Flag),
    ("-hide_banner", Arity::Flag),
    ("-nostdin", Arity::Flag),
    ("-v", Arity::Value),
    ("-loglevel", Arity::Value),
    ("-ignore_unknown", Arity::Flag),
];

/// Options that apply to the next file named, codec options aside.
const PER_FILE: [(&str, Arity); 36] = [
    ("-ar", Arity::Value),
    ("-ac", Arity::Value),
    ("-frames:v", Arity::Value),
    ("-vframes", Arity::Value),
    ("-update", Arity::Value),
    ("-r", Arity::Value),
    ("-ss", Arity::Value),
    ("-t", Arity::Value),
    ("-to", Arity::Value),
    ("-f", Arity::Value),
    ("-map", Arity::Value),
    ("-map_metadata", Arity::Value),
    ("-movflags", Arity::Value),
    ("-fflags", Arity::Value),
    ("-safe", Arity::Value),
    ("-segment_time", Arity::Value),
    ("-segment_format", Arity::Value),
    ("-threads", Arity::Value),
    ("-max_muxing_queue_size", Arity::Value),
    ("-an", Arity::Flag),
    ("-vn", Arity::Flag),
    ("-sn", Arity::Flag),
    ("-dn", Arity::Flag),
    ("-noautorotate", Arity::Flag),
    ("-crf", Arity::Value),
    ("-b:v", Arity::Value),
    ("-b:a", Arity::Value),
    ("-q:a", Arity::Value),
    ("-qscale:a", Arity::Value),
    ("-preset", Arity::Value),
    ("-vf", Arity::Value),
    ("-filter:v", Arity::Value),
    ("-s", Arity::Value),
    ("-pix_fmt", Arity::Value),
    ("-fps_mode", Arity::Value),
    ("-vsync", Arity::Value),
];

/// Options of `ffmpeg` that are refused, with the reason.
const REFUSED: [(&str, &str); 21] = [
    ("-vf", "filtering needs a decoder and an encoder"),
    ("-af", "filtering needs a decoder and an encoder"),
    ("-filter", "filtering needs a decoder and an encoder"),
    (
        "-filter_complex",
        "filtering needs a decoder and an encoder",
    ),
    ("-bsf", "bitstream filters are not applied"),
    ("-r", "changing the frame rate needs an encoder"),
    ("-s", "scaling needs an encoder"),
    ("-b", "setting a bit rate needs an encoder"),
    ("-crf", "encoding is not native"),
    ("-preset", "encoding is not native"),
    ("-metadata", "writing metadata is not native"),
    ("-map_chapters", "chapter selection is not native yet"),
    ("-sseof", "seeking from the end is not native yet"),
    (
        "-copyts",
        "keeping the input's timestamps is not native yet",
    ),
    (
        "-start_at_zero",
        "keeping the input's timestamps is not native yet",
    ),
    ("-avoid_negative_ts", "timestamp shifting is not native yet"),
    (
        "-reset_timestamps",
        "segment timestamp options are not native yet",
    ),
    ("-segment_list", "segment lists are not native yet"),
    ("-shortest", "-shortest is not native yet"),
    ("-hls_time", "HLS output is not native yet"),
    ("-hls_list_size", "HLS output is not native yet"),
];

fn not_native(option: &str, value: Option<&str>, why: &str) -> Refusal {
    let named = value.map_or_else(|| option.to_owned(), |v| format!("{option} {v}"));
    Refusal::NotNative(NotNative::option(
        option,
        value,
        format!("'{named}' is not supported: {why}"),
    ))
}

/// Whether a file name is surely a path to `ffmpeg`: no colon in its first
/// component (before the first `/`, and on Windows the first `\`), where
/// `ffmpeg` may read a protocol and its options (`file:in.mp4`,
/// `subfile,,start,0,,:in.mp4`, `:in.mp4`). A name that starts with `/` or
/// `./` is a path, and on Windows so is a drive letter before a separator
/// (`C:\in.mp4`, `C:/in.mp4`); elsewhere `ffmpeg` reads `C` as a protocol.
/// The concat demuxer reads the paths of a list the same way.
pub(crate) fn local(name: &str, windows: bool) -> bool {
    let drive = windows
        && matches!(name.as_bytes(), [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic());
    let separator = |c: char| c == '/' || (windows && c == '\\');
    let first = name.split(separator).next().unwrap_or(name);
    name.starts_with('/') || name.starts_with("./") || drive || !first.contains(':')
}

fn file_path(name: &str) -> String {
    match name.strip_prefix("file:") {
        Some(path)
            if cfg!(windows)
                && matches!(path.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic()) =>
        {
            path.to_owned()
        }
        Some(path) if path == "-" || !local(path, cfg!(windows)) => format!("./{path}"),
        Some(path) => path.to_owned(),
        None => name.to_owned(),
    }
}

fn usage(text: impl Into<String>) -> Refusal {
    Refusal::Usage(text.into())
}

/// A codec option's name: `-c`, `-codec`, `-vcodec`, `-acodec`, `-scodec`,
/// the first two with a stream specifier (`-c:v`, `-codec:a:0`).
fn codec_option(name: &str) -> Option<Result<Spec, Refusal>> {
    match name {
        "-c" | "-codec" => return Some(Ok(Spec::All)),
        "-vcodec" => return Some(Ok(Spec::Kind(Kind2::Video, None))),
        "-acodec" => return Some(Ok(Spec::Kind(Kind2::Audio, None))),
        "-scodec" => return Some(Ok(Spec::Kind(Kind2::Subtitle, None))),
        _ => {}
    }
    let spec = name
        .strip_prefix("-c:")
        .or_else(|| name.strip_prefix("-codec:"))?;
    Some(parse_spec(spec).ok_or_else(|| {
        Refusal::not_native("a codec option whose stream specifier is not a type or an index")
    }))
}

/// `N`, `TYPE` or `TYPE:N`.
fn parse_spec(text: &str) -> Option<Spec> {
    if let Ok(n) = text.parse::<usize>() {
        return Some(Spec::Index(n));
    }
    let (kind, index) = text
        .split_once(':')
        .map_or((text, None), |(k, n)| (k, Some(n)));
    let kind = match kind {
        "v" => Kind2::Video,
        "V" => Kind2::VideoNotPicture,
        "a" => Kind2::Audio,
        "s" => Kind2::Subtitle,
        "d" => Kind2::Data,
        "t" => Kind2::Attachment,
        _ => return None,
    };
    let index = match index {
        None => None,
        Some(n) => Some(n.parse::<usize>().ok()?),
    };
    Some(Spec::Kind(kind, index))
}

/// `[-]0[:SPEC]`, with `m:language[:VALUE]` among the specifiers.
fn parse_map(text: &str) -> Result<Map, Refusal> {
    if text.starts_with('[') && text.ends_with(']') {
        return Err(not_native("-map", Some(text), "a filtergraph output label"));
    }
    let (remove, rest) = text
        .strip_prefix('-')
        .map_or((false, text), |rest| (true, rest));
    if rest.ends_with('?') {
        return Err(Refusal::not_native("an optional -map (ending in ?)"));
    }
    if rest == "a" {
        return Ok(Map {
            remove,
            file: 0,
            spec: Spec::Kind(Kind2::Audio, None),
        });
    }
    let (file, spec) = rest
        .split_once(':')
        .map_or((rest, None), |(f, s)| (f, Some(s)));
    let file = file
        .parse::<usize>()
        .map_err(|_| usage(format!("Invalid stream specifier: {text}.")))?;
    let spec = match spec {
        None => Spec::All,
        Some(metadata) if metadata.starts_with("m:") => {
            let mut parts = metadata.splitn(3, ':').skip(1);
            match (parts.next(), parts.next()) {
                (Some("language"), value) => Spec::Language(value.map(str::to_owned)),
                _ => {
                    return Err(Refusal::not_native(
                        "a -map metadata specifier other than language",
                    ));
                }
            }
        }
        Some(spec) => parse_spec(spec).ok_or_else(|| {
            Refusal::not_native("a -map stream specifier other than a type, an index or language")
        })?,
    };
    Ok(Map { file, remove, spec })
}

/// A time in `ffmpeg`'s duration syntax, exactly: `[HH:]MM:SS[.m...]`
/// with `MM` and `SS` below 60, or
/// `S+[.m...]` with an optional unit `s`, `ms` or `us`. `Err(true)` is
/// outside the native exact representation; `Err(false)` is malformed.
pub fn parse_duration(text: &str) -> Result<Rational, bool> {
    if text.starts_with(['-', '+']) {
        return Err(true);
    }
    let digits = |s: &str, most: usize| {
        !s.is_empty() && s.len() <= most && s.bytes().all(|b| b.is_ascii_digit())
    };
    if text.contains(':') {
        let (clock, fraction) = text
            .split_once('.')
            .map_or((text, None), |(c, f)| (c, Some(f)));
        let parts: Vec<&str> = clock.split(':').collect();
        let (hours, minutes, seconds) = match parts.as_slice() {
            [m, s] => ("0", *m, *s),
            [h, m, s] => (*h, *m, *s),
            _ => return Err(false),
        };
        let sexagesimal = |s: &str| digits(s, 2) && s.parse::<u8>().is_ok_and(|v| v < 60);
        if !digits(hours, usize::MAX) || !sexagesimal(minutes) || !sexagesimal(seconds) {
            return Err(false);
        }
        if hours.len() > 9 {
            return Err(true);
        }
        if fraction.is_some_and(|f| !f.bytes().all(|b| b.is_ascii_digit())) {
            return Err(false);
        }
        let seconds = match fraction {
            None => parse_time(seconds),
            Some(f) => parse_time(&format!("{seconds}.{f}")),
        }
        .ok_or(true)?;
        let whole = |s: &str| s.parse::<i128>().ok();
        let base = whole(hours)
            .zip(whole(minutes))
            .and_then(|(h, m)| h.checked_mul(3600)?.checked_add(m.checked_mul(60)?))
            .ok_or(true)?;
        return Rational::from_int(base).checked_add(seconds).ok_or(true);
    }
    let (number, scale) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1_000)
    } else if let Some(n) = text.strip_suffix("us") {
        (n, 1_000_000)
    } else {
        (text.strip_suffix('s').unwrap_or(text), 1)
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if !digits(whole, usize::MAX) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return Err(false);
    }
    let value = parse_time(number).ok_or(true)?;
    Rational::new(value.num, value.den.checked_mul(scale).ok_or(true)?).ok_or(true)
}

fn time(option: &str, value: &str) -> Result<Rational, Refusal> {
    parse_duration(value).map_err(|unsupported| {
        if value.starts_with('-') {
            Refusal::not_native("a negative time")
        } else if unsupported {
            not_native(
                option,
                Some(value),
                "a duration outside the native exact time syntax",
            )
        } else {
            usage(format!(
                "Invalid duration specification for {}: {value}",
                option.trim_start_matches('-')
            ))
        }
    })
}

fn audio_rate(text: &str) -> Option<u32> {
    let (number, scale) = match text.as_bytes().last() {
        Some(b'k' | b'K') => (text.get(..text.len() - 1)?, 1_000u128),
        Some(b'M') => (text.get(..text.len() - 1)?, 1_000_000),
        Some(b'G') => (text.get(..text.len() - 1)?, 1_000_000_000),
        _ => (text, 1),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let denominator = 10u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u128>().ok()?
    };
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().ok()?
    };
    let value = whole
        .checked_mul(denominator)?
        .checked_add(fraction)?
        .checked_mul(scale)?;
    if value % denominator != 0 {
        return None;
    }
    u32::try_from(value / denominator)
        .ok()
        .filter(|rate| *rate > 0)
}

fn rate(option: &str, text: &str) -> Result<u32, Refusal> {
    audio_rate(text).ok_or_else(|| {
        not_native(
            option,
            Some(text),
            "the sample rate must be a positive whole number of Hz",
        )
    })
}

/// The options given for one file, in order.
type Group<'a> = Vec<(&'a str, Option<&'a str>)>;

/// Parses `ffmpeg`'s arguments, without the program name.
pub fn parse(args: &[String]) -> Result<Command, Refusal> {
    let mut inputs: Vec<(&str, Group)> = Vec::new();
    let mut outputs: Vec<(&str, Group)> = Vec::new();
    let (mut yes, mut no) = (false, false);
    let mut group: Group = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        let arg = arg.as_str();
        i += 1;
        if matches!(arg, "-h" | "-help" | "--help" | "-?") {
            return match args.get(i) {
                None => Err(Refusal::Help),
                Some(topic) => Err(not_native(
                    arg,
                    Some(topic),
                    "help on a topic describes FFmpeg's own components; coreav ffmpeg prints only its own usage (https://coreav.dev/docs/commands/ffmpeg)",
                )),
            };
        }
        if !arg.starts_with('-') || arg == "-" {
            outputs.push((arg, std::mem::take(&mut group)));
            continue;
        }
        let arity = GLOBAL
            .iter()
            .chain(PER_FILE.iter())
            .find(|(name, _)| *name == arg)
            .map(|(_, arity)| *arity)
            .or_else(|| (arg == "-i" || codec_option(arg).is_some()).then_some(Arity::Value));
        let Some(arity) = arity else {
            let base = arg.split(':').next().unwrap_or(arg);
            let why = REFUSED.iter().find(|(name, _)| *name == base).map_or(
                "it is not in coreav ffmpeg's stream-copy vocabulary",
                |(_, why)| *why,
            );
            return Err(not_native(arg, None, why));
        };
        let value = match arity {
            Arity::Flag => None,
            Arity::Value => {
                let Some(value) = args.get(i) else {
                    return Err(usage(format!(
                        "Missing argument for option '{}'.",
                        arg.trim_start_matches('-')
                    )));
                };
                i += 1;
                Some(value.as_str())
            }
        };
        match (arg, value) {
            ("-i", Some(input)) => {
                #[cfg(not(feature = "net"))]
                if input.starts_with("http://") || input.starts_with("https://") {
                    return Err(usage(
                        "this build has no network; URL inputs are not supported",
                    ));
                }
                inputs.push((input, std::mem::take(&mut group)));
            }
            ("-y", _) => yes = true,
            ("-n", _) => no = true,
            ("-v" | "-loglevel", Some(level)) => crate::ffprobe::args::loglevel(level)
                .map_err(|_| usage(format!("Invalid loglevel \"{level}\".")))?,
            ("-hide_banner" | "-nostdin" | "-ignore_unknown", _) => {}
            _ => group.push((arg, value)),
        }
    }
    if !group.is_empty() {
        return Err(Refusal::not_native("an option after the last output file"));
    }
    let overwrite = match (yes, no) {
        (true, true) => return Err(usage("Error, both -y and -n supplied. Exiting.")),
        (true, false) => Overwrite::Yes,
        (false, true) => Overwrite::No,
        (false, false) => Overwrite::Ask,
    };
    if inputs.len() > coreav_isobmff_write::MAX_REMUX_INPUTS {
        return Err(Refusal::not_native("more than sixteen input files"));
    }
    if inputs.len() > 1 && inputs.iter().any(|(_, group)| !group.is_empty()) {
        return Err(Refusal::not_native("input options with several inputs"));
    }
    let (input, input_options) = inputs
        .first()
        .cloned()
        .ok_or_else(|| usage("No input file: give one with -i INPUT."))?;
    let more_inputs: Vec<String> = inputs
        .iter()
        .skip(1)
        .map(|(path, _)| file_path(path))
        .collect();
    for (path, _) in inputs.iter().skip(1) {
        if *path == "-"
            || path.starts_with("pipe:")
            || (!path.starts_with("file:") && !local(path, cfg!(windows)))
        {
            return Err(Refusal::not_native(
                "an input named with a protocol or pipe",
            ));
        }
    }
    let (output, output_options) = match outputs.as_slice() {
        [] => return Err(usage("At least one output file must be specified")),
        [one] => one.clone(),
        _ => return Err(Refusal::not_native("a command with several outputs")),
    };
    let mut command = Command {
        input: file_path(input),
        more_inputs,
        demuxer: None,
        input_seek: None,
        input_duration: None,
        input_until: None,
        output: file_path(output),
        muxer: Muxer::Mp4,
        sample_rate: None,
        channels: None,
        seek: None,
        duration: None,
        until: None,
        maps: Vec::new(),
        codecs: Vec::new(),
        encoding: Encoding::default(),
        frames: FrameOptions::default(),
        noautorotate: false,
        no_video: false,
        no_audio: false,
        no_subtitle: false,
        no_data: false,
        map_metadata: MapMetadata::Default,
        segment_time: Rational::from_int(2),
        overwrite,
    };
    let input_format = input_options
        .iter()
        .rev()
        .find_map(|(option, value)| (*option == "-f").then_some(*value).flatten());
    let raw = input_format == Some("s16le");
    let mut input_rate = None;
    let mut input_channels = None;
    let mut safe = None;
    for (option, value) in input_options {
        let text = value.unwrap_or_default();
        match option {
            "-ss" => command.input_seek = Some(time(option, text)?),
            "-t" => command.input_duration = Some(time(option, text)?),
            "-to" => command.input_until = Some(time(option, text)?),
            "-f" => {}
            "-ar" if raw => input_rate = Some(rate(option, text)?),
            "-ac" if raw => {
                input_channels =
                    Some(text.parse::<u16>().ok().filter(|n| *n > 0).ok_or_else(|| {
                        not_native(
                            option,
                            Some(text),
                            "an audio channel count outside the native integer syntax",
                        )
                    })?)
            }
            "-fflags" if matches!(text, "+genpts" | "genpts") => {}
            "-fflags" => {
                return Err(not_native(
                    option,
                    Some(text),
                    "of the demuxer flags only +genpts is native (MP4 and Matroska time every packet, so it changes nothing)",
                ));
            }
            "-threads" => threads(text)?,
            "-noautorotate" => command.noautorotate = true,
            "-safe" => safe = Some(boolean(option, text)?),
            _ => {
                return Err(not_native(
                    option,
                    None,
                    "it is not native as an input option",
                ));
            }
        }
    }
    command.demuxer = if raw {
        let (Some(rate), Some(channels)) = (input_rate, input_channels) else {
            return Err(Refusal::not_native(
                "raw PCM input without explicit -ar and -ac",
            ));
        };
        Some(Demuxer::S16le(
            coreav_audio::raw::RawS16Format::new(rate, channels)
                .map_err(|error| not_native("-f", Some("s16le"), &error.to_string()))?,
        ))
    } else {
        input_format.map(demuxer).transpose()?
    };
    match (&mut command.demuxer, safe) {
        (Some(Demuxer::Concat { safe }), Some(given)) => *safe = given,
        (_, Some(_)) => return Err(usage("Option safe not found.")),
        (_, None) => {}
    }
    if input == "-" || input.starts_with("pipe:") {
        return Err(Refusal::not_native("an input from a pipe"));
    }
    if !input.starts_with("file:") && !local(input, cfg!(windows)) {
        return Err(Refusal::not_native("an input named with a protocol"));
    }
    let mut format = None;
    let mut segment_format = None;
    let mut segment_time = None;
    let mut encoders = Vec::new();
    let mut audio_encoders = Vec::new();
    let mut movflags = false;
    for (option, value) in output_options {
        let text = value.unwrap_or_default();
        if let Some(spec) = codec_option(option) {
            let codec = match text {
                "copy" => Codec::Copy,
                "libx264" => Codec::Libx264,
                "libopus" => Codec::Audio(AudioEncoder::Libopus),
                "aac" => Codec::Audio(AudioEncoder::Aac),
                "libmp3lame" => Codec::Audio(AudioEncoder::Libmp3lame),
                "flac" => Codec::Flac,
                "pcm_s16le" => Codec::AviAudio(AviAudio::PcmS16),
                "pcm_u8" => Codec::AviAudio(AviAudio::PcmU8),
                "png" => Codec::Video(VideoEncoder::Png),
                "bmp" => Codec::Video(VideoEncoder::Bmp),
                "rawvideo" => Codec::Video(VideoEncoder::Rawvideo),
                _ => {
                    return Err(not_native(
                        option,
                        Some(text),
                        "coreav ffmpeg copies streams (-c copy) and encodes video with libx264 and audio with flac, libopus, aac or libmp3lame",
                    ));
                }
            };
            command.codecs.push((spec?, codec));
            continue;
        }
        let value = Some(text.to_owned());
        let encoding = &mut command.encoding;
        match option {
            "-crf" => encoding.crf = value,
            "-b:v" => encoding.bitrate = value,
            "-preset" => encoding.preset = value,
            "-vf" | "-filter:v" => encoding.filter = value,
            "-s" => encoding.size = value,
            "-pix_fmt" => encoding.pixel_format = value,
            "-b:a" => encoding.audio_bitrate = value,
            "-q:a" | "-qscale:a" => encoding.audio_quality = value,
            "-fps_mode" | "-vsync" => {
                if !matches!(text, "passthrough") && !(option == "-vsync" && text == "0") {
                    return Err(not_native(
                        option,
                        Some(text),
                        "of the frame-rate modes only passthrough is native",
                    ));
                }
                encoding.passthrough = true;
            }
            _ => {}
        }
        if matches!(
            option,
            "-crf"
                | "-b:v"
                | "-preset"
                | "-vf"
                | "-filter:v"
                | "-s"
                | "-pix_fmt"
                | "-fps_mode"
                | "-vsync"
        ) {
            encoders.push(option);
            continue;
        }
        if matches!(option, "-b:a" | "-q:a" | "-qscale:a") {
            audio_encoders.push(option);
            continue;
        }
        match option {
            "-frames:v" | "-vframes" => {
                command.frames.limit = Some(text.parse().map_err(|_| {
                    not_native(
                        option,
                        Some(text),
                        "a frame limit outside the native unsigned integer syntax",
                    )
                })?);
            }
            "-update" => command.frames.update = boolean(option, text)?,
            "-r" => {
                let rate = match text.split_once('/') {
                    Some((n, d)) => n
                        .parse::<i128>()
                        .ok()
                        .zip(d.parse::<u64>().ok())
                        .and_then(|(n, d)| Rational::new(n, d)),
                    None if !text.contains(':') => crate::clip::parse_time(text),
                    None => None,
                };
                let rate = match rate {
                    Some(rate) if rate.num > 0 => rate,
                    Some(_) => return Err(usage("Invalid output frame rate")),
                    None if matches!(
                        text,
                        "ntsc" | "ntsc-film" | "pal" | "film" | "qntsc" | "qpal" | "sntsc" | "spal"
                    ) || text.parse::<f64>().is_ok_and(|r| r.is_finite() && r > 0.0)
                        || text.split_once(['/', ':']).is_some_and(|(n, d)| {
                            n.parse::<f64>()
                                .ok()
                                .zip(d.parse::<f64>().ok())
                                .is_some_and(|(n, d)| {
                                    d != 0.0 && (n / d).is_finite() && n / d > 0.0
                                })
                        })
                        || (text.parse::<f64>().is_err()
                            && !text.ends_with("/0")
                            && text.contains(['(', '+', '*', '^'])
                            && text.bytes().any(|b| b.is_ascii_digit())) =>
                    {
                        return Err(not_native(
                            option,
                            Some(text),
                            "a frame-rate name or expression outside the native integer-ratio and decimal syntax",
                        ));
                    }
                    None => return Err(usage("Invalid output frame rate")),
                };
                command.frames.rate = Some(rate);
            }
            "-ss" => command.seek = Some(time(option, text)?),
            "-t" => command.duration = Some(time(option, text)?),
            "-to" => command.until = Some(time(option, text)?),
            "-f" => format = Some(text),
            "-ar" => command.sample_rate = Some(rate(option, text)?),
            "-ac" => {
                command.channels =
                    Some(text.parse::<u16>().ok().filter(|n| *n > 0).ok_or_else(|| {
                        not_native(
                            option,
                            Some(text),
                            "an audio channel count outside the native integer syntax",
                        )
                    })?)
            }
            "-map" => command.maps.push(parse_map(text)?),
            "-map_metadata" => {
                let stream = text
                    .strip_prefix("0:s:")
                    .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|n| n.parse().ok());
                command.map_metadata = match (command.map_metadata, stream) {
                    (MapMetadata::Default, _) if text == "-1" => MapMetadata::Strip,
                    (MapMetadata::Default, Some(index)) => MapMetadata::Stream(index),
                    _ => {
                        return Err(not_native(
                            option,
                            Some(text),
                            "of -map_metadata only one -1 or 0:s:N (an input stream's index) is native",
                        ));
                    }
                };
            }
            "-movflags" if matches!(text, "+faststart" | "faststart") => movflags = true,
            "-movflags" => {
                return Err(not_native(
                    option,
                    Some(text),
                    "of the MP4 flags only +faststart is native (the movie box always comes first)",
                ));
            }
            "-segment_time" => {
                segment_time = Some(match time(option, text)? {
                    t if t.num > 0 => t,
                    _ => {
                        return Err(usage(format!(
                            "Invalid time duration specification '{text}' for segment_time option"
                        )));
                    }
                });
            }
            "-segment_format" => segment_format = Some(text),
            "-threads" => {
                threads(text)?;
                command.encoding.threads = Some(text.to_owned());
            }
            "-max_muxing_queue_size" => {
                text.parse::<u32>().map_err(|_| {
                    not_native(
                        option,
                        Some(text),
                        "a queue size outside the native unsigned integer syntax",
                    )
                })?;
            }
            "-an" => command.no_audio = true,
            "-vn" => command.no_video = true,
            "-sn" => command.no_subtitle = true,
            "-dn" => command.no_data = true,
            _ => {
                return Err(not_native(
                    option,
                    None,
                    "it is not native as an output option",
                ));
            }
        }
    }
    command.muxer = muxer(format, output)?;
    if (output == "-" || output.starts_with("pipe:")) && command.muxer != Muxer::Rawvideo {
        return Err(Refusal::not_native("an output to a pipe"));
    }
    if !output.starts_with("file:") && !local(output, cfg!(windows)) && output != "-" {
        return Err(Refusal::not_native("an output named with a protocol"));
    }
    if let Some(option) = audio_encoders
        .iter()
        .copied()
        .find(|option| *option != "-b:a")
        && command.muxer != Muxer::Mp3
        && !command
            .codecs
            .iter()
            .any(|(_, codec)| *codec == Codec::Audio(AudioEncoder::Libmp3lame))
    {
        return Err(not_native(
            option,
            None,
            "audio quality is supported only with libmp3lame",
        ));
    }
    if let Some(option) = encoders.first()
        && !command.codecs.iter().any(|(_, c)| *c == Codec::Libx264)
        && !command.muxer.frames()
        && command.muxer != Muxer::Matroska
    {
        return Err(not_native(
            option,
            None,
            "it is an encoder option, and coreav ffmpeg encodes video only with -c:v libx264",
        ));
    }
    if let Some(option) = audio_encoders.first()
        && command.muxer != Muxer::Mp3
        && !command
            .codecs
            .iter()
            .any(|(_, c)| matches!(c, Codec::Audio(_) | Codec::AviAudio(_)))
    {
        return Err(not_native(
            option,
            None,
            "it is an audio encoder option, and coreav ffmpeg encodes audio with libopus, aac or libmp3lame",
        ));
    }
    if !matches!(
        command.muxer,
        Muxer::Avi | Muxer::Wav | Muxer::S16le | Muxer::Flac
    ) && command
        .codecs
        .iter()
        .any(|(_, codec)| matches!(codec, Codec::AviAudio(_)))
    {
        return Err(Refusal::not_native(
            "PCM encoding to an unsupported container",
        ));
    }
    if matches!(command.muxer, Muxer::Wav | Muxer::S16le | Muxer::Flac) {
        if movflags {
            return Err(not_native(
                "-movflags",
                None,
                "it is not an option of native audio output",
            ));
        }
    } else if command.sample_rate.is_some() {
        return Err(not_native(
            "-ar",
            None,
            "it is not supported on this output",
        ));
    } else if command.channels.is_some() {
        return Err(not_native(
            "-ac",
            None,
            "it is not supported on this output",
        ));
    }
    if !command.muxer.frames() && command.frames != FrameOptions::default() {
        return Err(Refusal::not_native(
            "image and raw-video options on another output",
        ));
    }
    if command.muxer == Muxer::Segment {
        let written = match segment_format {
            None => extension_muxer(output),
            Some("mp4") => Some(Muxer::Mp4),
            Some(_) => None,
        };
        if written != Some(Muxer::Mp4) {
            return Err(Refusal::not_native("a segment format other than MP4"));
        }
        if let Some(t) = segment_time {
            command.segment_time = t;
        }
    } else {
        for (option, given) in [
            ("-segment_format", segment_format.is_some()),
            ("-segment_time", segment_time.is_some()),
        ] {
            if given {
                return Err(not_native(
                    option,
                    None,
                    "it is an option of the segment muxer (-f segment)",
                ));
            }
        }
    }
    for map in &command.maps {
        if map.file >= inputs.len() {
            return Err(usage(format!("Invalid input file index: {}.", map.file)));
        }
    }
    if !command.more_inputs.is_empty() && command.maps.is_empty() {
        return Err(Refusal::NotNative(NotNative::property(
            "a command with several inputs",
            "not supported by coreav ffmpeg: automatic stream selection across several inputs; give explicit -map options",
        )));
    }
    Ok(command)
}

fn demuxer(name: &str) -> Result<Demuxer, Refusal> {
    match name {
        "mov,mp4,m4a,3gp,3g2,mj2" | "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2" => {
            Ok(Demuxer::Isobmff)
        }
        "matroska,webm" | "matroska" | "webm" => Ok(Demuxer::Matroska),
        "concat" => Ok(Demuxer::Concat { safe: true }),
        _ => Err(not_native(
            "-f",
            Some(name),
            "coreav ffmpeg reads MP4/MOV, Matroska, WebM or explicit s16le with -ar and -ac",
        )),
    }
}

/// A boolean option's value: a number that is 0 or 1, or `true`, `yes`,
/// `false` or `no` (black box); another number is a usage error, as in
/// `ffmpeg`.
fn boolean(option: &str, text: &str) -> Result<bool, Refusal> {
    match (text, text.parse::<i64>()) {
        (_, Ok(1)) | ("true" | "yes", _) => Ok(true),
        (_, Ok(0)) | ("false" | "no", _) => Ok(false),
        (_, Ok(_)) => Err(usage(format!(
            "Unable to parse option value \"{text}\" as boolean"
        ))),
        _ => Err(not_native(option, Some(text), "an unknown boolean value")),
    }
}

fn threads(text: &str) -> Result<(), Refusal> {
    if text == "auto" || text.parse::<u32>().is_ok() {
        Ok(())
    } else {
        Err(not_native(
            "-threads",
            Some(text),
            "a thread count outside the native unsigned integer syntax",
        ))
    }
}

/// The muxer `ffmpeg` guesses from an output name's extension, among those
/// CoreAV writes.
fn extension_muxer(output: &str) -> Option<Muxer> {
    let extension = std::path::Path::new(output)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    match extension.as_str() {
        "mp4" => Some(Muxer::Mp4),
        "m4a" => Some(Muxer::Ipod),
        "mp3" => Some(Muxer::Mp3),
        "flac" => Some(Muxer::Flac),
        "avi" => Some(Muxer::Avi),
        "wav" => Some(Muxer::Wav),
        "png" | "bmp" => Some(Muxer::Image2),
        "yuv" => Some(Muxer::Rawvideo),
        "y4m" => Some(Muxer::Yuv4mpeg),
        "mkv" => Some(Muxer::Matroska),
        "mka" => Some(Muxer::MatroskaAudio),
        "webm" => Some(Muxer::Webm),
        _ => None,
    }
}

fn muxer(format: Option<&str>, output: &str) -> Result<Muxer, Refusal> {
    match format {
        Some("mp4") => Ok(Muxer::Mp4),
        Some("ipod") => Ok(Muxer::Ipod),
        Some("mp3") => Ok(Muxer::Mp3),
        Some("flac") => Ok(Muxer::Flac),
        Some("avi") => Ok(Muxer::Avi),
        Some("wav") => Ok(Muxer::Wav),
        Some("s16le") => Ok(Muxer::S16le),
        Some("segment") => Ok(Muxer::Segment),
        Some("image2") => Ok(Muxer::Image2),
        Some("rawvideo") => Ok(Muxer::Rawvideo),
        Some("yuv4mpegpipe") => Ok(Muxer::Yuv4mpeg),
        Some("matroska") => Ok(Muxer::Matroska),
        Some("webm") => Ok(Muxer::Webm),
        Some(other) => Err(not_native(
            "-f",
            Some(other),
            "coreav ffmpeg writes avi, mp4, ipod (M4A), mp3, flac, wav, s16le, image2, rawvideo, yuv4mpegpipe, matroska, webm and segment",
        )),
        None => extension_muxer(output)
            .ok_or_else(|| Refusal::not_native("an unsupported output container")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::Reason;

    fn q(num: i128, den: u64) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn refused(text: &str) -> Reason {
        match parse(&args(text)) {
            Err(Refusal::NotNative(n)) => n.reason,
            other => panic!("{text}: {other:?}"),
        }
    }

    #[test]
    fn frame_output_options_belong_to_the_output() {
        for (output, muxer) in [
            ("out.png", Muxer::Image2),
            ("out.BMP", Muxer::Image2),
            ("out.yuv", Muxer::Rawvideo),
            ("out.y4m", Muxer::Yuv4mpeg),
        ] {
            let command = parse(&args(&format!(
                "-i in.mp4 -frames:v 3 -r 30000/1001 {output}"
            )))
            .unwrap();
            assert_eq!(command.muxer, muxer);
            assert_eq!(command.frames.limit, Some(3));
            assert_eq!(command.frames.rate, Rational::new(30000, 1001));
        }
        assert!(parse(&args("-i in.mp4 -f rawvideo -")).is_ok());
        assert!(parse(&args("-i in.mp4 -c:v png -f rawvideo -")).is_ok());
        for text in [
            "-r 1 -i in.mp4 out.png",
            "-i in.mp4 -frames:v -1 out.png",
            "-i in.mp4 -r 0 out.png",
            "-i in.mp4 -r 1/0 out.png",
            "-i in.mp4 -c copy -frames:v 1 out.mp4",
            "-i in.mp4 -f image2 -",
        ] {
            assert!(parse(&args(text)).is_err(), "{text}");
        }
    }

    #[test]
    fn unsupported_rate_syntax_can_fall_back_but_invalid_rates_fail_usage() {
        for rate in [
            "ntsc",
            "ntsc-film",
            "pal",
            "film",
            "qntsc",
            "qpal",
            "sntsc",
            "spal",
            "1e1",
            "+1",
            "2.5/1",
            "25/1.0",
            "1e1/2",
            "1:2",
            "1+1",
        ] {
            assert!(
                matches!(
                    parse(&args(&format!("-i in.mp4 -r {rate} out.png"))),
                    Err(Refusal::NotNative(_))
                ),
                "{rate}"
            );
        }
        for rate in ["bogus", "0", "+0", "-1", "1/0", "nan", "inf"] {
            assert!(
                matches!(
                    parse(&args(&format!("-i in.mp4 -r {rate} out.png"))),
                    Err(Refusal::Usage(_))
                ),
                "{rate}"
            );
        }
    }

    #[test]
    fn help_is_coreavs_without_a_topic_and_ffmpegs_with_one() {
        for text in ["-h", "-?", "-help", "--help", "-i in.mp4 -c copy -h"] {
            assert_eq!(parse(&args(text)), Err(Refusal::Help), "{text}");
        }
        assert_eq!(
            refused("-h long"),
            Reason::Option("-h".to_owned(), Some("long".to_owned()))
        );
        assert_eq!(
            refused("--help muxer=mp4"),
            Reason::Option("--help".to_owned(), Some("muxer=mp4".to_owned()))
        );
        assert_ne!(parse(&args("-i -h -c copy out.mp4")), Err(Refusal::Help));
    }

    #[test]
    fn durations_parse_exactly() {
        assert_eq!(parse_duration("120"), Ok(q(120, 1)));
        assert_eq!(parse_duration("00:01:02.500"), Ok(q(125, 2)));
        assert_eq!(parse_duration("03:10"), Ok(q(190, 1)));
        assert_eq!(parse_duration("1:00"), Ok(q(60, 1)));
        assert_eq!(parse_duration("01:19:27"), Ok(q(4767, 1)));
        assert_eq!(parse_duration("100ms"), Ok(q(1, 10)));
        assert_eq!(parse_duration("600ms"), Ok(q(3, 5)));
        assert_eq!(parse_duration("1500us"), Ok(q(3, 2000)));
        assert_eq!(parse_duration("2.5s"), Ok(q(5, 2)));
        assert_eq!(parse_duration("-1"), Err(true));
        assert_eq!(parse_duration("00:59.5"), Ok(q(119, 2)));
        assert_eq!(parse_duration("0:5"), Ok(q(5, 1)));
        assert_eq!(parse_duration("100:00:00"), Ok(q(360_000, 1)));
        assert_eq!(parse_duration("1000000000:00:00"), Err(true));
        for bad in [
            "", "1:2:3:4", "1:234", "a", "1e3", ".5", "1:00:", "00:99", "00:60", "60:00",
            "1:00:60", "01:60:00", "00:60.5", "1.5:00", "5:", ":5",
        ] {
            assert_eq!(parse_duration(bad), Err(false), "{bad}");
        }
    }

    #[test]
    fn options_apply_to_the_next_file() {
        let c = parse(&args("-ss 60 -i in.mp4 -to 70 -c copy -y out.mp4")).unwrap();
        assert_eq!(c.input_seek, Some(q(60, 1)));
        assert_eq!(c.until, Some(q(70, 1)));
        assert_eq!(c.seek, None);
        assert_eq!(c.overwrite, Overwrite::Yes);
        assert_eq!(c.codecs, vec![(Spec::All, Codec::Copy)]);
        let c = parse(&args("-t 5 -i in.mp4 -ss 3 -c:a copy out.m4a")).unwrap();
        assert_eq!((c.input_duration, c.seek), (Some(q(5, 1)), Some(q(3, 1))));
        assert_eq!(c.muxer, Muxer::Ipod);
        assert_eq!(
            c.codecs,
            vec![(Spec::Kind(Kind2::Audio, None), Codec::Copy)]
        );
    }

    #[test]
    fn the_concat_demuxer_takes_its_safe_option() {
        let demuxer = |text: &str| parse(&args(text)).unwrap().demuxer;
        let concat = |safe| Some(Demuxer::Concat { safe });
        assert_eq!(demuxer("-f concat -i l.txt -c copy o.mp4"), concat(true));
        assert_eq!(
            demuxer("-f concat -safe 0 -i l.txt -c copy o.mp4"),
            concat(false)
        );
        assert_eq!(
            demuxer("-safe 00 -f concat -i l.txt -c copy o.mp4"),
            concat(false)
        );
        assert_eq!(
            demuxer("-safe yes -f concat -i l.txt -c copy o.mp4"),
            concat(true)
        );
        for bad in [
            "-f concat -safe 2 -i l.txt -c copy o.mp4",
            "-f concat -safe -1 -i l.txt -c copy o.mp4",
            "-safe 0 -i in.mp4 -c copy o.mp4",
        ] {
            assert!(matches!(parse(&args(bad)), Err(Refusal::Usage(_))), "{bad}");
        }
    }

    #[test]
    fn maps_read_the_selector_grammar() {
        let c = parse(&args(
            "-i in.mkv -map 0 -map -0:a:2 -map 0:1 -map -0:m:language:eng -map 0:m:language -c copy out.mp4",
        ))
        .unwrap();
        let map = |remove, spec| Map {
            file: 0,
            remove,
            spec,
        };
        assert_eq!(
            c.maps,
            vec![
                map(false, Spec::All),
                map(true, Spec::Kind(Kind2::Audio, Some(2))),
                map(false, Spec::Index(1)),
                map(true, Spec::Language(Some("eng".to_owned()))),
                map(false, Spec::Language(None)),
            ]
        );
    }

    #[test]
    fn mapped_inputs_are_bounded_and_validate_file_indexes() {
        let command = parse(&args(
            "-i first.mp4 -i second.mp4 -map 1:a -map -0:a -c copy out.mp4",
        ))
        .unwrap();
        assert_eq!(command.more_inputs, ["second.mp4"]);
        assert_eq!(
            command.maps.iter().map(|map| map.file).collect::<Vec<_>>(),
            [1, 0]
        );
        assert!(matches!(
            parse(&args("-i first.mp4 -i second.mp4 -map 2:a -c copy out.mp4")),
            Err(Refusal::Usage(_))
        ));
        assert!(matches!(
            parse(&args(
                "-i first.mp4 -i https://example.invalid/second.mp4 -map 0 -c copy out.mp4"
            )),
            Err(Refusal::NotNative(_))
        ));
        let mut many = Vec::new();
        for _ in 0..=coreav_isobmff_write::MAX_REMUX_INPUTS {
            many.extend(["-i".to_owned(), "in.mp4".to_owned()]);
        }
        many.extend(["-map", "0", "-c", "copy", "out.mp4"].map(String::from));
        assert!(matches!(parse(&many), Err(Refusal::NotNative(_))));
    }

    #[test]
    fn segments_need_the_segment_muxer() {
        let c = parse(&args(
            "-i in.mp4 -c copy -map 0 -f segment -segment_time 300 -segment_format mp4 out%03d.mp4",
        ))
        .unwrap();
        assert_eq!((c.muxer, c.segment_time), (Muxer::Segment, q(300, 1)));
        assert!(matches!(
            refused("-i in.mp4 -c copy -segment_time 3 out.mp4"),
            Reason::Option(o, None) if o == "-segment_time"
        ));
        assert!(matches!(
            refused("-i in.mp4 -c copy -f segment -segment_format mpegts out%d.ts"),
            Reason::Property(_)
        ));
    }

    #[test]
    fn what_is_not_native_is_named() {
        let option = |text: &str| match refused(text) {
            Reason::Option(name, value) => (name, value),
            Reason::Property(p) => panic!("{text}: {p}"),
        };
        let named = |name: &str, value: Option<&str>| (name.to_owned(), value.map(str::to_owned));
        assert_eq!(
            option("-i in.mp4 -vf scale=2:2 out.mp4"),
            named("-vf", None)
        );
        assert_eq!(
            option("-i in.mp4 -c:v libx265 out.mp4"),
            named("-c:v", Some("libx265"))
        );
        assert_eq!(
            option("-i in.mp4 -c copy -crf 23 out.mp4"),
            named("-crf", None)
        );
        assert_eq!(
            option("-crf 23 -i in.mp4 -c:v libx264 out.mp4"),
            named("-crf", None)
        );
        assert_eq!(
            option("-i in.mp4 -c:v libx264 -fps_mode cfr out.mp4"),
            named("-fps_mode", Some("cfr"))
        );
        assert_eq!(
            option("-f concat -safe auto -i list.txt -c copy out.mp4"),
            named("-safe", Some("auto"))
        );
        assert_eq!(
            option("-fflags +igndts -i in.mp4 -c copy out.mp4"),
            named("-fflags", Some("+igndts"))
        );
        assert_eq!(
            option("-i in.mp4 -c copy -map_metadata 0 out.m4a"),
            named("-map_metadata", Some("0"))
        );
        assert_eq!(
            option("-i in.mp4 -c copy -movflags +frag_keyframe out.mp4"),
            named("-movflags", Some("+frag_keyframe"))
        );
        assert_eq!(
            option("-i in.mp4 -c copy -copyts out.mp4"),
            named("-copyts", None)
        );
        assert_eq!(
            option("-i in.mp4 -filter:v x out.mp4"),
            named("-filter:v", None)
        );
        assert_eq!(
            option("-i in.mp4 -c copy -f unsupported-test-muxer out.unsupported"),
            named("-f", Some("unsupported-test-muxer"))
        );
        for text in [
            "-i in.mp4 -c copy out.unsupported",
            "-i in.mp4 -i b.mp4 -c copy out.mp4",
            "-i - -c copy out.mp4",
            "-i in.mp4 -c copy -",
            "-i https://example.com/a.mp4 -c copy out.mp4",
            "-i in.mp4 -c copy -map 0:p:1 out.mp4",
            "-i in.mp4 -c copy -map 0:a? out.mp4",
            "-i in.mp4 -map 0:m:title:x -c copy out.mp4",
            "-i in.mp4 -c copy out.mp4 -an",
            "-ss -1 -i in.mp4 -c copy out.mp4",
        ] {
            assert!(matches!(refused(text), Reason::Property(_)), "{text}");
        }
    }

    #[test]
    fn names_with_a_colon_in_their_first_component_are_not_paths() {
        for name in [
            "file:in.mp4",
            "pipe:1",
            "https://e.com/a.mp4",
            "crypto+file:a",
            "subfile,,start,0,end,99999,,:in.mp4",
            ":in.mp4",
            ":x",
            "a,b:c",
            "a:b/c.mp4",
            "C:in.mp4",
        ] {
            assert!(!local(name, false), "{name}");
            assert!(!local(name, true), "{name}");
        }
        for name in [
            "in.mp4",
            "dir/a:b.mp4",
            "./file:in.mp4",
            "./a:b.mp4",
            "/a:b.mp4",
        ] {
            assert!(local(name, false), "{name}");
            assert!(local(name, true), "{name}");
        }
        for name in [r"C:\in.mp4", "C:/in.mp4", r"dir\a:b.mp4"] {
            assert!(local(name, true), "{name}");
            assert!(!local(name, false), "{name}");
        }
        assert!(matches!(
            refused("-i in.mp4 -c copy pipe:1"),
            Reason::Property(_)
        ));
        let command = parse(&args("-i file:in.mp4 -c copy file:out.mp4")).unwrap();
        assert_eq!(command.input, "in.mp4");
        assert_eq!(command.output, "out.mp4");
    }

    #[test]
    fn libx264_commands_keep_their_encoder_options() {
        let c = parse(&args(
            "-noautorotate -i in.mp4 -c copy -c:v libx264 -crf 19 -preset slow -preset fast -b:v 1M \
             -vf scale=-2:360 -s 640x360 -pix_fmt yuv420p -threads 2 -vsync 0 out.mp4",
        ))
        .unwrap();
        assert!(c.noautorotate);
        assert_eq!(
            c.codecs,
            vec![
                (Spec::All, Codec::Copy),
                (Spec::Kind(Kind2::Video, None), Codec::Libx264)
            ]
        );
        let text = |s: &str| Some(s.to_owned());
        assert_eq!(
            c.encoding,
            Encoding {
                crf: text("19"),
                bitrate: text("1M"),
                preset: text("fast"),
                threads: text("2"),
                filter: text("scale=-2:360"),
                size: text("640x360"),
                pixel_format: text("yuv420p"),
                passthrough: true,
                audio_bitrate: None,
                audio_quality: None,
            }
        );
    }

    #[test]
    fn libopus_and_its_bit_rate() {
        let c = parse(&args(
            "-i in.mkv -c:v libx264 -c:a libopus -b:a 128k out.mp4",
        ))
        .unwrap();
        assert_eq!(
            c.codecs,
            vec![
                (Spec::Kind(Kind2::Video, None), Codec::Libx264),
                (
                    Spec::Kind(Kind2::Audio, None),
                    Codec::Audio(AudioEncoder::Libopus)
                )
            ]
        );
        assert_eq!(c.encoding.audio_bitrate.as_deref(), Some("128k"));
        assert!(matches!(
            parse(&args("-i in.mkv -c:v libx264 -b:a 128k out.mp4")),
            Err(Refusal::NotNative(_))
        ));
    }

    #[test]
    fn aac_and_its_bit_rate() {
        for line in [
            "-i in.mkv -c:v libx264 -c:a aac -b:a 96k out.mp4",
            "-i in.mkv -c:v libx264 -acodec aac -b:a 96k out.mp4",
        ] {
            let c = parse(&args(line)).unwrap();
            assert_eq!(
                c.codecs,
                vec![
                    (Spec::Kind(Kind2::Video, None), Codec::Libx264),
                    (
                        Spec::Kind(Kind2::Audio, None),
                        Codec::Audio(AudioEncoder::Aac)
                    )
                ],
                "{line}"
            );
            assert_eq!(c.encoding.audio_bitrate.as_deref(), Some("96k"), "{line}");
        }
        let option = |text: &str| match refused(text) {
            Reason::Option(name, value) => (name, value),
            Reason::Property(p) => panic!("{text}: {p}"),
        };
        let named = |name: &str, value: Option<&str>| (name.to_owned(), value.map(str::to_owned));
        for (extra, name, value) in [
            ("-profile:a aac_low", "-profile:a", None),
            ("-aac_coder twoloop", "-aac_coder", None),
            ("-q:a 2", "-q:a", None),
            ("-vbr 3", "-vbr", None),
            ("-ar 44100", "-ar", None),
            ("-ac 1", "-ac", None),
            ("-af volume=2", "-af", None),
            ("-strict experimental", "-strict", None),
            ("-c:a libfdk_aac", "-c:a", Some("libfdk_aac")),
            ("-c:a aac_at", "-c:a", Some("aac_at")),
        ] {
            let line = format!("-i in.mkv -c:v libx264 -c:a aac {extra} out.mp4");
            assert_eq!(option(&line), named(name, value), "{line}");
        }
    }

    #[test]
    fn malformed_commands_are_usage_errors() {
        for text in [
            "-i in.mp4",
            "-c copy out.mp4",
            "-i in.mp4 -ss 1:2:3:4 -c copy out.mp4",
            "-i in.mp4 -map 1 -c copy out.mp4",
            "-i in.mp4 -c",
            "-v loud -i in.mp4 -c copy out.mp4",
        ] {
            assert!(
                matches!(parse(&args(text)), Err(Refusal::Usage(_))),
                "{text}"
            );
        }
    }
}
