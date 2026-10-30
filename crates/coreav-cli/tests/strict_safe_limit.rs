//! Strict-safe mode starts no sandboxed encoder where the sandbox's memory
//! limit is not enforced (`coreav_worker::enforces_memory_limit`: macOS,
//! and outside Unix; `docs/design/compat-cli.md`, "Strict-safe mode"). On
//! every platform, Windows included: `strict_safe.rs` needs Unix for its
//! stand-in programs.
#![cfg(feature = "transcode")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

const NO_LIMIT: &str = "strict-safe: no enforceable memory limit for the sandbox on this platform";

#[test]
fn the_encoder_is_refused_before_the_input_is_read_without_an_enforced_limit() {
    for mode in ["--strict-safe", "COREAV_STRICT_SAFE"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
        command
            .current_dir(env!("CARGO_TARGET_TMPDIR"))
            .env_remove("COREAV_X264")
            .env_remove("COREAV_FFMPEG")
            .env_remove("COREAV_STRICT_SAFE");
        if mode == "COREAV_STRICT_SAFE" {
            command.env(mode, "1");
        } else {
            command.arg(mode);
        }
        let out = command
            .args(["transcode", "missing.mp4", "out.mp4", "--encoder", "x264"])
            .output()
            .unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        if coreav_worker::enforces_memory_limit() {
            assert!(!stderr.contains(NO_LIMIT), "{mode}: {stderr}");
            continue;
        }
        // Refused before the input is read: the input is missing.
        assert_eq!(out.status.code(), Some(4), "{mode}: {stderr}");
        assert!(stderr.contains(NO_LIMIT), "{mode}: {stderr}");
        assert!(stderr.contains("unsupported_feature"), "{mode}: {stderr}");
    }
}
