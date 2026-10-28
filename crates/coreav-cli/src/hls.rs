//! `coreav hls` (design note `docs/design/hls.md`): the index of a file, the
//! resources a server cuts from it on request, and the `coreav.hls-index/2`
//! summary.

use crate::clip::ClipFailure;
use crate::error::Failure;
use crate::native::time;
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Rational};
use coreav_isobmff_write::ExtractError;
use coreav_isobmff_write::hls::{HlsIndex, HlsOptions, MediaSegment, Resource, build_index};
use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Name of the index file `coreav hls package` writes next to the
/// resources.
pub const INDEX_NAME: &str = "index.cavhls";

/// Builds the index of the file at `path` with the library defaults.
pub fn index(
    path: &Path,
    options: &HlsOptions,
    allowlist: &Allowlist,
) -> Result<Vec<u8>, ClipFailure> {
    let source = BufferedSource::new(
        FileSource::open(path).map_err(|e| ClipFailure::Rejected(Failure::library(e)))?,
    );
    Ok(build_index(source, &Limits::default(), allowlist, options)?)
}

/// Opens the index at `path`, reading only its header and directories.
pub fn open(path: &Path) -> Result<HlsIndex, ClipFailure> {
    let file = FileSource::open(path).map_err(|e| ClipFailure::Rejected(Failure::library(e)))?;
    Ok(HlsIndex::open(file, &Limits::default())?)
}

/// A resource ready to be written: everything that can fail before the
/// output is touched has been done.
pub enum Prepared {
    /// A playlist or an init segment.
    Bytes(Vec<u8>),
    /// A media segment and its source, whose length has been checked.
    Segment(MediaSegment, std::fs::File),
}

/// Prepares `resource`: a playlist or init segment from the index alone, a
/// media segment from the index at `index_path` and the file at `source`,
/// which must have the length the index records.
pub fn prepare(
    index_path: &Path,
    index: &HlsIndex,
    source: &Path,
    resource: Resource,
) -> Result<Prepared, ClipFailure> {
    let library = |e: coreav_isobmff::Error| ClipFailure::Rejected(Failure::library(e));
    let missing = || library(coreav_isobmff::Error::NoSuchTrack);
    Ok(match resource {
        Resource::Master => Prepared::Bytes(index.master_playlist().into_bytes()),
        Resource::Playlist(rendition) => Prepared::Bytes(
            index
                .media_playlist(rendition)
                .ok_or_else(missing)?
                .into_bytes(),
        ),
        Resource::Init(rendition) => Prepared::Bytes(index.init_segment(rendition)?),
        Resource::Segment(rendition, number) => {
            let records = FileSource::open(index_path).map_err(library)?;
            // A deferred rendition's samples come from the source's own
            // movie fragment (`docs/design/hls.md`).
            let media = FileSource::open(source).map_err(library)?;
            let segment =
                index.media_segment(rendition, number, records, media, &Limits::default())?;
            let read_error = |e: std::io::Error| library(coreav_isobmff::Error::from(e));
            let file = std::fs::File::open(source).map_err(read_error)?;
            if file.metadata().map_err(read_error)?.len() != segment.source_len() {
                return Err(source_changed());
            }
            Prepared::Segment(segment, file)
        }
    })
}

/// Writes a prepared resource to `out`; returns the bytes written. A media
/// segment's ranges are copied by `std::io::copy` from the file, which the
/// standard library hands to the kernel (`copy_file_range` on Linux) when
/// `out` is a file.
pub fn emit<W: Write>(prepared: Prepared, mut out: W) -> Result<u64, ClipFailure> {
    let write_error = |e: std::io::Error| ClipFailure::from(ExtractError::from(e));
    match prepared {
        Prepared::Bytes(bytes) => {
            out.write_all(&bytes)
                .and_then(|()| out.flush())
                .map_err(write_error)?;
            Ok(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        }
        Prepared::Segment(segment, mut file) => {
            let read_error = |e: std::io::Error| {
                ClipFailure::Rejected(Failure::library(coreav_isobmff::Error::from(e)))
            };
            out.write_all(segment.header()).map_err(write_error)?;
            for range in segment.ranges() {
                file.seek(SeekFrom::Start(range.offset))
                    .map_err(read_error)?;
                let copied = std::io::copy(&mut (&mut file).take(range.len), &mut out)
                    .map_err(write_error)?;
                if copied != range.len {
                    return Err(source_changed());
                }
            }
            out.flush().map_err(write_error)?;
            Ok(segment.len())
        }
    }
}

/// The source does not have the length the index was built from: a
/// rejection (exit 1), not an I/O failure.
fn source_changed() -> ClipFailure {
    ClipFailure::Index(source_mismatch())
}

/// The `coreav.error/1` object of a source that is not the one an index
/// was built from (its length differs); the WebAssembly module reports it
/// too.
pub fn source_mismatch() -> Value {
    json!({
        "schema": "coreav.error/1",
        "kind": "source_mismatch",
        "limit": null,
        "offset": null,
        "message": "the source is not the file the index was built from (its length differs)",
    })
}

/// Every resource of `index`: the master playlist, then per rendition its
/// playlist, init segment and media segments.
pub fn resources(index: &HlsIndex) -> Vec<Resource> {
    let mut all = vec![Resource::Master];
    for track in index.tracks() {
        let r = track.rendition;
        all.push(Resource::Playlist(r));
        all.push(Resource::Init(r));
        let count = u32::try_from(track.segments().len()).unwrap_or(u32::MAX);
        all.extend((0..count).map(|n| Resource::Segment(r, n)));
    }
    all
}

/// The `coreav.hls-index/2` summary.
pub fn document(index: &HlsIndex) -> Value {
    let renditions: Vec<Value> = index
        .tracks()
        .iter()
        .map(|t| {
            let ticks = |v: u64| time(Rational::from_int(i128::from(v)), t.timescale);
            // In ticks, exact (its EXTINF before rounding).
            let longest = t
                .segments()
                .iter()
                .enumerate()
                .filter_map(|(n, s)| t.segment_end(n)?.checked_sub(s.start))
                .max();
            json!({
                "rendition": t.rendition.name(),
                "stream_index": t.stream_index,
                "track_id": t.track_id,
                "codecs": t.codecs,
                "segments": t.segments().len(),
                // A deferred rendition's index does not count the samples
                // or their bytes: its segments are the source's
                // subsegments, whose fragments hold both
                // (`docs/design/hls.md`).
                "deferred": t.deferred.is_some(),
                "samples": t.deferred.is_none().then_some(t.samples),
                "sample_bytes": t.deferred.is_none().then_some(t.data_bytes),
                "duration": ticks(t.end),
                "longest_segment": longest.map(ticks),
                "peak_bit_rate": t.peak_bit_rate,
                "average_bit_rate": t.average_bit_rate,
            })
        })
        .collect();
    let target = index.target_segment_duration();
    json!({
        "schema": "coreav.hls-index/2",
        "source": {"bytes": index.source_len()},
        "index": {"bytes": index.index_len()},
        "segment_duration": crate::native::rational(target),
        "renditions": renditions,
    })
}
