//! AVI selection, packet preservation, refusal and atomic-output behavior.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn input(dir: &Path, spec: &common::Spec) -> PathBuf {
    let path = dir.join("in.mp4");
    std::fs::write(&path, common::movie(spec)).unwrap();
    path
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("ffmpeg")
        .args(args)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap()
}

#[test]
fn a_copy_keeps_the_payloads_and_explicit_stream_order() {
    let dir = scratch("avi-packet-copy");
    let path = input(
        &dir,
        &common::Spec {
            composition: vec![0; 4],
            elst: None,
            ..Default::default()
        },
    );
    let output = dir.join("out.avi");
    let result = run(&[
        "-i",
        path.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "0:v",
        "-c",
        "copy",
        output.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    let data = std::fs::read(&output).unwrap();
    assert!(data.starts_with(b"RIFF"));
    let index = data.windows(4).position(|b| b == b"idx1").unwrap();
    let movi = data.windows(4).position(|b| b == b"movi").unwrap();
    assert_eq!(
        u32::from_le_bytes(data[index + 4..index + 8].try_into().unwrap()),
        8 * 16
    );
    for (i, entry) in data[index + 8..].chunks_exact(16).enumerate() {
        assert_eq!(&entry[..4], if i % 2 == 0 { b"00dc" } else { b"01dc" });
        let offset = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize + movi;
        let size = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        assert!(
            data[offset + 8..offset + 8 + size]
                .iter()
                .all(|b| *b == 0xab)
        );
    }
}

#[test]
fn unrepresentable_packet_timing_does_not_replace_the_destination() {
    let dir = scratch("avi-timing-refusals");
    let output = dir.join("out.avi");
    for spec in [
        common::Spec::default(),
        common::Spec {
            composition: vec![0; 4],
            elst: Some(vec![(80, 0)]),
            ..Default::default()
        },
        common::Spec {
            composition: vec![0; 4],
            elst: None,
            stts: Some(vec![(2, 512), (2, 1024)]),
            ..Default::default()
        },
    ] {
        let path = input(&dir, &spec);
        std::fs::write(&output, b"preserved").unwrap();
        let result = run(&[
            "-y",
            "-i",
            path.to_str().unwrap(),
            "-c",
            "copy",
            output.to_str().unwrap(),
        ]);
        assert_eq!(
            result.status.code(),
            Some(4),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"preserved");
    }
}

#[test]
fn mp3_is_refused_in_strict_safe_mode_before_opening_the_input() {
    let dir = scratch("avi-strict-mp3");
    let output = dir.join("out.avi");
    let result = run(&[
        "-i",
        "/nonexistent/avi-source.mp4",
        "-c:a",
        "libmp3lame",
        output.to_str().unwrap(),
    ]);
    assert_eq!(result.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&result.stderr).contains("strict-safe"));
    assert!(!output.exists());
}

#[test]
fn avi_defaults_never_turn_an_omitted_video_encoder_into_copy() {
    let dir = scratch("avi-defaults");
    let path = input(
        &dir,
        &common::Spec {
            composition: vec![0; 4],
            elst: None,
            ..Default::default()
        },
    );
    let output = dir.join("out.avi");
    let result = run(&["-i", path.to_str().unwrap(), output.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&result.stderr).contains("MPEG-4 Part 2"));
    assert!(!output.exists());
}
