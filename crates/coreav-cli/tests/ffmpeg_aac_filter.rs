//! AAC concat filtering stays refused until complete output is byte-exact.
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

#[test]
fn aac_filter_is_an_exact_concat_identity_or_preserved_fallback() {
    let pair = test_oracle::require();
    let ffmpeg = pair.ffmpeg().to_path_buf();
    let ffprobe = pair.ffprobe().to_path_buf();
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ffmpeg-aac-filter-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    let owner = FixtureDir(path);
    let dir = &owner.0;
    for (name, frequency) in [("first", "751"), ("second", "599")] {
        let generated = Command::new(&ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=25:duration=0.32",
                "-f",
                "lavfi",
                "-i",
            ])
            .arg(format!(
                "sine=frequency={frequency}:sample_rate=48000:duration=0.298666667"
            ))
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "0",
                "-g",
                "8",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-use_editlist",
                "0",
            ])
            .arg(dir.join(format!("{name}.mp4")))
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
    }
    let list = dir.join("concat.txt");
    std::fs::write(&list, "file 'first.mp4'\nfile 'second.mp4'\n").unwrap();
    let bin = env!("CARGO_BIN_EXE_coreav");
    for video_only in [false, true] {
        let output = dir.join("output.mp4");
        let reference = dir.join("reference.mp4");
        let ordinary = dir.join("ordinary.mp4");
        let plain_reference = dir.join("plain-reference.mp4");
        let mut args = vec![
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-bsf:a",
            "aac_adtstoasc",
            "-c",
            "copy",
        ];
        if video_only {
            args.push("-an");
        }
        let pin = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error"])
            .args(&args)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{}",
            String::from_utf8_lossy(&pin.stderr)
        );
        let plain: Vec<_> = args
            .iter()
            .copied()
            .filter(|arg| !matches!(*arg, "-bsf:a" | "aac_adtstoasc"))
            .collect();
        let unfiltered_pin = Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error"])
            .args(&plain)
            .arg(&plain_reference)
            .output()
            .unwrap();
        assert!(unfiltered_pin.status.success());
        assert_eq!(
            std::fs::read(&reference).unwrap(),
            std::fs::read(&plain_reference).unwrap()
        );
        let native = Command::new(bin)
            .args(["--strict-safe", "ffmpeg"])
            .args(&plain)
            .arg(&ordinary)
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = std::fs::read_dir(dir).unwrap().count();
            let refused = Command::new(bin)
                .args(["--strict-safe", "ffmpeg"])
                .args(&args)
                .arg(&output)
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                refused.status.code(),
                Some(4),
                "{}",
                String::from_utf8_lossy(&refused.stderr)
            );
            assert!(refused.stdout.is_empty());
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), entries);
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
            let fallback = Command::new(bin)
                .arg("ffmpeg")
                .args(&args)
                .arg(&output)
                .env("COREAV_FALLBACK", &ffmpeg)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(fallback.status.code(), pin.status.code());
            assert_eq!(fallback.stdout, pin.stdout);
            assert_eq!(
                std::fs::read(&output).unwrap(),
                std::fs::read(&reference).unwrap()
            );
            assert_eq!(
                std::fs::read_dir(dir).unwrap().count(),
                entries + usize::from(!existing)
            );
            std::fs::remove_file(&output).unwrap();
        }
    }
    let alac = dir.join("alac.mp4");
    let generated = Command::new(&ffmpeg)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=25:duration=0.32",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=751:sample_rate=48000:duration=0.32",
            "-c:v",
            "libx264",
            "-threads:v",
            "1",
            "-bf",
            "0",
            "-g",
            "8",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "alac",
            "-shortest",
            "-use_editlist",
            "0",
        ])
        .arg(&alac)
        .output()
        .unwrap();
    assert!(generated.status.success());
    let alac_list = dir.join("alac.txt");
    std::fs::write(&alac_list, "file 'alac.mp4'\nfile 'alac.mp4'\n").unwrap();
    let position = Command::new(&ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "packet=pos",
            "-of",
            "json",
        ])
        .arg(dir.join("second.mp4"))
        .output()
        .unwrap();
    assert!(position.status.success());
    let position: Value = serde_json::from_slice(&position.stdout).unwrap();
    let at = position["packets"][0]["pos"]
        .as_str()
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut transport = std::fs::read(dir.join("second.mp4")).unwrap();
    transport[at..at + 2].copy_from_slice(&[0xff, 0xf1]);
    std::fs::write(dir.join("transport-like.mp4"), transport).unwrap();
    let transport_list = dir.join("transport.txt");
    std::fs::write(
        &transport_list,
        "file 'first.mp4'\nfile 'transport-like.mp4'\n",
    )
    .unwrap();
    let native_alac = Command::new(bin)
        .args([
            "--strict-safe",
            "ffmpeg",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&alac_list)
        .args(["-c", "copy"])
        .arg(dir.join("alac-native.mp4"))
        .output()
        .unwrap();
    assert!(
        native_alac.status.success(),
        "{}",
        String::from_utf8_lossy(&native_alac.stderr)
    );
    let native_transport = Command::new(bin)
        .args([
            "--strict-safe",
            "ffmpeg",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&transport_list)
        .args(["-c", "copy"])
        .arg(dir.join("transport-native.mp4"))
        .output()
        .unwrap();
    assert!(
        native_transport.status.success(),
        "{}",
        String::from_utf8_lossy(&native_transport.stderr)
    );
    let output = dir.join("refused output.mp4");
    let reference = dir.join("refused reference.mp4");
    let ordinary_input = dir.join("first.mp4");
    let cases = [
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            alac_list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
        ],
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            transport_list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
        ],
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "null",
        ],
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc,aac_adtstoasc",
        ],
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc=1",
        ],
        vec![
            "-bsf:a",
            "aac_adtstoasc",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
        ],
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:v",
            "h264_mp4toannexb",
        ],
        vec![
            "-i",
            ordinary_input.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a",
            "aac_adtstoasc",
        ],
        vec![
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
            "-bsf:a:0",
            "aac_adtstoasc",
        ],
    ];
    for (index, args) in cases.iter().enumerate() {
        for existing in [false, true] {
            for path in [&output, &reference] {
                if existing {
                    std::fs::write(path, b"existing output").unwrap();
                }
            }
            let pin = Command::new(&ffmpeg)
                .args(["-nostdin", "-v", "error", "-y"])
                .args(args)
                .arg(&reference)
                .output()
                .unwrap();
            assert!(pin.status.code().is_some());
            let entries = std::fs::read_dir(dir).unwrap().count();
            let strict = Command::new(bin)
                .args(["--strict-safe", "ffmpeg", "-y"])
                .args(args)
                .arg(&output)
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                strict.status.code(),
                Some(4),
                "{args:?}: {}",
                String::from_utf8_lossy(&strict.stderr)
            );
            assert!(strict.stdout.is_empty());
            if index < 2 {
                assert!(String::from_utf8_lossy(&strict.stderr).contains("bitstream filters"));
            }
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), entries);
            let fallback = Command::new(bin)
                .args(["ffmpeg", "-y"])
                .args(args)
                .arg(&output)
                .env("COREAV_FALLBACK", &ffmpeg)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                fallback.status.code(),
                pin.status.code(),
                "{args:?}: {}",
                String::from_utf8_lossy(&fallback.stderr)
            );
            assert_eq!(output.exists(), reference.exists());
            if output.exists() {
                assert_eq!(
                    std::fs::read(&output).unwrap(),
                    std::fs::read(&reference).unwrap(),
                    "{args:?}"
                );
            }
            for path in [&output, &reference] {
                if path.exists() {
                    std::fs::remove_file(path).unwrap();
                }
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mock = dir.join("fallback-bin");
            std::fs::create_dir_all(&mock).unwrap();
            let program = mock.join("ffmpeg");
            std::fs::write(&program, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
            std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let replay = Command::new(bin)
                .args(["ffmpeg", "-y"])
                .args(args)
                .arg(&output)
                .env_clear()
                .env("PATH", &mock)
                .output()
                .unwrap();
            assert_eq!(replay.status.code(), Some(19));
            let expected = std::iter::once("-hide_banner")
                .chain(std::iter::once("-y"))
                .chain(args.iter().copied())
                .chain(std::iter::once(output.to_str().unwrap()))
                .flat_map(|arg| arg.bytes().chain([0]))
                .collect::<Vec<_>>();
            assert_eq!(replay.stdout, expected);
            assert!(!output.exists());
        }
    }
}
