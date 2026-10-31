//! `decode-oracle stream` against FFmpeg's `framemd5` of the same stream
//! (docs/measurements/compat-0.1.0/codec-coverage-plan.md, "CoreAV's side"),
//! on streams this test encodes with `ffmpeg` (`COREAV_FFMPEG`, else the
//! one on PATH). Without an `ffmpeg` the test passes having compared
//! nothing; with `COREAV_REQUIRE_FFMPEG` set, a missing `ffmpeg` or a codec
//! of this build that no encoder of it makes fails the test. The sandboxed
//! backends (dav1d, libvpx) are compared only where the sandbox enforces its
//! memory limit (Linux).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const FRAMES: &str = "40";

struct Case {
    name: &'static str,
    codec: &'static str,
    decoder: Option<&'static str>,
    enabled: bool,
    encoders: &'static [&'static [&'static str]],
}

fn ffmpeg() -> Option<String> {
    let program = std::env::var("COREAV_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    Command::new(&program)
        .arg("-version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| program)
}

fn scratch(test: &str) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("decode-oracle-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn encode(ffmpeg: &str, file: &Path, encoder: &[&str]) -> bool {
    Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!(
            "testsrc=size=160x96:rate=25:duration=4,select=lt(n\\,{FRAMES})"
        ))
        .args(["-frames:v", FRAMES])
        .args(encoder)
        .arg(file)
        .status()
        .is_ok_and(|s| s.success())
        && file.exists()
}

fn reference(ffmpeg: &str, file: &Path, ext: &str, decoder: Option<&str>) -> Option<Vec<String>> {
    let mut command = Command::new(ffmpeg);
    command.args(["-nostdin", "-v", "error", "-noautorotate"]);
    if matches!(ext, "mp4" | "mov") {
        command.args(["-ignore_editlist", "1"]);
    }
    if let Some(decoder) = decoder {
        command.args(["-c:v", decoder]);
    }
    let out = command
        .args(["-flags", "unaligned", "-i"])
        .arg(file)
        .args([
            "-map",
            "0:0",
            "-fps_mode",
            "passthrough",
            "-noautoscale",
            "-f",
            "framemd5",
            "-",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.rsplit(',').next().map(|s| s.trim().to_owned()))
            .collect(),
    )
}

fn stream(file: &Path) -> Option<(Option<i32>, Value)> {
    let out = Command::new(env!("CARGO_BIN_EXE_decode-oracle"))
        .arg("stream")
        .arg(file)
        .output()
        .ok()?;
    let document = serde_json::from_slice(&out.stdout).ok()?;
    Some((out.status.code(), document))
}

#[test]
fn frames_equal_framemd5() {
    let require = std::env::var_os("COREAV_REQUIRE_FFMPEG").is_some();
    let Some(ffmpeg) = ffmpeg() else {
        assert!(!require, "no ffmpeg");
        return;
    };
    let sandboxed = cfg!(all(unix, not(target_vendor = "apple")));
    let cases = [
        Case {
            name: "h264.mp4",
            codec: "h264",
            decoder: None,
            enabled: cfg!(feature = "native-h264"),
            encoders: &[&["-c:v", "libx264", "-pix_fmt", "yuv420p", "-bf", "2"]],
        },
        Case {
            name: "h264.mkv",
            codec: "h264",
            decoder: None,
            enabled: cfg!(feature = "native-h264"),
            encoders: &[&["-c:v", "libx264", "-pix_fmt", "yuv420p", "-bf", "0"]],
        },
        Case {
            name: "vp9.webm",
            codec: "vp9",
            decoder: Some("vp9"),
            enabled: cfg!(feature = "libvpx") && sandboxed,
            encoders: &[&["-c:v", "libvpx-vp9"]],
        },
        Case {
            name: "vp8.webm",
            codec: "vp8",
            decoder: Some("vp8"),
            enabled: cfg!(feature = "libvpx") && sandboxed,
            encoders: &[&["-c:v", "libvpx"]],
        },
        Case {
            name: "av1.mkv",
            codec: "av1",
            decoder: Some("libdav1d"),
            enabled: cfg!(feature = "dav1d") && sandboxed,
            encoders: &[
                &["-c:v", "libsvtav1"],
                &["-c:v", "libaom-av1", "-cpu-used", "8"],
            ],
        },
    ];
    let dir = scratch("frames").expect("scratch directory");
    let mut compared = 0;
    for case in cases.iter().filter(|c| c.enabled) {
        let file = dir.join(case.name);
        let made = case
            .encoders
            .iter()
            .any(|encoder| encode(&ffmpeg, &file, encoder));
        if !made {
            assert!(!require, "no encoder for {}", case.name);
            continue;
        }
        let ext = case.name.rsplit('.').next().unwrap_or("");
        let md5s = reference(&ffmpeg, &file, ext, case.decoder).expect("the reference decodes");
        let (code, document) = stream(&file).expect("decode-oracle prints one JSON object");
        assert_eq!(document["outcome"], "ok", "{}: {document}", case.name);
        assert_eq!(code, Some(0), "{}", case.name);
        assert_eq!(document["codec"], case.codec, "{}", case.name);
        assert_eq!(document["stream_index"], 0, "{}", case.name);
        let ours: Vec<&str> = document["frames"]
            .as_array()
            .expect("frames")
            .iter()
            .filter_map(|f| f["md5"].as_str())
            .collect();
        assert!(
            !md5s.is_empty(),
            "{}: the reference lists no frame",
            case.name
        );
        assert_eq!(ours, md5s, "{}", case.name);
        println!("compared {} ({} frames)", case.name, md5s.len());
        compared += 1;
    }
    std::fs::remove_dir_all(&dir).ok();
    assert!(!require || compared > 0, "nothing compared");
}

#[cfg(feature = "native-h264")]
#[test]
fn a_stream_outside_the_decoder_fails_at_open_with_exit_1() {
    let Some(ffmpeg) = ffmpeg() else {
        return;
    };
    let dir = scratch("outside").expect("scratch directory");
    let file = dir.join("h264_422.mp4");
    if !encode(&ffmpeg, &file, &["-c:v", "libx264", "-pix_fmt", "yuv422p"]) {
        std::fs::remove_dir_all(&dir).ok();
        return;
    }
    let (code, document) = stream(&file).expect("decode-oracle prints one JSON object");
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(code, Some(1));
    assert_eq!(document["codec"], "h264");
    assert_eq!(document["frames"].as_array().map(Vec::len), Some(0));
    assert_eq!(document["outcome"]["stage"], "open");
    assert_eq!(document["outcome"]["kind"], "unsupported");
    assert_eq!(document["outcome"]["packets_sent"], 0);
}

#[test]
fn a_file_that_is_not_media_fails_at_read_with_exit_1() {
    let dir = scratch("notmedia").expect("scratch directory");
    let file = dir.join("not_media.mp4");
    std::fs::write(&file, b"RIFF\x24\x00\x00\x00AVI LIST").expect("write");
    let (code, document) = stream(&file).expect("decode-oracle prints one JSON object");
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(code, Some(1));
    assert_eq!(document["stream_index"], Value::Null);
    assert_eq!(document["outcome"]["stage"], "read");
    assert_eq!(document["outcome"]["kind"], "unsupported_container");
}
