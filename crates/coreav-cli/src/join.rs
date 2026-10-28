//! `coreav join` (`docs/design/join.md`): the join of files with the
//! library defaults and the `coreav.join/1` summary.

use crate::clip::{left_out, seconds_as_time, stream};
use crate::error::Failure;
use crate::output::WriteFailure;
use crate::pretty::{write_array, write_indented};
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Rational};
use coreav_isobmff_write::{
    ExtractError, JoinError, JoinOptions, JoinSummary, JoinedInput, join_mp4,
};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Why a join failed, with the exit code it maps to.
pub struct JoinFailure {
    /// The `coreav.error/1` object, with the input it is about.
    pub json: Value,
    /// Exit code 3 rather than 1.
    pub io: bool,
}

impl JoinFailure {
    fn new(error: &JoinError) -> Self {
        let (mut json, io, input) = match error {
            JoinError::NoInputs => (
                object("unsupported_feature", "no input to join"),
                false,
                None,
            ),
            JoinError::Input { input, error } => {
                // As every command maps a writer's failure; the message
                // below names the input.
                let failure = WriteFailure::from(error.clone());
                (failure.to_json(), failure.is_io(), Some(*input))
            }
            JoinError::Mismatch { input, .. } => {
                (object("incompatible_inputs", ""), false, Some(*input))
            }
            JoinError::Output(e) => {
                let failure = Failure::library(e.clone());
                (failure.to_json(), failure.is_io(), None)
            }
            JoinError::Write(kind) => (object("io", &format!("write error: {kind}")), true, None),
        };
        if let Some(map) = json.as_object_mut() {
            map.insert("message".into(), Value::String(error.to_string()));
            map.insert("input".into(), input.map_or(Value::Null, Value::from));
        }
        Self { json, io }
    }
}

impl From<JoinFailure> for WriteFailure {
    fn from(failure: JoinFailure) -> Self {
        Self::Object {
            json: failure.json,
            io: failure.io,
        }
    }
}

fn object(kind: &str, message: &str) -> Value {
    json!({
        "schema": "coreav.error/1",
        "kind": kind,
        "limit": null,
        "offset": null,
        "message": message,
    })
}

/// Joins the files at `paths`, in order, into `out` with the library
/// defaults.
pub fn run<W: Write>(
    paths: &[PathBuf],
    options: &JoinOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<JoinSummary, JoinFailure> {
    let mut sources = Vec::new();
    for (input, path) in paths.iter().enumerate() {
        let file = FileSource::open(path).map_err(|e| {
            JoinFailure::new(&JoinError::Input {
                input,
                error: ExtractError::Source(e),
            })
        })?;
        sources.push(BufferedSource::new(file));
    }
    join_mp4(sources, &Limits::default(), allowlist, options, out).map_err(|e| JoinFailure::new(&e))
}

/// Writes the `coreav.join/1` summary (`docs/schema/join-v1.md`) to `out`
/// as pretty-printed JSON, an input at a time (`crate::pretty`); `paths`
/// holds the inputs' names.
pub fn write_document<W: Write + ?Sized>(
    out: &mut W,
    summary: &JoinSummary,
    paths: &[PathBuf],
) -> std::io::Result<()> {
    let timescale = summary.inputs.first().map_or(1, |i| i.video.timescale);
    let boxes: Vec<String> = summary
        .boxes_left_out
        .iter()
        .map(ToString::to_string)
        .collect();
    // The keys in serde_json's order (sorted).
    out.write_all(b"{\n  \"boxes_left_out\": ")?;
    write_indented(out, &json!(boxes), "  ")?;
    out.write_all(b",\n  \"duration\": ")?;
    write_indented(out, &seconds_as_time(summary.duration, timescale), "  ")?;
    out.write_all(b",\n  \"edits\": ")?;
    write_indented(
        out,
        &json!({"video": summary.video_edits, "audio": summary.audio_edits}),
        "  ",
    )?;
    out.write_all(b",\n  \"inputs\": ")?;
    write_array(
        out,
        summary
            .inputs
            .iter()
            .zip(paths)
            .enumerate()
            .map(|(index, (input, path))| input_document(index, input, path, timescale)),
    )?;
    out.write_all(b",\n  \"output\": ")?;
    write_indented(out, &json!({"bytes": summary.bytes_written}), "  ")?;
    out.write_all(b",\n  \"schema\": \"coreav.join/1\"\n}\n")
}

/// One input of the summary.
fn input_document(index: usize, input: &JoinedInput, path: &Path, timescale: u32) -> Value {
    let mut streams = vec![stream("video", &input.video)];
    if let Some(audio) = &input.audio {
        streams.push(stream("audio", audio));
    }
    json!({
        "index": index,
        "path": path.display().to_string(),
        "offset": seconds_as_time(input.offset, timescale),
        "video_start": seconds_as_time(input.video_start, timescale),
        "video_end": seconds_as_time(input.video_end, timescale),
        "extended": input.extended,
        "streams": streams,
        "left_out": left_out(&input.left_out),
    })
}

/// Seconds as an exact rational, for messages.
pub fn seconds(value: Rational) -> f64 {
    value.num as f64 / value.den as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_isobmff::{Codec, FourCc};
    use coreav_isobmff_write::{ClipTrack, LeftOut, LeftOutReason};

    #[test]
    fn the_streamed_summary_is_the_whole_document_pretty_printed() {
        let q = |n, d| Rational::new(n, d).unwrap();
        let track = |stream_index, codec, timescale| ClipTrack {
            stream_index,
            track_id: 1,
            codec,
            first_sample: 0,
            samples: 25,
            timescale,
            duration: 25_000,
        };
        let input = |n: i128| JoinedInput {
            video: track(0, Codec::H264, 12_800),
            audio: (n > 0).then(|| track(1, Codec::Aac, 48_000)),
            offset: q(n, 1),
            video_start: q(n, 1),
            video_end: q(n + 1, 1),
            extended: 0,
            left_out: vec![LeftOut {
                stream_index: 2,
                track_id: 3,
                reason: LeftOutReason::NotSelected,
            }],
        };
        let paths: Vec<PathBuf> = (0..3).map(|n| format!("in \"{n}\".mp4").into()).collect();
        for (inputs, boxes) in [
            (vec![], vec![]),
            (vec![input(0), input(1), input(2)], vec![FourCc(*b"pasp")]),
        ] {
            let summary = JoinSummary {
                inputs,
                video_edits: 1,
                audio_edits: 2,
                duration: q(3, 1),
                bytes_written: 1_000,
                boxes_left_out: boxes,
            };
            let timescale = summary.inputs.first().map_or(1, |i| i.video.timescale);
            let whole = json!({
                "schema": "coreav.join/1",
                "output": {"bytes": summary.bytes_written},
                "duration": seconds_as_time(summary.duration, timescale),
                "edits": {"video": summary.video_edits, "audio": summary.audio_edits},
                "inputs": summary
                    .inputs
                    .iter()
                    .zip(&paths)
                    .enumerate()
                    .map(|(n, (input, path))| input_document(n, input, path, timescale))
                    .collect::<Vec<_>>(),
                "boxes_left_out": summary
                    .boxes_left_out
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            });
            let mut out = Vec::new();
            write_document(&mut out, &summary, &paths).unwrap();
            let expected = serde_json::to_string_pretty(&whole).unwrap() + "\n";
            assert_eq!(String::from_utf8(out).unwrap(), expected);
        }
    }
}
