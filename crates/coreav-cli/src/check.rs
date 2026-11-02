//! Upload policy checks (`coreav check`): probe with the library defaults,
//! then evaluate the `coreav.probe/1` document (ISO base media) or the
//! `coreav.probe-matroska/1` document (Matroska and WebM) or
//! `coreav.probe-mpegts/2` document (MPEG-TS) with `coreav-policy`.

use crate::error::Failure;
use crate::{matroska, mpegts, native, probe};
use coreav_isobmff::{Allowlist, BufferedSource, FileSource, Limits, Source};
use coreav_policy::{Action, KINDS, Policy, Verdict, evaluate};
use serde_json::Value;
use std::path::Path;

/// Checks the file at `path`. A file the probe rejects gets a reject
/// verdict whose reason carries the probe's `coreav.error/1` object; only a
/// file that cannot be opened or read is an `Err`.
pub fn run(path: &Path, policy: &Policy) -> Result<Verdict, Failure> {
    let source = FileSource::open(path).map_err(Failure::library)?;
    let mut source = BufferedSource::new(source);
    let size = source.len().map_err(Failure::library)?;
    run_source(source, size, policy)
}

/// Checks any [`Source`] of `size` bytes, as [`run`] does for a file.
pub fn run_source<S: Source>(source: S, size: u64, policy: &Policy) -> Result<Verdict, Failure> {
    run_source_limited(source, size, policy, &Limits::default())
}

/// [`run_source`] with an MP4 or MOV file probed under `limits` instead of
/// the library defaults (the other containers keep their own defaults).
pub fn run_source_limited<S: Source>(
    source: S,
    size: u64,
    policy: &Policy,
    limits: &Limits,
) -> Result<Verdict, Failure> {
    verdict(probe_source_limited(source, size, policy, limits), policy)
}

/// The native probe document a check evaluates, with the scans its policy
/// needs. `limits` bounds ISO base media probing; other containers use
/// their own defaults. File, source and fuzz callers share this dispatch.
pub fn probe_source_limited<S: Source>(
    mut source: S,
    size: u64,
    policy: &Policy,
    limits: &Limits,
) -> Result<Value, Failure> {
    let mut prefix = [0u8; 4];
    let matroska = size >= 4
        && source
            .read_exact_at(0, &mut prefix)
            .map_err(Failure::library)
            .map(|()| coreav_mkv::sniff(&prefix))?;
    if matroska {
        let probed = matroska::run_source_with(
            source,
            size,
            &coreav_mkv::Allowlist::all(),
            needs_block_scan(policy),
        );
        return probed.map(|p| matroska::document(&p));
    }
    let scan = needs_scan(policy);
    match probe::run_source_limited(&mut source, size, &Allowlist::default(), true, scan, limits) {
        Err(failure @ Failure::Library(coreav_isobmff::Error::NotIsobmff)) => {
            match mpegts::run_source_with(
                source,
                size,
                &coreav_mpegts::Allowlist::all(),
                needs_ts_scan(policy),
            ) {
                Err(Failure::Mpegts(coreav_mpegts::Error::NotTransportStream)) => Err(failure),
                other => other.map(|p| mpegts::document(&p)),
            }
        }
        other => other.map(|p| native::document(&p)),
    }
}

fn needs_ts_scan(policy: &Policy) -> bool {
    KINDS.into_iter().any(|kind| {
        policy
            .streams
            .kind(kind)
            .is_some_and(|rules| rules.max_bit_rate.is_some())
    })
}

/// Whether the samples of video streams must be scanned for their picture
/// sizes: the policy limits the resolution with the scanned size
/// (`docs/design/frame-sizes.md`, decision 4).
fn needs_scan(policy: &Policy) -> bool {
    policy
        .streams
        .video
        .as_ref()
        .is_some_and(|v| v.needs_scan())
}

/// Whether the blocks of a Matroska file must be scanned: a rule needs a
/// value only the scan gives (whether every frame could be listed, stream
/// durations, bit rates, the file duration of a live recording, scanned
/// sizes; `docs/design/policy-matroska.md`, decisions 2 and 8). The
/// default policy rejects streams whose frames could not be listed, so a
/// check scans unless `invalid_tables` is `"ignore"` and no other rule
/// needs the scan.
fn needs_block_scan(policy: &Policy) -> bool {
    policy.streams.invalid_tables == Action::Reject
        || policy.duration.max_seconds.is_some()
        || policy.file.max_bit_rate.is_some()
        || KINDS.into_iter().any(|kind| {
            policy
                .streams
                .kind(kind)
                .is_some_and(|rules| rules.max_bit_rate.is_some())
        })
        || needs_scan(policy)
}

/// The verdict for a probe document. The probe allows every codec CoreAV
/// identifies (the default allowlist for ISO base media files, every named
/// codec for Matroska and MPEG-TS) and reads what the rules need (fragments,
/// samples, blocks or transport packets), so every rule sees the values `coreav probe` prints with
/// the matching options, and its dropped streams are exactly the codecs
/// CoreAV does not identify. MPEG-TS also keeps unidentified streams in
/// `streams`, as its document specifies.
fn verdict(document: Result<Value, Failure>, policy: &Policy) -> Result<Verdict, Failure> {
    match document {
        Ok(document) => Ok(evaluate(policy, &document)),
        Err(failure) if failure.is_io() => Err(failure),
        Err(failure) => Ok(Verdict::unreadable(policy.name.clone(), failure.to_json())),
    }
}
