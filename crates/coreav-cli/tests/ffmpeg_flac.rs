//! Bounded standalone FLAC input, complete PCM identity and safe publication.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use coreav_audio::{OutputFormat, SampleFormat, Sink};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[path = "../../coreav-audio/tests/support/flac_ranges.rs"]
mod flac_ranges;

fn scratch(name: &str) -> PathBuf {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("flac-source-{name}-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(bits: u16, channels: u16, frames: usize) -> (Vec<u8>, Vec<u8>) {
    let raw: Vec<u8> = (0..frames * usize::from(channels))
        .flat_map(|index| {
            let value = match index % 9 {
                0 => -(1 << (bits - 1)),
                1 => (1 << (bits - 1)) - 1,
                2 => -1,
                3 => 1,
                4 => 0,
                _ => (index as i32).wrapping_mul(1_073_741_789) >> (32 - bits),
            };
            value.to_le_bytes()[..usize::from(bits / 8)].to_vec()
        })
        .collect();
    let mut bytes = Vec::new();
    let mut encoder =
        coreav_audio::flac::encode::FlacSink::new(&mut bytes, coreav_audio::Limits::default());
    encoder
        .start(&OutputFormat {
            sample_rate: 48000,
            channels,
            channel_mask: if channels == 1 { 4 } else { 3 },
            format: if bits == 16 {
                SampleFormat::S16
            } else {
                SampleFormat::S24
            },
        })
        .unwrap();
    encoder.write(&raw).unwrap();
    let header = encoder.finish().unwrap();
    bytes[..header.len()].copy_from_slice(&header);
    (bytes, raw)
}

fn comments(mut bytes: Vec<u8>, fields: &[&str]) -> Vec<u8> {
    let mut payload = 3u32.to_le_bytes().to_vec();
    payload.extend_from_slice(b"own");
    payload.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    for field in fields {
        payload.extend_from_slice(&(field.len() as u32).to_le_bytes());
        payload.extend_from_slice(field.as_bytes());
    }
    let mut block = vec![0x84];
    block.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
    block.extend(payload);
    bytes[4] &= 127;
    bytes.splice(42..42, block);
    bytes
}

fn strict() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command.args(["--strict-safe", "ffmpeg"]);
    command
}

fn piped(command: &mut Command, bytes: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let payload = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&payload).unwrap());
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    output
}

fn checksum(bytes: &[u8], bits: u8, polynomial: u16) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << (bits - 8);
        for _ in 0..8 {
            let high = crc & (1 << (bits - 1));
            crc = crc.wrapping_shl(1) ^ if high != 0 { polynomial } else { 0 };
        }
    }
    if bits == 8 { crc & 255 } else { crc }
}

fn success(result: &Output) {
    assert!(result.status.success(), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
}

fn pin() -> Option<PathBuf> {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    if !ffmpeg.is_file() {
        assert!(
            std::env::var_os("CI").is_none() && std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none(),
            "set COREAV_FFMPEG to the pin"
        );
        return None;
    }
    let verified = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/oracle_pin.py"))
        .arg("--verify-bin")
        .arg(ffmpeg.parent().unwrap())
        .output()
        .unwrap();
    assert!(verified.status.success(), "{verified:?}");
    Some(ffmpeg)
}

#[test]
fn complete_raw_bytes_match_the_pin_for_files_stdout_and_stdin() {
    let Some(ffmpeg) = pin() else {
        return;
    };
    let dir = scratch("identity");
    let input = dir.join("input.flac");
    let output = dir.join("output.pcm");
    for bits in [16, 24] {
        for channels in [1, 2] {
            for frames in [1, 4096, 12305] {
                let (bytes, raw) = fixture(bits, channels, frames);
                let bytes = comments(bytes, &["TITLE=independent fixture", "language=en"]);
                std::fs::write(&input, &bytes).unwrap();
                for muxer in ["s16le", "f32le"] {
                    let reference = Command::new(&ffmpeg)
                        .args(["-v", "error", "-i"])
                        .arg(&input)
                        .args(["-f", muxer, "pipe:1"])
                        .output()
                        .unwrap();
                    success(&reference);
                    assert_eq!(
                        reference.stdout.len(),
                        frames * usize::from(channels) * if muxer == "s16le" { 2 } else { 4 }
                    );
                    if bits == 16 && muxer == "s16le" {
                        assert_eq!(reference.stdout, raw);
                    }
                    let native = strict()
                        .args(["-threads", "auto", "-i"])
                        .arg(&input)
                        .args(["-f", muxer, "-"])
                        .output()
                        .unwrap();
                    success(&native);
                    assert_eq!(
                        native.stdout, reference.stdout,
                        "{bits}/{channels}/{frames}/{muxer}"
                    );
                    let native = strict()
                        .args(["-y", "-i"])
                        .arg(&input)
                        .args(["-f", muxer])
                        .arg(&output)
                        .output()
                        .unwrap();
                    success(&native);
                    assert!(native.stdout.is_empty());
                    assert_eq!(std::fs::read(&output).unwrap(), reference.stdout);
                    let native = piped(
                        strict().args(["-threads", "2", "-i", "pipe:0", "-f", muxer, "pipe:1"]),
                        &bytes,
                    );
                    success(&native);
                    assert_eq!(native.stdout, reference.stdout);
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corrupt_late_payloads_never_publish_or_delegate() {
    let dir = scratch("integrity");
    let input = dir.join("input.flac");
    let output = dir.join("output.pcm");
    let (valid, _) = fixture(24, 2, 12305);
    let mut cases = Vec::new();
    let info = coreav_audio::flac::StreamInfo::parse(&valid[8..42]).unwrap();
    let mut decoder =
        coreav_audio::flac::FlacDecoder::new(info, &coreav_audio::Limits::default()).unwrap();
    let mut pcm = coreav_audio::Pcm::new();
    let second = 42 + decoder.decode(&valid[42..], &mut pcm).unwrap().bytes;
    let length = decoder.decode(&valid[second..], &mut pcm).unwrap().bytes;
    let mut changed = valid.clone();
    let frame = &mut changed[second..second + length];
    assert_eq!(checksum(&frame[..7], 8, 7) as u8, frame[7]);
    assert_eq!(
        checksum(&frame[..length - 2], 16, 0x8005).to_be_bytes(),
        frame[length - 2..]
    );
    frame[4] = 4;
    frame[7] = checksum(&frame[..7], 8, 7) as u8;
    let crc = checksum(&frame[..length - 2], 16, 0x8005).to_be_bytes();
    frame[length - 2..].copy_from_slice(&crc);
    assert!(decoder.decode(frame, &mut pcm).unwrap().crc_ok);
    cases.push(changed);
    let mut changed = valid.clone();
    *changed.last_mut().unwrap() ^= 1;
    cases.push(changed);
    let mut changed = valid.clone();
    changed.truncate(changed.len() - 3);
    cases.push(changed);
    let mut changed = valid.clone();
    changed.extend_from_slice(b"trailing");
    cases.push(changed);
    let mut changed = valid.clone();
    changed[26] ^= 1;
    cases.push(changed);
    let mut changed = valid.clone();
    changed[25] ^= 1;
    cases.push(changed);
    let mut changed = valid.clone();
    changed[7] = 35;
    cases.push(changed);
    for bytes in cases {
        std::fs::write(&input, &bytes).unwrap();
        std::fs::write(&output, b"keep").unwrap();
        for stdout in [false, true] {
            let result = strict()
                .args(["-y", "-i"])
                .arg(&input)
                .args(["-f", "s16le"])
                .arg(if stdout { Path::new("pipe:1") } else { &output })
                .output()
                .unwrap();
            assert!(!result.status.success(), "{result:?}");
            assert_ne!(
                result.status.code(),
                Some(4),
                "integrity errors must stay terminal: {result:?}"
            );
            assert!(result.stdout.is_empty());
            assert_eq!(std::fs::read(&output).unwrap(), b"keep");
        }
        let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["ffmpeg", "-y", "-i"])
            .arg(&input)
            .args(["-f", "s16le"])
            .arg(&output)
            .env_remove("COREAV_STRICT_SAFE")
            .env("COREAV_FALLBACK", dir.join("must-not-run"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("must-not-run"));
        assert_eq!(std::fs::read(&output).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pinned_encoded_frames_and_zero_md5_use_complete_exact_output() {
    let Some(ffmpeg) = pin() else {
        return;
    };
    let dir = scratch("encoded-frames");
    let raw = dir.join("input.pcm");
    let input = dir.join("input.flac");
    for bits in [16, 24] {
        for channels in [1, 2] {
            std::fs::write(&raw, fixture(bits, channels, 9233).1).unwrap();
            let encoded = Command::new(&ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    if bits == 16 { "s16le" } else { "s24le" },
                    "-ar",
                    "48000",
                    "-ac",
                ])
                .arg(channels.to_string())
                .arg("-i")
                .arg(&raw)
                .args(["-c:a", "flac"])
                .arg(&input)
                .output()
                .unwrap();
            success(&encoded);
            let encoded = std::fs::read(&input).unwrap();
            for zero_md5 in [false, true] {
                let mut bytes = encoded.clone();
                if zero_md5 {
                    bytes[26..42].fill(0);
                }
                std::fs::write(&input, bytes).unwrap();
                for (muxer, codec) in [("s16le", "pcm_s16le"), ("f32le", "pcm_f32le")] {
                    let reference = Command::new(&ffmpeg)
                        .args(["-v", "error", "-i"])
                        .arg(&input)
                        .args(["-c:a", codec, "-f", muxer, "pipe:1"])
                        .output()
                        .unwrap();
                    success(&reference);
                    let native = strict()
                        .arg("-i")
                        .arg(&input)
                        .args(["-vn", "-c:a", codec, "-f", muxer, "pipe:1"])
                        .output()
                        .unwrap();
                    success(&native);
                    assert_eq!(native.stdout, reference.stdout);
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn oversized_payload_is_a_terminal_limit_without_staging() {
    let dir = scratch("payload-cap");
    let input = dir.join("input.flac");
    let output = dir.join("output.pcm");
    let (mut bytes, _) = fixture(16, 1, 4099);
    bytes.resize(42 + (1 << 20) + 1, 0);
    std::fs::write(&input, bytes).unwrap();
    std::fs::write(&output, b"keep").unwrap();
    let result = strict()
        .args(["-y", "-i"])
        .arg(&input)
        .args(["-f", "s16le"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_ne!(result.status.code(), Some(4));
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("limit BoxBytes exceeded at offset 42"),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_sources_refuse_and_replay_the_real_fallback() {
    let Some(ffmpeg) = pin() else {
        return;
    };
    let dir = scratch("fallback");
    let input = dir.join("input.flac");
    let (valid, _) = fixture(16, 1, 4099);
    let mut prefixed = b"ID3\x04\0\0\0\0\0\0".to_vec();
    prefixed.extend_from_slice(&valid);
    let mut unknown = valid.clone();
    unknown[21] &= 0xf0;
    unknown[22..26].fill(0);
    for bytes in [
        comments(valid.clone(), &["WAVEFORMATEXTENSIBLE_CHANNEL_MASK=0x4"]),
        prefixed,
        unknown,
    ] {
        std::fs::write(&input, &bytes).unwrap();
        let refused = strict()
            .arg("-i")
            .arg(&input)
            .args(["-f", "s16le", "pipe:1"])
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(4), "{refused:?}");
        assert!(refused.stdout.is_empty());
        let reference = Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&input)
            .args(["-f", "s16le", "pipe:1"])
            .output()
            .unwrap();
        success(&reference);
        let fallback = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["ffmpeg", "-v", "error", "-i"])
            .arg(&input)
            .args(["-f", "s16le", "pipe:1"])
            .env_remove("COREAV_STRICT_SAFE")
            .env("COREAV_FALLBACK", &ffmpeg)
            .output()
            .unwrap();
        assert_eq!(fallback.status.code(), reference.status.code());
        assert_eq!(fallback.stdout, reference.stdout);
        let refused = piped(
            strict().args(["-i", "pipe:0", "-f", "s16le", "pipe:1"]),
            &bytes,
        );
        assert_eq!(refused.status.code(), Some(4), "{refused:?}");
        assert!(refused.stdout.is_empty());
        let fallback = piped(
            Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args([
                    "ffmpeg", "-v", "error", "-i", "pipe:0", "-f", "s16le", "pipe:1",
                ])
                .env_remove("COREAV_STRICT_SAFE")
                .env("COREAV_FALLBACK", &ffmpeg),
            &bytes,
        );
        assert_eq!(fallback.status.code(), reference.status.code());
        assert_eq!(fallback.stdout, reference.stdout);
    }
    std::fs::write(&input, valid).unwrap();
    for options in [["-ar", "16000"], ["-ac", "2"]] {
        let refused = strict()
            .arg("-i")
            .arg(&input)
            .args(options)
            .args(["-f", "s16le", "pipe:1"])
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(4), "{refused:?}");
        assert!(refused.stdout.is_empty());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn broken_stdout_is_quiet_and_never_delegates() {
    let dir = scratch("broken-pipe");
    let input = dir.join("input.flac");
    std::fs::write(&input, fixture(24, 2, 12305).0).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["ffmpeg", "-i"])
        .arg(&input)
        .args(["-f", "f32le", "pipe:1"])
        .env_remove("COREAV_STRICT_SAFE")
        .env("COREAV_FALLBACK", dir.join("must-not-run"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let result = child.wait_with_output().unwrap();
    assert!(result.stderr.is_empty(), "{result:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_reconstructed_ranges_refuse_before_any_output() {
    let Some(ffmpeg) = pin() else {
        return;
    };
    let dir = scratch("sample-ranges");
    let input = dir.join("input.flac");
    let output = dir.join("output.pcm");
    for bits in [16, 24] {
        let half = 1i64 << (bits - 1);
        for (assignment, values, fixed) in [
            (0, [half, 0], true),
            (8, [half - 1, -2 * half], false),
            (9, [2 * half - 1, half - 1], false),
            (10, [half - 1, 2 * half - 1], false),
        ] {
            for late in [false, true] {
                let bytes = flac_ranges::file(bits, assignment, values, fixed, late);
                std::fs::write(&input, &bytes).unwrap();
                for muxer in ["s16le", "f32le"] {
                    let reference = Command::new(&ffmpeg)
                        .args(["-v", "error", "-i"])
                        .arg(&input)
                        .args(["-f", muxer, "pipe:1"])
                        .output()
                        .unwrap();
                    assert!(reference.status.success(), "{reference:?}");
                    assert!(!reference.stdout.is_empty());
                    if bits == 16 && assignment == 0 && !late && muxer == "f32le" {
                        assert_eq!(reference.stdout, (-1.0f32).to_le_bytes().repeat(16));
                    }
                    for stdout in [false, true] {
                        std::fs::write(&output, b"keep").unwrap();
                        let native = strict()
                            .args(["-y", "-i"])
                            .arg(&input)
                            .args(["-f", muxer])
                            .arg(if stdout { Path::new("pipe:1") } else { &output })
                            .output()
                            .unwrap();
                        assert!(
                            !native.status.success(),
                            "{bits}/{assignment}/{late}/{muxer}: {native:?}"
                        );
                        assert_ne!(native.status.code(), Some(4));
                        assert!(native.stdout.is_empty());
                        assert!(
                            String::from_utf8_lossy(&native.stderr).contains("outside"),
                            "{native:?}"
                        );
                        assert_eq!(std::fs::read(&output).unwrap(), b"keep");
                        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
                    }
                    std::fs::remove_file(&output).unwrap();
                    let missing = strict()
                        .args(["-y", "-i"])
                        .arg(&input)
                        .args(["-f", muxer])
                        .arg(&output)
                        .output()
                        .unwrap();
                    assert!(!missing.status.success());
                    assert!(missing.stdout.is_empty());
                    assert!(!output.exists());
                    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
                    std::fs::write(&output, b"keep").unwrap();
                    let normal = Command::new(env!("CARGO_BIN_EXE_coreav"))
                        .args(["ffmpeg", "-y", "-i"])
                        .arg(&input)
                        .args(["-f", muxer])
                        .arg(&output)
                        .env_remove("COREAV_STRICT_SAFE")
                        .env("COREAV_FALLBACK", dir.join("must-not-run"))
                        .output()
                        .unwrap();
                    assert!(!normal.status.success());
                    assert_ne!(normal.status.code(), Some(4));
                    assert!(normal.stdout.is_empty());
                    assert!(!String::from_utf8_lossy(&normal.stderr).contains("must-not-run"));
                    assert_eq!(std::fs::read(&output).unwrap(), b"keep");
                    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn valid_reconstructed_range_boundaries_match_complete_pinned_bytes() {
    let Some(ffmpeg) = pin() else {
        return;
    };
    let dir = scratch("range-boundaries");
    let input = dir.join("input.flac");
    for bits in [16, 24] {
        let half = 1i64 << (bits - 1);
        for (assignment, values, fixed) in [
            (0, [half - 1, 0], true),
            (0, [-half, 0], true),
            (8, [half - 1, 2 * half - 1], false),
            (9, [2 * half - 1, -half], false),
            (10, [-1, 2 * half - 1], false),
        ] {
            for late in [false, true] {
                std::fs::write(
                    &input,
                    flac_ranges::file(bits, assignment, values, fixed, late),
                )
                .unwrap();
                for muxer in ["s16le", "f32le"] {
                    let reference = Command::new(&ffmpeg)
                        .args(["-v", "error", "-i"])
                        .arg(&input)
                        .args(["-f", muxer, "pipe:1"])
                        .output()
                        .unwrap();
                    success(&reference);
                    let native = strict()
                        .arg("-i")
                        .arg(&input)
                        .args(["-f", muxer, "pipe:1"])
                        .output()
                        .unwrap();
                    success(&native);
                    assert_eq!(
                        native.stdout, reference.stdout,
                        "{bits}/{assignment}/{late}/{muxer}"
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}
