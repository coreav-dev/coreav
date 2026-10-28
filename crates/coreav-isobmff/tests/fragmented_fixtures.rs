//! Remote index routes on files written by the pinned `ffmpeg`
//! through `scripts/gen-fixtures.sh`. Set `COREAV_FIXTURES` to the output
//! directory; without it these tests only print that they were not run.

// Test helpers outside `#[test]` functions unwrap and panic like the tests.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use coreav_isobmff::{
    Allowlist, Budget, DownloadReason, Error, IndexRoute, IndexStatus, Limits, Movie, RangePolicy,
    RangeReader, SliceSource, Source, plan_index, probe,
};
use std::path::PathBuf;

fn fixture(name: &str) -> Option<Vec<u8>> {
    let Some(dir) = std::env::var_os("COREAV_FIXTURES") else {
        eprintln!("COREAV_FIXTURES is not set; {name} not checked");
        return None;
    };
    let path = PathBuf::from(dir).join(name);
    Some(std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

type Row = (usize, u64, u64, u32, i64, i64, bool);

fn packets<S: Source>(movie: &Movie, source: &mut S) -> Result<Vec<Row>, Error> {
    let limits = Limits::default();
    let mut rows = Vec::new();
    for track in 0..movie.tracks.len() {
        let mut budget = Budget::new(&limits);
        for p in movie.packets(track, &mut *source, &limits, &mut budget)? {
            let p = p?;
            rows.push((track, p.sample, p.offset, p.size, p.dts, p.cts, p.keyframe));
        }
    }
    Ok(rows)
}

/// Payload ranges of the top-level media data boxes.
fn mdat_payloads(data: &[u8]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        assert!(size >= 8, "box at {at}");
        if &data[at + 4..at + 8] == b"mdat" {
            out.push(((at + 8) as u64, (at + size) as u64));
        }
        at += size;
    }
    out
}

fn route(data: &[u8]) -> (RangeReader<SliceSource<'_>>, Movie, u64, IndexRoute) {
    let policy = RangePolicy::default();
    let mut reader = RangeReader::new(SliceSource::new(data), &policy);
    let movie = probe(&mut reader, &Limits::default(), &Allowlist::default()).unwrap();
    let probe_requests = reader.requests();
    let route = plan_index(&movie, &mut reader, &policy).unwrap();
    (reader, movie, probe_requests, route)
}

#[test]
fn global_segment_index_is_indexed_within_the_budget() {
    let Some(data) = fixture("fragmented_sidx.mp4") else {
        return;
    };
    let (mut reader, movie, probe_requests, route) = route(&data);
    let IndexRoute::SegmentIndex(plan) = route else {
        panic!("route {route:?}");
    };
    assert_eq!(plan.subsegments, 20);
    reader.fetch(&plan.windows).unwrap();
    let rows = packets(&movie, &mut reader).unwrap();
    let local = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert_eq!(rows, packets(&local, &mut SliceSource::new(&data)).unwrap());
    // Every sample lies in media data, not in a fragment header.
    let mdats = mdat_payloads(&data);
    for (.., offset, size, _, _, _) in &rows {
        let end = offset + u64::from(*size);
        assert!(
            mdats.iter().any(|(s, e)| s <= offset && end <= *e),
            "sample at {offset} outside every mdat"
        );
    }
    let len = data.len() as u64;
    let bytes = reader.bytes_requested();
    println!(
        "fragmented_sidx.mp4: {len} bytes, {} requests ({probe_requests} probe, {} windows), \
         {bytes} bytes ({:.3}%)",
        reader.requests(),
        plan.windows.len(),
        bytes as f64 * 100.0 / len as f64
    );
    assert!(bytes * 20 <= len);
    assert!(reader.requests() <= probe_requests + 1 + plan.windows.len() as u64);
}

#[test]
fn heavily_fragmented_without_index_is_routed_to_download() {
    let Some(data) = fixture("fragmented_many.mp4") else {
        return;
    };
    let (reader, _, probe_requests, route) = route(&data);
    match route {
        IndexRoute::DownloadThenIndex(DownloadReason::TooManyUnindexedFragments {
            index: IndexStatus::Absent,
            estimated_fragments,
        }) => assert!(estimated_fragments > 50, "{estimated_fragments}"),
        other => panic!("route {other:?}"),
    }
    assert!(reader.requests() <= probe_requests + 1);
}

#[test]
fn regular_fragmented_fixture_without_index() {
    // Two-second fixtures: a few fragments, walked one by one.
    let Some(data) = fixture("fragmented.mp4") else {
        return;
    };
    let (mut reader, movie, _, route) = route(&data);
    match route {
        IndexRoute::Sequential { .. } => {
            let local = probe(
                SliceSource::new(&data),
                &Limits::default(),
                &Allowlist::default(),
            )
            .unwrap();
            assert_eq!(
                packets(&movie, &mut reader).unwrap(),
                packets(&local, &mut SliceSource::new(&data)).unwrap()
            );
        }
        // A small object can break the byte bar before the walk.
        IndexRoute::DownloadThenIndex(_) => {}
        other => panic!("route {other:?}"),
    }
}
