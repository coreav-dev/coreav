//! `coreav split` (`docs/design/split.md`): output name patterns, time
//! lists, the split of a file with the library defaults, and the
//! `coreav.split/1` summary.

use crate::clip::{left_out, parse_time, seconds_as_time, stream};
use crate::output::WriteFailure;
use crate::pretty::{write_array, write_indented};
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Rational};
use coreav_isobmff_write::{Segment, SplitOptions, SplitSummary, split_mp4};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Most digits a pattern may pad the segment number to.
const MAX_WIDTH: usize = 9;

/// An output name with one segment number in it: `%d`, or `%0Nd` for a
/// number padded with zeros to `N` digits (1 to 9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    prefix: String,
    width: usize,
    suffix: String,
}

impl Pattern {
    /// Parses a pattern; `None` unless it holds exactly one `%d` or `%0Nd`
    /// and no other `%`.
    pub fn parse(text: &str) -> Option<Self> {
        let (prefix, rest) = text.split_once('%')?;
        let (width, suffix) = if let Some(suffix) = rest.strip_prefix('d') {
            (0, suffix)
        } else {
            let digits = rest.strip_prefix('0')?;
            let end = digits.find('d')?;
            let (width, suffix) = digits.split_at(end);
            if !width.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let width: usize = width.parse().ok()?;
            if !(1..=MAX_WIDTH).contains(&width) {
                return None;
            }
            (width, suffix.get(1..)?)
        };
        if suffix.contains('%') {
            return None;
        }
        Some(Self {
            prefix: prefix.to_owned(),
            width,
            suffix: suffix.to_owned(),
        })
    }

    /// The name of segment `index`.
    pub fn name(&self, index: usize) -> String {
        format!(
            "{}{index:0width$}{}",
            self.prefix,
            self.suffix,
            width = self.width
        )
    }
}

/// Parses a comma-separated list of times in decimal seconds, as
/// [`parse_time`] reads each; `None` for anything else, an empty list
/// included.
pub fn parse_times(text: &str) -> Option<Vec<Rational>> {
    text.split(',').map(parse_time).collect()
}

/// Splits the file at `path` with the library defaults, writing segment
/// `n` to the writer `open(n)` returns.
pub fn run<W: Write, F: FnMut(usize) -> std::io::Result<W>>(
    path: &Path,
    options: &SplitOptions,
    allowlist: &Allowlist,
    open: F,
) -> Result<SplitSummary, WriteFailure> {
    let source = BufferedSource::new(FileSource::open(path)?);
    Ok(split_mp4(
        source,
        &Limits::default(),
        allowlist,
        options,
        open,
    )?)
}

/// Splits any [`Source`](coreav_isobmff::Source), as [`run`] does for a
/// file.
pub fn run_source<S: coreav_isobmff::Source, W: Write, F: FnMut(usize) -> std::io::Result<W>>(
    source: S,
    options: &SplitOptions,
    allowlist: &Allowlist,
    open: F,
) -> Result<SplitSummary, WriteFailure> {
    Ok(split_mp4(
        source,
        &Limits::default(),
        allowlist,
        options,
        open,
    )?)
}

/// Writes the `coreav.split/1` summary (`docs/schema/split-v1.md`) to
/// `out` as pretty-printed JSON, a segment at a time (`crate::pretty`);
/// `names` holds each segment's output name.
pub fn write_document<W: Write + ?Sized>(
    out: &mut W,
    summary: &SplitSummary,
    names: &[String],
) -> std::io::Result<()> {
    let boxes: Vec<String> = summary
        .boxes_left_out
        .iter()
        .map(ToString::to_string)
        .collect();
    // The keys in serde_json's order (sorted).
    out.write_all(b"{\n  \"boxes_left_out\": ")?;
    write_indented(out, &json!(boxes), "  ")?;
    out.write_all(b",\n  \"left_out\": ")?;
    write_indented(out, &Value::Array(left_out(&summary.left_out)), "  ")?;
    out.write_all(b",\n  \"schema\": \"coreav.split/1\",\n  \"segments\": ")?;
    write_array(
        out,
        summary
            .segments
            .iter()
            .zip(names)
            .enumerate()
            .map(|(index, (segment, name))| segment_document(index, segment, name)),
    )?;
    out.write_all(b"\n}\n")
}

/// One segment of the summary.
fn segment_document(index: usize, segment: &Segment, name: &str) -> Value {
    let timescale = segment.video.timescale;
    let mut streams = vec![stream("video", &segment.video)];
    if let Some(audio) = &segment.audio {
        streams.push(stream("audio", audio));
    }
    json!({
        "index": index,
        "output": {"path": name, "bytes": segment.bytes_written},
        "start": seconds_as_time(segment.start, timescale),
        "video_start": seconds_as_time(segment.video_start, timescale),
        "end": seconds_as_time(segment.end, timescale),
        "streams": streams,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let p = Pattern::parse("out/seg-%03d.mp4").unwrap();
        assert_eq!(p.name(7), "out/seg-007.mp4");
        assert_eq!(p.name(1234), "out/seg-1234.mp4");
        assert_eq!(
            Pattern::parse("%d.mp4").map(|p| p.name(12)),
            Some("12.mp4".into())
        );
        for bad in [
            "seg.mp4", "%3d.mp4", "%010d", "%00d", "%d-%d", "100%", "%s", "%0d", "a%",
        ] {
            assert_eq!(Pattern::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_streamed_summary_is_the_whole_document_pretty_printed() {
        use coreav_isobmff::{Codec, FourCc};
        use coreav_isobmff_write::{ClipTrack, LeftOut, LeftOutReason};
        let q = |n, d| Rational::new(n, d).unwrap();
        let track = |stream_index, codec, timescale| ClipTrack {
            stream_index,
            track_id: 1,
            codec,
            first_sample: 3,
            samples: 25,
            timescale,
            duration: 25_000,
        };
        let segment = |n: i128| Segment {
            start: q(n, 1),
            video_start: q(n, 1),
            end: q(n + 1, 1),
            video: track(0, Codec::H264, 12_800),
            audio: (n > 0).then(|| track(1, Codec::Aac, 48_000)),
            bytes_written: 1_000,
        };
        let names: Vec<String> = (0..3).map(|n| format!("seg-{n}.mp4")).collect();
        for (segments, left_out_tracks, boxes) in [
            (vec![], vec![], vec![]),
            (
                vec![segment(0), segment(1), segment(2)],
                vec![LeftOut {
                    stream_index: 2,
                    track_id: 3,
                    reason: LeftOutReason::NotSelected,
                }],
                vec![FourCc(*b"pasp"), FourCc(*b"colr")],
            ),
        ] {
            let summary = SplitSummary {
                segments,
                left_out: left_out_tracks,
                boxes_left_out: boxes,
            };
            let whole = json!({
                "schema": "coreav.split/1",
                "segments": summary
                    .segments
                    .iter()
                    .zip(&names)
                    .enumerate()
                    .map(|(n, (segment, name))| segment_document(n, segment, name))
                    .collect::<Vec<_>>(),
                "left_out": left_out(&summary.left_out),
                "boxes_left_out": summary
                    .boxes_left_out
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            });
            let mut out = Vec::new();
            write_document(&mut out, &summary, &names).unwrap();
            let expected = serde_json::to_string_pretty(&whole).unwrap() + "\n";
            assert_eq!(String::from_utf8(out).unwrap(), expected);
        }
    }

    #[test]
    fn time_lists() {
        let q = |n, d| Rational::new(n, d).unwrap();
        assert_eq!(parse_times("10,20.5"), Some(vec![q(10, 1), q(41, 2)]));
        for bad in ["", "1,", ",1", "1,,2", "1;2", "1, 2"] {
            assert_eq!(parse_times(bad), None, "{bad}");
        }
    }
}
