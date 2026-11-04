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

fn grow_parents(bytes: &mut [u8], amount: usize) {
    for kind in [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"] {
        let at = bytes.windows(4).position(|w| w == kind).unwrap() - 4;
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        bytes[at..at + 4].copy_from_slice(&(size + amount as u32).to_be_bytes());
    }
}

fn multiple_descriptions(clap: bool, selected: u32) -> Vec<u8> {
    let mut bytes = movie(true);
    let stsd = bytes.windows(4).position(|w| w == b"stsd").unwrap();
    let at = stsd + 12;
    let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let mut second = bytes[at..at + size].to_vec();
    let first_size = size - 40;
    bytes[at..at + 4].copy_from_slice(&(first_size as u32).to_be_bytes());
    bytes.drain(at + first_size..at + size);
    if !clap {
        second.truncate(first_size);
        second[..4].copy_from_slice(&(first_size as u32).to_be_bytes());
    }
    let added = second.len() - 40;
    bytes.splice(at + first_size..at + first_size, second);
    bytes[stsd + 8..stsd + 12].copy_from_slice(&2u32.to_be_bytes());
    grow_parents(&mut bytes, added);
    let stsc = bytes.windows(4).position(|w| w == b"stsc").unwrap();
    bytes[stsc + 20..stsc + 24].copy_from_slice(&selected.to_be_bytes());
    bytes
}

fn assert_refused(name: &str, bytes: &[u8]) {
    let dir = FixtureDir::new(name);
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, bytes).unwrap();
    for format in [&[][..], &["-of", "json"][..]] {
        for selection in [
            &["-show_streams"][..],
            &["-show_entries", "stream=index"][..],
            &["-show_packets"][..],
            &["-count_packets", "-show_streams"][..],
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
fn later_description_used_refuses() {
    assert_refused("later-used", &multiple_descriptions(true, 2));
}

#[test]
fn later_description_unused_refuses() {
    assert_refused("later-unused", &multiple_descriptions(true, 1));
}

#[test]
fn zero_sized_final_clap_refuses() {
    let mut bytes = movie(true);
    let clap = bytes.windows(4).position(|w| w == b"clap").unwrap();
    bytes[clap - 4..clap].copy_from_slice(&0u32.to_be_bytes());
    assert_refused("zero-size", &bytes);
}

#[test]
fn extended_sized_final_clap_refuses() {
    let mut bytes = movie(true);
    let clap = bytes.windows(4).position(|w| w == b"clap").unwrap();
    bytes[clap - 4..clap].copy_from_slice(&1u32.to_be_bytes());
    bytes.splice(clap + 4..clap + 4, 48u64.to_be_bytes());
    let entry = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let size = u32::from_be_bytes(bytes[entry..entry + 4].try_into().unwrap());
    bytes[entry..entry + 4].copy_from_slice(&(size + 8).to_be_bytes());
    grow_parents(&mut bytes, 8);
    assert_refused("extended-size", &bytes);
}

#[test]
fn incomplete_visual_children_refuse_without_a_retained_aperture() {
    let mut bytes = movie(false);
    let entry = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let size = u32::from_be_bytes(bytes[entry..entry + 4].try_into().unwrap()) as usize;
    bytes[entry..entry + 4].copy_from_slice(&((size + 4) as u32).to_be_bytes());
    bytes.splice(entry + size..entry + size, *b"clap");
    grow_parents(&mut bytes, 4);
    let parsed = probe(
        SliceSource::new(&bytes),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap();
    assert!(!parsed.tracks[0].description.visual_children_complete);
    assert!(parsed.tracks[0].description.entry_boxes.is_empty());
    assert_refused("incomplete-layout", &bytes);
}

#[test]
fn uninspected_visual_containers_refuse() {
    for kind in [b"sinf", b"schi", b"udta", b"ilst"] {
        let mut bytes = movie(true);
        let clap = bytes.windows(4).position(|w| w == b"clap").unwrap();
        let nested = common::bx(kind, &bytes[clap - 4..clap + 36]);
        bytes.splice(clap - 4..clap + 36, nested);
        let entry = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
        let size = u32::from_be_bytes(bytes[entry..entry + 4].try_into().unwrap());
        bytes[entry..entry + 4].copy_from_slice(&(size + 8).to_be_bytes());
        grow_parents(&mut bytes, 8);
        assert_refused(std::str::from_utf8(kind).unwrap(), &bytes);
    }
}

#[test]
fn unknown_visual_children_refuse_even_without_aperture() {
    let mut bytes = movie(true);
    let clap = bytes.windows(4).position(|w| w == b"clap").unwrap();
    bytes[clap..clap + 4].copy_from_slice(b"zzzz");
    assert_refused("unknown-child", &bytes);
}

#[test]
fn field_information_leaf_remains_native() {
    let mut bytes = movie(false);
    let child = common::bx(b"fiel", &[1, 0]);
    let entry = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let size = u32::from_be_bytes(bytes[entry..entry + 4].try_into().unwrap()) as usize;
    bytes[entry..entry + 4].copy_from_slice(&((size + child.len()) as u32).to_be_bytes());
    bytes.splice(entry + size..entry + size, child.iter().copied());
    grow_parents(&mut bytes, child.len());
    let dir = FixtureDir::new("field-information");
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, bytes).unwrap();
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

#[test]
fn h263_configuration_leaf_remains_native() {
    let mut bytes = movie(false);
    let child = common::bx(b"d263", &[0, 0, 0, 0, 0, 10, 0]);
    let entry = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let size = u32::from_be_bytes(bytes[entry..entry + 4].try_into().unwrap()) as usize;
    bytes[entry + 4..entry + 8].copy_from_slice(b"s263");
    bytes[entry..entry + 4].copy_from_slice(&((size + child.len()) as u32).to_be_bytes());
    bytes.splice(entry + size..entry + size, child.iter().copied());
    grow_parents(&mut bytes, child.len());
    let dir = FixtureDir::new("h263-configuration");
    let path = dir.0.join("input.3gp");
    std::fs::write(&path, bytes).unwrap();
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

#[test]
fn h263_configuration_extensions_refuse() {
    let mut bytes = movie(true);
    let clap = bytes.windows(4).position(|w| w == b"clap").unwrap();
    bytes[clap..clap + 4].copy_from_slice(b"d263");
    assert_refused("h263-configuration-extension", &bytes);
}

#[test]
fn opaque_global_header_is_not_an_aperture_container() {
    let mut bytes = movie(true);
    let clap = bytes.windows(4).position(|w| w == b"clap").unwrap();
    let header = common::bx(b"glbl", &bytes[clap - 4..clap + 36]);
    bytes.splice(clap - 4..clap + 36, header);
    let entry = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let size = u32::from_be_bytes(bytes[entry..entry + 4].try_into().unwrap());
    bytes[entry..entry + 4].copy_from_slice(&(size + 8).to_be_bytes());
    grow_parents(&mut bytes, 8);
    let dir = FixtureDir::new("global-header");
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, bytes).unwrap();
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

#[test]
fn multiple_descriptions_without_clap_remain_native() {
    let dir = FixtureDir::new("multiple-control");
    let path = dir.0.join("input.mp4");
    std::fs::write(&path, multiple_descriptions(false, 2)).unwrap();
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
