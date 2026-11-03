//! Explicit raw PCM option scope, sample preservation and atomic output.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let path =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("raw-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run(input: &Path, output: &Path, before: &[&str], after: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["--strict-safe", "ffmpeg"])
        .args(before)
        .arg("-i")
        .arg(input)
        .args(after)
        .arg(output)
        .output()
        .unwrap()
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_raw_input_preserves_integer_bytes_and_wav_fields() {
    let dir = scratch("identity");
    let input = dir.join("input.bin");
    let bytes: Vec<u8> = (0..100_000u32)
        .flat_map(|n| (n.wrapping_mul(73) as i16).to_le_bytes())
        .collect();
    std::fs::write(&input, &bytes).unwrap();
    let before = ["-ar", "44.1k", "-ac", "2", "-f", "s16le"];
    let wav = dir.join("output.wav");
    success(run(
        &input,
        &wav,
        &before,
        &["-ar", "0.0441M", "-ac", "2", "-threads", "2"],
    ));
    let output = std::fs::read(&wav).unwrap();
    assert_eq!(&output[..4], b"RIFF");
    assert_eq!(&output[8..12], b"WAVE");
    assert_eq!(
        u32::from_le_bytes(output[4..8].try_into().unwrap()) as usize,
        output.len() - 8
    );
    assert_eq!(u16::from_le_bytes(output[20..22].try_into().unwrap()), 1);
    assert_eq!(u16::from_le_bytes(output[22..24].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(output[24..28].try_into().unwrap()),
        44100
    );
    assert_eq!(
        u32::from_le_bytes(output[28..32].try_into().unwrap()),
        44100 * 4
    );
    assert_eq!(u16::from_le_bytes(output[32..34].try_into().unwrap()), 4);
    assert_eq!(u16::from_le_bytes(output[34..36].try_into().unwrap()), 16);
    assert_eq!(&output[36..40], b"data");
    assert_eq!(
        u32::from_le_bytes(output[40..44].try_into().unwrap()) as usize,
        bytes.len()
    );
    assert_eq!(&output[44..], bytes);
    let raw = dir.join("output.raw");
    success(run(
        &input,
        &raw,
        &before,
        &["-f", "s16le", "-c:a", "pcm_s16le"],
    ));
    assert_eq!(std::fs::read(raw).unwrap(), bytes);
}

#[test]
fn short_raw_inputs_keep_the_compatibility_startup_and_drain_lengths() {
    let dir = scratch("short");
    for (frames, incoming, outgoing, expected) in [
        (0, 44100, 48000, 0),
        (21, 44100, 48000, 0),
        (22, 44100, 48000, 19),
        (100, 48000, 16000, 34),
    ] {
        let input = dir.join(format!("{frames}.pcm"));
        let output = dir.join(format!("{frames}.raw"));
        std::fs::write(&input, [0u8, 32].repeat(frames)).unwrap();
        success(run(
            &input,
            &output,
            &["-f", "s16le", "-ar", &incoming.to_string(), "-ac", "1"],
            &["-ar", &outgoing.to_string(), "-f", "s16le"],
        ));
        assert_eq!(std::fs::read(output).unwrap(), [0u8, 32].repeat(expected));
    }
}

#[test]
fn high_rate_raw_wav_publishes_its_layout_without_changing_samples() {
    let dir = scratch("high-rate");
    for (channels, mask) in [(1u16, 4u32), (2, 3)] {
        let input = dir.join(format!("input-{channels}.pcm"));
        let output = dir.join(format!("output-{channels}.wav"));
        let samples: Vec<u8> = (0..257 * channels)
            .flat_map(|n| (n as i16 - 128).to_le_bytes())
            .collect();
        std::fs::write(&input, &samples).unwrap();
        success(run(
            &input,
            &output,
            &["-f", "s16le", "-ar", "96000", "-ac", &channels.to_string()],
            &[],
        ));
        let bytes = std::fs::read(output).unwrap();
        assert_eq!(
            u16::from_le_bytes(bytes[20..22].try_into().unwrap()),
            0xFFFE
        );
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), mask);
        assert_eq!(
            u32::from_le_bytes(bytes[64..68].try_into().unwrap()) as usize,
            samples.len()
        );
        assert_eq!(&bytes[68..], samples);
    }
}

#[test]
fn unsupported_raw_options_never_replace_an_output() {
    let dir = scratch("refusals");
    let input = dir.join("input.pcm");
    let output = dir.join("output.wav");
    std::fs::write(&input, [0u8; 400]).unwrap();
    std::fs::write(&output, b"keep").unwrap();
    for after in [
        vec!["-c", "copy"],
        vec!["-c:a", "aac"],
        vec!["-c:a", "flac"],
        vec!["-ac", "1"],
        vec!["-ar", "44099"],
        vec!["-ar", "44100.5"],
        vec!["-map", "0:a"],
        vec!["-ss", "0"],
        vec!["-t", "0"],
        vec!["-af", "volume=2"],
        vec!["-f", "mp4"],
        vec!["-f", "flac"],
        vec!["-an"],
    ] {
        let mut options = vec!["-y"];
        options.extend(after);
        let result = run(
            &input,
            &output,
            &["-f", "s16le", "-ar", "44100", "-ac", "2"],
            &options,
        );
        assert_eq!(
            result.status.code(),
            Some(4),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"keep");
    }
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 2);
}

#[test]
fn framing_and_output_identity_fail_without_publishing_a_temporary_file() {
    let dir = scratch("atomic");
    let input = dir.join("input.pcm");
    let output = dir.join("output.wav");
    let before = ["-f", "s16le", "-ar", "44100", "-ac", "2"];
    std::fs::write(&input, [0u8; 3]).unwrap();
    std::fs::write(&output, b"keep").unwrap();
    assert!(!run(&input, &output, &before, &["-y"]).status.success());
    assert_eq!(std::fs::read(&output).unwrap(), b"keep");
    std::fs::write(&input, [0u8; 4]).unwrap();
    assert_eq!(
        run(&input, &output, &before, &["-n"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run(&input, &input, &before, &["-y", "-f", "wav"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(std::fs::read(&input).unwrap(), [0u8; 4]);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    success(run(&input, &output, &before, &["-y"]));
    assert_eq!(&std::fs::read(output).unwrap()[44..], [0u8; 4]);
}

#[test]
#[cfg(unix)]
fn raw_output_uses_the_shared_owners_unix_hardlink_protection() {
    let dir = scratch("hardlink");
    let input = dir.join("input.pcm");
    let alias = dir.join("alias.pcm");
    std::fs::write(&input, [0u8; 4]).unwrap();
    std::fs::hard_link(&input, &alias).unwrap();
    assert_eq!(
        run(
            &input,
            &alias,
            &["-f", "s16le", "-ar", "44100", "-ac", "2"],
            &["-y", "-f", "wav"]
        )
        .status
        .code(),
        Some(2)
    );
    assert_eq!(std::fs::read(&input).unwrap(), [0u8; 4]);
    assert_eq!(std::fs::read(&alias).unwrap(), [0u8; 4]);
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 2);
}

#[test]
fn raw_declarations_are_explicit_and_do_not_change_container_input_options() {
    let dir = scratch("declarations");
    let input = dir.join("absent.pcm");
    let output = dir.join("absent.wav");
    for before in [
        vec!["-f", "s16le", "-ar", "44100"],
        vec!["-f", "s16le", "-ac", "2"],
        vec!["-ar", "44100", "-ac", "2"],
        vec!["-f", "mp4", "-ar", "44100"],
        vec!["-f", "s16le", "-ar", "NaN", "-ac", "2"],
        vec!["-f", "s16le", "-ar", "500", "-ac", "2"],
        vec!["-f", "s16le", "-ar", "44100", "-ac", "3"],
    ] {
        assert_eq!(run(&input, &output, &before, &[]).status.code(), Some(4));
        assert!(!output.exists());
    }
}
