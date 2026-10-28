//! Opening and probing a file with the library defaults: fragment
//! scanning and the codec allowlist.

use crate::error::Failure;
use crate::names;
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Movie, Source, probe};
use std::path::Path;

/// A successful probe and what the output needs to describe it.
pub struct Probed {
    /// The probe result.
    pub movie: Movie,
    /// File size in bytes.
    pub size: u64,
    /// Every movie fragment was read ([`Movie::scan_fragments`]).
    pub scanned: bool,
    /// For each track, the bound of the picture sizes its configuration and
    /// samples define ([`Movie::scan_frame_sizes`]), when the samples were
    /// scanned; `None` when they were not.
    pub frame_sizes: Option<Vec<Option<(u32, u32)>>>,
}

/// Builds the allowlist from `--allow` names; returns the first unknown
/// name on failure.
pub fn allowlist(names: Option<&[String]>, strict: bool) -> Result<Allowlist, String> {
    let mut list = match names {
        None => Allowlist::default(),
        Some(names) => {
            let mut codecs = Vec::with_capacity(names.len());
            for name in names {
                let trimmed = name.trim();
                match names::codec_from_name(trimmed) {
                    Some(codec) => codecs.push(codec),
                    None => return Err(trimmed.to_owned()),
                }
            }
            Allowlist::new(&codecs)
        }
    };
    if strict {
        list = list.strict();
    }
    Ok(list)
}

/// Builds one allowlist per container from `--allow` names in one pass:
/// each container's list keeps the names it knows, so `--allow vp8,h264`
/// allows H.264 in an MP4 file and both in a WebM file. Returns the first
/// name no container knows on failure; `other` is never a known name.
pub fn allowlists(
    names: Option<&[String]>,
    strict: bool,
) -> Result<(Allowlist, coreav_mkv::Allowlist, coreav_mpegts::Allowlist), String> {
    let (mut isobmff, mut mkv, mut ts) = match names {
        None => (
            Allowlist::default(),
            coreav_mkv::Allowlist::default(),
            coreav_mpegts::Allowlist::default(),
        ),
        Some(names) => {
            let (mut iso_codecs, mut mkv_codecs, mut ts_codecs) =
                (Vec::new(), Vec::new(), Vec::new());
            for name in names {
                let trimmed = name.trim();
                let iso = names::codec_from_name(trimmed);
                let mkv = crate::matroska::codec_from_name(trimmed);
                let ts = crate::mpegts::codec_from_name(trimmed);
                if iso.is_none() && mkv.is_none() && ts.is_none() {
                    return Err(trimmed.to_owned());
                }
                iso_codecs.extend(iso);
                mkv_codecs.extend(mkv);
                ts_codecs.extend(ts);
            }
            (
                Allowlist::new(&iso_codecs),
                coreav_mkv::Allowlist::new(&mkv_codecs),
                coreav_mpegts::Allowlist::new(&ts_codecs),
            )
        }
    };
    if strict {
        isobmff = isobmff.strict();
        mkv = mkv.strict();
        ts = ts.strict();
    }
    Ok((isobmff, mkv, ts))
}

/// Probes `path`. With `scan`, fragments are read when a fragmented stream's
/// duration or sample bytes are not known from the movie box.
pub fn run(path: &Path, allowlist: &Allowlist, scan: bool) -> Result<Probed, Failure> {
    run_with(path, allowlist, scan, false)
}

/// [`run`], and with `frame_sizes`, also scans the samples of video tracks
/// for the picture sizes their parameter sets and frame headers define
/// (`docs/design/frame-sizes.md`).
pub fn run_with(
    path: &Path,
    allowlist: &Allowlist,
    scan: bool,
    frame_sizes: bool,
) -> Result<Probed, Failure> {
    let size = std::fs::metadata(path).map_err(Failure::io)?.len();
    let source = BufferedSource::new(FileSource::open(path).map_err(Failure::library)?);
    run_source_with(source, size, allowlist, scan, frame_sizes)
}

/// Probes any [`Source`] of `size` bytes with the library defaults, as
/// [`run`] does for a file.
pub fn run_source<S: Source>(
    source: S,
    size: u64,
    allowlist: &Allowlist,
    scan: bool,
) -> Result<Probed, Failure> {
    run_source_with(source, size, allowlist, scan, false)
}

/// [`run_with`] for any [`Source`] of `size` bytes.
pub fn run_source_with<S: Source>(
    source: S,
    size: u64,
    allowlist: &Allowlist,
    scan: bool,
    frame_sizes: bool,
) -> Result<Probed, Failure> {
    run_source_limited(
        source,
        size,
        allowlist,
        scan,
        frame_sizes,
        &Limits::default(),
    )
}

/// [`run_source_with`] under `limits` instead of the library defaults.
pub fn run_source_limited<S: Source>(
    mut source: S,
    size: u64,
    allowlist: &Allowlist,
    scan: bool,
    frame_sizes: bool,
    limits: &Limits,
) -> Result<Probed, Failure> {
    let mut movie = probe(&mut source, limits, allowlist).map_err(Failure::library)?;
    let mut scanned = !movie.has_fragments();
    if scan && movie.needs_fragment_scan() {
        movie
            .scan_fragments(&mut source, limits)
            .map_err(Failure::library)?;
        scanned = true;
    }
    let frame_sizes = if frame_sizes {
        Some(scan_sizes(&movie, &mut source, limits)?)
    } else {
        None
    };
    Ok(Probed {
        movie,
        size,
        scanned,
        frame_sizes,
    })
}

/// The scanned size of every video track, under one work budget for the
/// whole scan ([`Movie::scan_all_frame_sizes`]); only an I/O error fails the
/// probe.
fn scan_sizes<S: Source>(
    movie: &Movie,
    source: &mut S,
    limits: &Limits,
) -> Result<Vec<Option<(u32, u32)>>, Failure> {
    movie
        .scan_all_frame_sizes(&mut *source, limits)
        .map_err(Failure::library)
}
