//! The sandbox launcher through the `coreav-worker` binary: the limits are
//! in force in the program it runs, the environment is empty, and a program
//! that cannot be run fails with a status and a message, never a panic.
#![cfg(unix)]
// Test-only sources outside `#[test]` functions.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;

use coreav_worker::{ExecLauncher, HugePages, SandboxLimits, WorkerCommand};

fn launcher() -> ExecLauncher {
    ExecLauncher {
        command: WorkerCommand {
            program: PathBuf::from(env!("CARGO_BIN_EXE_coreav-worker")),
            args: vec![coreav_worker::EXEC_ARG.into()],
        },
    }
}

fn run(limits: &SandboxLimits, program: &str, args: &[&str]) -> Output {
    run_with(limits, HugePages::Kept, program, args)
}

fn run_with(limits: &SandboxLimits, huge_pages: HugePages, program: &str, args: &[&str]) -> Output {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let mut command = launcher().command(limits, huge_pages, Path::new(program), &args);
    // The test feeds nothing on standard input.
    command.stdin(std::process::Stdio::null());
    command.output().expect("the launcher starts")
}

#[test]
fn limits_are_in_force_in_the_program() {
    let limits = SandboxLimits {
        address_space: Some(3 << 30),
        cpu_seconds: Some(77),
        open_files: Some(48),
    };
    let out = run(
        &limits,
        "/bin/sh",
        &[
            "-c",
            "ulimit -f; ulimit -v; ulimit -t; ulimit -n; ulimit -c; ulimit -H -v",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // `ulimit -v` counts KiB; the hard limit is the soft one.
    let kib = (3u64 << 30) / 1024;
    assert_eq!(
        lines,
        ["0", &kib.to_string(), "77", "48", "0", &kib.to_string()],
        "{text}"
    );
}

#[test]
fn a_regular_file_cannot_be_written() {
    let dir = std::env::temp_dir().join(format!("coreav-exec-fsize-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("out");
    let script = format!("printf x > '{}'; echo $?", file.display());
    let out = run(
        &SandboxLimits {
            address_space: None,
            cpu_seconds: None,
            open_files: None,
        },
        "/bin/sh",
        &["-c", &script],
    );
    // The write fails (SIGXFSZ ends the `printf` builtin's shell, or the
    // write returns EFBIG): either way no byte reaches the file.
    let written = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    assert_eq!(written, 0, "{out:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn huge_pages_are_off_in_the_program_when_asked() {
    let none = SandboxLimits {
        address_space: None,
        cpu_seconds: None,
        open_files: None,
    };
    let thp = |huge_pages| {
        let out = run_with(
            &none,
            huge_pages,
            "/bin/grep",
            &["^THP_enabled:", "/proc/self/status"],
        );
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout)
            .unwrap()
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
    };
    let inherited = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("THP_enabled:"))
        .map(|value| value.trim().to_owned());
    assert_eq!(thp(HugePages::Off).as_deref(), Some("0"));
    assert_eq!(thp(HugePages::Kept), inherited);
}

#[test]
fn the_environment_is_empty() {
    // The test's own environment (PATH, CARGO_*, ...) must not reach the
    // program.
    assert!(std::env::var_os("PATH").is_some());
    let out = run(
        &SandboxLimits {
            address_space: None,
            cpu_seconds: None,
            open_files: Some(32),
        },
        "/usr/bin/env",
        &[],
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn a_missing_program_fails_cleanly() {
    let none = SandboxLimits {
        address_space: None,
        cpu_seconds: None,
        open_files: None,
    };
    let out = run(&none, "/nonexistent/coreav-no-such-encoder", &["-x"]);
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    let message = String::from_utf8_lossy(&out.stderr);
    assert!(
        message.contains("coreav exec: cannot run /nonexistent/coreav-no-such-encoder"),
        "{message}"
    );
    // A directory exists but is not a program.
    let out = run(&none, "/", &[]);
    assert_eq!(out.status.code(), Some(126), "{out:?}");
    // A name to search for is refused: there is no PATH.
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_coreav-worker"));
    let out = command
        .args([coreav_worker::EXEC_ARG, "--", "sh", "-c", "true"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(125), "{out:?}");
    // Bad limits.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_coreav-worker"))
        .args([
            coreav_worker::EXEC_ARG,
            "--cpu-seconds",
            "many",
            "--",
            "/bin/true",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(125), "{out:?}");
}

/// The program replaces the launcher and leads the process group the
/// launcher was started in (`/proc/<pid>/stat`, field 5, Linux).
#[cfg(target_os = "linux")]
#[test]
fn the_program_leads_its_own_process_group() {
    let out = run(
        &SandboxLimits {
            address_space: None,
            cpu_seconds: None,
            open_files: None,
        },
        "/bin/sh",
        &["-c", "read -r a b c d e rest < /proc/$$/stat; echo $$ $e"],
    );
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    let fields: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(fields.len(), 2, "{text}");
    assert_eq!(fields[0], fields[1], "pid and process group: {text}");
    assert_ne!(
        fields[1],
        std::process::id().to_string(),
        "not the test's group"
    );
}
