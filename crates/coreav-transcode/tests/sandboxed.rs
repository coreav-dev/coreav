//! The encoder through the sandbox launcher (`docs/design/transcode.md`,
//! "Sandbox", and bar B6's hostile encoders). This test program is its own
//! launcher: run with `EXEC_ARG` first, it sets the limits and becomes the
//! encoder, as `coreav` does. Without it, it runs the cases below in turn
//! (no test harness: libtest would take the launcher's arguments for its
//! own).
//!
//! Hostile encoders are small `/bin/sh` scripts given as the `x264`
//! program: each must end in an error within the deadline, never a panic,
//! a hang or a deadlock.
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use coreav_decode::{Error, ErrorKind};
use coreav_frame::{PixelFormat, SharedMemory, SharedPicture};
use coreav_transcode::encoder::{Encoder, EncoderLimits, EncoderProgram, KEPT_STDERR};
use coreav_worker::ExecLauncher;

use common::*;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new(coreav_worker::EXEC_ARG)) {
        return coreav_worker::sandbox_exec_main(args, |_| Ok(()));
    }
    if !cfg!(unix) {
        eprintln!("skipping: the launcher needs Unix");
        return ExitCode::SUCCESS;
    }
    let cases: [(&str, fn()); 14] = [
        ("real encoders through the launcher", real_encoders),
        ("an encoder that exits early with an error", exits_early),
        (
            "a stopped encoder gives back its queued frames",
            returns_queued_frames,
        ),
        ("an encoder that floods its output", floods_output),
        ("an encoder that never reads its input", never_reads),
        ("an encoder that hangs after its input", hangs),
        ("an encoder that floods its standard error", floods_stderr),
        (
            "an encoder that exits 0 without reading",
            exits_without_reading,
        ),
        ("an encoder whose child keeps its pipes", child_keeps_pipes),
        ("an encoder that writes a file", writes_a_file),
        ("an encoder that is not there", missing_program),
        ("dropping an encoder returns at once", drop_returns_at_once),
        (
            "an encoder that fails while its child holds its input",
            fails_with_child_on_input,
        ),
        (
            "an encoder that exits 0 while its child holds its input",
            quits_with_child_on_input,
        ),
    ];
    // `cargo test -- --list` and tools that list tests first: nothing to
    // list in libtest's format, and nothing must run.
    if std::env::args().any(|a| a == "--list") {
        return ExitCode::SUCCESS;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    for (name, case) in cases {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        let start = Instant::now();
        case();
        println!(
            "test {name} ... ok ({:.2} s)",
            start.elapsed().as_secs_f64()
        );
    }
    ExitCode::SUCCESS
}

fn launcher() -> ExecLauncher {
    ExecLauncher::current_exe().unwrap()
}

fn real_encoders() {
    let dir = scratch("sandboxed");
    let launcher = launcher();
    for program in programs() {
        for threads in [None, Some(8)] {
            let format = PixelFormat::YUV420P;
            let frames: Vec<_> = (0..30).map(|n| moving(320, 240, format, n)).collect();
            let s = coreav_transcode::encoder::EncoderSettings {
                threads,
                ..settings(320, 240, format)
            };
            let stream = encode(&program, &s, Some(&launcher), &frames).unwrap();
            let lines = probe(&stream, &dir, program.name());
            assert_eq!(field(&lines, "nb_read_frames"), "30", "{program:?}");
            assert_eq!(field(&lines, "width"), "320");
            assert_eq!(field(&lines, "profile"), "High");
        }
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A script that stands in for the encoder. A child process writes it, so
/// this process never holds it open for writing: a child forked meanwhile
/// would inherit that descriptor until its exec, and running the script
/// then fails with ETXTBSY.
fn script(name: &str, body: &str) -> PathBuf {
    use std::io::Write;
    let dir = scratch("hostile");
    let path = dir.join(format!("{name}.sh"));
    let mut sh = std::process::Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&path)
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin
        .take()
        .unwrap()
        .write_all(format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{body}\n").as_bytes())
        .unwrap();
    assert!(sh.wait().unwrap().success());
    path
}

const DEADLINE: Duration = Duration::from_secs(3);
/// What "at once" means on a loaded CI runner: the keeper's polls and the
/// failure's settling time, with room for scheduling.
const WELL_BEFORE_THE_DEADLINE: Duration = Duration::from_millis(1500);
/// How far past the deadline an error may come: the keeper's report and
/// the last messages.
const SLACK: Duration = Duration::from_secs(3);

/// Runs `program` on `frames` frames of 640x480 (460,800 bytes, far more
/// than a pipe holds) with a sink of `limit` bytes, through the launcher.
/// Returns the first error and when it came.
fn run(program: &Path, frames: usize, limit: usize) -> (Result<usize, Error>, Duration) {
    let start = Instant::now();
    let format = PixelFormat::YUV420P;
    let s = settings(640, 480, format);
    let frame = moving(640, 480, format, 0);
    let launcher = launcher();
    let result = (|| {
        let mut encoder = Encoder::spawn(
            &EncoderProgram::X264(program.to_path_buf()),
            &s,
            Some(&launcher),
            &EncoderLimits::new(1 << 30),
            start + DEADLINE,
            Collect::limited(limit),
        )?;
        for _ in 0..frames {
            encoder.send_frame(frame.clone())?;
        }
        encoder.finish().map(|sink| sink.bytes.len())
    })();
    let took = start.elapsed();
    assert!(took < DEADLINE + SLACK, "took {took:?}: {result:?}");
    (result, took)
}

fn exits_early() {
    let p = script(
        "early",
        "echo 'x264 [error]: something is wrong' >&2; exit 3",
    );
    let (result, _) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Backend, "{e:?}");
    assert!(e.message.contains("something is wrong"), "{e:?}");
    assert!(e.message.contains("exit status: 3"), "{e:?}");
}

struct LentFrame(Vec<u8>);

impl SharedMemory for LentFrame {
    fn read_packed(&self, at: usize, out: &mut [u8]) -> Option<usize> {
        let rest = self.0.get(at..)?;
        let n = rest.len().min(out.len());
        out.get_mut(..n)?.copy_from_slice(rest.get(..n)?);
        Some(n)
    }
}

fn returns_queued_frames() {
    let dir = scratch("return-queued-frames");
    let go = dir.join("go");
    let p = script(
        "return-queued-frames",
        &format!(
            "while [ ! -e '{}' ]; do sleep 0.01; done\nhead -c 1000 > /dev/null\nexit 0",
            go.display()
        ),
    );
    let format = PixelFormat::YUV420P;
    let s = settings(640, 480, format);
    let mut packed = Vec::new();
    moving(s.width, s.height, format, 0)
        .write_packed(&mut packed)
        .unwrap();
    let mut encoder = Encoder::spawn(
        &EncoderProgram::X264(p),
        &s,
        Some(&launcher()),
        &EncoderLimits::new(1 << 30),
        Instant::now() + Duration::from_secs(60),
        Collect::new(),
    )
    .unwrap();
    let leases: Vec<_> = (0..3)
        .map(|_| {
            let memory = Arc::new(LentFrame(packed.clone()));
            let lease = Arc::downgrade(&memory);
            encoder
                .send_frame(SharedPicture {
                    width: s.width,
                    height: s.height,
                    format,
                    memory,
                })
                .unwrap();
            lease
        })
        .collect();
    assert!(leases.iter().all(|lease| lease.upgrade().is_some()));
    std::fs::write(&go, []).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while leases.iter().any(|lease| lease.upgrade().is_some()) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        leases.iter().all(|lease| lease.upgrade().is_none()),
        "the encoder retained decoder frames after its early exit"
    );
    drop(encoder);
    std::fs::remove_dir_all(dir).unwrap();
}

fn floods_output() {
    let p = script("flood", "exec cat /dev/zero");
    let (result, took) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    // The sink's own error, at once, not at the deadline.
    assert_eq!(e.kind, ErrorKind::Limit, "{e:?}");
    assert!(e.message.contains("over the test's limit"), "{e:?}");
    assert!(took < DEADLINE, "{took:?}");
}

fn never_reads() {
    let p = script("deaf", "exec sleep 60");
    let (result, took) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Timeout, "{e:?}");
    assert!(took >= DEADLINE, "{took:?}");
}

fn hangs() {
    let p = script("hang", "cat > /dev/null; exec sleep 60");
    let (result, took) = run(&p, 5, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Timeout, "{e:?}");
    assert!(took >= DEADLINE, "{took:?}");
}

fn floods_stderr() {
    // 8 MB of messages while it reads its input: neither pipe may stall.
    let p = script(
        "noisy",
        "head -c 8000000 /dev/zero | tr '\\0' e >&2; cat > /dev/null; echo last words >&2; exit 2",
    );
    let (result, took) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Backend, "{e:?}");
    assert!(
        e.message.ends_with("last words"),
        "{}",
        &e.message[e.message.len() - 100..]
    );
    assert!(e.message.len() <= KEPT_STDERR + 200, "{}", e.message.len());
    assert!(took < DEADLINE, "{took:?}");
}

fn exits_without_reading() {
    let p = script("quitter", "exit 0");
    let (result, took) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Backend, "{e:?}");
    assert!(took < DEADLINE, "{took:?}");
}

/// The encoder exits with an error while a child of it keeps its input
/// open without reading: the session ends at once with the encoder's
/// status, not at the deadline (the keeper watches for the exit all along,
/// and kills the group). The input goes to the child through descriptor 3:
/// a shell gives a background command `/dev/null` as its input before its
/// own redirections (POSIX; dash does, bash keeps `<&0`). The variant
/// without a child is `exits_early`.
fn fails_with_child_on_input() {
    if !sees_the_exit() {
        return;
    }
    let p = script("boom", "echo boom >&2; exec 3<&0; sleep 60 <&3 & exit 3");
    let (result, took) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Backend, "{e:?}");
    assert!(e.message.contains("exit status: 3"), "{e:?}");
    assert!(e.message.contains("boom"), "{e:?}");
    assert!(took < WELL_BEFORE_THE_DEADLINE, "{took:?}");
}

/// The same with a successful exit: the frames it never read make it an
/// error, also at once.
fn quits_with_child_on_input() {
    if !sees_the_exit() {
        return;
    }
    let p = script("quit", "exec 3<&0; sleep 60 <&3 & exit 0");
    let (result, took) = run(&p, 30, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Backend, "{e:?}");
    assert!(took < WELL_BEFORE_THE_DEADLINE, "{took:?}");
}

/// Whether the encoder's exit is seen while a child holds its pipes, so the
/// cases that call this end at once: only Linux's `waitid` with `WNOWAIT`
/// sees it; elsewhere the encode ends at its deadline
/// (`docs/design/transcode.md`, "Sandbox"). Says so when it is not.
fn sees_the_exit() -> bool {
    if cfg!(target_os = "linux") {
        return true;
    }
    eprintln!("skipping: only Linux sees the encoder exit while its child holds its pipes");
    false
}

fn child_keeps_pipes() {
    if !sees_the_exit() {
        return;
    }
    // The encoder exits, its child (same process group) keeps the pipes
    // open: the group is killed when the encoder exits, so the output ends
    // without waiting for the deadline.
    let p = script("orphan", "cat > /dev/null; sleep 60 & exit 0");
    let (result, took) = run(&p, 3, 1 << 20);
    assert_eq!(result.unwrap(), 0);
    assert!(took < DEADLINE, "{took:?}");
}

fn writes_a_file() {
    let dir = scratch("fsize");
    let file = dir.join("leak");
    let p = script(
        "writer",
        &format!(
            "cat > /dev/null; printf x > '{}' || exit 9; exit 0",
            file.display()
        ),
    );
    let (result, _) = run(&p, 3, 1 << 20);
    assert!(result.is_err(), "{result:?}");
    let written = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    assert_eq!(written, 0);
}

fn missing_program() {
    let (result, took) = run(Path::new("/nonexistent/x264"), 3, 1 << 20);
    let e = result.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Backend, "{e:?}");
    assert!(e.message.contains("cannot run /nonexistent/x264"), "{e:?}");
    assert!(took < DEADLINE, "{took:?}");
}

fn drop_returns_at_once() {
    let p = script("sleeper", "exec sleep 60");
    let format = PixelFormat::YUV420P;
    let mut encoder = Encoder::spawn(
        &EncoderProgram::X264(p),
        &settings(640, 480, format),
        Some(&launcher()),
        &EncoderLimits::new(1 << 30),
        Instant::now() + Duration::from_secs(60),
        Collect::new(),
    )
    .unwrap();
    let frame = moving(640, 480, format, 0);
    // Two frames fit the queue; the writer is blocked on the pipe.
    encoder.send_frame(frame.clone()).unwrap();
    encoder.send_frame(frame).unwrap();
    let start = Instant::now();
    drop(encoder);
    assert!(
        start.elapsed() < Duration::from_millis(200),
        "{:?}",
        start.elapsed()
    );
}
