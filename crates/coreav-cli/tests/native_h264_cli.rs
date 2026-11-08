//! Outer selector, unchanged help, and strict-safe behavior without an oracle.
#![cfg(feature = "native-h264-encoder")]
#![allow(clippy::unwrap_used)]

use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(args)
        .env_remove("COREAV_STRICT_SAFE")
        .env("COREAV_X264", "/nonexistent/native-h264-x264")
        .env("COREAV_FFMPEG", "/nonexistent/native-h264-ffmpeg")
        .env("COREAV_FALLBACK", "/nonexistent/native-h264-fallback")
        .output()
        .unwrap()
}

#[test]
fn strict_safe_selection_keeps_the_existing_refusal_bytes() {
    for extra in [["-crf", "23"], ["-qp", "23"], ["-bf", "2"]] {
        let args = [
            "--strict-safe",
            "ffmpeg",
            "-i",
            "/nonexistent/native-input.mp4",
            "-an",
            "-c:v",
            "libx264",
            extra[0],
            extra[1],
            "/nonexistent/native-output.mp4",
        ];
        let expected = run(&args);
        let mut native = vec!["--native-h264-encoder"];
        native.extend_from_slice(&args);
        let actual = run(&native);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[test]
fn selection_is_outer_and_requires_the_ffmpeg_command() {
    let output = run(&["--native-h264-encoder", "ffprobe", "--help"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("--native-h264-encoder requires the ffmpeg command")
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn missing_input_keeps_the_existing_error_bytes() {
    let args = [
        "ffmpeg",
        "-i",
        "/nonexistent/native-input.mp4",
        "-an",
        "-c:v",
        "libx264",
        "/nonexistent/native-output.mp4",
    ];
    let expected = run(&args);
    let mut native = vec!["--native-h264-encoder"];
    native.extend_from_slice(&args);
    let actual = run(&native);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

#[test]
fn the_ffmpeg_help_bytes_do_not_depend_on_selection() {
    let expected = run(&["ffmpeg", "-h"]);
    let actual = run(&["--native-h264-encoder", "ffmpeg", "-h"]);
    assert_eq!(actual.status.code(), Some(0));
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}
