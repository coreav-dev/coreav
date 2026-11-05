//! Transcode-only selection preflight, refusal and legacy-error regressions.

#![cfg(feature = "transcode")]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "support/bounded.rs"]
mod bounded;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("transcode-selection-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(input: &Path, output: &Path, flags: &[&str], strict: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command
        .env_remove("COREAV_STRICT_SAFE")
        .env("COREAV_X264", "/nonexistent/coreav-selection-x264")
        .env("COREAV_FFMPEG", "/nonexistent/coreav-selection-ffmpeg");
    if strict {
        command.arg("--strict-safe");
    }
    command.arg("transcode").arg(input).arg(output).args(flags);
    bounded::output(&mut command, None, Duration::from_secs(10)).unwrap()
}

fn program(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(format!("{name}.exe"));
    std::fs::write(&path, b"#!/bin/sh\nexit 99\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn refused(output: Output, message: &str) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(
        error,
        json!({
            "schema": "coreav.error/1",
            "kind": "unsupported_feature",
            "limit": null,
            "offset": null,
            "message": format!("unsupported: {message}"),
        })
    );
}

const NO_AV1: &str = "no AV1 encoder: this build has no SVT-AV1 backend";
const AV1_THREADS: &str = "AV1 does not support --threads: parallelism level is not a thread limit";
const NO_PLATFORM_VIDEO: &str = "no platform H.264 encoder: this build has no platform backend";
const NO_PLATFORM_AUDIO: &str = "no platform AAC encoder: this build has no platform backend";
const NO_HARDWARE: &str =
    "no hardware H.264 encoder is available: this build has no VideoToolbox backend";
const NO_H264: &str = "no H.264 encoder: install x264 or FFmpeg with libx264";
const NO_AAC: &str = "no AAC encoder: install FFmpeg or select --audio opus";
const STRICT_PLATFORM: &str = "strict-safe mode does not permit an in-process platform encoder";
const STRICT_AAC: &str = "strict-safe mode never runs the ffmpeg program, so it cannot encode AAC audio: use --audio copy, none or opus";

#[test]
fn new_selections_refuse_before_a_missing_input_or_output_is_opened() {
    let dir = dir("missing");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    let cases: &[(&[&str], &str)] = &[
        (&["--video", "av1"], NO_AV1),
        (&["--video", "av1", "--encoder", "auto"], NO_AV1),
        (&["--video", "av1", "--encoder", "svt-av1"], NO_AV1),
        (
            &["--encoder", "svt-av1"],
            "--encoder svt-av1 requires --video av1",
        ),
        (
            &["--video", "av1", "--encoder", "x264"],
            "--encoder x264 cannot encode AV1",
        ),
        (
            &["--video", "av1", "--encoder", "ffmpeg"],
            "--video av1 --encoder ffmpeg is not supported",
        ),
        (
            &["--video", "av1", "--encoder", "platform"],
            "--encoder platform cannot encode AV1",
        ),
        (&["--encoder", "platform"], NO_PLATFORM_VIDEO),
        (
            &["--encoder", "platform", "--require-hardware"],
            NO_HARDWARE,
        ),
        (
            &["--encoder", "platform", "--bitrate", "2000"],
            NO_PLATFORM_VIDEO,
        ),
        (
            &["--encoder", "platform", "--crf", "23"],
            "the selected platform encoder cannot honor --crf; use --bitrate or select x264",
        ),
        (
            &["--encoder", "platform", "--preset", "medium"],
            "the selected platform encoder cannot honor --preset; omit it or select x264",
        ),
        (&["--require-hardware"], NO_HARDWARE),
        (&["--require-hardware", "--encoder", "auto"], NO_HARDWARE),
        (
            &["--require-hardware", "--encoder", "x264"],
            "--require-hardware is incompatible with a non-platform video encoder",
        ),
        (
            &["--require-hardware", "--encoder", "ffmpeg"],
            "--require-hardware is incompatible with a non-platform video encoder",
        ),
        (
            &["--video", "av1", "--require-hardware"],
            "--encoder platform cannot encode AV1",
        ),
        (
            &[
                "--video",
                "av1",
                "--encoder",
                "svt-av1",
                "--require-hardware",
            ],
            "--require-hardware is incompatible with a non-platform video encoder",
        ),
        (&["--audio", "aac", "--audio-encoder", "auto"], NO_H264),
        (&["--audio", "aac", "--audio-encoder", "ffmpeg"], NO_H264),
        (
            &["--audio", "aac", "--audio-encoder", "platform"],
            NO_PLATFORM_AUDIO,
        ),
        (
            &["--encoder", "platform", "--scale", "invalid"],
            NO_PLATFORM_VIDEO,
        ),
        (&["--video", "av1", "--audio-bitrate", "64"], NO_AV1),
    ];
    for (flags, message) in cases {
        if *message == NO_AV1 && coreav_cli::transcode::av1::compiled() {
            continue;
        }
        refused(run(&input, &output, flags, false), message);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "{flags:?}");
    }
}

#[test]
fn audio_encoder_requires_aac_even_when_opus_is_not_built() {
    let dir = dir("audio");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    for encoder in ["auto", "platform", "ffmpeg"] {
        for mode in ["copy", "none", "opus"] {
            let flags = ["--audio", mode, "--audio-encoder", encoder];
            refused(
                run(&input, &output, &flags, false),
                "--audio-encoder needs --audio aac",
            );
        }
        refused(
            run(&input, &output, &["--audio-encoder", encoder], false),
            "--audio-encoder needs --audio aac",
        );
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn platform_strict_safe_refusals_use_the_transcode_exit() {
    let dir = dir("strict");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    for flags in [
        &["--encoder", "platform"][..],
        &["--encoder", "platform", "--require-hardware"],
        &["--require-hardware"],
        &["--encoder", "auto", "--require-hardware"],
        &["--audio", "aac", "--audio-encoder", "platform"],
    ] {
        refused(run(&input, &output, flags, true), STRICT_PLATFORM);
    }
    if !coreav_cli::transcode::av1::compiled() {
        refused(run(&input, &output, &["--video", "av1"], true), NO_AV1);
    }
    refused(
        run(&input, &output, &["--video", "av1", "--threads", "1"], true),
        AV1_THREADS,
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn explicit_aac_selection_keeps_strict_safe_but_uses_the_transcode_exit() {
    let dir = dir("strict-aac");
    for audio_encoder in ["auto", "ffmpeg"] {
        refused(
            run(
                &dir.join("missing.mp4"),
                &dir.join("out.mp4"),
                &["--audio", "aac", "--audio-encoder", audio_encoder],
                true,
            ),
            STRICT_AAC,
        );
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn aac_selection_discovers_video_and_audio_programs_before_reading_input() {
    let dir = dir("aac-programs");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    let x264 = program(&dir, "x264");
    let ffmpeg = program(&dir, "ffmpeg");
    refused(
        run(
            &input,
            &output,
            &["--video", "av1", "--ffmpeg", ffmpeg.to_str().unwrap()],
            false,
        ),
        if coreav_cli::transcode::av1::compiled() {
            "with AV1, --ffmpeg applies only to --audio aac"
        } else {
            NO_AV1
        },
    );
    for audio_encoder in ["auto", "ffmpeg"] {
        refused(
            run(
                &input,
                &output,
                &[
                    "--audio",
                    "aac",
                    "--audio-encoder",
                    audio_encoder,
                    "--x264",
                    x264.to_str().unwrap(),
                ],
                false,
            ),
            NO_AAC,
        );
        for video_encoder in ["auto", "x264", "ffmpeg"] {
            let flags = [
                "--audio",
                "aac",
                "--audio-encoder",
                audio_encoder,
                "--encoder",
                video_encoder,
                "--x264",
                x264.to_str().unwrap(),
                "--ffmpeg",
                ffmpeg.to_str().unwrap(),
            ];
            let result = run(&input, &output, &flags, false);
            assert_eq!(result.status.code(), Some(3), "{flags:?}: {result:?}");
            assert!(result.stdout.is_empty());
            let error: Value = serde_json::from_slice(&result.stderr).unwrap();
            assert_eq!(error["kind"], "io");
            assert_eq!(
                error["message"],
                format!("I/O error: {}", std::io::ErrorKind::NotFound)
            );
        }
        let result = run(
            &input,
            &output,
            &[
                "--audio",
                "aac",
                "--audio-encoder",
                audio_encoder,
                "--ffmpeg",
                ffmpeg.to_str().unwrap(),
            ],
            false,
        );
        assert_eq!(result.status.code(), Some(3), "{result:?}");
    }
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
}

#[test]
fn new_flags_refuse_missing_explicit_program_paths_without_fallback() {
    let dir = dir("explicit-programs");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    let x264 = program(&dir, "x264");
    let ffmpeg = program(&dir, "ffmpeg");
    let missing = dir.join("missing-encoder.exe");
    for (flags, message) in [
        (
            vec![
                "--encoder",
                "x264",
                "--x264",
                missing.to_str().unwrap(),
                "--ffmpeg",
                ffmpeg.to_str().unwrap(),
            ],
            "no H.264 encoder: the selected x264 program is unavailable",
        ),
        (
            vec![
                "--encoder",
                "ffmpeg",
                "--x264",
                x264.to_str().unwrap(),
                "--ffmpeg",
                missing.to_str().unwrap(),
            ],
            "no H.264 encoder: the selected FFmpeg program is unavailable",
        ),
        (
            vec![
                "--x264",
                x264.to_str().unwrap(),
                "--ffmpeg",
                missing.to_str().unwrap(),
            ],
            NO_AAC,
        ),
    ] {
        let mut line = vec!["--audio", "aac", "--audio-encoder", "auto"];
        line.extend(flags);
        refused(run(&input, &output, &line, false), message);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }
}

#[cfg(unix)]
#[test]
fn new_flags_require_executable_files_and_auto_skips_a_nonexecutable_x264() {
    use std::os::unix::fs::PermissionsExt;

    let dir = dir("nonexecutable");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    let x264 = program(&dir, "x264");
    let ffmpeg = program(&dir, "ffmpeg");
    std::fs::set_permissions(&x264, std::fs::Permissions::from_mode(0o644)).unwrap();
    refused(
        run(
            &input,
            &output,
            &[
                "--audio",
                "aac",
                "--audio-encoder",
                "auto",
                "--encoder",
                "x264",
                "--x264",
                x264.to_str().unwrap(),
                "--ffmpeg",
                ffmpeg.to_str().unwrap(),
            ],
            false,
        ),
        "no H.264 encoder: the selected x264 program is unavailable",
    );
    let result = run(
        &input,
        &output,
        &[
            "--audio",
            "aac",
            "--audio-encoder",
            "auto",
            "--x264",
            x264.to_str().unwrap(),
            "--ffmpeg",
            ffmpeg.to_str().unwrap(),
        ],
        false,
    );
    assert_eq!(result.status.code(), Some(3), "{result:?}");
    std::fs::set_permissions(&x264, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o644)).unwrap();
    refused(
        run(
            &input,
            &output,
            &[
                "--audio",
                "aac",
                "--audio-encoder",
                "ffmpeg",
                "--x264",
                x264.to_str().unwrap(),
                "--ffmpeg",
                ffmpeg.to_str().unwrap(),
            ],
            false,
        ),
        NO_AAC,
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
}

#[test]
fn a_refused_selection_does_not_touch_an_existing_output() {
    let dir = dir("existing-output");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    std::fs::write(&output, b"retain this output").unwrap();
    if !coreav_cli::transcode::av1::compiled() {
        for flags in [&["--video", "av1"][..], &["--video", "av1", "--force"]] {
            refused(run(&input, &output, flags, false), NO_AV1);
            assert_eq!(std::fs::read(&output).unwrap(), b"retain this output");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        }
    }
    for flags in [
        &["--video", "av1", "--threads", "1"][..],
        &["--video", "av1", "--threads", "1", "--force"],
    ] {
        refused(run(&input, &output, flags, false), AV1_THREADS);
        assert_eq!(std::fs::read(&output).unwrap(), b"retain this output");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn hardware_refusals_do_not_read_input_or_modify_forced_output() {
    let dir = dir("hardware-boundary");
    let input = dir.join("fifo.mp4");
    let output = dir.join("out.mp4");
    assert!(
        Command::new("mkfifo")
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(&output, b"retain hardware refusal output").unwrap();
    for (flags, message) in [
        (&["--require-hardware", "--force"][..], NO_HARDWARE),
        (
            &["--require-hardware", "--encoder", "platform", "--force"][..],
            NO_HARDWARE,
        ),
        (
            &["--require-hardware", "--encoder", "x264", "--force"][..],
            "--require-hardware is incompatible with a non-platform video encoder",
        ),
        (
            &["--require-hardware", "--encoder", "ffmpeg", "--force"][..],
            "--require-hardware is incompatible with a non-platform video encoder",
        ),
    ] {
        refused(run(&input, &output, flags, false), message);
        assert_eq!(
            std::fs::read(&output).unwrap(),
            b"retain hardware refusal output"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }
}

#[cfg(unix)]
#[test]
fn new_selections_do_not_open_a_blocking_input() {
    let dir = dir("fifo");
    let input = dir.join("fifo.mp4");
    let output = dir.join("out.mp4");
    assert!(
        Command::new("mkfifo")
            .arg(&input)
            .status()
            .unwrap()
            .success()
    );
    if !coreav_cli::transcode::av1::compiled() {
        refused(run(&input, &output, &["--video", "av1"], false), NO_AV1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
    for (flags, message) in [
        (&["--video", "av1", "--threads", "1"][..], AV1_THREADS),
        (&["--encoder", "platform"][..], NO_PLATFORM_VIDEO),
        (
            &["--audio", "aac", "--audio-encoder", "platform"][..],
            NO_PLATFORM_AUDIO,
        ),
        (
            &["--audio", "aac", "--audio-encoder", "ffmpeg"][..],
            NO_H264,
        ),
    ] {
        refused(run(&input, &output, flags, false), message);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
}

#[cfg(feature = "net")]
#[test]
fn new_selections_make_no_network_request() {
    let dir = dir("network");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/missing.mp4", listener.local_addr().unwrap());
    let output = dir.join("out.mp4");
    if !coreav_cli::transcode::av1::compiled() {
        for flags in [
            &["--video", "av1"][..],
            &["--video", "av1", "--encoder", "svt-av1"],
        ] {
            refused(run(Path::new(&url), &output, flags, false), NO_AV1);
            assert!(listener.accept().is_err());
        }
    }
    for (flags, message) in [
        (&["--video", "av1", "--threads", "1"][..], AV1_THREADS),
        (
            &["--video", "av1", "--encoder", "svt-av1", "--threads", "1"][..],
            AV1_THREADS,
        ),
        (&["--encoder", "platform"][..], NO_PLATFORM_VIDEO),
        (&["--require-hardware"][..], NO_HARDWARE),
        (
            &["--audio", "aac", "--audio-encoder", "ffmpeg"][..],
            NO_H264,
        ),
    ] {
        refused(run(Path::new(&url), &output, flags, false), message);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
}

#[test]
fn legacy_missing_input_and_usage_precedence_stay_unchanged() {
    let dir = dir("legacy");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    for flags in [
        &[][..],
        &["--encoder", "auto"],
        &["--encoder", "x264"],
        &["--encoder", "ffmpeg"],
        &["--video", "h264"],
        &["--preset", "medium"],
    ] {
        let result = run(&input, &output, flags, false);
        assert_eq!(result.status.code(), Some(3), "{flags:?}: {result:?}");
        assert!(result.stdout.is_empty());
        let error: Value = serde_json::from_slice(&result.stderr).unwrap();
        assert_eq!(
            error,
            json!({
                "schema": "coreav.error/1",
                "kind": "io",
                "limit": null,
                "offset": null,
                "message": format!("I/O error: {}", std::io::ErrorKind::NotFound),
            })
        );
    }
    let result = run(
        &input,
        &output,
        &["--audio-bitrate", "64", "--scale", "invalid"],
        false,
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert_eq!(
        result.stderr,
        b"error: --audio-bitrate is the bit rate of the audio encode: it needs --audio opus or aac\n"
    );
    let result = run(&input, &output, &["--scale", "invalid"], false);
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert_eq!(
        result.stderr,
        b"error: --scale must be W:H, each side 1 to 16384 pixels, -1 (keep the aspect ratio) or -2 (keep it, even), not 'invalid'\n"
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn legacy_crf_and_preset_validators_still_reject_new_backend_settings() {
    let dir = dir("validators");
    for flags in [&["--crf", "52"][..], &["--preset", "8"]] {
        let result = run(&dir.join("missing.mp4"), &dir.join("out.mp4"), flags, false);
        assert_eq!(result.status.code(), Some(2), "{flags:?}: {result:?}");
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr).unwrap();
        assert!(stderr.starts_with("error: invalid value"), "{stderr}");
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
}

#[test]
fn valid_av1_settings_follow_build_availability_before_source_opening() {
    let dir = dir("av1-availability");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    for flags in [
        &["--video", "av1"][..],
        &["--video", "av1", "--encoder", "auto"],
        &["--video", "av1", "--encoder", "svt-av1"],
        &["--video", "av1", "--crf", "0"],
        &["--video", "av1", "--crf", "1"],
        &["--video", "av1", "--crf", "32"],
        &["--video", "av1", "--crf", "51"],
        &["--video", "av1", "--crf", "52"],
        &["--video", "av1", "--crf", "63"],
        &["--video", "av1", "--preset", "8"],
        &["--video", "av1", "--preset", "13", "--crf", "0"],
    ] {
        let result = run(&input, &output, flags, false);
        if coreav_cli::transcode::av1::compiled() {
            assert_eq!(result.status.code(), Some(3), "{flags:?}: {result:?}");
            assert!(result.stdout.is_empty());
            let error: Value = serde_json::from_slice(&result.stderr).unwrap();
            assert_eq!(error["kind"], "io");
        } else {
            refused(result, NO_AV1);
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
}

#[test]
fn av1_crf_admission_uses_the_exact_integer_token() {
    let dir = dir("av1-crf-token");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    std::fs::write(&output, b"retain this output").unwrap();
    for crf in ["32.0000001", "63.0000001", "1e-50", "32.5", "64"] {
        let result = run(
            &input,
            &output,
            &["--video", "av1", "--crf", crf, "--force"],
            false,
        );
        assert_eq!(result.status.code(), Some(2), "{crf}: {result:?}");
        assert!(result.stdout.is_empty(), "{crf}: {result:?}");
        let error = String::from_utf8(result.stderr).unwrap();
        assert!(
            error.contains("AV1 --crf must be an integer from 0 to 63"),
            "{crf}: {error}"
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"retain this output");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
}

#[test]
fn av1_rejects_unsupported_controls_before_media_or_output_access() {
    let dir = dir("av1-controls");
    let input = dir.join("missing.mp4");
    let output = dir.join("out.mp4");
    for (flags, message) in [
        (&["--video", "av1", "--threads", "1"][..], AV1_THREADS),
        (
            &["--video", "av1", "--bitrate", "1000"][..],
            "AV1 supports integer --crf only, not --bitrate",
        ),
        (
            &["--video", "av1", "--preset", "medium"][..],
            "AV1 --preset must be an integer from 0 to 13",
        ),
    ] {
        refused(run(&input, &output, flags, false), message);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }
}

#[test]
fn legacy_invalid_media_keeps_its_error_document() {
    let dir = dir("legacy-invalid");
    let input = dir.join("junk.bin");
    let output = dir.join("out.mp4");
    std::fs::write(&input, b"RIFF\x24\x00\x00\x00AVI LIST").unwrap();
    for encoder in ["auto", "x264", "ffmpeg"] {
        let result = run(&input, &output, &["--encoder", encoder], false);
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(result.stdout.is_empty());
        let error: Value = serde_json::from_slice(&result.stderr).unwrap();
        assert_eq!(
            error,
            json!({
                "schema": "coreav.error/1",
                "kind": "unsupported_container",
                "limit": null,
                "offset": null,
                "message": "not an ISO base media or QuickTime file",
            })
        );
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn help_names_unavailable_backends_and_settings() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command.args(["transcode", "--help"]);
    let result = bounded::output(&mut command, None, Duration::from_secs(10)).unwrap();
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let help = String::from_utf8(result.stdout).unwrap();
    let normalized = help.split_whitespace().collect::<Vec<_>>().join(" ");
    for phrase in [
        "--video",
        "AV1 requires a supported Unix build with native SVT-AV1",
        "integer 0 to 63 (default 32)",
        "numeric preset 0 to 13 (default 8)",
        "without an external encoder fallback",
        "AV1 refuses this option",
        "--audio-encoder",
        "`platform` is unavailable in this build",
        "--require-hardware",
        "Needs the VideoToolbox backend, unavailable in this build",
    ] {
        assert!(normalized.contains(phrase), "{phrase}: {help}");
    }
}
