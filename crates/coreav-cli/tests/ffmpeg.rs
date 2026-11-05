//! `coreav ffmpeg` (`docs/design/compat-cli.md`): what it runs and what it
//! refuses, through the binary, and the `ffmpeg` commands of `commands-0.1.0`
//! on the generated fixtures, whose outputs
//! `tools/compat-corpus/copy_check.py` judges with the oracle of each job (R3).

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;

use common::{Spec, aac_entry, av_movie, av_movie_with, bx, cat, full, u32s};
use coreav_cli::fallback::Reason;
use coreav_cli::ffmpeg::{Job, Refusal, Stop};
use coreav_cli::ffprobe::StreamKind;
use coreav_cli::strict_safe::StrictSafe;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

fn fresh(dir: &Path) -> PathBuf {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    dir.to_path_buf()
}

fn scratch(name: &str) -> PathBuf {
    fresh(&Path::new(env!("CARGO_TARGET_TMPDIR")).join(name))
}

struct FixtureDir(PathBuf);

impl std::ops::Deref for FixtureDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for FixtureDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn oracle_scratch(name: &str) -> FixtureDir {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    FixtureDir(path)
}

#[test]
fn oracle_scratch_owns_isolated_directories_until_drop() {
    let first = oracle_scratch("ffmpeg-oracle-isolation");
    let first_path = first.to_path_buf();
    std::fs::write(first.join("marker"), b"first owner").unwrap();
    let second = oracle_scratch("ffmpeg-oracle-isolation");
    let second_path = second.to_path_buf();
    assert_ne!(first_path, second_path);
    assert_eq!(std::fs::read(first.join("marker")).unwrap(), b"first owner");
    drop(second);
    assert!(!second_path.exists());
    assert!(first_path.exists());
    drop(first);
    assert!(!first_path.exists());
}

struct Run {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

/// Runs in strict-safe mode, so that a refused command is refused rather
/// than handed to an FFmpeg on `PATH` (`tests/fallback.rs` tests that).
fn ffmpeg(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("ffmpeg")
        .args(args)
        .env("COREAV_STRICT_SAFE", "1")
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: out.stdout,
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn movie(dir: &Path) -> String {
    let path = dir.join("in.mp4");
    std::fs::write(&path, av_movie_with(aac_entry(), false)).unwrap();
    path.to_string_lossy().into_owned()
}

fn fallback_arguments(args: &[&str]) -> Vec<u8> {
    std::iter::once("-hide_banner")
        .chain(args.iter().copied())
        .flat_map(|arg| arg.bytes().chain([0]))
        .collect()
}

#[test]
fn fallback_argument_expectations_keep_the_banner_and_exact_original_bytes() {
    let args = ["-y", "-i", "file with spaces.mp4", "-c", "copy"];
    let expected = b"-hide_banner\0-y\0-i\0file with spaces.mp4\0-c\0copy\0";
    assert_eq!(fallback_arguments(&args), expected);
    assert_ne!(fallback_arguments(&args[1..]), expected);
    assert_ne!(
        fallback_arguments(&["-i", "file with spaces.mp4", "-y", "-c", "copy"]),
        expected
    );
    assert_ne!(
        fallback_arguments(&["-y", "-i", "changed.mp4", "-c", "copy"]),
        expected
    );
}

#[test]
fn copied_video_aac_encoding_preserves_destinations_and_exact_fallback() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let dir = oracle_scratch("ffmpeg-copied-video-aac");
    let first = dir.join("first input.mp4");
    let second = dir.join("second input.mp4");
    for (path, size, frequency, rate, delay) in [
        (&first, "64x48", "751", "44100", "0"),
        (&second, "80x64", "919", "48000", "0.125"),
    ] {
        let generated = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size={size}:rate=24:duration=0.5"))
            .args(["-itsoffset", delay, "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={frequency}:sample_rate={rate}:duration=0.5"
            ))
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "2",
                "-c:a",
                "aac",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let probe = Command::new(&ffprobe_bin)
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_packets",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(probe.status.success() && probe.stderr.is_empty());
        let document: Value = serde_json::from_slice(&probe.stdout).unwrap();
        let streams = document["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2);
        assert_eq!(streams[0]["codec_name"], "h264");
        assert!(streams[0]["has_b_frames"].as_u64().unwrap() > 0);
        assert_eq!(streams[1]["codec_name"], "aac");
        if delay == "0" {
            assert_eq!(streams[1]["start_pts"], 0);
            assert!(
                document["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|packet| {
                        packet["stream_index"] == 1
                            && packet["side_data_list"]
                                .as_array()
                                .is_some_and(|side_data| {
                                    side_data.iter().any(|side| {
                                        side["side_data_type"] == "Skip Samples"
                                            && side["skip_samples"].as_u64().is_some_and(|n| n > 0)
                                    })
                                })
                    })
            );
        } else {
            assert!(streams[1]["start_pts"].as_i64().unwrap() > 0);
        }
    }
    let neighbor = dir.join(".output.mp4.coreav-neighbor.part");
    std::fs::write(&neighbor, b"unrelated staging file").unwrap();
    #[cfg(unix)]
    let recorder = {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("ffmpeg");
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf 'started' > \"$COREAV_TEST_STARTED\"\nprintf '%s\\0' \"$@\"\nexit 19\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        program
    };
    for (name, inputs, options) in [
        ("one", 1, vec!["-c:v", "copy", "-c:a", "aac"]),
        ("automatic", 2, vec!["-c:v", "copy", "-c:a", "aac"]),
        (
            "legacy",
            2,
            vec!["-c:v", "copy", "-c:a", "aac", "-strict", "-2"],
        ),
        (
            "selected",
            2,
            vec![
                "-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac", "-b:a", "96k",
            ],
        ),
        (
            "first",
            2,
            vec![
                "-map",
                "0",
                "-c",
                "copy",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
            ],
        ),
        (
            "indexed",
            2,
            vec![
                "-map", "1:v:0", "-map", "0:a:0", "-c:v:0", "copy", "-c:a:0", "aac",
            ],
        ),
    ] {
        let output = dir.join("output.mp4");
        let reference = dir.join(format!("{name}-reference.mp4"));
        let mut original = vec!["-nostdin", "-v", "error", "-y"];
        let mut args = original.clone();
        for path in [&first, &second].into_iter().take(inputs) {
            original.extend(["-i", path.to_str().unwrap()]);
            args.extend(audio_inputs.iter().copied());
            args.extend(["-i", path.to_str().unwrap()]);
        }
        original.extend(options.iter().copied());
        args.extend(options);
        let pin = Command::new(&ffmpeg_bin)
            .args(&args)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&pin.stderr)
        );
        let expected = std::fs::read(&reference).unwrap();
        assert!(!expected.is_empty(), "{name}");
        args.push(output.to_str().unwrap());
        original.push(output.to_str().unwrap());
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = || -> BTreeSet<_> {
                std::fs::read_dir(&dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect()
            };
            let before = entries();
            for strict_args in [&original, &args] {
                let mut strict = Command::new(bin());
                strict
                    .args(["--strict-safe", "ffmpeg"])
                    .args(strict_args)
                    .env_remove("COREAV_FALLBACK_LOG")
                    .env_remove("COREAV_FALLBACK_ACTIVE");
                #[cfg(unix)]
                strict
                    .env("COREAV_FALLBACK", &recorder)
                    .env("COREAV_FFMPEG", &recorder)
                    .env("COREAV_TEST_STARTED", dir.join("external-started"));
                let refused = strict.output().unwrap();
                assert_eq!(
                    refused.status.code(),
                    Some(4),
                    "{name}: {}",
                    String::from_utf8_lossy(&refused.stderr)
                );
                assert!(refused.stdout.is_empty(), "{name}");
                assert_eq!(entries(), before, "{name}");
                assert_eq!(std::fs::read(&neighbor).unwrap(), b"unrelated staging file");
                if existing {
                    assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                } else {
                    assert!(!output.exists(), "{name}");
                }
            }
            let log = dir.join("fallback.jsonl");
            let fallback = Command::new(bin())
                .arg("ffmpeg")
                .args(&args)
                .env("COREAV_FALLBACK", &ffmpeg_bin)
                .env("COREAV_FALLBACK_LOG", &log)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                fallback.status.code(),
                pin.status.code(),
                "{name}: {}",
                String::from_utf8_lossy(&fallback.stderr)
            );
            assert_eq!(fallback.stdout, pin.stdout, "{name}");
            assert_eq!(std::fs::read(&output).unwrap(), expected, "{name}");
            let receipt: Value = serde_json::from_slice(&std::fs::read(&log).unwrap()).unwrap();
            assert_eq!(receipt["ran"], ffmpeg_bin.to_str().unwrap(), "{name}");
            std::fs::remove_file(&log).unwrap();
            let mut after = before;
            after.insert(output.file_name().unwrap().to_os_string());
            assert_eq!(entries(), after, "{name}");
            assert_eq!(std::fs::read(&neighbor).unwrap(), b"unrelated staging file");
            #[cfg(unix)]
            {
                let replay = Command::new(bin())
                    .arg("ffmpeg")
                    .args(&args)
                    .env("COREAV_FALLBACK", &recorder)
                    .env("COREAV_TEST_STARTED", dir.join("external-started"))
                    .env_remove("COREAV_FALLBACK_LOG")
                    .env_remove("COREAV_STRICT_SAFE")
                    .env_remove("COREAV_FALLBACK_ACTIVE")
                    .output()
                    .unwrap();
                assert_eq!(
                    replay.status.code(),
                    Some(19),
                    "{name}: {}",
                    String::from_utf8_lossy(&replay.stderr)
                );
                assert_eq!(replay.stdout, fallback_arguments(&args), "{name}");
                assert_eq!(
                    std::fs::read(dir.join("external-started")).unwrap(),
                    b"started",
                    "{name}"
                );
                std::fs::remove_file(dir.join("external-started")).unwrap();
                assert_eq!(std::fs::read(&output).unwrap(), expected, "{name}");
                assert_eq!(entries(), after, "{name}");
            }
            std::fs::remove_file(&output).unwrap();
        }
    }
}

#[test]
fn a_stream_copy_runs_and_prints_nothing() {
    let dir = scratch("ffmpeg-copy");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let r = ffmpeg(&["-i", &input, "-c", "copy", o]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.is_empty(), "{}", r.stderr);
    assert!(std::fs::metadata(&output).unwrap().len() > 0);
    // The output exists: ffmpeg would ask, -n refuses, -y replaces.
    assert_eq!(ffmpeg(&["-i", &input, "-c", "copy", o]).code, 4);
    assert_eq!(ffmpeg(&["-n", "-i", &input, "-c", "copy", o]).code, 2);
    assert_eq!(ffmpeg(&["-y", "-i", &input, "-c", "copy", o]).code, 0);
}

#[test]
fn leading_negative_maps_preserve_destinations_and_match_pinned_fallback() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-leading-negative-maps");
    let first = dir.join("first.mp4");
    let second = dir.join("second.mp4");
    for (path, size, frequency) in [(&first, "64x48", "751"), (&second, "80x64", "919")] {
        let result = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size={size}:rate=24:duration=1"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={frequency}:sample_rate=44100:duration=1"
            ))
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "0",
                "-c:a",
                "aac",
                "-metadata",
            ])
            .arg(format!("title={size}"))
            .arg(path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let path = std::env::join_paths(
        std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    for (inputs, maps) in [
        (2, vec!["-1:a", "0"]),
        (2, vec!["-0:a", "0"]),
        (2, vec!["-1:s", "0"]),
        (2, vec!["-0:a", "-1:v", "0"]),
        (2, vec!["-1:a"]),
        (1, vec!["-0:a", "0"]),
    ] {
        let mut args = vec!["-y", "-i", first.to_str().unwrap()];
        if inputs == 2 {
            args.extend(["-i", second.to_str().unwrap()]);
        }
        args.extend(["-c", "copy"]);
        for map in &maps {
            args.extend(["-map", *map]);
        }
        let output = dir.join("refused.mp4");
        let reference = dir.join("rejected-reference.mp4");
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
                std::fs::write(&reference, b"existing output").unwrap();
            }
            let pin = Command::new(&ffmpeg_bin)
                .args(["-nostdin", "-v", "error"])
                .args(&args)
                .arg(&reference)
                .output()
                .unwrap();
            assert!(!pin.status.success(), "{maps:?}");
            assert!(String::from_utf8_lossy(&pin.stderr).contains("matches no streams"));
            let count = std::fs::read_dir(&dir).unwrap().count();
            let mut full = args.clone();
            full.push(output.to_str().unwrap());
            let strict = ffmpeg(&full);
            assert_eq!(strict.code, 4, "{maps:?}: {}", strict.stderr);
            assert!(
                strict
                    .stderr
                    .contains("negative -map before any positive -map"),
                "{}",
                strict.stderr
            );
            assert!(strict.stdout.is_empty());
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                assert_eq!(std::fs::read(&reference).unwrap(), b"existing output");
            } else {
                assert!(!output.exists() && !reference.exists());
            }
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), count);
            let fallback = Command::new(bin())
                .arg("ffmpeg")
                .args(&full)
                .env("PATH", &path)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(
                fallback.status.code(),
                pin.status.code(),
                "{}",
                String::from_utf8_lossy(&fallback.stderr)
            );
            assert_eq!(fallback.stdout, pin.stdout);
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory = dir.join("fallback-bin");
            std::fs::create_dir_all(&directory).unwrap();
            let program = directory.join("ffmpeg");
            std::fs::write(&program, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut full = args.clone();
            full.push(output.to_str().unwrap());
            let replay = Command::new(bin())
                .arg("ffmpeg")
                .args(&full)
                .env_clear()
                .env("PATH", &directory)
                .output()
                .unwrap();
            assert_eq!(replay.status.code(), Some(19));
            assert_eq!(
                replay.stdout,
                fallback_arguments(&full),
                "stage=fallback_argument_replay"
            );
            assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
        }
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(reference).unwrap();
    }
    let observe = |file: &Path| -> Value {
        let result = Command::new(&ffprobe_bin).args(["-v", "error", "-show_streams", "-show_packets", "-show_format", "-show_data_hash", "sha256", "-show_entries",
            "stream=codec_name,codec_type,time_base,start_pts,duration_ts,width,height,sample_rate,channels:stream_tags=language:packet=stream_index,pts,dts,duration,size,data_hash:format=start_time,duration:format_tags=title", "-of", "json"])
            .arg(file).output().unwrap();
        assert!(result.status.success() && result.stderr.is_empty());
        let mut result: Value = serde_json::from_slice(&result.stdout).unwrap();
        let packets = result["packets"].take();
        let mut per_stream = vec![Vec::new(); result["streams"].as_array().unwrap().len()];
        for packet in packets.as_array().unwrap() {
            per_stream[packet["stream_index"].as_u64().unwrap() as usize].push(packet.clone());
        }
        result["packets"] = serde_json::to_value(per_stream).unwrap();
        result
    };
    for maps in [
        vec!["0", "-1:a"],
        vec!["0", "-0:a"],
        vec!["0", "-0", "0:a"],
        vec!["0", "-0", "-1:a", "0:a"],
        vec!["1", "-1:a", "0:a"],
        vec!["0", "-0:v", "1:v"],
    ] {
        let mut args = vec![
            "-y",
            "-i",
            first.to_str().unwrap(),
            "-i",
            second.to_str().unwrap(),
            "-c",
            "copy",
        ];
        for map in &maps {
            args.extend(["-map", *map]);
        }
        let output = dir.join("native.mp4");
        let reference = dir.join("reference.mp4");
        let mut full = args.clone();
        full.push(output.to_str().unwrap());
        let result = ffmpeg(&full);
        assert_eq!(result.code, 0, "{maps:?}: {}", result.stderr);
        let pin = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(&args)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{}",
            String::from_utf8_lossy(&pin.stderr)
        );
        assert_eq!(observe(&output), observe(&reference), "{maps:?}");
    }
    for maps in [vec!["-1:a?", "0"], vec!["0:s?", "-1:a", "0"]] {
        let mut args = vec![
            "-y",
            "-i",
            first.to_str().unwrap(),
            "-i",
            second.to_str().unwrap(),
            "-c",
            "copy",
        ];
        for map in maps {
            args.extend(["-map", map]);
        }
        let output = dir.join("optional.mp4");
        let reference = dir.join("optional-reference.mp4");
        let mut full = args.clone();
        full.push(output.to_str().unwrap());
        let strict = ffmpeg(&full);
        assert_eq!(strict.code, 4);
        assert!(strict.stderr.contains("optional -map"));
        assert!(!output.exists());
        let pin = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(&args)
            .arg(&reference)
            .output()
            .unwrap();
        let fallback = Command::new(bin())
            .arg("ffmpeg")
            .args(&full)
            .env("PATH", &path)
            .env_remove("COREAV_STRICT_SAFE")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert_eq!(fallback.status.code(), pin.status.code());
        if pin.status.success() {
            assert_eq!(
                std::fs::read(&output).unwrap(),
                std::fs::read(&reference).unwrap()
            );
            std::fs::remove_file(&output).unwrap();
            std::fs::remove_file(&reference).unwrap();
        } else {
            assert!(!output.exists() && !reference.exists());
        }
    }
    let output = dir.join("all-removed.mp4");
    let args = [
        "-y",
        "-i",
        first.to_str().unwrap(),
        "-i",
        second.to_str().unwrap(),
        "-c",
        "copy",
        "-map",
        "0",
        "-map",
        "-0",
    ];
    let mut full = args.to_vec();
    full.push(output.to_str().unwrap());
    assert_eq!(ffmpeg(&full).code, 2);
    let pin = Command::new(&ffmpeg_bin)
        .args(["-nostdin", "-v", "error"])
        .args(args)
        .arg(&output)
        .output()
        .unwrap();
    assert!(!pin.status.success() && !output.exists());
}

#[test]
fn mapped_inputs_copy_streams_in_map_order() {
    let dir = scratch("ffmpeg-mapped-inputs");
    let input = movie(&dir);
    for (case, count, maps, kinds) in [
        (
            "all-and-audio",
            2,
            vec!["0", "1:a"],
            vec![
                coreav_isobmff::TrackKind::Video,
                coreav_isobmff::TrackKind::Audio,
                coreav_isobmff::TrackKind::Audio,
            ],
        ),
        (
            "separate-av",
            2,
            vec!["0:v", "1:a"],
            vec![
                coreav_isobmff::TrackKind::Video,
                coreav_isobmff::TrackKind::Audio,
            ],
        ),
        (
            "three-inputs",
            3,
            vec!["0:0", "1:0", "2:0", "2:1"],
            vec![
                coreav_isobmff::TrackKind::Video,
                coreav_isobmff::TrackKind::Video,
                coreav_isobmff::TrackKind::Video,
                coreav_isobmff::TrackKind::Audio,
            ],
        ),
        (
            "remove-only-second-audio",
            2,
            vec!["0", "1:a", "-1:a"],
            vec![
                coreav_isobmff::TrackKind::Video,
                coreav_isobmff::TrackKind::Audio,
            ],
        ),
    ] {
        let output = dir.join(format!("{case}.mp4"));
        let mut args: Vec<&str> = (0..count).flat_map(|_| ["-i", input.as_str()]).collect();
        for map in maps {
            args.extend(["-map", map]);
        }
        args.extend(["-c", "copy", output.to_str().unwrap()]);
        let run = ffmpeg(&args);
        assert_eq!(run.code, 0, "{case}: {}", run.stderr);
        let data = std::fs::read(output).unwrap();
        let result = coreav_isobmff::probe(
            coreav_isobmff::SliceSource::new(&data),
            &coreav_isobmff::Limits::default(),
            &coreav_isobmff::Allowlist::default(),
        )
        .unwrap();
        assert_eq!(
            result.tracks.iter().map(|t| t.kind).collect::<Vec<_>>(),
            kinds
        );
    }
    let output = dir.join("refused.mp4");
    for options in [
        vec!["-ss", "0.5"],
        vec!["-f", "ipod"],
        vec!["-c:v", "libx264"],
    ] {
        let mut args = vec![
            "-i", &input, "-i", &input, "-map", "0:v", "-map", "1:a", "-c", "copy",
        ];
        args.extend(options);
        args.push(output.to_str().unwrap());
        assert_eq!(ffmpeg(&args).code, 4, "{args:?}");
        assert!(!output.exists());
    }
    let original = std::fs::read(&input).unwrap();
    assert_eq!(
        ffmpeg(&[
            "-y", "-i", &input, "-i", &input, "-map", "1", "-c", "copy", &input
        ])
        .code,
        2
    );
    assert_eq!(std::fs::read(&input).unwrap(), original);
}

#[test]
fn mapped_inputs_refuse_missing_default_dispositions() {
    let dir = scratch("ffmpeg-mapped-inputs-no-default");
    let mut data = av_movie_with(aac_entry(), false);
    let headers: Vec<usize> = data
        .windows(4)
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == b"tkhd").then_some(offset))
        .collect();
    assert_eq!(headers.len(), 2);
    for offset in headers {
        data[offset + 7] &= !1;
    }
    let input = dir.join("no-default.mp4");
    std::fs::write(&input, data).unwrap();
    let input = input.to_str().unwrap();
    let output = dir.join("out.mp4");
    let run = ffmpeg(&[
        "-i",
        input,
        "-i",
        input,
        "-map",
        "0:v",
        "-map",
        "1:v",
        "-c",
        "copy",
        output.to_str().unwrap(),
    ]);
    assert_eq!(run.code, 4, "{}", run.stderr);
    assert!(run.stderr.contains("default disposition"), "{}", run.stderr);
    assert!(!output.exists());
}

#[test]
fn mapped_inputs_agree_with_the_pinned_ffmpeg() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-mapped-inputs-oracle");
    let inputs: Vec<PathBuf> = (0..3)
        .map(|i| dir.join(format!("source-{i}.mp4")))
        .collect();
    for (index, path) in inputs.iter().enumerate() {
        let made = Command::new(&ffmpeg_bin)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!(
                    "testsrc2=size=64x64:rate=10:duration=1,hue=h={}",
                    index * 45
                ),
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                &format!("sine=frequency={}:duration=1", 440 + index * 110),
            ])
            .args(["-c:v", "libx264", "-bf", "0", "-c:a", "aac"])
            .args(["-metadata", &format!("title=source-{index}")])
            .args([
                "-metadata:s:a:0",
                &format!("language={}", ["eng", "fra", "deu"][index]),
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stderr)
        );
    }
    let capture = |path: &Path| {
        let output = Command::new(&ffprobe_bin).args(["-v", "error", "-of", "json", "-show_packets", "-show_data_hash", "MD5", "-show_entries", "stream=codec_type,codec_name,width,height,time_base,sample_rate,channels:stream_tags=language,handler_name:stream_disposition=default:format_tags=title:packet=stream_index,pts_time,dts_time,duration_time,data_hash,flags"])
            .arg(path).output().unwrap();
        assert!(output.status.success());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let originals: Vec<Value> = inputs.iter().map(|path| capture(path)).collect();
    for stream in [0, 1] {
        let distinct: BTreeSet<Vec<String>> = originals
            .iter()
            .map(|document| {
                let hashes: Vec<String> = document["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|packet| packet["stream_index"] == stream)
                    .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
                    .collect();
                assert!(!hashes.is_empty());
                hashes
            })
            .collect();
        assert_eq!(distinct.len(), inputs.len(), "stream {stream}");
    }
    for (name, count, maps) in [
        ("all-and-audio", 2, vec!["0", "1:a"]),
        ("separate-av", 2, vec!["0:v", "1:a"]),
        ("three-inputs", 3, vec!["0:0", "1:0", "2:0", "2:1"]),
        ("remove-second-audio", 2, vec!["0", "1:a", "-1:a"]),
        ("repeated-audio-first", 2, vec!["1:a", "0:v", "1:a"]),
    ] {
        let mut args = Vec::new();
        for path in inputs.iter().take(count) {
            args.extend(["-i", path.to_str().unwrap()]);
        }
        for map in maps {
            args.extend(["-map", map]);
        }
        args.extend(["-c", "copy"]);
        let output = dir.join(format!("{name}.mp4"));
        let oracle = dir.join(format!("{name}-oracle.mp4"));
        let expected = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(&args)
            .arg(&oracle)
            .output()
            .unwrap();
        assert!(
            expected.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&expected.stderr)
        );
        args.push(output.to_str().unwrap());
        let run = ffmpeg(&args);
        assert_eq!(run.code, 0, "{name}: {}", run.stderr);
        let mut actual = capture(&output);
        let mut expected = capture(&oracle);
        for result in [&mut actual, &mut expected] {
            result["packets"].as_array_mut().unwrap().sort_by_key(|p| {
                (
                    p["stream_index"].as_i64().unwrap(),
                    p["dts_time"].as_str().unwrap().to_owned(),
                )
            });
        }
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn commands_outside_the_vocabulary_are_refused_by_name() {
    let dir = scratch("ffmpeg-refused");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let i = input.as_str();
    for (args, named) in [
        (vec!["-i", i, "-c:v", "libx265", o], "libx265"),
        (
            vec!["-i", i, "-vf", "scale=320:-2", "-c:a", "copy", o],
            "-vf",
        ),
        (vec!["-f", "mpegts", "-i", i, "-c", "copy", o], "mpegts"),
        (vec!["-i", i, "-c", "copy", "-f", "hls", o], "hls"),
        (
            vec!["-i", i, "-c", "copy", "-movflags", "+frag_keyframe", o],
            "frag_keyframe",
        ),
        (vec!["-h", "long"], "-h long"),
        (vec!["-h", "full"], "-h full"),
        (vec!["--help", "encoder=libx264"], "--help encoder=libx264"),
        (vec!["-help", o], "-help"),
        (vec!["-filter_complex", "null", "-h"], "-filter_complex"),
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, 4, "{args:?}: {}", r.stderr);
        assert!(r.stdout.is_empty(), "{args:?}");
        assert!(r.stderr.contains(named), "{args:?}: {}", r.stderr);
        assert!(!output.exists(), "{args:?}");
    }
}

#[test]
fn help_without_a_topic_is_the_usage_text() {
    for args in [
        vec!["-h"],
        vec!["-?"],
        vec!["-help"],
        vec!["--help"],
        vec!["-hide_banner", "-h"],
        vec!["-i", "in.mp4", "-c", "copy", "--help"],
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, 0, "{args:?}: {}", r.stderr);
        assert_eq!(
            String::from_utf8(r.stdout).unwrap(),
            coreav_cli::ffmpeg::USAGE,
            "{args:?}"
        );
        assert!(r.stderr.is_empty(), "{args:?}: {}", r.stderr);
    }
}

#[test]
fn a_refusal_carries_its_reason() {
    let refusal = |list: &[&str]| {
        let args: Vec<String> = list.iter().map(|a| (*a).to_owned()).collect();
        match coreav_cli::ffmpeg::run(&args, StrictSafe::new(false)) {
            Err(Stop::Refused(Refusal::NotNative(refusal))) => refusal.reason,
            _ => panic!("{list:?} was not refused"),
        }
    };
    assert_eq!(
        refusal(&["-i", "in.mp4", "-c:v", "libx265", "out.mp4"]),
        Reason::Option("-c:v".to_owned(), Some("libx265".to_owned()))
    );
    assert_eq!(
        refusal(&["-f", "mpegts", "-i", "in.ts", "-c", "copy", "out.mp4"]),
        Reason::Option("-f".to_owned(), Some("mpegts".to_owned()))
    );
    assert_eq!(
        refusal(&["-i", "a.mp4", "-i", "b.mp4", "-c", "copy", "out.mp4"]),
        Reason::Property("a command with several inputs")
    );
}

#[test]
fn malformed_commands_are_usage_errors_and_a_missing_input_is_named() {
    let dir = scratch("ffmpeg-usage");
    let o = dir.join("out.mp4");
    let o = o.to_str().unwrap();
    assert_eq!(ffmpeg(&["-c", "copy", o]).code, 2);
    assert_eq!(ffmpeg(&["-i"]).code, 2);
    let r = ffmpeg(&["-i", "/nonexistent/in.mp4", "-c", "copy", o]);
    assert_eq!(r.code, 3, "{}", r.stderr);
    assert!(r.stderr.contains("/nonexistent/in.mp4"), "{}", r.stderr);
}

#[test]
fn frame_plans_validate_common_options() {
    let dir = scratch("ffmpeg-frame-plan-options");
    let input = movie(&dir);
    let size = std::fs::metadata(&input).unwrap().len();
    let mut file = coreav_isobmff::FileSource::open(&input).unwrap();
    let Ok(probed) = coreav_cli::ffprobe::probe(&mut file, size, &input, None) else {
        panic!("{input} is rejected");
    };
    let report = probed.report;
    for output in ["out.png", "out.bmp", "out.yuv", "out.y4m"] {
        let plan = |options: &str| {
            let args: Vec<String> = ["-i", &input]
                .into_iter()
                .chain(options.split_whitespace())
                .chain([output])
                .map(str::to_owned)
                .collect();
            let command = coreav_cli::ffmpeg::args::parse(&args)?;
            coreav_cli::ffmpeg::plan(&command, &report, None)
        };
        for options in ["", "-threads 1", "-pix_fmt yuv420p"] {
            assert!(
                matches!(plan(options), Ok(Job::Frames { .. })),
                "{output}: {options}"
            );
        }
        for options in [
            "-map_metadata -1",
            "-map_metadata 0:s:0",
            "-map_metadata 0:s:999",
            "-crf 23",
            "-b:v 100k",
            "-preset fast",
            "-vf scale=32:24",
            "-s 32x24",
            "-c:a aac -b:a 64k",
            "-c:a libmp3lame -q:a 2",
            "-c:a libmp3lame -qscale:a 2",
            "-ar 48000",
            "-ac 2",
            "-fps_mode passthrough",
        ] {
            let result = plan(options);
            assert!(
                matches!(result, Err(Refusal::NotNative(_))),
                "{output}: {options}: {result:?}"
            );
        }
    }
}

#[test]
fn frame_metadata_refusals_create_no_output() {
    let dir = scratch("ffmpeg-frame-metadata");
    let input = movie(&dir);
    for name in ["out.png", "out.bmp", "out.yuv", "out.y4m"] {
        let output = dir.join(name);
        for mapping in ["-1", "0:s:0", "0:s:999"] {
            let r = ffmpeg(&[
                "-i",
                &input,
                "-map_metadata",
                mapping,
                "-frames:v",
                "1",
                output.to_str().unwrap(),
            ]);
            assert_eq!(r.code, 4, "{name} {mapping}: {}", r.stderr);
            assert!(r.stderr.contains("-map_metadata"), "{}", r.stderr);
            assert!(r.stdout.is_empty());
            assert!(!output.exists());
        }
    }
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
}

#[test]
fn a_concat_list_is_joined() {
    let dir = scratch("ffmpeg-concat");
    let input = movie(&dir);
    vp9_file(&dir.join("in.mp4"));
    let list = dir.join("list.txt");
    std::fs::write(&list, "ffconcat version 1.0\nfile in.mp4\nfile 'in.mp4'\n").unwrap();
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let l = list.to_str().unwrap();
    let args: Vec<String> = ["-f", "concat", "-i", l, "-c", "copy", o]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let Ok(summary) = coreav_cli::ffmpeg::run(&args, StrictSafe::new(false)) else {
        panic!("{args:?} did not run");
    };
    assert_eq!(summary["schema"], "coreav.join/1");
    assert_eq!(summary["inputs"].as_array().unwrap().len(), 2, "{summary}");
    assert_eq!(summary["inputs"][0]["streams"].as_array().unwrap().len(), 2);
    let r = ffmpeg(&[
        "-y", "-f", "concat", "-safe", "0", "-i", l, "-an", "-c", "copy", o,
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    // A list file is read from the list's directory; the same path in it
    // named absolutely needs -safe 0, as in ffmpeg.
    let absolute = dir.join("absolute.txt");
    std::fs::write(&absolute, format!("file '{input}'\n")).unwrap();
    let a = absolute.to_str().unwrap();
    std::fs::remove_file(&output).unwrap();
    let r = ffmpeg(&["-f", "concat", "-i", a, "-c", "copy", o]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("unsafe file name"), "{}", r.stderr);
    assert!(!output.exists());
    assert_eq!(
        ffmpeg(&["-f", "concat", "-safe", "0", "-i", a, "-c", "copy", o]).code,
        0
    );
}

/// A join copies the audio stream `ffmpeg` selects or refuses the
/// command: AMR, which the join does not copy, is refused (exit status 4),
/// never left out of an output written without it.
#[test]
fn a_join_refuses_audio_it_does_not_copy() {
    let dir = scratch("ffmpeg-concat-amr");
    std::fs::write(dir.join("in.mp4"), av_movie(b"samr")).unwrap();
    vp9_file(&dir.join("in.mp4"));
    let list = dir.join("list.txt");
    std::fs::write(&list, "file in.mp4\nfile in.mp4\n").unwrap();
    let output = dir.join("out.mp4");
    let (l, o) = (list.to_str().unwrap(), output.to_str().unwrap());
    let r = ffmpeg(&["-f", "concat", "-i", l, "-c", "copy", o]);
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(
        r.stderr.contains("a codec that CoreAV does not copy"),
        "{}",
        r.stderr
    );
    assert!(!output.exists());
    let r = ffmpeg(&["-f", "concat", "-i", l, "-an", "-c", "copy", o]);
    assert_eq!(r.code, 0, "{}", r.stderr);
}

/// [`movie`]'s file with other tracks: `traks` takes its video and audio
/// `trak` boxes and returns the boxes the movie holds instead, in order.
fn retracked(traks: impl Fn(&[u8], &[u8]) -> Vec<Vec<u8>>) -> Vec<u8> {
    let mut bytes = av_movie_with(aac_entry(), false);
    let boxes = |kind: &[u8]| -> Vec<usize> {
        (4..bytes.len())
            .filter(|&i| bytes[i..].starts_with(kind))
            .map(|i| i - 4)
            .collect()
    };
    let (moov, at) = (boxes(b"moov")[0], boxes(b"trak"));
    // The audio `trak` ends the movie box, which ends the file.
    let new = traks(&bytes[at[0]..at[1]], &bytes[at[1]..]).concat();
    bytes.truncate(at[0]);
    bytes.extend_from_slice(&new);
    let size = u32::try_from(bytes.len() - moov).unwrap();
    bytes[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
    bytes
}

fn vp9_video(video: &[u8]) -> Vec<u8> {
    let mut video = video.to_vec();
    let stsd = video.windows(4).position(|word| word == b"stsd").unwrap();
    let start = stsd + 12;
    let old = u32::from_be_bytes(video[start..start + 4].try_into().unwrap()) as usize;
    let entry = common::visual_entry(b"vp09", &full(b"vpcC", 1, 0, &[0, 10, 0x82, 1, 1, 1, 0, 0]));
    for kind in [b"trak", b"mdia", b"minf", b"stbl", b"stsd"] {
        let at = video.windows(4).position(|word| word == kind).unwrap() - 4;
        let size = u32::from_be_bytes(video[at..at + 4].try_into().unwrap()) as usize;
        video[at..at + 4].copy_from_slice(&((size + entry.len() - old) as u32).to_be_bytes());
    }
    video.splice(start..start + old, entry);
    video
}

fn vp9_file(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let moov = bytes.windows(4).position(|word| word == b"moov").unwrap() - 4;
    let start = bytes.windows(4).position(|word| word == b"trak").unwrap() - 4;
    let size = u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
    let video = vp9_video(&bytes[start..start + size]);
    let old = u32::from_be_bytes(bytes[moov..moov + 4].try_into().unwrap()) as usize;
    bytes[moov..moov + 4].copy_from_slice(&((old + video.len() - size) as u32).to_be_bytes());
    bytes.splice(start..start + size, video);
    std::fs::write(path, bytes).unwrap();
}

/// `trak` (as [`av_movie_with`] writes it, `tkhd` first) with track ID
/// `id`, enabled (the default disposition) or not, and the first run of
/// the bytes `from` written over with `to`.
fn track(trak: &[u8], id: u32, enabled: bool, from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut trak = trak.to_vec();
    trak[19] = if enabled { 3 } else { 2 };
    trak[28..32].copy_from_slice(&id.to_be_bytes());
    let at = (0..trak.len())
        .find(|&i| trak[i..].starts_with(from))
        .unwrap();
    trak[at..at + to.len()].copy_from_slice(to);
    trak
}

/// Streams are written in the order `ffmpeg` maps them (its `Stream
/// mapping`, black box): the automatic selection's video before its audio
/// whatever their order in the file, `-map`s in their order, a stream
/// mapped twice as two tracks. A clip writes its video first, so a clip
/// that maps its audio first is refused.
#[test]
fn streams_are_written_in_the_order_they_are_mapped() {
    let dir = scratch("ffmpeg-order");
    let input = movie(&dir);
    let audio_first = dir.join("audio-first.mp4");
    std::fs::write(&audio_first, retracked(|v, a| vec![a.to_vec(), v.to_vec()])).unwrap();
    let audio_first = audio_first.to_str().unwrap();
    assert_eq!(listed(Path::new(audio_first)).0, [A, V]);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    for (args, expected) in [
        (vec!["-i", audio_first, "-c", "copy"], vec![V, A]),
        (
            vec![
                "-i",
                audio_first,
                "-map",
                "0:v",
                "-map",
                "0:a",
                "-c",
                "copy",
            ],
            vec![V, A],
        ),
        (
            vec!["-i", &input, "-map", "0:a", "-map", "0:v", "-c", "copy"],
            vec![A, V],
        ),
        (
            vec!["-i", &input, "-map", "0:v", "-map", "0", "-c", "copy"],
            vec![V, V, A],
        ),
        (
            vec!["-i", audio_first, "-ss", "0", "-t", "1", "-c", "copy"],
            vec![V, A],
        ),
    ] {
        let r = ffmpeg(&[&["-y"], args.as_slice(), &[o]].concat());
        assert_eq!(r.code, 0, "{args:?}: {}", r.stderr);
        assert_eq!(listed(&output).0, expected, "{args:?}");
    }
    let r = ffmpeg(&[
        "-y", "-i", &input, "-map", "0:a", "-map", "0:v", "-ss", "0", "-t", "1", "-c", "copy", o,
    ]);
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(
        r.stderr.contains("audio selected before the video"),
        "{}",
        r.stderr
    );
}

/// More output streams than CoreAV writes to one file (`Limits::max_tracks`)
/// are refused before any stream is planned, counted once `-an` and the
/// negative maps have removed theirs: 3,000 `-map 0` on a file of two
/// streams (near the longest command line Windows starts, 32,767
/// characters) would otherwise plan 6,000 outputs, and 513 `-map 0` less
/// the audio are 513 video tracks.
#[test]
fn a_selection_larger_than_an_output_holds_is_refused() {
    let dir = scratch("ffmpeg-many-maps");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    for less in [vec!["-an"], vec!["-map", "-0:a"]] {
        let mut args = vec!["-y", "-i", input.as_str()];
        for _ in 0..513 {
            args.extend(["-map", "0"]);
        }
        args.extend(less.iter().copied());
        args.extend(["-c", "copy", o]);
        let r = ffmpeg(&args);
        assert_eq!(r.code, 0, "{less:?}: {}", r.stderr);
        assert_eq!(listed(&output).0, vec![V; 513], "{less:?}");
    }
    std::fs::remove_file(&output).unwrap();
    for maps in [513, 3_000] {
        let mut args = vec!["-y", "-i", input.as_str()];
        for _ in 0..maps {
            args.extend(["-map", "0"]);
        }
        args.extend(["-c", "copy", o]);
        let started = std::time::Instant::now();
        let r = ffmpeg(&args);
        assert_eq!(r.code, 4, "{maps}: {}", r.stderr);
        assert!(
            r.stderr
                .contains("more output streams than CoreAV writes to one file"),
            "{}",
            r.stderr
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(30));
        assert!(!output.exists());
    }
}

/// A join copies the video stream `ffmpeg` selects or refuses the command:
/// of two video streams of one size, the first, which the join does not
/// copy (MPEG-4 Visual), is refused (exit status 4), never replaced by the
/// other. Named by `-map`, the other is joined.
#[test]
fn a_join_refuses_video_it_does_not_copy() {
    let dir = scratch("ffmpeg-concat-mp4v");
    let movie = retracked(|video, audio| {
        vec![
            track(video, 1, true, b"avc1", b"mp4v"),
            track(&vp9_video(video), 3, true, b"vp09", b"vp09"),
            audio.to_vec(),
        ]
    });
    std::fs::write(dir.join("in.mp4"), movie).unwrap();
    let list = dir.join("list.txt");
    std::fs::write(&list, "file in.mp4\nfile in.mp4\n").unwrap();
    let output = dir.join("out.mp4");
    let (l, o) = (list.to_str().unwrap(), output.to_str().unwrap());
    let r = ffmpeg(&["-f", "concat", "-i", l, "-c", "copy", o]);
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(
        r.stderr.contains("a codec that CoreAV does not copy"),
        "{}",
        r.stderr
    );
    assert!(!output.exists());
    let args: Vec<String> = [
        "-f", "concat", "-i", l, "-map", "0:1", "-map", "0:2", "-c", "copy", o,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let Ok(summary) = coreav_cli::ffmpeg::run(&args, StrictSafe::new(false)) else {
        panic!("{args:?} did not run");
    };
    for input in summary["inputs"].as_array().unwrap() {
        let streams = input["streams"].as_array().unwrap();
        let indexes: Vec<&Value> = streams.iter().map(|s| &s["stream_index"]).collect();
        assert_eq!(indexes, [1, 2], "{summary}");
    }
}

/// The concat demuxer exposes the first file's streams with every
/// disposition flag cleared (black box: `ffprobe` of a list), so `ffmpeg`
/// selects among them by picture size and channels alone: of a mono AAC
/// stream with the default disposition and a stereo one without, the
/// stereo one, which the join copies.
#[test]
fn a_join_selects_the_streams_the_concat_demuxer_exposes() {
    let dir = scratch("ffmpeg-concat-selection");
    let channels = |n: u8| -> Vec<u8> { [&b"mp4a"[..], &[0; 7], &[1], &[0; 9], &[n]].concat() };
    let movie = retracked(|video, audio| {
        vec![
            vp9_video(video),
            track(audio, 2, true, &channels(2), &channels(1)),
            track(audio, 3, false, &channels(2), &channels(2)),
        ]
    });
    std::fs::write(dir.join("in.mp4"), movie).unwrap();
    let probed = stream_channels(&dir.join("in.mp4"));
    assert_eq!(probed, [None, Some(1), Some(2)]);
    let list = dir.join("list.txt");
    std::fs::write(&list, "file in.mp4\nfile in.mp4\n").unwrap();
    let output = dir.join("out.mp4");
    let args: Vec<String> = [
        "-f",
        "concat",
        "-i",
        list.to_str().unwrap(),
        "-c",
        "copy",
        output.to_str().unwrap(),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let Ok(summary) = coreav_cli::ffmpeg::run(&args, StrictSafe::new(false)) else {
        panic!("{args:?} did not run");
    };
    for input in summary["inputs"].as_array().unwrap() {
        let streams = input["streams"].as_array().unwrap();
        let indexes: Vec<&Value> = streams.iter().map(|s| &s["stream_index"]).collect();
        assert_eq!(indexes, [0, 2], "{summary}");
    }
}

/// [`movie`] with words changed: each `(n, from, to)` writes the 32-bit
/// big-endian words `to` over the `n`th (from 1) run of the words `from`.
fn patched_movie(dir: &Path, name: &str, patch: &[(usize, &[u32], &[u32])]) -> String {
    let mut bytes = std::fs::read(movie(dir)).unwrap();
    let be = |words: &[u32]| -> Vec<u8> { words.iter().flat_map(|w| w.to_be_bytes()).collect() };
    for (n, from, to) in patch {
        let (from, to) = (be(from), be(to));
        let at = (0..bytes.len())
            .filter(|&i| bytes[i..].starts_with(&from))
            .nth(n - 1)
            .unwrap();
        bytes[at..at + to.len()].copy_from_slice(&to);
    }
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

/// A join places each file where the concat demuxer does, after the files
/// before it, each as long as its duration (the pinned `ffmpeg` 9.0.2,
/// black box: a file whose audio outlasts its video is followed at the
/// audio's end, and its last video frame is held until then).
#[test]
fn a_join_places_each_file_where_the_concat_demuxer_does() {
    let dir = scratch("ffmpeg-concat-placement");
    movie(&dir);
    // The audio lasts 0.32 s, twice the video (its `stts`, `mdhd` and the
    // movie's duration).
    patched_movie(
        &dir,
        "long.mp4",
        &[
            (2, &[1, 4, 512], &[1, 4, 1024]),
            (2, &[12_800, 2048], &[12_800, 4096]),
            (1, &[1000, 160], &[1000, 320]),
        ],
    );
    vp9_file(&dir.join("long.mp4"));
    let list = dir.join("list.txt");
    std::fs::write(&list, "file long.mp4\nfile long.mp4\n").unwrap();
    let output = dir.join("out.mp4");
    let args: Vec<String> = ["-f", "concat", "-i", list.to_str().unwrap(), "-c", "copy"]
        .iter()
        .map(|a| (*a).to_owned())
        .chain([output.display().to_string()])
        .collect();
    let Ok(summary) = coreav_cli::ffmpeg::run(&args, StrictSafe::new(true)) else {
        panic!("the join is refused");
    };
    let inputs = summary["inputs"].as_array().unwrap();
    assert_eq!(inputs[1]["offset"]["seconds"], json!(0.32));
    assert!(inputs[0]["extended"].as_u64().unwrap() > 0, "{summary:#}");
}

#[test]
fn joins_ffmpeg_would_time_or_read_otherwise_are_refused() {
    let dir = scratch("ffmpeg-concat-not-native");
    movie(&dir);
    // The video's media timescale is 20000 rather than 12800.
    patched_movie(
        &dir,
        "timescale.mp4",
        &[
            (1, &[1, 4, 512], &[1, 4, 800]),
            (1, &[12_800, 2048], &[20_000, 3200]),
        ],
    );
    std::fs::write(dir.join("pcm.mp4"), av_movie(b"sowt")).unwrap();
    // Eight frames of 40 ms, presented by two edits that meet exactly and
    // merge in the join with the next file's single edit.
    let video = |name: &str, elst: Vec<(u32, i32)>| {
        let spec = Spec {
            sizes: vec![100; 8],
            composition: Vec::new(),
            sync: (1..=8).collect(),
            elst: Some(elst),
            ctts: Some(Vec::new()),
            ..Spec::default()
        };
        std::fs::write(dir.join(name), common::movie(&spec)).unwrap();
    };
    video("edits.mp4", vec![(160, 0), (160, 2048)]);
    video("plain.mp4", vec![(320, 0)]);
    // The same tracks with the audio first: the demuxer pairs its first
    // stream with the first file's video, and its second, the video, with
    // the first file's audio.
    let swapped = retracked(|video, audio| vec![audio.to_vec(), video.to_vec()]);
    std::fs::write(dir.join("swapped.mp4"), swapped).unwrap();
    // The video alone: the first file's audio has no stream to pair with.
    let video_only = retracked(|video, _| vec![video.to_vec()]);
    std::fs::write(dir.join("video.mp4"), video_only).unwrap();
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    for (files, named) in [
        ("file in.mp4\nfile timescale.mp4\n", "streams differ"),
        ("file pcm.mp4\nfile pcm.mp4\n", "not supported"),
        (
            "file edits.mp4\nfile plain.mp4\n",
            "tracks have several edits",
        ),
        (
            "file in.mp4\nfile swapped.mp4\n",
            "the stream is not a video track",
        ),
        ("file in.mp4\nfile video.mp4\n", "streams differ"),
    ] {
        let list = dir.join("list.txt");
        std::fs::write(&list, files).unwrap();
        let r = ffmpeg(&[
            "-f",
            "concat",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
            o,
        ]);
        assert_eq!(r.code, 4, "{files:?}: {}", r.stderr);
        assert!(r.stderr.contains(named), "{files:?}: {}", r.stderr);
        assert!(!output.exists(), "{files:?}");
    }
}

fn concat_aac_edit(media_time: u32, later_aac: bool) -> Vec<u8> {
    retracked(|video, audio| {
        let mut audio = audio.to_vec();
        if later_aac {
            let stsd = audio.windows(4).position(|word| word == b"stsd").unwrap();
            let start = stsd + 12;
            let old = u32::from_be_bytes(audio[start..start + 4].try_into().unwrap()) as usize;
            let entries = cat(&[common::audio_entry(b"sowt"), aac_entry()]);
            for kind in [b"mdia", b"minf", b"stbl", b"stsd"] {
                let at = audio.windows(4).position(|word| word == kind).unwrap() - 4;
                let size = u32::from_be_bytes(audio[at..at + 4].try_into().unwrap()) as usize;
                audio[at..at + 4]
                    .copy_from_slice(&((size + entries.len() - old) as u32).to_be_bytes());
            }
            audio[stsd + 8..stsd + 12].copy_from_slice(&2u32.to_be_bytes());
            audio.splice(start..start + old, entries);
        }
        let edits = full(b"elst", 0, 0, &u32s(&[1, 160, media_time, 0x10000]));
        vec![
            vp9_video(video),
            bx(b"trak", &cat(&[audio[8..].to_vec(), bx(b"edts", &edits)])),
        ]
    })
}

#[test]
fn concat_positive_aac_edits_are_refused_without_publishing_output() {
    let dir = scratch("ffmpeg-concat-aac-edits");
    movie(&dir);
    vp9_file(&dir.join("in.mp4"));
    let list = dir.join("list with spaces.txt");
    let output = dir.join("out.mp4");
    let l = list.to_str().unwrap();
    let o = output.to_str().unwrap();
    for later in [false, true] {
        for offset in [0, 512] {
            let bytes = concat_aac_edit(offset, later);
            let parsed = coreav_isobmff::probe(
                coreav_isobmff::SliceSource::new(&bytes),
                &coreav_isobmff::Limits::default(),
                &coreav_isobmff::Allowlist::listing(),
            )
            .unwrap();
            assert_eq!(parsed.tracks[1].edits[0].media_time, i64::from(offset));
            if later {
                assert_eq!(
                    parsed.tracks[1].later_descriptions[0].codec,
                    coreav_isobmff::Codec::Aac
                );
                assert_ne!(
                    parsed.tracks[1].description.codec,
                    coreav_isobmff::Codec::Aac
                );
            }
            std::fs::write(dir.join("edited.mp4"), bytes).unwrap();
            for files in [
                "file edited.mp4\nfile in.mp4\n",
                "file in.mp4\nfile edited.mp4\n",
            ] {
                std::fs::write(&list, files).unwrap();
                for selection in [&[][..], &["-an"][..], &["-map", "0:v:0"][..]] {
                    if later && selection.is_empty() {
                        continue;
                    }
                    let mut args = vec!["-f", "concat", "-safe", "0", "-i", l];
                    args.extend_from_slice(selection);
                    args.extend(["-c", "copy", o]);
                    let before = std::fs::read_dir(&dir).unwrap().count();
                    let r = ffmpeg(&args);
                    assert!(r.stdout.is_empty());
                    if offset == 0 {
                        assert_eq!(
                            r.code, 0,
                            "later={later} {files:?} {selection:?}: {}",
                            r.stderr
                        );
                        std::fs::remove_file(&output).unwrap();
                    } else {
                        assert_eq!(
                            r.code, 4,
                            "later={later} {files:?} {selection:?}: {}",
                            r.stderr
                        );
                        assert!(
                            r.stderr.contains("positive AAC edit offset"),
                            "{}",
                            r.stderr
                        );
                        assert!(!output.exists());
                        std::fs::write(&output, b"keep existing output").unwrap();
                        let r = ffmpeg(&[&["-y"], args.as_slice()].concat());
                        assert_eq!(r.code, 4, "{}", r.stderr);
                        assert_eq!(std::fs::read(&output).unwrap(), b"keep existing output");
                        std::fs::remove_file(&output).unwrap();
                    }
                    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);
                }
            }
        }
    }
    std::fs::write(dir.join("edited.mp4"), concat_aac_edit(512, false)).unwrap();
    let paths = [dir.join("edited.mp4"), dir.join("in.mp4")];
    let options = coreav_isobmff_write::JoinOptions {
        video: coreav_isobmff_write::VideoChoice::First,
        audio: coreav_isobmff_write::JoinAudio::First,
        placement: coreav_isobmff_write::Placement::AfterVideo,
        compatibility: None,
    };
    assert!(coreav_cli::join::run(&paths, &options, &Default::default(), Vec::new()).is_ok());
    let r = ffmpeg(&["-i", paths[0].to_str().unwrap(), "-c", "copy", o]);
    assert_eq!(r.code, 0, "{}", r.stderr);
}

#[cfg(unix)]
#[test]
fn concat_positive_aac_edits_forward_the_original_fallback_arguments() {
    use std::os::unix::fs::PermissionsExt;

    let dir = scratch("ffmpeg-concat-aac-fallback");
    std::fs::write(dir.join("in.mp4"), concat_aac_edit(512, false)).unwrap();
    let list = dir.join("list with spaces.txt");
    std::fs::write(&list, "file in.mp4\nfile in.mp4\n").unwrap();
    let fallback = dir.join("ffmpeg");
    std::fs::write(&fallback, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 23\n").unwrap();
    std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = dir.join("output with spaces.mp4");
    let args = [
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        list.to_str().unwrap(),
        "-an",
        "-c",
        "copy",
        output.to_str().unwrap(),
    ];
    let before = std::fs::read_dir(&dir).unwrap().count();
    let r = Command::new(bin())
        .arg("ffmpeg")
        .args(args)
        .env_clear()
        .env("PATH", &dir)
        .output()
        .unwrap();
    assert_eq!(
        r.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert_eq!(r.stdout, fallback_arguments(&args));
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);
}

#[test]
fn concat_lists_and_options_outside_the_join_are_refused() {
    let dir = scratch("ffmpeg-concat-refused");
    movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let list = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    };
    let trimmed = list("inpoint.txt", "file in.mp4\ninpoint 1\n");
    let plain = list("list.txt", "file in.mp4\nfile in.mp4\n");
    let empty = list("empty.txt", "# nothing\n");
    let missing = dir.join("missing.txt").to_string_lossy().into_owned();
    std::fs::write(dir.join("latin1.txt"), b"file \xff.mp4\n").unwrap();
    let latin1 = dir.join("latin1.txt").to_string_lossy().into_owned();
    let input = dir.join("in.mp4");
    let i = input.to_str().unwrap();
    for (args, code) in [
        (vec!["-f", "concat", "-i", &trimmed, "-c", "copy", o], 4),
        (
            vec!["-f", "concat", "-i", &plain, "-t", "1", "-c", "copy", o],
            4,
        ),
        (
            vec!["-f", "concat", "-i", &plain, "-map", "0:a", "-c", "copy", o],
            4,
        ),
        (vec!["-f", "concat", "-i", &empty, "-c", "copy", o], 1),
        (vec!["-f", "concat", "-i", &latin1, "-c", "copy", o], 1),
        (vec!["-f", "concat", "-i", &missing, "-c", "copy", o], 3),
        (
            vec!["-f", "concat", "-safe", "2", "-i", &plain, "-c", "copy", o],
            2,
        ),
        (vec!["-safe", "0", "-i", i, "-c", "copy", o], 2),
        (vec!["-y", "-f", "concat", "-i", &plain, "-c", "copy", i], 2),
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, code, "{args:?}: {}", r.stderr);
        assert!(!output.exists(), "{args:?}");
    }
}

#[test]
fn concat_nonzero_first_origin_is_refused_without_changing_native_join() {
    let dir = scratch("ffmpeg-concat-first-origin");
    let source = dir.join("late.mp4");
    let bytes = common::movie(&Spec {
        sizes: vec![100; 8],
        sync: (1..=8).collect(),
        elst: None,
        ctts: Some(vec![(8, 512)]),
        ..Spec::default()
    });
    std::fs::write(&source, bytes).unwrap();
    let list = dir.join("list.txt");
    std::fs::write(&list, "file late.mp4\nfile late.mp4\n").unwrap();
    let output = dir.join("out.mp4");
    let args = [
        "-f",
        "concat",
        "-i",
        list.to_str().unwrap(),
        "-c",
        "copy",
        output.to_str().unwrap(),
    ];
    let count = std::fs::read_dir(&dir).unwrap().count();
    let r = ffmpeg(&args);
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(r.stdout.is_empty());
    assert!(
        r.stderr.contains("first input starts at a nonzero time"),
        "{}",
        r.stderr
    );
    assert!(!output.exists());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), count);
    std::fs::write(&output, b"existing output").unwrap();
    let r = ffmpeg(&[&["-y"], args.as_slice()].concat());
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
    let mut native = Vec::new();
    let options = coreav_isobmff_write::JoinOptions {
        video: coreav_isobmff_write::VideoChoice::First,
        audio: coreav_isobmff_write::JoinAudio::None,
        placement: coreav_isobmff_write::Placement::AfterVideo,
        compatibility: None,
    };
    let summary = coreav_cli::join::run(
        &[source.clone(), source],
        &options,
        &Default::default(),
        &mut native,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.json));
    assert_eq!(
        summary.inputs[0].video_start,
        coreav_isobmff::Rational::new(1, 25).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let fallback = dir.join("ffmpeg");
        std::fs::write(&fallback, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
        std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
        let original = [&["-y"], args.as_slice()].concat();
        let r = Command::new(bin())
            .arg("ffmpeg")
            .args(&original)
            .env_clear()
            .env("PATH", &dir)
            .output()
            .unwrap();
        assert_eq!(
            r.status.code(),
            Some(19),
            "{}",
            String::from_utf8_lossy(&r.stderr)
        );
        assert_eq!(r.stdout, fallback_arguments(&original));
        assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
    }
}

#[test]
fn av1_video_concat_compares_every_exposed_sample_entry_field_and_packet_byte() {
    let Some((reference, probe)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-av1-complete");
    let policy: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let source = dir.join("source.mp4");
    let generated = Command::new(&reference)
        .args(["-v", "error", "-f", "lavfi"])
        .args(&policy)
        .args([
            "-i",
            "testsrc2=size=80x64:rate=24:duration=1",
            "-an",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-threads:v",
            "1",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let list = dir.join("list.txt");
    std::fs::write(&list, "file source.mp4\nfile source.mp4\n").unwrap();
    let candidate = dir.join("candidate.mp4");
    let expected = dir.join("reference.mp4");
    let result = ffmpeg(&[
        "-f",
        "concat",
        "-i",
        list.to_str().unwrap(),
        "-c",
        "copy",
        candidate.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let copied = Command::new(&reference)
        .args(["-v", "error", "-f", "concat"])
        .args(&policy)
        .arg("-i")
        .arg(&list)
        .args(["-c", "copy"])
        .arg(&expected)
        .output()
        .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stderr)
    );
    let capture = |path: &Path| {
        let output = Command::new(&probe)
            .args(["-v", "error"])
            .args(&policy)
            .arg("-i")
            .arg(path)
            .args([
                "-show_streams",
                "-show_packets",
                "-show_data",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut document: Value = serde_json::from_slice(&output.stdout).unwrap();
        for packet in document["packets"].as_array_mut().unwrap() {
            packet.as_object_mut().unwrap().remove("pos");
        }
        document
    };
    assert_eq!(capture(&candidate), capture(&expected));
}

#[test]
fn both_y_and_n_are_refused_and_the_output_kept() {
    let dir = scratch("ffmpeg-y-n");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    std::fs::write(&output, b"sentinel bytes").unwrap();
    for args in [
        ["-i", &input, "-n", "-y", "-c", "copy", o],
        ["-i", &input, "-y", "-n", "-c", "copy", o],
        ["-y", "-i", &input, "-n", "-c", "copy", o],
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, 2, "{args:?}: {}", r.stderr);
        assert!(
            r.stderr.contains("both -y and -n"),
            "{args:?}: {}",
            r.stderr
        );
        assert_eq!(
            std::fs::read(&output).unwrap(),
            b"sentinel bytes",
            "{args:?}"
        );
    }
}

#[test]
fn a_clock_time_out_of_range_is_a_usage_error() {
    let dir = scratch("ffmpeg-clock");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    for args in [
        vec!["-i", &input, "-t", "00:99", "-c", "copy", o],
        vec!["-i", &input, "-ss", "00:60", "-c", "copy", o],
        vec!["-i", &input, "-to", "1:60", "-c", "copy", o],
        vec!["-ss", "00:99", "-i", &input, "-c", "copy", o],
        vec!["-t", "01:60:00", "-i", &input, "-c", "copy", o],
        vec!["-to", "00:61", "-i", &input, "-c", "copy", o],
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, 2, "{args:?}: {}", r.stderr);
        assert!(
            r.stderr.contains("Invalid duration"),
            "{args:?}: {}",
            r.stderr
        );
        assert!(!output.exists(), "{args:?}");
    }
}

#[test]
fn names_with_a_protocol_are_refused_and_never_written() {
    let dir = scratch("ffmpeg-protocol");
    let input = movie(&dir);
    let subfile = "subfile,,start,0,end,99999,,:";
    let cases = [subfile, ":", "a,b:"].into_iter().flat_map(|p| {
        [
            (vec![format!("{p}in.mp4"), "out.mp4".to_owned()], "an input"),
            (
                vec!["in.mp4".to_owned(), format!("{p}out.mp4")],
                "an output",
            ),
        ]
    });
    for (names, named) in cases.chain([(vec![":x".to_owned(), "out.mp4".to_owned()], "an input")]) {
        let args = ["-i", &names[0], "-c", "copy", &names[1]];
        let out = Command::new(bin())
            .arg("ffmpeg")
            .args(args)
            .current_dir(&dir)
            .env("COREAV_STRICT_SAFE", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(4), "{args:?}: {stderr}");
        assert!(
            stderr.contains(&format!("{named} named with a protocol")),
            "{args:?}: {stderr}"
        );
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["in.mp4"], "{args:?}");
    }
    assert!(input.ends_with("in.mp4"));
}

#[test]
fn names_that_are_paths_despite_a_colon_run_natively() {
    let dir = scratch("ffmpeg-colon-paths");
    movie(&dir);
    let run = |args: &[&str]| {
        Command::new(bin())
            .arg("ffmpeg")
            .args(args)
            .current_dir(&dir)
            .env("COREAV_STRICT_SAFE", "1")
            .output()
            .unwrap()
    };
    // Windows file names cannot hold a colon.
    if !cfg!(windows) {
        std::fs::copy(dir.join("in.mp4"), dir.join("a:b.mp4")).unwrap();
        let out = run(&["-i", "./a:b.mp4", "-c", "copy", "./c:d.mp4"]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        assert!(dir.join("c:d.mp4").exists());
    }
    // A drive letter starts a path on Windows only; elsewhere `ffmpeg`
    // reads `C` as a protocol.
    let drive = if cfg!(windows) {
        dir.join("e.mp4").to_str().unwrap().replace('\\', "/")
    } else {
        "C:/e.mp4".to_owned()
    };
    let out = run(&["-i", "in.mp4", "-c", "copy", &drive]);
    let code = if cfg!(windows) { 0 } else { 4 };
    assert_eq!(out.status.code(), Some(code), "{drive}: {out:?}");
    assert_eq!(dir.join("e.mp4").exists(), cfg!(windows), "{drive}");
}

#[test]
fn the_largest_stream_ordinal_matches_nothing_without_overflow() {
    let dir = scratch("ffmpeg-ordinal");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let last = u64::MAX.to_string();
    let map = format!("0:v:{last}");
    let codec = format!("-c:v:{last}");
    // A map that matches no stream fails in ffmpeg too; a codec option
    // that covers no stream leaves the video to be encoded.
    for (args, code) in [
        (vec!["-i", &input, "-map", &map, "-c", "copy", o], 2),
        (vec!["-i", &input, &codec, "copy", o], 4),
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, code, "{args:?}: {}", r.stderr);
        assert!(!output.exists(), "{args:?}");
    }
}

#[test]
fn clip_times_count_from_the_input_start_time() {
    let dir = scratch("ffmpeg-start-time");
    // Fourteen sync samples of 40 ms, presented from 200 ms on (an empty
    // edit): `-ss 0.12` is 320 ms on the timeline, the fourth sample.
    let input = dir.join("in.mp4");
    std::fs::write(
        &input,
        common::movie(&Spec {
            sizes: vec![100; 14],
            composition: Vec::new(),
            sync: (1..=14).collect(),
            elst: Some(vec![(200, -1), (560, 0)]),
            ctts: Some(Vec::new()),
            ..Spec::default()
        }),
    )
    .unwrap();
    let output = dir.join("out.mp4");
    let args: Vec<String> = ["-ss", "0.12", "-i", input.to_str().unwrap(), "-t", "0.04"]
        .into_iter()
        .chain(["-c", "copy", output.to_str().unwrap()])
        .map(str::to_owned)
        .collect();
    let Ok(summary) = coreav_cli::ffmpeg::run(&args, StrictSafe::new(false)) else {
        panic!("{args:?} did not run");
    };
    assert_eq!(summary["start"]["requested"], json!({"num": 8, "den": 25}));
    assert_eq!(summary["end"]["requested"], json!({"num": 9, "den": 25}));
    assert_eq!(summary["streams"][0]["first_sample"], 3, "{summary}");
    assert_eq!(summary["streams"][0]["samples"], 1, "{summary}");
}

/// The file extensions of each container family the command set names.
const FAMILIES: [(&str, &[&str]); 2] = [
    ("mp4-mov", &["mp4", "mov", "m4a", "3gp", "m4v"]),
    ("matroska-webm", &["mkv", "webm"]),
];

const INPUT_SEEK_REFUSALS: [(&str, usize, &str); 17] = [
    ("C18", 2, "edit_multi_empty.mp4"),
    ("C18", 2, "edit_multi_audio.mp4"),
    ("C18", 2, "edit_multi_audio_average.mp4"),
    ("C18", 2, "edit_multi_av.mp4"),
    ("C18", 2, "chapters_sized.mp4"),
    ("C18", 2, "hls_h264.mp4"),
    ("C18", 2, "hls_open_gop.mp4"),
    ("C18", 2, "hls_hevc.mp4"),
    ("C18", 2, "hls_long_gop.mp4"),
    ("C18", 2, "fragmented_sidx.mp4"),
    ("C18", 2, "fragmented_many.mp4"),
    ("C19", 0, "edit_multi_audio.mp4"),
    ("C19", 0, "edit_multi_audio_average.mp4"),
    ("C19", 0, "edit_trim_audio.mp4"),
    ("C19", 1, "edit_multi_audio.mp4"),
    ("C19", 1, "edit_multi_audio_average.mp4"),
    ("C19", 1, "edit_trim_audio.mp4"),
];

#[test]
fn input_seek_refusal_contract_keeps_its_exact_command_scope() {
    let set: Value = serde_json::from_str(include_str!(
        "../../../tools/compat-corpus/commands-0.1.0/commands.json"
    ))
    .unwrap();
    let unique: BTreeSet<_> = INPUT_SEEK_REFUSALS.into_iter().collect();
    assert_eq!(unique.len(), 17);
    assert_eq!(unique.iter().filter(|(id, _, _)| *id == "C18").count(), 11);
    assert_eq!(unique.iter().filter(|(id, _, _)| *id == "C19").count(), 6);
    for number in [0, 1] {
        assert_eq!(
            unique
                .iter()
                .filter(|(id, command, _)| *id == "C19" && *command == number)
                .count(),
            3
        );
    }
    for (id, number, _) in unique {
        let entry = set["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap();
        let expected = match (id, number) {
            ("C18", 2) => json!([
                "-ss", "00:00:09", "-i", "INPUT", "-t", "00:00:03", "-vcodec", "copy", "-acodec",
                "copy", "-y", "OUTPUT"
            ]),
            ("C19", 0) => json!([
                "-ss", "3", "-i", "INPUT", "-t", "5", "-c:a", "copy", "OUTPUT"
            ]),
            ("C19", 1) => json!([
                "-ss", "3", "-t", "5", "-i", "INPUT", "-c:a", "copy", "OUTPUT"
            ]),
            _ => panic!("unapproved input-seek refusal command"),
        };
        assert_eq!(entry["commands"][number]["args"], expected);
    }
    for outside in [
        ("C18", 0, "hls_h264.mp4"),
        ("C18", 1, "hls_h264.mp4"),
        ("C18", 2, "basic.mp4"),
        ("C19", 2, "edit_multi_audio.mp4"),
        ("C19", 0, "basic.mp4"),
        ("C05", 2, "hls_h264.mp4"),
    ] {
        assert!(!INPUT_SEEK_REFUSALS.contains(&outside));
    }
}

fn verify_input_seek_refusal(args: &[String], output: &Path) {
    let pin = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH")
                .iter()
                .flat_map(std::env::split_paths)
                .map(|directory| directory.join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX)))
                .find(|path| coreav_cli::fallback::executable(path))
        })
        .expect("stage=oracle_selection: no pinned FFmpeg executable");
    let pin = std::path::absolute(pin).unwrap();
    assert!(
        coreav_cli::fallback::executable(&pin),
        "stage=oracle_selection: pinned FFmpeg is not executable"
    );
    let reference = output.with_file_name("reference.mp4");
    let directory = output.parent().unwrap();
    assert!(!output.exists(), "refusal left output before verification");
    for existing in [false, true] {
        if existing {
            std::fs::write(output, b"existing destination").unwrap();
        }
        let entries: BTreeSet<_> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let refused = Command::new(bin())
            .args(["--strict-safe", "ffmpeg", "-y"])
            .args(args)
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(4), "{args:?}: {refused:?}");
        assert!(refused.stdout.is_empty(), "{args:?}");
        assert_eq!(
            std::fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<BTreeSet<_>>(),
            entries,
            "{args:?}"
        );
        if existing {
            assert_eq!(std::fs::read(output).unwrap(), b"existing destination");
        } else {
            assert!(!output.exists(), "{args:?}");
        }
        let expected = Command::new(&pin)
            .args(["-nostdin", "-hide_banner", "-y"])
            .args(args)
            .output()
            .unwrap();
        assert!(expected.status.success(), "{args:?}: {expected:?}");
        std::fs::rename(output, &reference).unwrap();
        if existing {
            std::fs::write(output, b"existing destination").unwrap();
        }
        let fallback = Command::new(bin())
            .args(["ffmpeg", "-y"])
            .args(args)
            .env("COREAV_FALLBACK", &pin)
            .env_remove("COREAV_STRICT_SAFE")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert_eq!(
            fallback.status.code(),
            expected.status.code(),
            "stage=real_fallback_exit {args:?}: {}",
            String::from_utf8_lossy(&fallback.stderr)
        );
        assert_eq!(fallback.stdout, expected.stdout, "{args:?}");
        assert_eq!(
            std::fs::read(output).unwrap(),
            std::fs::read(&reference).unwrap(),
            "{args:?}"
        );
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(&reference).unwrap();
    }
}

/// A concat list in `dir` that names a copy of `source` twice, by its
/// name alone, as the demuxer's `safe` option takes it.
fn concat_list(dir: &Path, source: &Path) -> PathBuf {
    let name = source.file_name().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let copy = dir.join(name);
    if std::fs::hard_link(source, &copy).is_err() {
        std::fs::copy(source, &copy).unwrap();
    }
    let list = dir.join("list.txt");
    let line = format!("file '{}'\n", name.to_str().unwrap());
    std::fs::write(&list, line.repeat(2)).unwrap();
    list
}

fn stream_kind(letter: &str) -> StreamKind {
    match letter {
        "v" => StreamKind::Video,
        "a" => StreamKind::Audio,
        "s" => StreamKind::Subtitle,
        "d" => StreamKind::Data,
        "t" => StreamKind::Attachment,
        other => panic!("input_without names {other}"),
    }
}

/// The channels of each stream of `path`, `None` for a stream without.
fn stream_channels(path: &Path) -> Vec<Option<i64>> {
    let size = std::fs::metadata(path).unwrap().len();
    let mut file = coreav_isobmff::FileSource::open(path).unwrap();
    let name = path.display().to_string();
    let Ok(probed) = coreav_cli::ffprobe::probe(&mut file, size, &name, None) else {
        panic!("{name} is rejected");
    };
    let channels = |s: &coreav_cli::ffprobe::Stream| {
        s.entries.iter().find_map(|(k, v)| match v {
            coreav_cli::ffprobe::model::Value::Int(n) if *k == "channels" => Some(*n),
            _ => None,
        })
    };
    probed.report.streams.iter().map(channels).collect()
}

/// The kinds of the streams of `path`, or `None` when the probe rejects it.
fn stream_kinds(path: &Path) -> Option<Vec<StreamKind>> {
    let size = std::fs::metadata(path).ok()?.len();
    let mut file = coreav_isobmff::FileSource::open(path).ok()?;
    let name = path.display().to_string();
    let probed = coreav_cli::ffprobe::probe(&mut file, size, &name, None).ok()?;
    Some(probed.report.streams.iter().map(|s| s.kind).collect())
}

/// Every `ffmpeg` command of `commands-0.1.0` on every fixture of its
/// container families that has none of the stream types its
/// `input_without` names (R3's rule), run as the binary runs it, writing
/// the output to `out.mp4`, `out.m4a` for an audio extraction or
/// `out%03d.mp4` for segments; a join's input is a concat list that names
/// the fixture twice ([`concat_list`]). A command a file cannot take (a `-map` that
/// matches no stream of it, or a selection left empty, which `ffmpeg` fails
/// too) is recorded as not taken. Every entry runs
/// natively on at least one fixture of each of its families; a refusal or a
/// rejection leaves no file behind, and the fixtures whose video is not
/// valid HEVC give no output. With `COREAV_FFMPEG_OUT` the outputs and
/// `records.json` (each run, its outcome and its writer's summary) are
/// written there for `tools/compat-corpus/copy_check.py`.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn the_0_1_0_commands_on_the_fixtures() {
    let fixtures = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let out = std::env::var("COREAV_FFMPEG_OUT")
        .map_or_else(|_| scratch("ffmpeg-fixtures"), |d| fresh(Path::new(&d)));
    let set: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tools/compat-corpus/commands-0.1.0/commands.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let names: Vec<String> = std::fs::read_to_string(fixtures.join("MANIFEST"))
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .map(str::to_owned)
        .collect();
    let family = |name: &str| {
        let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
        FAMILIES
            .iter()
            .find(|(_, extensions)| extensions.contains(&extension.as_str()))
            .map(|(family, _)| *family)
    };
    let present: BTreeSet<&str> = names.iter().filter_map(|n| family(n)).collect();
    let mut records = Vec::new();
    let mut native = BTreeSet::new();
    let mut required_refusals = BTreeSet::new();
    for entry in set["entries"].as_array().unwrap() {
        if entry["tool"] != "ffmpeg" {
            continue;
        }
        let id = entry["id"].as_str().unwrap();
        let job = entry["job"].as_str().unwrap();
        let families: Vec<&str> = entry["containers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        for (n, command) in entry["commands"].as_array().unwrap().iter().enumerate() {
            let template: Vec<&str> = command["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap())
                .collect();
            let without: Vec<StreamKind> = command["input_without"]
                .as_array()
                .map_or_else(Vec::new, |w| {
                    w.iter().map(|k| stream_kind(k.as_str().unwrap())).collect()
                });
            for name in &names {
                let Some(family) = family(name).filter(|f| families.contains(f)) else {
                    continue;
                };
                let source = fixtures.join(name);
                if stream_kinds(&source)
                    .is_some_and(|kinds| kinds.iter().any(|k| without.contains(k)))
                {
                    continue;
                }
                let work = out.join(format!("{id}.{n}.{name}"));
                std::fs::create_dir_all(&work).unwrap();
                let input = if job == "join" {
                    concat_list(&out.join(format!("{id}.{n}.{name}.list")), &source)
                } else {
                    source.clone()
                };
                let output = work.join(match job {
                    "extract-audio" => "out.m4a",
                    "split" => "out%03d.mp4",
                    _ => "out.mp4",
                });
                let args: Vec<String> = template
                    .iter()
                    .map(|a| match *a {
                        "INPUT" => input.display().to_string(),
                        "OUTPUT" => output.display().to_string(),
                        other => other.to_owned(),
                    })
                    .collect();
                let label = format!("{id} {} on {name}", template.join(" "));
                let required =
                    family == "mp4-mov" && INPUT_SEEK_REFUSALS.contains(&(id, n, name.as_str()));
                let (status, detail) = match coreav_cli::ffmpeg::run(&args, StrictSafe::new(false))
                {
                    Ok(summary) => {
                        assert!(!required, "{label}: required refusal ran natively");
                        native.insert((id, family));
                        ("written", summary)
                    }
                    Err(Stop::Refused(Refusal::NotNative(refusal))) => {
                        ("refused", json!(refusal.message))
                    }
                    Err(Stop::Refused(Refusal::Usage(text))) => {
                        assert!(
                            text.starts_with("A -map matches no streams")
                                || text == "Output file does not contain any stream",
                            "{label}: {text}"
                        );
                        ("not taken", json!(text))
                    }
                    Err(Stop::Refused(Refusal::Help)) => panic!("{label}: read as a help query"),
                    Err(Stop::Failed(failure)) => {
                        assert!(!failure.is_io(), "{label}: {}", failure.to_json());
                        ("rejected", failure.to_json())
                    }
                };
                if required {
                    assert_eq!(status, "refused", "{label}: {detail}");
                    verify_input_seek_refusal(&args, &output);
                    required_refusals.insert((id, n, name.as_str()));
                }
                // Not valid HEVC (ITU-T H.265 7.4.3.2.1): no command writes
                // an output from these, a copy included.
                if ["size_hevc_mixed_window.mp4", "size_hevc_window.mp4"].contains(&name.as_str()) {
                    assert_ne!(status, "written", "{label}");
                }
                let files = std::fs::read_dir(&work).unwrap().count();
                if status == "written" {
                    assert!(files > 0, "{label}: no output");
                } else {
                    assert_eq!(files, 0, "{label}: {status}, but a file was left");
                }
                eprintln!("{label}: {status}");
                records.push(json!({
                    "id": id,
                    "job": job,
                    "command": n,
                    "args": template,
                    "source": source.display().to_string(),
                    "input": input.display().to_string(),
                    "family": family,
                    "output": output.display().to_string(),
                    "status": status,
                    "detail": detail,
                }));
            }
        }
        for family in families.iter().filter(|f| present.contains(*f)) {
            let required: Vec<_> = INPUT_SEEK_REFUSALS
                .iter()
                .filter(|(entry, _, _)| *entry == id && *family == "mp4-mov")
                .collect();
            if required.is_empty() {
                assert!(
                    native.contains(&(id, *family)),
                    "{id}: no {family} fixture ran natively"
                );
            } else {
                for witness in required {
                    assert!(
                        required_refusals.contains(witness),
                        "{witness:?}: required refusal was not verified"
                    );
                }
            }
        }
    }
    std::fs::write(
        out.join("records.json"),
        serde_json::to_string_pretty(&Value::Array(records)).unwrap(),
    )
    .unwrap();
}

/// The job a writer's summary says CoreAV chose, by its schema.
fn chosen(summary: &Value) -> Option<&'static str> {
    match summary["schema"].as_str()? {
        "coreav.remux/1" => Some("remux"),
        "coreav.clip/1" => Some("clip"),
        "coreav.split/1" => Some("split"),
        "coreav.join/1" => Some("join"),
        "coreav.extract-audio/1" => Some("extract-audio"),
        "coreav.transcode/1" => Some("transcode"),
        _ => None,
    }
}

/// The R3 runner's half in Rust (`tools/compat-corpus/r3.py`): each pair
/// of `COREAV_R3_PAIRS` (a JSON array whose objects carry `args`, the
/// `coreav ffmpeg` arguments with every placeholder filled, and `work`,
/// the directory that holds the pair's output and nothing else) is run
/// through [`coreav_cli::ffmpeg::run`], the function `coreav ffmpeg` runs,
/// in strict-safe mode. The object is written back to `COREAV_R3_RECORDS`
/// with its `status` (`written`, `refused`, `not taken`, `usage`,
/// `rejected` or `io`), its `detail` (the writer's summary, which the
/// binary does not print, or the reason) and the number of files left in
/// `work`, which `tools/compat-corpus/copy_check.py` reads. A written pair
/// also gets `chosen`, the job CoreAV chose (`remux`, `clip`, `split`,
/// `join`, `extract-audio` or `transcode`), read from the schema of the
/// summary the writer returned and never from the command, so a pair needs
/// no `job` (`tools/compat-corpus/encode_breadth.py` gives none). Unlike
/// [`the_0_1_0_commands_on_the_fixtures`], an outcome is recorded, never
/// asserted: the files are not the fixtures, and the runner judges.
#[test]
#[ignore = "run by tools/compat-corpus/r3.py; set COREAV_R3_PAIRS and COREAV_R3_RECORDS"]
fn the_0_1_0_commands_on_listed_pairs() {
    let pairs: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(std::env::var("COREAV_R3_PAIRS").expect("set COREAV_R3_PAIRS"))
            .unwrap(),
    )
    .unwrap();
    let mut records = Vec::with_capacity(pairs.len());
    for mut pair in pairs {
        let args: Vec<String> = pair["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect();
        let work = PathBuf::from(pair["work"].as_str().unwrap());
        std::fs::create_dir_all(&work).unwrap();
        let (status, detail) = match coreav_cli::ffmpeg::run(&args, StrictSafe::new(true)) {
            Ok(summary) => ("written", summary),
            Err(Stop::Refused(Refusal::NotNative(refusal))) => ("refused", json!(refusal.message)),
            Err(Stop::Refused(Refusal::Usage(text))) => {
                let taken = text.starts_with("A -map matches no streams")
                    || text == "Output file does not contain any stream";
                (if taken { "not taken" } else { "usage" }, json!(text))
            }
            Err(Stop::Refused(Refusal::Help)) => panic!("{args:?}: read as a help query"),
            Err(Stop::Failed(failure)) => (
                if failure.is_io() { "io" } else { "rejected" },
                failure.to_json(),
            ),
        };
        pair["chosen"] = json!((status == "written").then(|| chosen(&detail)).flatten());
        pair["status"] = json!(status);
        pair["detail"] = detail;
        pair["files"] = json!(std::fs::read_dir(&work).unwrap().count());
        records.push(pair);
    }
    std::fs::write(
        std::env::var("COREAV_R3_RECORDS").expect("set COREAV_R3_RECORDS"),
        serde_json::to_string(&Value::Array(records)).unwrap(),
    )
    .unwrap();
}

/// A JPEG picture of `width` by `height` as far as a listing reads it: the
/// start of image, a baseline frame header of three components at 4:2:0
/// (ITU-T T.81 B.2.2) and the end of image.
fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let ([h0, h1], [w0, w1]) = (height.to_be_bytes(), width.to_be_bytes());
    vec![
        0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8, h0, h1, w0, w1, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1,
        0xFF, 0xD9,
    ]
}

/// `movie` (as [`av_movie_with`] writes it, the movie box last) with its
/// tracks' language `eng` and, in the movie's item list
/// (`moov/udta/meta/ilst`), one `covr` item per entry of `items` holding its
/// pictures as JPEG `data` boxes (type 13), written to `dir/name`.
fn covered(dir: &Path, name: &str, mut movie: Vec<u8>, items: &[&[Vec<u8>]]) -> String {
    let at = |bytes: &[u8], kind: &[u8]| -> Vec<usize> {
        (4..bytes.len())
            .filter(|&i| bytes[i..].starts_with(kind))
            .map(|i| i - 4)
            .collect()
    };
    // A version 0 `mdhd`: header, version and flags, four 32-bit fields,
    // then the packed language.
    for mdhd in at(&movie, b"mdhd") {
        movie[mdhd + 28..mdhd + 30].copy_from_slice(&[0x15, 0xc7]);
    }
    let items: Vec<u8> = items
        .iter()
        .flat_map(|pictures| {
            let data: Vec<u8> = pictures
                .iter()
                .flat_map(|p| bx(b"data", &cat(&[u32s(&[13, 0]), p.clone()])))
                .collect();
            bx(b"covr", &data)
        })
        .collect();
    let hdlr = full(b"hdlr", 0, 0, b"\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0");
    let moov = at(&movie, b"moov")[0];
    movie.extend_from_slice(&bx(
        b"udta",
        &full(b"meta", 0, 0, &cat(&[hdlr, bx(b"ilst", &items)])),
    ));
    let size = u32::try_from(movie.len() - moov).unwrap();
    movie[moov..moov + 4].copy_from_slice(&size.to_be_bytes());
    let path = dir.join(name);
    std::fs::write(&path, movie).unwrap();
    path.to_string_lossy().into_owned()
}

/// Each stream's kind and whether it is an attached picture, and the
/// movie's cover pictures as stored.
type Listing = (Vec<(StreamKind, bool)>, Vec<Vec<u8>>);

/// What `coreav ffprobe` lists of `path`.
fn listed(path: &Path) -> Listing {
    let size = std::fs::metadata(path).unwrap().len();
    let mut file = coreav_isobmff::FileSource::open(path).unwrap();
    let name = path.display().to_string();
    let Ok(probed) = coreav_cli::ffprobe::probe(&mut file, size, &name, None) else {
        panic!("{name} is rejected");
    };
    let streams = probed
        .report
        .streams
        .iter()
        .map(|s| (s.kind, s.attached_pic))
        .collect();
    let meta = coreav_isobmff::metadata::read(
        coreav_isobmff::FileSource::open(path).unwrap(),
        &coreav_isobmff::Limits::default(),
    )
    .unwrap();
    let covers = meta
        .items
        .iter()
        .filter_map(|item| match &item.value {
            coreav_isobmff::metadata::ItemValue::ListItem {
                key: coreav_isobmff::metadata::ListKey::Atom(kind),
                data,
                ..
            } if kind.0 == *b"covr" => Some(data.iter().map(|d| d.value.clone())),
            _ => None,
        })
        .flatten()
        .collect();
    (streams, covers)
}

const V: (StreamKind, bool) = (StreamKind::Video, false);
const A: (StreamKind, bool) = (StreamKind::Audio, false);
const PICTURE: (StreamKind, bool) = (StreamKind::Video, true);

/// A cover picture is a stream after the tracks (`ffprobe` lists it so):
/// a selection that names it keeps it as a cover picture, every other
/// selection leaves it out, as the pinned `ffmpeg` does (black box). A
/// file without one copies as before.
#[test]
fn a_cover_picture_is_copied_only_when_selected() {
    let dir = scratch("ffmpeg-cover");
    let picture = jpeg(640, 480);
    let input = covered(
        &dir,
        "in.mp4",
        av_movie_with(aac_entry(), false),
        &[std::slice::from_ref(&picture)],
    );
    assert_eq!(
        listed(Path::new(&input)),
        (vec![V, A, PICTURE], vec![picture.clone()])
    );
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let cases: [(&[&str], Listing); 5] = [
        (&["-map", "0"], (vec![V, A, PICTURE], vec![picture.clone()])),
        (
            &["-map", "0", "-map", "-0:s", "-map", "-0:d"],
            (vec![V, A, PICTURE], vec![picture]),
        ),
        (&[], (vec![V, A], vec![])),
        (&["-map", "0:0", "-map", "0:1"], (vec![V, A], vec![])),
        (&["-map", "0", "-map", "-0:2"], (vec![V, A], vec![])),
    ];
    for (maps, expected) in cases {
        let mut args = vec!["-y", "-i", &input, "-c", "copy"];
        args.extend_from_slice(maps);
        args.push(o);
        let r = ffmpeg(&args);
        assert_eq!(r.code, 0, "{maps:?}: {}", r.stderr);
        assert_eq!(listed(&output), expected, "{maps:?}");
    }
    let plain = movie(&dir);
    let r = ffmpeg(&["-y", "-i", &plain, "-map", "0", "-c", "copy", o]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(listed(&output), (vec![V, A], vec![]));
}

/// A `meta` box needs a handler box (ISO/IEC 14496-12 8.11.1): without one
/// its item list's cover picture is no stream, as the pinned `ffprobe`
/// lists it (black box); a handler of any type keeps it.
#[test]
fn a_cover_picture_needs_the_meta_handler() {
    let dir = scratch("ffmpeg-cover-handler");
    let picture = jpeg(640, 480);
    let input = covered(
        &dir,
        "in.mp4",
        av_movie_with(aac_entry(), false),
        &[std::slice::from_ref(&picture)],
    );
    let bytes = std::fs::read(&input).unwrap();
    let hdlr = bytes.windows(4).rposition(|w| w == b"hdlr").unwrap();
    for (name, at, kind, expected) in [
        ("unknown.mp4", hdlr + 12, b"zzzz", vec![V, A, PICTURE]),
        ("none.mp4", hdlr, b"free", vec![V, A]),
    ] {
        let mut changed = bytes.clone();
        changed[at..at + 4].copy_from_slice(kind);
        let path = dir.join(name);
        std::fs::write(&path, changed).unwrap();
        assert_eq!(listed(&path), (expected, vec![picture.clone()]), "{name}");
    }
}

/// Of two cover pictures, the one selected stays.
#[test]
fn of_two_cover_pictures_the_selected_one_stays() {
    let dir = scratch("ffmpeg-two-covers");
    let (first, second) = (jpeg(32, 32), jpeg(48, 48));
    let pictures = [first.clone(), second.clone()];
    let input = covered(
        &dir,
        "in.mp4",
        av_movie_with(aac_entry(), false),
        &[&pictures[..1], &pictures[1..]],
    );
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    for (left_out, kept) in [("-0:2", second), ("-0:3", first)] {
        let r = ffmpeg(&[
            "-y", "-i", &input, "-map", "0", "-map", left_out, "-c", "copy", o,
        ]);
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert_eq!(listed(&output), (vec![V, A, PICTURE], vec![kept]));
    }
}

/// One `covr` item holding several pictures, one `data` box each (as `ffmpeg`
/// writes them), is one stream per picture, in order, as `ffprobe` lists
/// them; a selection keeps or drops each picture alone, and the kept ones
/// stay in one item, in their order (the pinned `ffmpeg`, black box).
#[test]
fn each_picture_of_one_cover_item_is_selected_alone() {
    let dir = scratch("ffmpeg-cover-item");
    let pictures = [jpeg(32, 32), jpeg(64, 64), jpeg(48, 48)];
    let input = covered(
        &dir,
        "in.mp4",
        av_movie_with(aac_entry(), false),
        &[&pictures],
    );
    let [first, second, third] = &pictures;
    let listing = |kept: &[&Vec<u8>]| -> Listing {
        let mut streams = vec![V, A];
        streams.extend(kept.iter().map(|_| PICTURE));
        (streams, kept.iter().map(|p| (*p).clone()).collect())
    };
    assert_eq!(listed(Path::new(&input)), listing(&[first, second, third]));
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let items = |path: &Path| {
        let meta = coreav_isobmff::metadata::read(
            coreav_isobmff::FileSource::open(path).unwrap(),
            &coreav_isobmff::Limits::default(),
        )
        .unwrap();
        meta.items
            .iter()
            .filter(|item| {
                matches!(
                    &item.value,
                    coreav_isobmff::metadata::ItemValue::ListItem {
                        key: coreav_isobmff::metadata::ListKey::Atom(kind),
                        ..
                    } if kind.0 == *b"covr"
                )
            })
            .count()
    };
    let cases: [(&[&str], Vec<&Vec<u8>>); 6] = [
        (&["-map", "0"], vec![first, second, third]),
        (&[], vec![]),
        (
            &["-map", "0:v:0", "-map", "0:a", "-map", "0:v:2"],
            vec![second],
        ),
        (&["-map", "0", "-map", "-0:v:1"], vec![second, third]),
        (&["-map", "0", "-map", "-0:v:2"], vec![first, third]),
        (&["-map", "0:v:0", "-map", "0:a"], vec![]),
    ];
    for (maps, kept) in cases {
        let mut args = vec!["-y", "-i", &input, "-c", "copy"];
        args.extend_from_slice(maps);
        args.push(o);
        let r = ffmpeg(&args);
        assert_eq!(r.code, 0, "{maps:?}: {}", r.stderr);
        assert_eq!(listed(&output), listing(&kept), "{maps:?}");
        assert_eq!(items(&output), usize::from(!kept.is_empty()), "{maps:?}");
    }
    std::fs::remove_file(&output).unwrap();
    let r = ffmpeg(&["-i", &input, "-c", "copy", "-map", "0:v:2", o]);
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(r.stderr.contains("cover pictures alone"), "{}", r.stderr);
    assert!(!output.exists());
}

/// In a compressed movie box (QuickTime `cmov`), a picture's offset is in
/// the inflated box, not in the file, so its packet is listed without a
/// position; its size stays.
#[test]
fn pictures_of_a_compressed_movie_have_no_packet_position() {
    let dir = scratch("ffprobe-compressed-covers");
    let plain = covered(
        &dir,
        "plain.mp4",
        av_movie_with(aac_entry(), false),
        &[&[jpeg(32, 32), jpeg(64, 64), jpeg(48, 48)]],
    );
    let file = std::fs::read(&plain).unwrap();
    let at = file.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let moov = &file[at..];
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(moov, 6);
    let cmvd = bx(
        b"cmvd",
        &cat(&[u32s(&[u32::try_from(moov.len()).unwrap()]), packed]),
    );
    let cmov = bx(b"cmov", &cat(&[bx(b"dcom", b"zlib"), cmvd]));
    let compressed = dir.join("compressed.mov");
    std::fs::write(&compressed, cat(&[file[..at].to_vec(), bx(b"moov", &cmov)])).unwrap();
    let pictures = |path: &Path| -> Vec<Value> {
        let out = Command::new(bin())
            .args([
                "ffprobe",
                "-v",
                "error",
                "-show_entries",
                "packet=stream_index,pos,size",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
        listed["packets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["stream_index"].as_u64() >= Some(2))
            .cloned()
            .collect()
    };
    let placed = pictures(Path::new(&plain));
    assert_eq!(placed.len(), 3);
    assert!(placed.iter().all(|p| p.get("pos").is_some()), "{placed:?}");
    let unplaced: Vec<Value> = placed
        .iter()
        .map(|p| json!({ "stream_index": p["stream_index"], "size": p["size"] }))
        .collect();
    assert_eq!(pictures(&compressed), unplaced);
}

/// A selection of cover pictures alone is refused by name: `ffmpeg`
/// writes a movie without a track for it (black box), which ISO/IEC
/// 14496-12 8.3.1 does not allow.
#[test]
fn cover_pictures_alone_are_refused_by_name() {
    let dir = scratch("ffmpeg-cover-alone");
    let input = covered(
        &dir,
        "in.mp4",
        av_movie_with(aac_entry(), false),
        &[&[jpeg(32, 32)]],
    );
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let selections: [&[&str]; 3] = [
        &["-map", "0", "-map", "-0:m:language:eng"],
        &["-map", "0:2"],
        &["-map", "0:v:1"],
    ];
    for maps in selections {
        let mut args = vec!["-i", &input, "-c", "copy"];
        args.extend_from_slice(maps);
        args.push(o);
        let r = ffmpeg(&args);
        assert_eq!(r.code, 4, "{maps:?}: {}", r.stderr);
        assert!(r.stderr.contains("cover pictures alone"), "{}", r.stderr);
        assert!(!output.exists());
    }
}

/// An encode follows the cover picture selection of a copy: a selection
/// that copies the cover picture keeps it, every other leaves it out, and
/// one that would encode it with libx264 is refused (the pinned `ffmpeg`
/// fails on it, black box).
#[test]
fn an_encode_keeps_a_cover_picture_only_when_it_is_copied() {
    let dir = scratch("ffmpeg-encode-cover");
    let input = covered(
        &dir,
        "in.mp4",
        av_movie_with(aac_entry(), false),
        &[&[jpeg(32, 32)]],
    );
    let size = std::fs::metadata(&input).unwrap().len();
    let mut file = coreav_isobmff::FileSource::open(&input).unwrap();
    let Ok(probed) = coreav_cli::ffprobe::probe(&mut file, size, &input, None) else {
        panic!("{input} is rejected");
    };
    let report = probed.report;
    let [(2, at)] = report.covers[..] else {
        panic!("{:?}", report.covers);
    };
    let plan = |words: &[&str]| {
        let mut args = vec!["-i", input.as_str()];
        args.extend_from_slice(words);
        args.push("out.mp4");
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        let command = coreav_cli::ffmpeg::args::parse(&args).unwrap();
        coreav_cli::ffmpeg::plan(&command, &report, None)
    };
    let cases: [(&[&str], Vec<u64>); 3] = [
        (&["-c:v", "libx264", "-c:a", "copy"], vec![at]),
        (
            &[
                "-map", "0:v:0", "-map", "0:a", "-c:v", "libx264", "-c:a", "copy",
            ],
            vec![at],
        ),
        (
            &[
                "-map", "0", "-c:v", "libx264", "-c:v:1", "copy", "-c:a", "copy",
            ],
            vec![],
        ),
    ];
    for (words, left_out) in cases {
        match plan(words) {
            Ok(Job::Transcode(job)) => {
                assert_eq!((job.video, job.audio), (0, vec![1]), "{words:?}");
                assert_eq!(job.left_out_covers, left_out, "{words:?}");
            }
            other => panic!("{words:?}: {other:?}"),
        }
    }
    let encoded = plan(&["-map", "0", "-c:v", "libx264", "-c:a", "copy"]);
    assert!(
        matches!(&encoded, Err(Refusal::NotNative(n)) if n.message.contains("encoding a cover picture")),
        "{encoded:?}"
    );
}

/// Without `-map`, a video track wins over a cover picture, a larger one
/// too, when neither has the default disposition (the pinned `ffmpeg`,
/// black box).
#[test]
fn the_automatic_selection_takes_video_before_a_cover_picture() {
    let dir = scratch("ffmpeg-cover-not-default");
    let tracks =
        retracked(|video, audio| vec![track(video, 1, false, b"avc1", b"avc1"), audio.to_vec()]);
    let input = covered(&dir, "in.mp4", tracks, &[&[jpeg(640, 480)]]);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let r = ffmpeg(&["-i", &input, "-c", "copy", o]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(listed(&output), (vec![V, A], vec![]));
}

/// The pinned `ffmpeg` and its `ffprobe`: `None` (a skip) when there are
/// none, a failure in CI or with `COREAV_REQUIRE_FFMPEG` set.
fn oracle() -> Option<(PathBuf, PathBuf)> {
    let ffmpeg = std::env::var_os("COREAV_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/coreav-oracle-bin/ffmpeg"));
    let ffprobe = ffmpeg.with_file_name("ffprobe");
    if ffmpeg.is_file() && ffprobe.is_file() {
        return Some((ffmpeg, ffprobe));
    }
    let required =
        std::env::var_os("CI").is_some() || std::env::var_os("COREAV_REQUIRE_FFMPEG").is_some();
    assert!(
        !required,
        "no pinned ffmpeg and ffprobe (set COREAV_FFMPEG)"
    );
    eprintln!("skipping: no pinned ffmpeg and ffprobe (set COREAV_FFMPEG)");
    None
}

#[cfg(unix)]
#[test]
fn raw_s16_fifo_refuses_and_preserves_exact_pinned_fallback() {
    use std::io::Read;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
    let Some((pin, _)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("raw-s16-fifo");
    let source = dir.join("source.s16");
    let bytes = [0u8, 128, 0, 0, 255, 127];
    std::fs::write(&source, bytes).unwrap();
    let mut payloads = Vec::new();
    for fallback in [false, true] {
        let fifo = dir.join(if fallback { "fallback.s16" } else { "pin.s16" });
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let args = [
            "-y",
            "-f",
            "s16le",
            "-ar",
            "48000",
            "-ac",
            "1",
            "-i",
            source.to_str().unwrap(),
            "-f",
            "s16le",
            fifo.to_str().unwrap(),
        ];
        let inode = std::fs::metadata(&fifo).unwrap().ino();
        let entries = std::fs::read_dir(&dir).unwrap().count();
        let refused = Command::new(bin())
            .args(["--strict-safe", "ffmpeg"])
            .args(args)
            .env("COREAV_FALLBACK", &pin)
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(4), "{refused:?}");
        assert!(refused.stdout.is_empty());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("non-regular destination"));
        assert_eq!(std::fs::metadata(&fifo).unwrap().ino(), inode);
        assert!(std::fs::metadata(&fifo).unwrap().file_type().is_fifo());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
        let mut reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().try_into().unwrap())
            .open(&fifo)
            .unwrap();
        let mut command = if fallback {
            let mut command = Command::new(bin());
            command.arg("ffmpeg");
            command.env("COREAV_FALLBACK", &pin);
            command.env_remove("COREAV_STRICT_SAFE");
            command.env_remove("COREAV_FALLBACK_ACTIVE");
            command
        } else {
            Command::new(&pin)
        };
        let result = command.args(args).output().unwrap();
        assert!(result.status.success(), "{result:?}");
        assert!(result.stdout.is_empty());
        let mut payload = Vec::new();
        reader.read_to_end(&mut payload).unwrap();
        assert_eq!(payload, bytes);
        assert!(std::fs::metadata(&fifo).unwrap().file_type().is_fifo());
        assert_eq!(std::fs::metadata(&fifo).unwrap().ino(), inode);
        payloads.push(payload);
    }
    assert_eq!(payloads[0], payloads[1]);
}

#[cfg(unix)]
#[test]
fn raw_s16_linked_outputs_refuse_and_fall_back_through_the_target() {
    use std::os::unix::fs::{MetadataExt, symlink};
    let Some((pin, _)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("raw-s16-links");
    let source = dir.join("source.s16");
    let bytes = [0u8, 128, 0, 0, 255, 127];
    std::fs::write(&source, bytes).unwrap();
    for (kind, flag) in [
        ("symlink", "-y"),
        ("dangling", "-y"),
        ("dangling", "-n"),
        ("hardlink", "-y"),
    ] {
        for fallback in [false, true] {
            let tag = format!("{kind}-{flag}-{fallback}");
            let target = dir.join(format!("{tag}-target.s16"));
            let destination = dir.join(format!("{tag}.s16"));
            if kind != "dangling" {
                std::fs::write(&target, b"preserve target").unwrap();
            }
            if kind == "hardlink" {
                std::fs::hard_link(&target, &destination).unwrap();
            } else {
                symlink(&target, &destination).unwrap();
            }
            let args = [
                flag,
                "-f",
                "s16le",
                "-ar",
                "48000",
                "-ac",
                "1",
                "-i",
                source.to_str().unwrap(),
                "-f",
                "s16le",
                destination.to_str().unwrap(),
            ];
            let inode = std::fs::symlink_metadata(&destination).unwrap().ino();
            let entries = std::fs::read_dir(&dir).unwrap().count();
            let refused = Command::new(bin())
                .args(["--strict-safe", "ffmpeg"])
                .args(args)
                .output()
                .unwrap();
            assert_eq!(refused.status.code(), Some(4), "{refused:?}");
            assert!(refused.stdout.is_empty());
            assert_eq!(
                std::fs::symlink_metadata(&destination).unwrap().ino(),
                inode
            );
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
            if kind == "dangling" {
                assert!(!target.exists());
            } else {
                assert_eq!(std::fs::read(&target).unwrap(), b"preserve target");
            }
            let mut command = if fallback {
                let mut command = Command::new(bin());
                command.arg("ffmpeg");
                command.env("COREAV_FALLBACK", &pin);
                command.env_remove("COREAV_STRICT_SAFE");
                command.env_remove("COREAV_FALLBACK_ACTIVE");
                command
            } else {
                Command::new(&pin)
            };
            let result = command.args(args).output().unwrap();
            assert!(result.status.success(), "{result:?}");
            assert!(result.stdout.is_empty());
            assert_eq!(
                std::fs::symlink_metadata(&destination).unwrap().ino(),
                inode
            );
            assert_eq!(destination.is_symlink(), kind != "hardlink");
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);
            assert_eq!(std::fs::read(target).unwrap(), bytes);
        }
    }
}

#[cfg(unix)]
#[test]
fn raw_s16_unwritable_targets_preserve_the_pinned_exit_status() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let Some((pin, _)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("raw-s16-unwritable");
    let source = dir.join("source.s16");
    let bytes = [0u8, 128, 0, 0, 255, 127];
    std::fs::write(&source, bytes).unwrap();
    for dangling in [false, true] {
        for flag in ["-y", "-n"] {
            let mut outcomes = Vec::new();
            for fallback in [false, true] {
                let tag = format!("{dangling}-{flag}-{fallback}");
                let target = if dangling {
                    dir.join(format!("{tag}-missing/target.s16"))
                } else {
                    dir.join(format!("{tag}-target.s16"))
                };
                let destination = if dangling {
                    let link = dir.join(format!("{tag}-link.s16"));
                    symlink(&target, &link).unwrap();
                    link
                } else {
                    std::fs::write(&target, b"preserve").unwrap();
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o444))
                        .unwrap();
                    target.clone()
                };
                let args = [
                    flag,
                    "-f",
                    "s16le",
                    "-ar",
                    "48000",
                    "-ac",
                    "1",
                    "-i",
                    source.to_str().unwrap(),
                    "-f",
                    "s16le",
                    destination.to_str().unwrap(),
                ];
                let inode = std::fs::symlink_metadata(&destination).unwrap().ino();
                let entries = std::fs::read_dir(&dir).unwrap().count();
                let strict = Command::new(bin())
                    .args(["--strict-safe", "ffmpeg"])
                    .args(args)
                    .output()
                    .unwrap();
                assert_eq!(strict.status.code(), Some(4), "{strict:?}");
                assert!(strict.stdout.is_empty());
                assert_eq!(
                    std::fs::symlink_metadata(&destination).unwrap().ino(),
                    inode
                );
                assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
                if !dangling {
                    assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
                }
                let mut command = if fallback {
                    let mut command = Command::new(bin());
                    command.arg("ffmpeg");
                    command.env("COREAV_FALLBACK", &pin);
                    command.env_remove("COREAV_STRICT_SAFE");
                    command.env_remove("COREAV_FALLBACK_ACTIVE");
                    command
                } else {
                    Command::new(&pin)
                };
                let result = command.args(args).output().unwrap();
                assert!(result.stdout.is_empty());
                assert_eq!(
                    std::fs::symlink_metadata(&destination).unwrap().ino(),
                    inode
                );
                let payload = if dangling {
                    assert!(!target.exists());
                    None
                } else {
                    let payload = std::fs::read(&target).unwrap();
                    if !result.status.success() || flag == "-n" {
                        assert_eq!(payload, b"preserve");
                    } else {
                        assert_eq!(payload, bytes);
                    }
                    Some(payload)
                };
                outcomes.push((result.status.code(), payload));
            }
            assert_eq!(outcomes[0], outcomes[1], "dangling={dangling}, {flag}");
        }
    }
}

#[test]
fn contributing_inputs_preserve_complete_packet_order_and_decoded_output() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-contributing-inputs");
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let capture = |path: &Path| -> Value {
        let result = Command::new(&ffprobe_bin)
            .args(["-v", "error"])
            .args(&audio_inputs)
            .arg("-i")
            .arg(path)
            .args(["-show_streams", "-show_packets", "-show_frames", "-show_format", "-show_data_hash", "sha256", "-show_entries",
                "stream=codec_name,codec_type,width,height,pix_fmt,sample_rate,channels,channel_layout,time_base,start_pts,duration_ts,nb_frames:stream_tags=language,handler_name:stream_disposition=default:packet=stream_index,pts,dts,duration,size,data_hash,flags:frame=stream_index,pts,best_effort_timestamp,duration,nb_samples,width,height,pix_fmt:format=start_time,duration:format_tags=title", "-of", "json"])
            .output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert!(!value["packets_and_frames"].as_array().unwrap().is_empty());
        value
    };
    for frames in [0, 2] {
        let inputs: Vec<_> = (0..3)
            .map(|index| dir.join(format!("source-{frames}-{index}.mp4")))
            .collect();
        for (index, path) in inputs.iter().enumerate() {
            let rate = if index == 1 { 44_100 } else { 48_000 };
            let generated = Command::new(&ffmpeg_bin)
                .args(["-nostdin", "-v", "error", "-f", "lavfi"])
                .args(&audio_inputs)
                .args([
                    "-i",
                    &format!(
                        "testsrc2=size=64x48:rate=24:duration=1,hue=h={}",
                        index * 60
                    ),
                    "-f",
                    "lavfi",
                ])
                .args(&audio_inputs)
                .args([
                    "-i",
                    &format!(
                        "sine=frequency={}:sample_rate={rate}:duration=1",
                        751 + index * 168
                    ),
                    "-c:v",
                    "libx264",
                    "-threads:v",
                    "1",
                    "-bf",
                    &frames.to_string(),
                    "-c:a",
                    "aac",
                    "-movie_timescale",
                    "1000",
                    "-metadata",
                    &format!("title=Source {index}"),
                    "-metadata:s:a:0",
                    ["language=eng", "language=fra", "language=deu"][index],
                ])
                .arg(path)
                .output()
                .unwrap();
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
        }
        let originals: Vec<_> = inputs.iter().map(|path| capture(path)).collect();
        for stream in [0, 1] {
            let distinct: BTreeSet<Vec<_>> = originals
                .iter()
                .map(|document| {
                    let hashes: Vec<_> = document["packets_and_frames"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|packet| {
                            packet["type"] == "packet" && packet["stream_index"] == stream
                        })
                        .map(|packet| packet["data_hash"].as_str().unwrap().to_owned())
                        .collect();
                    assert!(!hashes.is_empty());
                    hashes
                })
                .collect();
            assert_eq!(distinct.len(), inputs.len(), "source stream {stream}");
        }
        for (name, count, maps, faststart) in [
            ("separate", 2, vec!["0:v:0", "1:a:0"], false),
            ("audio-first", 2, vec!["1:a:0", "0:v:0"], false),
            ("faststart", 2, vec!["0:v:0", "1:a:0"], true),
            ("duplicate", 2, vec!["0:v:0", "1:a:0", "1:a:0"], false),
            ("three", 3, vec!["0:v:0", "1:v:0", "2:a:0"], false),
            ("reversed", 2, vec!["1:v:0", "0:a:0"], false),
        ] {
            let mut args = vec!["-y"];
            for path in inputs.iter().take(count) {
                args.extend(["-i", path.to_str().unwrap()]);
            }
            for map in maps {
                args.extend(["-map", map]);
            }
            args.extend(["-c", "copy"]);
            if faststart {
                args.extend(["-movflags", "+faststart"]);
            }
            let expected = dir.join("expected.mp4");
            let actual = dir.join("actual.mp4");
            let reference = Command::new(&ffmpeg_bin)
                .args(["-nostdin", "-v", "error"])
                .args(&args)
                .arg(&expected)
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            args.push(actual.to_str().unwrap());
            let native = ffmpeg(&args);
            assert_eq!(native.code, 0, "{name}/{frames}: {}", native.stderr);
            let ours = capture(&actual);
            let theirs = capture(&expected);
            assert_eq!(ours, theirs, "{name}/{frames}");
            let entries = theirs["streams"].as_array().unwrap();
            for (index, stream) in entries.iter().enumerate() {
                let decode = |path: &Path| {
                    let mut command = Command::new(&ffmpeg_bin);
                    command
                        .args(["-nostdin", "-v", "error"])
                        .args(&audio_inputs)
                        .arg("-i")
                        .arg(path)
                        .args(["-map", &format!("0:{index}")]);
                    if stream["codec_type"] == "audio" {
                        command.args(["-c:a", "pcm_f32le"]);
                    } else {
                        command.args(["-c:v", "rawvideo", "-fps_mode", "passthrough"]);
                    }
                    let result = command.args(["-f", "md5", "-"]).output().unwrap();
                    assert!(
                        result.status.success(),
                        "{name}/{index}: {}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                    assert!(result.stdout.starts_with(b"MD5="));
                    result.stdout
                };
                assert_eq!(
                    decode(&actual),
                    decode(&expected),
                    "{name}/{frames}/{index}"
                );
            }
        }
        for selected in [0, 1] {
            let original = std::fs::read(&inputs[selected]).unwrap();
            let mut shortened = original.clone();
            let edit = shortened
                .windows(4)
                .position(|bytes| bytes == b"elst")
                .unwrap();
            assert_eq!(shortened[edit + 4], 0);
            shortened[edit + 12..edit + 16].copy_from_slice(&100u32.to_be_bytes());
            std::fs::write(&inputs[selected], &shortened).unwrap();
            let actual = dir.join("refused.mp4");
            let expected = dir.join("fallback-reference.mp4");
            let video_map = format!("{selected}:v:0");
            let audio_map = format!("{}:a:0", 1 - selected);
            let args = [
                "-y",
                "-i",
                inputs[0].to_str().unwrap(),
                "-i",
                inputs[1].to_str().unwrap(),
                "-map",
                &video_map,
                "-map",
                &audio_map,
                "-c",
                "copy",
            ];
            let reference = Command::new(&ffmpeg_bin)
                .args(["-nostdin", "-v", "error"])
                .args(args)
                .arg(&expected)
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{}",
                String::from_utf8_lossy(&reference.stderr)
            );
            for existing in [false, true] {
                if existing {
                    std::fs::write(&actual, b"existing output").unwrap();
                }
                let entries: BTreeSet<_> = std::fs::read_dir(&dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect();
                let full = [args.as_slice(), &[actual.to_str().unwrap()]].concat();
                let refused = ffmpeg(&full);
                assert_eq!(refused.code, 4, "{}", refused.stderr);
                assert!(refused.stderr.contains("edit"), "{}", refused.stderr);
                assert!(refused.stdout.is_empty());
                assert_eq!(
                    std::fs::read_dir(&dir)
                        .unwrap()
                        .map(|entry| entry.unwrap().file_name())
                        .collect::<BTreeSet<_>>(),
                    entries
                );
                if existing {
                    assert_eq!(std::fs::read(&actual).unwrap(), b"existing output");
                } else {
                    assert!(!actual.exists());
                }
                let fallback = Command::new(bin())
                    .arg("ffmpeg")
                    .args(&full)
                    .env("COREAV_FALLBACK", &ffmpeg_bin)
                    .env_remove("COREAV_STRICT_SAFE")
                    .env_remove("COREAV_FALLBACK_ACTIVE")
                    .env_remove("COREAV_FALLBACK_LOG")
                    .output()
                    .unwrap();
                assert_eq!(
                    fallback.status.code(),
                    reference.status.code(),
                    "{}",
                    String::from_utf8_lossy(&fallback.stderr)
                );
                assert_eq!(fallback.stdout, reference.stdout);
                assert_eq!(
                    std::fs::read(&actual).unwrap(),
                    std::fs::read(&expected).unwrap()
                );
                std::fs::remove_file(&actual).unwrap();
            }
            std::fs::write(&inputs[selected], original).unwrap();
        }
    }
}

#[test]
fn ordinary_copy_preserves_complete_packet_order() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-copy-packet-order");
    let capture = |path: &Path| {
        let result = Command::new(&ffprobe_bin)
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_packets",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(result.stderr.is_empty());
        let document: Value = serde_json::from_slice(&result.stdout).unwrap();
        let streams = document["streams"].as_array().unwrap();
        let packets: Vec<_> = document["packets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|packet| {
                let stream = packet["stream_index"].as_u64().unwrap() as usize;
                let (num, den) = streams[stream]["time_base"]
                    .as_str()
                    .unwrap()
                    .split_once('/')
                    .unwrap();
                let num: i128 = num.parse().unwrap();
                let den: u64 = den.parse().unwrap();
                let clocks = ["pts", "dts", "duration"].map(|key| {
                    packet[key].as_i64().map(|value| {
                        coreav_isobmff::Rational::new(i128::from(value) * num, den).unwrap()
                    })
                });
                (
                    stream,
                    packet["data_hash"].as_str().unwrap().to_owned(),
                    clocks,
                )
            })
            .collect();
        assert!(!packets.is_empty());
        (document, packets)
    };
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    for (rate, frames, delay) in [
        (48_000, 0, false),
        (48_000, 2, false),
        (44_100, 2, false),
        (48_000, 2, true),
    ] {
        let source = dir.join(format!("source-{rate}-{frames}-{delay}.mp4"));
        let mut generation = Command::new(&ffmpeg_bin);
        generation
            .args(["-nostdin", "-v", "error", "-f", "lavfi"])
            .args(&audio_inputs)
            .args(["-i", "testsrc2=size=80x64:rate=10:duration=1.4"]);
        if delay {
            generation.args(["-itsoffset", "0.1"]);
        }
        let generated = generation
            .args(["-f", "lavfi"])
            .args(&audio_inputs)
            .args([
                "-i",
                &format!("sine=frequency=523:sample_rate={rate}:duration=1.4"),
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                &frames.to_string(),
                "-c:a",
                "aac",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        for (name, options, extra_input) in [
            ("ordinary", vec!["-c", "copy"], false),
            (
                "faststart",
                vec!["-c", "copy", "-movflags", "+faststart"],
                false,
            ),
            ("explicit", vec!["-c:v", "copy", "-c:a", "copy"], false),
            ("all", vec!["-map", "0", "-c", "copy"], false),
            (
                "audio-first",
                vec!["-map", "0:a", "-map", "0:v", "-c", "copy"],
                false,
            ),
            (
                "duplicate",
                vec!["-map", "0:v", "-map", "0:a", "-map", "0:a", "-c", "copy"],
                false,
            ),
            (
                "input-zero",
                vec!["-map", "0:v:0", "-map", "0:a:0", "-c", "copy"],
                true,
            ),
            ("video-only", vec!["-an", "-c:v", "copy"], false),
            ("audio-only", vec!["-vn", "-c:a", "copy"], false),
        ] {
            let actual = dir.join("actual.mp4");
            let expected = dir.join("expected.mp4");
            let mut args = vec!["-y", "-i", source.to_str().unwrap()];
            if extra_input {
                args.extend(["-i", source.to_str().unwrap()]);
            }
            args.extend(options);
            let reference = Command::new(&ffmpeg_bin)
                .args(["-nostdin", "-v", "error"])
                .args(&args)
                .arg(&expected)
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{name}: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let result = ffmpeg(&[args.as_slice(), &[actual.to_str().unwrap()]].concat());
            assert_eq!(result.code, 0, "{name}: {}", result.stderr);
            let (mine, packets) = capture(&actual);
            let (theirs, reference_packets) = capture(&expected);
            assert_eq!(
                packets, reference_packets,
                "{name}: {rate}/{frames}/{delay}"
            );
            for (a, b) in mine["streams"]
                .as_array()
                .unwrap()
                .iter()
                .zip(theirs["streams"].as_array().unwrap())
            {
                for key in [
                    "codec_name",
                    "codec_type",
                    "width",
                    "height",
                    "sample_rate",
                    "channels",
                    "start_time",
                    "duration",
                ] {
                    assert_eq!(a[key], b[key], "{name}: {key}");
                }
            }
        }
    }
}

#[test]
fn unedited_video_copy_normalizes_its_origin_without_losing_frames() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-copy-video-origin");
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let make = |name: &str, extra: &[&str]| {
        let path = dir.join(name);
        let result = Command::new(&ffmpeg_bin)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=24:duration=3.25",
                "-an",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "2",
                "-metadata",
                "title=Origin example",
            ])
            .args(extra)
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        path
    };
    let input = make("unedited.mp4", &["-use_editlist", "0"]);
    let edited = make("edited.mp4", &["-output_ts_offset", "1"]);
    let listing = |path: &Path| -> Value {
        let result = Command::new(&ffprobe_bin).args(["-v", "error"])
            .args(&audio_inputs).args(["-i"]).arg(path)
            .args(["-show_streams", "-show_frames", "-show_format", "-show_entries",
                "stream=codec_type,codec_name,time_base,start_pts,duration_ts,nb_frames,width,height,pix_fmt,sample_aspect_ratio:stream_tags=language:frame=pts,pkt_dts,best_effort_timestamp,duration,width,height,pix_fmt,key_frame,pict_type:format=start_time,duration:format_tags=title",
                "-of", "json"]).output().unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    };
    let pixels = |path: &Path| {
        let result = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(&audio_inputs)
            .arg("-i")
            .arg(path)
            .args([
                "-map",
                "0:v:0",
                "-an",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout.len(), 78 * 64 * 48 * 3 / 2);
        result.stdout
    };
    let source = listing(&input);
    assert_eq!(source["streams"][0]["start_pts"], 1024);
    assert_eq!(source["frames"].as_array().unwrap().len(), 78);
    let source_pixels = pixels(&input);
    for (case, options) in [
        ("plain", vec!["-c", "copy"]),
        ("faststart", vec!["-c", "copy", "-movflags", "+faststart"]),
        ("explicit", vec!["-c:v", "copy", "-c:a", "copy"]),
        ("mapped", vec!["-map", "0:v:0", "-c", "copy"]),
    ] {
        let mine = dir.join(format!("{case}-coreav.mp4"));
        let reference = dir.join(format!("{case}-ffmpeg.mp4"));
        let mut args = vec!["-i", input.to_str().unwrap()];
        args.extend(&options);
        args.push(mine.to_str().unwrap());
        let result = ffmpeg(&args);
        assert_eq!(result.code, 0, "{case}: {}", result.stderr);
        assert!(result.stdout.is_empty() && result.stderr.is_empty());
        let result = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&input)
            .args(&options)
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let observed = listing(&mine);
        assert_eq!(observed, listing(&reference), "{case}");
        assert_eq!(observed["streams"][0]["start_pts"], 0);
        assert_eq!(pixels(&mine), source_pixels, "{case}");
        assert_eq!(pixels(&mine), pixels(&reference), "{case}");
    }
    let refused = dir.join("refused.mp4");
    for existing in [false, true] {
        if existing {
            std::fs::write(&refused, b"existing output").unwrap();
        }
        let result = ffmpeg(&[
            "-y",
            "-i",
            edited.to_str().unwrap(),
            "-c",
            "copy",
            refused.to_str().unwrap(),
        ]);
        assert_eq!(result.code, 4, "{}", result.stderr);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.contains("video origin"), "{}", result.stderr);
        if existing {
            assert_eq!(std::fs::read(&refused).unwrap(), b"existing output");
        } else {
            assert!(!refused.exists());
        }
    }
    let path = std::env::join_paths(
        std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let result = Command::new(bin())
        .args(["ffmpeg", "-y", "-i"])
        .arg(&edited)
        .args(["-c", "copy"])
        .arg(&refused)
        .env_remove("COREAV_STRICT_SAFE")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let reference = dir.join("edited-reference.mp4");
    assert!(
        Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&edited)
            .args(["-c", "copy"])
            .arg(&reference)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(listing(&refused), listing(&reference));
    assert_eq!(pixels(&refused), pixels(&reference));
}

#[test]
fn copied_video_refuses_a_later_sample_before_its_first_composition() {
    copy_origin_mutation_agrees("first-composition", b"ctts", 16, 4096, true);
}

#[test]
fn ordinary_copy_rebuilds_declared_durations_for_video_and_audio() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-copy-header-durations");
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let av = dir.join("av.mp4");
    let generated = Command::new(&ffmpeg_bin)
        .args(["-nostdin", "-v", "error", "-f", "lavfi"])
        .args(&audio_inputs)
        .args([
            "-i",
            "testsrc2=size=64x48:rate=24:duration=3.25",
            "-f",
            "lavfi",
        ])
        .args(&audio_inputs)
        .args([
            "-i",
            "sine=frequency=751:sample_rate=44100:duration=3.25",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-threads:v",
            "1",
            "-bf",
            "2",
            "-c:a",
            "aac",
            "-metadata",
            "title=Copied clock example",
        ])
        .arg(&av)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let unedited = dir.join("unedited.mp4");
    let video = dir.join("video.mp4");
    for (path, options) in [
        (
            &unedited,
            vec!["-map", "0", "-c", "copy", "-use_editlist", "0"],
        ),
        (&video, vec!["-map", "0:v:0", "-c", "copy"]),
    ] {
        let result = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&av)
            .args(options)
            .arg(path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let listing = |path: &Path| -> Value {
        let result = Command::new(&ffprobe_bin).args(["-v", "error"]).args(&audio_inputs)
            .arg("-i").arg(path).args(["-show_streams", "-show_frames", "-show_format", "-show_entries",
                "stream=codec_type,codec_name,time_base,start_pts,duration_ts,nb_frames,width,height,pix_fmt,sample_fmt,sample_rate,channels,channel_layout:stream_tags=language:frame=stream_index,pts,pkt_dts,best_effort_timestamp,duration,nb_samples,width,height,pix_fmt:format=start_time,duration:format_tags=title",
                "-of", "json"]).output().unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut value: Value = serde_json::from_slice(&result.stdout).unwrap();
        let frames = value["frames"].take();
        let mut per_stream = vec![Vec::new(); value["streams"].as_array().unwrap().len()];
        for frame in frames.as_array().unwrap() {
            per_stream[frame["stream_index"].as_u64().unwrap() as usize].push(frame.clone());
        }
        value["frames"] = serde_json::to_value(per_stream).unwrap();
        value
    };
    let decoded = |path: &Path, audio| {
        let mut command = Command::new(&ffmpeg_bin);
        command
            .args(["-nostdin", "-v", "error"])
            .args(&audio_inputs)
            .arg("-i")
            .arg(path);
        if audio {
            command.args([
                "-map",
                "0:a:0",
                "-vn",
                "-c:a",
                "pcm_f64le",
                "-f",
                "f64le",
                "-",
            ]);
        } else {
            command.args([
                "-map",
                "0:v:0",
                "-an",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
                "-",
            ]);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.stdout.is_empty());
        result.stdout
    };
    let mut unordered = std::fs::read(&av).unwrap();
    let at = unordered
        .windows(4)
        .position(|bytes| bytes == b"ctts")
        .unwrap()
        + 16;
    unordered[at..at + 4].copy_from_slice(&4096u32.to_be_bytes());
    let refused_input = dir.join("unordered.mp4");
    let refused_output = dir.join("refused.mp4");
    std::fs::write(&refused_input, unordered).unwrap();
    assert_eq!(listing(&refused_input)["format"]["start_time"], "0.000000");
    for existing in [false, true] {
        if existing {
            std::fs::write(&refused_output, b"existing output").unwrap();
        }
        let result = ffmpeg(&[
            "-y",
            "-i",
            refused_input.to_str().unwrap(),
            "-c",
            "copy",
            refused_output.to_str().unwrap(),
        ]);
        assert_eq!(result.code, 4, "{}", result.stderr);
        if existing {
            assert_eq!(std::fs::read(&refused_output).unwrap(), b"existing output");
        } else {
            assert!(!refused_output.exists());
        }
    }
    let reference = dir.join("unordered-reference.mp4");
    assert!(
        Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&refused_input)
            .args(["-c", "copy"])
            .arg(&reference)
            .status()
            .unwrap()
            .success()
    );
    let path = std::env::join_paths(
        std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let result = Command::new(bin())
        .args(["ffmpeg", "-y", "-i"])
        .arg(&refused_input)
        .args(["-c", "copy"])
        .arg(&refused_output)
        .env("PATH", path)
        .env_remove("COREAV_STRICT_SAFE")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(listing(&refused_output), listing(&reference));
    assert_eq!(
        std::fs::read(&refused_output).unwrap(),
        std::fs::read(&reference).unwrap()
    );
    for (source, stream) in [
        (&av, 0),
        (&av, 1),
        (&unedited, 0),
        (&unedited, 1),
        (&video, 0),
    ] {
        for declared in [10_000u32, 100_000, 0] {
            let mut bytes = std::fs::read(source).unwrap();
            let at = bytes
                .windows(4)
                .enumerate()
                .filter_map(|(i, b)| (b == b"mdhd").then_some(i))
                .nth(stream)
                .unwrap()
                + 20;
            bytes[at..at + 4].copy_from_slice(&declared.to_be_bytes());
            let input = dir.join("mutated.mp4");
            std::fs::write(&input, bytes).unwrap();
            for options in [
                vec!["-c", "copy"],
                vec!["-c", "copy", "-movflags", "+faststart"],
                vec!["-c:v", "copy", "-c:a", "copy"],
                vec!["-map", "0", "-c", "copy"],
            ] {
                let output = dir.join("output.mp4");
                let reference = dir.join("reference.mp4");
                let mut args = vec!["-y", "-i", input.to_str().unwrap()];
                args.extend(&options);
                args.push(output.to_str().unwrap());
                let result = ffmpeg(&args);
                assert_eq!(result.code, 0, "{}", result.stderr);
                assert!(result.stdout.is_empty() && result.stderr.is_empty());
                let reference_run = Command::new(&ffmpeg_bin)
                    .args(["-nostdin", "-v", "error", "-y", "-i"])
                    .arg(&input)
                    .args(&options)
                    .arg(&reference)
                    .output()
                    .unwrap();
                assert!(
                    reference_run.status.success(),
                    "{}",
                    String::from_utf8_lossy(&reference_run.stderr)
                );
                assert_eq!(
                    listing(&output),
                    listing(&reference),
                    "{} stream {stream}, mdhd {declared}, {options:?}",
                    source.display()
                );
                assert_eq!(decoded(&output, false), decoded(&reference, false));
                assert_eq!(decoded(&output, false).len(), 78 * 64 * 48 * 3 / 2);
                if source != &video {
                    assert_eq!(decoded(&output, true), decoded(&reference, true));
                }
            }
        }
    }
}

#[test]
fn ordinary_copy_refuses_trimmed_simple_edits() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-copy-trimmed-edits");
    let base = dir.join("base.mp4");
    let generated = Command::new(&ffmpeg_bin)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=24:duration=3.25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=751:sample_rate=44100:duration=3.25",
            "-c:v",
            "libx264",
            "-threads:v",
            "1",
            "-bf",
            "2",
            "-c:a",
            "aac",
            "-movie_timescale",
            "1000",
        ])
        .arg(&base)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let video = dir.join("video.mp4");
    assert!(
        Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&base)
            .args(["-map", "0:v:0", "-c", "copy", "-movie_timescale", "1000"])
            .arg(&video)
            .status()
            .unwrap()
            .success()
    );
    let path = std::env::join_paths(
        std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    for (source, stream, segment) in [
        (&video, 0, 1000u32),
        (&base, 0, 1000),
        (&base, 1, 1000),
        (&video, 0, 3249),
        (&video, 0, 4000),
        (&base, 1, 4000),
    ] {
        for declared in [None, Some(10_000u32)] {
            let mut bytes = std::fs::read(source).unwrap();
            let at = bytes
                .windows(4)
                .enumerate()
                .filter_map(|(i, b)| (b == b"elst").then_some(i))
                .nth(stream)
                .unwrap()
                + 12;
            bytes[at..at + 4].copy_from_slice(&segment.to_be_bytes());
            if let Some(declared) = declared {
                let at = bytes
                    .windows(4)
                    .enumerate()
                    .filter_map(|(i, b)| (b == b"mdhd").then_some(i))
                    .nth(stream)
                    .unwrap()
                    + 20;
                bytes[at..at + 4].copy_from_slice(&declared.to_be_bytes());
            }
            let input = dir.join("trimmed.mp4");
            std::fs::write(&input, bytes).unwrap();
            for options in [
                vec!["-c", "copy"],
                vec!["-c", "copy", "-movflags", "+faststart"],
                vec!["-map", "0", "-c", "copy"],
            ] {
                let reference = dir.join("reference.mp4");
                let copied = Command::new(&ffmpeg_bin)
                    .args(["-nostdin", "-v", "error", "-y", "-i"])
                    .arg(&input)
                    .args(&options)
                    .arg(&reference)
                    .output()
                    .unwrap();
                assert!(
                    copied.status.success(),
                    "{}",
                    String::from_utf8_lossy(&copied.stderr)
                );
                if source == &video && segment == 1000 {
                    let probe = Command::new(&ffprobe_bin)
                        .args([
                            "-v",
                            "error",
                            "-show_streams",
                            "-show_format",
                            "-of",
                            "json",
                        ])
                        .arg(&reference)
                        .output()
                        .unwrap();
                    assert!(probe.status.success());
                    let listing: Value = serde_json::from_slice(&probe.stdout).unwrap();
                    assert_eq!(listing["streams"][0]["duration_ts"], 12_800);
                    assert_eq!(listing["format"]["duration"], "1.041667");
                }
                let output = dir.join("output.mp4");
                let mut args = vec!["-y", "-i", input.to_str().unwrap()];
                args.extend(&options);
                args.push(output.to_str().unwrap());
                if source == &base && stream == 1 && segment == 4000 {
                    let result = ffmpeg(&args);
                    assert_eq!(result.code, 0, "{}", result.stderr);
                    let listing = |path: &Path| {
                        let result = Command::new(&ffprobe_bin).args([
                            "-v", "error", "-show_streams", "-show_frames", "-show_format",
                            "-show_entries", "stream=codec_name,time_base,start_pts,duration_ts,nb_frames:stream_tags=language:frame=stream_index,pts,pkt_dts,best_effort_timestamp,duration,nb_samples:format=start_time,duration:format_tags=title", "-of", "json"])
                            .arg(path).output().unwrap();
                        assert!(result.status.success() && result.stderr.is_empty());
                        let mut listing: Value = serde_json::from_slice(&result.stdout).unwrap();
                        let frames = listing["frames"].take();
                        let mut per_stream =
                            vec![Vec::new(); listing["streams"].as_array().unwrap().len()];
                        for frame in frames.as_array().unwrap() {
                            per_stream[frame["stream_index"].as_u64().unwrap() as usize]
                                .push(frame.clone());
                        }
                        listing["frames"] = serde_json::to_value(per_stream).unwrap();
                        listing
                    };
                    assert_eq!(listing(&output), listing(&reference));
                    let decoded = |path: &Path| {
                        let result = Command::new(&ffmpeg_bin)
                            .args(["-nostdin", "-v", "error"])
                            .args(
                                include_str!("../../../scripts/oracle-audio-args.txt")
                                    .split_whitespace(),
                            )
                            .arg("-i")
                            .arg(path)
                            .args([
                                "-map",
                                "0:a:0",
                                "-vn",
                                "-c:a",
                                "pcm_f64le",
                                "-f",
                                "f64le",
                                "-",
                            ])
                            .output()
                            .unwrap();
                        assert!(result.status.success() && result.stderr.is_empty());
                        assert!(!result.stdout.is_empty());
                        result.stdout
                    };
                    assert_eq!(decoded(&output), decoded(&reference));
                    std::fs::remove_file(&output).unwrap();
                    continue;
                }
                for existing in [false, true] {
                    if existing {
                        std::fs::write(&output, b"existing output").unwrap();
                    }
                    let result = ffmpeg(&args);
                    assert_eq!(result.code, 4, "{}", result.stderr);
                    if existing {
                        assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                    } else {
                        assert!(!output.exists());
                    }
                }
                let result = Command::new(bin())
                    .arg("ffmpeg")
                    .args(&args)
                    .env("PATH", &path)
                    .env_remove("COREAV_STRICT_SAFE")
                    .env_remove("COREAV_FALLBACK_ACTIVE")
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(
                    std::fs::read(&output).unwrap(),
                    std::fs::read(&reference).unwrap()
                );
                std::fs::remove_file(&output).unwrap();
            }
        }
    }
}

#[test]
fn first_input_only_maps_reuse_ordinary_copy_timing() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-first-input-copy");
    let first = dir.join("first.mp4");
    let unused = dir.join("unused.mp4");
    for (path, size, frequency, title) in [
        (&first, "64x48", "751", "First mapped input"),
        (&unused, "80x64", "919", "Unused input"),
    ] {
        let result = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size={size}:rate=24:duration=3.25"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency={frequency}:sample_rate=44100:duration=3.25"
            ))
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-bf",
                "2",
                "-c:a",
                "aac",
                "-movie_timescale",
                "1000",
                "-metadata",
            ])
            .arg(format!("title={title}"))
            .arg(path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let listing = |path: &Path| -> Value {
        let result = Command::new(&ffprobe_bin).args(["-v", "error"]).args(&audio_inputs)
            .arg("-i").arg(path).args(["-show_streams", "-show_frames", "-show_format", "-show_entries",
                "stream=codec_type,codec_name,time_base,start_pts,duration_ts,nb_frames,width,height,pix_fmt,sample_fmt,sample_rate,channels,channel_layout:stream_tags=language:frame=stream_index,pts,pkt_dts,best_effort_timestamp,duration,nb_samples,width,height,pix_fmt:format=start_time,duration:format_tags=title", "-of", "json"])
            .output().unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut value: Value = serde_json::from_slice(&result.stdout).unwrap();
        let frames = value["frames"].take();
        let mut per_stream = vec![Vec::new(); value["streams"].as_array().unwrap().len()];
        for frame in frames.as_array().unwrap() {
            per_stream[frame["stream_index"].as_u64().unwrap() as usize].push(frame.clone());
        }
        value["frames"] = serde_json::to_value(per_stream).unwrap();
        value
    };
    let decoded = |path: &Path, audio| {
        let mut command = Command::new(&ffmpeg_bin);
        command
            .args(["-nostdin", "-v", "error"])
            .args(&audio_inputs)
            .arg("-i")
            .arg(path);
        if audio {
            command.args([
                "-map",
                "0:a:0",
                "-vn",
                "-c:a",
                "pcm_f64le",
                "-f",
                "f64le",
                "-",
            ]);
        } else {
            command.args([
                "-map",
                "0:v:0",
                "-an",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
                "-",
            ]);
        }
        let result = command.output().unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.stdout.is_empty());
        result.stdout
    };
    for stream in [0, 1] {
        for declared in [10_000u32, 100_000, 1_000_000, 0] {
            let mut bytes = std::fs::read(&first).unwrap();
            let at = bytes
                .windows(4)
                .enumerate()
                .filter_map(|(i, b)| (b == b"mdhd").then_some(i))
                .nth(stream)
                .unwrap()
                + 20;
            bytes[at..at + 4].copy_from_slice(&declared.to_be_bytes());
            let input = dir.join("mutated.mp4");
            std::fs::write(&input, bytes).unwrap();
            for (maps, extra, video, audio) in [
                (vec!["0:v:0", "0:a:0"], vec![], true, true),
                (vec!["0", "1:a", "-1:a"], vec![], true, true),
                (
                    vec!["0:v:0", "0:a:0"],
                    vec!["-movflags", "+faststart"],
                    true,
                    true,
                ),
                (vec!["0:v:0"], vec![], true, false),
                (vec!["0:a:0"], vec![], false, true),
            ] {
                let output = dir.join("output.mp4");
                let reference = dir.join("reference.mp4");
                let mut args = vec![
                    "-y",
                    "-i",
                    input.to_str().unwrap(),
                    "-i",
                    unused.to_str().unwrap(),
                    "-c",
                    "copy",
                ];
                for map in &maps {
                    args.extend(["-map", *map]);
                }
                args.extend(extra);
                let copied = Command::new(&ffmpeg_bin)
                    .args(["-nostdin", "-v", "error"])
                    .args(&args)
                    .arg(&reference)
                    .output()
                    .unwrap();
                assert!(
                    copied.status.success(),
                    "{}",
                    String::from_utf8_lossy(&copied.stderr)
                );
                args.push(output.to_str().unwrap());
                let result = ffmpeg(&args);
                assert_eq!(result.code, 0, "{}", result.stderr);
                assert!(result.stdout.is_empty() && result.stderr.is_empty());
                let actual = listing(&output);
                let expected = listing(&reference);
                assert_eq!(
                    actual["streams"], expected["streams"],
                    "stream {stream}, mdhd {declared}, {maps:?}"
                );
                assert_eq!(actual["format"], expected["format"]);
                assert_eq!(actual["frames"], expected["frames"]);
                assert_eq!(actual["format"]["tags"]["title"], "First mapped input");
                if video {
                    let pixels = decoded(&output, false);
                    assert_eq!(pixels, decoded(&reference, false));
                    assert_eq!(pixels.len(), 78 * 64 * 48 * 3 / 2);
                }
                if audio {
                    assert_eq!(decoded(&output, true), decoded(&reference, true));
                }
            }
        }
    }
    for (kind, offset, value) in [(b"elst", 12, 1000u32), (b"ctts", 16, 4096)] {
        let mut bytes = std::fs::read(&first).unwrap();
        let at = bytes.windows(4).position(|word| word == kind).unwrap() + offset;
        bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
        let input = dir.join("refused-input.mp4");
        let output = dir.join("refused-output.mp4");
        std::fs::write(&input, bytes).unwrap();
        let args = [
            "-y",
            "-i",
            input.to_str().unwrap(),
            "-i",
            unused.to_str().unwrap(),
            "-map",
            "0:v:0",
            "-map",
            "0:a:0",
            "-c",
            "copy",
            output.to_str().unwrap(),
        ];
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let count = std::fs::read_dir(&dir).unwrap().count();
            let result = ffmpeg(&args);
            assert_eq!(result.code, 4, "{}", result.stderr);
            assert!(result.stdout.is_empty());
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), count);
        }
        let reference = dir.join("refused-reference.mp4");
        let copied = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(&args[..args.len() - 1])
            .arg(&reference)
            .output()
            .unwrap();
        assert!(
            copied.status.success(),
            "{}",
            String::from_utf8_lossy(&copied.stderr)
        );
        let path = std::env::join_paths(
            std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ),
        )
        .unwrap();
        let fallback = Command::new(bin())
            .arg("ffmpeg")
            .args(args)
            .env("PATH", path)
            .env_remove("COREAV_STRICT_SAFE")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .output()
            .unwrap();
        assert!(
            fallback.status.success(),
            "{}",
            String::from_utf8_lossy(&fallback.stderr)
        );
        assert_eq!(
            std::fs::read(&output).unwrap(),
            std::fs::read(&reference).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let directory = dir.join("fallback-bin");
            std::fs::create_dir_all(&directory).unwrap();
            let program = directory.join("ffmpeg");
            std::fs::write(&program, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let result = Command::new(bin())
                .arg("ffmpeg")
                .args(args)
                .env_clear()
                .env("PATH", &directory)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(19));
            assert_eq!(result.stdout, fallback_arguments(&args));
        }
        std::fs::remove_file(output).unwrap();
    }
}

#[test]
fn copied_video_reconstructs_conflicting_declared_media_durations() {
    for duration in [10_000, 100_000, 0] {
        copy_origin_mutation_agrees(
            &format!("media-duration-{duration}"),
            b"mdhd",
            20,
            duration,
            false,
        );
    }
}

fn copy_origin_mutation_agrees(
    name: &str,
    kind: &[u8; 4],
    offset: usize,
    value: u32,
    refuse: bool,
) {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch(&format!("ffmpeg-copy-origin-{name}"));
    let input = dir.join("input.mp4");
    let generated = Command::new(&ffmpeg_bin)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x48:rate=24",
            "-frames:v",
            "77",
            "-c:v",
            "libx264",
            "-threads:v",
            "1",
            "-bf",
            "2",
            "-use_editlist",
            "0",
        ])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let mut data = std::fs::read(&input).unwrap();
    let at = data.windows(4).position(|bytes| bytes == kind).unwrap() + offset;
    data[at..at + 4].copy_from_slice(&value.to_be_bytes());
    std::fs::write(&input, data).unwrap();
    let reference = dir.join("reference.mp4");
    let result = Command::new(&ffmpeg_bin)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(&input)
        .args(["-c", "copy"])
        .arg(&reference)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let audio_inputs: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let listing = |path: &Path| -> Value {
        let result = Command::new(&ffprobe_bin).args(["-v", "error"])
            .args(&audio_inputs).arg("-i").arg(path)
            .args(["-show_streams", "-show_frames", "-show_format", "-show_entries",
                "stream=codec_type,codec_name,time_base,start_pts,duration_ts,nb_frames,width,height,pix_fmt:stream_tags=language:frame=pts,pkt_dts,best_effort_timestamp,duration,width,height,pix_fmt,key_frame,pict_type:format=start_time,duration:format_tags=title",
                "-of", "json"]).output().unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    };
    let pixels = |path: &Path| {
        let result = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(&audio_inputs)
            .arg("-i")
            .arg(path)
            .args([
                "-map",
                "0:v:0",
                "-an",
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "rawvideo",
                "-",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.stdout.is_empty());
        result.stdout
    };
    let expected = listing(&reference);
    assert_eq!(expected["streams"][0]["start_pts"], 0);
    let output = dir.join("output.mp4");
    let strict = || {
        ffmpeg(&[
            "-y",
            "-i",
            input.to_str().unwrap(),
            "-c",
            "copy",
            output.to_str().unwrap(),
        ])
    };
    let result = strict();
    if refuse {
        assert_eq!(
            result.code, 4,
            "{}; pinned output: {expected}",
            result.stderr
        );
        assert!(result.stdout.is_empty() && !output.exists());
        std::fs::write(&output, b"existing output").unwrap();
        assert_eq!(strict().code, 4);
        assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
        let path = std::env::join_paths(
            std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ),
        )
        .unwrap();
        let result = Command::new(bin())
            .args(["ffmpeg", "-y", "-i"])
            .arg(&input)
            .args(["-c", "copy"])
            .arg(&output)
            .env_remove("COREAV_STRICT_SAFE")
            .env_remove("COREAV_FALLBACK_ACTIVE")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    } else {
        assert_eq!(result.code, 0, "{}", result.stderr);
        assert!(result.stdout.is_empty() && result.stderr.is_empty());
        assert_eq!(expected["streams"][0]["duration_ts"], 39_424);
        assert_eq!(pixels(&output), pixels(&input));
        assert_eq!(pixels(&output).len(), 77 * 64 * 48 * 3 / 2);
    }
    assert_eq!(listing(&output), expected, "{name}");
    assert_eq!(pixels(&output), pixels(&reference), "{name}");
}

/// Cover picture selections against the pinned `ffmpeg`, on a file it
/// makes with one JPEG cover picture.
#[test]
fn cover_selections_agree_with_the_pinned_ffmpeg() {
    covers_agree(
        "ffmpeg-cover-oracle",
        &[("32x32", "cover.jpg")],
        &[
            &["-map", "0"],
            &["-map", "0:0", "-map", "0:1"],
            &[],
            &["-map", "0", "-map", "-0:2"],
        ],
    );
}

/// The same with three pictures in one `covr` item, one `data` box each,
/// as `ffmpeg` writes them (a JPEG, a larger JPEG, a PNG): one stream per
/// picture, and a selection keeps or drops each picture alone.
#[test]
fn picture_by_picture_selections_agree_with_the_pinned_ffmpeg() {
    covers_agree(
        "ffmpeg-covers-oracle",
        &[
            ("32x32", "p0.jpg"),
            ("64x64", "p1.jpg"),
            ("48x48", "p2.png"),
        ],
        &[
            &["-map", "0"],
            &[],
            &["-map", "0:v:0", "-map", "0:a", "-map", "0:v:2"],
            &["-map", "0", "-map", "-0:v:1"],
            &["-map", "0", "-map", "-0:v:2"],
            &["-map", "0:v:0", "-map", "0:a"],
        ],
    );
}

/// On a file the pinned `ffmpeg` makes of H.264 and AAC tagged `eng`, then
/// `pictures` (each a size and a file name) as attached pictures:
/// `coreav ffprobe` lists the streams and the pictures' packets as `ffprobe`
/// does; for each of `selections`, both write outputs that list the same
/// streams (type, codec, attached picture) and the same picture bytes; and
/// for the pictures alone, `ffmpeg` writes a movie without a track, which
/// `coreav ffmpeg` refuses.
fn covers_agree(name: &str, pictures: &[(&str, &str)], selections: &[&[&str]]) {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch(name);
    let make = |args: &[&str]| {
        let status = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-y"])
            .args(args)
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(status.success(), "{args:?}");
    };
    make(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=160x120:rate=10:duration=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "v.mp4",
    ]);
    make(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1",
        "-c:a",
        "aac",
        "a.m4a",
    ]);
    let mut args = vec!["-i", "v.mp4", "-i", "a.m4a"];
    let mut maps = vec!["-map", "0:v", "-map", "1:a"];
    let inputs: Vec<String> = (2..2 + pictures.len()).map(|n| n.to_string()).collect();
    let dispositions: Vec<String> = (1..=pictures.len())
        .map(|n| format!("-disposition:v:{n}"))
        .collect();
    for (((size, file), input), disposition) in pictures.iter().zip(&inputs).zip(&dispositions) {
        let source = format!("testsrc2=size={size}");
        make(&["-f", "lavfi", "-i", &source, "-frames:v", "1", file]);
        args.extend(["-i", file]);
        maps.extend(["-map", input, disposition, "attached_pic"]);
    }
    args.extend(maps);
    args.extend([
        "-c",
        "copy",
        "-metadata:s:v:0",
        "language=eng",
        "-metadata:s:a:0",
        "language=eng",
        "in.mp4",
    ]);
    make(&args);
    let input = dir.join("in.mp4");
    let i = input.to_str().unwrap();
    let streams = |path: &Path| -> Value {
        let out = Command::new(&ffprobe_bin)
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,codec_name:stream_disposition=attached_pic",
                "-of",
                "json",
            ])
            .arg(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", path.display());
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["streams"].clone()
    };
    let md5 = |path: &Path, index: usize| -> String {
        let out = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(path)
            .args([
                "-map",
                &format!("0:{index}"),
                "-c",
                "copy",
                "-f",
                "md5",
                "-",
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "{} stream {index}", path.display());
        let digest = String::from_utf8(out.stdout).unwrap();
        let hex = digest.trim().strip_prefix("MD5=");
        assert!(
            hex.is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())),
            "{} stream {index}: {digest:?}",
            path.display()
        );
        digest
    };
    let picture_md5s = |path: &Path| -> Vec<String> {
        let listed = streams(path);
        (0..listed.as_array().unwrap().len())
            .filter(|n| listed[*n]["disposition"]["attached_pic"] == 1)
            .map(|n| md5(path, n))
            .collect()
    };
    assert_eq!(picture_md5s(&input).len(), pictures.len());
    let probed = |program: &Path| -> Value {
        let mut command = Command::new(program);
        if program == Path::new(bin()) {
            command.arg("ffprobe");
        }
        let out = command
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,codec_name,width,height:packet=stream_index,pos,size",
                "-of",
                "json",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", program.display());
        let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
        let pictures: Vec<Value> = listed["packets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["stream_index"].as_u64() >= Some(2))
            .cloned()
            .collect();
        json!({ "streams": listed["streams"], "pictures": pictures })
    };
    assert_eq!(probed(Path::new(bin())), probed(&ffprobe_bin));
    let (theirs, ours) = (dir.join("ffmpeg.mp4"), dir.join("coreav.mp4"));
    for &maps in selections {
        let run = |program: &Path, output: &Path| {
            let mut command = Command::new(program);
            if program == Path::new(bin()) {
                command.arg("ffmpeg").env("COREAV_STRICT_SAFE", "1");
            } else {
                command.args(["-nostdin", "-v", "error"]);
            }
            let status = command
                .args(["-y", "-i", i, "-c", "copy"])
                .args(maps)
                .arg(output)
                .status()
                .unwrap();
            assert!(status.success(), "{maps:?}: {}", program.display());
        };
        run(&ffmpeg_bin, &theirs);
        run(Path::new(bin()), &ours);
        assert_eq!(streams(&ours), streams(&theirs), "{maps:?}");
        assert_eq!(picture_md5s(&ours), picture_md5s(&theirs), "{maps:?}");
    }
    let alone = [
        "-y",
        "-i",
        i,
        "-c",
        "copy",
        "-map",
        "0",
        "-map",
        "-0:m:language:eng",
    ];
    let status = Command::new(&ffmpeg_bin)
        .args(["-nostdin", "-v", "error"])
        .args(alone)
        .arg(&theirs)
        .status()
        .unwrap();
    assert!(status.success());
    let listed = streams(&theirs);
    assert_eq!(listed.as_array().unwrap().len(), pictures.len());
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["disposition"]["attached_pic"] == 1)
    );
    let written = std::fs::read(&theirs).unwrap();
    assert!(
        !written.windows(4).any(|w| w == b"trak"),
        "ffmpeg wrote a track"
    );
    let _ = std::fs::remove_file(&ours);
    let mut args = alone.to_vec();
    args.push(ours.to_str().unwrap());
    let r = ffmpeg(&args);
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(!ours.exists());
}

/// `-map_metadata -1` on an M4A extraction against the pinned `ffmpeg`, on
/// a file it makes with a language, a handler name, a title and a creation
/// time: both outputs carry none of them (the language `und`, the creation
/// times not set), as `ffprobe` lists their tags.
#[test]
fn stripped_metadata_agrees_with_the_pinned_ffmpeg() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-strip-oracle");
    let input = dir.join("in.mp4");
    let status = Command::new(&ffmpeg_bin)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-metadata",
            "creation_time=2020-01-02T03:04:05Z",
            "-metadata",
            "title=A title",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:0",
            "handler_name=A handler",
        ])
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success());
    let tags = |path: &Path| metadata_tags(&ffprobe_bin, path);
    let source = tags(&input);
    assert_eq!(source[0]["creation_time"], "2020-01-02T03:04:05.000000Z");
    assert_eq!(source[1]["language"], "eng");
    let (theirs, ours) = (dir.join("ffmpeg.m4a"), dir.join("coreav.m4a"));
    let command = |output: &Path| -> Vec<String> {
        [
            "-y",
            "-i",
            input.to_str().unwrap(),
            "-map",
            "0:a",
            "-c:a",
            "copy",
            "-map_metadata",
            "-1",
        ]
        .iter()
        .map(|a| (*a).to_owned())
        .chain([output.display().to_string()])
        .collect()
    };
    let status = Command::new(&ffmpeg_bin)
        .args(["-nostdin", "-v", "error"])
        .args(command(&theirs))
        .status()
        .unwrap();
    assert!(status.success());
    let args = command(&ours);
    let r = ffmpeg(&args.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(tags(&ours), tags(&theirs));
    assert_eq!(
        tags(&ours),
        json!([{}, {"language": "und", "handler_name": "SoundHandler"}])
    );
}

/// A plain M4A extraction against the pinned `ffmpeg`, on a file it makes
/// with an item list, a language, a handler name and a creation time: both
/// outputs carry the same file and stream tags, the creation times and the
/// file type box's brands aside.
/// `coreav ffmpeg` keeps the headers' creation times, which `ffmpeg`
/// writes as 0 (`docs/design/compat-cli.md`, "M4A metadata").
#[test]
fn plain_extraction_metadata_agrees_with_the_pinned_ffmpeg() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-plain-metadata-oracle");
    let input = dir.join("in.mp4");
    let mut make = Command::new(&ffmpeg_bin);
    make.args([
        "-nostdin",
        "-v",
        "error",
        "-y",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=160x120:rate=10:duration=1",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-metadata:s:a:0",
        "language=eng",
        "-metadata:s:a:0",
        "handler_name=A handler",
    ]);
    for tag in [
        "creation_time=2020-01-02T03:04:05Z",
        "title=A title",
        "artist=An artist",
        "album=An album",
        "date=2020",
        "comment=A comment",
        "genre=A genre",
        "track=3/9",
        "disc=1/2",
    ] {
        make.args(["-metadata", tag]);
    }
    assert!(make.arg(&input).status().unwrap().success());
    let tags = |path: &Path| {
        let out = Command::new(&ffprobe_bin)
            .args(["-v", "error", "-select_streams", "a", "-show_entries"])
            .args(["format_tags:stream_tags", "-of", "json"])
            .arg(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", path.display());
        let listed = serde_json::from_slice::<Value>(&out.stdout).unwrap();
        json!([listed["format"]["tags"], listed["streams"][0]["tags"]])
    };
    let created = json!("2020-01-02T03:04:05.000000Z");
    let source = tags(&input);
    assert_eq!(source[0]["creation_time"], created);
    assert_eq!(source[1]["handler_name"], "A handler");
    for selection in [["-map", "0:a"], ["-vn", "-sn"]] {
        let (theirs, ours) = (dir.join("ffmpeg.m4a"), dir.join("coreav.m4a"));
        let command = |output: &Path| -> Vec<String> {
            ["-y", "-i", input.to_str().unwrap()]
                .iter()
                .chain(&selection)
                .chain(&["-c:a", "copy"])
                .map(|a| (*a).to_owned())
                .chain([output.display().to_string()])
                .collect()
        };
        let status = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error"])
            .args(command(&theirs))
            .status()
            .unwrap();
        assert!(status.success());
        let args = command(&ours);
        let r = ffmpeg(&args.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(r.code, 0, "{}", r.stderr);
        let (mut ours, mut theirs) = (tags(&ours), tags(&theirs));
        for side in 0..2 {
            assert_eq!(ours[side]["creation_time"], created, "{selection:?}");
            assert_eq!(theirs[side].get("creation_time"), None, "{selection:?}");
            ours[side].as_object_mut().unwrap().remove("creation_time");
        }
        // The file type box is each writer's own, not metadata.
        for tags in [&mut ours, &mut theirs] {
            assert_eq!(tags[0]["major_brand"], "M4A ");
            for brand in ["major_brand", "minor_version", "compatible_brands"] {
                tags[0].as_object_mut().unwrap().remove(brand);
            }
        }
        assert_eq!(ours, theirs, "{selection:?}");
        assert_eq!(ours[0]["title"], "A title");
        assert_eq!(ours[1]["handler_name"], "A handler");
        assert_eq!(ours[1]["language"], "eng");
        assert_eq!(ours[0]["genre"], "A genre");
    }
}

/// The tags `ffprobe` lists for `path` that `-map_metadata` decides: the
/// file's creation time and title, and the first audio stream's language,
/// creation time and handler name.
fn metadata_tags(ffprobe: &Path, path: &Path) -> Value {
    let out = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "format_tags=creation_time,title:stream_tags=language,creation_time,handler_name",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", path.display());
    let listed = serde_json::from_slice::<Value>(&out.stdout).unwrap();
    json!([listed["format"]["tags"], listed["streams"][0]["tags"]])
}

/// `-map_metadata 0:s:0` on an M4A extraction against the pinned `ffmpeg`,
/// on an audio-only file it makes with a language, a handler name, a title
/// and a creation time: the file takes its metadata from the stream (its
/// creation time), and the stream keeps none of its own (the language
/// `und`), in both outputs.
#[test]
fn stream_metadata_agrees_with_the_pinned_ffmpeg() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = scratch("ffmpeg-stream-metadata-oracle");
    let input = dir.join("in.m4a");
    let status = Command::new(&ffmpeg_bin)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "aac",
            "-metadata",
            "creation_time=2020-01-02T03:04:05Z",
            "-metadata",
            "title=A title",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:0",
            "handler_name=A handler",
        ])
        .arg(&input)
        .status()
        .unwrap();
    assert!(status.success());
    let tags = |path: &Path| metadata_tags(&ffprobe_bin, path);
    let source = tags(&input);
    assert_eq!(source[0]["title"], "A title");
    assert_eq!(source[1]["language"], "eng");
    let (theirs, ours) = (dir.join("ffmpeg.m4a"), dir.join("coreav.m4a"));
    let command = |output: &Path| -> Vec<String> {
        [
            "-y",
            "-i",
            input.to_str().unwrap(),
            "-acodec",
            "copy",
            "-map",
            "0",
            "-map_metadata",
            "0:s:0",
        ]
        .iter()
        .map(|a| (*a).to_owned())
        .chain([output.display().to_string()])
        .collect()
    };
    let status = Command::new(&ffmpeg_bin)
        .args(["-nostdin", "-v", "error"])
        .args(command(&theirs))
        .status()
        .unwrap();
    assert!(status.success());
    let args = command(&ours);
    let r = ffmpeg(&args.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(tags(&ours), tags(&theirs));
    let created = "2020-01-02T03:04:05.000000Z";
    assert_eq!(
        tags(&ours),
        json!([
            {"creation_time": created},
            {"creation_time": created, "language": "und", "handler_name": "SoundHandler"}
        ])
    );
}
