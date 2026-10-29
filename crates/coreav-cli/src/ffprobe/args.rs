//! `ffprobe`'s arguments, read as its documentation describes them, and
//! the vocabulary of `docs/design/compat-cli.md` ("Step 1's vocabulary"):
//! what is in it is parsed, what is not is refused by name before the
//! input is opened. The vocabulary is the one
//! `tools/compat-corpus/classify.py` labels the command-coverage set by,
//! so a command that set calls native runs and every other one is
//! refused.

use super::model::{Selection, Sid};
use super::writer::{Escape, Kind, Style};
use crate::fallback::NotNative;

/// A command that is not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Outside the vocabulary: exit 4, or FFmpeg in its place. The message
    /// names the option or property and says why.
    Unsupported(NotNative),
    /// Malformed, as `ffprobe` would also fail it: exit 2.
    Usage(String),
    /// `-h`, `-help`, `--help`: the usage text, exit 0.
    Help,
}

/// The containers `-f` may name, as FFmpeg spells their demuxers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    /// `mov,mp4,m4a,3gp,3g2,mj2`.
    Isobmff,
    /// `matroska,webm`.
    Matroska,
    /// `mpegts`.
    Mpegts,
}

/// A stream specifier of `-select_streams`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamSpec {
    /// A stream index.
    Index(usize),
    /// Every stream of a type, or the `n`-th of them.
    Kind(Kind2, Option<usize>),
}

/// A stream type a specifier names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind2 {
    /// `v`.
    Video,
    /// `V`: video that is not an attached picture.
    VideoNotPicture,
    /// `a`.
    Audio,
    /// `s`.
    Subtitle,
    /// `d`.
    Data,
    /// `t`.
    Attachment,
}

/// One `-read_intervals` interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    /// Where reading starts, in seconds; `None` from the start.
    pub start: Option<f64>,
    /// The start is relative to the current position (`+`).
    pub start_relative: bool,
    /// Where it ends.
    pub end: Option<IntervalEnd>,
}

/// The end of an interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntervalEnd {
    /// An absolute time in seconds.
    At(f64),
    /// A duration after the start.
    After(f64),
    /// A number of packets after the start (`+#N`).
    Packets(u64),
}

/// What the command line asks for.
#[derive(Clone, Debug)]
pub struct Options {
    /// The input, a file or a URL.
    pub input: String,
    /// `-of`.
    pub writer: Kind,
    /// `-pretty`, `-sexagesimal`.
    pub style: Style,
    /// `-show_*` and `-show_entries`.
    pub selection: Selection,
    /// `-select_streams`.
    pub select_streams: Option<StreamSpec>,
    /// `-count_packets`.
    pub count_packets: bool,
    /// `-show_error`.
    pub show_error: bool,
    /// `-read_intervals`.
    pub read_intervals: Vec<Interval>,
    /// `-f`.
    pub force: Option<Container>,
    /// Whether any section was asked for.
    pub sections: bool,
}

const VALUE_OPTIONS: [&str; 10] = [
    "-i",
    "-v",
    "-loglevel",
    "-of",
    "-print_format",
    "-output_format",
    "-show_entries",
    "-select_streams",
    "-read_intervals",
    "-f",
];

const FLAG_OPTIONS: [&str; 11] = [
    "-show_format",
    "-show_streams",
    "-show_chapters",
    "-show_programs",
    "-show_packets",
    "-show_error",
    "-count_packets",
    "-hide_banner",
    "-pretty",
    "-sexagesimal",
    "-nostdin",
];

/// Options `ffprobe` has that `coreav ffprobe` refuses, with the reason.
const REFUSED: [(&str, &str); 14] = [
    ("-show_frames", "frames need a decoder"),
    ("-count_frames", "counting frames needs a decoder"),
    ("-show_log", "frames need a decoder"),
    ("-skip_frame", "frames need a decoder"),
    ("-show_data", "packet payloads are not printed"),
    ("-show_data_hash", "packet payloads are not printed"),
    ("-show_stream_groups", "stream groups are not read"),
    (
        "-show_pixel_formats",
        "informational queries are not answered",
    ),
    (
        "-show_program_version",
        "informational queries are not answered",
    ),
    (
        "-show_library_versions",
        "informational queries are not answered",
    ),
    ("-show_versions", "informational queries are not answered"),
    ("-sections", "informational queries are not answered"),
    ("-version", "informational queries are not answered"),
    ("-formats", "informational queries are not answered"),
];

/// The sections `-show_entries` may name.
const ENTRY_SECTIONS: [&str; 11] = [
    "format",
    "format_tags",
    "stream",
    "stream_tags",
    "stream_side_data",
    "packet",
    "packet_tags",
    "chapter",
    "chapter_tags",
    "program",
    "program_tags",
];

fn unsupported(option: &str, value: Option<&str>, text: impl Into<String>) -> Refusal {
    Refusal::Unsupported(NotNative::option(option, value, text))
}

/// Parses `ffprobe`'s arguments, without the program name.
pub fn parse(args: &[String]) -> Result<Options, Refusal> {
    let mut input: Option<String> = None;
    let mut writer = Kind::Default {
        nokey: false,
        noprint_wrappers: false,
    };
    let mut style = Style::default();
    let mut selection = Selection::default();
    let mut select_streams = None;
    let mut count_packets = false;
    let mut show_error = false;
    let mut read_intervals = Vec::new();
    let mut force = None;
    let mut sections = false;
    // Every option is checked before any is acted on, so that a refusal
    // names the first option outside the vocabulary.
    let mut i = 0;
    let mut pairs: Vec<(&str, Option<&str>)> = Vec::new();
    while let Some(arg) = args.get(i) {
        let arg = arg.as_str();
        if matches!(arg, "-h" | "-help" | "--help" | "-?") {
            return Err(Refusal::Help);
        }
        if is_option(arg) {
            if VALUE_OPTIONS.contains(&arg) {
                let Some(value) = args.get(i + 1) else {
                    return Err(Refusal::Usage(format!(
                        "Missing argument for option '{}'.",
                        &arg[1..]
                    )));
                };
                pairs.push((arg, Some(value.as_str())));
                i += 2;
                continue;
            }
            if FLAG_OPTIONS.contains(&arg) {
                pairs.push((arg, None));
                i += 1;
                continue;
            }
            if let Some((_, why)) = REFUSED.iter().find(|(name, _)| *name == arg) {
                return Err(unsupported(
                    arg,
                    None,
                    format!("option '{arg}' is not supported: {why}"),
                ));
            }
            return Err(unsupported(
                arg,
                None,
                format!("option '{arg}' is not supported by coreav ffprobe"),
            ));
        }
        pairs.push(("", Some(arg)));
        i += 1;
    }
    for (option, value) in pairs {
        let value = value.unwrap_or_default();
        match option {
            "" | "-i" => {
                if let Some(first) = &input {
                    return Err(Refusal::Usage(format!(
                        "Argument '{value}' provided as input filename, but '{first}' was already specified."
                    )));
                }
                input = Some(value.to_owned());
            }
            "-v" | "-loglevel" => loglevel(value)?,
            "-of" | "-print_format" | "-output_format" => writer = parse_writer(option, value)?,
            "-show_entries" => {
                parse_entries(value, &mut selection)?;
                sections = true;
            }
            "-select_streams" => select_streams = Some(parse_spec(value)?),
            "-read_intervals" => read_intervals = parse_intervals(value)?,
            "-f" => {
                force = Some(match value {
                    "mov,mp4,m4a,3gp,3g2,mj2" => Container::Isobmff,
                    "matroska,webm" => Container::Matroska,
                    "mpegts" => Container::Mpegts,
                    other => {
                        return Err(unsupported(
                            "-f",
                            Some(other),
                            format!(
                                "'-f {other}' is not supported: coreav ffprobe reads mov,mp4,m4a,3gp,3g2,mj2, matroska,webm and mpegts"
                            ),
                        ));
                    }
                });
            }
            "-show_format" => {
                selection.select_all(Sid::Format);
                sections = true;
            }
            "-show_streams" => {
                selection.select_all(Sid::Stream);
                selection.select_bare(Sid::ProgramStreamDisposition);
                selection.select_bare(Sid::ProgramStreamTags);
                sections = true;
            }
            "-show_chapters" => {
                selection.select_all(Sid::Chapters);
                sections = true;
            }
            "-show_programs" => {
                selection.select_all(Sid::Programs);
                sections = true;
            }
            "-show_packets" => {
                selection.select_all(Sid::Packet);
                sections = true;
            }
            "-show_error" => {
                show_error = true;
                sections = true;
            }
            "-count_packets" => count_packets = true,
            "-pretty" => {
                style.units = true;
                style.sexagesimal = true;
            }
            "-sexagesimal" => style.sexagesimal = true,
            _ => {}
        }
    }
    let Some(input) = input else {
        return Err(Refusal::Usage(
            "You have to specify one input file.".to_owned(),
        ));
    };
    if !input.starts_with("http://") && !input.starts_with("https://") && input.contains(':') {
        if let Some((protocol, _)) = input.split_once(':') {
            if protocol.len() > 1 && protocol.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(Refusal::Unsupported(NotNative::property(
                    "an input other than a file or an HTTP(S) URL",
                    format!(
                        "input '{input}' is not supported: coreav ffprobe reads files and HTTP(S) URLs"
                    ),
                )));
            }
        }
    }
    if input == "-" {
        return Err(Refusal::Unsupported(NotNative::property(
            "standard input",
            "input '-' is not supported: coreav ffprobe reads files and HTTP(S) URLs",
        )));
    }
    selection.finish();
    Ok(Options {
        input,
        writer,
        style,
        selection,
        select_streams,
        count_packets,
        show_error,
        read_intervals,
        force,
        sections,
    })
}

fn is_option(arg: &str) -> bool {
    arg.starts_with('-') && arg != "-" && !arg[1..].starts_with(|c: char| c.is_ascii_digit())
}

/// `-v`: a level name or number, with `+`/`-` flags; checked, then
/// ignored (nothing is logged).
pub(crate) fn loglevel(value: &str) -> Result<(), Refusal> {
    const LEVELS: [&str; 9] = [
        "quiet", "panic", "fatal", "error", "warning", "info", "verbose", "debug", "trace",
    ];
    let mut rest = value;
    loop {
        let flagged = rest
            .strip_prefix('+')
            .or_else(|| rest.strip_prefix('-'))
            .and_then(|r| ["repeat", "level"].iter().find_map(|f| r.strip_prefix(f)));
        match flagged {
            Some(r) => rest = r,
            None => break,
        }
    }
    let level = rest.strip_prefix('+').unwrap_or(rest);
    if level.is_empty() && rest != value {
        return Ok(());
    }
    if LEVELS.contains(&level) || level.parse::<i32>().is_ok() {
        Ok(())
    } else {
        Err(Refusal::Usage(format!("Invalid loglevel \"{value}\".")))
    }
}

fn parse_bool(option: &str, value: &str) -> Result<bool, Refusal> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => value
            .parse::<i64>()
            .map(|n| n != 0)
            .map_err(|_| Refusal::Usage(format!("Invalid value '{value}' for option '{option}'"))),
    }
}

fn parse_char(option: &str, value: &str) -> Result<u8, Refusal> {
    match value.as_bytes() {
        [c] => Ok(*c),
        _ => Err(Refusal::Usage(format!(
            "Item separator '{value}' specified for option '{option}', but must be one character."
        ))),
    }
}

fn parse_writer(option: &str, value: &str) -> Result<Kind, Refusal> {
    let (name, settings) = value.split_once('=').unwrap_or((value, ""));
    let mut kind = match name {
        "default" => Kind::Default {
            nokey: false,
            noprint_wrappers: false,
        },
        "compact" => Kind::Compact {
            sep: b'|',
            nokey: false,
            escape: Escape::C,
            print_section: true,
        },
        "csv" => Kind::Compact {
            sep: b',',
            nokey: true,
            escape: Escape::Csv,
            print_section: true,
        },
        "flat" => Kind::Flat {
            sep: b'.',
            hierarchical: true,
        },
        "json" => Kind::Json { compact: false },
        other => {
            return Err(unsupported(
                option,
                Some(value),
                format!(
                    "writer '{other}' is not supported: coreav ffprobe writes default, compact, csv, flat and json"
                ),
            ));
        }
    };
    for setting in settings.split(':').filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = setting.split('=').collect();
        let [key, v] = parts.as_slice() else {
            return Err(unsupported(
                option,
                Some(value),
                format!("writer option '{setting}' is not supported: each option is one key=value"),
            ));
        };
        let (key, v) = (*key, *v);
        match (&mut kind, key) {
            (Kind::Default { nokey, .. }, "nokey" | "nk") => *nokey = parse_bool(key, v)?,
            (
                Kind::Default {
                    noprint_wrappers, ..
                },
                "noprint_wrappers" | "nw",
            ) => *noprint_wrappers = parse_bool(key, v)?,
            (Kind::Compact { sep, .. }, "item_sep" | "s") => *sep = parse_char(key, v)?,
            (Kind::Compact { nokey, .. }, "nokey" | "nk") => *nokey = parse_bool(key, v)?,
            (Kind::Compact { escape, .. }, "escape" | "e") => {
                *escape = match v {
                    "c" => Escape::C,
                    "csv" => Escape::Csv,
                    "none" => Escape::None,
                    _ => {
                        return Err(Refusal::Usage(format!("Unknown escape mode value '{v}'")));
                    }
                }
            }
            (Kind::Compact { print_section, .. }, "print_section" | "p") => {
                *print_section = parse_bool(key, v)?;
            }
            (Kind::Flat { sep, .. }, "sep_char" | "s") => *sep = parse_char(key, v)?,
            (Kind::Flat { hierarchical, .. }, "hierarchical" | "h") => {
                *hierarchical = parse_bool(key, v)?;
            }
            (Kind::Json { compact }, "compact" | "c") => *compact = parse_bool(key, v)?,
            _ => {
                return Err(unsupported(
                    option,
                    Some(value),
                    format!("writer option '{key}' of '{name}' is not supported"),
                ));
            }
        }
    }
    Ok(kind)
}

/// `-show_entries SECTION[=KEY,...][:SECTION...]`.
fn parse_entries(value: &str, selection: &mut Selection) -> Result<(), Refusal> {
    for part in value.split(':').filter(|p| !p.is_empty()) {
        let (section, keys) = match part.split_once('=') {
            Some((section, keys)) => (section.trim(), Some(keys)),
            None => (part.trim(), None),
        };
        if !ENTRY_SECTIONS.contains(&section) {
            return Err(unsupported(
                "-show_entries",
                Some(section),
                format!(
                    "'-show_entries {section}' is not supported: coreav ffprobe prints {}",
                    ENTRY_SECTIONS.join(", ")
                ),
            ));
        }
        let keys: Option<Vec<String>> = keys.map(|k| {
            k.split(',')
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(str::to_owned)
                .collect()
        });
        for key in keys.iter().flatten() {
            if let Some(why) = super::undecided(section, key) {
                return Err(unsupported(
                    "-show_entries",
                    Some(&format!("{section}={key}")),
                    format!("'-show_entries {section}={key}' is not supported: {why}"),
                ));
            }
        }
        for sid in Sid::ALL {
            let def = sid.def();
            if def.unique == section || def.name == section {
                match &keys {
                    None => selection.select_all(sid),
                    Some(keys) => selection.select_keys(sid, keys),
                }
            }
        }
        // `stream` names the streams of a stream group too, whose array
        // `ffprobe` then prints, empty.
        if section == "stream" && keys.as_ref().is_none_or(|k| !k.is_empty()) {
            selection.select_keys(Sid::StreamGroups, &["".to_owned()]);
        }
    }
    Ok(())
}

/// `-select_streams`: an index, or a type with an optional index.
fn parse_spec(value: &str) -> Result<StreamSpec, Refusal> {
    let refuse = || {
        unsupported(
            "-select_streams",
            Some(value),
            format!(
                "'-select_streams {value}' is not supported: coreav ffprobe takes N, v, a, s, d, t, V and TYPE:N"
            ),
        )
    };
    if let Ok(n) = value.parse::<usize>() {
        return Ok(StreamSpec::Index(n));
    }
    let (kind, rest) = value
        .split_once(':')
        .map_or((value, None), |(k, r)| (k, Some(r)));
    let kind = match kind {
        "v" => Kind2::Video,
        "V" => Kind2::VideoNotPicture,
        "a" => Kind2::Audio,
        "s" => Kind2::Subtitle,
        "d" => Kind2::Data,
        "t" => Kind2::Attachment,
        _ => return Err(refuse()),
    };
    let n = match rest {
        None => None,
        Some(r) => Some(r.parse::<usize>().map_err(|_| refuse())?),
    };
    Ok(StreamSpec::Kind(kind, n))
}

/// A time of `-read_intervals`: seconds, or `[HH:]MM:SS[.m]`.
fn parse_time(text: &str) -> Option<f64> {
    let (negative, text) = text.strip_prefix('-').map_or((false, text), |t| (true, t));
    let parts: Vec<&str> = text.split(':').collect();
    let seconds = match parts.as_slice() {
        [s] => s.parse::<f64>().ok()?,
        [m, s] => m.parse::<f64>().ok()? * 60.0 + s.parse::<f64>().ok()?,
        [h, m, s] => {
            h.parse::<f64>().ok()? * 3600.0
                + m.parse::<f64>().ok()? * 60.0
                + s.parse::<f64>().ok()?
        }
        _ => return None,
    };
    seconds
        .is_finite()
        .then_some(if negative { -seconds } else { seconds })
}

/// `-read_intervals INTERVAL[,INTERVAL...]`, each
/// `[START|+START_OFFSET][%[END|+END_OFFSET|+#PACKETS]]`.
fn parse_intervals(value: &str) -> Result<Vec<Interval>, Refusal> {
    let bad = || Refusal::Usage(format!("Invalid interval specification '{value}'"));
    let mut out = Vec::new();
    for spec in value.split(',') {
        let (start, end) = spec
            .split_once('%')
            .map_or((spec, None), |(s, e)| (s, Some(e)));
        let (start_relative, start) = start
            .strip_prefix('+')
            .map_or((false, start), |s| (true, s));
        let start = if start.is_empty() {
            None
        } else {
            Some(parse_time(start).ok_or_else(bad)?)
        };
        let end = match end {
            None | Some("") => None,
            Some(e) => Some(if let Some(n) = e.strip_prefix("+#") {
                IntervalEnd::Packets(n.parse().map_err(|_| bad())?)
            } else if let Some(d) = e.strip_prefix('+') {
                IntervalEnd::After(parse_time(d).ok_or_else(bad)?)
            } else if e.starts_with('#') {
                return Err(unsupported(
                    "-read_intervals",
                    Some(value),
                    format!(
                        "'-read_intervals {value}' is not supported: an end given as a packet count needs +#N"
                    ),
                ));
            } else {
                IntervalEnd::At(parse_time(e).ok_or_else(bad)?)
            }),
        };
        // A start needs a seek, which is not emulated: `ffprobe` seeks to
        // it and reads on (with `-show_entries`, to the end of the file
        // when no end is given, and from the last packet when the start is
        // past the end).
        // An interval with no end reads all or nothing depending on the
        // sections asked for (`%` reads every packet with `-show_entries`
        // and none with `-show_packets` alone): it is refused too.
        if start.is_some() || end.is_none() {
            return Err(unsupported(
                "-read_intervals",
                Some(value),
                format!(
                    "'-read_intervals {value}' is not supported: coreav ffprobe does not seek; give the interval as %END, %+DURATION or %+#PACKETS"
                ),
            ));
        }
        out.push(Interval {
            start,
            start_relative,
            end,
        });
    }
    if out.len() > 1 {
        return Err(unsupported(
            "-read_intervals",
            Some(value),
            format!(
                "'-read_intervals {value}' is not supported: coreav ffprobe reads one interval"
            ),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::Reason;

    fn args(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn a_common_command() {
        let o = parse(&args(
            "-v error -select_streams v:0 -show_entries stream=width,height -of csv=s=x:p=0 in.mp4",
        ))
        .unwrap();
        assert_eq!(o.input, "in.mp4");
        assert_eq!(
            o.select_streams,
            Some(StreamSpec::Kind(Kind2::Video, Some(0)))
        );
        assert_eq!(
            o.writer,
            Kind::Compact {
                sep: b'x',
                nokey: true,
                escape: Escape::Csv,
                print_section: false
            }
        );
        assert!(o.selection.entry(Sid::Stream, "width"));
        assert!(!o.selection.entry(Sid::Stream, "index"));
    }

    #[test]
    fn refusals_name_the_option() {
        let Err(Refusal::Unsupported(refusal)) = parse(&args("-show_frames in.mp4")) else {
            panic!()
        };
        assert!(refusal.message.contains("-show_frames"));
        assert_eq!(refusal.reason, Reason::Option("-show_frames".into(), None));
        assert!(matches!(
            parse(&args("-of xml in.mp4")),
            Err(Refusal::Unsupported(_))
        ));
        assert!(matches!(
            parse(&args("-of default=nw=1=nk=1 in.mp4")),
            Err(Refusal::Unsupported(_))
        ));
        assert!(matches!(
            parse(&args("-show_entries frame=pict_type in.mp4")),
            Err(Refusal::Unsupported(_))
        ));
        assert!(matches!(
            parse(&args("-f lavfi -i x")),
            Err(Refusal::Unsupported(_))
        ));
        assert!(matches!(
            parse(&args("-show_streams")),
            Err(Refusal::Usage(_))
        ));
        assert!(matches!(
            parse(&args("a.mp4 b.mp4")),
            Err(Refusal::Usage(_))
        ));
        assert!(matches!(
            parse(&args("-show_streams -of")),
            Err(Refusal::Usage(_))
        ));
    }

    #[test]
    fn intervals() {
        for refused in ["999999", "%", "+0", "10%20"] {
            assert!(matches!(
                parse_intervals(refused),
                Err(Refusal::Unsupported(_))
            ));
        }
        let i = parse_intervals("%+#2").unwrap();
        assert_eq!(i.first().unwrap().end, Some(IntervalEnd::Packets(2)));
        assert!(matches!(
            parse_intervals("10%+5,1:00%1:30"),
            Err(Refusal::Unsupported(_))
        ));
        assert!(matches!(
            parse_intervals("10%20"),
            Err(Refusal::Unsupported(_))
        ));
    }
}
