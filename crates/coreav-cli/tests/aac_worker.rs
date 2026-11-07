//! Actual AAC worker starts and generated output checked against the pinned oracle.
#![cfg(all(unix, feature = "transcode"))]
#![allow(clippy::unwrap_used, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

#[test]
fn aac_worker_starts_and_output_pass_the_frozen_checks() {
    let oracle = test_oracle::require();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("aac-worker-protocol");
    if work.exists() {
        std::fs::remove_dir_all(&work).unwrap();
    }
    let result = std::process::Command::new("python3")
        .arg(root.join("scripts/aac_worker_check.py"))
        .args(["--coreav", env!("CARGO_BIN_EXE_coreav"), "--work"])
        .arg(&work)
        .env("COREAV_FFMPEG", oracle.ffmpeg())
        .env("COREAV_FFPROBE", oracle.ffprobe())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    print!("{}", String::from_utf8_lossy(&result.stdout));
    std::fs::remove_dir_all(work).unwrap();
}
