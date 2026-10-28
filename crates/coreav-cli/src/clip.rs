//! `coreav clip` (`docs/schema/clip-v1.md`): time parsing, the clip of a file
//! with the library defaults, and the `coreav.clip/1` summary.

use crate::names;
use crate::native::{rational, time};
use crate::output::WriteFailure;
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Rational, SliceSource};
use coreav_isobmff_write::{ClipOptions, ClipSummary, ClipTrack, LeftOut, LeftOutReason, clip_mp4};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Most digits of a time before and after the point.
const MAX_WHOLE_DIGITS: usize = 12;
const MAX_FRACTION_DIGITS: usize = 9;

/// Parses a time in decimal seconds (`12`, `12.5`, `0.040`) exactly; `None`
/// for anything else, including signs, exponents, more than twelve digits
/// before the point and more than nine after it.
pub fn parse_time(text: &str) -> Option<Rational> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty()
        || whole.len() > MAX_WHOLE_DIGITS
        || !digits(whole)
        || !digits(fraction)
        || fraction.len() > MAX_FRACTION_DIGITS
        || (text.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let scale = 10u64.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let whole: i128 = whole.parse().ok()?;
    let fraction: i128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    let num = whole
        .checked_mul(i128::from(scale))?
        .checked_add(fraction)?;
    Rational::new(num, scale)
}

/// Why a clip failed; the name predates the other commands that write.
pub type ClipFailure = WriteFailure;

/// Clips the file at `path` into `out` with the library defaults.
pub fn run<W: Write>(
    path: &Path,
    options: &ClipOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<ClipSummary, ClipFailure> {
    let source = BufferedSource::new(FileSource::open(path)?);
    Ok(clip_mp4(
        source,
        &Limits::default(),
        allowlist,
        options,
        out,
    )?)
}

/// Clips bytes already in memory, as [`run`] does for a file.
pub fn run_slice<W: Write>(
    data: &[u8],
    options: &ClipOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<ClipSummary, ClipFailure> {
    run_source(SliceSource::new(data), options, allowlist, out)
}

/// Clips any [`Source`](coreav_isobmff::Source), as [`run`] does for a
/// file.
pub fn run_source<S: coreav_isobmff::Source, W: Write>(
    source: S,
    options: &ClipOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<ClipSummary, ClipFailure> {
    Ok(clip_mp4(
        source,
        &Limits::default(),
        allowlist,
        options,
        out,
    )?)
}

/// The `coreav.clip/1` summary (`docs/schema/clip-v1.md`).
pub fn document(summary: &ClipSummary) -> Value {
    let video_timescale = summary.video.timescale;
    let mut streams = vec![stream("video", &summary.video)];
    if let Some(audio) = &summary.audio {
        streams.push(stream("audio", audio));
    }
    let boxes: Vec<String> = summary
        .boxes_left_out
        .iter()
        .map(ToString::to_string)
        .collect();
    json!({
        "schema": "coreav.clip/1",
        "output": {"bytes": summary.bytes_written},
        "start": {
            "requested": rational(summary.start),
            "snapped": seconds_as_time(summary.snapped_start, video_timescale),
        },
        "end": {
            "requested": summary.end.map_or(Value::Null, rational),
            "written": seconds_as_time(summary.written_end, video_timescale),
        },
        "streams": streams,
        "left_out": left_out(&summary.left_out),
        "boxes_left_out": boxes,
    })
}

/// A time in seconds as a `time` in ticks of `timescale`.
pub fn seconds_as_time(seconds: Rational, timescale: u32) -> Value {
    let ticks = Rational::new(
        seconds.num.saturating_mul(i128::from(timescale)),
        seconds.den,
    );
    ticks.map_or(Value::Null, |t| time(t, timescale))
}

/// The `left_out` array of a summary.
pub fn left_out(left_out: &[LeftOut]) -> Vec<Value> {
    left_out
        .iter()
        .map(|l| {
            json!({
                "stream_index": l.stream_index,
                "track_id": l.track_id,
                "reason": match l.reason {
                    LeftOutReason::NotSelected => "not_selected",
                    LeftOutReason::NoSamplesInClip => "no_samples_in_clip",
                    LeftOutReason::CodecNotAllowed => "codec_not_allowed",
                },
            })
        })
        .collect()
}

/// One kept stream of a summary.
pub fn stream(kind: &str, kept: &ClipTrack) -> Value {
    json!({
        "kind": kind,
        "stream_index": kept.stream_index,
        "track_id": kept.track_id,
        "codec": names::codec(kept.codec),
        "first_sample": kept.first_sample,
        "samples": kept.samples,
        "duration": time(Rational::from_int(i128::from(kept.duration)), kept.timescale),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_parse_exactly() {
        let q = |n, d| Rational::new(n, d).unwrap();
        assert_eq!(parse_time("12"), Some(q(12, 1)));
        assert_eq!(parse_time("12.5"), Some(q(25, 2)));
        assert_eq!(parse_time("0.04"), Some(q(1, 25)));
        assert_eq!(parse_time("0.000000001"), Some(q(1, 1_000_000_000)));
        for bad in [
            "",
            ".5",
            "5.",
            "-1",
            "+1",
            "1e3",
            "1.0000000001",
            "1,5",
            " 1",
            "0x10",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }
}
