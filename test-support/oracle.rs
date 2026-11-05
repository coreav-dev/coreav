#![allow(dead_code, clippy::panic)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

struct Support(PathBuf);

impl Drop for Support {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
    match try_require() {
        Ok(oracle) => oracle,
        Err(error) => panic!("{error}"),
    }
}

pub fn try_require() -> Result<&'static Oracle, String> {
    static ORACLE: OnceLock<Result<Oracle, String>> = OnceLock::new();
    let result = ORACLE.get_or_init(|| {
        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            return Err("no registered oracle for this platform".into());
        }
        let mut nonce = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut nonce))
            .map_err(|error| format!("cannot create oracle support identity: {error}"))?;
        let directory = std::env::temp_dir().join(format!(
            "coreav-oracle-{}-{:032x}",
            std::process::id(),
            u128::from_ne_bytes(nonce)
        ));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        builder
            .create(&directory)
            .map_err(|error| format!("cannot create oracle support directory: {error}"))?;
        let support = Support(directory);
        let scripts = support.0.join("scripts");
        std::fs::create_dir(&scripts)
            .map_err(|error| format!("cannot create oracle scripts directory: {error}"))?;
        for (name, source) in [
            ("test_oracle.py", include_str!("../scripts/test_oracle.py")),
            ("oracle_pin.py", include_str!("../scripts/oracle_pin.py")),
            (
                "oracle_pin_macos.py",
                include_str!("../scripts/oracle_pin_macos.py"),
            ),
            (
                "oracle_audio.py",
                include_str!("../scripts/oracle_audio.py"),
            ),
            (
                "oracle-ffmpeg.env",
                include_str!("../scripts/oracle-ffmpeg.env"),
            ),
            (
                "oracle-ffmpeg-macos.env",
                include_str!("../scripts/oracle-ffmpeg-macos.env"),
            ),
            (
                "oracle-audio-args.txt",
                include_str!("../scripts/oracle-audio-args.txt"),
            ),
        ] {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(scripts.join(name))
                .and_then(|mut file| file.write_all(source.as_bytes()))
                .map_err(|error| format!("cannot stage oracle support file {name}: {error}"))?;
        }
        let result = Command::new("python3")
            .arg("-I")
            .arg("-B")
            .arg(scripts.join("test_oracle.py"))
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
        Ok(oracle) => Ok(oracle),
        Err(error) => Err(format!("stage=oracle_qualification: {error}")),
    }
}
