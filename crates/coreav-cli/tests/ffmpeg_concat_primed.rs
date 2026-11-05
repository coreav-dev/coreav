//! Primed AAC concat preserves stored samples and independently derived presentation edits.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "support/concat_presentation.rs"]
mod concat_presentation;

use concat_presentation::Oracle;
use std::path::{Path, PathBuf};
use std::process::Command;

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(name: &str) -> FixtureDir {
    let path =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    FixtureDir(path)
}

fn oracle() -> Option<Oracle> {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    let ffprobe = ffmpeg.with_file_name("ffprobe");
    if !ffmpeg.is_file() || !ffprobe.is_file() {
        assert!(
            std::env::var_os("CI").is_none() && std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none(),
            "set COREAV_FFMPEG to the pinned oracle"
        );
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let verified = Command::new("python3")
        .arg(root.join("scripts/oracle_pin.py"))
        .arg("--verify-bin")
        .arg(ffmpeg.parent().unwrap())
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    Some(Oracle { ffmpeg, ffprobe })
}

fn native(
    oracle: &Oracle,
    inputs: &[PathBuf],
    args: &[&str],
    output: &Path,
    video_only: bool,
    counts: &[usize],
) {
    for existing in [false, true] {
        if existing {
            std::fs::write(output, b"existing output").unwrap();
        }
        let entries = std::fs::read_dir(output.parent().unwrap()).unwrap().count();
        let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .arg("ffmpeg")
            .args(args)
            .arg(output)
            .env("COREAV_STRICT_SAFE", "1")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
        assert_eq!(
            std::fs::read_dir(output.parent().unwrap()).unwrap().count(),
            entries + usize::from(!existing)
        );
        concat_presentation::verify(oracle, inputs, output, video_only, counts);
        std::fs::remove_file(output).unwrap();
    }
}

#[test]
fn twelve_primed_concat_recipes_preserve_continuous_presentation() {
    let Some(oracle) = oracle() else {
        return;
    };
    let owner = fixture("ffmpeg-concat-primed-presentation");
    let dir = &owner.0;
    for (channels, duration, samples) in [
        ("1", "2.25", 108_000),
        ("2", "1.75", 84_000),
        ("1", "2.5", 120_000),
    ] {
        let source = format!("source-{channels}-{duration}.mp4");
        let peer = format!("peer-{channels}-{duration}.mp4");
        for (name, pitch, length) in [(&source, "857", duration), (&peer, "1163", "2.25")] {
            oracle.run(
                &oracle.ffmpeg,
                &[
                    "-nostdin",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=80x64:rate=24:duration=2.25",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={pitch}:sample_rate=48000:duration={length}"),
                    "-c:v",
                    "libx264",
                    "-threads:v",
                    "1",
                    "-bf",
                    "2",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-ac",
                    channels,
                    name,
                ],
                dir,
            );
        }
        for (count, video_only, faststart, mixed) in [
            (2, false, false, false),
            (3, false, true, false),
            (2, true, false, false),
            (3, false, false, true),
        ] {
            let names = if mixed {
                vec![source.clone(), peer.clone(), source.clone()]
            } else {
                vec![source.clone(); count]
            };
            let inputs: Vec<_> = names.iter().map(|name| dir.join(name)).collect();
            let list = dir.join("list.txt");
            std::fs::write(
                &list,
                names
                    .iter()
                    .map(|name| format!("file {name}\n"))
                    .collect::<String>(),
            )
            .unwrap();
            let output = dir.join("candidate.mp4");
            let mut args = vec![
                "-y",
                "-f",
                "concat",
                "-i",
                list.to_str().unwrap(),
                "-c",
                "copy",
            ];
            if video_only {
                args.push("-an");
            }
            if mixed {
                args.extend(["-map", "0"]);
            }
            if faststart {
                args.extend(["-movflags", "+faststart"]);
            }
            let counts = if video_only {
                vec![]
            } else if mixed {
                vec![samples, 108_000, samples]
            } else {
                vec![samples; count]
            };
            eprintln!(
                "primed recipe channels={channels} duration={duration} count={count} video_only={video_only} faststart={faststart} mixed={mixed}"
            );
            native(&oracle, &inputs, &args, &output, video_only, &counts);
        }
    }
}

#[test]
fn rate_mixed_later_idr_and_aud_controls_qualify_separately() {
    let Some(oracle) = oracle() else {
        return;
    };
    let owner = fixture("ffmpeg-concat-primed-controls");
    let dir = &owner.0;
    for (name, rate, options) in [
        ("rate", "44100", vec![]),
        ("mixed", "48000", vec![]),
        ("later-idr", "48000", vec!["-g", "24"]),
        ("aud", "48000", vec!["-x264-params", "aud=1"]),
        ("in-band", "48000", vec!["-x264-params", "repeat-headers=1"]),
    ] {
        let source = dir.join(format!("{name}.mp4"));
        let sine = format!("sine=frequency=659:sample_rate={rate}:duration=2.25");
        let mut recipe = vec![
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=80x64:rate=24:duration=2.25",
            "-f",
            "lavfi",
            "-i",
            &sine,
            "-c:v",
            "libx264",
            "-threads:v",
            "1",
            "-bf",
            "2",
            "-c:a",
            "aac",
        ];
        recipe.extend(options);
        recipe.push(source.to_str().unwrap());
        oracle.run(&oracle.ffmpeg, &recipe, dir);
        let list = dir.join("list.txt");
        let inputs = if name == "mixed" {
            let bare = dir.join("bare.mp4");
            oracle.run(
                &oracle.ffmpeg,
                &[
                    "-nostdin",
                    "-i",
                    source.to_str().unwrap(),
                    "-c",
                    "copy",
                    "-use_editlist",
                    "0",
                    bare.to_str().unwrap(),
                ],
                dir,
            );
            std::fs::write(&list, format!("file {name}.mp4\nfile bare.mp4\n")).unwrap();
            vec![source, bare]
        } else {
            std::fs::write(&list, format!("file {name}.mp4\n").repeat(2)).unwrap();
            vec![source.clone(), source]
        };
        let output = dir.join("output.mp4");
        let mut args = vec![
            "-y",
            "-f",
            "concat",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
        ];
        if name == "mixed" {
            args.push("-an");
        }
        let stdout = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .arg("ffmpeg")
            .args([
                "-y",
                "-f",
                "concat",
                "-i",
                list.to_str().unwrap(),
                "-c",
                "copy",
                "-f",
                "mp4",
                "pipe:1",
            ])
            .env("COREAV_STRICT_SAFE", "1")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert_eq!(
            stdout.status.code(),
            Some(4),
            "{name}: {}",
            String::from_utf8_lossy(&stdout.stderr)
        );
        assert!(stdout.stdout.is_empty());
        if name != "in-band" {
            native(
                &oracle,
                &inputs,
                &args,
                &output,
                name == "mixed",
                if name == "mixed" {
                    &[]
                } else if name == "rate" {
                    &[99_225, 99_225]
                } else {
                    &[108_000, 108_000]
                },
            );
            continue;
        }
        let reference = dir.join("reference.mp4");
        let mut oracle_args = vec!["-nostdin"];
        oracle_args.extend(args.iter().copied());
        oracle_args.push(reference.to_str().unwrap());
        oracle.run(&oracle.ffmpeg, &oracle_args, dir);
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = std::fs::read_dir(dir).unwrap().count();
            let strict = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .arg("ffmpeg")
                .args(&args)
                .arg(&output)
                .env("COREAV_STRICT_SAFE", "1")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(strict.status.code(), Some(4));
            assert!(strict.stdout.is_empty());
            assert_eq!(std::fs::read_dir(dir).unwrap().count(), entries);
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
            let fallback = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .arg("ffmpeg")
                .args(&args)
                .arg(&output)
                .env("COREAV_FALLBACK", &oracle.ffmpeg)
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
                std::fs::read(&reference).unwrap()
            );
            std::fs::remove_file(&output).unwrap();
        }
    }
}
