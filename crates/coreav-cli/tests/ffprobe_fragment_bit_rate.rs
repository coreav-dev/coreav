//! Fragmented bitrate estimates keep encoded bytes and durations paired.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use common::fragments::{Fragmented, Track};
use common::{bx, cat, full, u32s};
use coreav_isobmff::{Allowlist, FragmentTotals, Limits, Movie, SliceSource, Source};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fragment(sequence: u32, start: u64, samples: &[(u32, u32)], explicit: bool) -> Vec<u8> {
    let build = |offset: u32| {
        let (duration, size) = samples.first().copied().unwrap_or((1024, 300));
        let tfhd = full(b"tfhd", 0, 0x02_0038, &u32s(&[1, duration, size, 0]));
        let tfdt = full(b"tfdt", 1, 0, &start.to_be_bytes());
        let chunks: Vec<_> = if explicit && !samples.is_empty() {
            samples.chunks(1).collect()
        } else {
            vec![samples]
        };
        let mut truns = Vec::new();
        let mut offset = offset;
        for chunk in chunks {
            let mut trun = u32s(&[chunk.len() as u32, offset]);
            if explicit {
                for (duration, size) in chunk {
                    trun.extend_from_slice(&u32s(&[*duration, *size]));
                }
            }
            truns.extend_from_slice(&full(b"trun", 0, if explicit { 0x301 } else { 1 }, &trun));
            offset += chunk.iter().map(|(_, size)| size).sum::<u32>();
        }
        bx(
            b"moof",
            &cat(&[
                full(b"mfhd", 0, 0, &u32s(&[sequence])),
                bx(b"traf", &cat(&[tfhd, tfdt, truns])),
            ]),
        )
    };
    let moof = build(build(0).len() as u32 + 8);
    let bytes = samples.iter().map(|(_, size)| *size as usize).sum();
    cat(&[moof, bx(b"mdat", &vec![0xab; bytes])])
}

fn header(tracks: &[Track]) -> Vec<u8> {
    let mut source = Fragmented::new(tracks, 1);
    let mut header = vec![0; source.len().unwrap() as usize];
    source.read_exact_at(0, &mut header).unwrap();
    let first_moof = header.windows(4).position(|w| w == b"moof").unwrap() - 4;
    header.truncate(first_moof);
    header
}

fn movie(indexed: bool, start: u64, explicit: bool, first_duration: u32) -> Vec<u8> {
    let mut header = header(&[Track::AAC]);
    let first = fragment(1, start, &[(first_duration, 300); 2], explicit);
    let second = fragment(
        2,
        start + u64::from(first_duration) * 2,
        &[(2048, 400); 3],
        explicit,
    );
    if indexed {
        let mut index = u32s(&[1, 48_000, 0, 0]);
        index.extend_from_slice(&[0, 0, 0, 2]);
        index.extend_from_slice(&u32s(&[
            first.len() as u32,
            first_duration * 2,
            0x9000_0000,
        ]));
        index.extend_from_slice(&u32s(&[second.len() as u32, 6144, 0x9000_0000]));
        header.extend_from_slice(&full(b"sidx", 0, 0, &index));
    }
    header.extend_from_slice(&first);
    header.extend_from_slice(&second);
    header
}

fn stream(name: &str, bytes: &[u8]) -> serde_json::Value {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "fragment-bit-rate-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).unwrap();
    let dir = FixtureDir(dir);
    let path = dir.0.join(name);
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "stream=bit_rate,start_pts,duration_ts",
            "-of",
            "json",
        ])
        .arg(&path)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    value["streams"][0].clone()
}

#[test]
fn fragment_bit_rates_pair_bytes_with_their_encoded_duration() {
    for indexed in [false, true] {
        for start in [0, 96_000] {
            for explicit in [false, true] {
                let result = stream(
                    &format!("fragment-bit-rate-{indexed}-{start}-{explicit}.m4a"),
                    &movie(indexed, start, explicit, 1024),
                );
                assert_eq!(result["start_pts"], start);
                assert_eq!(result["duration_ts"], start + 8192);
                assert_eq!(
                    result["bit_rate"],
                    if indexed { "112500" } else { "84375" },
                    "indexed={indexed}, start={start}, explicit={explicit}"
                );
            }
        }
    }
}

#[test]
fn a_zero_duration_first_fragment_does_not_borrow_a_later_span() {
    let result = stream("fragment-bit-rate-zero.m4a", &movie(true, 0, true, 0));
    assert!(result.get("bit_rate").is_none());
}

#[test]
fn fragment_bit_rates_round_to_the_nearest_integer() {
    let result = stream("fragment-bit-rate-rounded.m4a", &movie(true, 0, true, 1023));
    assert_eq!(result["bit_rate"], "112610");
}

#[test]
fn non_fragmented_bit_rates_keep_their_media_span() {
    let result = stream(
        "fragment-bit-rate-regular.mp4",
        &common::movie(&common::Spec::default()),
    );
    assert_eq!(result["bit_rate"], "14000");
}

fn scan(bytes: &[u8]) -> Movie {
    let limits = Limits::default();
    let mut movie =
        coreav_isobmff::probe(SliceSource::new(bytes), &limits, &Allowlist::listing()).unwrap();
    movie
        .scan_fragments(SliceSource::new(bytes), &limits)
        .unwrap();
    movie
}

#[test]
fn fragment_totals_keep_one_fragment_identity_across_runs_and_rescans() {
    let mut bytes = header(&[Track::AAC]);
    bytes.extend_from_slice(&fragment(1, 0, &[], true));
    bytes.extend_from_slice(&fragment(2, 96_000, &[(700, 200), (900, 300)], true));
    bytes.extend_from_slice(&fragment(3, 97_600, &[(2048, 400); 3], false));
    let expected = Some(FragmentTotals {
        bytes: 500,
        duration: 1600,
    });
    let mut movie = scan(&bytes);
    assert_eq!(movie.tracks[0].first_fragment_totals, expected);
    assert_eq!(movie.tracks[0].sample_bytes, Some(1700));
    assert_eq!(movie.tracks[0].sample_duration, 7744);
    movie
        .scan_fragments(SliceSource::new(&bytes), &Limits::default())
        .unwrap();
    assert_eq!(movie.tracks[0].first_fragment_totals, expected);
    assert_eq!(movie.tracks[0].sample_bytes, Some(1700));
    assert_eq!(movie.tracks[0].sample_duration, 7744);
}

#[test]
fn each_track_selects_its_own_first_sample_bearing_fragment() {
    let mut bytes = header(&[Track::AAC; 2]);
    bytes.extend_from_slice(&fragment(1, 0, &[], true));
    let mut second_track = fragment(2, 0, &[(512, 100); 3], true);
    let tfhd = second_track.windows(4).position(|w| w == b"tfhd").unwrap();
    second_track[tfhd + 8..tfhd + 12].copy_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&second_track);
    bytes.extend_from_slice(&fragment(3, 0, &[(1024, 300); 2], false));
    let movie = scan(&bytes);
    assert_eq!(
        movie.tracks[0].first_fragment_totals,
        Some(FragmentTotals {
            bytes: 600,
            duration: 2048
        })
    );
    assert_eq!(
        movie.tracks[1].first_fragment_totals,
        Some(FragmentTotals {
            bytes: 300,
            duration: 1536
        })
    );
}

#[test]
fn a_zero_duration_first_sample_bearing_fragment_stays_selected() {
    let movie = scan(&movie(true, 0, true, 0));
    assert_eq!(
        movie.tracks[0].first_fragment_totals,
        Some(FragmentTotals {
            bytes: 600,
            duration: 0
        })
    );
}

#[test]
fn uncountable_first_fragment_bytes_stay_unknown() {
    let mut bytes = header(&[Track::AAC]);
    bytes.extend_from_slice(&fragment(1, i64::MAX as u64 - 1, &[(1024, 300); 2], false));
    bytes.extend_from_slice(&fragment(2, 0, &[(1024, 300); 2], false));
    let movie = scan(&bytes);
    assert_eq!(movie.tracks[0].first_fragment_totals, None);
    assert_eq!(movie.tracks[0].sample_bytes, None);
}
