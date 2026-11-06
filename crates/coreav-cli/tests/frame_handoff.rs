#![doc = "Shared decoder storage preserves the complete encoded output."]
#![cfg(all(feature = "transcode", any(feature = "dav1d", feature = "libvpx")))]

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

#[test]
fn shared_and_copied_handoffs_produce_identical_files() -> Result<(), Box<dyn std::error::Error>> {
    let ffmpeg = test_oracle::require().ffmpeg();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("frame-handoff-identity");
    std::fs::create_dir_all(&dir)?;
    let x264 = std::env::var_os("COREAV_X264").map(PathBuf::from);
    let mut cases = Vec::new();
    if cfg!(feature = "dav1d") {
        cases.extend([
            ("libaom-av1", "yuv420p", "128x96", true),
            ("libaom-av1", "yuv420p10le", "128x96", false),
            ("libaom-av1", "yuv444p", "257x129", false),
        ]);
    }
    if cfg!(feature = "libvpx") {
        cases.extend([
            ("libvpx-vp9", "yuv420p", "128x96", false),
            ("libvpx-vp9", "yuv420p10le", "128x96", false),
            ("libvpx-vp9", "yuv444p", "257x129", false),
        ]);
    }
    for (codec, format, size, grain) in cases {
        let input = dir.join(format!("{codec}-{format}.mkv"));
        let mut generate = Command::new(ffmpeg);
        generate.args([
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc=size={size}:rate=6:duration=2"),
            "-c:v",
            codec,
            "-pix_fmt",
            format,
            "-threads",
            "1",
            "-g",
            "8",
        ]);
        if codec == "libaom-av1" {
            generate.args(["-cpu-used", "8"]);
        }
        if grain {
            generate.args(["-denoise-noise-level", "50"]);
        }
        let generated = generate.arg("-y").arg(&input).output()?;
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let mut encoders = vec!["ffmpeg"];
        if x264.as_ref().is_some_and(|p| p.is_file()) {
            encoders.push("x264");
        }
        for encoder in encoders {
            let mut copied = None;
            for mode in ["1", "0"] {
                let output = dir.join("output.mp4");
                let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
                command
                    .env_remove("COREAV_STRICT_SAFE")
                    .env("COREAV_DIAG_TRANSCODE_IN_PROCESS", mode)
                    .arg("transcode")
                    .arg(&input)
                    .arg(&output)
                    .args(["--encoder", encoder, "--ffmpeg"])
                    .arg(ffmpeg)
                    .args(["--preset", "medium", "--crf", "23", "--force"]);
                if let Some(x264) = &x264 {
                    command.arg("--x264").arg(x264);
                }
                let result = command.output()?;
                assert!(
                    result.status.success(),
                    "{codec}/{format}/{encoder}/{mode}: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                let receipt: serde_json::Value = serde_json::from_slice(&result.stdout)?;
                assert_eq!(
                    receipt.pointer("/video/frames").and_then(|n| n.as_u64()),
                    Some(12)
                );
                let bytes = std::fs::read(&output)?;
                if let Some(expected) = &copied {
                    assert!(
                        expected == &bytes,
                        "complete file differs for {codec}/{format}/{encoder}"
                    );
                } else {
                    copied = Some(bytes);
                }
            }
        }
    }
    Ok(())
}
