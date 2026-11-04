//! Clean-aperture refusal at the ffprobe compatibility boundary.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use coreav_isobmff::{Allowlist, FourCc, Limits, SliceSource, probe};
use std::path::PathBuf;
use std::process::Command;

fn movie(clap: bool) -> Vec<u8> {
    let mut bytes = common::movie(&common::Spec::default());
    if !clap {
        return bytes;
    }
    let mut extra = common::bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]);
    extra.extend_from_slice(&common::bx(
        b"clap",
        &common::u32s(&[304, 1, 224, 1, 0, 1, 0, 1]),
    ));
    let entry = common::visual_entry(b"avc1", &extra);
    let at = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let old = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    for kind in [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"] {
        let parent = bytes.windows(4).position(|w| w == kind).unwrap() - 4;
        let size = u32::from_be_bytes(bytes[parent..parent + 4].try_into().unwrap()) as usize;
        bytes[parent..parent + 4]
            .copy_from_slice(&((size + entry.len() - old) as u32).to_be_bytes());
    }
    bytes.splice(at..at + old, entry);
    bytes
}

struct FixtureDir(PathBuf);

impl FixtureDir {
    fn new(name: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("clean-aperture-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn raw_probe_retains_clean_aperture() {
    let bytes = movie(true);
    let parsed = probe(
        SliceSource::new(&bytes),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    let boxes = &parsed.tracks[0].description.entry_boxes;
    let clap = boxes.iter().find(|b| b.kind == FourCc(*b"clap")).unwrap();
    assert_eq!(clap.data, common::u32s(&[304, 1, 224, 1, 0, 1, 0, 1]));
}

#[test]
fn clean_aperture_refuses_every_selection_without_output() {
    let dir = FixtureDir::new("strict");
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, movie(true)).unwrap();
    for format in [&[][..], &["-of", "json"][..]] {
        for selection in [
            &["-show_streams"][..],
            &["-show_entries", "stream=index"][..],
            &["-show_packets"][..],
            &["-count_packets", "-show_streams"][..],
            &["-show_format", "-show_streams", "-show_packets"][..],
        ] {
            let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["ffprobe", "-v", "error"])
                .args(format)
                .args(selection)
                .arg(&path)
                .env("COREAV_STRICT_SAFE", "1")
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(4),
                "{format:?} {selection:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.stdout.is_empty(), "{format:?} {selection:?}");
        }
    }
}

#[test]
fn no_clean_aperture_remains_native() {
    let dir = FixtureDir::new("control");
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, movie(false)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "stream=index",
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
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["streams"][0]["index"], 0);
}

#[cfg(unix)]
#[test]
fn fallback_preserves_original_arguments_and_status() {
    use std::os::unix::fs::PermissionsExt;
    let dir = FixtureDir::new("fallback");
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, movie(true)).unwrap();
    let standin = dir.0.join("ffprobe");
    std::fs::write(&standin, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 17\n").unwrap();
    std::fs::set_permissions(&standin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let input = path.to_str().unwrap();
    let args = [
        "-v",
        "error",
        "-show_entries",
        "stream=index",
        "-of",
        "json",
        input,
    ];
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("ffprobe")
        .args(args)
        .env_clear()
        .env("PATH", &dir.0)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}\n", args.join("\n"))
    );
}
