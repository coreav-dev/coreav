//! Compare legacy transcode bytes and diagnostics with a frozen command.

#![cfg(feature = "transcode")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[path = "support/bounded.rs"]
mod bounded;

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
struct ResultBytes {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    media: Option<Vec<u8>>,
}

fn run(
    binary: &Path,
    input: &Path,
    output: &Path,
    args: &[&str],
    missing_encoders: bool,
) -> ResultBytes {
    let _ = std::fs::remove_file(output);
    let mut command = Command::new(binary);
    if args.contains(&"--strict-safe") {
        command.arg("--strict-safe");
    }
    command
        .arg("transcode")
        .arg(input)
        .arg(output)
        .args(args.iter().filter(|arg| **arg != "--strict-safe"));
    if missing_encoders {
        command.env("COREAV_X264", "/nonexistent/coreav-legacy-x264");
        command.env("COREAV_FFMPEG", "/nonexistent/coreav-legacy-ffmpeg");
    }
    let done = bounded::output(&mut command, None, Duration::from_secs(60)).unwrap();
    ResultBytes {
        code: done.status.code(),
        stdout: done.stdout,
        stderr: done.stderr,
        media: std::fs::read(output).ok(),
    }
}

#[test]
fn legacy_encoder_choices_keep_media_summaries_errors_and_exit_codes() {
    let Some(baseline) = std::env::var_os("COREAV_LEGACY_TRANSCODE").map(PathBuf::from) else {
        eprintln!("skipping frozen-binary comparison outside its hosted fixture job");
        return;
    };
    assert!(baseline.is_file(), "missing frozen transcode binary");
    let candidate = Path::new(env!("CARGO_BIN_EXE_coreav"));
    let fixtures = PathBuf::from(std::env::var_os("COREAV_FIXTURES").unwrap());
    let valid = fixtures.join("basic.mp4");
    assert!(valid.is_file());
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("legacy-encoders");
    std::fs::create_dir_all(&directory).unwrap();
    let invalid = directory.join("invalid.mp4");
    let absent = directory.join("absent.mp4");
    std::fs::write(&invalid, b"not a media header").unwrap();
    let _ = std::fs::remove_file(&absent);
    let lossless = directory.join("lossless.mkv");
    let mut make = Command::new(test_oracle::require().ffmpeg());
    make.args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(&valid)
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "flac",
            "-shortest",
        ])
        .arg(&lossless);
    let made = bounded::output(&mut make, None, Duration::from_secs(60)).unwrap();
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let output = directory.join("output.mp4");
    for encoder in [None, Some("auto"), Some("x264"), Some("ffmpeg")] {
        let mut choice = Vec::new();
        if let Some(encoder) = encoder {
            choice.extend(["--encoder", encoder]);
        }
        for audio in ["copy", "none", "aac", "opus"] {
            let mut args = choice.clone();
            args.extend(["--audio", audio, "--threads", "1"]);
            let input = if matches!(audio, "aac" | "opus") {
                &lossless
            } else {
                &valid
            };
            let before = run(&baseline, input, &output, &args, false);
            let after = run(candidate, input, &output, &args, false);
            assert!(
                after == before,
                "legacy request {args:?}: code {} stdout {} stderr {} media {}",
                after.code == before.code,
                after.stdout == before.stdout,
                after.stderr == before.stderr,
                after.media == before.media,
            );
            assert_eq!(
                after.code,
                Some(0),
                "{args:?}: {}",
                String::from_utf8_lossy(&after.stderr)
            );
            assert!(after.media.is_some());
            if audio == "aac" {
                for selected in ["auto", "ffmpeg"] {
                    let mut explicit = args.clone();
                    explicit.extend(["--audio-encoder", selected]);
                    let selected = run(candidate, input, &output, &explicit, false);
                    assert!(
                        selected == before,
                        "explicit AAC choice {explicit:?} changed output"
                    );
                }
            }
        }
        for input in [&valid, &invalid, &absent] {
            for missing in [false, true] {
                for strict in [false, true] {
                    let mut args = choice.clone();
                    if strict {
                        args.push("--strict-safe");
                    }
                    let before = run(&baseline, input, &output, &args, missing);
                    let after = run(candidate, input, &output, &args, missing);
                    assert!(
                        after == before,
                        "legacy {args:?}, {input:?}, missing={missing}: code {} stdout {} stderr {} media {}",
                        after.code == before.code,
                        after.stdout == before.stdout,
                        after.stderr == before.stderr,
                        after.media == before.media,
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}
