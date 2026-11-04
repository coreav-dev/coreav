//! ISO format timing from audio/video and ancillary stream spans.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn movie(delay: u32, durations: &[u32], subtitle: bool, video: bool) -> Vec<u8> {
    let mut tracks = Vec::new();
    if video {
        let mut track = common::TrackSpec::video(1, &[40; 50]);
        for (sample, _) in &mut track.samples {
            sample[..6].copy_from_slice(&[0, 0, 0, 96, 0x65, 0x88]);
        }
        if delay > 0 {
            track.elst = Some(vec![(delay, -1), (2000, 0)]);
        }
        tracks.push(track);
    }
    for (index, duration) in durations.iter().enumerate() {
        let id = index as u32 + 2;
        if subtitle {
            tracks.push(common::TrackSpec::tx3g(
                id,
                vec![(common::text_sample(b"x", &[]), *duration)],
            ));
        } else {
            let mut entry = vec![0; 26];
            entry[7] = 1;
            entry[16..20].copy_from_slice(&1000u32.to_be_bytes());
            entry[20..24].copy_from_slice(&40u32.to_be_bytes());
            entry[24] = 25;
            tracks.push(common::TrackSpec {
                handler: *b"tmcd",
                entry: common::bx(b"tmcd", &entry),
                ..common::TrackSpec::qt_text(id, vec![(vec![0; 4], *duration)])
            });
        }
    }
    common::movie(&tracks)
}

fn run(path: &Path, writer: &str, entries: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-of",
            writer,
            "-show_entries",
            entries,
        ])
        .arg(path)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap()
}

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new(name: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("format-span-{name}-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ancillary_timing_reaches_duration_writers_and_format_bit_rate() {
    let dir = FixtureDir::new("writers");
    for (case, delay, duration, expected) in [
        ("near", 0, 2500, 2_500_000),
        ("boundary", 0, 3000, 2_000_000),
        ("early", 500, 2000, 2_500_000),
        ("early-far", 500, 4000, 2_500_000),
    ] {
        for subtitle in [false, true] {
            let bytes = movie(delay, &[duration], subtitle, true);
            let path = dir.0.join(format!("{case}-{subtitle}.mov"));
            std::fs::write(&path, &bytes).unwrap();
            let text = format!("{}.{:06}", expected / 1_000_000, expected % 1_000_000);
            let output = run(&path, "json", "format=start_time,duration,bit_rate");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(doc["format"]["start_time"], "0.000000", "{case}");
            assert_eq!(doc["format"]["duration"], text, "{case} {subtitle}");
            let rate = bytes.len() as u128 * 8 * 1_000_000 / expected as u128;
            assert_eq!(doc["format"]["bit_rate"], rate.to_string(), "{case}");
            for writer in ["default=noprint_wrappers=1:nokey=1", "csv=p=0"] {
                let output = run(&path, writer, "format=duration");
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert_eq!(
                    output.stdout,
                    format!("{text}\n").as_bytes(),
                    "{writer} {case}"
                );
            }
        }
    }
}

#[test]
fn ancillary_cutoff_is_aggregate_and_order_independent() {
    let dir = FixtureDir::new("aggregate");
    for (index, durations) in [[2900, 3800], [3800, 2900]].into_iter().enumerate() {
        let path = dir.0.join(format!("{index}.mov"));
        std::fs::write(&path, movie(0, &durations, false, true)).unwrap();
        let output = run(&path, "csv=p=0", "format=duration");
        assert!(output.status.success());
        assert_eq!(output.stdout, b"2.000000\n");
    }
}

#[test]
fn secondary_only_movies_keep_their_known_span() {
    let dir = FixtureDir::new("secondary-only");
    let path = dir.0.join("input.mov");
    std::fs::write(&path, movie(0, &[2500, 4000], false, false)).unwrap();
    let output = run(&path, "csv=p=0", "format=duration");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"4.000000\n");
}

#[test]
fn output_span_does_not_change_copy_planning_or_trimmed_bytes() {
    let dir = FixtureDir::new("copy");
    let input = dir.0.join("input.mov");
    let output = dir.0.join("output.mp4");
    let bytes = movie(500, &[4000], false, true);
    std::fs::write(&input, &bytes).unwrap();
    let mut source = coreav_isobmff::SliceSource::new(&bytes);
    let probed = coreav_cli::ffprobe::probe(&mut source, bytes.len() as u64, "input.mov", None)
        .unwrap_or_else(|error| panic!("{}", error.to_json()));
    for (key, expected) in [("start_time", 0.5), ("duration", 2.0)] {
        assert!(probed.report.format.iter().any(|(name, value)| {
            *name == key
                && matches!(value, coreav_cli::ffprobe::model::Value::Time(time) if *time == expected)
        }));
    }
    let printed = run(&input, "csv=p=0", "format=start_time,duration");
    assert!(printed.status.success());
    assert_eq!(printed.stdout, b"0.000000,2.500000\n");
    let copied = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["ffmpeg", "-v", "error", "-ss", "0", "-i"])
        .arg(&input)
        .args(["-t", "1", "-map", "0:v", "-c", "copy"])
        .arg(&output)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stderr)
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(std::fs::read(output).unwrap())),
        "4064dba9ec64429a954c8492f77e7934b63348a6bc4d77ea2fb1346511d49c92"
    );
}
