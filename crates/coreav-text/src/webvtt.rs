//! Writing WebVTT (W3C WebVTT: The Web Video Text Tracks Format): the file
//! header, cue timings and cue text with its escapes.

use std::io::{self, Write};

/// The file header and the blank line after it (WebVTT 4.1): no BOM and
/// nothing after `WEBVTT` on its line.
pub(crate) const HEADER: &[u8] = b"WEBVTT\n\n";

/// Writes `ms` milliseconds as a WebVTT timestamp (WebVTT 4.5): hours,
/// always written and at least two digits, then two-digit minutes and
/// seconds and three-digit milliseconds.
pub(crate) fn timestamp(ms: u64) -> String {
    let millis = ms % 1000;
    let seconds = (ms / 1000) % 60;
    let minutes = (ms / 60_000) % 60;
    let hours = ms / 3_600_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

/// Cue text from plain text (WebVTT 4.2.2, cue text):
///
/// - `&`, `<` and `>` are escaped (`&amp;`, `&lt;`, `&gt;`, which every
///   version of WebVTT accepts), so no tag, character reference or `-->`
///   can appear;
/// - the line breaks of 3GPP TS 26.245 5.11 (LF, U+2028, U+2029, and CR,
///   U+0085 and CR LF) become LF, the WebVTT line terminator;
/// - lines that are empty or hold only white space are removed, since an
///   empty line would end the cue;
/// - NUL characters are replaced by U+FFFD, as a WebVTT parser would
///   (sample text never holds one: it ends at the first).
///
/// `None` when no line remains: the sample shows nothing.
pub fn cue_text(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let breaks = ['\r', '\n', '\u{2028}', '\u{2029}', '\u{0085}'];
    for line in text.split("\r\n").flat_map(|l| l.split(breaks)) {
        if line.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        for c in line.chars() {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '\0' => out.push('\u{FFFD}'),
                c => out.push(c),
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Writes one cue: its timings line and its text, then a blank line. `text`
/// comes from [`cue_text`].
pub(crate) fn write_cue<W: Write + ?Sized>(
    out: &mut W,
    start_ms: u64,
    end_ms: u64,
    text: &str,
) -> io::Result<u64> {
    let timings = format!("{} --> {}\n", timestamp(start_ms), timestamp(end_ms));
    out.write_all(timings.as_bytes())?;
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n\n")?;
    Ok((timings.len() as u64)
        .saturating_add(text.len() as u64)
        .saturating_add(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0), "00:00:00.000");
        assert_eq!(timestamp(500), "00:00:00.500");
        assert_eq!(timestamp(61_001), "00:01:01.001");
        assert_eq!(timestamp(3_600_000), "01:00:00.000");
        assert_eq!(timestamp(360_000_000), "100:00:00.000");
    }

    #[test]
    fn cue_text_is_escaped_and_has_no_empty_line() {
        assert_eq!(cue_text("a & b").as_deref(), Some("a &amp; b"));
        assert_eq!(
            cue_text("<b>x</b>").as_deref(),
            Some("&lt;b&gt;x&lt;/b&gt;")
        );
        assert_eq!(cue_text("a --> b").as_deref(), Some("a --&gt; b"));
        assert_eq!(
            cue_text("one\r\ntwo\rthree\nfour").as_deref(),
            Some("one\ntwo\nthree\nfour")
        );
        assert_eq!(cue_text("one\n\n\ntwo\n").as_deref(), Some("one\ntwo"));
        assert_eq!(
            cue_text("a\u{2028}b\u{2029}c\u{0085}d").as_deref(),
            Some("a\nb\nc\nd")
        );
        assert_eq!(cue_text("a\0b").as_deref(), Some("a\u{FFFD}b"));
        assert_eq!(cue_text(" a \n \t\nb").as_deref(), Some(" a \nb"));
        assert_eq!(cue_text(""), None);
        assert_eq!(cue_text("\n\r\n"), None);
        assert_eq!(cue_text(" "), None);
        assert_eq!(cue_text("\r \u{2028}\t"), None);
    }
}
