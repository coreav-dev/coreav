//! A configured video clock can use a single bounded tail timestamp.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-mpegts/tests/common/mod.rs"]
mod common;

use common::Mux;
use coreav_isobmff::SliceSource;
use coreav_mpegts::{Allowlist, Codec, Limits};
use std::path::{Path, PathBuf};
use std::process::Command;

fn capture(clock: bool, timed: bool) -> Vec<u8> {
    let mut mux = Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(2, 0x101)]);
    for index in 0..12 {
        if index == 11 {
            for _ in 0..4000 {
                mux.packet(0x1fff, false, &[0xff; 184]);
            }
        }
        let mut picture = Vec::new();
        if clock && index == 0 {
            picture.extend([
                0, 0, 1, 0xb3, 0x14, 0, 0xf0, 0x13, 0xff, 0xff, 0xe0, 0x18, 0, 0, 1, 0xb5, 0x14,
                0x8a, 0, 1, 0, 0,
            ]);
        }
        picture.extend([
            0, 0, 1, 0, 0, 0x0f, 0xff, 0xf8, 0, 0, 1, 0xb5, 0x8f, 0xff, 0xf3, 0x80, 0, 0, 1, 1,
            0x55, 0x55, 0x55, 0x55,
        ]);
        if timed {
            mux.pes(0x101, 0xe0, 90_000 + index * 3600, &picture, true);
        } else {
            let mut pes = vec![0, 0, 1, 0xe0];
            pes.extend_from_slice(&((picture.len() + 3) as u16).to_be_bytes());
            pes.extend_from_slice(&[0x80, 0, 0]);
            pes.extend_from_slice(&picture);
            mux.unit(0x101, &pes);
        }
    }
    for _ in 0..40 {
        mux.packet(0x1fff, false, &[0xff; 184]);
    }
    mux.out
}

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_single_tail_timestamp_survives_without_an_inferred_unit_end() {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("single-tail-pts-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let directory = FixtureDir(path);
    for (clock, timed) in [(true, true), (false, true), (true, false)] {
        let bytes = capture(clock, timed);
        for retries in [0, 1] {
            let limits = Limits {
                max_tail_bytes: 64 * 188,
                tail_retries: retries,
                ..Limits::default()
            };
            let ts = coreav_mpegts::probe(
                SliceSource::new(&bytes),
                &limits,
                &Allowlist::new(&[Codec::Mpeg1Video, Codec::Mpeg2Video]),
            )
            .unwrap();
            assert_eq!(ts.streams.len(), 1);
            assert_eq!(ts.streams[0].last_pts, timed.then_some(129_600));
            assert_eq!(ts.streams[0].end, None);
        }
        let path = directory.0.join(format!("{clock}-{timed}.ts"));
        std::fs::write(&path, bytes).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args([
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "stream=start_pts,duration_ts",
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
        let stream = &value["streams"][0];
        if clock && timed {
            assert_eq!(stream["start_pts"], 90_000);
            assert_eq!(stream["duration_ts"], 43_200);
        } else {
            assert!(stream.get("duration_ts").is_none(), "{stream}");
        }
    }
}
