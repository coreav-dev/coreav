//! Private AV1 dispatch, parser-only refusal and sandbox reexecution.

#![cfg(all(feature = "svt-av1", feature = "cli"))]
#![allow(clippy::unwrap_used)]

use std::process::{Command, Stdio};

const ENTRY: &str = coreav_transcode::av1::WORKER_ARG;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command
        .env_remove("COREAV_STRICT_SAFE")
        .stdin(Stdio::null());
    command
}

#[test]
fn private_entry_rejects_unexpected_arguments_before_worker_io() {
    for args in [
        vec![ENTRY, "unexpected"],
        vec![ENTRY, "--strict-safe", "unexpected"],
    ] {
        let output = command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"invalid private AV1 worker arguments\n");
    }
}

#[test]
#[cfg(not(feature = "svt-av1-native"))]
fn parser_only_entry_is_unavailable_without_emitting_worker_output() {
    let output = command().arg(ENTRY).output().unwrap();
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("no native SVT-AV1 backend")
    );
}

#[test]
#[cfg(not(feature = "svt-av1-native"))]
fn private_entry_honors_strict_safe_flag_and_environment() {
    for flag in [false, true] {
        let mut command = command();
        command.arg(ENTRY);
        if flag {
            command.arg("--strict-safe");
        } else {
            command.env("COREAV_STRICT_SAFE", "1");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert!(output.stdout.is_empty());
        let message = String::from_utf8(output.stderr).unwrap();
        if coreav_worker::enforces_memory_limit() {
            assert!(message.contains("no native SVT-AV1 backend"));
        } else {
            assert!(message.contains("no enforceable memory limit"));
        }
    }
}

#[test]
#[cfg(unix)]
fn existing_sandbox_launcher_reexecutes_the_private_entry() {
    let output = command()
        .args([
            coreav_worker::EXEC_ARG,
            "--open-files",
            "32",
            "--",
            env!("CARGO_BIN_EXE_coreav"),
            ENTRY,
            "unexpected",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"invalid private AV1 worker arguments\n");
}
