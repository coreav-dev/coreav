//! Structured stdin probing and complete pinned replay on generated inputs.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn structured_stdin_and_complete_pinned_replay() {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    assert_eq!(ffmpeg.file_name(), Some(std::ffi::OsStr::new("ffmpeg")));
    if let Some(ffprobe) = std::env::var_os("COREAV_FFPROBE") {
        assert_eq!(
            PathBuf::from(ffprobe).canonicalize().unwrap(),
            ffmpeg
                .parent()
                .unwrap()
                .join("ffprobe")
                .canonicalize()
                .unwrap()
        );
    }
    let result = Command::new("python3")
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ffprobe_stdin_check.py"))
        .args(["--coreav", env!("CARGO_BIN_EXE_coreav"), "--oracle-dir"])
        .arg(ffmpeg.parent().unwrap())
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
