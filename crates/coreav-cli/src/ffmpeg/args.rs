//! `ffmpeg`'s arguments, read as its documentation describes them: options
//! apply to the next file named, `-i` names an input, and any other bare
//! argument an output. Only the stream-copy vocabulary of
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
    /// `mov,mp4,m4a,3gp,3g2,mj2` and its aliases.
    Isobmff,
    /// `matroska,webm` and its aliases.
    Matroska,
}

/// What the output is written as: `-f`, or the output's extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Muxer {
    /// `mp4` (`.mp4`).
    Mp4,
    /// `ipod` (`.m4a`): an audio-only M4A file.
    Ipod,
    /// `segment`, writing MP4 segments.
    Segment,
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
    /// `-map -0:a`: removes from what the earlier `-map` options selected.
    pub remove: bool,
    /// Which streams of the input.
    pub spec: Spec,
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
    /// The specifiers of the codec options, all `copy`, in order.
    pub copied: Vec<Spec>,
    /// `-vn`.
    pub no_video: bool,
    /// `-an`.
    pub no_audio: bool,
    /// `-sn`.
    pub no_subtitle: bool,
    /// `-dn`.
    pub no_data: bool,
    /// `-map_metadata -1`.
    pub strip_metadata: bool,
    /// `-segment_time`, for the segment muxer (2 seconds by default).
    pub segment_time: Rational,
    /// `-y`, `-n`.
    pub overwrite: Overwrite,
}

/// An option's value, or the flag itself.
#[derive(Clone, Copy)]
enum Arity {
    Flag,
    Value,
}

/// Options that may come anywhere and apply to the whole command.
const GLOBAL: [(&str, Arity); 6] = [
    ("-y", Arity::Flag),
    ("-n", Arity::Flag),
    ("-hide_banner", Arity::Flag),
    ("-nostdin", Arity::Flag),
    ("-v", Arity::Value),
    ("-loglevel", Arity::Value),
];

/// Options that apply to the next file named, codec options aside.
const PER_FILE: [(&str, Arity); 17] = [
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
];

/// Options of `ffmpeg` that are refused, with the reason.
const REFUSED: [(&str, &str); 22] = [
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
    ("-ignore_unknown", "-ignore_unknown is not native yet"),
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
fn local(name: &str, windows: bool) -> bool {
    let drive = windows
        && matches!(name.as_bytes(), [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic());
    let separator = |c: char| c == '/' || (windows && c == '\\');
    let first = name.split(separator).next().unwrap_or(name);
    name.starts_with('/') || name.starts_with("./") || drive || !first.contains(':')
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
    let (remove, rest) = text
        .strip_prefix('-')
        .map_or((false, text), |rest| (true, rest));
    if rest.ends_with('?') {
        return Err(Refusal::not_native("an optional -map (ending in ?)"));
    }
    let (file, spec) = rest
        .split_once(':')
        .map_or((rest, None), |(f, s)| (f, Some(s)));
    match file.parse::<usize>() {
        Ok(0) => {}
        Ok(_) => return Err(usage(format!("Invalid input file index: {file}."))),
        Err(_) => return Err(usage(format!("Invalid stream specifier: {text}."))),
    }
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
    Ok(Map { remove, spec })
}

/// A time in `ffmpeg`'s duration syntax, exactly: `[HH:]MM:SS[.m...]`
/// with `MM` and `SS` below 60, or
/// `S+[.m...]` with an optional unit `s`, `ms` or `us`. A leading `-`
/// (a negative time) is `Err(true)`; anything else not a time `Err(false)`.
pub fn parse_duration(text: &str) -> Result<Rational, bool> {
    if text.starts_with('-') {
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
        if !digits(hours, 9) || !sexagesimal(minutes) || !sexagesimal(seconds) {
            return Err(false);
        }
        let seconds = match fraction {
            None => parse_time(seconds),
            Some(f) => parse_time(&format!("{seconds}.{f}")),
        }
        .ok_or(false)?;
        let whole = |s: &str| s.parse::<i128>().ok();
        let base = whole(hours)
            .zip(whole(minutes))
            .and_then(|(h, m)| h.checked_mul(3600)?.checked_add(m.checked_mul(60)?))
            .ok_or(false)?;
        return Rational::from_int(base).checked_add(seconds).ok_or(false);
    }
    let (number, scale) = if let Some(n) = text.strip_suffix("ms") {
        (n, 1_000)
    } else if let Some(n) = text.strip_suffix("us") {
        (n, 1_000_000)
    } else {
        (text.strip_suffix('s').unwrap_or(text), 1)
    };
    let value = parse_time(number).ok_or(false)?;
    Rational::new(value.num, value.den.checked_mul(scale).ok_or(false)?).ok_or(false)
}

fn time(option: &str, value: &str) -> Result<Rational, Refusal> {
    parse_duration(value).map_err(|negative| {
        if negative {
            Refusal::not_native("a negative time")
        } else {
            usage(format!(
                "Invalid duration specification for {}: {value}",
                option.trim_start_matches('-')
            ))
        }
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
            return Err(not_native(
                arg,
                None,
                "an informational query; coreav ffmpeg runs stream-copy command lines only (docs/design/compat-cli.md)",
            ));
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
            ("-i", Some(input)) => inputs.push((input, std::mem::take(&mut group))),
            ("-y", _) => yes = true,
            ("-n", _) => no = true,
            ("-v" | "-loglevel", Some(level)) => crate::ffprobe::args::loglevel(level)
                .map_err(|_| usage(format!("Invalid loglevel \"{level}\".")))?,
            ("-hide_banner" | "-nostdin", _) => {}
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
    let (input, input_options) = match inputs.as_slice() {
        [] => return Err(usage("No input file: give one with -i INPUT.")),
        [one] => one.clone(),
        _ => return Err(Refusal::not_native("a command with several inputs")),
    };
    let (output, output_options) = match outputs.as_slice() {
        [] => return Err(usage("At least one output file must be specified")),
        [one] => one.clone(),
        _ => return Err(Refusal::not_native("a command with several outputs")),
    };
    let mut command = Command {
        input: input.to_owned(),
        demuxer: None,
        input_seek: None,
        input_duration: None,
        input_until: None,
        output: output.to_owned(),
        muxer: Muxer::Mp4,
        seek: None,
        duration: None,
        until: None,
        maps: Vec::new(),
        copied: Vec::new(),
        no_video: false,
        no_audio: false,
        no_subtitle: false,
        no_data: false,
        strip_metadata: false,
        segment_time: Rational::from_int(2),
        overwrite,
    };
    let mut safe = false;
    for (option, value) in input_options {
        let text = value.unwrap_or_default();
        match option {
            "-ss" => command.input_seek = Some(time(option, text)?),
            "-t" => command.input_duration = Some(time(option, text)?),
            "-to" => command.input_until = Some(time(option, text)?),
            "-f" => command.demuxer = Some(demuxer(text)?),
            "-fflags" if matches!(text, "+genpts" | "genpts") => {}
            "-fflags" => {
                return Err(not_native(
                    option,
                    Some(text),
                    "of the demuxer flags only +genpts is native (MP4 and Matroska time every packet, so it changes nothing)",
                ));
            }
            "-threads" => threads(text)?,
            "-safe" => safe = true,
            _ => {
                return Err(not_native(
                    option,
                    None,
                    "it is not native as an input option",
                ));
            }
        }
    }
    if safe {
        return Err(not_native(
            "-safe",
            None,
            "it is an option of the concat demuxer, and joining files is not native yet",
        ));
    }
    if input == "-" || input.starts_with("pipe:") {
        return Err(Refusal::not_native("an input from a pipe"));
    }
    if !local(input, cfg!(windows)) {
        return Err(Refusal::not_native("an input named with a protocol"));
    }
    let mut format = None;
    let mut segment_format = None;
    let mut segment_time = None;
    for (option, value) in output_options {
        let text = value.unwrap_or_default();
        if let Some(spec) = codec_option(option) {
            if text != "copy" {
                return Err(not_native(
                    option,
                    Some(text),
                    "encoding is not native: coreav ffmpeg copies streams (-c copy)",
                ));
            }
            command.copied.push(spec?);
            continue;
        }
        match option {
            "-ss" => command.seek = Some(time(option, text)?),
            "-t" => command.duration = Some(time(option, text)?),
            "-to" => command.until = Some(time(option, text)?),
            "-f" => format = Some(text),
            "-map" => command.maps.push(parse_map(text)?),
            "-map_metadata" if text == "-1" => command.strip_metadata = true,
            "-map_metadata" => {
                return Err(not_native(
                    option,
                    Some(text),
                    "copying metadata from a stream to the file is not native; of -map_metadata only -1 is",
                ));
            }
            "-movflags" if matches!(text, "+faststart" | "faststart") => {}
            "-movflags" => {
                return Err(not_native(
                    option,
                    Some(text),
                    "of the MP4 flags only +faststart is native (the movie box always comes first)",
                ));
            }
            "-segment_time" => {
                segment_time = Some(match parse_duration(text) {
                    Ok(t) if t.num > 0 => t,
                    _ => {
                        return Err(usage(format!(
                            "Invalid time duration specification '{text}' for segment_time option"
                        )));
                    }
                });
            }
            "-segment_format" => segment_format = Some(text),
            "-threads" => threads(text)?,
            "-max_muxing_queue_size" => {
                text.parse::<u32>().map_err(|_| {
                    usage(format!("Invalid value '{text}' for max_muxing_queue_size"))
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
    if output == "-" || output.starts_with("pipe:") {
        return Err(Refusal::not_native("an output to a pipe"));
    }
    if !local(output, cfg!(windows)) {
        return Err(Refusal::not_native("an output named with a protocol"));
    }
    command.muxer = muxer(format, output)?;
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
    Ok(command)
}

fn demuxer(name: &str) -> Result<Demuxer, Refusal> {
    match name {
        "mov,mp4,m4a,3gp,3g2,mj2" | "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2" => {
            Ok(Demuxer::Isobmff)
        }
        "matroska,webm" | "matroska" | "webm" => Ok(Demuxer::Matroska),
        "concat" => Err(not_native(
            "-f",
            Some(name),
            "joining files is not native yet",
        )),
        _ => Err(not_native(
            "-f",
            Some(name),
            "coreav ffmpeg reads MP4/MOV, Matroska and WebM",
        )),
    }
}

fn threads(text: &str) -> Result<(), Refusal> {
    if text == "auto" || text.parse::<u32>().is_ok() {
        Ok(())
    } else {
        Err(usage(format!("Invalid value '{text}' for threads")))
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
        _ => None,
    }
}

fn muxer(format: Option<&str>, output: &str) -> Result<Muxer, Refusal> {
    match format {
        Some("mp4") => Ok(Muxer::Mp4),
        Some("ipod") => Ok(Muxer::Ipod),
        Some("segment") => Ok(Muxer::Segment),
        Some(other) => Err(not_native(
            "-f",
            Some(other),
            "coreav ffmpeg writes mp4, ipod (M4A) and segment",
        )),
        None => extension_muxer(output)
            .ok_or_else(|| Refusal::not_native("an output container other than MP4 or M4A")),
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
        assert_eq!(c.copied, vec![Spec::All]);
        let c = parse(&args("-t 5 -i in.mp4 -ss 3 -c:a copy out.m4a")).unwrap();
        assert_eq!((c.input_duration, c.seek), (Some(q(5, 1)), Some(q(3, 1))));
        assert_eq!(c.muxer, Muxer::Ipod);
        assert_eq!(c.copied, vec![Spec::Kind(Kind2::Audio, None)]);
    }

    #[test]
    fn maps_read_the_selector_grammar() {
        let c = parse(&args(
            "-i in.mkv -map 0 -map -0:a:2 -map 0:1 -map -0:m:language:eng -map 0:m:language -c copy out.mp4",
        ))
        .unwrap();
        let map = |remove, spec| Map { remove, spec };
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
            option("-i in.mp4 -c:v libx264 out.mp4"),
            named("-c:v", Some("libx264"))
        );
        assert_eq!(
            option("-f concat -safe 0 -i list.txt -c copy out.mp4"),
            named("-f", Some("concat"))
        );
        assert_eq!(
            option("-safe 0 -f concat -i list.txt -c copy out.mp4"),
            named("-f", Some("concat"))
        );
        assert_eq!(
            option("-fflags +igndts -i in.mp4 -c copy out.mp4"),
            named("-fflags", Some("+igndts"))
        );
        assert_eq!(
            option("-i in.mp4 -c copy -map_metadata 0:s:0 out.m4a"),
            named("-map_metadata", Some("0:s:0"))
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
            option("-i in.mp4 -c copy -f matroska out.mkv"),
            named("-f", Some("matroska"))
        );
        for text in [
            "-i in.mp4 -c copy out.mkv",
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
        for text in [
            "-i file:in.mp4 -c copy out.mp4",
            "-i in.mp4 -c copy file:out.mp4",
            "-i in.mp4 -c copy pipe:1",
        ] {
            assert!(matches!(refused(text), Reason::Property(_)), "{text}");
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
