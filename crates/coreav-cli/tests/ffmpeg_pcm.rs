//! Native WAV and raw PCM output, including refusals and atomic replacement.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

fn run(input: &Path, output: &Path, options: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["--strict-safe", "ffmpeg", "-i"])
        .arg(input)
        .args(options)
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

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn input_thread_expressions_reach_fallback_without_consuming_stdin() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let dir = scratch("pcm-thread-expression-fallback");
    let fallback = dir.join("ffmpeg");
    let record = dir.join("args");
    std::fs::write(
        &fallback,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGUMENT_RECORD\"\ncat\nexit 7\n",
    )
    .unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    let args = [
        "-threads", "2*2", "-f", "s16le", "-ar", "44100", "-ac", "1", "-i", "pipe:", "-f", "s16le",
        "pipe:",
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("ffmpeg")
        .args(args)
        .env("COREAV_FALLBACK", &fallback)
        .env("ARGUMENT_RECORD", &record)
        .env_remove("COREAV_STRICT_SAFE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes: Vec<u8> = (0..1009i16).flat_map(i16::to_le_bytes).collect();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let result = child.wait_with_output().unwrap();
    assert_eq!(result.status.code(), Some(7), "{result:?}");
    assert_eq!(result.stdout, bytes);
    assert_eq!(
        std::fs::read_to_string(record).unwrap(),
        args.join("\n") + "\n"
    );
}

#[test]
fn copy_jobs_ignore_decoder_capacity_but_audio_decode_refuses() {
    let dir = scratch("pcm-thread-capacity");
    let (input, _) = fixture(&dir, false);
    for threads in ["17", "32", "4294967295"] {
        let output = dir.join(format!("copy-{threads}.mka"));
        success(
            Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["--strict-safe", "ffmpeg", "-threads", threads, "-i"])
                .arg(&input)
                .args(["-c", "copy"])
                .arg(&output)
                .output()
                .unwrap(),
        );
        assert!(std::fs::metadata(output).unwrap().len() > 0);
        for form in ["wav", "s16le", "f32le", "flac"] {
            let output = dir.join(format!("decoded-{threads}.{form}"));
            let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(["--strict-safe", "ffmpeg", "-threads", threads, "-i"])
                .arg(&input)
                .args(["-f", form, "-map_metadata", "-1"])
                .arg(&output)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(4), "{result:?}");
            assert!(result.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("sixteen audio decoder workers")
            );
            assert!(!output.exists());
        }
    }
}

#[test]
fn raw_pcm_identity_has_headerless_float_and_integer_stdout() {
    let dir = scratch("raw-pcm-stdout-identity");
    let input = dir.join("input.raw");
    let values = [i16::MIN, -12345, -1, 0, 1, 12345, i16::MAX];
    let bytes: Vec<u8> = values.into_iter().flat_map(i16::to_le_bytes).collect();
    std::fs::write(&input, &bytes).unwrap();
    for (format, expected) in [
        ("s16le", bytes.clone()),
        (
            "f32le",
            values
                .into_iter()
                .flat_map(|sample| (f32::from(sample) / 32768.0).to_le_bytes())
                .collect(),
        ),
    ] {
        for output in ["-", "pipe:", "pipe:1"] {
            let before = [
                "--strict-safe",
                "ffmpeg",
                "-threads",
                "2",
                "-f",
                "s16le",
                "-ar",
                "48000",
                "-ac",
                "1",
                "-i",
            ];
            let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .args(before)
                .arg(&input)
                .args(["-f", format, output])
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            assert!(result.stderr.is_empty(), "{result:?}");
            assert_eq!(result.stdout, expected);
        }
    }
}

#[test]
fn decoded_flac_raw_stdout_keeps_samples_and_float_packing() {
    let dir = scratch("decoded-pcm-stdout");
    let (input, samples) = fixture(&dir, false);
    for format in ["s16le", "f32le"] {
        let result = run(&input, Path::new("pipe:"), &["-f", format]);
        assert!(result.status.success(), "{result:?}");
        assert!(result.stderr.is_empty(), "{result:?}");
        let expected = if format == "s16le" {
            samples.clone()
        } else {
            samples
                .chunks_exact(2)
                .flat_map(|sample| {
                    (f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0).to_le_bytes()
                })
                .collect()
        };
        assert_eq!(result.stdout, expected);
    }
}

#[test]
fn encoded_stdin_is_replayed_for_native_raw_output() {
    use std::io::Write;
    use std::process::Stdio;
    let dir = scratch("encoded-stdin-pcm");
    let (input, samples) = fixture(&dir, false);
    for name in ["-", "pipe:", "pipe:0"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args([
                "--strict-safe",
                "ffmpeg",
                "-i",
                name,
                "-f",
                "s16le",
                "pipe:",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&std::fs::read(&input).unwrap())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{result:?}");
        assert!(result.stderr.is_empty(), "{result:?}");
        assert_eq!(result.stdout, samples);
    }
}

#[test]
fn raw_stdin_writes_before_eof_without_a_whole_input_spool() {
    use std::io::{Read, Write};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "--strict-safe",
            "ffmpeg",
            "-f",
            "s16le",
            "-ar",
            "48000",
            "-ac",
            "1",
            "-i",
            "pipe:",
            "-f",
            "s16le",
            "pipe:",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut first = [0u8; 1];
        let result = stdout.read_exact(&mut first);
        sender.send((result, first, stdout)).unwrap();
    });
    let samples = vec![0x31; 16384];
    stdin.write_all(&samples).unwrap();
    let received = receiver.recv_timeout(Duration::from_secs(10));
    if received.is_err() {
        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        reader.join().unwrap();
        panic!("raw stdin did not produce samples before EOF");
    }
    let (result, first, mut stdout) = received.unwrap();
    result.unwrap();
    drop(stdin);
    let mut actual = first.to_vec();
    stdout.read_to_end(&mut actual).unwrap();
    reader.join().unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    assert_eq!(actual, samples);
}

#[test]
fn tiny_pcm_stdout_reports_a_closed_reader_during_final_flush() {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args([
            "--strict-safe",
            "ffmpeg",
            "-f",
            "s16le",
            "-ar",
            "48000",
            "-ac",
            "1",
            "-i",
        ])
        .arg("pipe:")
        .args(["-f", "s16le", "pipe:"])
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
        .write_all(&[0x12, 0x34])
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert_eq!(result.status.code(), Some(141), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
}

#[cfg(unix)]
#[test]
fn encoded_stdin_fallback_replaces_the_process_without_leaving_its_spool() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let dir = scratch("pcm-stdin-exec");
    let (input, _) = fixture(&dir, false);
    let bytes = std::fs::read(input).unwrap();
    let fallback = dir.join("ffmpeg");
    let record = dir.join("pid");
    let temporary = dir.join("temporary");
    std::fs::create_dir(&temporary).unwrap();
    std::fs::write(
        &fallback,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$PID_RECORD\"\ncat\n",
    )
    .unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["ffmpeg", "-i", "pipe:", "-ac", "1", "-f", "f32le", "pipe:"])
        .env("COREAV_FALLBACK", &fallback)
        .env("PID_RECORD", &record)
        .env("TMPDIR", &temporary)
        .env_remove("COREAV_STRICT_SAFE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, bytes);
    assert_eq!(std::fs::read_to_string(record).unwrap(), pid.to_string());
    assert_eq!(std::fs::read_dir(temporary).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn replayed_fallback_keeps_full_windows_exit_codes_and_cleans_its_spool() {
    use std::io::Write;
    use std::process::Stdio;
    let dir = scratch("pcm-stdin-windows-status");
    let (input, _) = fixture(&dir, false);
    let bytes = std::fs::read(input).unwrap();
    let fallback = dir.join("ffmpeg.exe");
    let built = Command::new("rustc")
        .arg("--edition=2024")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/fallback_program.rs"))
        .arg("-o")
        .arg(&fallback)
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let args = ["-i", "pipe:", "-ac", "1", "-f", "f32le", "pipe:"];
    for code in [0, 37, 256, 321] {
        let temporary = dir.join(format!("temporary-{code}"));
        let record = dir.join(format!("input-{code}"));
        std::fs::create_dir(&temporary).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .arg("ffmpeg")
            .args(args)
            .env("COREAV_FALLBACK", &fallback)
            .env("FALLBACK_TEST_INPUT", &record)
            .env("FALLBACK_TEST_EXIT", code.to_string())
            .env("TMP", &temporary)
            .env("TEMP", &temporary)
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .env_remove("COREAV_STRICT_SAFE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        let result = child.wait_with_output().unwrap();
        assert_eq!(result.status.code(), Some(code), "{result:?}");
        assert_eq!(std::fs::read(record).unwrap(), bytes);
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            format!("args: {}\n", args.join(" "))
        );
        assert_eq!(std::fs::read_dir(temporary).unwrap().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn consumed_encoded_stdin_reaches_fallback_with_original_arguments() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let dir = scratch("pcm-stdin-fallback");
    let (input, samples) = fixture(&dir, false);
    let bytes = std::fs::read(input).unwrap();
    let fallback = dir.join("ffmpeg");
    let record = dir.join("args");
    let temporary = dir.join("temporary");
    std::fs::create_dir(&temporary).unwrap();
    std::fs::write(
        &fallback,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGUMENT_RECORD\"\ncat\n",
    )
    .unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    let args = [
        "-i", "pipe:", "-ac", "1", "-ar", "16000", "-f", "f32le", "pipe:",
    ];
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .arg("ffmpeg")
        .args(args)
        .env("COREAV_FALLBACK", &fallback)
        .env("ARGUMENT_RECORD", &record)
        .env("TMPDIR", &temporary)
        .env_remove("COREAV_STRICT_SAFE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, bytes);
    assert_ne!(result.stdout, samples);
    assert_eq!(
        std::fs::read_to_string(record).unwrap(),
        args.join("\n") + "\n"
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("ran"));
    assert_eq!(std::fs::read_dir(temporary).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn a_partial_raw_frame_after_output_never_invokes_fallback() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let dir = scratch("pcm-no-late-fallback");
    let fallback = dir.join("ffmpeg");
    let record = dir.join("invoked");
    std::fs::write(
        &fallback,
        "#!/bin/sh\nprintf invoked > \"$FALLBACK_RECORD\"\nprintf invalid-fallback-output\n",
    )
    .unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
    let before = ["ffmpeg", "-f", "s16le", "-ar", "48000", "-ac", "1", "-i"];
    let after = ["-f", "s16le", "pipe:"];
    let mut child = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(before)
        .arg("pipe:")
        .args(after)
        .env("COREAV_FALLBACK", fallback)
        .env("FALLBACK_RECORD", &record)
        .env_remove("COREAV_STRICT_SAFE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&vec![0x31; 131073]));
    let result = child.wait_with_output().unwrap();
    writer.join().unwrap().unwrap();
    assert!(!result.status.success(), "{result:?}");
    assert_ne!(result.status.code(), Some(4), "{result:?}");
    assert!(!result.stdout.is_empty());
    assert!(result.stdout.iter().all(|byte| *byte == 0x31));
    assert!(!record.exists());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("ran"));
}

#[test]
fn nonstandard_pipe_descriptors_and_unregistered_compositions_refuse() {
    let dir = scratch("pcm-unregistered-stages");
    let (input, _) = fixture(&dir, false);
    for options in [
        vec!["-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:"],
        vec!["-ac", "1", "-f", "f32le", "pipe:"],
        vec!["-ar", "16000", "-f", "f32le", "pipe:"],
        vec!["-f", "s16le", "pipe:2"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffmpeg", "-i"])
            .arg(&input)
            .args(options)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(4), "{result:?}");
        assert!(result.stdout.is_empty(), "{result:?}");
    }
}

fn crc(data: &[u8], bits: u32, polynomial: u16) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        crc ^= u16::from(byte) << (bits - 8);
        for _ in 0..8 {
            crc = if crc & (1 << (bits - 1)) == 0 {
                crc << 1
            } else {
                (crc << 1) ^ polynomial
            };
        }
    }
    crc & ((1u32 << bits) - 1) as u16
}

fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
    let mut bytes = id.to_vec();
    bytes.push(1);
    bytes.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
    bytes.extend_from_slice(data);
    bytes
}

fn fixture(dir: &Path, broken: bool) -> (PathBuf, Vec<u8>) {
    fixture_with_bitmap(dir, broken, false)
}

fn fixture_with_bitmap(dir: &Path, broken: bool, bitmap: bool) -> (PathBuf, Vec<u8>) {
    let samples: Vec<u8> = (0..512i32)
        .flat_map(|n| [(n * 101 - 12000) as i16, (12000 - n * 83) as i16])
        .flat_map(i16::to_le_bytes)
        .collect();
    let mut frame = vec![0xff, 0xf8, 0x79, 0x18, 0, 1, 0xff];
    frame.push(crc(&frame, 8, 7) as u8);
    for channel in 0..2 {
        frame.push(2);
        for sample in samples.chunks_exact(4) {
            frame.extend([sample[channel * 2 + 1], sample[channel * 2]]);
        }
    }
    frame.extend_from_slice(&crc(&frame, 16, 0x8005).to_be_bytes());
    if broken {
        frame[20] ^= 0x40;
    }
    let mut private = b"fLaC\x80\x00\x00\x22".to_vec();
    private.extend_from_slice(&[2, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    private.extend_from_slice(&((44_100u64 << 44) | (1 << 41) | (15 << 36) | 512).to_be_bytes());
    private.extend_from_slice(&[0; 16]);
    let header = element(&[0x42, 0x82], b"matroska");
    let mut bytes = element(&[0x1a, 0x45, 0xdf, 0xa3], &header);
    let mut track = element(&[0xd7], &[1]);
    track.extend(element(&[0x83], &[2]));
    track.extend(element(&[0x86], b"A_FLAC"));
    track.extend(element(&[0x63, 0xa2], &private));
    let mut audio = element(&[0xb5], &44_100f64.to_be_bytes());
    audio.extend(element(&[0x9f], &[2]));
    track.extend(element(&[0xe1], &audio));
    let mut segment = element(
        &[0x15, 0x49, 0xa9, 0x66],
        &element(&[0x2a, 0xd7, 0xb1], &[0x0f, 0x42, 0x40]),
    );
    let mut tracks = element(&[0xae], &track);
    if bitmap {
        let mut subtitle = element(&[0xd7], &[2]);
        subtitle.extend(element(&[0x83], &[0x11]));
        subtitle.extend(element(&[0x86], b"S_HDMV/PGS"));
        tracks.extend(element(&[0xae], &subtitle));
    }
    segment.extend(element(&[0x16, 0x54, 0xae, 0x6b], &tracks));
    let mut block = vec![0x81, 0, 0, 0x80];
    block.extend(frame);
    let mut cluster = element(&[0xe7], &[0]);
    cluster.extend(element(&[0xa3], &block));
    segment.extend(element(&[0x1f, 0x43, 0xb6, 0x75], &cluster));
    bytes.extend(element(&[0x18, 0x53, 0x80, 0x67], &segment));
    let path = dir.join("input.mka");
    std::fs::write(&path, bytes).unwrap();
    (path, samples)
}

#[test]
fn bitmap_subtitle_selection_is_refused_unless_disabled() {
    let dir = scratch("ffmpeg-pcm-bitmap");
    let (input, expected) = fixture_with_bitmap(&dir, false, true);
    for form in ["wav", "s16le"] {
        let output = dir.join(format!("out.{form}"));
        std::fs::write(&output, b"original").unwrap();
        let refused = run(&input, &output, &["-y", "-scodec", "copy", "-f", form]);
        assert_eq!(refused.status.code(), Some(4));
        assert!(refused.stdout.is_empty());
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        for options in [
            vec!["-y", "-f", form],
            vec!["-y", "-scodec", "copy", "-sn", "-f", form],
        ] {
            success(run(&input, &output, &options));
            let actual = std::fs::read(&output).unwrap();
            let start = if form == "wav" { 44 } else { 0 };
            assert_eq!(&actual[start..], expected);
        }
    }
}

#[test]
fn only_the_effective_pcm_codec_is_validated() {
    let dir = scratch("ffmpeg-pcm-effective-codec");
    let (input, expected) = fixture(&dir, false);
    for form in ["wav", "s16le"] {
        let output = dir.join(format!("out.{form}"));
        for options in [
            vec!["-y", "-c:a", "pcm_u8", "-c:a", "pcm_s16le", "-f", form],
            vec!["-y", "-c:a:1", "pcm_u8", "-c:a:0", "pcm_s16le", "-f", form],
        ] {
            success(run(&input, &output, &options));
            let actual = std::fs::read(&output).unwrap();
            let start = if form == "wav" { 44 } else { 0 };
            assert_eq!(&actual[start..], expected);
        }
        std::fs::write(&output, b"original").unwrap();
        let refused = run(
            &input,
            &output,
            &["-y", "-c:a", "pcm_s16le", "-c:a", "pcm_u8", "-f", form],
        );
        assert_eq!(refused.status.code(), Some(4));
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
    }
}

#[test]
fn native_wav_and_raw_preserve_the_lossless_samples() {
    let dir = scratch("ffmpeg-pcm-native");
    let (input, expected) = fixture(&dir, false);
    let wav = dir.join("out.wav");
    success(run(&input, &wav, &[]));
    let bytes = std::fs::read(&wav).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
        bytes.len() - 8
    );
    assert_eq!(
        u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize,
        expected.len()
    );
    assert_eq!(&bytes[44..], expected);
    let raw = dir.join("out.raw");
    for args in [
        vec!["-f", "s16le", "-acodec", "pcm_s16le"],
        vec!["-y", "-f", "s16le", "-ar", "44100", "-ac", "2"],
        vec!["-y", "-f", "s16le", "-map", "0:a:0", "-c:0", "pcm_s16le"],
        vec!["-y", "-f", "s16le", "-c:a", "aac", "-c:a", "pcm_s16le"],
    ] {
        success(run(&input, &raw, &args));
        assert_eq!(std::fs::read(&raw).unwrap(), expected);
    }
    success(run(&input, &wav, &["-y", "-ss", "0", "-t", "0.01"]));
    assert_eq!(std::fs::metadata(&wav).unwrap().len(), 44 + 441 * 4);
}

#[test]
fn output_threads_does_not_change_pcm_or_allow_other_encoder_options() {
    let dir = scratch("ffmpeg-pcm-threads");
    let (input, expected) = fixture(&dir, false);
    let raw = dir.join("out.raw");
    for threads in ["0", "auto", "4"] {
        success(run(
            &input,
            &raw,
            &["-y", "-f", "s16le", "-threads", threads],
        ));
        assert_eq!(std::fs::read(&raw).unwrap(), expected);
    }
    let wav = dir.join("out.wav");
    success(run(&input, &wav, &["-threads", "4"]));
    assert_eq!(&std::fs::read(&wav).unwrap()[44..], expected);
    for option in ["-crf", "-b:v", "-b:a"] {
        let result = run(
            &input,
            &raw,
            &["-y", "-f", "s16le", "-threads", "4", option, "20"],
        );
        assert_eq!(result.status.code(), Some(4));
        assert_eq!(std::fs::read(&raw).unwrap(), expected);
    }
}

#[test]
fn refusals_and_damage_never_replace_an_existing_output() {
    let dir = scratch("ffmpeg-pcm-refused");
    let (input, _) = fixture(&dir, false);
    let wav = dir.join("out.wav");
    std::fs::write(&wav, b"original").unwrap();
    for (args, code) in [
        (vec![], 4),
        (vec!["-n"], 2),
        (vec!["-y", "-map", "0:a", "-map", "0:a"], 4),
        (vec!["-y", "-c:a", "copy"], 4),
        (vec!["-y", "-c:a", "pcm_s16le", "-c:a", "aac"], 4),
        (vec!["-y", "-an"], 2),
        (vec!["-y", "-ar", "16000"], 4),
        (vec!["-y", "-ac", "1"], 4),
        (vec!["-y", "-ss", "1"], 4),
        (vec!["-y", "-movflags", "+faststart"], 4),
        (vec!["-y", "-sample_fmt", "s16"], 4),
        (vec!["-y", "-t", "0.001"], 4),
    ] {
        let result = run(&input, &wav, &args);
        assert_eq!(
            result.status.code(),
            Some(code),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(&wav).unwrap(), b"original");
    }
    let before = std::fs::read(&input).unwrap();
    assert_eq!(
        run(&input, &input, &["-y", "-f", "wav"]).status.code(),
        Some(2)
    );
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let (input, _) = fixture(&dir, true);
    assert_eq!(run(&input, &wav, &["-y"]).status.code(), Some(1));
    assert_eq!(std::fs::read(&wav).unwrap(), b"original");
    assert!(
        !std::fs::read_dir(&dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part"))
    );
}

#[test]
fn pcm_selection_refuses_video_and_keeps_codec_output_positions() {
    let dir = scratch("ffmpeg-pcm-selection");
    let input = dir.join("video.mp4");
    std::fs::write(&input, common::av_movie_with(common::aac_entry(), false)).unwrap();
    let wav = dir.join("out.wav");
    for options in [
        vec!["-map", "0:v:0"],
        vec!["-map", "0"],
        vec!["-an"],
        vec!["-c:a", "copy"],
        vec!["-map", "0:a", "-c:0", "copy"],
    ] {
        let result = run(&input, &wav, &options);
        assert!(!result.status.success(), "{options:?}");
        assert!(!wav.exists());
    }
    #[cfg(not(feature = "aac"))]
    assert_eq!(run(&input, &wav, &[]).status.code(), Some(4));
    let (input, expected) = fixture(&dir, false);
    success(run(&input, &wav, &["-map", "0:a", "-c:1", "aac"]));
    assert_eq!(&std::fs::read(&wav).unwrap()[44..], expected);
    let out = dir.join("out.mp4");
    assert_eq!(
        run(&input, &out, &["-c:a", "pcm_s16le"]).status.code(),
        Some(4)
    );
    assert!(!out.exists());
}

#[test]
fn opus_pcm_keeps_refusal_and_the_existing_ffmpeg_fallback() {
    let dir = scratch("ffmpeg-pcm-opus-fallback");
    let input = dir.join("opus.mka");
    let mut track = element(&[0xd7], &[1]);
    track.extend(element(&[0x83], &[2]));
    track.extend(element(&[0x86], b"A_OPUS"));
    track.extend(element(
        &[0x63, 0xa2],
        b"OpusHead\x01\x01\x00\x00\x80\xbb\x00\x00\x00\x00\x00",
    ));
    let mut audio = element(&[0xb5], &48_000f64.to_be_bytes());
    audio.extend(element(&[0x9f], &[1]));
    track.extend(element(&[0xe1], &audio));
    let mut segment = element(
        &[0x15, 0x49, 0xa9, 0x66],
        &element(&[0x2a, 0xd7, 0xb1], &[0x0f, 0x42, 0x40]),
    );
    segment.extend(element(
        &[0x16, 0x54, 0xae, 0x6b],
        &element(&[0xae], &track),
    ));
    let mut cluster = element(&[0xe7], &[0]);
    cluster.extend(element(&[0xa3], &[0x81, 0, 0, 0x80, 0xf8, 0xff, 0xfe]));
    segment.extend(element(&[0x1f, 0x43, 0xb6, 0x75], &cluster));
    let mut bytes = element(
        &[0x1a, 0x45, 0xdf, 0xa3],
        &element(&[0x42, 0x82], b"matroska"),
    );
    bytes.extend(element(&[0x18, 0x53, 0x80, 0x67], &segment));
    std::fs::write(&input, bytes).unwrap();
    let output = dir.join("output");
    for format in ["wav", "s16le"] {
        std::fs::write(&output, b"original").unwrap();
        let args = ["-y", "-f", format, "-acodec", "pcm_s16le"];
        let refused = run(&input, &output, &args);
        assert_eq!(refused.status.code(), Some(4), "{refused:?}");
        assert!(refused.stdout.is_empty());
        assert_eq!(std::fs::read(&output).unwrap(), b"original");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let fallback = dir.join("ffmpeg");
            std::fs::write(&fallback, "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 7\n").unwrap();
            std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
            let ran = Command::new(env!("CARGO_BIN_EXE_coreav"))
                .env_clear()
                .env("PATH", "/nonexistent")
                .env("COREAV_FALLBACK", &fallback)
                .args(["ffmpeg", "-i"])
                .arg(&input)
                .args(args)
                .arg(&output)
                .output()
                .unwrap();
            assert_eq!(ran.status.code(), Some(7), "{ran:?}");
            let expected = [
                "-i",
                input.to_str().unwrap(),
                "-y",
                "-f",
                format,
                "-acodec",
                "pcm_s16le",
                output.to_str().unwrap(),
            ]
            .join("\n")
                + "\n";
            assert_eq!(ran.stdout, expected.as_bytes());
            assert_eq!(std::fs::read(&output).unwrap(), b"original");
        }
    }
}

#[test]
fn input_and_output_zero_time_options_keep_their_distinct_meanings() {
    let dir = scratch("ffmpeg-pcm-zero-time");
    let (input, expected) = fixture(&dir, false);
    let out = dir.join("out.wav");
    for (before, after, code, length) in [
        (vec!["-t", "0"], vec![], 0, 44 + expected.len()),
        (vec!["-to", "0"], vec![], 2, 0),
        (vec![], vec!["-to", "0"], 2, 0),
        (vec![], vec!["-t", "0"], 0, 44),
        (vec!["-to", "0", "-t", "0"], vec![], 0, 44 + expected.len()),
    ] {
        let _ = std::fs::remove_file(&out);
        let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
            .args(["--strict-safe", "ffmpeg"])
            .args(&before)
            .arg("-i")
            .arg(&input)
            .args(&after)
            .arg(&out)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(code),
            "{before:?} {after:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::metadata(&out).map_or(0, |m| m.len()),
            length as u64
        );
    }
    let input = dir.join("aac.mp4");
    std::fs::write(&input, common::av_movie_with(common::aac_entry(), false)).unwrap();
    let _ = std::fs::remove_file(&out);
    let result = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(["--strict-safe", "ffmpeg", "-ss", "0", "-i"])
        .arg(input)
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(4));
    assert!(!out.exists());
}
