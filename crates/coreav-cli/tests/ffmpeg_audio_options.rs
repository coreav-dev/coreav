//! Native admission for identity audio properties and default AAC bitrate.
#![cfg(all(feature = "native-h264", any(target_os = "linux", target_os = "macos")))]
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn success(result: Output) -> Vec<u8> {
    assert!(result.status.success(), "{result:?}");
    result.stdout
}

fn policy() -> impl Iterator<Item = &'static str> {
    include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace()
}

fn fixture(name: &str) -> Fixture {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("audio-options-{name}-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    success(
        Command::new(test_oracle::require().ffmpeg())
            .args(["-nostdin", "-v", "error", "-f", "lavfi"])
            .args(policy())
            .arg("-i")
            .arg("testsrc2=size=32x24:rate=24:duration=0.25")
            .args(["-f", "lavfi"])
            .args(policy())
            .arg("-i")
            .arg("sine=frequency=701:sample_rate=48000:duration=0.25")
            .args(["-f", "lavfi"])
            .args(policy())
            .arg("-i")
            .arg("sine=frequency=997:sample_rate=44100:duration=0.25")
            .args([
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "0",
                "-c:a",
                "flac",
                "-ac:a:1",
                "2",
                "-disposition:a",
                "0",
                "-strict",
                "-2",
            ])
            .arg(dir.join("source.mp4"))
            .output()
            .unwrap(),
    );
    Fixture(dir)
}

fn native(dir: &Path, output: &Path, options: &[&str], strict: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    if strict {
        command.arg("--strict-safe");
    }
    command
        .args(["ffmpeg", "-y"])
        .arg("-i")
        .arg(dir.join("source.mp4"))
        .args(options)
        .arg(output)
        .env("COREAV_FFMPEG", test_oracle::require().ffmpeg())
        .env("COREAV_FALLBACK", dir.join("no-whole-command-fallback"))
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .env_remove("COREAV_STRICT_SAFE")
        .output()
        .unwrap()
}

fn audio_properties(path: &Path) -> serde_json::Value {
    let bytes = success(
        Command::new(test_oracle::require().ffprobe())
            .args(["-v", "error"])
            .args(policy())
            .args(["-select_streams", "a"])
            .args([
                "-show_entries",
                "stream=codec_name,sample_rate,channels",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap(),
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn identity_audio_options_and_default_aac_bitrate_run_natively() {
    let dir = fixture("encode");
    let mut cases = vec![
        (
            "default-mp4",
            "mp4",
            "-vn -map 0:a:0 -b:a 96k",
            "48000",
            "1",
            "aac",
        ),
        (
            "default-m4a",
            "m4a",
            "-vn -map 0:a:1 -b:a 96k",
            "44100",
            "2",
            "aac",
        ),
        (
            "explicit",
            "mp4",
            "-vn -map 0:a:0 -c:a aac -b:a 96k",
            "48000",
            "1",
            "aac",
        ),
        (
            "last-kind",
            "mp4",
            "-vn -map 0:a:1 -c copy -c:a aac",
            "44100",
            "2",
            "aac",
        ),
        (
            "last-all",
            "mp4",
            "-vn -map 0:a:0 -c:a copy -c aac",
            "48000",
            "1",
            "aac",
        ),
        (
            "output-index",
            "mp4",
            "-vn -map 0:a:1 -c copy -c:0 aac",
            "44100",
            "2",
            "aac",
        ),
        (
            "kind-index",
            "mp4",
            "-vn -map 0:a:1 -c copy -c:a:0 aac",
            "44100",
            "2",
            "aac",
        ),
        (
            "unmatched",
            "mp4",
            "-vn -map 0:a:0 -c:a:1 copy -c:v copy",
            "48000",
            "1",
            "aac",
        ),
        (
            "overridden-pcm",
            "mp4",
            "-vn -map 0:a:0 -c:a pcm_s16le -c:a aac",
            "48000",
            "1",
            "aac",
        ),
        (
            "video-default",
            "mp4",
            "-map 0:v -map 0:a:0 -threads 1 -b:a 96k",
            "48000",
            "1",
            "aac",
        ),
        (
            "mp3",
            "mp3",
            "-vn -map 0:a:1 -c:a libmp3lame -b:a 96k",
            "44100",
            "2",
            "mp3",
        ),
    ];
    if cfg!(feature = "libopus") {
        cases.push((
            "opus",
            "mp4",
            "-vn -map 0:a:0 -c:a libopus",
            "48000",
            "1",
            "opus",
        ));
        cases.push((
            "matroska",
            "mkv",
            "-map 0:v -map 0:a:0 -c:v libx264 -c:a libopus -threads 1",
            "48000",
            "1",
            "opus",
        ));
    }
    for (name, extension, options, rate, channels, codec) in cases {
        let options: Vec<_> = options.split_whitespace().collect();
        let control = dir.0.join(format!("{name}-control.{extension}"));
        let mut explicit = vec![
            "-c:a",
            if codec == "mp3" {
                "libmp3lame"
            } else if codec == "opus" {
                "libopus"
            } else {
                "aac"
            },
        ];
        explicit.extend(&options);
        success(native(&dir.0, &control, &explicit, false));
        let reference = dir.0.join(format!("{name}-reference.{extension}"));
        success(
            Command::new(test_oracle::require().ffmpeg())
                .args(["-nostdin", "-v", "error"])
                .args(policy())
                .arg("-i")
                .arg(dir.0.join("source.mp4"))
                .args(&options)
                .args(["-ar", rate, "-ac", channels])
                .arg(&reference)
                .output()
                .unwrap(),
        );
        let properties = audio_properties(&reference);
        assert_eq!(properties["streams"][0]["codec_name"], codec, "{name}");
        assert_eq!(properties["streams"][0]["sample_rate"], rate, "{name}");
        assert_eq!(
            properties["streams"][0]["channels"],
            channels.parse::<u32>().unwrap(),
            "{name}"
        );
        for identity in [
            vec![],
            vec!["-ar", rate],
            vec!["-ac", channels],
            vec!["-ar", rate, "-ac", channels],
        ] {
            let actual = dir
                .0
                .join(format!("{name}-{}.{}", identity.len(), extension));
            let mut args = options.clone();
            args.extend(identity);
            let result = native(&dir.0, &actual, &args, false);
            assert!(result.status.success(), "{name} {args:?}: {result:?}");
            assert!(!String::from_utf8_lossy(&result.stderr).contains("Fallback"));
            assert_eq!(
                std::fs::read(&actual).unwrap(),
                std::fs::read(&control).unwrap(),
                "{name} {args:?}"
            );
            assert_eq!(audio_properties(&actual), properties, "{name} {args:?}");
        }
    }
}

#[test]
fn nonidentity_and_copy_audio_options_preserve_destinations() {
    let dir = fixture("refuse");
    let mut cases: Vec<Vec<_>> = [
        "-vn -map 0:a:0 -ar 44100",
        "-vn -map 0:a:0 -ac 2",
        "-vn -map 0:a:1 -ar 48000",
        "-vn -map 0:a:1 -ac 1",
        "-f ipod -vn -map 0:a:0 -ar 44100",
        "-f mp3 -vn -map 0:a:0 -c:a libmp3lame -ac 2",
        "-vn -map 0:a:0 -c:a aac -c copy -ar 48000",
        "-vn -map 0:a:0 -c:a aac -c:a:0 copy -ac 1",
        "-vn -map 0:a:0 -c:a:1 aac -c copy -ar 48000",
        "-vn -map 0:a:0 -c:a pcm_s16le -c:a copy -ar 48000",
        "-vn -map 0:a:0 -c:v aac -c:a copy -ac 1",
        "-map 0:v -an -ar 48000",
        "-map 0:v -map 0:a:0 -c:a copy -ac 1",
        "-vn -map 0:a:0 -c copy -b:a 96k",
    ]
    .into_iter()
    .map(|options| options.split_whitespace().collect())
    .collect();
    if cfg!(feature = "libopus") {
        cases.push(vec![
            "-vn", "-map", "0:a:1", "-c:a", "libopus", "-ar", "44100",
        ]);
    }
    let input = dir.0.join("source.mp4");
    let mut copy = vec![
        "-i",
        input.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a:0",
        "-c",
        "copy",
    ];
    success(native(&dir.0, &dir.0.join("multiple.mp4"), &copy, false));
    copy.extend(["-ar", "48000"]);
    cases.push(copy);
    cases.push(vec![
        "-i",
        input.to_str().unwrap(),
        "-map",
        "1:a:0",
        "-c",
        "copy",
        "-ac",
        "1",
    ]);
    for options in cases {
        for strict in [false, true] {
            for existing in [false, true] {
                let output = dir.0.join("refused.mp4");
                if existing {
                    std::fs::write(&output, b"keep destination").unwrap();
                }
                let result = native(&dir.0, &output, &options, strict);
                assert_eq!(result.status.code(), Some(4), "{options:?}: {result:?}");
                let error = String::from_utf8_lossy(&result.stderr);
                let option = if options.contains(&"-ar") {
                    "'-ar'"
                } else {
                    "'-ac'"
                };
                assert!(
                    error.contains(option)
                        || error.contains("encoder options")
                        || strict && error.contains("strict-safe"),
                    "{options:?}: {error}"
                );
                if existing {
                    assert_eq!(std::fs::read(&output).unwrap(), b"keep destination");
                    std::fs::remove_file(&output).unwrap();
                } else {
                    assert!(!output.exists(), "{options:?}");
                }
                assert!(std::fs::read_dir(&dir.0).unwrap().all(|entry| {
                    let name = entry.unwrap().file_name();
                    !name.to_string_lossy().contains(".coreav-")
                }));
            }
        }
    }
}
