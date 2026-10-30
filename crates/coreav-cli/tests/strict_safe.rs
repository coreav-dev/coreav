//! Strict-safe mode runs no `ffmpeg` program (`docs/design/compat-cli.md`,
//! "Strict-safe mode"): not as the decoder of `coreav thumbnail` and
//! `coreav transcode`, not as the encoder of `coreav transcode` and the
//! `coreav ffmpeg` command lines that encode with libx264, and not in place
//! of a command that is not native. Observed without tracing: a stand-in
//! `ffmpeg` on `PATH` and in `COREAV_FFMPEG` (and a stand-in `x264` where
//! one is named) leaves a file behind when it runs. Each case also runs
//! outside the mode, where the stand-in does run, so that a stand-in that
//! could not record would not pass for one that was never run.
//!
//! In every build with a decoder backend: run in CI with the FFmpeg
//! fallback alone and with the native H.264 decoder alone. The source is an
//! H.264 MP4 made by the pinned FFmpeg (`COREAV_FFMPEG`, else
//! `/opt/coreav-oracle-bin/ffmpeg`, else `ffmpeg` on `PATH`); without one
//! the cases that need it skip, or fail in CI.
#![cfg(all(unix, feature = "transcode"))]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const NATIVE: bool = cfg!(feature = "native-h264");
const FALLBACK: bool = cfg!(feature = "ffmpeg-fallback");

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

/// A new, empty directory for one test: `bin`, the `PATH`, holds the
/// stand-in `ffmpeg` and `ffprobe`, and `x264` the stand-in `x264`, off
/// the `PATH`.
fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("strict-safe-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    for (sub, program) in [("bin", "ffmpeg"), ("bin", "ffprobe"), ("x264", "x264")] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
        let path = dir.join(sub).join(program);
        let mark = dir.join(format!("{program}-ran"));
        std::fs::write(
            &path,
            format!("#!/bin/sh\n: > '{}'\nexit 1\n", mark.display()),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// Whether the stand-in `program` ran.
fn ran(dir: &Path, program: &str) -> bool {
    dir.join(format!("{program}-ran")).exists()
}

fn forget(dir: &Path) {
    for program in ["ffmpeg", "ffprobe", "x264"] {
        let _ = std::fs::remove_file(dir.join(format!("{program}-ran")));
    }
}

/// An H.264 MP4 in `dir`, made by the pinned FFmpeg; `None` (a skip) when
/// there is none, a failure in CI.
fn source(dir: &Path) -> Option<PathBuf> {
    made(dir, "in.mp4", &[])
}

/// An H.264 MP4 with `audio` (the audio input and codec options) in `dir`,
/// as [`source`] makes it.
fn made(dir: &Path, name: &str, audio: &[&str]) -> Option<PathBuf> {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .or_else(|| {
            let pinned = PathBuf::from("/opt/coreav-oracle-bin/ffmpeg");
            pinned.is_file().then_some(pinned)
        })
        .unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let path = dir.join(name);
    let made = Command::new(&ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=s=64x48:d=0.4:r=25")
        .args(audio)
        .args(["-c:v", "libx264", "-bf", "0", "-pix_fmt", "yuv420p"])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    if made {
        return Some(path);
    }
    let required =
        std::env::var_os("CI").is_some() || std::env::var_os("COREAV_REQUIRE_FFMPEG").is_some();
    assert!(!required, "no ffmpeg with libx264 at {}", ffmpeg.display());
    eprintln!("skipping: no ffmpeg with libx264 (set COREAV_FFMPEG)");
    None
}

/// How a run turns strict-safe mode on.
#[derive(Clone, Copy, Debug)]
enum Mode {
    Off,
    Flag,
    Variable,
}

struct Run {
    code: i32,
    stderr: String,
}

/// `coreav ARGS` with only the stand-ins to find: `PATH` and
/// `COREAV_FFMPEG` name the stand-in `ffmpeg`, and `COREAV_X264` the
/// stand-in `x264` when `x264` is set.
fn coreav(dir: &Path, mode: Mode, x264: bool, args: &[&str]) -> Run {
    let out = command(dir, mode, x264, args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// [`coreav`] for a run that may block: failed when it has not ended
/// within ten seconds.
fn coreav_promptly(dir: &Path, mode: Mode, args: &[&str]) -> Run {
    let mut child = command(dir, mode, false, args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("{args:?} {mode:?}: still running after ten seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn command(dir: &Path, mode: Mode, x264: bool, args: &[&str]) -> Command {
    let mut command = Command::new(bin());
    command
        .current_dir(dir)
        .env_clear()
        .env("PATH", dir.join("bin"))
        .env("COREAV_FFMPEG", dir.join("bin/ffmpeg"));
    if x264 {
        command.env("COREAV_X264", dir.join("x264/x264"));
    }
    match mode {
        Mode::Off => {}
        Mode::Flag => {
            command.arg("--strict-safe");
        }
        Mode::Variable => {
            command.env("COREAV_STRICT_SAFE", "1");
        }
    }
    command.args(args);
    command
}

/// A refusal of strict-safe mode with exit status 4: the reason, the way
/// out `also` names, and turning the mode off; nothing written.
fn refused(run: &Run, dir: &Path, also: &str) {
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("strict-safe mode never runs the ffmpeg program"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains(also), "{}", run.stderr);
    assert!(
        run.stderr
            .contains("run without --strict-safe and COREAV_STRICT_SAFE"),
        "{}",
        run.stderr
    );
    assert!(!dir.join("out.mp4").exists() && !dir.join("out.jpg").exists());
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("out") || n.starts_with(".out"))
        .collect();
    names.sort();
    assert!(names.is_empty(), "left behind: {names:?}");
}

/// The `coreav.error/1` object of a refusal of `coreav transcode` or
/// `coreav thumbnail`.
fn error(run: &Run) -> Value {
    let error: Value = serde_json::from_str(run.stderr.trim()).unwrap();
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["kind"], "unsupported_feature");
    error
}

#[test]
fn the_ffmpeg_encoder_is_refused_before_the_input_is_read() {
    let dir = dir("encoder");
    for mode in [Mode::Flag, Mode::Variable] {
        let run = coreav(
            &dir,
            mode,
            false,
            &[
                "transcode",
                "missing.mp4",
                "out.mp4",
                "--encoder",
                "ffmpeg",
                "--ffmpeg",
                "bin/ffmpeg",
            ],
        );
        refused(&run, &dir, "use --encoder x264 with an x264 program");
        error(&run);
        let run = coreav(&dir, mode, false, &["transcode", "missing.mp4", "out.mp4"]);
        refused(&run, &dir, "install x264");
        error(&run);
        assert!(!ran(&dir, "ffmpeg") && !ran(&dir, "x264"), "{mode:?}");
    }
    // Outside the mode the input is read first.
    let run = coreav(
        &dir,
        Mode::Off,
        false,
        &["transcode", "missing.mp4", "out.mp4", "--encoder", "ffmpeg"],
    );
    assert_eq!(run.code, 3, "{}", run.stderr);
}

#[test]
fn transcode_runs_no_ffmpeg() {
    let dir = dir("transcode");
    let Some(_) = source(&dir) else { return };
    for mode in [Mode::Flag, Mode::Variable] {
        let run = coreav(&dir, mode, true, &["transcode", "in.mp4", "out.mp4"]);
        if NATIVE {
            // CoreAV's decoder, then the stand-in x264, which fails.
            assert_eq!(run.code, 1, "{}", run.stderr);
            assert!(ran(&dir, "x264"));
        } else {
            refused(&run, &dir, "build coreav with the native-h264 feature");
            error(&run);
            assert!(!ran(&dir, "x264"));
        }
        assert!(!ran(&dir, "ffmpeg"), "{mode:?}: {}", run.stderr);
        forget(&dir);
    }
    let run = coreav(&dir, Mode::Off, false, &["transcode", "in.mp4", "out.mp4"]);
    assert_ne!(run.code, 0);
    assert!(ran(&dir, "ffmpeg"), "{}", run.stderr);
}

/// The Opus encoder of `--audio opus` is the command itself, run through
/// the sandbox launcher: in the mode it runs, and no `ffmpeg` does.
#[cfg(feature = "libopus")]
#[test]
fn opus_audio_runs_no_ffmpeg() {
    let dir = dir("opus");
    let flac = ["-f", "lavfi", "-i", "sine=d=0.4", "-c:a", "flac"];
    let Some(_) = made(&dir, "in.mp4", &flac) else {
        return;
    };
    let line = ["transcode", "in.mp4", "out.mp4", "--audio", "opus"];
    for mode in [Mode::Flag, Mode::Variable] {
        let run = coreav(&dir, mode, true, &line);
        if NATIVE {
            // The Opus encode, then CoreAV's decoder and the stand-in
            // x264, which fails.
            assert_eq!(run.code, 1, "{}", run.stderr);
            assert!(run.stderr.contains("x264 failed"), "{}", run.stderr);
            assert!(ran(&dir, "x264"));
        } else {
            refused(&run, &dir, "build coreav with the native-h264 feature");
            error(&run);
            assert!(!ran(&dir, "x264"));
        }
        assert!(!ran(&dir, "ffmpeg"), "{mode:?}: {}", run.stderr);
        forget(&dir);
    }
    let run = coreav(&dir, Mode::Off, false, &line);
    assert_ne!(run.code, 0);
    assert!(ran(&dir, "ffmpeg"), "{}", run.stderr);
}

#[test]
fn thumbnail_runs_no_ffmpeg() {
    let dir = dir("thumbnail");
    let Some(_) = source(&dir) else { return };
    for exact in [false, true] {
        let mut args = vec!["thumbnail", "in.mp4", "-o", "out.jpg"];
        if exact {
            args.push("--exact");
        }
        for mode in [Mode::Flag, Mode::Variable] {
            let run = coreav(&dir, mode, false, &args);
            if NATIVE {
                assert_eq!(run.code, 0, "{}", run.stderr);
                std::fs::remove_file(dir.join("out.jpg")).unwrap();
            } else {
                refused(&run, &dir, "build coreav with the native-h264 feature");
                error(&run);
            }
            assert!(!ran(&dir, "ffmpeg"), "{args:?} {mode:?}: {}", run.stderr);
        }
        // Outside the mode the FFmpeg fallback decodes when it comes first:
        // always without the native decoder, and for --exact with it.
        let run = coreav(&dir, Mode::Off, false, &args);
        if FALLBACK && (exact || !NATIVE) {
            assert!(ran(&dir, "ffmpeg"), "{args:?}: {}", run.stderr);
            assert_ne!(run.code, 0);
        } else {
            assert_eq!(run.code, 0, "{}", run.stderr);
            assert!(!ran(&dir, "ffmpeg"));
            std::fs::remove_file(dir.join("out.jpg")).unwrap();
        }
        forget(&dir);
    }
}

#[test]
fn ffmpeg_libx264_lines_run_no_ffmpeg() {
    let dir = dir("ffmpeg-libx264");
    let Some(_) = source(&dir) else { return };
    let line = [
        "ffmpeg",
        "-i",
        "in.mp4",
        "-an",
        "-c:v",
        "libx264",
        "-fps_mode",
        "passthrough",
        "out.mp4",
    ];
    for mode in [Mode::Flag, Mode::Variable] {
        // No x264: refused, where the fallback would have run the stand-in
        // ffmpeg on PATH.
        let run = coreav(&dir, mode, false, &line);
        refused(&run, &dir, "install x264");
        assert!(run.stderr.starts_with("coreav ffmpeg: "), "{}", run.stderr);
        let run = coreav(&dir, mode, true, &line);
        if NATIVE {
            assert_eq!(run.code, 1, "{}", run.stderr);
            assert!(ran(&dir, "x264"));
        } else {
            refused(&run, &dir, "build coreav with the native-h264 feature");
            assert!(!ran(&dir, "x264"));
        }
        assert!(!ran(&dir, "ffmpeg"), "{mode:?}: {}", run.stderr);
        forget(&dir);
    }
    let run = coreav(&dir, Mode::Off, false, &line);
    assert_ne!(run.code, 0);
    assert!(ran(&dir, "ffmpeg"), "{}", run.stderr);
}

#[test]
fn ffmpeg_libx264_lines_are_refused_before_the_input_is_read() {
    let dir = dir("ffmpeg-libx264-early");
    let made = Command::new("mkfifo")
        .arg(dir.join("fifo.mp4"))
        .status()
        .unwrap();
    assert!(made.success());
    // A missing input, and a pipe no one writes to, which blocks a read.
    for input in ["missing.mp4", "fifo.mp4"] {
        let line = [
            "ffmpeg",
            "-i",
            input,
            "-an",
            "-c:v",
            "libx264",
            "-fps_mode",
            "passthrough",
            "out.mp4",
        ];
        for mode in [Mode::Flag, Mode::Variable] {
            let run = coreav_promptly(&dir, mode, &line);
            refused(&run, &dir, "install x264");
            assert!(run.stderr.starts_with("coreav ffmpeg: "), "{}", run.stderr);
            assert!(!ran(&dir, "ffmpeg") && !ran(&dir, "x264"), "{mode:?}");
        }
    }
}

#[test]
fn the_sandbox_launcher_runs_no_ffmpeg() {
    let dir = dir("exec");
    std::os::unix::fs::symlink(dir.join("bin/ffmpeg"), dir.join("encoder")).unwrap();
    // The programs by their names, and a link of another name to one.
    for (program, standin) in [
        ("bin/ffmpeg", "ffmpeg"),
        ("bin/ffprobe", "ffprobe"),
        ("encoder", "ffmpeg"),
    ] {
        let path = dir.join(program);
        let line = ["__coreav_exec", "--", path.to_str().unwrap(), "-version"];
        let run = coreav(&dir, Mode::Variable, false, &line);
        assert_eq!(run.code, 126, "{}", run.stderr);
        assert!(
            run.stderr
                .contains("strict-safe mode never runs the ffmpeg program"),
            "{}",
            run.stderr
        );
        assert!(!ran(&dir, standin), "{program}");
        let run = coreav(&dir, Mode::Off, false, &line);
        assert!(ran(&dir, standin), "{program}: {}", run.stderr);
        forget(&dir);
    }
    // The encoder the launcher is there for still runs.
    let x264 = dir.join("x264/x264");
    let line = ["__coreav_exec", "--", x264.to_str().unwrap()];
    let run = coreav(&dir, Mode::Variable, false, &line);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(ran(&dir, "x264"));
}

#[cfg(feature = "ffmpeg-fallback")]
#[test]
fn the_decode_worker_runs_no_ffmpeg() {
    use coreav_decode::{Backend, Codec, Limits, StreamConfig};
    use coreav_worker::{SandboxedBackend, WorkerBackend, WorkerCommand, WorkerOptions};
    let dir = dir("worker");
    for strict in [true, false] {
        // The worker run directly, with the variable set or not.
        let mut args: Vec<std::ffi::OsString> = Vec::new();
        if strict {
            args.push("COREAV_STRICT_SAFE=1".into());
        }
        args.extend([bin().into(), "__coreav_worker".into()]);
        let backend = SandboxedBackend {
            backend: WorkerBackend::Ffmpeg,
            options: WorkerOptions {
                ffmpeg: Some(dir.join("bin/ffmpeg")),
                ..WorkerOptions::new(WorkerCommand {
                    program: "/usr/bin/env".into(),
                    args,
                })
            },
        };
        let opened = backend.open(&StreamConfig::new(Codec::H264), &Limits::default());
        if strict {
            let Err(error) = opened else {
                panic!("the FFmpeg fallback opened in strict-safe mode")
            };
            assert!(
                error
                    .message
                    .contains("strict-safe mode never runs the ffmpeg program"),
                "{error}"
            );
            assert!(!ran(&dir, "ffmpeg"));
        } else {
            // The worker starts the stand-in once it has the stream; wait
            // for its file before the decoder ends the worker.
            let start = Instant::now();
            while !ran(&dir, "ffmpeg") && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(opened);
            assert!(ran(&dir, "ffmpeg"));
        }
    }
}

#[test]
fn an_x264_program_named_ffmpeg_is_refused_before_the_input_is_read() {
    let dir = dir("x264-named-ffmpeg");
    std::os::unix::fs::symlink(dir.join("bin/ffmpeg"), dir.join("x264/encoder")).unwrap();
    // Named by --x264, by its name or through a link of another name.
    for x264 in ["bin/ffmpeg", "x264/encoder"] {
        for mode in [Mode::Flag, Mode::Variable] {
            let run = coreav(
                &dir,
                mode,
                false,
                &[
                    "transcode",
                    "missing.mp4",
                    "out.mp4",
                    "--encoder",
                    "x264",
                    "--x264",
                    x264,
                ],
            );
            refused(&run, &dir, "so it does not run");
            error(&run);
            assert!(!ran(&dir, "ffmpeg"), "{x264} {mode:?}");
        }
    }
    // Named by COREAV_X264, for --encoder auto and a libx264 line.
    for args in [
        &["transcode", "missing.mp4", "out.mp4"][..],
        &[
            "ffmpeg",
            "-i",
            "missing.mp4",
            "-an",
            "-c:v",
            "libx264",
            "-fps_mode",
            "passthrough",
            "out.mp4",
        ][..],
    ] {
        for mode in [Mode::Flag, Mode::Variable] {
            let out = command(&dir, mode, false, args)
                .env("COREAV_X264", dir.join("bin/ffmpeg"))
                .output()
                .unwrap();
            let run = Run {
                code: out.status.code().unwrap(),
                stderr: String::from_utf8(out.stderr).unwrap(),
            };
            refused(&run, &dir, "so it does not run");
            assert!(!ran(&dir, "ffmpeg"), "{args:?} {mode:?}");
        }
    }
}

#[test]
fn every_launch_carries_the_mode_in_its_arguments() {
    use coreav_cli::strict_safe::StrictSafe;
    let dir = dir("exec-flag");
    // The arguments a run in the mode starts the launcher with, and no
    // variable: the launch clears the environment.
    let entry = StrictSafe::new(true).entry(coreav_worker::EXEC_ARG);
    for (program, standin, code) in [("bin/ffmpeg", "ffmpeg", 126), ("x264/x264", "x264", 1)] {
        let out = command(&dir, Mode::Off, false, &[])
            .args(&entry)
            .arg("--")
            .arg(dir.join(program))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(ran(&dir, standin), code == 1, "{program}");
    }
}
