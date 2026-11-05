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

fn info_tag(tag: &[u8; 4], text: &[u8]) -> Vec<u8> {
    let mut payload = b"INFO".to_vec();
    payload.extend_from_slice(tag);
    payload.extend_from_slice(&(text.len() as u32 + 1).to_le_bytes());
    payload.extend_from_slice(text);
    payload.push(0);
    if payload.len() % 2 != 0 {
        payload.push(0);
    }
    let mut chunk = b"LIST".to_vec();
    chunk.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    chunk.extend(payload);
    chunk
}

#[test]
fn integer_wav_output_matches_complete_pinned_files() {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    if !ffmpeg.is_file() {
        assert!(
            std::env::var_os("CI").is_none() && std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none(),
            "set COREAV_FFMPEG to the pin"
        );
        return;
    }
    let verified = Command::new("python3")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/oracle_pin.py"))
        .arg("--verify-bin")
        .arg(ffmpeg.parent().unwrap())
        .output()
        .unwrap();
    assert!(verified.status.success(), "{verified:?}");
    let dir = scratch("output-oracle");
    for bits in [8u16, 16, 24, 32] {
        for channels in [1u16, 2] {
            for frames in [0, 3, 40001] {
                let samples: Vec<u8> = (0..frames * usize::from(channels))
                    .flat_map(|n| {
                        let value = match n % 7 {
                            0 => i32::MIN,
                            1 => i32::MAX,
                            2 => 0,
                            _ => (n as i32).wrapping_mul(1_073_741_789),
                        };
                        if bits == 8 {
                            vec![((value >> 24) + 128) as u8]
                        } else {
                            (value >> (32 - bits)).to_le_bytes()[..usize::from(bits / 8)].to_vec()
                        }
                    })
                    .collect();
                let mut source = wav(1, bits, channels, &samples);
                let info = info_tag(b"ISFT", b"Input encoder");
                source.splice(36..36, info);
                let size = source.len() as u32 - 8;
                source[4..8].copy_from_slice(&size.to_le_bytes());
                let input = dir.join("input.wav");
                std::fs::write(&input, source).unwrap();
                for explicit in [false, true] {
                    let candidate = dir.join("candidate.wav");
                    let reference = dir.join("reference.wav");
                    let count = channels.to_string();
                    let mut args = vec![
                        "-y",
                        "-i",
                        input.to_str().unwrap(),
                        "-ac",
                        &count,
                        "-ar",
                        "48000",
                        "-f",
                        "wav",
                    ];
                    if explicit {
                        args.extend(["-acodec", "pcm_s16le"]);
                    }
                    let pin = Command::new(&ffmpeg)
                        .args(&args)
                        .arg(&reference)
                        .output()
                        .unwrap();
                    assert!(pin.status.success(), "{pin:?}");
                    let native = strict()
                        .args(&args)
                        .arg(&candidate)
                        .env("COREAV_FALLBACK", dir.join("missing-ffmpeg"))
                        .output()
                        .unwrap();
                    assert!(native.status.success(), "{native:?}");
                    assert!(native.stdout.is_empty() && native.stderr.is_empty());
                    assert_eq!(
                        std::fs::read(&candidate).unwrap(),
                        std::fs::read(&reference).unwrap(),
                        "{bits} bits, {channels} channels, {frames} frames, explicit={explicit}"
                    );
                }
            }
        }
    }
    for rate in [16000u32, 96000] {
        let mut source = wav(1, 16, 1, &[0, 128, 255, 127]);
        source[24..28].copy_from_slice(&rate.to_le_bytes());
        source[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
        source.splice(36..36, info_tag(b"INAM", b"Discard this title"));
        let size = source.len() as u32 - 8;
        source[4..8].copy_from_slice(&size.to_le_bytes());
        let input = dir.join("input.wav");
        std::fs::write(&input, source).unwrap();
        let args = ["-y", "-i", input.to_str().unwrap(), "-map_metadata", "-1"];
        let reference = dir.join("reference.wav");
        let pin = Command::new(&ffmpeg)
            .args(args)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(pin.status.success(), "{pin:?}");
        let candidate = dir.join("candidate.wav");
        let native = strict().args(args).arg(&candidate).output().unwrap();
        assert!(native.status.success(), "{native:?}");
        assert!(native.stdout.is_empty() && native.stderr.is_empty());
        assert_eq!(
            std::fs::read(candidate).unwrap(),
            std::fs::read(reference).unwrap()
        );
    }
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
        let source = dir.join("fifo-input.wav");
        std::fs::write(&source, wav(1, 16, 1, &[0; 6])).unwrap();
        let mut payloads = Vec::new();
        for fallback in [false, true] {
            let fifo = dir.join(if fallback { "fallback.wav" } else { "pin.wav" });
            assert!(
                Command::new("mkfifo")
                    .arg(&fifo)
                    .status()
                    .unwrap()
                    .success()
            );
            let mut reader = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().try_into().unwrap())
                .open(&fifo)
                .unwrap();
            let mut command = if fallback {
                let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
                command.arg("ffmpeg");
                command.env("COREAV_FALLBACK", &ffmpeg);
                command.env_remove("COREAV_STRICT_SAFE");
                command.env_remove("COREAV_FALLBACK_ACTIVE");
                command
            } else {
                Command::new(&ffmpeg)
            };
            let result = command
                .args(["-y", "-i"])
                .arg(&source)
                .args(["-f", "wav"])
                .arg(&fifo)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            assert!(result.stdout.is_empty());
            assert!(std::fs::metadata(&fifo).unwrap().file_type().is_fifo());
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            assert_eq!(&bytes[4..8], &[255; 4]);
            payloads.push(bytes);
        }
        assert_eq!(payloads[0], payloads[1]);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn wav_non_regular_destinations_refuse_without_staging_or_replacement() {
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::os::unix::net::UnixListener;
    let dir = scratch("special-destinations");
    let input = dir.join("input.wav");
    std::fs::write(&input, wav(1, 16, 1, &[0; 6])).unwrap();
    let fifo = dir.join("fifo.wav");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let socket = dir.join("socket.wav");
    let _listener = UnixListener::bind(&socket).unwrap();
    let directory = dir.join("directory.wav");
    std::fs::create_dir(&directory).unwrap();
    let device_link = dir.join("device.wav");
    symlink("/dev/null", &device_link).unwrap();
    let fifo_link = dir.join("fifo-link.wav");
    symlink(&fifo, &fifo_link).unwrap();
    for destination in [&fifo, &socket, &directory, &device_link, &fifo_link] {
        let original = std::fs::symlink_metadata(destination).unwrap();
        let entries = std::fs::read_dir(&dir).unwrap().count();
        let result = strict()
            .args(["-y", "-i"])
            .arg(&input)
            .arg(destination)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(4), "{result:?}");
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains("non-regular destination"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
        let after = std::fs::symlink_metadata(destination).unwrap();
        assert_eq!(original.file_type(), after.file_type());
        assert_eq!(original.ino(), after.ino());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unregistered_wav_output_preserves_destinations() {
    let dir = scratch("output-refusals");
    let input = dir.join("input.wav");
    let destination = dir.join("output.wav");
    let integer = wav(1, 16, 1, &[0; 12]);
    let float = wav(3, 32, 1, &[0; 16]);
    let mut tagged = integer.clone();
    tagged.splice(36..36, info_tag(b"INAM", b"Keep this title"));
    let size = tagged.len() as u32 - 8;
    tagged[4..8].copy_from_slice(&size.to_le_bytes());
    for (source, options) in [
        (&integer, vec!["-ar", "16000"]),
        (&integer, vec!["-ac", "2"]),
        (&integer, vec!["-ss", "0.1"]),
        (&integer, vec!["-c:a", "copy"]),
        (&float, vec![]),
        (&tagged, vec![]),
    ] {
        std::fs::write(&input, source).unwrap();
        std::fs::write(&destination, b"preserve").unwrap();
        let entries = std::fs::read_dir(&dir).unwrap().count();
        let result = strict()
            .args(["-y", "-i"])
            .arg(&input)
            .args(options)
            .arg(&destination)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(4), "{result:?}");
        assert!(result.stdout.is_empty());
        assert_eq!(std::fs::read(&destination).unwrap(), b"preserve");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
    }
    std::fs::write(&input, &integer).unwrap();
    let alias = strict()
        .args(["-y", "-i"])
        .arg(&input)
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(alias.status.code(), Some(2), "{alias:?}");
    assert_eq!(std::fs::read(&input).unwrap(), integer);
    let mut truncated = integer;
    truncated.pop();
    std::fs::write(&input, truncated).unwrap();
    let result = strict()
        .args(["-y", "-i"])
        .arg(&input)
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!result.status.success(), "{result:?}");
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read(destination).unwrap(), b"preserve");
    std::fs::remove_dir_all(dir).unwrap();
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
fn unsigned_wave_s16_rate_composition_refuses_before_output() {
    let dir = scratch("u8-rate");
    let input = dir.join("input.wav");
    let samples: Vec<u8> = (0..4098).map(|i| (i % 256) as u8).collect();
    std::fs::write(&input, wav(1, 8, 1, &samples)).unwrap();
    for rate in ["16000", "22050"] {
        let output = strict()
            .arg("-i")
            .arg(&input)
            .args(["-ar", rate, "-f", "s16le", "pipe:1"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("unregistered WAVE PCM source, packing or rate composition")
        );
        let destination = dir.join("preserve.raw");
        std::fs::write(&destination, b"preserve").unwrap();
        let output = strict()
            .args(["-y", "-i"])
            .arg(&input)
            .args(["-ar", rate, "-f", "s16le"])
            .arg(&destination)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(4), "{output:?}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"preserve");
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
