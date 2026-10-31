//! `coreav thumbnail` end to end (`docs/design/thumbnails.md`): usage and
//! error exits always; with the `ffmpeg-fallback` feature and an `ffmpeg`
//! on `PATH` (ignored tests, run in the fixtures workflow), thumbnails of
//! generated files in every format and mode, each summary validated against
//! `docs/schema/thumbnail-v1.schema.json` and each image compared with
//! FFmpeg's of the same frame and size.
#![cfg(feature = "thumbnail")]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[cfg(feature = "ffmpeg-fallback")]
#[allow(dead_code)]
mod schema;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(feature = "ffmpeg-fallback")]
const THUMBNAIL_SCHEMA: &str = include_str!("../../../docs/schema/thumbnail-v1.schema.json");

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

fn tmp(name: &str) -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn thumbnail(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .arg("thumbnail")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn usage_and_rejected_files() {
    let junk = tmp("thumbnail-junk.bin");
    std::fs::write(&junk, b"RIFF\x24\x00\x00\x00AVI LIST").unwrap();
    let out = tmp("thumbnail-junk.jpg");
    let (code, stdout, stderr) = thumbnail(&[
        junk.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--force",
    ]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.is_empty());
    let error: Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["kind"], "unsupported_container");
    assert!(!out.exists());
    for bad in ["x", "-1", "1:61", "12.", "1ms5"] {
        let (code, _, _) = thumbnail(&[junk.to_str().unwrap(), "--at", bad, "-o", "o.jpg"]);
        assert_eq!(code, 2, "--at {bad}");
    }
    let (code, _, _) = thumbnail(&[junk.to_str().unwrap(), "--width", "0", "-o", "o.jpg"]);
    assert_eq!(code, 2);
}

#[cfg(feature = "ffmpeg-fallback")]
mod with_ffmpeg {
    use super::*;

    fn ffmpeg(args: &[&str]) {
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(args)
            .status()
            .expect("ffmpeg on PATH");
        assert!(status.success(), "ffmpeg {args:?}");
    }

    fn rgb(path: &Path) -> Vec<u8> {
        let out = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(path)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        out.stdout
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        assert_eq!(a.len(), b.len());
        let sum: f64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2))
            .sum();
        if sum == 0.0 {
            return 99.0;
        }
        10.0 * (255.0f64 * 255.0 * a.len() as f64 / sum).log10()
    }

    /// Generated sources: (name, ffmpeg arguments after the lavfi input).
    fn sources() -> Vec<(&'static str, Vec<&'static str>)> {
        vec![
            (
                "h264.mp4",
                vec![
                    "-c:v", "libx264", "-bf", "3", "-g", "25", "-pix_fmt", "yuv420p",
                ],
            ),
            (
                "h264_rot.mp4",
                vec![
                    "-c:v",
                    "libx264",
                    "-g",
                    "25",
                    "-pix_fmt",
                    "yuv420p",
                    "-metadata:s:v",
                    "rotate=90",
                ],
            ),
            (
                "hevc10.mkv",
                vec![
                    "-c:v",
                    "libx265",
                    "-x265-params",
                    "log-level=error",
                    "-pix_fmt",
                    "yuv420p10le",
                    "-g",
                    "25",
                ],
            ),
            (
                "vp9.webm",
                vec![
                    "-c:v",
                    "libvpx-vp9",
                    "-g",
                    "25",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                ],
            ),
            (
                "vp8.webm",
                vec![
                    "-c:v",
                    "libvpx",
                    "-g",
                    "25",
                    "-deadline",
                    "realtime",
                    "-cpu-used",
                    "8",
                ],
            ),
            ("prores.mov", vec!["-c:v", "prores_ks", "-profile:v", "2"]),
        ]
    }

    #[test]
    #[ignore = "needs ffmpeg (libx264, libx265, libvpx, prores_ks) on PATH and the ffmpeg-fallback feature"]
    fn thumbnails_of_generated_files_agree_with_ffmpeg() {
        let schema: Value = serde_json::from_str(THUMBNAIL_SCHEMA).unwrap();
        for (name, args) in sources() {
            let src = tmp(&format!("thumb-src-{name}"));
            let mut full = vec![
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=25",
                "-t",
                "3",
            ];
            full.extend(args.iter().copied());
            full.push(src.to_str().unwrap());
            ffmpeg(&full);
            for exact in [false, true] {
                for format in ["png", "jpg", "webp"] {
                    let out = tmp(&format!("thumb-{name}-{exact}.{format}"));
                    let mut a = vec![
                        src.to_str().unwrap(),
                        "--at",
                        "1.3",
                        "--width",
                        "160",
                        "-o",
                        out.to_str().unwrap(),
                        "--force",
                    ];
                    if exact {
                        a.push("--exact");
                    }
                    let (code, stdout, stderr) = thumbnail(&a);
                    assert_eq!(code, 0, "{name} {format} exact={exact}: {stderr}");
                    let summary: Value = serde_json::from_str(&stdout).unwrap();
                    schema::validate(&summary, &schema, &schema, "$").unwrap();
                    let libvpx = matches!(name, "vp9.webm" | "vp8.webm");
                    let backend = if libvpx && cfg!(feature = "libvpx") {
                        "libvpx"
                    } else {
                        "ffmpeg"
                    };
                    assert_eq!(summary["decoder"]["backend"], backend);
                    assert_eq!(
                        summary["keyframe"],
                        !exact || summary["packets_decoded"] == 1
                    );
                    if format != "png" {
                        continue;
                    }
                    let frame = &summary["time"]["frame"];
                    let micros =
                        frame["num"].as_i64().unwrap() * 1_000_000 / frame["den"].as_i64().unwrap();
                    let at = format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000);
                    let (w, h) = (
                        summary["output"]["width"].as_u64().unwrap(),
                        summary["output"]["height"].as_u64().unwrap(),
                    );
                    let reference = tmp(&format!("thumb-{name}-{exact}-ref.png"));
                    let vf = format!("scale={w}:{h}");
                    ffmpeg(&[
                        "-ss",
                        &at,
                        "-i",
                        src.to_str().unwrap(),
                        "-frames:v",
                        "1",
                        "-vf",
                        &vf,
                        "-pix_fmt",
                        "rgb24",
                        reference.to_str().unwrap(),
                    ]);
                    let p = psnr(&rgb(&out), &rgb(&reference));
                    // The image bar's per-file floor (docs/design/thumbnails.md, I1).
                    assert!(p >= 35.0, "{name} exact={exact}: PSNR {p:.2}");
                }
            }
        }
    }

    #[test]
    #[ignore = "needs ffmpeg (libx264) on PATH and the ffmpeg-fallback feature"]
    fn image_to_standard_output() {
        let src = tmp("thumb-stdout.mp4");
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10",
            "-t",
            "1",
            "-c:v",
            "libx264",
            src.to_str().unwrap(),
        ]);
        let out = Command::new(bin())
            .args([
                "thumbnail",
                src.to_str().unwrap(),
                "-o",
                "-",
                "--format",
                "png",
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert!(out.stdout.starts_with(b"\x89PNG"));
        let summary: Value =
            serde_json::from_str(std::str::from_utf8(&out.stderr).unwrap()).unwrap();
        assert_eq!(
            summary["output"]["bytes"].as_u64().unwrap() as usize,
            out.stdout.len()
        );
    }
}
