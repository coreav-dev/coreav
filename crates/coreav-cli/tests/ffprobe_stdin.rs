//! Structured stdin probing and complete pinned replay on generated inputs.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
#![allow(clippy::unwrap_used, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::Path;
use std::process::Command;

#[test]
fn structured_stdin_and_complete_pinned_replay() {
    let oracle = test_oracle::require();
    let result = Command::new("python3")
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ffprobe_stdin_check.py"))
        .args(["--coreav", env!("CARGO_BIN_EXE_coreav"), "--oracle-dir"])
        .arg(oracle.ffmpeg().parent().unwrap())
        .arg("--directory")
        .arg(env!("CARGO_TARGET_TMPDIR"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
