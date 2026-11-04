//! ISO ancillary bit rates use encoded media spans rather than edit windows.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn movie(subtitle: bool, edits: Option<Vec<(u32, i32)>>, header: u32, samples: u32) -> Vec<u8> {
    let mut track = if subtitle {
        common::TrackSpec::tx3g(1, vec![(common::text_sample(b"x", &[]), samples)])
    } else {
        let mut entry = vec![0; 26];
        entry[7] = 1;
        entry[16..20].copy_from_slice(&1000u32.to_be_bytes());
        entry[20..24].copy_from_slice(&40u32.to_be_bytes());
        entry[24] = 25;
        common::TrackSpec {
            handler: *b"tmcd",
            entry: common::bx(b"tmcd", &entry),
            ..common::TrackSpec::qt_text(1, vec![(vec![0; 4], samples)])
        }
    };
    track.elst = edits;
    let mut bytes = common::movie(&[track]);
    let duration = bytes.windows(4).position(|w| w == b"mdhd").unwrap() + 20;
    bytes[duration..duration + 4].copy_from_slice(&header.to_be_bytes());
    bytes
}

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new(name: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ancillary-rate-{name}-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(path: &Path, writer: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-of",
            writer,
            "-show_entries",
            "stream=bit_rate",
        ])
        .arg(path)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap()
}

#[test]
fn ancillary_edits_do_not_change_the_encoded_rate() {
    let dir = FixtureDir::new("edits");
    for subtitle in [false, true] {
        for (index, edits) in [
            None,
            Some(vec![(1000, 0)]),
            Some(vec![(4000, 0)]),
            Some(vec![(500, -1), (1000, 0)]),
            Some(vec![(1000, 400)]),
            Some(vec![(500, 0), (500, 1000)]),
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.0.join(format!("{subtitle}-{index}.mov"));
            std::fs::write(&path, movie(subtitle, edits, 2000, 2000)).unwrap();
            let expected = if subtitle { "12" } else { "16" };
            let output = run(&path, "json");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                doc["streams"][0]["bit_rate"], expected,
                "{subtitle} {index}"
            );
            for writer in ["csv=p=0", "default=noprint_wrappers=1:nokey=1"] {
                let output = run(&path, writer);
                assert!(output.status.success());
                assert_eq!(output.stdout, format!("{expected}\n").as_bytes());
            }
        }
    }
}

#[test]
fn ancillary_rates_require_a_positive_header_and_bound_it_by_known_samples() {
    let dir = FixtureDir::new("bounds");
    for subtitle in [false, true] {
        for (index, header, samples, factor) in [
            (0, 1000, 2000, Some(2)),
            (1, 4000, 2000, Some(1)),
            (2, 0, 2000, None),
            (3, 2000, 0, Some(1)),
            (4, 0, 0, None),
        ] {
            let path = dir.0.join(format!("{subtitle}-{index}.mov"));
            std::fs::write(
                &path,
                movie(subtitle, Some(vec![(1000, 0)]), header, samples),
            )
            .unwrap();
            let output = run(&path, "json");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let expected = factor.map(|n| (n * if subtitle { 12 } else { 16 }).to_string());
            assert_eq!(
                doc["streams"][0].get("bit_rate").and_then(|v| v.as_str()),
                expected.as_deref(),
                "{subtitle} {index}"
            );
        }
    }
}
