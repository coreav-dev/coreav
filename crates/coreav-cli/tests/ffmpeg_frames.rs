//! Image, raw-video and YUV4MPEG output against the pinned oracle binary.

#![cfg(feature = "native-h264")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    dir: PathBuf,
    oracle: PathBuf,
    input: PathBuf,
}

fn checked(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn fixture(name: &str) -> Option<Fixture> {
    let oracle = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    if !oracle.is_file() {
        assert!(
            std::env::var_os("CI").is_none() && std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none(),
            "install the pinned oracle or set COREAV_FFMPEG"
        );
        eprintln!("skipping: no pinned oracle (set COREAV_FFMPEG)");
        return None;
    }
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ffmpeg-frames-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.mp4");
    checked(
        Command::new(&oracle)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=5:duration=2",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&input),
    );
    Some(Fixture { dir, oracle, input })
}

impl Fixture {
    fn coreav(&self, args: &[&str], name: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffmpeg", "-i"])
            .arg(&self.input)
            .args(args)
            .arg(if name == "-" {
                PathBuf::from("-")
            } else {
                self.dir.join(name)
            })
            .env("COREAV_FALLBACK", self.dir.join("must-not-run"))
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str], name: &str) -> Vec<u8> {
        let output = self.coreav(args, name);
        assert!(
            output.status.success(),
            "{args:?} {name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        output.stdout
    }

    fn decode(&self, name: &str, format: &str) -> Vec<u8> {
        checked(
            Command::new(&self.oracle)
                .args(["-v", "error", "-i"])
                .arg(self.dir.join(name))
                .args(["-pix_fmt", format, "-f", "rawvideo", "-"]),
        )
    }

    fn fields(&self, name: &str) -> serde_json::Value {
        serde_json::from_slice(&checked(
            Command::new(self.oracle.with_file_name("ffprobe"))
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                "stream=codec_name,width,height,pix_fmt,sample_aspect_ratio,display_aspect_ratio,color_range,chroma_location,r_frame_rate",
                    "-of",
                    "json",
                ])
                .arg(self.dir.join(name)),
        ))
        .unwrap()
    }
}

#[test]
fn raw_file_pipe_and_y4m_keep_every_decoded_sample() {
    let Some(f) = fixture("raw") else { return };
    let reference = f.decode("input.mp4", "yuv420p");
    assert_eq!(reference.len(), 64 * 48 * 3 / 2 * 10);
    f.success(&["-c:v", "rawvideo", "-pix_fmt", "yuv420p"], "out.yuv");
    assert_eq!(std::fs::read(f.dir.join("out.yuv")).unwrap(), reference);
    assert_eq!(f.success(&["-f", "rawvideo"], "-"), reference);
    f.success(&[], "out.y4m");
    assert_eq!(f.decode("out.y4m", "yuv420p"), reference);
    checked(
        Command::new(&f.oracle)
            .args(["-v", "error", "-i"])
            .arg(&f.input)
            .arg(f.dir.join("reference.y4m")),
    );
    assert_eq!(f.fields("out.y4m"), f.fields("reference.y4m"));
    f.success(&["-ss", "0.4", "-t", "0.6"], "trim.yuv");
    let frame_bytes = 64 * 48 * 3 / 2;
    assert_eq!(
        std::fs::read(f.dir.join("trim.yuv")).unwrap(),
        reference[frame_bytes * 2..frame_bytes * 5]
    );
}

#[test]
fn image_names_limits_seeking_and_update_are_codec_independent() {
    let Some(f) = fixture("images") else { return };
    for extension in ["png", "bmp"] {
        let single = format!("single.{extension}");
        f.success(&["-frames:v", "1"], &single);
        let reference = format!("reference.{extension}");
        checked(
            Command::new(&f.oracle)
                .args(["-v", "error", "-i"])
                .arg(&f.input)
                .args(["-frames:v", "1"])
                .arg(f.dir.join(&reference)),
        );
        assert_eq!(f.fields(&single), f.fields(&reference));
        for pattern in ["%d", "%03d", "%07d"] {
            f.success(
                &["-frames:v", "2", "-f", "image2"],
                &format!("{pattern}.{extension}"),
            );
            let width = match pattern {
                "%03d" => 3,
                "%07d" => 7,
                _ => 0,
            };
            assert!(f.dir.join(format!("{:0width$}.{extension}", 2)).is_file());
            assert!(!f.dir.join(format!("{:0width$}.{extension}", 3)).exists());
        }
        f.success(&["-update", "1"], &format!("update.{extension}"));
        f.success(
            &["-ss", "1.8", "-vframes", "1"],
            &format!("last.{extension}"),
        );
        assert_eq!(
            f.decode(&format!("update.{extension}"), "rgb24"),
            f.decode(&format!("last.{extension}"), "rgb24")
        );
        f.success(&["-ss", "1", "-vframes", "1"], &format!("seek.{extension}"));
        f.success(
            &["-ss", "1", "-vframes", "1", "-update", "1"],
            &format!("same.{extension}"),
        );
        assert_eq!(
            f.decode(&format!("seek.{extension}"), "rgb24"),
            f.decode(&format!("same.{extension}"), "rgb24")
        );
        checked(
            Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["--strict-safe", "ffmpeg", "-ss", "1", "-i"])
                .arg(&f.input)
                .args(["-vframes", "1"])
                .arg(f.dir.join(format!("input-seek.{extension}"))),
        );
        assert_eq!(
            f.decode(&format!("input-seek.{extension}"), "rgb24"),
            f.decode(&format!("seek.{extension}"), "rgb24")
        );
    }
}

#[test]
fn raw_stdout_keeps_the_closed_pipe_exit_status() {
    let Some(f) = fixture("closed-pipe") else {
        return;
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["--strict-safe", "ffmpeg", "-i"])
        .arg(&f.input)
        .args(["-frames:v", "1", "-f", "rawvideo", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(141));
    assert!(output.stderr.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn raw_stdout_reports_a_final_flush_failure() {
    let Some(f) = fixture("flush-failure") else {
        return;
    };
    checked(
        Command::new(&f.oracle)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=black:size=8x8:rate=1:duration=1",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&f.input),
    );
    let reference = f.decode("input.mp4", "yuv420p");
    assert_eq!(reference.len(), 96);
    assert!(!reference.contains(&b'\n'));
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["--strict-safe", "ffmpeg", "-i"])
        .arg(&f.input)
        .args(["-frames:v", "1", "-f", "rawvideo", "-"])
        .stdout(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(!output.stderr.is_empty());
}

#[test]
fn dropping_frames_matches_the_oracles_raw_samples() {
    let Some(f) = fixture("rate") else { return };
    for rate in ["1", "1/1", "0.5", "5"] {
        let name = format!("rate-{}.yuv", rate.replace('/', "-"));
        f.success(&["-r", rate], &name);
        let reference = checked(
            Command::new(&f.oracle)
                .args(["-v", "error", "-i"])
                .arg(&f.input)
                .args(["-r", rate, "-f", "rawvideo", "-"]),
        );
        assert_eq!(
            std::fs::read(f.dir.join(name)).unwrap(),
            reference,
            "rate {rate}"
        );
    }
    f.success(&["-r", "1/1"], "frame%03d.bmp");
    assert!(f.dir.join("frame004.bmp").is_file());
    assert!(!f.dir.join("frame005.bmp").exists());
}

#[test]
fn unsupported_conversion_and_duplication_create_no_output() {
    let Some(f) = fixture("refusals") else { return };
    for (args, name) in [
        (vec!["-pix_fmt", "rgb24"], "conversion.yuv"),
        (vec!["-r", "1000"], "duplicate%07d.png"),
        (vec!["-s", "1920x1080"], "scaled.yuv"),
        (vec!["-vf", "scale=32:24"], "filtered.png"),
        (vec!["-map", "0:v", "-map", "0:v"], "twice.yuv"),
    ] {
        let output = f.coreav(&args, name);
        assert_eq!(
            output.status.code(),
            Some(4),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert!(!f.dir.join(name).exists());
    }
    assert_eq!(std::fs::read_dir(&f.dir).unwrap().count(), 1);
}

#[test]
fn images_and_raw_output_never_replace_the_input() {
    let Some(f) = fixture("overwrite") else {
        return;
    };
    let before = std::fs::read(&f.input).unwrap();
    let output = f.coreav(&["-y", "-f", "rawvideo"], "input.mp4");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(std::fs::read(&f.input).unwrap(), before);
    f.success(&["-frames:v", "1"], "out.png");
    assert_eq!(
        f.coreav(&["-n", "-frames:v", "1"], "out.png").status.code(),
        Some(2)
    );
    assert_eq!(
        f.coreav(&["-frames:v", "1"], "out.png").status.code(),
        Some(4)
    );
    f.success(&["-y", "-frames:v", "1"], "out.png");
    f.success(
        &[
            "-ss", "10", "-vframes", "1", "-c:v", "png", "-an", "-f", "rawvideo",
        ],
        "empty.png",
    );
    assert!(std::fs::read(f.dir.join("empty.png")).unwrap().is_empty());
}

#[test]
fn the_oracle_recovers_every_sample_of_the_image_encoders() {
    use coreav_frame::{Chroma, Picture, PixelFormat};
    let Some(f) = fixture("image-samples") else {
        return;
    };
    for (format, name, decoded) in [
        (PixelFormat::Rgb24, "rgb.png", "rgb24"),
        (PixelFormat::Rgba, "rgba.png", "rgba"),
        (
            PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 8,
            },
            "gray.png",
            "gray",
        ),
        (
            PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 16,
            },
            "gray16.png",
            "gray16le",
        ),
    ] {
        let mut picture = Picture::zeroed(5, 3, format).unwrap();
        for y in 0..3 {
            for (x, value) in picture.row_mut(0, y).unwrap().iter_mut().enumerate() {
                *value = (y * 73 + x * 41) as u8;
            }
        }
        let mut expected = Vec::new();
        picture.write_packed(&mut expected).unwrap();
        std::fs::write(
            f.dir.join(name),
            coreav_thumbnail::encode_png(&picture).unwrap(),
        )
        .unwrap();
        assert_eq!(f.decode(name, decoded), expected, "{format:?}");
        if format == PixelFormat::Rgb24 {
            std::fs::write(
                f.dir.join("rgb.bmp"),
                coreav_thumbnail::encode_bmp(&picture).unwrap(),
            )
            .unwrap();
            assert_eq!(f.decode("rgb.bmp", "rgb24"), expected);
        }
    }
}

#[test]
fn seeking_counts_leading_empty_edits_on_the_input_timeline() {
    let Some(mut f) = fixture("empty-edit") else {
        return;
    };
    let delayed = f.dir.join("delayed.mp4");
    checked(
        Command::new(&f.oracle)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=4",
                "-itsoffset",
                "2",
                "-i",
            ])
            .arg(&f.input)
            .args([
                "-map",
                "1:v",
                "-map",
                "0:a",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-use_editlist",
                "1",
            ])
            .arg(&delayed),
    );
    f.input = delayed;
    f.success(&["-ss", "1", "-frames:v", "1"], "after-seek.yuv");
    let expected = checked(
        Command::new(&f.oracle)
            .args(["-v", "error", "-i"])
            .arg(&f.input)
            .args(["-ss", "1", "-frames:v", "1", "-f", "rawvideo", "-"]),
    );
    assert!(!expected.is_empty());
    assert_eq!(
        std::fs::read(f.dir.join("after-seek.yuv")).unwrap(),
        expected
    );
    f.success(&["-t", "1"], "before-video.yuv");
    assert!(
        std::fs::read(f.dir.join("before-video.yuv"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unavailable_decoders_refuse_before_output_and_allow_normal_fallback() {
    let Some(mut f) = fixture("unavailable") else {
        return;
    };
    let input = f.dir.join("hevc.mp4");
    checked(
        Command::new(&f.oracle)
            .args(["-v", "error", "-i"])
            .arg(&f.input)
            .args([
                "-frames:v",
                "2",
                "-c:v",
                "libx265",
                "-x265-params",
                "pools=1:frame-threads=1:log-level=error",
                "-threads",
                "1",
            ])
            .arg(&input),
    );
    f.input = input;
    let strict = f.coreav(&["-frames:v", "1"], "strict.png");
    assert_eq!(
        strict.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&strict.stderr)
    );
    assert!(!f.dir.join("strict.png").exists());
    let output = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["ffmpeg", "-i"])
        .arg(&f.input)
        .args(["-frames:v", "1"])
        .arg(f.dir.join("normal.png"))
        .env_remove("COREAV_STRICT_SAFE")
        .env("COREAV_FALLBACK", &f.oracle)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(f.dir.join("normal.png").is_file());
}

#[test]
fn y4m_refuses_autorotation_that_would_misstate_chroma_siting() {
    let Some(mut f) = fixture("rotation") else {
        return;
    };
    let input = f.dir.join("turned.mp4");
    checked(
        Command::new(&f.oracle)
            .args(["-v", "error", "-display_rotation:v:0", "180", "-i"])
            .arg(&f.input)
            .args(["-c", "copy"])
            .arg(&input),
    );
    f.input = input;
    assert_eq!(f.coreav(&[], "turned.y4m").status.code(), Some(4));
    assert!(!f.dir.join("turned.y4m").exists());
    checked(
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffmpeg", "-noautorotate", "-i"])
            .arg(&f.input)
            .arg(f.dir.join("unturned.y4m")),
    );
    assert_eq!(
        f.decode("unturned.y4m", "yuv420p"),
        f.decode("input.mp4", "yuv420p")
    );
}
