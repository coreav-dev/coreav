//! Oracle-free process-cache contracts using Linux banner stand-ins.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pair(root: &Path, version: &str) {
    for name in ["ffmpeg", "ffprobe"] {
        let binary = root.join(name);
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nprintf '%s\\n' {name} >> '{}'\nprintf '%s\\n' '{name} version {version}'\n",
                root.join("calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}

fn build() -> &'static str {
    include_str!("../../../scripts/oracle-ffmpeg.env")
        .lines()
        .find_map(|line| line.strip_prefix("BUILD="))
        .unwrap()
}

#[test]
#[ignore = "oracle-free child process of the cache contract"]
fn cache_child() {
    let mode = std::env::var("COREAV_ORACLE_CACHE_CHILD").unwrap();
    if mode == "success" {
        let threads: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| test_oracle::require() as *const _ as usize))
            .collect();
        let addresses: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(
            addresses
                .iter()
                .all(|address| Some(address) == addresses.first())
        );
    } else {
        assert!(std::panic::catch_unwind(test_oracle::require).is_err());
        let root = PathBuf::from(std::env::var_os("COREAV_FFMPEG").unwrap());
        pair(root.parent().unwrap(), build());
        assert!(std::panic::catch_unwind(test_oracle::require).is_err());
    }
}

#[test]
fn rust_owner_caches_success_and_failure_once_per_process() {
    let root =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("oracle-cache-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let owner = Fixture(root);
    for (version, mode) in [
        (build(), "success"),
        ("0.0.0", "failure"),
        (build(), "success"),
    ] {
        pair(&owner.0, version);
        let _ = std::fs::remove_file(owner.0.join("calls"));
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "cache_child"])
            .env("COREAV_ORACLE_CACHE_CHILD", mode)
            .env("COREAV_FFMPEG", owner.0.join("ffmpeg"))
            .env("COREAV_FFPROBE", owner.0.join("ffprobe"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(owner.0.join("calls"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}
