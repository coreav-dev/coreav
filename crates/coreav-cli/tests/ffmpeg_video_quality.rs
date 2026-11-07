//! Generated native H.264 CLI admission and pinned quality-option comparisons.
#![cfg(feature = "native-h264")]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "support/bounded.rs"]
mod bounded;
#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

const UNAVAILABLE: &str = "/nonexistent/coreav-video-quality-tool";
const SENTINEL: &[u8] = b"preserve the complete destination";

struct Tools {
    oracle: &'static test_oracle::Oracle,
    x264: PathBuf,
    dir: PathBuf,
}

impl Drop for Tools {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn tools(name: &str) -> Option<Tools> {
    let oracle = test_oracle::require();
    let x264 = std::env::var_os("COREAV_X264")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/x264"));
    if !x264.is_file() {
        assert!(
            std::env::var_os("CI").is_none() && std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none(),
            "install x264 and set COREAV_X264 for required verification"
        );
        eprintln!("skipping optional x264 media test");
        return None;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("video-quality-{name}-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    Some(Tools { oracle, x264, dir })
}

fn run(command: &mut Command) -> Output {
    bounded::output(command, None, Duration::from_secs(30)).unwrap()
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "status={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn native(tools: &Tools, source: &Path, options: &[&str], output: &Path) -> Output {
    run(Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["ffmpeg", "-y", "-i"])
        .arg(source)
        .args(options)
        .arg(output)
        .env("COREAV_FALLBACK", UNAVAILABLE)
        .env("COREAV_FFMPEG", UNAVAILABLE)
        .env("COREAV_X264", &tools.x264)
        .env_remove("COREAV_STRICT_SAFE"))
}

fn reference(tools: &Tools, source: &Path, options: &[&str], output: &Path) -> Output {
    run(Command::new(tools.oracle.ffmpeg())
        .args(["-v", "error", "-y", "-i"])
        .arg(source)
        .args(options)
        .arg(output))
}

fn video(tools: &Tools) -> PathBuf {
    let raw = tools.dir.join("generated.yuv");
    let pixels: Vec<u8> = (0..32 * 24 * 3 / 2 * 3)
        .map(|i| ((i * 13 + i / 32 * 7) % 220 + 16) as u8)
        .collect();
    std::fs::write(&raw, pixels).unwrap();
    let source = tools.dir.join("source.mp4");
    success(run(Command::new(tools.oracle.ffmpeg())
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            "yuv420p",
            "-video_size",
            "32x24",
            "-framerate",
            "10",
            "-i",
        ])
        .arg(raw)
        .args(["-an", "-c:v", "libx264", "-threads", "1"])
        .arg(&source)));
    check_video(tools, &source);
    source
}

fn wave(tools: &Tools) -> PathBuf {
    let samples = 2400u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + samples * 2).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    bytes.extend_from_slice(&8000u32.to_le_bytes());
    bytes.extend_from_slice(&16000u32.to_le_bytes());
    bytes.extend_from_slice(b"\x02\0\x10\0data");
    bytes.extend_from_slice(&(samples * 2).to_le_bytes());
    for i in 0..samples {
        bytes.extend_from_slice(&((i % 200) as i16 * 100 - 10000).to_le_bytes());
    }
    let path = tools.dir.join("generated.wav");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn check_video(tools: &Tools, path: &Path) {
    let output = run(Command::new(tools.oracle.ffprobe())
        .args([
            "-v",
            "error",
            "-of",
            "json",
            "-count_frames",
            "-show_entries",
            "stream=codec_name,codec_type,width,height,nb_read_frames",
        ])
        .arg(path));
    assert!(output.status.success());
    let probe: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(probe["streams"].as_array().unwrap().len(), 1);
    let stream = &probe["streams"][0];
    assert_eq!(stream["codec_type"], "video");
    assert_eq!(stream["codec_name"], "h264");
    assert_eq!(stream["width"], 32);
    assert_eq!(stream["height"], 24);
    assert_eq!(stream["nb_read_frames"], "3");
}

fn refused(output: Output, destination: &Path) {
    assert_eq!(
        output.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read(destination).unwrap(), SENTINEL);
}

#[test]
fn h264_video_quality_is_ignored() {
    let Some(tools) = tools("ignored") else {
        return;
    };
    let source = video(&tools);
    let control = tools.dir.join("control.mp4");
    let candidate = tools.dir.join("candidate.mp4");
    let oracle_control = tools.dir.join("oracle-control.mp4");
    let oracle_candidate = tools.dir.join("oracle-candidate.mp4");
    let explicit = ["-an", "-c:v", "libx264", "-threads", "1"];
    for (codecs, values) in [
        (
            vec!["-c:v", "libx264"],
            vec!["2", "2.5", "0", "31", "-1", "100", "+2e0"],
        ),
        (vec![], vec!["2.5"]),
        (vec!["-c", "copy", "-c:v", "libx264"], vec!["2"]),
        (vec!["-c:v", "copy", "-c:v:0", "libx264"], vec!["2"]),
    ] {
        let mut options = vec!["-an"];
        options.extend(codecs);
        options.extend(["-threads", "1"]);
        success(native(&tools, &source, &options, &control));
        check_video(&tools, &control);
        success(reference(&tools, &source, &options, &oracle_control));
        for value in values {
            let mut requested = options.clone();
            requested.extend(["-q:v", value]);
            success(native(&tools, &source, &requested, &candidate));
            check_video(&tools, &candidate);
            assert_eq!(
                std::fs::read(&control).unwrap(),
                std::fs::read(&candidate).unwrap(),
                "native {requested:?}"
            );
            success(reference(&tools, &source, &requested, &oracle_candidate));
            check_video(&tools, &oracle_candidate);
            assert_eq!(
                std::fs::read(&oracle_control).unwrap(),
                std::fs::read(&oracle_candidate).unwrap(),
                "pinned {requested:?}"
            );
        }
    }
    success(reference(&tools, &source, &explicit, &oracle_control));
    success(reference(
        &tools,
        &source,
        &["-an", "-c:v", "libx264", "-threads", "1", "-q:v", "nan"],
        &oracle_candidate,
    ));
    assert_eq!(
        std::fs::read(oracle_control).unwrap(),
        std::fs::read(oracle_candidate).unwrap()
    );
    success(native(&tools, &source, &explicit, &control));
    success(native(
        &tools,
        &source,
        &[
            "-an", "-c:v", "libx264", "-threads", "1", "-q:v", "2", "-q:v", "2.5",
        ],
        &candidate,
    ));
    assert_eq!(
        std::fs::read(control).unwrap(),
        std::fs::read(candidate).unwrap()
    );
}

#[test]
fn video_quality_refusals_preserve_destinations() {
    let Some(tools) = tools("refusals") else {
        return;
    };
    let source = video(&tools);
    let wave = wave(&tools);
    let destination = tools.dir.join("refused.mp4");
    for value in ["abc", "nan", "inf", "-inf", "1e309", "2+1", "2k", ""] {
        std::fs::write(&destination, SENTINEL).unwrap();
        refused(
            native(
                &tools,
                &source,
                &["-an", "-c:v", "libx264", "-q:v", value],
                &destination,
            ),
            &destination,
        );
    }
    std::fs::write(&destination, SENTINEL).unwrap();
    assert!(
        !reference(
            &tools,
            &source,
            &["-an", "-c:v", "libx264", "-q:v", "abc"],
            &destination
        )
        .status
        .success()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), SENTINEL);
    refused(
        native(
            &tools,
            &source,
            &["-an", "-c:v", "libx264", "-q:v", "abc", "-q:v", "2"],
            &destination,
        ),
        &destination,
    );
    refused(
        run(Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["ffmpeg", "-y", "-q:v", "2", "-i"])
            .arg(&source)
            .args(["-an", "-c:v", "libx264"])
            .arg(&destination)
            .env("COREAV_FALLBACK", UNAVAILABLE)
            .env("COREAV_FFMPEG", UNAVAILABLE)
            .env("COREAV_X264", &tools.x264)
            .env_remove("COREAV_STRICT_SAFE")),
        &destination,
    );
    for (input, options, extension) in [
        (&source, vec!["-an", "-c", "copy"], "mp4"),
        (&source, vec!["-an", "-c:v", "libx264", "-c", "copy"], "mp4"),
        (
            &source,
            vec!["-an", "-c", "copy", "-c:v:9", "libx264"],
            "mp4",
        ),
        (&source, vec!["-an", "-c:v", "ffv1"], "mkv"),
        (&source, vec!["-an", "-c:v", "png", "-update", "1"], "png"),
        (&source, vec!["-an", "-c:v", "rawvideo"], "yuv"),
        (&wave, vec!["-vn", "-c:a", "pcm_s16le"], "wav"),
    ] {
        let destination = tools.dir.join(format!("scope.{extension}"));
        success(native(&tools, input, &options, &destination));
        std::fs::write(&destination, SENTINEL).unwrap();
        let mut requested = options;
        requested.extend(["-q:v", "2"]);
        refused(
            native(&tools, input, &requested, &destination),
            &destination,
        );
    }
    for (options, extension) in [
        (vec!["-an", "-c:v", "libx264", "-vf", "transpose=1"], "mp4"),
        (vec!["-an", "-c:v", "mjpeg"], "jpg"),
        (vec!["-an", "-c", "copy"], "ts"),
    ] {
        let destination = tools.dir.join(format!("unsupported.{extension}"));
        std::fs::write(&destination, SENTINEL).unwrap();
        refused(
            native(&tools, &source, &options, &destination),
            &destination,
        );
        let mut requested = options;
        requested.extend(["-q:v", "2"]);
        refused(
            native(&tools, &source, &requested, &destination),
            &destination,
        );
    }
}
