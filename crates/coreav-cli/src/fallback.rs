//! The FFmpeg fallback of `coreav ffprobe` and `coreav ffmpeg`
//! (`docs/design/compat-cli.md`, "Fallback to FFmpeg"): why a command is
//! not native, and the line `COREAV_FALLBACK_LOG` gets for it. The command
//! line in that line is redacted to the public names of `names.rs`, so the
//! log holds nothing its user could not share. The binary finds and runs
//! the program (`fallback_cmd.rs`).

mod names;

use std::path::Path;

use serde_json::{Value, json};

/// Whether this process can run the file at `path`: on Unix `access` with
/// `X_OK`, so that a file with an execute bit only for others is not one.
pub fn executable(path: &Path) -> bool {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return false;
    }
    #[cfg(unix)]
    {
        rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The tool a command line is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// `ffprobe`.
    Ffprobe,
    /// `ffmpeg`.
    Ffmpeg,
}

impl Tool {
    /// The program's name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ffprobe => "ffprobe",
            Self::Ffmpeg => "ffmpeg",
        }
    }
}

/// What a refusal names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// An option as given (`-vf`, `-c:v`), with its value when the value is
    /// what is refused (`-f lavfi`).
    Option(String, Option<String>),
    /// What the command asks for, in fixed words that hold no user data
    /// (`no section`, `standard input`).
    Property(&'static str),
}

impl Reason {
    /// The reason as the notice and the log print it, redacted as the
    /// command line is.
    pub fn redacted(&self, tool: Tool) -> String {
        match self {
            Self::Option(option, value) => {
                let parts: Vec<String> = std::iter::once(option.clone())
                    .chain(value.clone())
                    .collect();
                redact(tool, &parts).join(" ")
            }
            Self::Property(words) => (*words).to_owned(),
        }
    }
}

/// A command that is not native: refused with exit status 4, or run by
/// FFmpeg in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotNative {
    /// What the refusal names.
    pub reason: Reason,
    /// The refusal as printed, which names the reason.
    pub message: String,
}

impl NotNative {
    /// A refusal of an option, or of one of its values.
    pub fn option(option: &str, value: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            reason: Reason::Option(option.to_owned(), value.map(str::to_owned)),
            message: message.into(),
        }
    }

    /// A refusal of what the command asks for.
    pub fn property(words: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason: Reason::Property(words),
            message: message.into(),
        }
    }
}

/// Why nothing ran in place of a command that is not native.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotRun {
    /// `COREAV_STRICT_SAFE` or `--strict-safe`.
    StrictSafe,
    /// No program on `PATH` other than this one.
    NoProgram,
    /// `COREAV_FALLBACK` names no program that can run.
    Unusable,
    /// `COREAV_FALLBACK_ACTIVE`: this run is a fallback's.
    Guard,
}

impl NotRun {
    fn name(self) -> &'static str {
        match self {
            Self::StrictSafe => "strict_safe",
            Self::NoProgram => "no_program",
            Self::Unusable => "fallback_unusable",
            Self::Guard => "recursion_guard",
        }
    }
}

/// The JSON line `COREAV_FALLBACK_LOG` gets for a command that is not
/// native, newline included: `ran` is the program that ran instead, or why
/// none did.
pub fn log_line(
    time: &str,
    tool: Tool,
    args: &[String],
    reason: &Reason,
    ran: Result<&str, NotRun>,
) -> String {
    let mut line = json!({
        "time": time,
        "coreav": env!("CARGO_PKG_VERSION"),
        "tool": tool.name(),
        "args": redact(tool, args),
        "reason": reason.redacted(tool),
        "ran": ran.ok(),
    });
    if let (Err(not_run), Value::Object(map)) = (ran, &mut line) {
        map.insert("not_run".to_owned(), Value::from(not_run.name()));
    }
    format!("{line}\n")
}

/// The command line with every part that is not a listed public name
/// replaced: file names and other bare arguments by `INPUT` or `OUTPUT`,
/// the values of file options by `FILE`, an option off the listing by
/// `-OPTION`, a stream specifier other than a type or an index by `SPEC`,
/// and every other value by `VALUE` (or, in `-show_entries`, `SECTION` and
/// `KEY`).
pub fn redact(tool: Tool, args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if !is_option(arg) {
            out.push(match tool {
                Tool::Ffprobe => "INPUT".to_owned(),
                Tool::Ffmpeg => "OUTPUT".to_owned(),
            });
            continue;
        }
        let (head, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), false), |(h, _)| (h, true));
        let (base, spec) = head
            .split_once(':')
            .map_or((head, None), |(b, s)| (b, Some(s)));
        let takes_value = takes_value(tool, base);
        let mut name = if takes_value.is_some() {
            base
        } else {
            "-OPTION"
        }
        .to_owned();
        if let Some(spec) = spec {
            name.push(':');
            name.push_str(if stream_spec(spec) { spec } else { "SPEC" });
        }
        out.push(name);
        if inline || takes_value == Some(false) {
            continue;
        }
        if let Some(value) = args.next() {
            out.push(redact_value(base, value));
        }
    }
    out
}

/// Whether an argument is an option, as FFmpeg reads one: a negative
/// number is a value.
fn is_option(arg: &str) -> bool {
    arg.starts_with('-') && arg != "-" && !arg[1..].starts_with(|c: char| c.is_ascii_digit())
}

/// Whether a listed option takes a value; `None` for one off the listing. A
/// flag's `-no` form (`-nostdin`) is a flag.
fn takes_value(tool: Tool, option: &str) -> Option<bool> {
    let (values, flags) = match tool {
        Tool::Ffprobe => (names::FFPROBE_VALUES, names::FFPROBE_FLAGS),
        Tool::Ffmpeg => (names::FFMPEG_VALUES, names::FFMPEG_FLAGS),
    };
    if listed(values, option) {
        return Some(true);
    }
    if listed(flags, option) {
        return Some(false);
    }
    let flag = format!("-{}", option.strip_prefix("-no")?);
    listed(flags, &flag).then_some(false)
}

/// Whether `name` is a line of `list`.
fn listed(list: &str, name: &str) -> bool {
    list.split('\n').any(|line| line == name)
}

/// The value of an option, kept when it is a public name or a time or an
/// integer where the option takes one (the design note's table).
fn redact_value(option: &str, value: &str) -> String {
    let keep = match option {
        "-i" => return "INPUT".to_owned(),
        "-segment_list" | "-hls_segment_filename" => return "FILE".to_owned(),
        "-show_entries" => return entries(value),
        "-ss" | "-sseof" | "-t" | "-to" | "-segment_time" | "-hls_time" => is_time(value),
        "-threads"
        | "-hls_list_size"
        | "-max_muxing_queue_size"
        | "-reset_timestamps"
        | "-safe" => integer(value.strip_prefix('-').unwrap_or(value)),
        "-map" | "-select_streams" | "-map_metadata" | "-map_chapters" => {
            let value = value.strip_prefix('-').unwrap_or(value);
            stream_spec(value.strip_suffix('?').unwrap_or(value))
        }
        "-c" | "-codec" | "-vcodec" | "-acodec" | "-scodec" => {
            value == "copy" || listed(names::ENCODERS, value)
        }
        "-f" | "-segment_format" => listed(names::FORMATS, value),
        "-v" | "-loglevel" => named(value, LOG_LEVELS),
        "-movflags" => named(value, names::MOVFLAGS),
        "-fflags" => named(value, names::FFLAGS),
        "-avoid_negative_ts" => named(value, names::AVOID_NEGATIVE_TS),
        "-hls_playlist_type" => named(value, names::HLS_PLAYLIST_TYPE),
        "-of" | "-print_format" | "-output_format" => writer(value),
        _ => false,
    };
    if keep { value } else { "VALUE" }.to_owned()
}

/// The level names and flags of `-loglevel` in FFmpeg's documentation.
const LOG_LEVELS: &str = "datetime\ndebug\nerror\nfatal\ninfo\nlevel\npanic\nquiet\nrepeat\ntime\ntrace\nverbose\nwarning";

/// Names of `list` or integers, joined by `+` or `-`.
fn named(value: &str, list: &str) -> bool {
    let mut parts = value.split(['+', '-']).filter(|p| !p.is_empty()).peekable();
    parts.peek().is_some() && parts.all(|p| integer(p) || listed(list, p))
}

fn integer(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// A stream specifier of a file index, a type and an index only (`0`,
/// `v`, `0:a:1`).
fn stream_spec(text: &str) -> bool {
    text.split(':')
        .all(|part| integer(part) || matches!(part, "v" | "V" | "a" | "s" | "d" | "t"))
}

/// FFmpeg's time syntax: `[-][HH:]MM:SS[.m...]`, with minutes and seconds
/// below 60, or `[-]S+[.m...][s|ms|us]` (`tools/compat-corpus/classify.py`).
fn is_time(value: &str) -> bool {
    let value = value.strip_prefix('-').unwrap_or(value);
    let decimal = |text: &str| match text.split_once('.') {
        Some((whole, fraction)) => integer(whole) && integer(fraction),
        None => integer(text),
    };
    let sixty = |text: &str| integer(text) && (text.len() == 1 || text.len() == 2 && text < "60");
    let parts: Vec<&str> = value.split(':').collect();
    match parts.as_slice() {
        [seconds] => {
            let number = seconds
                .strip_suffix("ms")
                .or_else(|| seconds.strip_suffix("us"))
                .or_else(|| seconds.strip_suffix('s'))
                .unwrap_or(seconds);
            decimal(number)
        }
        [clock @ .., minutes, seconds] if clock.len() <= 1 => {
            let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, "0"));
            clock.iter().all(|h| integer(h)) && sixty(minutes) && sixty(whole) && integer(fraction)
        }
        _ => false,
    }
}

/// An `ffprobe` writer of the vocabulary, with its documented settings only.
fn writer(value: &str) -> bool {
    let (name, settings) = value.split_once('=').unwrap_or((value, ""));
    matches!(name, "default" | "compact" | "csv" | "flat" | "json")
        && settings
            .split(':')
            .filter(|s| !s.is_empty())
            .all(|setting| {
                setting.split_once('=').is_some_and(|(key, v)| {
                    WRITER_SETTINGS.contains(&key)
                        && (matches!(v, "0" | "1" | "c" | "csv" | "none") || v.chars().count() == 1)
                })
            })
}

const WRITER_SETTINGS: [&str; 15] = [
    "c",
    "compact",
    "e",
    "escape",
    "h",
    "hierarchical",
    "item_sep",
    "nk",
    "nokey",
    "noprint_wrappers",
    "nw",
    "p",
    "print_section",
    "s",
    "sep_char",
];

/// `-show_entries SECTION[=KEY,...][:...]` with every section off `ffprobe`'s
/// listing replaced by `SECTION`, and every key of a tags section or off
/// the keys `coreav ffprobe` prints by `KEY`.
fn entries(value: &str) -> String {
    let part = |part: &str| {
        let (section, keys) = part
            .split_once('=')
            .map_or((part, None), |(s, k)| (s, Some(k)));
        let tags = section.ends_with("tags");
        let mut out = if listed(names::SECTIONS, section) {
            section
        } else {
            "SECTION"
        }
        .to_owned();
        if let Some(keys) = keys {
            let keys: Vec<&str> = keys
                .split(',')
                .map(|key| {
                    if key.is_empty() || !tags && listed(names::KEYS, key) {
                        key
                    } else {
                        "KEY"
                    }
                })
                .collect();
            out.push('=');
            out.push_str(&keys.join(","));
        }
        out
    };
    value.split(':').map(part).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    fn redacted(tool: Tool, text: &str) -> String {
        redact(tool, &line(text)).join(" ")
    }

    #[test]
    fn listings_hold_one_name_per_line() {
        for list in [
            names::FFMPEG_VALUES,
            names::FFMPEG_FLAGS,
            names::FFPROBE_VALUES,
            names::FFPROBE_FLAGS,
            LOG_LEVELS,
            names::ENCODERS,
            names::FORMATS,
            names::MOVFLAGS,
            names::FFLAGS,
            names::AVOID_NEGATIVE_TS,
            names::HLS_PLAYLIST_TYPE,
            names::SECTIONS,
            names::KEYS,
        ] {
            for name in list.split('\n') {
                assert!(
                    !name.is_empty() && !name.contains(char::is_whitespace),
                    "{name:?}"
                );
            }
        }
    }

    #[test]
    fn public_names_are_kept() {
        assert_eq!(
            redacted(
                Tool::Ffmpeg,
                "-hide_banner -ss 00:01:02.5 -i in.mp4 -t 10 -map 0:v:0 -map -0:a? -c:v copy -c:a aac -movflags +faststart -f mp4 -y out.mp4"
            ),
            "-hide_banner -ss 00:01:02.5 -i INPUT -t 10 -map 0:v:0 -map -0:a? -c:v copy -c:a aac -movflags +faststart -f mp4 -y OUTPUT"
        );
        assert_eq!(
            redacted(
                Tool::Ffprobe,
                "-v quiet -print_format json -show_entries stream=codec_name,width:format_tags=title -select_streams v:0 -of csv=p=0 in.mp4"
            ),
            "-v quiet -print_format json -show_entries stream=codec_name,width:format_tags=KEY -select_streams v:0 -of csv=p=0 INPUT"
        );
        assert_eq!(
            redacted(
                Tool::Ffmpeg,
                "-loglevel error+repeat -nostdin -vf scale=1:2 -segment_list list.m3u8 -threads 4 -"
            ),
            "-loglevel error+repeat -nostdin -vf VALUE -segment_list FILE -threads 4 OUTPUT"
        );
    }

    #[test]
    fn times_are_ffmpeg_syntax() {
        for time in [
            "10",
            "1.5",
            "-2",
            "90s",
            "500ms",
            "7us",
            "01:02",
            "1:02:03.25",
            "00:59:59",
        ] {
            assert!(is_time(time), "{time}");
        }
        for not in ["", "1:60", "1:2:3:4", "x", "1.", "1e3", "01:02:03.", "now"] {
            assert!(!is_time(not), "{not}");
        }
    }

    /// The design note's regression cases: a private marker that must
    /// appear nowhere in the log line, `args` and `reason` alike.
    #[test]
    fn private_values_never_reach_the_log() {
        let cases: [(Tool, &str, Reason); 12] = [
            (
                Tool::Ffmpeg,
                "-i in.mp4 -c:m:comment:MARKER copy out.mp4",
                Reason::Option("-c:m:comment:MARKER".into(), None),
            ),
            (
                Tool::Ffmpeg,
                "-i in.mp4 -map 0:m:comment:MARKER out.mp4",
                Reason::Option("-map".into(), Some("0:m:comment:MARKER".into())),
            ),
            (
                Tool::Ffprobe,
                "-show_entries format_tags=MARKER in.mp4",
                Reason::Option("-show_entries".into(), Some("format_tags=MARKER".into())),
            ),
            (
                Tool::Ffprobe,
                "-show_entries stream_tags=MARKER in.mp4",
                Reason::Option("-show_entries".into(), Some("stream_tags=MARKER".into())),
            ),
            (
                Tool::Ffmpeg,
                "-i in.mp4 -opt=MARKER out.mp4",
                Reason::Option("-opt=MARKER".into(), None),
            ),
            (
                Tool::Ffmpeg,
                "-headers Authorization:_Bearer_MARKER -i in.mp4 out.mp4",
                Reason::Option(
                    "-headers".into(),
                    Some("Authorization:_Bearer_MARKER".into()),
                ),
            ),
            (
                Tool::Ffprobe,
                "-show_streams https://example.com/a.mp4?token=MARKER",
                Reason::Property("an input other than a file or an HTTP(S) URL"),
            ),
            (
                Tool::Ffmpeg,
                "-i in.mp4 -c:v MARKER out.mp4",
                Reason::Option("-c:v".into(), Some("MARKER".into())),
            ),
            (
                Tool::Ffprobe,
                "-show_streams -of json=MARKER=1 in.mp4",
                Reason::Option("-of".into(), Some("json=MARKER=1".into())),
            ),
            (
                Tool::Ffprobe,
                "-show_streams -of csv=p=MARKER in.mp4",
                Reason::Option("-of".into(), Some("csv=p=MARKER".into())),
            ),
            (
                Tool::Ffmpeg,
                "-i in.mp4 -MARKER x out.mp4",
                Reason::Option("-MARKER".into(), None),
            ),
            (
                Tool::Ffmpeg,
                "-i MARKER.mp4 -metadata title=MARKER -vf drawtext=text=MARKER MARKER.mp4",
                Reason::Option("-vf".into(), Some("drawtext=text=MARKER".into())),
            ),
        ];
        for (tool, command, reason) in cases {
            let args = line(command);
            for ran in [Ok("/usr/bin/ffmpeg"), Err(NotRun::StrictSafe)] {
                let logged = log_line("2026-10-29T08:00:00Z", tool, &args, &reason, ran);
                assert!(!logged.contains("MARKER"), "{command}: {logged}");
                assert!(serde_json::from_str::<Value>(&logged).is_ok());
            }
        }
    }

    #[test]
    fn the_line_says_what_ran_or_why_nothing_did() {
        let args = line("-i in.mp4 -vf scale=640:-2 -c:a copy out.mp4");
        let reason = Reason::Option("-vf".into(), None);
        let ran: Value = serde_json::from_str(&log_line(
            "2026-10-29T08:00:00Z",
            Tool::Ffmpeg,
            &args,
            &reason,
            Ok("/usr/bin/ffmpeg"),
        ))
        .unwrap();
        assert_eq!(
            ran,
            json!({
                "time": "2026-10-29T08:00:00Z",
                "coreav": env!("CARGO_PKG_VERSION"),
                "tool": "ffmpeg",
                "args": ["-i", "INPUT", "-vf", "VALUE", "-c:a", "copy", "OUTPUT"],
                "reason": "-vf",
                "ran": "/usr/bin/ffmpeg",
            })
        );
        let refused: Value = serde_json::from_str(&log_line(
            "t",
            Tool::Ffprobe,
            &line("-show_frames in.mp4"),
            &Reason::Option("-show_frames".into(), None),
            Err(NotRun::Guard),
        ))
        .unwrap();
        assert_eq!(refused["ran"], Value::Null);
        assert_eq!(refused["not_run"], "recursion_guard");
        assert_eq!(refused["reason"], "-show_frames");
    }
}
