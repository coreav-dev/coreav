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

fn boxes<'a>(mut bytes: &'a [u8], kind: &[u8; 4]) -> Vec<&'a [u8]> {
    let mut found = Vec::new();
    while !bytes.is_empty() {
        let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert!(size >= 8 && size <= bytes.len());
        let (item, rest) = bytes.split_at(size);
        if &item[4..8] == kind {
            found.push(item);
        }
        bytes = rest;
    }
    found
}

fn child<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let found = boxes(&bytes[8..], kind);
    assert_eq!(found.len(), 1, "{kind:?}");
    found[0]
}

fn audio_timing(path: &Path) -> Vec<Vec<u8>> {
    let bytes = std::fs::read(path).unwrap();
    let moov = boxes(&bytes, b"moov")[0];
    let tracks: Vec<_> = boxes(&moov[8..], b"trak")
        .into_iter()
        .filter(|track| &child(child(track, b"mdia"), b"hdlr")[16..20] == b"soun")
        .collect();
    assert_eq!(tracks.len(), 1);
    let track = tracks[0];
    let media = child(track, b"mdia");
    vec![
        child(child(child(media, b"minf"), b"stbl"), b"stts").to_vec(),
        child(media, b"mdhd").to_vec(),
        child(child(track, b"edts"), b"elst").to_vec(),
    ]
}

fn decoded_audio(path: &Path) -> Vec<u8> {
    success(
        Command::new(test_oracle::require().ffmpeg())
            .args(["-nostdin", "-v", "error"])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .arg("-i")
            .arg(path)
            .args(["-map", "0:a:0", "-f", "f32le", "pipe:1"])
            .output()
            .unwrap(),
    )
}

#[test]
fn aac_terminal_timing_matches_the_complete_oracle_boxes() {
    let dir = fixture("aac-timing");
    let source = std::fs::read(dir.0.join("source.mp4")).unwrap();
    let movie = boxes(&source, b"moov")[0];
    let source_clock =
        u32::from_be_bytes(child(movie, b"mvhd")[20..24].try_into().unwrap()).to_string();
    for (stream, rate) in [("0:a:0", "48000"), ("0:a:1", "44100")] {
        for video in [false, true] {
            for explicit in [false, true] {
                let name = format!("{rate}-{video}-{explicit}");
                let actual = dir.0.join(format!("{name}-actual.mp4"));
                let reference = dir.0.join(format!("{name}-reference.mp4"));
                let mut args = if video {
                    vec!["-map", "0:v", "-c:v", "libx264", "-threads", "1"]
                } else {
                    vec!["-vn"]
                };
                args.extend(["-map", stream, "-b:a", "96k"]);
                if explicit {
                    args.extend(["-c:a", "aac"]);
                }
                success(native(&dir.0, &actual, &args, false));
                success(
                    Command::new(test_oracle::require().ffmpeg())
                        .args(["-nostdin", "-v", "error"])
                        .args(policy())
                        .arg("-i")
                        .arg(dir.0.join("source.mp4"))
                        .args(&args)
                        .args(["-movie_timescale", if video { &source_clock } else { rate }])
                        .arg(&reference)
                        .output()
                        .unwrap(),
                );
                assert_eq!(audio_timing(&actual), audio_timing(&reference), "{name}");
                assert_eq!(decoded_audio(&actual), decoded_audio(&reference), "{name}");
            }
        }
    }
    let source = dir.0.join("source.mkv");
    success(
        Command::new(test_oracle::require().ffmpeg())
            .args(["-nostdin", "-v", "error"])
            .args(policy())
            .arg("-i")
            .arg(dir.0.join("source.mp4"))
            .args(["-map", "0:v", "-map", "0:a:0", "-c", "copy"])
            .arg(&source)
            .output()
            .unwrap(),
    );
    let actual = dir.0.join("matroska-actual.mp4");
    let reference = dir.0.join("matroska-reference.mp4");
    let args = [
        "-c:v",
        "libx264",
        "-c:a",
        "aac",
        "-threads",
        "1",
        "-b:a",
        "96k",
        "-fps_mode",
        "passthrough",
    ];
    success(
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["ffmpeg", "-i"])
            .arg(&source)
            .args(args)
            .arg(&actual)
            .env("COREAV_FFMPEG", test_oracle::require().ffmpeg())
            .env("COREAV_FALLBACK", dir.0.join("no-whole-command-fallback"))
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .env_remove("COREAV_STRICT_SAFE")
            .output()
            .unwrap(),
    );
    success(
        Command::new(test_oracle::require().ffmpeg())
            .args(["-nostdin", "-v", "error"])
            .args(policy())
            .arg("-i")
            .arg(&source)
            .args(args)
            .args(["-movie_timescale", "48000"])
            .arg(&reference)
            .output()
            .unwrap(),
    );
    assert_eq!(audio_timing(&actual), audio_timing(&reference), "Matroska");
    assert_eq!(
        decoded_audio(&actual),
        decoded_audio(&reference),
        "Matroska"
    );
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
fn default_bitrate_admission_preserves_legacy_native_commands() {
    let dir = fixture("bitrate");
    let source = dir.0.join("aac-source.mp4");
    success(
        Command::new(test_oracle::require().ffmpeg())
            .args(["-nostdin", "-v", "error"])
            .args(policy())
            .arg("-i")
            .arg(dir.0.join("source.mp4"))
            .args([
                "-map", "0:v", "-map", "0:a:0", "-c:v", "copy", "-c:a", "aac",
            ])
            .arg(&source)
            .output()
            .unwrap(),
    );
    std::fs::rename(source, dir.0.join("source.mp4")).unwrap();
    for options in [
        "-c:a aac -c:a copy",
        "-c:a:9 aac -c:a copy",
        "-an -c:a aac",
        "-an -c:a aac -c:a copy",
    ] {
        let mut args = vec!["-c:v", "libx264", "-threads", "1"];
        args.extend(options.split_whitespace());
        let control = dir.0.join("control.mp4");
        success(native(&dir.0, &control, &args, false));
        args.extend(["-b:a", "96k"]);
        let actual = dir.0.join("actual.mp4");
        success(native(&dir.0, &actual, &args, false));
        assert_eq!(
            std::fs::read(control).unwrap(),
            std::fs::read(actual).unwrap(),
            "{options}"
        );
    }
    for options in ["-c:a copy", "-an", "-c:a:9 copy -an"] {
        let mut args = vec!["-c:v", "libx264", "-b:a", "96k"];
        args.extend(options.split_whitespace());
        for existing in [false, true] {
            let output = dir.0.join("refused.mp4");
            if existing {
                std::fs::write(&output, b"keep destination").unwrap();
            }
            let result = native(&dir.0, &output, &args, false);
            assert_eq!(result.status.code(), Some(4), "{options}: {result:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("'-b:a' without an explicit audio encoder"),
                "{result:?}"
            );
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"keep destination");
                std::fs::remove_file(output).unwrap();
            } else {
                assert!(!output.exists());
            }
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
                } else if options.contains(&"-b:a") {
                    "'-b:a'"
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
