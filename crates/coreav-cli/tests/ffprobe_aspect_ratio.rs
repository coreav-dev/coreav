//! ISO AV1 sample and display aspect ratios through the command.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

const CONFIG: &[u8] = &[
    0x81, 0x00, 0x0c, 0x00, 0x0a, 0x0a, 0x00, 0x00, 0x00, 0x02, 0x27, 0xee, 0x6d, 0x7c, 0x80, 0x20,
];
const FRAME: &[u8] = &[
    0x0a, 0x0a, 0x00, 0x00, 0x00, 0x02, 0x27, 0xee, 0x6d, 0x7c, 0x80, 0x20, 0x32, 0x0e, 0x10, 0x00,
    0xd0, 0x00, 0x00, 0x02, 0x80, 0x00, 0x00, 0x11, 0x73, 0xd7, 0x83, 0x70,
];

fn movie(config: &[u8], pasp: Option<(u32, u32)>, display: (u32, u32)) -> Vec<u8> {
    let mut bytes = common::movie(&common::Spec {
        sizes: vec![FRAME.len() as u32],
        composition: vec![0],
        ctts: Some(Vec::new()),
        elst: None,
        sample_entry: *b"av01",
        ..common::Spec::default()
    });
    bytes[8..12].copy_from_slice(b"iso6");
    bytes[16..20].copy_from_slice(b"av01");
    let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap() + 4;
    bytes[mdat..mdat + FRAME.len()].copy_from_slice(FRAME);
    let mut extra = common::bx(b"av1C", config);
    if let Some((h, v)) = pasp {
        extra.extend_from_slice(&common::bx(b"pasp", &common::u32s(&[h, v])));
    }
    let mut entry = common::visual_entry(b"av01", &extra);
    entry[32..36].copy_from_slice(&[0, 32, 0, 24]);
    let at = bytes.windows(4).position(|w| w == b"stsd").unwrap() + 12;
    let old = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    for kind in [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"stsd"] {
        let parent = bytes.windows(4).position(|w| w == kind).unwrap() - 4;
        let size = u32::from_be_bytes(bytes[parent..parent + 4].try_into().unwrap()) as usize;
        let size = (size + entry.len() - old) as u32;
        bytes[parent..parent + 4].copy_from_slice(&size.to_be_bytes());
    }
    bytes.splice(at..at + old, entry);
    let tkhd = bytes.windows(4).position(|w| w == b"tkhd").unwrap();
    bytes[tkhd + 80..tkhd + 88].copy_from_slice(&common::u32s(&[display.0 << 16, display.1 << 16]));
    bytes
}

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stream(path: &Path, bytes: &[u8]) -> serde_json::Value {
    std::fs::write(path, bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "stream=width,height,sample_aspect_ratio,display_aspect_ratio",
            "-of",
            "json",
        ])
        .arg(path)
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
fn av1_aspect_ratios_keep_container_precedence_and_unknown_configurations() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("av1-aspect-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let dir = FixtureDir(dir);
    for (name, config, pasp, display, rotated, expected) in [
        (
            "square",
            CONFIG,
            None,
            (32, 24),
            false,
            Some(("1:1", "4:3")),
        ),
        (
            "header",
            CONFIG,
            None,
            (64, 24),
            false,
            Some(("2:1", "8:3")),
        ),
        (
            "explicit",
            CONFIG,
            Some((3, 2)),
            (64, 24),
            false,
            Some(("3:2", "2:1")),
        ),
        (
            "rotated",
            CONFIG,
            None,
            (32, 24),
            true,
            Some(("1:1", "4:3")),
        ),
        ("unconfigured", &CONFIG[..4], None, (32, 24), false, None),
    ] {
        let mut bytes = movie(config, pasp, display);
        if rotated {
            let tkhd = bytes.windows(4).position(|w| w == b"tkhd").unwrap();
            bytes[tkhd + 44..tkhd + 80].copy_from_slice(&common::u32s(&[
                0,
                (-65536i32) as u32,
                0,
                65536,
                0,
                0,
                0,
                0,
                1 << 30,
            ]));
        }
        let result = stream(&dir.0.join(format!("{name}.mp4")), &bytes);
        assert_eq!(result["width"], 32, "{name}");
        assert_eq!(result["height"], 24, "{name}");
        if let Some((sar, dar)) = expected {
            assert_eq!(result["sample_aspect_ratio"], sar, "{name}");
            assert_eq!(result["display_aspect_ratio"], dar, "{name}");
        } else {
            assert!(result.get("sample_aspect_ratio").is_none(), "{name}");
            assert!(result.get("display_aspect_ratio").is_none(), "{name}");
        }
    }
    let control = stream(
        &dir.0.join("avc.mp4"),
        &common::movie(&common::Spec::default()),
    );
    assert!(control.get("sample_aspect_ratio").is_none());
    assert!(control.get("display_aspect_ratio").is_none());
}
