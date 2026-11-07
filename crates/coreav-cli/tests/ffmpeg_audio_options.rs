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

fn oracle(dir: &Path, output: &Path, options: &[&str]) {
    success(
        Command::new(test_oracle::require().ffmpeg())
            .args(["-y", "-nostdin", "-v", "error"])
            .args(policy())
            .arg("-i")
            .arg(dir.join("source.mp4"))
            .args(options)
            .arg(output)
            .output()
            .unwrap(),
    );
}

fn native_transcode_control(dir: &Path) -> Vec<u8> {
    let output = dir.join("native-control.mp4");
    success(
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .arg("transcode")
            .arg(dir.join("source.mp4"))
            .arg(&output)
            .args(["--threads", "1", "--encoder", "auto"])
            .env("COREAV_FFMPEG", test_oracle::require().ffmpeg())
            .env_remove("COREAV_STRICT_SAFE")
            .output()
            .unwrap(),
    );
    std::fs::read(output).unwrap()
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

fn audio_entries(bytes: &[u8]) -> Vec<&[u8]> {
    boxes(&boxes(bytes, b"moov")[0][8..], b"trak")
        .into_iter()
        .filter_map(|track| {
            let media = child(track, b"mdia");
            if &child(media, b"hdlr")[16..20] != b"soun" {
                return None;
            }
            let stsd = child(child(child(media, b"minf"), b"stbl"), b"stsd");
            assert_eq!(&stsd[12..16], &1u32.to_be_bytes());
            Some(&stsd[16..])
        })
        .collect()
}

fn esds_rates(payload: &[u8]) -> usize {
    let mut at = 4;
    for tag in [3, 4] {
        assert_eq!(payload[at], tag);
        at += 1;
        loop {
            let byte = payload[at];
            at += 1;
            if byte & 0x80 == 0 {
                break;
            }
        }
        if tag == 3 {
            assert_eq!(payload[at + 2], 0);
            at += 3;
        }
    }
    assert!(matches!(payload[at], 0x40 | 0x69 | 0x6B));
    at + 2
}

fn bitrate_reference(control: &[u8], reference: &[u8]) -> Vec<u8> {
    let rates: Vec<[u8; 12]> = audio_entries(reference)
        .into_iter()
        .map(|entry| {
            let btrt = boxes(&entry[36..], b"btrt");
            assert_eq!(btrt.len(), 1);
            let rate: [u8; 12] = btrt[0][8..].try_into().unwrap();
            assert_eq!(&rate[..4], &[0; 4]);
            if &entry[4..8] == b"mp4a" {
                let esds = boxes(&entry[36..], b"esds");
                assert_eq!(esds.len(), 1);
                let esds = &esds[0][8..];
                let at = esds_rates(esds);
                assert_eq!(&esds[at..at + 11], &rate[1..]);
            }
            rate
        })
        .collect();
    fn rewrite(bytes: &[u8], rates: &[[u8; 12]], next: &mut usize, delta: u32) -> Vec<u8> {
        let mut output = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let item = &bytes[at..at + size];
            let kind = &item[4..8];
            let mut body = match kind {
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => {
                    rewrite(&item[8..], rates, next, delta)
                }
                b"stsd" => [
                    item[8..16].to_vec(),
                    rewrite(&item[16..], rates, next, delta),
                ]
                .concat(),
                b"mp4a" | b"Opus" => {
                    let rate = rates[*next];
                    *next += 1;
                    let mut entry = item[8..36].to_vec();
                    let mut offset = 36;
                    let mut has_btrt = false;
                    while offset < item.len() {
                        let len = u32::from_be_bytes(item[offset..offset + 4].try_into().unwrap())
                            as usize;
                        let mut child = item[offset..offset + len].to_vec();
                        match &child[4..8] {
                            b"esds" => {
                                let start = 8 + esds_rates(&child[8..]);
                                child[start..start + 11].copy_from_slice(&rate[1..]);
                            }
                            b"btrt" => {
                                child[8..].copy_from_slice(&rate);
                                has_btrt = true;
                            }
                            _ => {}
                        }
                        entry.extend_from_slice(&child);
                        offset += len;
                    }
                    if !has_btrt {
                        entry.extend_from_slice(&20u32.to_be_bytes());
                        entry.extend_from_slice(b"btrt");
                        entry.extend_from_slice(&rate);
                    }
                    entry
                }
                _ => item[8..].to_vec(),
            };
            if kind == b"stco" {
                for offset in body[8..].chunks_exact_mut(4) {
                    let value = u32::from_be_bytes(offset.try_into().unwrap()) + delta;
                    offset.copy_from_slice(&value.to_be_bytes());
                }
            }
            output.extend_from_slice(&u32::try_from(body.len() + 8).unwrap().to_be_bytes());
            output.extend_from_slice(kind);
            output.extend_from_slice(&body);
            at += size;
        }
        output
    }
    let mut next = 0;
    let first = rewrite(control, &rates, &mut next, 0);
    assert_eq!(next, rates.len());
    let growth = u32::try_from(first.len() - control.len()).unwrap();
    let mut next = 0;
    rewrite(control, &rates, &mut next, growth)
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
fn copied_bitrate_metadata_preserves_command_admission() {
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
        let reference = dir.0.join("reference.mp4");
        oracle(&dir.0, &reference, &args);
        assert_eq!(
            bitrate_reference(
                &std::fs::read(control).unwrap(),
                &std::fs::read(reference).unwrap()
            ),
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
fn copied_aac_and_opus_match_oracle_metadata_without_other_file_changes() {
    for (encoder, container, variant) in [
        ("aac", "mp4", "ordinary"),
        ("aac", "mp4", "absent"),
        ("aac", "mp4", "dirty"),
        ("libopus", "mp4", "ordinary"),
        ("libopus", "mp4", "absent"),
        ("libopus", "mp4", "dirty"),
    ] {
        let dir = fixture(&format!("copied-{encoder}-{container}-{variant}"));
        let encoded = dir.0.join(format!("encoded.{container}"));
        let mut encode = vec![
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-c:v",
            "copy",
            "-c:a",
            encoder,
            "-ar",
            "48000",
            "-b:a",
            "72k",
            "-disposition:a",
            "0",
        ];
        if variant == "absent" {
            encode.extend(["-write_btrt", "0"]);
        }
        oracle(&dir.0, &encoded, &encode);
        let source = dir.0.join("source.mp4");
        std::fs::rename(encoded, &source).unwrap();
        if variant == "dirty" {
            let mut bytes = std::fs::read(&source).unwrap();
            let mut edits = Vec::new();
            for entry in audio_entries(&bytes) {
                let btrt = boxes(&entry[36..], b"btrt")[0];
                edits.push((btrt.as_ptr() as usize - bytes.as_ptr() as usize + 8, false));
                if encoder == "aac" {
                    let esds = &boxes(&entry[36..], b"esds")[0][8..];
                    edits.push((
                        esds.as_ptr() as usize - bytes.as_ptr() as usize + esds_rates(esds),
                        true,
                    ));
                }
            }
            let dirty: Vec<u8> = [1234u32, 220_000, 1000]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect();
            for (at, esds) in edits {
                let data = if esds { &dirty[1..] } else { &dirty };
                bytes[at..at + data.len()].copy_from_slice(data);
            }
            std::fs::write(&source, bytes).unwrap();
        }
        let options = [
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-fps_mode",
            "passthrough",
            "-c:a",
            "aac",
            "-c:a",
            "copy",
        ];
        let control = native_transcode_control(&dir.0);
        assert_eq!(audio_entries(&control).len(), 2);
        for rate in [None, Some("0"), Some("16k"), Some("96k"), Some("128k")] {
            let mut args = options.to_vec();
            if let Some(rate) = rate {
                args.extend(["-b:a", rate]);
            }
            let actual = dir.0.join("actual.mp4");
            let reference = dir.0.join("reference.mp4");
            success(native(&dir.0, &actual, &args, false));
            oracle(&dir.0, &reference, &args);
            let expected = bitrate_reference(&control, &std::fs::read(reference).unwrap());
            let actual = std::fs::read(actual).unwrap();
            assert_eq!(
                actual.len(),
                expected.len(),
                "{encoder} {container} {variant} {rate:?}"
            );
            assert!(
                actual == expected,
                "complete file differs: {encoder} {container} {variant} {rate:?}; first mismatch {:?}",
                actual.iter().zip(&expected).position(|(a, b)| a != b)
            );
        }
    }
}

#[test]
fn matroska_copy_bitrate_keeps_complete_native_files() {
    for encoder in ["aac", "libopus"] {
        let dir = fixture(&format!("copied-matroska-{encoder}"));
        let encoded = dir.0.join("encoded.mkv");
        oracle(
            &dir.0,
            &encoded,
            &[
                "-map",
                "0:v",
                "-map",
                "0:a",
                "-c:v",
                "copy",
                "-c:a",
                encoder,
                "-ar",
                "48000",
                "-b:a",
                "72k",
                "-disposition:a",
                "0",
            ],
        );
        std::fs::rename(encoded, dir.0.join("source.mp4")).unwrap();
        let options = [
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-fps_mode",
            "passthrough",
            "-c:a",
            "aac",
            "-c:a",
            "copy",
        ];
        let control = dir.0.join("control.mp4");
        success(native(&dir.0, &control, &options, false));
        let control = std::fs::read(control).unwrap();
        assert_eq!(audio_entries(&control).len(), 2);
        for rate in ["0", "16k", "96k", "128k", "garbage", "4294967296"] {
            let mut args = options.to_vec();
            args.extend(["-b:a", rate]);
            let actual = dir.0.join("actual.mp4");
            success(native(&dir.0, &actual, &args, false));
            assert_eq!(std::fs::read(actual).unwrap(), control, "{encoder} {rate}");
        }
    }
}

#[test]
fn ignored_copy_bitrates_match_the_native_default() {
    for encoder in ["aac", "libmp3lame"] {
        let dir = fixture(&format!("copied-preserved-{encoder}"));
        let encoded = dir.0.join("encoded.mp4");
        oracle(
            &dir.0,
            &encoded,
            &[
                "-map", "0:v", "-map", "0:a:0", "-c:v", "copy", "-c:a", encoder, "-ar", "48000",
                "-b:a", "64k",
            ],
        );
        std::fs::rename(encoded, dir.0.join("source.mp4")).unwrap();
        let options = [
            "-c:v", "libx264", "-threads", "1", "-c:a:9", "aac", "-c:a", "copy",
        ];
        let control = dir.0.join("control.mp4");
        success(native(&dir.0, &control, &options, false));
        let control = std::fs::read(control).unwrap();
        for rate in ["garbage", "4294967296", "1e6", "-1"] {
            let mut args = options.to_vec();
            args.extend(["-b:a", rate]);
            let actual = dir.0.join("actual.mp4");
            success(native(&dir.0, &actual, &args, false));
            assert_eq!(std::fs::read(actual).unwrap(), control, "{encoder} {rate}");
        }
    }
}

#[test]
fn copied_mp3_matches_oracle_metadata_with_and_without_bitrate() {
    for sample_rate in ["48000", "44100", "22050", "11025"] {
        for mode in ["cbr", "vbr"] {
            let dir = fixture(&format!("copied-mp3-{sample_rate}-{mode}"));
            let encoded = dir.0.join("encoded.mp4");
            let mut encode = vec![
                "-map",
                "0:v",
                "-map",
                "0:a",
                "-c:v",
                "copy",
                "-c:a",
                "libmp3lame",
                "-ar",
                sample_rate,
                "-disposition:a",
                "0",
            ];
            encode.extend(if mode == "cbr" {
                ["-b:a", "64k"]
            } else {
                ["-q:a", "4"]
            });
            if sample_rate == "11025" {
                encode.extend(["-strict", "-1"]);
            }
            oracle(&dir.0, &encoded, &encode);
            std::fs::rename(encoded, dir.0.join("source.mp4")).unwrap();
            let control = native_transcode_control(&dir.0);
            assert_eq!(audio_entries(&control).len(), 2);
            for rate in [None, Some("0"), Some("16k"), Some("96k"), Some("128k")] {
                let mut args = vec![
                    "-map", "0:v", "-map", "0:a", "-c:v", "libx264", "-threads", "1", "-c:a",
                    "aac", "-c:a", "copy",
                ];
                if let Some(rate) = rate {
                    args.extend(["-b:a", rate]);
                }
                let actual = dir.0.join("actual.mp4");
                success(native(&dir.0, &actual, &args, false));
                if sample_rate == "11025" {
                    args.extend(["-strict", "-1"]);
                }
                let reference = dir.0.join("reference.mp4");
                oracle(&dir.0, &reference, &args);
                let expected = bitrate_reference(&control, &std::fs::read(reference).unwrap());
                let actual = std::fs::read(actual).unwrap();
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "{sample_rate} {mode} {rate:?}"
                );
                assert!(
                    actual == expected,
                    "complete MP3 file differs: {sample_rate} {mode} {rate:?}; first mismatch {:?}",
                    actual.iter().zip(&expected).position(|(a, b)| a != b)
                );
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
