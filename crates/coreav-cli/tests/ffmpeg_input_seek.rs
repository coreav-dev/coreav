//! Input-seek copies retain exact packet timing or refuse before publication.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn snapshot(path: &Path, ffprobe: &Path) -> Value {
    let result = Command::new(ffprobe)
        .args(["-v", "error", "-show_streams", "-show_packets", "-show_frames", "-show_format", "-show_data_hash", "sha256", "-show_entries",
            "stream:stream_tags=language,handler_name:packet=stream_index,pts,dts,duration,size,flags,data_hash:frame=stream_index,best_effort_timestamp,pkt_dts,duration:format=start_time,duration:format_tags=", "-of", "json"])
        .arg(path).output().unwrap();
    assert!(
        result.status.success() && result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

fn pixels(path: &Path, ffmpeg: &Path) -> Vec<u8> {
    let result = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error"])
        .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
        .arg("-i")
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "yuv420p",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(result.status.success() && result.stderr.is_empty());
    assert!(!result.stdout.is_empty());
    result.stdout
}

#[test]
fn input_seek_copy_keeps_only_exact_packet_timelines() {
    let pair = test_oracle::require();
    let ffmpeg = pair.ffmpeg().to_path_buf();
    let ffprobe = pair.ffprobe().to_path_buf();
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ffmpeg-input-seek-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let owner = FixtureDir(path);
    let dir = &owner.0;
    let mut sources = Vec::new();
    for (name, bframes, gop) in [
        ("reordered", "2", "250"),
        ("inter", "0", "12"),
        ("intra", "0", "1"),
    ] {
        let path = dir.join(format!("{name}.mp4"));
        let result = Command::new(&ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=24:duration=3.25",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=751:sample_rate=44100:duration=3.25",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                bframes,
                "-g",
                gop,
                "-keyint_min",
                gop,
                "-sc_threshold",
                "0",
                "-c:a",
                "aac",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        sources.push((path, bframes));
    }
    let bin = env!("CARGO_BIN_EXE_coreav");
    let fallback_path = std::env::join_paths(
        std::iter::once(ffmpeg.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let mut index = 0;
    let mut check = |input: &Path, seek: &str, before: &[&str], after: &[&str], native: bool| {
        index += 1;
        let output = dir.join(format!("output-{index}.mp4"));
        let reference = dir.join(format!("reference-{index}.mp4"));
        let mut args = vec!["-y", "-ss", seek];
        args.extend(before);
        args.extend(["-i", input.to_str().unwrap(), "-c", "copy"]);
        args.extend(after);
        let pin = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error"])
            .args(&args)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&pin.stderr)
        );
        args.push(output.to_str().unwrap());
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = std::fs::read_dir(dir).unwrap().count();
            let strict = Command::new(bin)
                .args(["--strict-safe", "ffmpeg"])
                .args(&args)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                strict.status.code(),
                Some(if native { 0 } else { 4 }),
                "{args:?}: {}",
                String::from_utf8_lossy(&strict.stderr)
            );
            assert!(strict.stdout.is_empty());
            if native {
                assert!(strict.stderr.is_empty());
                assert_eq!(
                    snapshot(&output, &ffprobe),
                    snapshot(&reference, &ffprobe),
                    "{args:?}"
                );
                assert_eq!(
                    pixels(&output, &ffmpeg),
                    pixels(&reference, &ffmpeg),
                    "{args:?}"
                );
            } else {
                assert!(
                    String::from_utf8_lossy(&strict.stderr).contains(if seek.starts_with('-') {
                        "a negative time"
                    } else {
                        "input-seek copy"
                    }),
                    "{}",
                    String::from_utf8_lossy(&strict.stderr)
                );
                if existing {
                    assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                } else {
                    assert!(!output.exists());
                }
                assert_eq!(std::fs::read_dir(dir).unwrap().count(), entries);
                let fallback = Command::new(bin)
                    .arg("ffmpeg")
                    .args(&args)
                    .env("PATH", &fallback_path)
                    .env_remove("COREAV_STRICT_SAFE")
                    .env_remove("COREAV_FALLBACK_ACTIVE")
                    .output()
                    .unwrap();
                assert!(
                    fallback.status.success(),
                    "{}",
                    String::from_utf8_lossy(&fallback.stderr)
                );
                assert!(fallback.stdout.is_empty());
                assert_eq!(
                    std::fs::read(&output).unwrap(),
                    std::fs::read(&reference).unwrap(),
                    "{args:?}"
                );
            }
            std::fs::remove_file(&output).unwrap();
        }
        #[cfg(unix)]
        if !native && seek == "0.2" {
            use std::os::unix::fs::PermissionsExt;
            let mock = dir.join("fallback-bin");
            std::fs::create_dir_all(&mock).unwrap();
            let program = mock.join("ffmpeg");
            std::fs::write(&program, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
            std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let replay = Command::new(bin)
                .arg("ffmpeg")
                .args(&args)
                .env_clear()
                .env("PATH", &mock)
                .output()
                .unwrap();
            assert_eq!(replay.status.code(), Some(19));
            assert_eq!(
                replay.stdout,
                std::iter::once("-hide_banner")
                    .chain(args.iter().copied())
                    .flat_map(|arg| arg.bytes().chain([0]))
                    .collect::<Vec<_>>()
            );
            assert!(!output.exists());
        }
    };
    for (source, bframes) in &sources {
        for seek in ["0", "0.2", "0.5"] {
            for selection in ["av", "video", "audio"] {
                let mut after = vec!["-t", "1"];
                match selection {
                    "video" => after.push("-an"),
                    "audio" => after.push("-vn"),
                    _ => {}
                }
                check(
                    source,
                    seek,
                    &[],
                    &after,
                    *bframes == "0" && selection == "video" && seek != "0.2",
                );
            }
        }
    }
    let inter = &sources[1].0;
    check(inter, "0.5", &[], &["-an", "-t", "1.01"], false);
    check(inter, "0.5", &[], &["-an"], true);
    check(inter, "0.5", &[], &["-an", "-to", "1"], true);
    check(inter, "-0.2", &[], &["-an", "-t", "1"], false);
    check(inter, "0.2", &[], &["-an", "-ss", "0.3", "-t", "1"], false);
    check(inter, "0.5", &["-t", "1"], &["-an"], false);
    check(inter, "0.5", &["-to", "1"], &["-an"], false);
    let tagged = dir.join("tagged.mp4");
    let result = Command::new(&ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(inter)
        .args(["-c", "copy", "-metadata", "title=Source title"])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(result.status.success());
    check(&tagged, "0.5", &[], &["-an", "-t", "1"], false);
    for (scale, source, seek, native) in [
        ("1000", inter, "0.5", false),
        ("8000", inter, "0.5", false),
        ("10000", inter, "0.5", true),
        ("24000", inter, "0.5", true),
        ("1000000000", &sources[2].0, "0.541666667", false),
    ] {
        let clock = dir.join(format!("clock-{scale}.mp4"));
        let result = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(source)
            .args(["-an", "-c", "copy", "-video_track_timescale", scale])
            .arg(&clock)
            .output()
            .unwrap();
        assert!(result.status.success());
        check(&clock, seek, &[], &["-an", "-t", "1"], native);
    }
    check(inter, "0.5", &[], &["-an", "-t", "1.000000001"], false);
    check(inter, "0.5", &[], &["-an", "-to", "1.000000001"], false);
}
