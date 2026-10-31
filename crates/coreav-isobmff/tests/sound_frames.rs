//! QuickTime compressed audio whose sample tables count single audio
//! samples (`stsz` of 1, a version 1 sound description giving the samples
//! and bytes of one compressed frame): the bytes are the frames from each
//! chunk's offset, so a last chunk whose frames fit the file iterates,
//! although the tables' one byte a sample would run past its end, and one
//! whose frames do not fit stops as truncated.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use common::sound::{FRAME_BYTES, PER_PACKET, qdm2_movie};
use coreav_isobmff::{Allowlist, Budget, Error, Limits, SliceSource, probe};

/// Samples listed one by one, by runs, by batches of runs and by visits,
/// and from the middle: each the samples and the error that ended it.
fn listings(data: &[u8]) -> [(u64, Option<Error>); 5] {
    let limits = Limits::default();
    let movie = probe(SliceSource::new(data), &limits, &Allowlist::listing()).unwrap();
    assert_eq!(
        movie.tracks[0].description.sound_packet,
        Some((PER_PACKET, FRAME_BYTES))
    );
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut one = (0, None);
    for packet in &mut packets {
        match packet {
            Ok(_) => one.0 += 1,
            Err(error) => one.1 = Some(error),
        }
    }
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut runs = (0, None);
    while let Some(run) = packets.next_run(1000) {
        match run {
            Ok(run) => runs.0 += u64::from(run.count),
            Err(error) => runs.1 = Some(error),
        }
    }
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut batches = (0, None);
    let mut out = Vec::new();
    loop {
        out.clear();
        match packets.next_runs(1000, &mut out, 4) {
            Ok(0) => break,
            Ok(_) => batches.0 += out.iter().map(|r| u64::from(r.count)).sum::<u64>(),
            Err(error) => {
                batches.1 = Some(error);
                break;
            }
        }
    }
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut visits = (0, None);
    match packets.for_each_sample(|_| Ok::<(), Error>(())) {
        Ok(n) => visits.0 = n,
        Err(error) => visits.1 = Some(error),
    }
    let mut budget = Budget::new(&limits);
    let from = movie
        .packets_from(0, 5000, SliceSource::new(data), &limits, &mut budget)
        .unwrap();
    let mut middle = (0, None);
    for packet in from {
        match packet {
            Ok(_) => middle.0 += 1,
            Err(error) => middle.1 = Some(error),
        }
    }
    [one, runs, batches, visits, middle]
}

#[test]
fn frames_inside_the_file_iterate() {
    // 10,000 samples a chunk: 5 frames, 1,850 bytes, where the tables'
    // one byte a sample ends 8,150 bytes past the file.
    let data = qdm2_movie(&[10_000, 10_000], 0);
    let listed = listings(&data);
    for (i, (count, error)) in listed.iter().enumerate() {
        assert_eq!(*error, None, "listing {i}");
        assert_eq!(*count, if i == 4 { 15_000 } else { 20_000 }, "listing {i}");
    }
}

#[test]
fn frames_past_the_end_stop_as_truncated() {
    let data = qdm2_movie(&[10_000, 10_000], 1);
    for (i, (_, error)) in listings(&data).iter().enumerate() {
        if i == 4 {
            // Seeking stops before the samples it would list.
            continue;
        }
        assert!(
            matches!(error, Some(Error::Truncated { .. })),
            "listing {i}: {error:?}"
        );
    }
}
