//! `coreav extract-audio` (`docs/design/extract-audio-cli.md`): the
//! extraction of a file's AAC track to M4A with the library defaults, and
//! the `coreav.extract-audio/1` summary.

use crate::native::time;
use crate::output::WriteFailure;
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Rational, SliceSource};
use coreav_isobmff_write::{HeaderMetadata, Summary, TrackChoice, extract_audio};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Extracts the chosen track of the file at `path` into `out` with the
/// library defaults, carrying the header metadata `metadata` says.
pub fn run<W: Write>(
    path: &Path,
    choice: TrackChoice,
    allowlist: &Allowlist,
    metadata: HeaderMetadata,
    out: W,
) -> Result<Summary, WriteFailure> {
    let source = BufferedSource::new(FileSource::open(path)?);
    Ok(extract_audio(
        source,
        &Limits::default(),
        allowlist,
        choice,
        metadata,
        out,
    )?)
}

/// Extracts from bytes already in memory, as [`run`] does for a file.
pub fn run_slice<W: Write>(
    data: &[u8],
    choice: TrackChoice,
    allowlist: &Allowlist,
    out: W,
) -> Result<Summary, WriteFailure> {
    run_source(SliceSource::new(data), choice, allowlist, out)
}

/// Extracts from any [`Source`](coreav_isobmff::Source), as [`run`] does
/// for a file.
pub fn run_source<S: coreav_isobmff::Source, W: Write>(
    source: S,
    choice: TrackChoice,
    allowlist: &Allowlist,
    out: W,
) -> Result<Summary, WriteFailure> {
    Ok(extract_audio(
        source,
        &Limits::default(),
        allowlist,
        choice,
        HeaderMetadata::Copy,
        out,
    )?)
}

/// The `coreav.extract-audio/1` summary
/// (`docs/schema/extract-audio-v1.md`).
pub fn document(summary: &Summary) -> Value {
    json!({
        "schema": "coreav.extract-audio/1",
        "output": {"bytes": summary.bytes_written},
        "stream": {
            "stream_index": summary.stream_index,
            "track_id": summary.track_id,
            // `extract_audio` copies AAC only.
            "codec": "aac",
            "samples": summary.samples,
            "duration": time(
                Rational::from_int(i128::from(summary.duration)),
                summary.timescale,
            ),
        },
    })
}
