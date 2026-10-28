//! `coreav remux` (`docs/design/remux.md`, `docs/design/mkv-remux.md`) and
//! `coreav sanitize` (`docs/design/sanitize.md`): an MP4/MOV, Matroska or
//! WebM file rewritten as an MP4 file with the movie box first, by stream
//! copy, with the library defaults, and the `coreav.remux/1` summary they
//! share.

use crate::clip::ClipFailure;
use crate::error::Failure;
use crate::names;
use crate::native::time;
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Rational, SliceSource};
use coreav_isobmff_write::{
    BoxDropReason, ClearedField, Keep, MatroskaRemuxError, MatroskaRemuxOptions,
    MatroskaRemuxSummary, Metadata, MetadataKind, RemuxOptions, RemuxSummary, TrackDropReason,
    remux_matroska, remux_mp4,
};
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

/// Remuxes the file at `path` into `out` with the library defaults.
pub fn run<W: Write>(
    path: &Path,
    options: &RemuxOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<RemuxSummary, ClipFailure> {
    let source = BufferedSource::new(
        FileSource::open(path).map_err(|e| ClipFailure::Rejected(Failure::library(e)))?,
    );
    Ok(remux_mp4(
        source,
        &Limits::default(),
        allowlist,
        options,
        out,
    )?)
}

/// Remuxes bytes already in memory, as [`run`] does for a file.
pub fn run_slice<W: Write>(
    data: &[u8],
    options: &RemuxOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<RemuxSummary, ClipFailure> {
    run_source(SliceSource::new(data), options, allowlist, out)
}

/// Remuxes any [`Source`](coreav_isobmff::Source), as [`run`] does for a
/// file.
pub fn run_source<S: coreav_isobmff::Source, W: Write>(
    source: S,
    options: &RemuxOptions,
    allowlist: &Allowlist,
    out: W,
) -> Result<RemuxSummary, ClipFailure> {
    Ok(remux_mp4(
        source,
        &Limits::default(),
        allowlist,
        options,
        out,
    )?)
}

/// The names `--keep` accepts, and what they keep.
pub const KEEP_NAMES: [&str; 5] = ["rotation", "language", "dates", "gapless", "kinds"];

/// Parses a `--keep` list: names from [`KEEP_NAMES`], or `none` alone;
/// the unknown name otherwise.
pub fn parse_keep(names: &[String]) -> Result<Keep, String> {
    let mut keep = Keep::NOTHING;
    if names.len() == 1 && names.first().is_some_and(|n| n == "none") {
        return Ok(keep);
    }
    for name in names {
        match name.as_str() {
            "rotation" => keep.rotation = true,
            "language" => keep.language = true,
            "dates" => keep.creation_times = true,
            "gapless" => keep.gapless = true,
            "kinds" => keep.kinds = true,
            other => return Err(other.to_string()),
        }
    }
    Ok(keep)
}

/// The `coreav.remux/1` summary (`docs/schema/remux-v1.md`) of a remux
/// with `options`.
pub fn document(summary: &RemuxSummary, options: &RemuxOptions) -> Value {
    let streams: Vec<Value> = summary
        .tracks
        .iter()
        .map(|t| {
            json!({
                "kind": names::kind(t.kind),
                "stream_index": t.stream_index,
                "source_track_id": t.source_track_id,
                "track_id": t.track_id,
                "codec": names::codec(t.codec),
                "samples": t.samples,
                "duration": time(Rational::from_int(i128::from(t.duration)), t.timescale),
                "annex_b": false,
            })
        })
        .collect();
    let dropped_streams: Vec<Value> = summary
        .dropped_tracks
        .iter()
        .map(|d| {
            let (reason, message) = match d.reason {
                TrackDropReason::NotSelected => ("not_selected", None),
                TrackDropReason::CodecNotAllowed => ("codec_not_allowed", None),
                TrackDropReason::Unsupported { reason } => ("unsupported", Some(reason)),
            };
            json!({
                "stream_index": d.stream_index,
                "track_id": d.track_id,
                "reason": reason,
                "message": message,
            })
        })
        .collect();
    let dropped_boxes: Vec<Value> = summary
        .dropped_boxes
        .iter()
        .map(|b| {
            let path: Vec<String> = b.path.iter().map(ToString::to_string).collect();
            json!({
                "stream_index": b.stream_index,
                "path": path.join("/"),
                "offset": b.offset,
                "size": b.size,
                "reason": match b.reason {
                    BoxDropReason::NotCarried => "not_carried",
                    BoxDropReason::TooLarge => "too_large",
                    BoxDropReason::Sanitized => "sanitized",
                },
                "kind": b.kind.map(|k| match k {
                    MetadataKind::Location => "location",
                    MetadataKind::Device => "device",
                    MetadataKind::Date => "date",
                    MetadataKind::Other => "other",
                }),
                "name": b.name,
            })
        })
        .collect();
    let cleared: Vec<Value> = summary
        .cleared
        .iter()
        .map(|c| {
            json!({
                "stream_index": c.stream_index,
                "field": match c.field {
                    ClearedField::CreationTime => "creation_time",
                    ClearedField::Rotation => "rotation",
                    ClearedField::Language => "language",
                    ClearedField::CompressorName => "compressor_name",
                    ClearedField::SampleEntryExtras => "sample_entry_extras",
                    ClearedField::SphericalSource => "spherical_source",
                },
            })
        })
        .collect();
    let metadata = match options.metadata {
        Metadata::Copy => json!({"mode": "copy", "keep": Value::Null}),
        Metadata::Sanitize(keep) => {
            let kept: Vec<&str> = [
                (keep.rotation, "rotation"),
                (keep.language, "language"),
                (keep.creation_times, "dates"),
                (keep.gapless, "gapless"),
                (keep.kinds, "kinds"),
            ]
            .into_iter()
            .filter_map(|(on, name)| on.then_some(name))
            .collect();
            json!({"mode": "sanitize", "keep": kept})
        }
    };
    json!({
        "schema": "coreav.remux/1",
        "output": {"bytes": summary.bytes_written},
        "metadata": metadata,
        "source": {"moov_first": summary.source_moov_first, "truncated_at": null},
        "streams": streams,
        "dropped_streams": dropped_streams,
        "dropped_boxes": dropped_boxes,
        "dropped_boxes_not_listed": summary.dropped_boxes_not_listed,
        "cleared": cleared,
    })
}

impl From<MatroskaRemuxError> for ClipFailure {
    fn from(error: MatroskaRemuxError) -> Self {
        match error {
            MatroskaRemuxError::Source(error) => Self::Rejected(Failure::Matroska(error)),
            MatroskaRemuxError::Output(error) => Self::Rejected(Failure::library(error)),
            MatroskaRemuxError::Write(kind) => Self::Io(json!({
                "schema": "coreav.error/1",
                "kind": "io",
                "limit": null,
                "offset": null,
                "message": format!("write error: {kind}"),
            })),
            MatroskaRemuxError::SourceChanged => Self::Io(json!({
                "schema": "coreav.error/1",
                "kind": "io",
                "limit": null,
                "offset": null,
                "message": "the source changed while it was remuxed",
            })),
            video @ MatroskaRemuxError::VideoNotCopied { .. } => {
                Self::Rejected(Failure::Unsupported(format!(
                    "{video}; --drop-unsupported writes the other streams without it"
                )))
            }
        }
    }
}

/// Remuxes the Matroska or WebM file at `path` into `out` with the library
/// defaults.
pub fn run_matroska<W: Write>(
    path: &Path,
    options: &MatroskaRemuxOptions,
    allowlist: &coreav_mkv::Allowlist,
    out: W,
) -> Result<MatroskaRemuxSummary, ClipFailure> {
    let source = coreav_mkv::BufferedSource::new(
        FileSource::open(path).map_err(|e| ClipFailure::Rejected(Failure::library(e)))?,
    );
    Ok(remux_matroska(
        source,
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        allowlist,
        options,
        out,
    )?)
}

/// Remuxes Matroska or WebM bytes already in memory, as [`run_matroska`]
/// does for a file.
pub fn run_matroska_slice<W: Write>(
    data: &[u8],
    options: &MatroskaRemuxOptions,
    allowlist: &coreav_mkv::Allowlist,
    out: W,
) -> Result<MatroskaRemuxSummary, ClipFailure> {
    run_matroska_source(coreav_mkv::SliceSource::new(data), options, allowlist, out)
}

/// Remuxes Matroska or WebM from any [`Source`](coreav_isobmff::Source), as
/// [`run_matroska`] does for a file.
pub fn run_matroska_source<S: coreav_isobmff::Source, W: Write>(
    source: S,
    options: &MatroskaRemuxOptions,
    allowlist: &coreav_mkv::Allowlist,
    out: W,
) -> Result<MatroskaRemuxSummary, ClipFailure> {
    Ok(remux_matroska(
        source,
        &coreav_mkv::Limits::default(),
        &Limits::default(),
        allowlist,
        options,
        out,
    )?)
}

/// The `coreav.remux/1` summary of a Matroska or WebM source: the source
/// track IDs are `TrackNumber`s, and the dropped boxes are the
/// `BlockAdditions` elements of kept streams (alpha planes), which MP4
/// cannot carry.
pub fn matroska_document(summary: &MatroskaRemuxSummary) -> Value {
    let kind = |kind| match kind {
        coreav_mkv::TrackKind::Video => "video",
        coreav_mkv::TrackKind::Audio => "audio",
        coreav_mkv::TrackKind::Subtitle => "subtitle",
        coreav_mkv::TrackKind::Data => "data",
    };
    let streams: Vec<Value> = summary
        .tracks
        .iter()
        .map(|t| {
            json!({
                "kind": kind(t.kind),
                "stream_index": t.stream_index,
                "source_track_id": t.track_number,
                "track_id": t.track_id,
                "codec": t.codec.name(),
                "samples": t.samples,
                "duration": time(Rational::from_int(i128::from(t.duration)), t.timescale),
                "annex_b": t.annex_b,
            })
        })
        .collect();
    let dropped_streams: Vec<Value> = summary
        .dropped_tracks
        .iter()
        .map(|d| {
            let (reason, message) = match d.reason {
                TrackDropReason::NotSelected => ("not_selected", None),
                TrackDropReason::CodecNotAllowed => ("codec_not_allowed", None),
                TrackDropReason::Unsupported { reason } => ("unsupported", Some(reason)),
            };
            json!({
                "stream_index": d.stream_index,
                "track_id": d.track_number,
                "reason": reason,
                "message": message,
            })
        })
        .collect();
    json!({
        "schema": "coreav.remux/1",
        "output": {"bytes": summary.bytes_written},
        "metadata": {"mode": "copy", "keep": Value::Null},
        "source": {"moov_first": false, "truncated_at": summary.truncated_at},
        "streams": streams,
        "dropped_streams": dropped_streams,
        "dropped_boxes": summary.dropped_additions.iter().map(|a| json!({
            "stream_index": a.stream_index,
            "path": "Segment/Cluster/BlockGroup/BlockAdditions",
            "offset": a.offset,
            "size": a.size,
            "reason": "not_carried",
            "kind": Value::Null,
            "name": Value::Null,
        })).collect::<Vec<_>>(),
        "dropped_boxes_not_listed": summary.dropped_additions_not_listed,
        "cleared": [],
    })
}
