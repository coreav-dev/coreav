//! Audio-only PCM MP4 preserves the complete presented samples and packet clocks.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pcm-mp4-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn policy() -> Vec<&'static str> {
    include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect()
}

fn success(output: Output) {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

fn native(input: &Path, output: &Path, strict: bool, extra: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    if strict {
        command.arg("--strict-safe");
    }
    command
        .args(["ffmpeg", "-y", "-i"])
        .arg(input)
        .args(["-ac", "1", "-ar", "48000", "-c:a", "pcm_s16le", "-vn"])
        .args(extra)
        .arg(output)
        .env("COREAV_FALLBACK", output.with_file_name("missing-ffmpeg"))
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .env_remove("COREAV_STRICT_SAFE")
        .output()
        .unwrap()
}

fn children(mut bytes: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let size = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert!(size >= 8 && size <= bytes.len());
        result.push((bytes[4..8].try_into().unwrap(), &bytes[8..size]));
        bytes = &bytes[size..];
    }
    result
}

fn child<'a>(bytes: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    children(bytes)
        .into_iter()
        .find(|(key, _)| key == kind)
        .unwrap()
        .1
}

fn records(ffprobe: &Path, file: &Path, pcm: &[u8]) -> Value {
    let output = Command::new(ffprobe)
        .args(["-v", "error"])
        .args(policy())
        .arg("-i")
        .arg(file)
        .args([
            "-show_streams",
            "-show_format",
            "-show_packets",
            "-show_data",
            "-show_data_hash",
            "sha256",
            "-count_packets",
            "-of",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let mut record: Value = serde_json::from_slice(&output.stdout).unwrap();
    let bytes = std::fs::read(file).unwrap();
    let data = child(&bytes, b"mdat");
    assert_eq!(data, pcm);
    let start = data.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut end = start;
    for packet in record["packets"].as_array_mut().unwrap() {
        let pos = packet["pos"].as_str().unwrap().parse::<usize>().unwrap();
        let size = packet["size"].as_str().unwrap().parse::<usize>().unwrap();
        assert_eq!(pos, end);
        end += size;
        assert!(end <= start + data.len());
        packet.as_object_mut().unwrap().remove("pos");
    }
    assert_eq!(end, start + pcm.len());
    record["format"]["filename"] = Value::String("output.mp4".to_owned());
    record
}

fn wave(path: &Path, frames: usize) -> Vec<u8> {
    let pcm: Vec<u8> = (0..frames)
        .flat_map(|n| (n.wrapping_mul(73) as i16).to_le_bytes())
        .collect();
    let mut bytes = coreav_audio::wav::header(
        coreav_audio::SampleFormat::S16,
        1,
        48_000,
        4,
        Some(pcm.len() as u64),
    );
    bytes.extend_from_slice(&pcm);
    std::fs::write(path, bytes).unwrap();
    pcm
}

#[test]
fn lossless_sources_match_complete_records_payload_and_terminal_packet() {
    let oracle = test_oracle::require();
    let dir = scratch("lossless");
    for frames in [1, 4800, 52_800, 97_037] {
        let input = dir.join("source.wav");
        let pcm = wave(&input, frames);
        let flac = dir.join("flac.mp4");
        let matroska = dir.join("flac.mkv");
        let mut sources = vec![&input];
        if frames >= 16 {
            success(
                Command::new(oracle.ffmpeg())
                    .args(["-nostdin", "-v", "error", "-y"])
                    .args(policy())
                    .arg("-i")
                    .arg(&input)
                    .args(["-c:a", "flac", "-strict", "-2"])
                    .arg(&flac)
                    .output()
                    .unwrap(),
            );
            sources.push(&flac);
            success(
                Command::new(oracle.ffmpeg())
                    .args(["-nostdin", "-v", "error", "-y"])
                    .args(policy())
                    .arg("-i")
                    .arg(&input)
                    .args(["-c:a", "flac"])
                    .arg(&matroska)
                    .output()
                    .unwrap(),
            );
            sources.push(&matroska);
        }
        for source in sources {
            let reference = dir.join("reference.mp4");
            success(
                Command::new(oracle.ffmpeg())
                    .args(["-nostdin", "-v", "error", "-y"])
                    .args(policy())
                    .arg("-i")
                    .arg(source)
                    .args(["-ac", "1", "-ar", "48000", "-c:a", "pcm_s16le", "-vn"])
                    .arg(&reference)
                    .output()
                    .unwrap(),
            );
            let expected = records(oracle.ffprobe(), &reference, &pcm);
            for strict in [false, true] {
                for existing in [false, true] {
                    let output = dir.join("native.mp4");
                    if existing {
                        std::fs::write(&output, b"replace me").unwrap();
                    } else {
                        let _ = std::fs::remove_file(&output);
                    }
                    success(native(source, &output, strict, &[]));
                    let actual = records(oracle.ffprobe(), &output, &pcm);
                    assert_eq!(actual, expected, "frames={frames}, source={source:?}");
                    let raw = dir.join("native.raw");
                    success(
                        Command::new(env!("CARGO_BIN_EXE_coreav"))
                            .args(["--strict-safe", "ffmpeg", "-y", "-i"])
                            .arg(source)
                            .args([
                                "-ac",
                                "1",
                                "-ar",
                                "48000",
                                "-c:a",
                                "pcm_s16le",
                                "-vn",
                                "-f",
                                "s16le",
                            ])
                            .arg(&raw)
                            .env("COREAV_FALLBACK", dir.join("missing-ffmpeg"))
                            .output()
                            .unwrap(),
                    );
                    assert_eq!(std::fs::read(raw).unwrap(), pcm);
                    let bytes = std::fs::read(output).unwrap();
                    let moov = child(&bytes, b"moov");
                    let track = child(moov, b"trak");
                    let tables = child(child(child(track, b"mdia"), b"minf"), b"stbl");
                    assert_eq!(
                        child(tables, b"stts"),
                        [0u32, 1, frames as u32, 1].map(u32::to_be_bytes).concat()
                    );
                    assert_eq!(
                        child(tables, b"stsz"),
                        [0u32, 2, frames as u32].map(u32::to_be_bytes).concat()
                    );
                    assert_eq!(
                        child(tables, b"stsc"),
                        [0u32, 1, 1, frames as u32, 1]
                            .map(u32::to_be_bytes)
                            .concat()
                    );
                    let description = child(tables, b"stsd");
                    let entry = child(&description[8..], b"ipcm");
                    let reference_bytes = std::fs::read(&reference).unwrap();
                    let reference_tables = child(
                        child(
                            child(child(child(&reference_bytes, b"moov"), b"trak"), b"mdia"),
                            b"minf",
                        ),
                        b"stbl",
                    );
                    let reference_description = child(reference_tables, b"stsd");
                    assert_eq!(entry, child(&reference_description[8..], b"ipcm"));
                    assert_eq!(child(&entry[28..], b"pcmC"), &[0, 0, 0, 0, 1, 16]);
                    assert_eq!(
                        child(&entry[28..], b"btrt"),
                        [0u32, 768_000, 768_000].map(u32::to_be_bytes).concat()
                    );
                    assert!(
                        !children(&entry[28..])
                            .iter()
                            .any(|(kind, _)| kind == b"chnl")
                    );
                    assert_eq!(
                        children(&bytes)
                            .iter()
                            .map(|(kind, _)| *kind)
                            .collect::<Vec<_>>(),
                        vec![*b"ftyp", *b"moov", *b"mdat"]
                    );
                    assert_eq!(children(moov).last().unwrap(), &(*b"free", &[][..]));
                    let offsets = child(tables, b"stco");
                    assert_eq!(&offsets[..8], &[0, 0, 0, 0, 0, 0, 0, 1]);
                    let offset = u32::from_be_bytes(offsets[8..12].try_into().unwrap()) as usize;
                    assert_eq!(&bytes[offset..], pcm);
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn excluded_shapes_preserve_destinations_and_original_command_fallback() {
    let oracle = test_oracle::require();
    let dir = scratch("refusals");
    let input = dir.join("source.wav");
    wave(&input, 4800);
    let standalone = dir.join("source.flac");
    let tagged = dir.join("tagged.wav");
    let stereo = dir.join("stereo.wav");
    let rate = dir.join("rate.wav");
    for (destination, options) in [
        (&standalone, vec!["-c:a", "flac"]),
        (
            &tagged,
            vec!["-c:a", "pcm_s16le", "-metadata", "title=keep"],
        ),
        (&stereo, vec!["-c:a", "pcm_s16le", "-ac", "2"]),
        (&rate, vec!["-c:a", "pcm_s16le", "-ar", "44100"]),
    ] {
        success(
            Command::new(oracle.ffmpeg())
                .args(["-nostdin", "-v", "error", "-y"])
                .args(policy())
                .arg("-i")
                .arg(&input)
                .args(options)
                .arg(destination)
                .output()
                .unwrap(),
        );
    }
    let output = dir.join("output.mp4");
    for source in [&standalone, &tagged, &stereo, &rate] {
        std::fs::write(&output, b"preserve destination").unwrap();
        let before = std::fs::read_dir(&dir).unwrap().count();
        let rejected = native(source, &output, true, &[]);
        assert_eq!(rejected.status.code(), Some(4), "{source:?}: {rejected:?}");
        assert!(rejected.stdout.is_empty());
        assert_eq!(std::fs::read(&output).unwrap(), b"preserve destination");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);
    }
    let stdout = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .current_dir(&dir)
        .args(["--strict-safe", "ffmpeg", "-i"])
        .arg(&input)
        .args([
            "-ac",
            "1",
            "-ar",
            "48000",
            "-c:a",
            "pcm_s16le",
            "-vn",
            "-f",
            "mp4",
            "-",
        ])
        .output()
        .unwrap();
    assert_eq!(stdout.status.code(), Some(4));
    assert!(stdout.stdout.is_empty());
    assert!(!dir.join("-").exists());
    let failed = native(&input, &dir.join("absent-parent/output.mp4"), false, &[]);
    assert_eq!(failed.status.code(), Some(3), "{failed:?}");
    assert!(failed.stdout.is_empty());
    let video = dir.join("video.mp4");
    success(
        Command::new(oracle.ffmpeg())
            .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi"])
            .args(policy())
            .args(["-i", "testsrc2=size=32x24:rate=10:duration=0.1"])
            .args(policy())
            .arg("-i")
            .arg(&input)
            .args(["-c:v", "libx264", "-threads:v", "1", "-c:a", "aac"])
            .arg(&video)
            .output()
            .unwrap(),
    );
    let reference = dir.join("fallback-reference.mp4");
    let original = |strict: bool, fallback: &Path, output: &Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
        if strict {
            command.arg("--strict-safe");
        }
        command
            .args(["ffmpeg", "-y", "-i"])
            .arg(&video)
            .args(["-ac", "1", "-ar", "48000", "-c:a", "pcm_s16le"])
            .arg(output)
            .env("COREAV_FALLBACK", fallback)
            .output()
            .unwrap()
    };
    std::fs::write(&output, b"video PCM destination").unwrap();
    let refused = original(true, oracle.ffmpeg(), &output);
    assert_eq!(refused.status.code(), Some(4));
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("PCM encoding to an unsupported container")
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"video PCM destination");
    let pin = Command::new(oracle.ffmpeg())
        .args(["-nostdin", "-v", "error", "-y"])
        .args(policy())
        .arg("-i")
        .arg(&video)
        .args(["-ac", "1", "-ar", "48000", "-c:a", "pcm_s16le"])
        .arg(&reference)
        .output()
        .unwrap();
    success(pin);
    let fallback = original(false, oracle.ffmpeg(), &output);
    assert!(fallback.status.success(), "{fallback:?}");
    assert_eq!(
        std::fs::read(&output).unwrap(),
        std::fs::read(reference).unwrap()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(feature = "aac")]
#[test]
fn aac_keeps_its_complete_decode_tolerance_and_exact_non_pcm_records() {
    let oracle = test_oracle::require();
    let dir = scratch("aac");
    let wave_input = dir.join("source.wav");
    wave(&wave_input, 52_817);
    let input = dir.join("aac.mp4");
    success(
        Command::new(oracle.ffmpeg())
            .args(["-nostdin", "-v", "error", "-y"])
            .args(policy())
            .arg("-i")
            .arg(&wave_input)
            .args(["-c:a", "aac", "-aac_pns", "0", "-aac_tns", "0"])
            .arg(&input)
            .output()
            .unwrap(),
    );
    let reference = dir.join("reference.mp4");
    success(
        Command::new(oracle.ffmpeg())
            .args(["-nostdin", "-v", "error", "-y"])
            .args(policy())
            .arg("-i")
            .arg(&input)
            .args(["-ac", "1", "-ar", "48000", "-c:a", "pcm_s16le", "-vn"])
            .arg(&reference)
            .output()
            .unwrap(),
    );
    let reference_bytes = std::fs::read(&reference).unwrap();
    let reference_pcm = child(&reference_bytes, b"mdat");
    let raw = dir.join("qualified.raw");
    success(
        Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffmpeg", "-y", "-i"])
            .arg(&input)
            .args([
                "-ac",
                "1",
                "-ar",
                "48000",
                "-c:a",
                "pcm_s16le",
                "-vn",
                "-f",
                "s16le",
            ])
            .arg(&raw)
            .env("COREAV_FALLBACK", dir.join("missing-ffmpeg"))
            .output()
            .unwrap(),
    );
    let native_pcm = std::fs::read(raw).unwrap();
    assert_eq!(native_pcm.len(), reference_pcm.len());
    assert!(!native_pcm.is_empty());
    for (actual, expected) in native_pcm
        .chunks_exact(2)
        .zip(reference_pcm.chunks_exact(2))
    {
        let actual = i32::from(i16::from_le_bytes(actual.try_into().unwrap()));
        let expected = i32::from(i16::from_le_bytes(expected.try_into().unwrap()));
        assert!((actual - expected).abs() <= 1);
    }
    for strict in [false, true] {
        let output = dir.join("native.mp4");
        success(native(&input, &output, strict, &[]));
        let mut actual = records(oracle.ffprobe(), &output, &native_pcm);
        let mut expected = records(oracle.ffprobe(), &reference, reference_pcm);
        for record in [&mut actual, &mut expected] {
            for packet in record["packets"].as_array_mut().unwrap() {
                let fields = packet.as_object_mut().unwrap();
                fields.remove("data");
                fields.remove("data_hash");
            }
        }
        assert_eq!(actual, expected);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
