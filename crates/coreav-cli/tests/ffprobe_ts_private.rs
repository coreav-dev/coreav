//! Unidentified private MPEG-TS streams at the ffprobe fallback boundary.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-mpegts/tests/common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, kind: u8, descriptors: Vec<u8>) -> Self {
        Self::with_payload(name, kind, descriptors, b"private payload")
    }

    fn with_payload(name: &str, kind: u8, descriptors: Vec<u8>, payload: &[u8]) -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ts-private-{name}-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let mut mux = common::Mux::default();
        mux.pat(&[(1, 0x100)]);
        mux.pmt_streams(0x100, 1, &[], &[(kind, 0x101, descriptors)]);
        for tick in 1..=6 {
            mux.pes(0x101, 0xbd, tick * 90_000, payload, true);
        }
        std::fs::write(dir.join("input.ts"), mux.out).unwrap();
        Self(dir)
    }

    fn input(&self) -> PathBuf {
        self.0.join("input.ts")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn unidentified_private_ts_refuses_before_output() {
    let fixture = Fixture::new("refusal", 0x06, vec![]);
    for args in [
        vec!["-show_streams"],
        vec!["-show_programs"],
        vec!["-show_entries", "packet=stream_index:format=nb_streams"],
        vec!["-show_entries", "format=nb_streams"],
    ] {
        for writer in ["json", "default"] {
            let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .arg("ffprobe")
                .args(&args)
                .args(["-of", writer])
                .arg(fixture.input())
                .env("COREAV_STRICT_SAFE", "1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(4),
                "{args:?}: {:?}",
                output.stderr
            );
            assert!(output.stdout.is_empty(), "{args:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("unidentified private"),
                "{args:?}: {:?}",
                output.stderr
            );
        }
    }
}

#[test]
fn registered_unidentified_private_audio_refuses_before_output() {
    let mut frame = vec![0; 1253];
    frame[..4].copy_from_slice(&[0xff, 0xfd, 0xe0, 0x04]);
    let fixture = Fixture::with_payload(
        "registered-audio",
        0x06,
        common::Mux::registration(b"TEST"),
        &frame,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "stream=codec_name,codec_type,codec_tag",
            "-of",
            "json",
        ])
        .arg(fixture.input())
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4), "{:?}", output.stderr);
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unidentified private"));
}

#[test]
fn identified_streams_and_native_probe_remain_supported() {
    for (name, kind, descriptors) in [
        ("mpeg-audio", 0x03, vec![]),
        ("h264", 0x1b, vec![]),
        ("registered-ac3", 0x06, common::Mux::registration(b"AC-3")),
        ("descriptor-ac3", 0x06, vec![0x6a, 0]),
        (
            "subtitles",
            0x06,
            vec![0x59, 8, b'e', b'n', b'g', 0x10, 0, 1, 0, 1],
        ),
    ] {
        let fixture = Fixture::new(name, kind, descriptors);
        let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["ffprobe", "-show_streams", "-show_format", "-of", "json"])
            .arg(fixture.input())
            .env("COREAV_STRICT_SAFE", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{name}: {:?}", output.stderr);
        let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(doc["format"]["nb_streams"], 1, "{name}");
    }
    let fixture = Fixture::new("native", 0x06, vec![]);
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("probe")
        .arg(fixture.input())
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["schema"], "coreav.probe-mpegts/2");
}

#[cfg(unix)]
#[test]
fn unidentified_private_ts_preserves_fallback_arguments_and_status() {
    use std::os::unix::fs::PermissionsExt;
    let mut frame = vec![0; 1253];
    frame[..4].copy_from_slice(&[0xff, 0xfd, 0xe0, 0x04]);
    for (name, descriptors) in [
        ("fallback", vec![]),
        ("registered-fallback", common::Mux::registration(b"TEST")),
    ] {
        let fixture = Fixture::with_payload(name, 0x06, descriptors, &frame);
        let program = fixture.0.join("ffprobe");
        let log = fixture.0.join("called");
        std::fs::write(&program, b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PRIVATE_TS_LOG\"\nprintf 'fallback-only\\n'\nexit 7\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = ["ffprobe", "-show_packets", "-show_streams", "-of", "json"];
        let strict = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(args)
            .arg(fixture.input())
            .env_clear()
            .env("COREAV_STRICT_SAFE", "1")
            .env("COREAV_FALLBACK", &fixture.0)
            .env("PRIVATE_TS_LOG", &log)
            .output()
            .unwrap();
        assert_eq!(strict.status.code(), Some(4));
        assert!(strict.stdout.is_empty());
        assert!(!log.exists());
        let fallback = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(args)
            .arg(fixture.input())
            .env_clear()
            .env("COREAV_FALLBACK", &fixture.0)
            .env("PRIVATE_TS_LOG", &log)
            .output()
            .unwrap();
        assert_eq!(fallback.status.code(), Some(7));
        assert_eq!(fallback.stdout, b"fallback-only\n");
        assert_eq!(
            std::fs::read_to_string(log).unwrap(),
            format!("{}\n{}\n", args[1..].join("\n"), fixture.input().display())
        );
    }
}
