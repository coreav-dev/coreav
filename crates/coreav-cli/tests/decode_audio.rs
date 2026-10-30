//! `coreav decode-audio` end to end (`docs/design/audio-decode.md`): the
//! WAV file and its summary, standard output, refusals and exit codes, on
//! Matroska FLAC files built here.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "schema/mod.rs"]
mod schema;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA: &str = include_str!("../../../docs/schema/decode-audio-v1.schema.json");

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

fn path(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&path);
    path
}

fn file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = path(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

struct Run {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

fn run(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("decode-audio")
        .args(args)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn json(text: &str) -> Value {
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn assert_matches(document: &Value) {
    schema::assert_matches(document, SCHEMA);
}

fn assert_error(run: &Run, code: i32, kind: &str) {
    assert_eq!(run.code, code, "stderr: {}", run.stderr);
    let error = json(&run.stderr);
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["kind"], kind, "{error}");
}

// A FLAC stream in Matroska, as in crates/coreav-audio/tests/decode.rs.

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A frame of two 16-bit verbatim subframes of `block` samples: channel 0
/// counts up from `start`, channel 1 is its negation.
fn frame(number: u8, block: u16, start: i16) -> Vec<u8> {
    let mut frame = vec![0xFF, 0xF8, 0x79, 0x18, number];
    frame.extend_from_slice(&(block - 1).to_be_bytes());
    frame.push(crc8(&frame));
    for sign in [1i16, -1] {
        frame.push(0x02);
        for i in 0..block {
            frame.extend_from_slice(&(sign * (start + i as i16)).to_be_bytes());
        }
    }
    let crc = crc16(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    frame
}

fn element(id: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = id.to_vec();
    out.push(0x01);
    out.extend_from_slice(&(payload.len() as u64).to_be_bytes()[1..]);
    out.extend_from_slice(payload);
    out
}

fn uint(id: &[u8], value: u64) -> Vec<u8> {
    element(id, &value.to_be_bytes())
}

fn matroska(codec_id: &str, private: &[u8], frames: &[Vec<u8>]) -> Vec<u8> {
    let mut header = uint(&[0x42, 0x86], 1);
    header.extend(element(&[0x42, 0x82], b"matroska"));
    header.extend(uint(&[0x42, 0x87], 4));
    header.extend(uint(&[0x42, 0x85], 2));
    let mut file = element(&[0x1A, 0x45, 0xDF, 0xA3], &header);
    let mut audio = element(&[0xB5], &44_100f64.to_be_bytes());
    audio.extend(uint(&[0x9F], 2));
    let mut entry = uint(&[0xD7], 1);
    entry.extend(uint(&[0x83], 2));
    entry.extend(element(&[0x86], codec_id.as_bytes()));
    entry.extend(element(&[0x63, 0xA2], private));
    entry.extend(element(&[0xE1], &audio));
    let mut cluster = uint(&[0xE7], 0);
    for f in frames {
        let mut block = vec![0x81, 0, 0, 0x80];
        block.extend_from_slice(f);
        cluster.extend(element(&[0xA3], &block));
    }
    let mut segment = element(
        &[0x15, 0x49, 0xA9, 0x66],
        &uint(&[0x2A, 0xD7, 0xB1], 1_000_000),
    );
    segment.extend(element(
        &[0x16, 0x54, 0xAE, 0x6B],
        &element(&[0xAE], &entry),
    ));
    segment.extend(element(&[0x1F, 0x43, 0xB6, 0x75], &cluster));
    file.extend(element(&[0x18, 0x53, 0x80, 0x67], &segment));
    file
}

fn flac_file(name: &str) -> PathBuf {
    // fLaC, the last metadata block's header, streaminfo: 44.1 kHz, 2
    // channels, 16 bits.
    let mut private = b"fLaC\x80\x00\x00\x22".to_vec();
    private.extend_from_slice(&[0x00, 0x10, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0]);
    private.extend_from_slice(&[0x0A, 0xC4, 0x42, 0xF0, 0, 0, 0, 0]);
    private.extend_from_slice(&[0; 16]);
    let frames = vec![frame(0, 300, 0), frame(1, 300, 300), frame(2, 100, 600)];
    file(name, &matroska("A_FLAC", &private, &frames))
}

/// The samples the test file holds, interleaved 16-bit.
fn expected_samples() -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..700i16 {
        out.extend_from_slice(&i.to_le_bytes());
        out.extend_from_slice(&(-i).to_le_bytes());
    }
    out
}

#[test]
fn writes_a_wav_file_and_a_summary() {
    let input = flac_file("da_in.mka");
    let out = path("da_out.wav");
    let result = run(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(std::str::from_utf8(&result.stdout).unwrap());
    assert_matches(&summary);
    assert_eq!(summary["stream"]["codec"], "flac");
    assert_eq!(summary["stream"]["stream_index"], 0);
    assert_eq!(summary["stream"]["packets"], 3);
    assert_eq!(summary["output"]["samples"], 700);
    assert_eq!(summary["output"]["sample_format"], "s16");
    let wav = std::fs::read(&out).unwrap();
    assert_eq!(summary["output"]["bytes"], wav.len());
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize,
        wav.len() - 8
    );
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 2800);
    assert_eq!(&wav[44..], &expected_samples()[..]);
    // The same with one thread.
    let one = path("da_out_one.wav");
    let result = run(&[
        "--threads",
        "1",
        input.to_str().unwrap(),
        one.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(std::fs::read(&one).unwrap(), wav);
}

/// `--rate` and `--channels`: the output's format, the stream's unchanged,
/// and a downmix the library refuses is a refusal.
#[test]
fn resamples_and_downmixes() {
    let input = flac_file("da_rate.mka");
    let out = path("da_rate.wav");
    let result = run(&[
        "--rate",
        "16000",
        "--channels",
        "1",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(std::str::from_utf8(&result.stdout).unwrap());
    assert_matches(&summary);
    // 700 samples at 44.1 kHz: ceil(700 × 16000 / 44100) at 16 kHz.
    assert_eq!(summary["output"]["samples"], 254);
    assert_eq!(summary["output"]["sample_rate"], 16000);
    assert_eq!(summary["output"]["channels"], 1);
    assert_eq!(summary["output"]["channel_mask"], 4);
    assert_eq!(summary["stream"]["sample_rate"], 44100);
    assert_eq!(summary["stream"]["channels"], 2);
    let wav = std::fs::read(&out).unwrap();
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16000);
    assert_eq!(wav.len(), 44 + 254 * 2);
    // The samples are i and -i: their mean is silence.
    assert!(wav[44..].iter().all(|&b| b == 0));
    let result = run(&[
        "--force",
        "--channels",
        "3",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_error(&result, 1, "unsupported_feature");
}

/// A silent MPEG-1 Layer III frame: 44.1 kHz, 128 kbit/s, joint stereo,
/// every granule empty (`part2_3_length` 0).
fn silent_mp3_frame() -> Vec<u8> {
    let mut frame = vec![0xFF, 0xFB, 0x90, 0x64];
    frame.resize(417, 0);
    frame
}

#[test]
fn decodes_an_mp3_track() {
    let frames = vec![silent_mp3_frame(); 3];
    let input = file("da_mp3.mka", &matroska("A_MPEG/L3", &[], &frames));
    let out = path("da_mp3.wav");
    let result = run(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(std::str::from_utf8(&result.stdout).unwrap());
    assert_matches(&summary);
    assert_eq!(summary["stream"]["codec"], "mp3");
    assert_eq!(summary["stream"]["bits_per_sample"], Value::Null);
    assert_eq!(summary["stream"]["packets"], 3);
    assert_eq!(summary["output"]["samples"], 3 * 1152);
    let wav = std::fs::read(&out).unwrap();
    assert_eq!(wav.len(), 44 + 3 * 1152 * 4);
    assert!(wav[44..].iter().all(|&b| b == 0));
}

#[test]
fn decodes_an_opus_track() {
    // OpusHead (RFC 7845 5.1): version 1, 2 channels, no pre-skip, 48 kHz,
    // no gain, mapping family 0.
    let mut head = b"OpusHead\x01\x02\x00\x00".to_vec();
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&[0, 0, 0]);
    // A CELT-only 20 ms stereo packet whose one frame is empty (RFC 6716
    // 3.1-3.2): decoded as a lost frame.
    let frames = vec![vec![0xFC]; 3];
    let input = file("da_opus.mka", &matroska("A_OPUS", &head, &frames));
    let out = path("da_opus.wav");
    let result = run(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(std::str::from_utf8(&result.stdout).unwrap());
    assert_matches(&summary);
    assert_eq!(summary["stream"]["codec"], "opus");
    assert_eq!(summary["stream"]["bits_per_sample"], Value::Null);
    assert_eq!(summary["stream"]["packets"], 3);
    assert_eq!(summary["output"]["samples"], 3 * 960);
    let wav = std::fs::read(&out).unwrap();
    assert_eq!(wav.len(), 44 + 3 * 960 * 4);
}

#[test]
fn writes_wav_to_standard_output() {
    let input = flac_file("da_stdout.mka");
    let result = run(&["--format", "s24", input.to_str().unwrap(), "-"]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stderr);
    assert_matches(&summary);
    assert_eq!(summary["output"]["sample_format"], "s24");
    let wav = &result.stdout;
    // Extensible format for 24 bits; sizes unknown on a pipe.
    assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 0xFFFE);
    assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), u32::MAX);
    assert_eq!(&wav[60..64], b"data");
    assert_eq!(
        u32::from_le_bytes(wav[64..68].try_into().unwrap()),
        u32::MAX
    );
    assert_eq!(wav.len(), 68 + 700 * 2 * 3);
    assert_eq!(summary["output"]["bytes"], wav.len());
    assert_eq!(&wav[68..74], &[0, 0, 0, 0, 0, 0]);
    assert_eq!(&wav[74..80], &[0, 1, 0, 0, 0xFF, 0xFF]);
}

/// Standard output, a pipe or a file, carries the samples written to a file;
/// only the sizes in the header differ, unknown on standard output.
#[test]
fn standard_output_carries_the_file_samples() {
    let input = flac_file("da_same.mka");
    let input = input.to_str().unwrap();
    let wav = path("da_same.wav");
    let result = run(&[input, wav.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let expected = std::fs::read(&wav).unwrap();
    let piped = run(&[input, "-"]);
    assert_eq!(piped.code, 0, "{}", piped.stderr);
    let redirected = path("da_same_redirected.wav");
    let done = Command::new(bin())
        .args(["decode-audio", input, "-"])
        .stdout(std::fs::File::create(&redirected).unwrap())
        .output()
        .unwrap();
    assert_eq!(done.status.code(), Some(0));
    assert_eq!(std::fs::read(&redirected).unwrap(), piped.stdout);
    assert_eq!(piped.stdout.len(), expected.len());
    // Everything but the RIFF size and the data chunk's size.
    let data = expected.windows(4).position(|w| w == b"data").unwrap();
    let without_sizes = |wav: &[u8]| {
        [
            wav[..4].to_vec(),
            wav[8..data + 4].to_vec(),
            wav[data + 8..].to_vec(),
        ]
    };
    assert_eq!(without_sizes(&piped.stdout), without_sizes(&expected));
}

#[test]
fn refusals_exit_1_and_leave_no_file() {
    let out = path("da_refused.wav");
    let vorbis = file(
        "da_vorbis.mka",
        &matroska("A_VORBIS", &[2, 1, 1, 0, 0, 0], &[vec![0; 4]]),
    );
    let result = run(&[vorbis.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 1, "unsupported_feature");
    assert!(result.stderr.contains("vorbis"), "{}", result.stderr);
    assert!(!out.exists());
    let flac = flac_file("da_stream.mka");
    let result = run(&[
        "--stream",
        "3",
        flac.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_error(&result, 1, "unsupported_feature");
    let junk = file("da_junk.mka", b"not a media file at all");
    let result = run(&[junk.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 1, "unsupported_container");
    let missing = path("da_missing.mka");
    let result = run(&[missing.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 3, "io");
    assert!(!out.exists());
}

#[test]
fn usage_errors_exit_2_and_existing_files_are_kept() {
    let input = flac_file("da_usage.mka");
    let out = path("da_usage.wav");
    for args in [
        vec!["--format", "u8"],
        vec!["--threads", "17"],
        vec!["--stream", "x"],
        vec!["--rate", "999"],
        vec!["--rate", "768001"],
        vec!["--channels", "0"],
    ] {
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
        assert_eq!(run(&all).code, 2, "{args:?}");
        assert!(!out.exists());
    }
    std::fs::write(&out, b"keep me").unwrap();
    assert_eq!(
        run(&[input.to_str().unwrap(), out.to_str().unwrap()]).code,
        2
    );
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    assert_eq!(
        run(&["--force", input.to_str().unwrap(), input.to_str().unwrap()]).code,
        2
    );
    let result = run(&["--force", input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_ne!(std::fs::read(&out).unwrap(), b"keep me");
}

#[cfg(target_os = "linux")]
#[test]
fn a_full_standard_output_exits_3() {
    let input = flac_file("da_full.mka");
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let out = Command::new(bin())
        .args(["decode-audio", input.to_str().unwrap(), "-"])
        .stdout(full)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
}
