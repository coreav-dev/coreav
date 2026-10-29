//! The list file of `ffmpeg -f concat` (the concat demuxer of FFmpeg's
//! formats documentation, "Syntax" and the `safe` option): the files it
//! names, in order, for `join::run`. Only `file` directives and the
//! `ffconcat version 1.0` header are taken; every other directive changes
//! what the demuxer reads or where it places a file, and is refused by name.

use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Most bytes of a list file that are read. It holds ten thousand files
/// named by 100-byte paths, and it bounds every line, every path and the
/// number of files.
pub const MAX_LIST_BYTES: u64 = 1 << 20;

/// Why a concat list cannot be joined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListError {
    /// The list file cannot be read, or is not UTF-8.
    Read(String),
    /// A directive other than `file` and `ffconcat`; `known` when the
    /// concat demuxer documents it (`duration`, `inpoint`, ...).
    Directive {
        /// The line, from 1.
        line: usize,
        /// The directive as written.
        name: String,
        /// Whether the concat demuxer documents it.
        known: bool,
    },
    /// A `file` directive without a path.
    NoPath {
        /// The line, from 1.
        line: usize,
    },
    /// An `ffconcat` line other than `ffconcat version 1.0`.
    Version {
        /// The line, from 1.
        line: usize,
    },
    /// A path that names a protocol (`scheme:...`): only local files are
    /// joined.
    Protocol {
        /// The line, from 1.
        line: usize,
        /// The path as the list gives it.
        path: String,
    },
    /// A path the demuxer's `safe` option (on unless `-safe 0`) refuses.
    Unsafe {
        /// The line, from 1.
        line: usize,
        /// The path as the list gives it.
        path: String,
    },
    /// No `file` directive.
    Empty,
    /// A list file longer than [`MAX_LIST_BYTES`]; nothing after that
    /// limit is read.
    TooLarge,
}

impl fmt::Display for ListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(reason) => write!(f, "cannot read the concat list: {reason}"),
            Self::TooLarge => write!(f, "the concat list is longer than {MAX_LIST_BYTES} bytes"),
            Self::Directive {
                line,
                name,
                known: true,
            } => write!(
                f,
                "concat list line {line}: directive '{name}' is not supported"
            ),
            Self::Directive { line, name, .. } => {
                write!(f, "concat list line {line}: unknown directive '{name}'")
            }
            Self::NoPath { line } => write!(f, "concat list line {line}: 'file' without a path"),
            Self::Version { line } => {
                write!(
                    f,
                    "concat list line {line}: only 'ffconcat version 1.0' is read"
                )
            }
            Self::Protocol { line, path } => write!(
                f,
                "concat list line {line}: '{path}' names a protocol; only local files are joined"
            ),
            Self::Unsafe { line, path } => write!(
                f,
                "concat list line {line}: unsafe file name '{path}' (accepted with -safe 0)"
            ),
            Self::Empty => write!(f, "the concat list names no file"),
        }
    }
}

impl std::error::Error for ListError {}

/// The directives the concat demuxer documents besides `file` and
/// `ffconcat`.
const DOCUMENTED: &[&str] = &[
    "duration",
    "inpoint",
    "outpoint",
    "file_packet_metadata",
    "file_packet_meta",
    "option",
    "stream",
    "exact_stream_id",
    "stream_meta",
    "stream_codec",
    "stream_extradata",
    "chapter",
];

/// Reads the list file at `list` and returns the files it names, in
/// order: each relative path resolved against the list's directory, as the
/// concat demuxer resolves it. With `safe` (the demuxer's default, off
/// with `-safe 0`), a path must be relative, and each of its components
/// non-empty, made of letters, digits, `.`, `_` and `-`, and not start
/// with `.`. At most [`MAX_LIST_BYTES`] and one byte are read, so a
/// longer file, or an endless one such as a device, is refused.
pub fn read(list: &Path, safe: bool) -> Result<Vec<PathBuf>, ListError> {
    let error = |e: std::io::Error| ListError::Read(e.to_string());
    let mut bytes = Vec::new();
    File::open(list)
        .map_err(error)?
        .take(MAX_LIST_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > MAX_LIST_BYTES) {
        return Err(ListError::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| ListError::Read("not UTF-8".into()))?;
    let dir = list.parent().unwrap_or(Path::new(""));
    Ok(files(&text, safe)?
        .into_iter()
        .map(|path| dir.join(path))
        .collect())
}

/// The paths of a list's `file` directives, as written.
fn files(text: &str, safe: bool) -> Result<Vec<String>, ListError> {
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index.saturating_add(1);
        let mut rest = raw;
        let Some(keyword) = word(&mut rest) else {
            continue;
        };
        if keyword.starts_with('#') {
            continue;
        }
        match keyword {
            "file" => {
                let path = token(&mut rest)
                    .filter(|p| !p.is_empty())
                    .ok_or(ListError::NoPath { line })?;
                if protocol(&path) {
                    return Err(ListError::Protocol { line, path });
                }
                if safe && !portable(&path) {
                    return Err(ListError::Unsafe { line, path });
                }
                out.push(path);
            }
            "ffconcat" => {
                if token(&mut rest).as_deref() != Some("version")
                    || token(&mut rest).as_deref() != Some("1.0")
                {
                    return Err(ListError::Version { line });
                }
            }
            name => {
                return Err(ListError::Directive {
                    line,
                    name: name.to_owned(),
                    known: DOCUMENTED.contains(&name),
                });
            }
        }
    }
    if out.is_empty() {
        return Err(ListError::Empty);
    }
    Ok(out)
}

/// The next word of `rest`, as written: a directive's name is not
/// unquoted or unescaped (black box: `'file' a.mp4` and `fi\le a.mp4` are
/// unknown directives to `ffmpeg`). `None` at the end of the line.
fn word<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let trimmed = rest.trim_start_matches([' ', '\t', '\r']);
    let end = trimmed.find([' ', '\t', '\r']).unwrap_or(trimmed.len());
    let (word, after) = trimmed.split_at(end);
    *rest = after;
    (!word.is_empty()).then_some(word)
}

/// The next token of `rest`, by FFmpeg's quoting and escaping rules
/// (ffmpeg-utils, "Quoting and escaping"): whitespace ends a token, `\`
/// takes the next character as it is (a `\` that ends the line stays, as
/// `ffmpeg` keeps it in the name), and text between `'` quotes is
/// taken as it is. `None` at the end of the line.
fn token(rest: &mut &str) -> Option<String> {
    let trimmed = rest.trim_start_matches([' ', '\t', '\r']);
    if trimmed.is_empty() {
        *rest = trimmed;
        return None;
    }
    let mut out = String::new();
    let mut chars = trimmed.char_indices();
    let mut end = trimmed.len();
    while let Some((at, c)) = chars.next() {
        match c {
            ' ' | '\t' | '\r' => {
                end = at;
                break;
            }
            '\\' => out.push(chars.next().map_or('\\', |(_, next)| next)),
            '\'' => {
                for (_, quoted) in chars.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    out.push(quoted);
                }
            }
            c => out.push(c),
        }
    }
    *rest = trimmed.get(end..).unwrap_or("");
    Some(out)
}

/// Whether a path starts with a protocol name and a colon (`file:`,
/// `https:`), as the concat demuxer reads it.
fn protocol(path: &str) -> bool {
    path.split('/')
        .next()
        .is_some_and(|first| first.contains(':'))
}

/// The `safe` rule: relative, every component non-empty, of the portable
/// character set, not starting with `.`.
fn portable(path: &str) -> bool {
    !path.starts_with('/')
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(text: &str, safe: bool) -> Result<Vec<String>, ListError> {
        files(text, safe)
    }

    #[test]
    fn files_in_order() {
        let list = "ffconcat version 1.0\n# a comment\n   # indented\n\nfile a.mp4\r\n\tfile\tsub/b-1_x.mp4   \nfile c.mp4 # trailing\nfile d.mp4";
        assert_eq!(
            paths(list, true).unwrap(),
            vec!["a.mp4", "sub/b-1_x.mp4", "c.mp4", "d.mp4"]
        );
    }

    #[test]
    fn quoting_and_escaping() {
        let list = "file 'sp ace/c d.mp4'\nfile sp\\ ace/c\\ d.mp4\nfile 'q'\\''x.mp4'\nfile b'c'.mp4\nfile sp ace.mp4";
        assert_eq!(
            paths(list, false).unwrap(),
            vec![
                "sp ace/c d.mp4",
                "sp ace/c d.mp4",
                "q'x.mp4",
                "bc.mp4",
                "sp"
            ]
        );
    }

    #[test]
    fn directives_are_read_as_written_and_a_final_backslash_stays() {
        for (list, name) in [("'file' a.mp4\n", "'file'"), ("fi\\le a.mp4\n", "fi\\le")] {
            assert_eq!(
                paths(list, false),
                Err(ListError::Directive {
                    line: 1,
                    name: name.into(),
                    known: false
                }),
                "{list:?}"
            );
        }
        assert_eq!(paths("file a.mp4\\\n", false).unwrap(), vec!["a.mp4\\"]);
        assert!(matches!(
            paths("file a.mp4\\\n", true),
            Err(ListError::Unsafe { line: 1, .. })
        ));
    }

    #[test]
    fn the_safe_rule() {
        for unsafe_path in [
            "/abs/a.mp4",
            "./a.mp4",
            "../a.mp4",
            ".h.mp4",
            "sub//b.mp4",
            "'sp ace.mp4'",
            "a+b.mp4",
            "é.mp4",
            "a#b.mp4",
        ] {
            let list = format!("file {unsafe_path}\n");
            assert!(
                matches!(paths(&list, true), Err(ListError::Unsafe { line: 1, .. })),
                "{unsafe_path}"
            );
            assert!(paths(&list, false).is_ok(), "{unsafe_path}");
        }
    }

    #[test]
    fn refusals_name_the_line_and_the_cause() {
        let cases: [(&str, ListError); 7] = [
            (
                "file a.mp4\nduration 1\n",
                ListError::Directive {
                    line: 2,
                    name: "duration".into(),
                    known: true,
                },
            ),
            (
                "file a.mp4\ninpoint 1\n",
                ListError::Directive {
                    line: 2,
                    name: "inpoint".into(),
                    known: true,
                },
            ),
            (
                "FILE a.mp4\n",
                ListError::Directive {
                    line: 1,
                    name: "FILE".into(),
                    known: false,
                },
            ),
            ("file\n", ListError::NoPath { line: 1 }),
            ("file ''\n", ListError::NoPath { line: 1 }),
            (
                "ffconcat version 2.0\nfile a.mp4\n",
                ListError::Version { line: 1 },
            ),
            ("# nothing\nffconcat version 1.0\n", ListError::Empty),
        ];
        for (list, expected) in cases {
            assert_eq!(paths(list, true), Err(expected), "{list:?}");
        }
        assert_eq!(
            paths("file file:a.mp4\n", false),
            Err(ListError::Protocol {
                line: 1,
                path: "file:a.mp4".into()
            })
        );
        assert!(
            ListError::Directive {
                line: 2,
                name: "duration".into(),
                known: true
            }
            .to_string()
            .contains("'duration'")
        );
    }

    #[test]
    fn paths_resolve_against_the_list() {
        let dir = std::env::temp_dir().join(format!("coreav-concat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let list = dir.join("list.txt");
        std::fs::write(&list, "file a.mp4\nfile sub/b.mp4\n").unwrap();
        assert_eq!(
            read(&list, true).unwrap(),
            vec![dir.join("a.mp4"), dir.join("sub/b.mp4")]
        );
        std::fs::write(&list, b"file \xff.mp4\n").unwrap();
        assert!(matches!(read(&list, false), Err(ListError::Read(_))));
        let mut long = "file a.mp4\n".to_owned();
        long.push_str(&"#".repeat(usize::try_from(MAX_LIST_BYTES).unwrap() - long.len()));
        std::fs::write(&list, &long).unwrap();
        assert_eq!(read(&list, true).unwrap(), vec![dir.join("a.mp4")]);
        long.push('#');
        std::fs::write(&list, &long).unwrap();
        assert_eq!(read(&list, true), Err(ListError::TooLarge));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_endless_list_is_refused_at_the_limit() {
        assert_eq!(read(Path::new("/dev/zero"), true), Err(ListError::TooLarge));
    }
}
