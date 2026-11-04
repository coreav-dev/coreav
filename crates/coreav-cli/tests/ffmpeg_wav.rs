//! Bounded standalone WAVE PCM input and the shared fallback lifecycle.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn scratch(name: &str) -> PathBuf {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("wav-source-{name}-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn wav(code: u16, bits: u16, channels: u16, samples: &[u8]) -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0");
    bytes.extend_from_slice(&code.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&48000u32.to_le_bytes());
    let align = channels * (bits / 8);
    bytes.extend_from_slice(&(48000 * u32::from(align)).to_le_bytes());
    bytes.extend_from_slice(&align.to_le_bytes());
    bytes.extend_from_slice(&bits.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    bytes.extend_from_slice(samples);
    bytes
}

fn strict() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command.args(["--strict-safe", "ffmpeg"]);
    command
}

#[test]
fn wav_files_and_encoded_stdin_preserve_packing_without_decoder_workers() {
    let dir = scratch("identity");
    let samples: Vec<u8> = [-32768i16, 32767, -1, 1, 17, -29]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect();
    let encoded = wav(1, 16, 2, &samples);
    let input = dir.join("input.wav");
    std::fs::write(&input, &encoded).unwrap();
    let floats: Vec<u8> = samples
        .chunks_exact(2)
        .flat_map(|bytes| {
            (f32::from(i16::from_le_bytes(bytes.try_into().unwrap())) / 32768.0).to_le_bytes()
        })
        .collect();
    for (form, expected) in [("s16le", &samples), ("f32le", &floats)] {
        let output = strict()
            .args(["-threads", "4294967295", "-i"])
            .arg(&input)
            .args(["-map", "0:a", "-f", form, "pipe:1"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(&output.stdout, expected);
        assert!(output.stderr.is_empty());
        let temporary = dir.join(format!("spool-{form}"));
        std::fs::create_dir(&temporary).unwrap();
        let mut child = strict()
            .args(["-i", "pipe:0", "-f", form, "pipe:1"])
            .env("TMPDIR", &temporary)
            .env("TMP", &temporary)
            .env("TEMP", &temporary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&encoded).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(&output.stdout, expected);
        assert!(output.stderr.is_empty());
        assert_eq!(std::fs::read_dir(&temporary).unwrap().count(), 0);
    }
}

#[test]
fn float_wav_identity_preserves_bits_but_other_compositions_refuse() {
    let dir = scratch("float");
    let samples: Vec<u8> = [0u32, 0x80000000, 1, 0x80000001, 0x7f7fffff, 0xff7fffff]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let input = dir.join("input.wav");
    std::fs::write(&input, wav(3, 32, 2, &samples)).unwrap();
    let output = strict()
        .arg("-i")
        .arg(&input)
        .args(["-f", "f32le", "pipe:1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, samples);
    for args in [vec!["-f", "s16le"], vec!["-ar", "16000", "-f", "f32le"]] {
        let output = strict()
            .arg("-i")
            .arg(&input)
            .args(args)
            .arg("pipe:1")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn unregistered_extents_do_not_create_or_replace_output() {
    let dir = scratch("extents");
    let complete = wav(1, 16, 1, &[0; 8]);
    let mut empty_tail = wav(1, 16, 1, &[]);
    empty_tail.extend_from_slice(b"JUNK\x02\0\0\0\0\0");
    let len = empty_tail.len() as u32 - 8;
    empty_tail[4..8].copy_from_slice(&len.to_le_bytes());
    for (name, bytes) in [
        ("truncated", complete[..complete.len() - 2].to_vec()),
        ("empty-tail", empty_tail),
    ] {
        let input = dir.join(format!("{name}.wav"));
        let destination = dir.join(format!("{name}.raw"));
        std::fs::write(&input, bytes).unwrap();
        let output = strict()
            .arg("-i")
            .arg(&input)
            .args(["-f", "s16le"])
            .arg(&destination)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert!(!destination.exists());
        std::fs::write(&destination, b"preserve").unwrap();
        let output = strict()
            .args(["-y", "-i"])
            .arg(&input)
            .args(["-f", "s16le"])
            .arg(&destination)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"preserve");
    }
}

#[test]
fn wav_output_cannot_alias_its_input() {
    let dir = scratch("alias");
    let input = dir.join("input.wav");
    let original = wav(1, 16, 1, &[0; 8]);
    std::fs::write(&input, &original).unwrap();
    let output = strict()
        .args(["-y", "-i"])
        .arg(&input)
        .args(["-f", "s16le"])
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn unsupported_probe_replays_original_stdin_but_malformed_headers_do_not() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("fallback");
    let fallback = dir.join("ffmpeg");
    let record = dir.join("ran");
    std::fs::write(
        &fallback,
        "#!/bin/sh\nprintf ran > \"$WAVE_FALLBACK_RECORD\"\ncat\nexit 7\n",
    )
    .unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut unsupported = wav(1, 16, 1, &[0; 8]);
    unsupported.splice(36..36, b"bext\x04\0\0\0test".iter().copied());
    let len = unsupported.len() as u32 - 8;
    unsupported[4..8].copy_from_slice(&len.to_le_bytes());
    let mut malformed = wav(1, 16, 1, &[0; 8]);
    malformed[22..24].copy_from_slice(&0u16.to_le_bytes());
    for (bytes, expected) in [(unsupported, 7), (malformed, 1)] {
        let _ = std::fs::remove_file(&record);
        let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["ffmpeg", "-i", "pipe:0", "-f", "s16le", "pipe:1"])
            .env_remove("COREAV_STRICT_SAFE")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .env("COREAV_FALLBACK", &fallback)
            .env("WAVE_FALLBACK_RECORD", &record)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(expected), "{output:?}");
        if expected == 7 {
            assert_eq!(output.stdout, bytes);
            assert!(record.exists());
        } else {
            assert!(output.stdout.is_empty());
            assert!(!record.exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn wave_stdout_checks_the_final_flush_and_keeps_epipe_quiet() {
    let mut child = strict()
        .args(["-i", "pipe:0", "-f", "s16le", "pipe:1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&wav(1, 16, 1, &[0; 8]))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(141), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}
