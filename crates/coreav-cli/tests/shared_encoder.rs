#![doc = "The operator-only helper preserves complete transcode output."]
#![cfg(all(
    target_os = "linux",
    feature = "transcode",
    any(feature = "dav1d", feature = "libvpx")
))]

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn sealed_snapshots_match_the_pipe_encoder_complete_files() -> Result<(), Box<dyn std::error::Error>>
{
    let Some(helper) = std::env::var_os("COREAV_TEST_X264_SHM_HELPER").map(PathBuf::from) else {
        assert!(
            std::env::var_os("COREAV_REQUIRE_X264_HELPER").is_none(),
            "missing operator helper fixture"
        );
        eprintln!("skipping without the separately built helper fixture");
        return Ok(());
    };
    let ffmpeg = PathBuf::from(std::env::var_os("COREAV_FFMPEG").ok_or("missing pinned FFmpeg")?);
    let x264 = PathBuf::from(std::env::var_os("COREAV_X264").ok_or("missing pinned x264")?);
    assert!(helper.is_file() && ffmpeg.is_file() && x264.is_file());
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shared-encoder-identity");
    std::fs::create_dir_all(&directory)?;
    let mut cases = Vec::new();
    if cfg!(feature = "dav1d") {
        cases.extend([
            ("libaom-av1", "yuv420p"),
            ("libaom-av1", "yuv420p10le"),
            ("libaom-av1", "yuv444p"),
        ]);
    }
    if cfg!(feature = "libvpx") {
        cases.extend([
            ("libvpx-vp9", "yuv420p"),
            ("libvpx-vp9", "yuv420p10le"),
            ("libvpx-vp9", "yuv444p"),
        ]);
    }
    for (codec, format) in cases {
        let source = directory.join(format!("{codec}-{format}.mkv"));
        let generated = Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=128x96:rate=6:duration=2",
                "-c:v",
                codec,
                "-pix_fmt",
                format,
                "-threads",
                "1",
                "-g",
                "8",
                "-y",
            ])
            .arg(&source)
            .output()?;
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let mut expected = None;
        for shared in [false, true] {
            let output = directory.join("output.mp4");
            let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
            command
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_X264_SHM_HELPER")
                .env("COREAV_X264", &x264)
                .env("COREAV_DIAG_TRANSCODE_IN_PROCESS", "0")
                .arg("transcode")
                .arg(&source)
                .arg(&output)
                .args(["--encoder", "x264", "--ffmpeg"])
                .arg(&ffmpeg)
                .args(["--preset", "medium", "--crf", "23", "--force"]);
            if shared {
                command.env("COREAV_X264_SHM_HELPER", &helper);
            } else {
                command.arg("--x264").arg(&x264);
            }
            let encoded = command.output()?;
            assert!(
                encoded.status.success(),
                "{codec}/{format}/{shared}: {}",
                String::from_utf8_lossy(&encoded.stderr)
            );
            let receipt: serde_json::Value = serde_json::from_slice(&encoded.stdout)?;
            assert_eq!(
                receipt.pointer("/video/frames").and_then(|v| v.as_u64()),
                Some(12)
            );
            assert_eq!(
                receipt.pointer("/video/encoder").and_then(|v| v.as_str()),
                Some("x264")
            );
            assert!(
                String::from_utf8_lossy(&encoded.stderr)
                    .contains("coreav-diagnostic-decoder-host: sandbox")
            );
            let bytes = std::fs::read(&output)?;
            if let Some(expected) = &expected {
                assert_eq!(
                    &bytes, expected,
                    "complete file changed for {codec}/{format}"
                );
            } else {
                expected = Some(bytes);
            }
        }
    }
    Ok(())
}
