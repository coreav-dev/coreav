//! The hidden sandbox-launcher entry (`__coreav_exec`, `coreav_worker::exec`)
//! is in every build with a decoder backend (`docs/design/decode.md`,
//! "Exec"): the encoder of `coreav transcode` runs through it.
#![cfg(all(feature = "thumbnail", unix))]
#![allow(clippy::unwrap_used)]

use std::process::Command;

#[test]
fn the_launcher_runs_a_program_under_its_limits() {
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "__coreav_exec",
            "--open-files",
            "32",
            "--",
            "/bin/sh",
            "-c",
            "exit 0",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    // The program's own status comes back.
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["__coreav_exec", "--", "/bin/sh", "-c", "exit 7"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    // A launcher that cannot start the program says so.
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["__coreav_exec", "--", "/nonexistent/x264"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127), "{out:?}");
}
