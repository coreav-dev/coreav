#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

#[test]
fn probe_defaults_leave_pinned_output_byte_identical() {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    let ffprobe = ffmpeg.with_file_name("ffprobe");
    if !ffmpeg.is_file() || !ffprobe.is_file() {
        eprintln!("Set COREAV_FFMPEG to the pinned ffmpeg to compare probe tuning defaults");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("probe-tuning");
    std::fs::create_dir_all(&dir).unwrap();
    for extension in ["mp4", "mkv", "ts"] {
        let input = dir.join(format!("input.{extension}"));
        let generated = Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=32x32:r=5",
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000",
                "-t",
                "0.4",
                "-c:v",
                "libx264",
                "-g",
                "1",
                "-c:a",
                "aac",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let query = [
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ];
        let reference = Command::new(&ffprobe)
            .args(query)
            .arg(&input)
            .output()
            .unwrap();
        let baseline = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffprobe"])
            .args(query)
            .arg(&input)
            .output()
            .unwrap();
        assert!(reference.status.success());
        assert!(
            baseline.status.success(),
            "{}",
            String::from_utf8_lossy(&baseline.stderr)
        );
        for flags in [
            vec!["-probesize", "5000000"],
            vec!["-analyzeduration", "0"],
            vec!["-threads", "1"],
            vec!["-show_private_data"],
            vec![
                "-probesize",
                "5000000",
                "-analyzeduration",
                "0",
                "-threads",
                "1",
                "-show_private_data",
            ],
        ] {
            let oracle = Command::new(&ffprobe)
                .args(query)
                .args(&flags)
                .arg(&input)
                .output()
                .unwrap();
            assert!(
                oracle.status.success(),
                "{extension} {flags:?}: {}",
                String::from_utf8_lossy(&oracle.stderr)
            );
            assert_eq!(
                oracle.stdout, reference.stdout,
                "{extension} {flags:?}: the pinned output changed"
            );
            let native = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["--strict-safe", "ffprobe"])
                .args(query)
                .args(&flags)
                .arg(&input)
                .output()
                .unwrap();
            assert_eq!(
                native.status.code(),
                Some(0),
                "{extension} {flags:?}: {}",
                String::from_utf8_lossy(&native.stderr)
            );
            assert_eq!(native.stdout, baseline.stdout, "{extension} {flags:?}");
        }
    }
}
