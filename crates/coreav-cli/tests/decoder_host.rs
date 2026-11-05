#![doc = "Synthetic transcodes preserve complete pixels across diagnostic decoder hosts."]
#![cfg(feature = "transcode")]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::{Path, PathBuf};
use std::process::Command;

const SWITCH: &str = "COREAV_DIAG_TRANSCODE_IN_PROCESS";

fn directory(name: &str) -> std::io::Result<PathBuf> {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("decoder-host-{name}"));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

#[test]
fn diagnostic_errors_precede_media_probing() -> Result<(), Box<dyn std::error::Error>> {
    let dir = directory("refusal")?;
    let input = dir.join("invalid.mp4");
    std::fs::write(&input, b"not a media file")?;
    for (value, strict) in [("invalid", false), ("1", true)] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
        command.env_remove("COREAV_STRICT_SAFE").env(SWITCH, value);
        if strict {
            command.arg("--strict-safe");
        }
        command
            .arg("transcode")
            .arg(&input)
            .arg(dir.join("out.mp4"))
            .args(["--encoder", "x264", "--x264", "/nonexistent/coreav-x264"]);
        let output = command.output()?;
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .ok_or("missing error message")?;
        assert!(message.contains(if strict && cfg!(target_os = "linux") {
            "in-process decoder diagnostics are unavailable in strict-safe mode"
        } else if strict {
            "no enforceable memory limit"
        } else {
            SWITCH
        }));
        assert!(!dir.join("out.mp4").exists());
    }
    Ok(())
}

#[cfg(any(feature = "libvpx", feature = "dav1d"))]
fn hosting_pair(
    codec: &str,
    encoder: &str,
    backend: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ffmpeg = test_oracle::require().ffmpeg().to_path_buf();
    let dir = directory(codec)?;
    let input = dir.join("input.mkv");
    let generated = Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=5:duration=1",
            "-c:v",
            encoder,
            "-threads",
            "1",
            "-y",
        ])
        .arg(&input)
        .output()?;
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let mut identities = Vec::new();
    for mode in [None, Some("0"), Some("1")] {
        let output = dir.join("output.mp4");
        let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
        command.env_remove(SWITCH).env_remove("COREAV_STRICT_SAFE");
        if let Some(mode) = mode {
            command.env(SWITCH, mode);
        }
        let result = command
            .arg("transcode")
            .arg(&input)
            .arg(&output)
            .args(["--encoder", "ffmpeg", "--ffmpeg"])
            .arg(&ffmpeg)
            .args(["--preset", "medium", "--crf", "23", "--force"])
            .output()?;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&result.stdout)?;
        let video = document.get("video").ok_or("missing video")?;
        assert_eq!(video.get("backend").and_then(|v| v.as_str()), Some(backend));
        assert_eq!(video.get("frames").and_then(|v| v.as_u64()), Some(5));
        let stderr = String::from_utf8_lossy(&result.stderr);
        match mode {
            None => assert!(!stderr.contains("coreav-diagnostic-decoder-host")),
            Some("0") => assert!(stderr.contains("coreav-diagnostic-decoder-host: sandbox")),
            _ => assert!(stderr.contains("coreav-diagnostic-decoder-host: in-process")),
        }
        let decoded = Command::new(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(&output)
            .args(["-map", "0:v:0", "-f", "framemd5", "-"])
            .output()?;
        assert!(decoded.status.success());
        identities.push(decoded.stdout);
    }
    assert!(
        identities
            .windows(2)
            .all(|pair| pair.first() == pair.get(1))
    );
    Ok(())
}

#[test]
#[cfg(feature = "libvpx")]
fn vp9_hosting_preserves_complete_pixels() -> Result<(), Box<dyn std::error::Error>> {
    hosting_pair("vp9", "libvpx-vp9", "libvpx")
}

#[test]
#[cfg(feature = "dav1d")]
fn av1_hosting_preserves_complete_pixels() -> Result<(), Box<dyn std::error::Error>> {
    hosting_pair("av1", "libaom-av1", "dav1d")
}
