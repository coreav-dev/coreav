//! Sample runs describe exactly the packets of per-sample
//! iteration: same offsets, sizes, decode and composition times and sync flags.

// Test helpers outside `#[test]` functions unwrap and panic like the tests.
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use common::*;
use coreav_isobmff::{
    Allowlist, Budget, Error, LimitKind, Limits, Movie, SampleRun, SliceSource, StoredSample, probe,
};

/// (offset, size, dts, cts, keyframe) of every sample.
type Row = (u64, u32, i64, i64, bool);

/// A small work budget keeps per-sample iteration of hostile counts quick.
fn limits() -> Limits {
    Limits {
        max_sample_work: 100_000,
        ..Limits::default()
    }
}

fn by_packets(movie: &Movie, data: &[u8], track: usize) -> Result<Vec<Row>, Error> {
    let limits = limits();
    let mut budget = Budget::new(&limits);
    movie
        .packets(track, SliceSource::new(data), &limits, &mut budget)?
        .map(|p| p.map(|p| (p.offset, p.size, p.dts, p.cts, p.keyframe)))
        .collect()
}

fn by_runs(movie: &Movie, data: &[u8], track: usize, max: u32) -> Result<(Vec<Row>, usize), Error> {
    let limits = limits();
    let mut budget = Budget::new(&limits);
    let mut packets = movie.packets(track, SliceSource::new(data), &limits, &mut budget)?;
    let (mut rows, mut runs) = (Vec::new(), 0);
    while let Some(run) = packets.next_run(max) {
        let run = run?;
        assert!(run.count >= 1 && run.count <= max.max(1));
        runs += 1;
        // Expand at most what the per-sample path could list under the budget.
        for i in 0..run.count.min(200_000) {
            let dts = run.sample_dts(i).unwrap();
            rows.push((
                run.sample_offset(i).unwrap(),
                run.sample_size,
                dts,
                dts + run.composition_offset,
                run.keyframe,
            ));
        }
        assert_eq!(run.sample_offset(run.count), None);
    }
    Ok((rows, runs))
}

/// Every run of `next_run`, the error that ended them, and the work used.
fn one_by_one(
    movie: &Movie,
    data: &[u8],
    track: usize,
    max: u32,
    limits: &Limits,
) -> (Vec<SampleRun>, Option<Error>, u64) {
    let limits = *limits;
    let mut budget = Budget::new(&limits);
    let mut runs = Vec::new();
    let mut error = None;
    if let Ok(mut packets) = movie.packets(track, SliceSource::new(data), &limits, &mut budget) {
        while let Some(run) = packets.next_run(max) {
            match run {
                Ok(run) => runs.push(run),
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        assert!(
            packets.next_run(max).is_none(),
            "nothing after the end or an error"
        );
    }
    (runs, error, budget.used())
}

/// The same from `next_runs` in batches of `batch`.
fn batched(
    movie: &Movie,
    data: &[u8],
    track: usize,
    max: u32,
    batch: usize,
    limits: &Limits,
) -> (Vec<SampleRun>, Option<Error>, u64) {
    let limits = *limits;
    let mut budget = Budget::new(&limits);
    let mut runs = Vec::new();
    let mut error = None;
    if let Ok(mut packets) = movie.packets(track, SliceSource::new(data), &limits, &mut budget) {
        loop {
            let before = runs.len();
            match packets.next_runs(max, &mut runs, batch) {
                Ok(0) => break,
                Ok(n) => {
                    assert!(n <= batch);
                    assert_eq!(runs.len() - before, n);
                }
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        assert_eq!(packets.next_runs(max, &mut runs, batch).ok(), Some(0));
    }
    (runs, error, budget.used())
}

/// Every sample from `next_run(1)`, the error that ended them and the work:
/// the reference for [`walked`].
fn expanded(
    movie: &Movie,
    data: &[u8],
    track: usize,
    limits: &Limits,
) -> (Vec<StoredSample>, Option<String>, u64) {
    let limits = *limits;
    let mut budget = Budget::new(&limits);
    let (mut samples, mut error) = (Vec::new(), None);
    if let Ok(mut packets) = movie.packets(track, SliceSource::new(data), &limits, &mut budget) {
        while let Some(run) = packets.next_run(1) {
            match run {
                Ok(run) => samples.push(StoredSample {
                    sample: run.first_sample,
                    offset: run.offset,
                    size: run.sample_size,
                    dts: run.dts,
                    duration: run.sample_duration,
                    composition_offset: run.composition_offset,
                    keyframe: run.keyframe,
                }),
                Err(e) => {
                    error = Some(e.to_string());
                    break;
                }
            }
        }
    }
    (samples, error, budget.used())
}

/// The same samples from `for_each_sample` (#308).
fn walked(
    movie: &Movie,
    data: &[u8],
    track: usize,
    limits: &Limits,
) -> (Vec<StoredSample>, Option<String>, u64) {
    let limits = *limits;
    let mut budget = Budget::new(&limits);
    let (mut samples, mut error) = (Vec::new(), None);
    if let Ok(mut packets) = movie.packets(track, SliceSource::new(data), &limits, &mut budget) {
        let seen = packets.for_each_sample(|sample| -> Result<(), Error> {
            samples.push(sample);
            Ok(())
        });
        match seen {
            Ok(seen) => assert_eq!(seen, samples.len() as u64, "track {track}"),
            Err(e) => error = Some(e.to_string()),
        }
        // Nothing after the end or an error.
        assert_eq!(
            packets
                .for_each_sample(|_| -> Result<(), Error> { Ok(()) })
                .ok(),
            Some(0)
        );
    }
    (samples, error, budget.used())
}

/// The walk of every sample (#308) visits the samples `next_run(1)` lists,
/// with the same error and the same work, under `limits`.
fn same_walk(movie: &Movie, data: &[u8], limits: &Limits) {
    for track in 0..movie.tracks.len() {
        let expected = expanded(movie, data, track, limits);
        let got = walked(movie, data, track, limits);
        if movie.has_fragments() {
            // A fragment is taken a run at a time and its samples are
            // charged before they are expanded, so
            // the walk charges the run's own step beside them and a
            // budget stops it at a run boundary where listing sample by
            // sample stops at a sample. What it does list is the same,
            // and it lists all of it whenever the budget lets it finish.
            let shared = got.0.len().min(expected.0.len());
            assert_eq!(
                got.0.get(..shared),
                expected.0.get(..shared),
                "samples: track {track}"
            );
            assert!(
                got.2 >= expected.2 || got.1.is_some(),
                "work: track {track}"
            );
            if got.1.is_none() {
                assert_eq!(got.0, expected.0, "samples: track {track}");
                assert_eq!(got.1, expected.1, "error: track {track}");
            }
        } else {
            assert_eq!(got.0, expected.0, "samples: track {track}");
            assert_eq!(got.1, expected.1, "error: track {track}");
            assert_eq!(got.2, expected.2, "work: track {track}");
        }
    }
}

/// Batches of runs (#30) are the runs one by one, with the same error and
/// the same work, under `limits`.
fn same_batches(movie: &Movie, data: &[u8], limits: &Limits) {
    for track in 0..movie.tracks.len() {
        for max in [1, 2, 3, u32::MAX] {
            let expected = one_by_one(movie, data, track, max, limits);
            for batch in [1, 2, 7, 256] {
                let got = batched(movie, data, track, max, batch, limits);
                assert_eq!(got.0, expected.0, "track {track}, max {max}, batch {batch}");
                assert_eq!(
                    got.1.as_ref().map(|e| e.to_string()),
                    expected.1.as_ref().map(|e| e.to_string()),
                    "track {track}, max {max}, batch {batch}"
                );
                assert_eq!(
                    got.2, expected.2,
                    "work: track {track}, max {max}, batch {batch}"
                );
            }
        }
    }
}

fn same(data: &[u8]) {
    let Ok(movie) = probe(SliceSource::new(data), &limits(), &Allowlist::default()) else {
        return;
    };
    same_batches(&movie, data, &limits());
    same_walk(&movie, data, &limits());
    for track in 0..movie.tracks.len() {
        let expected = by_packets(&movie, data, track);
        for max in [1, 3, 1024, u32::MAX] {
            let got = by_runs(&movie, data, track, max).map(|(rows, _)| rows);
            match (&expected, &got) {
                (Ok(a), Ok(b)) => assert_eq!(a, b, "track {track}, max {max}"),
                (Err(_), Err(_)) => {}
                // Runs are described once, so they can finish where listing
                // every sample runs out of work budget.
                (
                    Err(Error::LimitExceeded {
                        limit: LimitKind::SampleWork,
                        ..
                    }),
                    Ok(_),
                ) => {}
                _ => panic!("packets {expected:?} but runs {got:?} (max {max})"),
            }
        }
    }
}

#[test]
fn runs_match_packets_on_movie_tables() {
    same(&movie(&Spec::default()));
    same(&movie(&Spec {
        elst: None,
        sync: vec![],
        ..Spec::default()
    }));
    same(&movie(&Spec {
        sync: vec![1, 3],
        ..Spec::default()
    }));
}

#[test]
fn constant_runs_are_described_once() {
    // Equal sizes, no composition offsets, no sync table: one chunk, one run.
    let spec = Spec {
        sizes: vec![64; 40],
        composition: vec![0; 40],
        sync: vec![],
        elst: None,
        ..Spec::default()
    };
    let data = movie(&spec);
    let movie = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let (rows, runs) = by_runs(&movie, &data, 0, u32::MAX).unwrap();
    assert_eq!(rows.len(), 40);
    // The builder writes one composition entry per sample, so runs stop at
    // every entry; equal sizes alone do not merge across ctts entries.
    assert_eq!(runs, 40);
    // Fragment runs without per-sample fields are a single run.
    let data = fragmented_movie(None, None);
    let movie = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let (rows, runs) = by_runs(&movie, &data, 0, u32::MAX).unwrap();
    assert_eq!((rows.len(), runs), (4, 1));
    assert_eq!(rows, by_packets(&movie, &data, 0).unwrap());
    let (_, runs) = by_runs(&movie, &data, 0, 3).unwrap();
    assert_eq!(runs, 2);
}

#[test]
fn runs_match_packets_on_mutations() {
    let bases = [
        movie(&Spec::default()),
        fragmented_movie(None, Some((80, 0))),
    ];
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for base in &bases {
        for _ in 0..2048 {
            let mut data = base.clone();
            for _ in 0..(1 + next() % 3) {
                let at = (next() % data.len() as u64) as usize;
                data[at] = next() as u8;
            }
            same(&data);
        }
    }
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }

    /// `total` in parts of 1 to `most`, with a part of 0 now and then if
    /// `zeros`.
    fn split(&mut self, total: u32, most: u64, zeros: bool) -> Vec<u32> {
        let mut parts = Vec::new();
        let mut left = total;
        while left > 0 {
            if zeros && self.below(8) == 0 {
                parts.push(0);
            }
            let part = (1 + self.below(most) as u32).min(left);
            parts.push(part);
            left -= part;
        }
        parts
    }
}

/// Runs from `next_run` and `next_runs` mixed in the order of `pattern`
/// (0: one run, otherwise a batch of that many), as `one_by_one` returns
/// them.
fn mixed(
    movie: &Movie,
    data: &[u8],
    track: usize,
    max: u32,
    pattern: &[usize],
    limits: &Limits,
) -> (Vec<SampleRun>, Option<Error>, u64) {
    let limits = *limits;
    let mut budget = Budget::new(&limits);
    let mut runs = Vec::new();
    let mut error = None;
    if let Ok(mut packets) = movie.packets(track, SliceSource::new(data), &limits, &mut budget) {
        for &batch in pattern.iter().cycle() {
            if batch == 0 {
                match packets.next_run(max) {
                    None => break,
                    Some(Ok(run)) => runs.push(run),
                    Some(Err(e)) => {
                        error = Some(e);
                        break;
                    }
                }
            } else {
                match packets.next_runs(max, &mut runs, batch) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
        }
    }
    (runs, error, budget.used())
}

/// A fragment run without per-sample fields is computed, not scanned, so a
/// run that declares as many samples as the file can back costs one step
/// and the budget. The walk of every sample keeps
/// that: it charges the run's samples before it expands any of them, so
/// such a `trun` costs the budget and not the loop (#308). Since #289 a
/// declared count must be backed by the bytes of the file, so the run
/// below is one of 1-byte samples with the media data to match.
#[test]
fn a_huge_fragment_run_costs_one_step() {
    const SAMPLES: u32 = 300_000;
    let built = fragmented_movie(None, None);
    // `trex`: track id, description index, default duration, default size,
    // default flags. One byte per sample, so `SAMPLES` of them are backed
    // by `SAMPLES` bytes of media data.
    let trex = built.windows(4).position(|w| w == b"trex").expect("trex") + 8;
    // The builder's `trun` carries the data offset alone, so every sample
    // takes those defaults and nothing in the box bounds the count.
    let trun = built.windows(4).position(|w| w == b"trun").expect("trun") + 8;
    let mdat = built.windows(4).position(|w| w == b"mdat").expect("mdat") - 4;
    let mut data = built.get(..mdat).expect("the boxes before mdat").to_vec();
    data.get_mut(trex + 12..trex + 16)
        .expect("the default size")
        .copy_from_slice(&1u32.to_be_bytes());
    data.get_mut(trun..trun + 4)
        .expect("the sample count")
        .copy_from_slice(&SAMPLES.to_be_bytes());
    data.extend_from_slice(&(SAMPLES + 8).to_be_bytes());
    data.extend_from_slice(b"mdat");
    data.extend(std::iter::repeat_n(0xABu8, SAMPLES as usize));
    let limits = Limits {
        max_sample_work: 100_000,
        ..Limits::default()
    };
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let mut budget = Budget::new(&limits);
    let mut packets = movie
        .packets(0, SliceSource::new(&data), &limits, &mut budget)
        .unwrap();
    let mut seen = 0u64;
    let walked = packets.for_each_sample(|_| -> Result<(), Error> {
        seen += 1;
        Ok(())
    });
    assert!(
        matches!(
            walked,
            Err(Error::LimitExceeded {
                limit: LimitKind::SampleWork,
                ..
            })
        ),
        "{walked:?}"
    );
    // The run is charged before it is expanded, so none of its samples is
    // visited; expanding first would visit as many as the budget allows.
    assert_eq!(seen, 0);
}

/// A caller that stops the walk stops the iteration: the error is its own,
/// and every later call returns `Ok(0)`, as after an error of the crate's
/// (#308). Checked on the movie tables and on fragments, which the walk
/// reaches by different paths.
#[test]
fn a_caller_can_stop_the_walk() {
    for (what, data) in [
        ("tables", movie(&Spec::default())),
        ("fragments", fragmented_movie(None, Some((80, 0)))),
    ] {
        let limits = Limits::default();
        let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
        for stop_at in [1usize, 2, 3] {
            let mut budget = Budget::new(&limits);
            let mut packets = movie
                .packets(0, SliceSource::new(&data), &limits, &mut budget)
                .unwrap();
            let mut seen = 0usize;
            let stopped = packets.for_each_sample(|_| {
                seen += 1;
                if seen == stop_at {
                    // An error of the caller's own, told apart by its text.
                    return Err(Error::Unsupported { reason: "stop" });
                }
                Ok(())
            });
            let Err(error) = stopped else {
                // Fewer samples than `stop_at`: nothing to check.
                continue;
            };
            assert_eq!(error.to_string(), "unsupported: stop", "{what}");
            assert_eq!(seen, stop_at, "{what}");
            assert_eq!(
                packets
                    .for_each_sample(|_| -> Result<(), Error> { Ok(()) })
                    .ok(),
                Some(0),
                "{what}: nothing after the caller stopped the walk"
            );
            assert!(packets.next_run(1).is_none(), "{what}");
        }
    }
}

/// Tables that reach the batch loop's fast path (#30): several chunks and
/// `stsc` entries, `stts` and `ctts` entries spanning several samples (some
/// with a count of 0), no `ctts`, equal and varying sizes, and `stss` absent,
/// sorted or hostile (unsorted, repeated, 0, past the end); a quarter of
/// them mutated; each under the test budget and under small ones.
#[test]
fn batched_runs_match_on_varied_tables() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut probed = 0;
    for _ in 0..1500 {
        let n = 1 + rng.below(40) as u32;
        let sizes: Vec<u32> = match rng.below(3) {
            0 => vec![24; n as usize],
            1 => (0..n)
                .map(|_| [24, 24, 24, 40][rng.below(4) as usize])
                .collect(),
            _ => (0..n).map(|_| 8 + rng.below(64) as u32).collect(),
        };
        let stts = rng
            .split(n, 12, true)
            .into_iter()
            .map(|count| (count, [512, 1024, 0][rng.below(3) as usize]))
            .collect();
        let composition = (0..n)
            .map(|_| [0, 512, 1024][rng.below(3) as usize])
            .collect();
        let ctts = match rng.below(3) {
            0 => Some(vec![]),
            1 => Some(
                rng.split(n, 12, true)
                    .into_iter()
                    .map(|count| (count, [0, 512, 1024, -512][rng.below(4) as usize]))
                    .collect(),
            ),
            _ => None,
        };
        let sync = match rng.below(4) {
            0 => vec![],
            1 => (1..=n).filter(|_| rng.below(5) == 0).collect(),
            2 => (1..=n).filter(|_| rng.below(2) == 0).collect(),
            _ => (0..1 + rng.below(6))
                .map(|_| rng.below(u64::from(n) + 3) as u32)
                .collect(),
        };
        let chunks = (rng.below(4) != 0).then(|| rng.split(n, 9, false));
        let mut data = movie(&Spec {
            sizes,
            composition,
            sync,
            elst: None,
            stts: Some(stts),
            ctts,
            chunks,
            ..Spec::default()
        });
        if rng.below(4) == 0 {
            for _ in 0..1 + rng.below(3) {
                let at = rng.below(data.len() as u64) as usize;
                data[at] = rng.below(256) as u8;
            }
        }
        same(&data);
        let Ok(movie) = probe(SliceSource::new(&data), &limits(), &Allowlist::default()) else {
            continue;
        };
        probed += 1;
        for work in [3, 5, 8, 13, 21] {
            let small = Limits {
                max_sample_work: work,
                ..limits()
            };
            same_batches(&movie, &data, &small);
            same_walk(&movie, &data, &small);
        }
        let pattern: Vec<usize> = (0..1 + rng.below(5))
            .map(|_| [0, 0, 1, 2, 7][rng.below(5) as usize])
            .collect();
        for work in [8, 100_000] {
            let limits = Limits {
                max_sample_work: work,
                ..limits()
            };
            for track in 0..movie.tracks.len() {
                for max in [1, 2, u32::MAX] {
                    assert_eq!(
                        format!("{:?}", mixed(&movie, &data, track, max, &pattern, &limits)),
                        format!("{:?}", one_by_one(&movie, &data, track, max, &limits)),
                        "pattern {pattern:?}, max {max}, work {work}"
                    );
                }
            }
        }
    }
    // Most generated movies are valid, so the comparisons above ran.
    assert!(probed > 1000, "{probed} movies probed");
}

/// The batch equivalence of `same` on real files: the paths in
/// `COREAV_RUNS_FILES`, separated by `:` (#30; run with `--ignored`).
#[test]
#[ignore = "needs COREAV_RUNS_FILES"]
fn batched_runs_match_on_files() {
    let paths = std::env::var("COREAV_RUNS_FILES").unwrap_or_default();
    let limits = Limits::default();
    let mut checked = 0;
    for path in paths.split(':').filter(|p| !p.is_empty()) {
        let data = std::fs::read(path).unwrap();
        let Ok(movie) = probe(SliceSource::new(&data), &limits, &Allowlist::default()) else {
            continue;
        };
        for track in 0..movie.tracks.len() {
            let run_all = |batch: Option<usize>| {
                let mut budget = Budget::new(&limits);
                let mut runs = Vec::new();
                let Ok(mut packets) =
                    movie.packets(track, SliceSource::new(&data), &limits, &mut budget)
                else {
                    return (runs, None, 0);
                };
                let mut error = None;
                match batch {
                    None => {
                        while let Some(run) = packets.next_run(u32::MAX) {
                            match run {
                                Ok(run) => runs.push(run),
                                Err(e) => {
                                    error = Some(e.to_string());
                                    break;
                                }
                            }
                        }
                    }
                    Some(batch) => loop {
                        match packets.next_runs(u32::MAX, &mut runs, batch) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(e) => {
                                error = Some(e.to_string());
                                break;
                            }
                        }
                    },
                }
                drop(packets);
                (runs, error, budget.used())
            };
            let expected = run_all(None);
            assert_eq!(run_all(Some(256)), expected, "{path} track {track}");
            checked += expected.0.len();
            // The walk of every sample (#308) lists the same samples.
            assert_eq!(
                walked(&movie, &data, track, &limits),
                expanded(&movie, &data, track, &limits),
                "{path} track {track}"
            );
        }
    }
    eprintln!("{checked} runs compared");
}
