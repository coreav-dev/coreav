//! ISO H.264 pixel-format range is independent of reported container colour.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use coreav_isobmff::{Allowlist, Limits, SliceSource, probe};
use std::path::{Path, PathBuf};
use std::process::Command;

const FULL_RANGE: &[u8] = &[
    0x01, 0x64, 0x00, 0x0a, 0xff, 0xe1, 0x00, 0x18, 0x67, 0x64, 0x00, 0x0a, 0xac, 0xd9, 0x49, 0x6c,
    0x05, 0xb2, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x04, 0x1e, 0x24, 0x4b, 0x2c,
    0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0, 0xfd, 0xf8, 0xf8, 0x00,
];
const NO_RANGE: &[u8] = &[
    0x01, 0x64, 0x00, 0x0a, 0xff, 0xe1, 0x00, 0x18, 0x67, 0x64, 0x00, 0x0a, 0xac, 0xd9, 0x49, 0x6c,
    0x04, 0x40, 0x00, 0x00, 0x03, 0x00, 0x40, 0x00, 0x00, 0x03, 0x00, 0x83, 0xc4, 0x89, 0x65, 0x80,
    0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0, 0xfd, 0xf8, 0xf8, 0x00,
];
const LIMITED_RANGE: &[u8] = &[
    0x01, 0x64, 0x00, 0x0a, 0xff, 0xe1, 0x00, 0x1b, 0x67, 0x64, 0x00, 0x0a, 0xac, 0xd9, 0x49, 0x6c,
    0x05, 0xa8, 0x08, 0x08, 0x0a, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x04, 0x1e,
    0x24, 0x4b, 0x2c, 0x01, 0x00, 0x06, 0x68, 0xeb, 0xe3, 0xcb, 0x22, 0xc0, 0xfd, 0xf8, 0xf8, 0x00,
];
const FRAME: &[u8] = &[
    0x00, 0x00, 0x00, 0x15, 0x65, 0x88, 0x84, 0x00, 0x15, 0xff, 0xfe, 0xf7, 0xc9, 0xef, 0xc0, 0xa6,
    0xeb, 0xdb, 0xde, 0xb5, 0xbf, 0x60, 0x01, 0x00, 0xf9,
];

fn movie(config: &[u8], boxed: Option<Option<bool>>, empty: bool) -> Vec<u8> {
    let mut bytes = common::movie(&common::Spec {
        sizes: if empty {
            vec![]
        } else {
            vec![FRAME.len() as u32]
        },
        composition: vec![],
        sync: vec![],
        ctts: Some(vec![]),
        elst: None,
        ..common::Spec::default()
    });
    if !empty {
        let mdat = bytes.windows(4).position(|w| w == b"mdat").unwrap() + 4;
        bytes[mdat..mdat + FRAME.len()].copy_from_slice(FRAME);
    }
    let mut extra = common::bx(b"avcC", config);
    if let Some(range) = boxed {
        let mut color = if range.is_some() { b"nclx" } else { b"nclc" }.to_vec();
        color.extend_from_slice(&[0, 1, 0, 1, 0, 1]);
        if let Some(full) = range {
            color.push(if full { 0x80 } else { 0 });
        }
        extra.extend_from_slice(&common::bx(b"colr", &color));
    }
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

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn h264_pixel_range_uses_the_sps_without_changing_reported_range() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("h264-pixel-range-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let dir = FixtureDir(dir);
    for (config_name, config, sps_range, pixel_format) in [
        ("full", FULL_RANGE, Some(true), "yuvj420p"),
        ("limited", LIMITED_RANGE, Some(false), "yuv420p"),
        ("no-range", NO_RANGE, None, "yuv420p"),
    ] {
        for (box_name, boxed) in [
            ("limited", Some(Some(false))),
            ("full", Some(Some(true))),
            ("absent", None),
            ("nclc", Some(None)),
        ] {
            for empty in [false, true] {
                let name = format!("{config_name}-{box_name}-empty-{empty}");
                let bytes = movie(config, boxed, empty);
                let native = probe(
                    SliceSource::new(&bytes),
                    &Limits::default(),
                    &Allowlist::default(),
                )
                .unwrap();
                let description = &native.tracks[0].description;
                assert_eq!(description.color.full_range, boxed.flatten(), "{name}");
                assert_eq!(
                    description.parameter_set_color.unwrap().full_range,
                    sps_range,
                    "{name}"
                );
                let path = dir.0.join(format!("{name}.mp4"));
                std::fs::write(&path, bytes).unwrap();
                let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
                    .args([
                        "ffprobe",
                        "-v",
                        "error",
                        "-show_entries",
                        "stream=codec_name,pix_fmt,color_range",
                        "-of",
                        "json",
                    ])
                    .arg(path)
                    .env("COREAV_STRICT_SAFE", "1")
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{name}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
                let stream = &value["streams"][0];
                assert_eq!(stream["codec_name"], "h264", "{name}");
                let range = boxed.flatten().or(if empty { None } else { sps_range });
                if let Some(full) = range {
                    assert_eq!(
                        stream["color_range"],
                        if full { "pc" } else { "tv" },
                        "{name}"
                    );
                } else {
                    assert!(stream.get("color_range").is_none(), "{name}: {stream}");
                }
                if empty {
                    assert!(stream.get("pix_fmt").is_none(), "{name}: {stream}");
                } else {
                    let expected = if sps_range.is_none() && boxed.flatten() == Some(true) {
                        "yuvj420p"
                    } else {
                        pixel_format
                    };
                    assert_eq!(stream["pix_fmt"], expected, "{name}");
                }
            }
        }
    }
}
