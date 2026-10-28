//! `Picture::resize_with(.., ChromaSiting::FfmpegCompatible)` against
//! FFmpeg's default `scale` filter, used as a black box
//! (`docs/design/thumbnails.md`, "Y'CbCr to Y'CbCr"): test pictures
//! (noise, a zone plate, ramps, impulses) and the decoded frames of
//! `size_vp9_resize.mp4` are scaled by both, raw in and raw out at the same
//! pixel format, and every plane must agree within FFmpeg's fixed-point
//! rounding: a PSNR of at least 50 dB and no sample more than 2 levels
//! apart (2 · 2^(depth − 12) at more than 12 bits: 32 of 65535 at 16).
//!
//! `ffmpeg` is found through `COREAV_FFMPEG`, the pinned
//! `/opt/coreav-oracle-bin/ffmpeg` or `PATH`; without it the tests print
//! why and pass, unless `COREAV_REQUIRE_FFMPEG=1`. The fixture comes from
//! `COREAV_FIXTURES` or `/opt/coreav-fixtures` and is skipped when absent,
//! except under `COREAV_REQUIRE_FFMPEG=1`. The fixtures workflow runs both
//! tests with it.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use coreav_frame::{Chroma, ChromaSiting, Filter, Picture, PixelFormat, Plane};

/// Bars, per plane: PSNR at the format's peak, and the largest difference.
const MIN_PSNR: f64 = 50.0;
/// The largest difference at `depth` bits.
fn max_diff(depth: u8) -> u32 {
    2 << depth.saturating_sub(12)
}

fn ffmpeg() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("COREAV_FFMPEG") {
        return Some(PathBuf::from(path));
    }
    let pinned = Path::new("/opt/coreav-oracle-bin/ffmpeg");
    if pinned.is_file() {
        return Some(pinned.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("ffmpeg"))
        .find(|p| p.is_file())
}

fn ffmpeg_or_skip(what: &str) -> Option<PathBuf> {
    let found = ffmpeg();
    if found.is_none() {
        assert!(
            std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none_or(|v| v != "1"),
            "COREAV_REQUIRE_FFMPEG=1 and no ffmpeg for {what}"
        );
        eprintln!("{what}: no ffmpeg, skipped");
    }
    found
}

fn format(name: &str) -> PixelFormat {
    let (chroma, depth) = match name {
        "yuv420p" => (Chroma::Cs420, 8),
        "yuv422p" => (Chroma::Cs422, 8),
        "yuv444p" => (Chroma::Cs444, 8),
        "yuv420p10le" => (Chroma::Cs420, 10),
        "yuv420p12le" => (Chroma::Cs420, 12),
        "yuv420p16le" => (Chroma::Cs420, 16),
        _ => panic!("{name}"),
    };
    PixelFormat::Yuv { chroma, depth }
}

fn frame_bytes(format: PixelFormat, w: u32, h: u32) -> usize {
    (0..format.plane_count())
        .map(|p| {
            format.row_bytes(p, w, h).unwrap() * format.plane_size(p, w, h).unwrap().1 as usize
        })
        .sum()
}

fn from_raw(format: PixelFormat, w: u32, h: u32, data: &[u8]) -> Picture {
    let mut planes = Vec::new();
    let mut off = 0;
    for p in 0..format.plane_count() {
        let row = format.row_bytes(p, w, h).unwrap();
        let len = row * format.plane_size(p, w, h).unwrap().1 as usize;
        planes.push(Plane::new(data[off..off + len].to_vec(), row));
        off += len;
    }
    Picture::new(w, h, format, planes).unwrap()
}

fn to_raw(picture: &Picture) -> Vec<u8> {
    let mut out = Vec::new();
    picture.write_packed(&mut out).unwrap();
    out
}

/// One plane's samples.
fn samples(picture: &Picture, plane: usize) -> Vec<u32> {
    let two = picture.format().sample_bytes() == 2;
    picture
        .rows(plane)
        .flat_map(|r| {
            if two {
                r.chunks_exact(2)
                    .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
                    .collect::<Vec<_>>()
            } else {
                r.iter().map(|b| u32::from(*b)).collect()
            }
        })
        .collect()
}

/// `ffmpeg -vf scale=W:H` on raw frames of `name`, the same format out.
fn ffmpeg_scale(
    ffmpeg: &Path,
    name: &str,
    w: u32,
    h: u32,
    raw: Vec<u8>,
    ow: u32,
    oh: u32,
) -> Vec<u8> {
    let mut child = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-f", "rawvideo"])
        .args(["-pix_fmt", name, "-s", &format!("{w}x{h}"), "-i", "-"])
        .args(["-vf", &format!("scale={ow}:{oh}")])
        .args(["-f", "rawvideo", "-pix_fmt", name, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&raw).unwrap());
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(out.status.success(), "ffmpeg scale failed");
    out.stdout
}

/// Per-plane PSNR and largest difference of two pictures.
fn compare(a: &Picture, b: &Picture) -> Vec<(f64, u32)> {
    let peak = f64::from((1u32 << a.format().depth()) - 1);
    (0..a.format().plane_count())
        .map(|p| {
            let (x, y) = (samples(a, p), samples(b, p));
            assert_eq!(x.len(), y.len());
            let mut se = 0f64;
            let mut max = 0;
            for (u, v) in x.iter().zip(&y) {
                let d = u.abs_diff(*v);
                se += f64::from(d * d);
                max = max.max(d);
            }
            let mse = se / x.len() as f64;
            let psnr = if mse == 0.0 {
                f64::INFINITY
            } else {
                10.0 * (peak * peak / mse).log10()
            };
            (psnr, max)
        })
        .collect()
}

fn check(what: &str, ours: &Picture, theirs: &Picture) {
    let planes = compare(ours, theirs);
    eprintln!("{what}: {planes:.2?}");
    for (p, (psnr, max)) in planes.iter().enumerate() {
        assert!(
            *psnr >= MIN_PSNR && *max <= max_diff(ours.format().depth()),
            "{what}: plane {p} {psnr:.2} dB, largest difference {max}"
        );
    }
}

/// A test picture: `kind` 0 noise, 1 zone plate, 2 ramps, 3 impulses on a
/// grey ground (sharp enough to overshoot), each plane its own pattern.
fn test_picture(format: PixelFormat, w: u32, h: u32, kind: u32) -> Picture {
    let mut p = Picture::zeroed(w, h, format).unwrap();
    let max = (1u32 << format.depth()) - 1;
    let two = format.sample_bytes() == 2;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for plane in 0..format.plane_count() {
        let (pw, ph) = format.plane_size(plane, w, h).unwrap();
        for y in 0..ph {
            let row = p.row_mut(plane, y as usize).unwrap();
            for x in 0..pw {
                let (fx, fy) = (f64::from(x), f64::from(y));
                let unit = match kind {
                    0 => {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        (state >> 40) as f64 / f64::from(1u32 << 24)
                    }
                    1 => {
                        let r = match plane {
                            0 => fx * fx + fy * fy,
                            1 => 2.0 * fx * fx,
                            _ => 2.0 * fy * fy,
                        };
                        0.5 + 0.4 * (r / 400.0).sin()
                    }
                    2 => match plane {
                        0 => fx / f64::from(pw),
                        1 => fy / f64::from(ph),
                        _ => (fx + fy) / f64::from(pw + ph),
                    },
                    _ => {
                        let plane = plane as u32;
                        if (x + 3 * plane) % 7 == 0 && (y + plane) % 5 == 0 {
                            1.0
                        } else if (x + y) % 11 == 0 {
                            0.0
                        } else {
                            0.5
                        }
                    }
                };
                let v = (unit * f64::from(max)).round().clamp(0.0, f64::from(max)) as u32;
                if two {
                    let i = x as usize * 2;
                    row[i..i + 2].copy_from_slice(&(v as u16).to_le_bytes());
                } else {
                    row[x as usize] = v as u8;
                }
            }
        }
    }
    p
}

#[test]
fn test_pictures_match_ffmpeg_scale() {
    let Some(ffmpeg) = ffmpeg_or_skip("test pictures") else {
        return;
    };
    let eight = [
        (352, 288, 176, 144),
        (176, 144, 352, 288),
        (1920, 1080, 640, 360),
        (640, 360, 1280, 720),
        (1280, 720, 1920, 1080),
        (1280, 720, 320, 240),
        (352, 288, 117, 95),
        (176, 144, 501, 333),
        (353, 289, 176, 144),
    ];
    let others = [
        (352, 288, 176, 144),
        (176, 144, 352, 288),
        (640, 360, 1280, 720),
        (352, 288, 117, 95),
    ];
    for name in [
        "yuv420p",
        "yuv422p",
        "yuv444p",
        "yuv420p10le",
        "yuv420p12le",
        "yuv420p16le",
    ] {
        let sizes: &[(u32, u32, u32, u32)] = if name == "yuv420p" { &eight } else { &others };
        let format = format(name);
        for &(w, h, ow, oh) in sizes {
            // Noise and the zone plate only on the large sizes, for time.
            let kinds = if (w * h).max(ow * oh) > 400_000 { 2 } else { 4 };
            for kind in 0..kinds {
                let src = test_picture(format, w, h, kind);
                let theirs = ffmpeg_scale(&ffmpeg, name, w, h, to_raw(&src), ow, oh);
                let theirs = from_raw(format, ow, oh, &theirs);
                let ours = src
                    .resize_with(ow, oh, Filter::Bicubic, ChromaSiting::FfmpegCompatible)
                    .unwrap();
                check(
                    &format!("{name} {w}x{h}->{ow}x{oh} picture {kind}"),
                    &ours,
                    &theirs,
                );
            }
        }
    }
}

#[test]
fn resolution_change_fixture_matches_ffmpeg_scale() {
    let Some(ffmpeg) = ffmpeg_or_skip("size_vp9_resize.mp4") else {
        return;
    };
    let dir = std::env::var_os("COREAV_FIXTURES")
        .map_or_else(|| PathBuf::from("/opt/coreav-fixtures"), PathBuf::from);
    let file = dir.join("size_vp9_resize.mp4");
    if !file.is_file() {
        // Where FFmpeg is required (the fixtures workflow), so is the fixture.
        assert!(
            std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none_or(|v| v != "1"),
            "COREAV_REQUIRE_FFMPEG=1 and {} is absent",
            file.display()
        );
        eprintln!("{}: absent, skipped", file.display());
        return;
    }
    // Decoded without FFmpeg's autoscale: 320x240 frames, then 1280x720
    // ones, which the transcode resizes to the first size.
    let out = Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&file)
        .args(["-noautoscale", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let format = PixelFormat::YUV420P;
    let (small, large) = (
        frame_bytes(format, 320, 240),
        frame_bytes(format, 1280, 720),
    );
    let first_large = out.stdout.len() - 25 * large;
    assert_eq!(first_large, 25 * small, "frame sizes of the fixture");
    let frames = &out.stdout[first_large..];
    let theirs = ffmpeg_scale(&ffmpeg, "yuv420p", 1280, 720, frames.to_vec(), 320, 240);
    for (i, (src, theirs)) in frames
        .chunks_exact(large)
        .zip(theirs.chunks_exact(small))
        .enumerate()
    {
        let ours = from_raw(format, 1280, 720, src)
            .resize_with(320, 240, Filter::Bicubic, ChromaSiting::FfmpegCompatible)
            .unwrap();
        check(
            &format!("size_vp9_resize.mp4 frame {}", 25 + i),
            &ours,
            &from_raw(format, 320, 240, theirs),
        );
    }
}
