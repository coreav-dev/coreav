#![allow(dead_code, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub struct Oracle {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl Oracle {
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }
}

pub fn require() -> &'static Oracle {
    static ORACLE: OnceLock<Result<Oracle, String>> = OnceLock::new();
    let result = ORACLE.get_or_init(|| {
        let result = Command::new("python3")
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test_oracle.py"))
            .arg("--resolve")
            .output()
            .map_err(|error| format!("cannot start oracle verifier: {error}"))?;
        if !result.status.success() {
            return Err(String::from_utf8_lossy(&result.stderr).into_owned());
        }
        let paths = String::from_utf8(result.stdout)
            .map_err(|error| format!("invalid oracle paths: {error}"))?;
        let mut paths = paths.lines();
        let ffmpeg = paths.next().ok_or("missing verified FFmpeg path")?;
        let ffprobe = paths.next().ok_or("missing verified ffprobe path")?;
        if ffmpeg.is_empty() || ffprobe.is_empty() || paths.next().is_some() {
            return Err("invalid verified oracle pair".into());
        }
        Ok(Oracle {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
        })
    });
    match result {
        Ok(oracle) => oracle,
        Err(error) => panic!("stage=oracle_qualification: {error}"),
    }
}
