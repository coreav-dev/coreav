//! `Movie::packets_from(i)` lists exactly what `Movie::packets()` lists
//! after skipping `i` packets (`docs/design/seek.md`): on generated sample
//! tables of every shape the builder makes (a seeded property test), on
//! hostile ones, on movie fragments, and on the fixtures of
//! `scripts/gen-fixtures.sh` when `COREAV_FIXTURES` names them.

// Test helpers outside `#[test]` functions unwrap and panic like the tests.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use common::*;
use coreav_isobmff::{
    Allowlist, Budget, Error, Limits, Movie, Packet, SliceSource, StoredSample, probe,
};

type Listing = Vec<Result<Packet, Error>>;

fn skipped(movie: &Movie, data: &[u8], track: usize, start: usize, limits: &Limits) -> Listing {
    let mut budget = Budget::new(limits);
    match movie.packets(track, SliceSource::new(data), limits, &mut budget) {
        Ok(packets) => packets.skip(start).collect(),
        Err(error) => vec![Err(error)],
    }
}

fn from(movie: &Movie, data: &[u8], track: usize, start: usize, limits: &Limits) -> Listing {
    let mut budget = Budget::new(limits);
    match movie.packets_from(
        track,
        start as u64,
        SliceSource::new(data),
        limits,
        &mut budget,
    ) {
        Ok(packets) => packets.collect(),
        Err(error) => vec![Err(error)],
    }
}

/// What `for_each_sample` visits from `packets_from(start)`, and whether it
/// ended with an error.
fn visited(
    movie: &Movie,
    data: &[u8],
    track: usize,
    start: usize,
    limits: &Limits,
) -> (Vec<StoredSample>, bool) {
    let mut budget = Budget::new(limits);
    let Ok(mut packets) = movie.packets_from(
        track,
        start as u64,
        SliceSource::new(data),
        limits,
        &mut budget,
    ) else {
        return (Vec::new(), true);
    };
    let mut out = Vec::new();
    let result = packets.for_each_sample(|s| {
        out.push(s);
        Ok::<(), Error>(())
    });
    (out, result.is_err())
}

/// The listing as `for_each_sample` gives its samples.
fn stored(listing: &Listing) -> (Vec<StoredSample>, bool) {
    let samples = listing
        .iter()
        .filter_map(|p| p.as_ref().ok())
        .map(|p| StoredSample {
            sample: p.sample,
            offset: p.offset,
            size: p.size,
            dts: p.dts,
            duration: p.duration,
            composition_offset: p.cts - p.dts,
            keyframe: p.keyframe,
        })
        .collect();
    (samples, listing.iter().any(Result::is_err))
}

/// Every start from 0 to past the end (all of them up to `spread` packets,
/// else `spread` spread over the listing and its edges), on every track;
/// returns the starts checked and whether a listing ended with an error.
fn same_at(data: &[u8], limits: &Limits, spread: usize) -> (usize, bool) {
    let movie = probe(SliceSource::new(data), limits, &Allowlist::default()).unwrap();
    let mut checked = 0;
    let mut failed = false;
    for track in 0..movie.tracks.len() {
        let all = skipped(&movie, data, track, 0, limits);
        let (from_start, from_start_error) = visited(&movie, data, track, 0, limits);
        failed |= all.iter().any(Result::is_err);
        let n = all.len();
        let starts: Vec<usize> = if n <= spread {
            (0..=n + 2).collect()
        } else {
            (0..spread)
                .map(|k| k * n / spread)
                .chain([n - 1, n, n + 1])
                .collect()
        };
        for start in starts {
            let expected = skipped(&movie, data, track, start, limits);
            let got = from(&movie, data, track, start, limits);
            assert_eq!(got, expected, "track {track}, start {start} of {n}");
            // The fused walk from the same position visits what it visits
            // from the start after `start` samples, wherever the listing
            // reached `start` (the fused walk reports stored offsets and
            // computes no composition time, so it does not stop where one
            // overflows; `stored` holds for the listing's samples).
            if !all.iter().take(start).any(Result::is_err) {
                let (seen, error) = visited(&movie, data, track, start, limits);
                let (want, want_error) = (
                    from_start.get(start..).unwrap_or(&[]).to_vec(),
                    from_start_error,
                );
                assert_eq!(error, want_error, "track {track}, start {start}: error");
                if error {
                    assert!(
                        want.starts_with(&seen),
                        "track {track}, start {start}: prefix"
                    );
                } else {
                    assert_eq!(seen, want, "track {track}, start {start}: for_each_sample");
                }
                if !expected.iter().any(Result::is_err) {
                    assert_eq!(
                        seen,
                        stored(&expected).0,
                        "track {track}, start {start}: stored"
                    );
                }
            }
            checked += 1;
        }
    }
    (checked, failed)
}

fn same(data: &[u8], limits: &Limits) -> (usize, bool) {
    same_at(data, limits, 400)
}

/// xorshift64: a fixed generator, so a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    /// `total` split into parts of at most `max`, zeros allowed when
    /// `zeros`.
    fn split(&mut self, total: u32, max: u32, zeros: bool) -> Vec<u32> {
        let mut out = Vec::new();
        let mut left = total;
        while left > 0 {
            let part = (self.below(u64::from(max.min(left))) as u32 + 1).min(left);
            if zeros && self.below(5) == 0 {
                out.push(0);
            }
            out.push(part);
            left -= part;
        }
        out
    }
}

fn random_spec(rng: &mut Rng) -> Spec {
    let n = rng.below(60) as u32 + 1;
    let sizes: Vec<u32> = (0..n)
        .map(|_| match rng.below(4) {
            0 => 0,
            1 => 64,
            _ => rng.below(300) as u32,
        })
        .collect();
    let stts = rng
        .split(n, 9, true)
        .into_iter()
        .map(|count| (count, rng.below(3000) as u32))
        .collect();
    let ctts = match rng.below(3) {
        0 => Some(Vec::new()),
        _ => Some(
            rng.split(n, 5, false)
                .into_iter()
                .map(|count| (count, rng.below(4000) as i32 - 1500))
                .collect(),
        ),
    };
    let sync = match rng.below(3) {
        0 => Vec::new(),
        _ => (1..=n).filter(|_| rng.below(4) == 0).collect(),
    };
    let elst = match rng.below(4) {
        0 => None,
        1 => Some(vec![(80, rng.below(2000) as i32)]),
        2 => Some(vec![(30, -1), (80, rng.below(1500) as i32)]),
        _ => Some(vec![(40, 0), (40, 2048), (40, 512)]),
    };
    Spec {
        sizes,
        sync,
        elst,
        stts: Some(stts),
        ctts,
        chunks: Some(rng.split(n, 7, true)),
        ..Spec::default()
    }
}

#[test]
fn from_any_sample_equals_skipping_on_generated_tables() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let limits = Limits::default();
    let mut checked = 0;
    for _ in 0..400 {
        let spec = random_spec(&mut rng);
        checked += same(&movie(&spec), &limits).0;
    }
    assert!(checked > 10_000, "{checked} starts checked");
}

#[test]
fn errors_before_the_start_leave_nothing_and_after_it_come_in_place() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut truncated, mut over_limit) = (0, 0);
    for _ in 0..200 {
        let spec = random_spec(&mut rng);
        let n = spec.sizes.len();
        let mut data = movie(&spec);
        // A sample past the end of the file: overwrite one listed size
        // (after the box type: version and flags, sample_size, count).
        let stsz = data.windows(4).position(|w| w == b"stsz").unwrap();
        let victim = rng.below(n as u64) as usize;
        let at = stsz + 4 + 4 + 4 + 4 + 4 * victim;
        data[at..at + 4].copy_from_slice(&0x7fff_fff0u32.to_be_bytes());
        truncated += usize::from(same(&data, &Limits::default()).1);
        // And a packet limit some samples break.
        let limits = Limits {
            max_packet_bytes: 150,
            ..Limits::default()
        };
        over_limit += usize::from(same(&movie(&spec), &limits).1);
    }
    assert!(
        truncated > 150 && over_limit > 100,
        "{truncated} {over_limit}"
    );
}

#[test]
fn from_any_sample_equals_skipping_on_fragments() {
    let limits = Limits::default();
    for data in [
        fragmented_movie(None, None),
        fragmented_movie(Some(4096), Some((100, 512))),
        implicit_base_fragments().0,
        two_track_fragmented_movie(b"avc1", false),
    ] {
        assert!(same(&data, &limits).0 > 0);
    }
}

#[test]
fn from_any_sample_equals_skipping_on_the_fixtures() {
    let Some(dir) = std::env::var_os("COREAV_FIXTURES") else {
        eprintln!("COREAV_FIXTURES is not set; the fixtures were not checked");
        return;
    };
    let limits = Limits::default();
    let mut files = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "mp4" | "mov" | "m4a" | "m4v" | "3gp") {
            continue;
        }
        let data = std::fs::read(&path).unwrap();
        if probe(SliceSource::new(&data), &limits, &Allowlist::default()).is_err() {
            continue;
        }
        // Each start lists the track from the beginning to compare, so the
        // long fixtures get 40 starts a track.
        same_at(&data, &limits, 40);
        files += 1;
    }
    assert!(files > 10, "{files} fixtures checked");
}

#[test]
fn seeker_lookups_agree_with_the_listing() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let limits = Limits::default();
    for _ in 0..400 {
        let data = movie(&random_spec(&mut rng));
        let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
        let mut budget = Budget::new(&limits);
        let all: Vec<Packet> = movie
            .packets(0, SliceSource::new(&data), &limits, &mut budget)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut budget = Budget::new(&limits);
        let seeker = movie.seeker(0, &mut budget).unwrap().unwrap();
        let n = all.len() as u32;
        assert_eq!(seeker.sample_count(), n);
        for (i, p) in all.iter().enumerate() {
            assert_eq!(seeker.dts(i as u32), Some(p.dts), "dts of {i}");
            let sync = all[..=i].iter().rposition(|q| q.keyframe).map(|k| k as u32);
            assert_eq!(
                seeker.sync_at_or_before(i as u32),
                sync,
                "sync at or before {i}"
            );
        }
        let end = all.last().map_or(0, |p| p.dts + i64::from(p.duration));
        assert_eq!(seeker.dts(n), Some(end));
        assert_eq!(seeker.dts(n + 1), None);
        assert_eq!(
            seeker.first_sync(),
            all.iter().position(|p| p.keyframe).map(|k| k as u32)
        );
        let offsets = all.iter().map(|p| p.cts - p.dts);
        let range = (
            offsets.clone().min().unwrap_or(0),
            offsets.max().unwrap_or(0),
        );
        assert_eq!(seeker.composition_offsets(), range);
        // Every decode time around the samples', and far outside them.
        let mut ticks: Vec<i64> = all
            .iter()
            .flat_map(|p| [p.dts - 1, p.dts, p.dts + 1])
            .collect();
        ticks.extend([i64::MIN, -1_000_000, end, end + 1, i64::MAX]);
        for t in ticks {
            let through = all.iter().filter(|p| p.dts <= t).count() as u32;
            assert_eq!(seeker.samples_through(t), through, "samples through {t}");
        }
    }
}

/// One fragment of four 100-byte samples of one tick each whose decode
/// times start at `i64::MAX - 100`, with composition offsets
/// `[2000, 0, 0, 0]`: the first sample's composition time overflows.
fn overflowing_fragment() -> Vec<u8> {
    let data = fragmented_movie(None, None);
    let moof = data.windows(4).position(|w| w == b"moof").unwrap() - 4;
    let head = &data[..moof];
    // The movie box as built, with trex's default duration set to 1.
    let mut head = head.to_vec();
    let trex = head.windows(4).position(|w| w == b"trex").unwrap();
    // trex after its type: version and flags, track id, description,
    // duration.
    head[trex + 16..trex + 20].copy_from_slice(&1u32.to_be_bytes());
    let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
    let start = (i64::MAX - 100) as u64;
    let tfdt = full(b"tfdt", 1, 0, &start.to_be_bytes());
    // trun: data offset and per-sample composition offsets.
    let trun_len = 8 + 4 + 8 + 4 * 4;
    let traf_len = 8 + tfhd.len() + tfdt.len() + trun_len;
    let moof_len = 8 + 16 + traf_len;
    let trun = full(
        b"trun",
        0,
        0x1 | 0x800,
        &u32s(&[4, (moof_len + 8) as u32, 2000, 0, 0, 0]),
    );
    let moof = bx(
        b"moof",
        &cat(&[
            full(b"mfhd", 0, 0, &u32s(&[1])),
            bx(b"traf", &cat(&[tfhd, tfdt, trun])),
        ]),
    );
    assert_eq!(moof.len(), moof_len);
    cat(&[head, moof, bx(b"mdat", &[0xAB; 400])])
}

#[test]
fn an_overflowing_composition_time_in_a_fragment_stops_both_listings() {
    let data = overflowing_fragment();
    let limits = Limits::default();
    let movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    let all = skipped(&movie, &data, 0, 0, &limits);
    assert!(matches!(all.as_slice(), [Err(_)]), "{all:?}");
    assert!(same(&data, &limits).1);
    assert_eq!(from(&movie, &data, 0, 1, &limits), Vec::new());
}
