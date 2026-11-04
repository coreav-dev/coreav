//! The FFmpeg fallback of `coreav ffprobe` and `coreav ffmpeg` through the
//! binary (`docs/design/compat-cli.md`, "Fallback to FFmpeg"), with a
//! stand-in program in place of FFmpeg: it prints its arguments, copies its
//! standard input to its standard output, reports the recursion guard on
//! standard error and exits 7.

#![cfg(unix)]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;
#[path = "../../coreav-mpegts/tests/common/mod.rs"]
mod ts;

#[path = "common/standalone_audio.rs"]
mod standalone_audio;

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use common::{aac_entry, av_movie_with};
use serde_json::Value;

const STAND_IN: &str = "#!/bin/sh
printf 'args:'
printf ' %s' \"$@\"
printf '\\n'
/bin/cat
echo \"guard=$COREAV_FALLBACK_ACTIVE\" >&2
exit 7
";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("fallback")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The directory of the stand-in `ffmpeg` and `ffprobe`, written once before
/// any test starts a process, so that no child holds a script open for
/// writing when one is run ("text file busy").
fn stand_in() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tmp("stand-in");
        for tool in ["ffmpeg", "ffprobe"] {
            let path = dir.join(tool);
            std::fs::write(&path, STAND_IN).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    })
}

fn movie(dir: &Path) -> String {
    let path = dir.join("in.mp4");
    std::fs::write(&path, av_movie_with(aac_entry(), false)).unwrap();
    path.to_string_lossy().into_owned()
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Runs `program` with `stdin` piped in, an environment of `PATH` and
/// `env` only.
fn run(program: &Path, args: &[&str], path: &[&Path], env: &[(&str, &str)], stdin: &str) -> Run {
    stand_in();
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::join_paths(path).unwrap())
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn coreav(args: &[&str], path: &[&Path], env: &[(&str, &str)]) -> Run {
    run(Path::new(bin()), args, path, env, "")
}

#[test]
fn file_protocol_and_downstream_copy_options_run_natively() {
    let dir = tmp("file-protocol");
    let input = movie(&dir);
    let input_url = format!("file:{input}");
    for flags in [
        vec![],
        vec!["-dn"],
        vec!["-ignore_unknown"],
        vec!["-dn", "-ignore_unknown"],
    ] {
        let output = dir.join(format!("out-{}.mp4", flags.join("")));
        let output_url = format!("file:{}", output.display());
        let mut args = vec![
            "--strict-safe",
            "ffmpeg",
            "-y",
            "-loglevel",
            "repeat+info",
            "-i",
            &input_url,
            "-map",
            "0",
        ];
        args.extend(flags);
        args.extend(["-c", "copy", "-movflags", "+faststart", &output_url]);
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 0, "{}", r.stderr);
        assert!(r.stderr.is_empty(), "{}", r.stderr);
        assert!(output.exists());
    }
    let r = coreav(
        &[
            "--strict-safe",
            "ffprobe",
            "-hide_banner",
            "-show_format",
            "-show_streams",
            "-print_format",
            "json",
            &input_url,
        ],
        &[stand_in()],
        &[],
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let json: Value = serde_json::from_str(&r.stdout).unwrap();
    assert_eq!(json["format"]["filename"], input_url);
    assert!(!json["streams"].as_array().unwrap().is_empty());
}

#[test]
fn file_protocol_names_literal_paths_not_pipes_or_escaped_urls() {
    let dir = tmp("file-protocol-literals");
    for name in ["-", "pipe:0", "with%20space.mp4"] {
        std::fs::write(dir.join(name), av_movie_with(aac_entry(), false)).unwrap();
        let input = format!("file:{name}");
        let r = Command::new(bin())
            .current_dir(&dir)
            .args([
                "--strict-safe",
                "ffmpeg",
                "-y",
                "-i",
                &input,
                "-c",
                "copy",
                "-f",
                "mp4",
                "file:output:literal.mp4",
            ])
            .output()
            .unwrap();
        assert_eq!(
            r.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        assert!(r.stdout.is_empty());
        assert!(dir.join("output:literal.mp4").exists());
    }
    let r = Command::new(bin())
        .current_dir(&dir)
        .args([
            "--strict-safe",
            "ffmpeg",
            "-y",
            "-i",
            "file:with%20space.mp4",
            "-c",
            "copy",
            "-f",
            "mp4",
            "file:-",
        ])
        .output()
        .unwrap();
    assert_eq!(
        r.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert!(r.stdout.is_empty());
    assert!(dir.join("-").exists());
}

#[test]
fn ignore_unknown_does_not_discard_an_unsupported_selected_codec() {
    let dir = tmp("ignore-unknown-selected");
    let input = dir.join("unknown.mp4");
    std::fs::write(&input, common::av_movie(b"xxxx")).unwrap();
    let output = dir.join("out.mp4");
    std::fs::write(&output, b"keep existing output").unwrap();
    let r = coreav(
        &[
            "--strict-safe",
            "ffmpeg",
            "-y",
            "-i",
            input.to_str().unwrap(),
            "-map",
            "0",
            "-ignore_unknown",
            "-c",
            "copy",
            output.to_str().unwrap(),
        ],
        &[stand_in()],
        &[],
    );
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert_eq!(std::fs::read(&output).unwrap(), b"keep existing output");
}

#[test]
fn documented_log_flags_and_numeric_levels_are_not_usage_errors() {
    let dir = tmp("log-level-syntax");
    let input = movie(&dir);
    for level in [
        "repeat+info",
        "level+warning",
        "+level",
        "repeat",
        "repeat+level+debug",
        "-repeat+info",
        "time+datetime+info",
        "repeat+-8",
        "+32",
        "-8",
        "2147483648",
        "++32",
        " 32",
        "repeatinfo",
        "levelrepeat",
        "repeat+",
        "+",
        "",
    ] {
        for tool in ["ffprobe", "ffmpeg"] {
            let output = dir.join("out.mp4");
            let args = if tool == "ffprobe" {
                vec!["--strict-safe", tool, "-v", level, "-show_streams", &input]
            } else {
                vec![
                    "--strict-safe",
                    tool,
                    "-y",
                    "-loglevel",
                    level,
                    "-i",
                    &input,
                    "-c",
                    "copy",
                    output.to_str().unwrap(),
                ]
            };
            let r = coreav(&args, &[stand_in()], &[]);
            assert_eq!(r.code, 0, "{tool} {level}: {}", r.stderr);
        }
    }
    for level in [
        "info+repeat",
        "repeat+bogus",
        "--8",
        "repeat++",
        "32foo",
        "32 ",
    ] {
        let r = coreav(
            &["ffprobe", "-v", level, "-show_streams", &input],
            &[stand_in()],
            &[],
        );
        assert_eq!(r.code, 2, "{level}: {}", r.stderr);
        assert!(!r.stderr.contains("ran "));
    }
}

#[test]
fn values_outside_the_native_numeric_grammar_reach_fallback() {
    let dir = tmp("numeric-fallback");
    let input = movie(&dir);
    for (option, value) in [
        ("-threads", "1+1"),
        ("-max_muxing_queue_size", "1k"),
        ("-frames:v", "-1"),
        ("-ac", "2.0"),
        ("-t", "+1"),
        ("-t", "1.s"),
        ("-t", "1.1234567891ms"),
        ("-segment_time", "+1"),
        ("-map", "[filtered]"),
        ("-t", "00:00:01ms"),
        ("-t", "00:00:01us"),
        ("-t", "1:00:00s"),
        ("-t", "00:00:01.5ms"),
        ("-t", " 1"),
        ("-ss", "\t1"),
        ("-to", "00:00:01ms"),
        ("-r", "30-1"),
        ("-r", "30/2-1"),
        ("-r", "PI"),
        ("-r", "E"),
        ("-r", "PHI"),
        ("-r", "1/2/3"),
        ("-r", "1k"),
        ("-r", " 30"),
        ("-r", "30 "),
        ("-r", "0x1e"),
        ("-map", " 0"),
        ("-map", "\t0"),
        ("-map", "0x0"),
        ("-update", "2"),
        ("-update", "-1"),
        ("-segment_time", "0"),
        ("-segment_time", "0.0"),
        ("-segment_time", " 1"),
        ("-segment_time", "00:00:01ms"),
    ] {
        let output = dir.join("out.mp4");
        let args = [
            "ffmpeg",
            "-i",
            &input,
            option,
            value,
            "-c",
            "copy",
            output.to_str().unwrap(),
        ];
        for strict in [false, true] {
            let env = if strict {
                vec![("COREAV_STRICT_SAFE", "1")]
            } else {
                vec![]
            };
            let r = coreav(&args, &[stand_in()], &env);
            assert_eq!(
                r.code,
                if strict { 4 } else { 7 },
                "{option} {value}: {}",
                r.stderr
            );
            if !strict {
                assert!(
                    r.stdout.contains(&format!("{option} {value}")),
                    "{}",
                    r.stdout
                );
            }
            assert!(!output.exists());
        }
    }
}

#[cfg(feature = "transcode")]
#[test]
fn audio_encoder_value_limits_are_not_usage_errors() {
    let dir = tmp("audio-value-fallback");
    let input = dir.join("in.mp4");
    std::fs::write(
        &input,
        av_movie_with(
            common::lossless_entry(b"fLaC", common::dfla(48_000, 16)),
            false,
        ),
    )
    .unwrap();
    let output = dir.join("out.mp3");
    for (option, value) in [
        ("-b:a", "128000+0"),
        ("-b:a", "128k+32k"),
        ("-b:a", " 128k"),
        ("-q:a", "10"),
        ("-q:a", "-1"),
        ("-q:a", "NaN"),
        ("-q:a", "inf"),
    ] {
        let args = [
            "ffmpeg",
            "-i",
            input.to_str().unwrap(),
            "-vn",
            "-c:a",
            "libmp3lame",
            option,
            value,
            output.to_str().unwrap(),
        ];
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{option} {value}: {}", r.stderr);
        assert!(r.stdout.contains(&format!("{option} {value}")));
        assert!(!output.exists());
    }
}

#[test]
fn alternate_base_map_indexes_are_not_decimal_input_indexes() {
    let dir = tmp("map-base-fallback");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let mut args = vec!["ffmpeg"];
    for _ in 0..9 {
        args.extend(["-i", input.as_str()]);
    }
    args.extend(["-map", "010", "-c", "copy", output.to_str().unwrap()]);
    for strict in [false, true] {
        let env = if strict {
            vec![("COREAV_STRICT_SAFE", "1")]
        } else {
            vec![]
        };
        let r = coreav(&args, &[stand_in()], &env);
        assert_eq!(r.code, if strict { 4 } else { 7 }, "{}", r.stderr);
        if !strict {
            assert!(r.stdout.contains("-map 010"));
        }
        assert!(!output.exists());
    }
}

#[cfg(feature = "transcode")]
#[test]
fn encoder_preset_names_outside_native_set_reach_fallback() {
    let dir = tmp("preset-value-fallback");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    for preset in ["FAST", "Fast"] {
        let r = coreav(
            &[
                "ffmpeg",
                "-i",
                &input,
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                preset,
                output.to_str().unwrap(),
            ],
            &[stand_in()],
            &[],
        );
        assert_eq!(r.code, 7, "{preset}: {}", r.stderr);
        assert!(r.stdout.contains(&format!("-preset {preset}")));
        assert!(!output.exists());
    }
}

#[test]
fn unsupported_audio_containers_fall_back_without_changing_arguments() {
    let dir = tmp("audio-container-fallback");
    for (name, bytes, format) in [
        ("audio.mp3", b"ID3\x04\0\0\0\0\0\0".as_slice(), "MP3"),
        ("big-endian.bin", b"RIFX\0\0\0\x24WAVE".as_slice(), "RIFX"),
        ("audio.ogg", b"OggS\0\0\0\0\0\0\0\0".as_slice(), "Ogg"),
        ("audio.bin", b"\xff\xfb\x90\x00".as_slice(), "MP3"),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let input = format!("file:{}", path.display());
        let output = dir.join("out.mp4");
        for args in [
            vec![
                "ffprobe",
                "-v",
                "error",
                "-show_format",
                "-show_streams",
                "-of",
                "json",
                &input,
            ],
            vec![
                "ffmpeg",
                "-i",
                &input,
                "-c",
                "copy",
                output.to_str().unwrap(),
            ],
        ] {
            let r = coreav(&args, &[stand_in()], &[]);
            assert_eq!(r.code, 7, "{name}: {}", r.stderr);
            assert!(r.stdout.contains(&input), "{}", r.stdout);
            let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
            assert_eq!(r.code, 4, "{name}: {}", r.stderr);
            assert!(r.stderr.contains(format), "{}", r.stderr);
            assert!(!r.stderr.contains("Invalid data"));
            assert!(!output.exists());
        }
    }
    let path = dir.join("broken.mp3");
    std::fs::write(&path, b"not a media container").unwrap();
    let r = coreav(
        &["ffprobe", "-show_streams", path.to_str().unwrap()],
        &[stand_in()],
        &[],
    );
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("Invalid data"));
}

#[test]
fn native_audio_reports_do_not_invoke_fallback_but_listings_still_do() {
    let dir = tmp("native-audio-fallback-boundary");
    for (name, bytes, format) in [
        ("audio.wav", standalone_audio::wave(), "wav"),
        ("audio.flac", standalone_audio::flac(), "flac"),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let input = format!("file:{}", path.display());
        let report = coreav(
            &[
                "ffprobe",
                "-show_format",
                "-show_streams",
                "-of",
                "json",
                &input,
            ],
            &[stand_in()],
            &[("COREAV_STRICT_SAFE", "1")],
        );
        assert_eq!(report.code, 0, "{}", report.stderr);
        let report: Value = serde_json::from_str(&report.stdout).unwrap();
        assert_eq!(report["format"]["format_name"], format);
        for option in [
            "-show_packets",
            "-show_frames",
            "-count_packets",
            "-count_frames",
        ] {
            let args = ["ffprobe", option, "-show_streams", "-of", "json", &input];
            let fallback = coreav(&args, &[stand_in()], &[]);
            assert_eq!(fallback.code, 7, "{option}: {}", fallback.stderr);
            assert_eq!(
                fallback.stdout,
                format!("args: -hide_banner {option} -show_streams -of json {input}\n")
            );
            let refused = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
            assert_eq!(refused.code, 4, "{option}: {}", refused.stderr);
            assert!(refused.stdout.is_empty());
        }
        let output = dir.join("out.mp4");
        let args = [
            "ffmpeg",
            "-i",
            &input,
            "-c",
            "copy",
            output.to_str().unwrap(),
        ];
        let fallback = coreav(&args, &[stand_in()], &[]);
        assert_eq!(fallback.code, 7, "{}", fallback.stderr);
        assert!(fallback.stdout.contains(&input));
        let refused = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(refused.code, 4, "{}", refused.stderr);
        assert!(!output.exists());
    }
}

#[test]
fn malformed_native_audio_fails_without_running_fallback() {
    let dir = tmp("malformed-native-audio");
    for (name, bytes) in [
        ("audio.flac", b"fLaC\0\0\0\x22".as_slice()),
        ("audio.wav", b"RIFF\x24\0\0\0WAVE".as_slice()),
        ("large.wav", b"RF64\xff\xff\xff\xffWAVE".as_slice()),
        ("broadcast.wav", b"BW64\xff\xff\xff\xffWAVE".as_slice()),
    ] {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let result = coreav(
            &["ffprobe", "-show_streams", path.to_str().unwrap()],
            &[stand_in()],
            &[],
        );
        assert_eq!(result.code, 1, "{}", result.stderr);
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.contains("ran "));
    }
}

#[test]
fn unsupported_summary_fallback_has_no_section_hint_or_partial_output() {
    let dir = tmp("unsupported-summary-notice");
    let input = movie(&dir);
    for arguments in [vec!["ffprobe", &input], vec!["ffprobe", "-i", &input]] {
        let r = coreav(&arguments, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert_eq!(
            r.stdout,
            format!("args: -hide_banner {}\n", arguments[1..].join(" "))
        );
        let notice = r.stderr.lines().next().unwrap();
        assert!(notice.ends_with("(not native: input summary)"), "{notice}");
        assert!(!notice.contains("-show_format -show_streams"), "{notice}");
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
        let r = coreav(&arguments, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(r.code, 4, "{}", r.stderr);
        assert!(r.stdout.is_empty(), "{}", r.stdout);
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
    }
    let r = coreav(
        &["ffprobe", "-show_format", "-show_streams", &input],
        &[stand_in()],
        &[("COREAV_STRICT_SAFE", "1")],
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stderr.is_empty(), "{}", r.stderr);
    assert!(r.stdout.contains("[STREAM]"), "{}", r.stdout);
}

#[test]
fn file_url_summaries_preserve_the_original_fallback_arguments() {
    let dir = tmp("summary-file-url");
    let input = format!("file:{}", movie(&dir));
    for args in [
        vec!["ffprobe", &input],
        vec!["ffprobe", "-i", &input],
        vec!["ffmpeg", "-i", &input],
    ] {
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert_eq!(
            r.stdout,
            format!("args: -hide_banner {}\n", args[1..].join(" "))
        );
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
        let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(r.code, 4, "{}", r.stderr);
        assert!(r.stdout.is_empty());
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
    }
}

#[test]
fn non_iso_summary_analysis_errors_use_atomic_fallback() {
    let dir = tmp("summary-non-iso-analysis");
    let large_name = mkv::string(0x536E, &"a".repeat((1 << 20) + 1));
    let track = mkv::audio_track(1, "A_PCM/INT/LIT", 48_000.0, 1, &large_name);
    let matroska = mkv::file(
        "matroska",
        &[
            mkv::info(Some(1.0)),
            mkv::el(mkv::TRACKS, &track),
            mkv::cluster(0, &[mkv::simple_block(1, 0, true, &[0; 96])]),
        ],
    );
    let mut transport = ts::Mux::default();
    for _ in 0..8 {
        transport.packet(0x1fff, false, &[0xff; 184]);
    }
    for (name, bytes) in [
        ("large-track.mkv", matroska),
        ("no-programs.ts", transport.out),
    ] {
        let mut source = coreav_isobmff::SliceSource::new(&bytes);
        let probed = coreav_cli::ffprobe::probe(&mut source, bytes.len() as u64, name, None);
        assert!(matches!(
            (name, probed),
            (
                "large-track.mkv",
                Err(coreav_cli::error::Failure::Matroska(
                    coreav_mkv::Error::LimitExceeded { .. }
                ))
            ) | (
                "no-programs.ts",
                Err(coreav_cli::error::Failure::Mpegts(
                    coreav_mpegts::Error::NoPrograms
                ))
            )
        ));
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let input = path.to_string_lossy();
        for args in [
            vec!["ffprobe", &input],
            vec!["ffprobe", "-i", &input],
            vec!["ffmpeg", "-i", &input],
        ] {
            let r = coreav(&args, &[stand_in()], &[]);
            assert_eq!(r.code, 7, "{name}: {}", r.stderr);
            assert_eq!(
                r.stdout,
                format!("args: -hide_banner {}\n", args[1..].join(" "))
            );
            assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
            let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
            assert_eq!(r.code, 4, "{name}: {}", r.stderr);
            assert!(r.stdout.is_empty());
            assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
        }
    }
}

#[test]
fn unqualified_container_analysis_falls_back_without_a_partial_summary() {
    let dir = tmp("summary-analysis-failure");
    let input = dir.join("truncated.mp4");
    std::fs::write(&input, b"\0\0\0\x20ftypisom").unwrap();
    let input = input.to_string_lossy();
    for args in [
        vec!["ffprobe", &input],
        vec!["ffprobe", "-i", &input],
        vec!["ffmpeg", "-i", &input],
    ] {
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert_eq!(
            r.stdout,
            format!("args: -hide_banner {}\n", args[1..].join(" "))
        );
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
        let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(r.code, 4, "{}", r.stderr);
        assert!(r.stdout.is_empty());
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
    }
    let missing = dir.join("missing.mp4");
    let input = missing.to_string_lossy();
    for args in [
        vec!["ffprobe", &input],
        vec!["ffprobe", "-i", &input],
        vec!["ffmpeg", "-i", &input],
    ] {
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert_eq!(
            r.stdout,
            format!("args: -hide_banner {}\n", args[1..].join(" "))
        );
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
        let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(r.code, 4, "{}", r.stderr);
        assert!(r.stdout.is_empty());
        assert!(!r.stderr.contains("Input #"), "{}", r.stderr);
    }
}

#[test]
fn changed_probe_budgets_and_thread_counts_still_use_fallback() {
    let dir = tmp("probe-tuning-fallback");
    let input = movie(&dir);
    for (option, value) in [
        ("-probesize", "32"),
        ("-probesize", "1G"),
        ("-analyzeduration", "1"),
        ("-analyzeduration", "200M"),
        ("-threads", "0"),
        ("-threads", "2"),
    ] {
        let args = ["ffprobe", option, value, "-show_streams", &input];
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{option} {value}: {}", r.stderr);
        assert!(r.stdout.contains(&format!("{option} {value}")));
        let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(r.code, 4, "{option} {value}: {}", r.stderr);
    }
}

#[test]
fn probe_value_syntax_outside_native_parsers_is_not_a_usage_error() {
    let dir = tmp("probe-value-fallback");
    let input = movie(&dir);
    for (option, value) in [
        ("-of", "default=nokey=yes"),
        ("-of", "csv=s=\\|"),
        ("-of", "csv=s=\\t"),
        ("-of", "csv=s='|'"),
        ("-of", "flat=s=\\."),
        ("-of", "compact=e='csv'"),
        ("-of", "compact=e=\\c"),
        ("-read_intervals", "%+1s"),
    ] {
        let args = ["ffprobe", option, value, "-show_streams", &input];
        let r = coreav(&args, &[stand_in()], &[]);
        assert_eq!(r.code, 7, "{option} {value}: {}", r.stderr);
        assert!(r.stdout.contains(&format!("{option} {value}")));
        let r = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "1")]);
        assert_eq!(r.code, 4, "{option} {value}: {}", r.stderr);
    }
}

#[cfg(not(feature = "net"))]
#[test]
fn url_inputs_without_networking_are_usage_errors() {
    for args in [
        vec!["ffprobe", "https://example.invalid/in.mp4"],
        vec!["ffprobe", "-show_streams", "https://example.invalid/in.mp4"],
        vec![
            "ffprobe",
            "-of",
            "default=nokey=yes",
            "https://example.invalid/in.mp4",
        ],
        vec![
            "ffprobe",
            "https://example.invalid/in.mp4",
            "-of",
            "default=nokey=yes",
        ],
        vec![
            "ffmpeg",
            "-vf",
            "null",
            "-i",
            "https://example.invalid/in.mp4",
            "out.mp4",
        ],
        vec![
            "ffmpeg",
            "-i",
            "https://example.invalid/in.mp4",
            "-vf",
            "null",
            "out.mp4",
        ],
        vec![
            "ffmpeg",
            "-i",
            "https://example.invalid/in.mp4",
            "-c",
            "copy",
            "out.mp4",
        ],
    ] {
        for strict in [false, true] {
            let env = if strict {
                vec![("COREAV_STRICT_SAFE", "1")]
            } else {
                vec![]
            };
            let r = coreav(&args, &[stand_in()], &env);
            assert_eq!(r.code, 2, "{args:?}: {}", r.stderr);
            assert!(r.stderr.contains("network"), "{}", r.stderr);
            assert!(!r.stderr.contains("ran "));
            assert!(!r.stderr.contains("No such file"));
            assert!(r.stdout.is_empty());
        }
    }
}

#[cfg(not(feature = "net"))]
#[test]
fn url_option_values_are_not_mistaken_for_url_inputs() {
    for tool in ["ffprobe", "ffmpeg"] {
        let r = coreav(
            &[
                tool,
                "-user_agent",
                "https://example.invalid/agent",
                "in.mp4",
            ],
            &[stand_in()],
            &[],
        );
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert!(r.stdout.contains("https://example.invalid/agent"));
    }
}

#[test]
fn frame_metadata_refusals_reach_fallback_before_output() {
    let dir = tmp("frame-metadata");
    let input = movie(&dir);
    for name in ["out.png", "out.bmp", "out.yuv", "out.y4m"] {
        let output = dir.join(name);
        for mapping in ["-1", "0:s:0", "0:s:999"] {
            let r = coreav(
                &[
                    "ffmpeg",
                    "-i",
                    &input,
                    "-map_metadata",
                    mapping,
                    "-frames:v",
                    "1",
                    output.to_str().unwrap(),
                ],
                &[stand_in()],
                &[],
            );
            assert_eq!(r.code, 7, "{name} {mapping}: {}", r.stderr);
            assert!(r.stderr.contains("-map_metadata"), "{}", r.stderr);
            assert!(
                r.stdout.contains(&format!("-map_metadata {mapping}")),
                "{}",
                r.stdout
            );
            assert!(!output.exists());
        }
    }
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 1);
}

fn log_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_command_that_is_not_native_runs_the_program_coreav_fallback_names() {
    let dir = stand_in().to_str().unwrap();
    for named in [dir.to_owned(), format!("{dir}/ffmpeg")] {
        let r = run(
            Path::new(bin()),
            &["ffmpeg", "-i", "-", "-af", "volume=2", "-f", "mpegts", "-"],
            &[],
            &[("COREAV_FALLBACK", &named)],
            "piped bytes",
        );
        assert_eq!(r.code, 7, "{named}: {}", r.stderr);
        assert_eq!(
            r.stdout, "args: -hide_banner -i - -af volume=2 -f mpegts -\npiped bytes",
            "{named}"
        );
        assert_eq!(
            r.stderr,
            format!("coreav: ran {dir}/ffmpeg instead (not native: -af)\nguard=1\n"),
            "{named}"
        );
    }
}

#[test]
fn the_first_program_on_path_that_is_not_coreav_runs() {
    let links = tmp("path-links");
    for tool in ["ffmpeg", "ffprobe"] {
        symlink(bin(), links.join(tool)).unwrap();
    }
    let hard = tmp("path-hard-link");
    std::fs::hard_link(bin(), hard.join("ffprobe")).unwrap();
    let path: &[&Path] = &[&links, &hard, stand_in()];
    let expected = format!("coreav: ran {}/ffprobe instead", stand_in().display());
    let direct = coreav(&["ffprobe", "-show_data", "in.mp4"], path, &[]);
    let multicall = run(
        &links.join("ffprobe"),
        &["-show_data", "in.mp4"],
        path,
        &[],
        "",
    );
    for r in [direct, multicall] {
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert_eq!(r.stdout, "args: -hide_banner -show_data in.mp4\n");
        assert!(
            r.stderr
                .starts_with(&format!("{expected} (not native: -show_data)\n")),
            "{}",
            r.stderr
        );
    }
}

#[test]
fn a_relative_coreav_fallback_is_the_file_in_the_working_directory() {
    let decoy = tmp("relative-decoy");
    symlink("/bin/false", decoy.join("ffmpeg")).unwrap();
    let out = Command::new(bin())
        .args(["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"])
        .current_dir(stand_in())
        .env_clear()
        .env("PATH", &decoy)
        .env("COREAV_FALLBACK", "ffmpeg")
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(7), "{stderr}");
    let expected = format!("coreav: ran {}/ffmpeg instead", stand_in().display());
    assert!(stderr.starts_with(&expected), "{stderr}");
}

#[test]
fn an_empty_path_element_is_the_working_directory_after_coreav() {
    let links = tmp("empty-path-element");
    symlink(bin(), links.join("ffmpeg")).unwrap();
    let out = Command::new(bin())
        .args(["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"])
        .current_dir(stand_in())
        .env_clear()
        .env("PATH", format!("{}:", links.display()))
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(7), "{stderr}");
    let expected = format!("coreav: ran {}/ffmpeg instead", stand_in().display());
    assert!(stderr.starts_with(&expected), "{stderr}");
}

#[test]
fn a_program_on_path_this_process_cannot_run_is_skipped() {
    let bad = tmp("not-executable-by-owner");
    let program = bad.join("ffmpeg");
    std::fs::write(&program, "#!/bin/sh\nexit 9\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o654)).unwrap();
    if std::fs::metadata(&program).unwrap().uid() == 0 {
        return;
    }
    let r = coreav(
        &["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"],
        &[&bad, stand_in()],
        &[],
    );
    assert_eq!(r.code, 7, "{}", r.stderr);
    let expected = format!("coreav: ran {}/ffmpeg instead", stand_in().display());
    assert!(r.stderr.starts_with(&expected), "{}", r.stderr);
}

#[test]
fn a_program_that_cannot_start_is_logged_as_not_run() {
    let dir = tmp("cannot-start");
    let program = dir.join("ffmpeg");
    std::fs::write(&program, "#!/nonexistent/interpreter\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = dir.join("fallback.log");
    let r = coreav(
        &["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"],
        &[],
        &[
            ("COREAV_FALLBACK", dir.to_str().unwrap()),
            ("COREAV_FALLBACK_LOG", log.to_str().unwrap()),
        ],
    );
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(
        r.stderr
            .contains(&format!("could not run {}", program.display())),
        "{}",
        r.stderr
    );
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["ran"], program.to_str().unwrap());
    assert_eq!(lines[1]["ran"], Value::Null);
    assert_eq!(lines[1]["not_run"], "fallback_unusable");
}

#[test]
fn without_a_program_the_command_is_refused_and_says_why() {
    let links = tmp("missing");
    symlink(bin(), links.join("ffmpeg")).unwrap();
    let r = coreav(
        &["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"],
        &[&links],
        &[],
    );
    assert_eq!(r.code, 4);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.contains("'-vf' is not supported"), "{}", r.stderr);
    assert!(
        r.stderr.contains("no other ffmpeg on PATH to run instead"),
        "{}",
        r.stderr
    );
}

#[test]
fn coreav_fallback_that_names_nothing_usable_is_never_replaced_by_path() {
    let dir = stand_in().to_str().unwrap();
    let empty = tmp("unusable");
    let missing = format!("{dir}/nothing/ffmpeg");
    let other_tool = format!("{dir}/ffprobe");
    let no_program = empty.to_str().unwrap().to_owned();
    for (named, why) in [
        (missing.as_str(), "which does not exist"),
        (other_tool.as_str(), "names no ffmpeg program"),
        (no_program.as_str(), "which does not exist"),
    ] {
        let r = coreav(
            &[
                "ffmpeg",
                "-i",
                "in.mp4",
                "-c:v",
                "unsupported-test-codec",
                "out.mkv",
            ],
            &[stand_in()],
            &[("COREAV_FALLBACK", named)],
        );
        assert_eq!(r.code, 4, "{named}: {}", r.stderr);
        assert!(r.stdout.is_empty());
        assert!(
            r.stderr.contains(&format!("COREAV_FALLBACK={named}")) && r.stderr.contains(why),
            "{named}: {}",
            r.stderr
        );
    }
}

#[test]
fn strict_safe_mode_never_runs_another_program() {
    let dir = stand_in().to_str().unwrap();
    let args = ["ffprobe", "-show_data", "in.mp4"];
    for (flag, value) in [(false, "1"), (false, "yes"), (true, "")] {
        let mut all: Vec<&str> = Vec::new();
        if flag {
            all.push("--strict-safe");
        }
        all.extend(args);
        let r = coreav(
            &all,
            &[stand_in()],
            &[("COREAV_FALLBACK", dir), ("COREAV_STRICT_SAFE", value)],
        );
        assert_eq!(r.code, 4, "{all:?} {value}: {}", r.stderr);
        assert!(r.stdout.is_empty());
        assert_eq!(
            r.stderr,
            "coreav ffprobe: option '-show_data' is not supported: packet payloads are not printed\n"
        );
    }
    let off = coreav(&args, &[stand_in()], &[("COREAV_STRICT_SAFE", "0")]);
    assert_eq!(off.code, 7, "{}", off.stderr);
}

#[test]
fn coreav_ffmpeg_help_runs_no_program() {
    let dir = stand_in().to_str().unwrap();
    let log = tmp("help-log").join("fallback.log");
    let log = log.to_str().unwrap();
    for strict_safe in ["0", "1"] {
        for help in ["-h", "-?", "-help", "--help"] {
            let r = coreav(
                &["ffmpeg", help],
                &[stand_in()],
                &[
                    ("COREAV_FALLBACK", dir),
                    ("COREAV_FALLBACK_LOG", log),
                    ("COREAV_STRICT_SAFE", strict_safe),
                ],
            );
            assert_eq!(r.code, 0, "{help} {strict_safe}: {}", r.stderr);
            assert_eq!(r.stdout, coreav_cli::ffmpeg::USAGE, "{help} {strict_safe}");
            assert!(r.stderr.is_empty(), "{help} {strict_safe}: {}", r.stderr);
        }
    }
    assert!(!Path::new(log).exists(), "help wrote a fallback log line");
}

#[test]
fn coreav_ffmpeg_help_on_a_topic_is_ffmpegs() {
    let dir = stand_in().to_str().unwrap();
    let args = ["ffmpeg", "-h", "encoder=libx264"];
    let r = coreav(&args, &[stand_in()], &[("COREAV_FALLBACK", dir)]);
    assert_eq!(r.code, 7, "{}", r.stderr);
    assert!(
        r.stdout
            .starts_with("args: -hide_banner -h encoder=libx264\n"),
        "{}",
        r.stdout
    );
    let r = coreav(
        &args,
        &[stand_in()],
        &[("COREAV_FALLBACK", dir), ("COREAV_STRICT_SAFE", "1")],
    );
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(r.stdout.is_empty());
    assert!(
        r.stderr
            .starts_with("coreav ffmpeg: '-h encoder=libx264' is not supported: "),
        "{}",
        r.stderr
    );
}

#[test]
fn the_guard_is_set_by_any_value() {
    let dir = stand_in().to_str().unwrap();
    for value in ["", "0", "1"] {
        let r = coreav(
            &["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"],
            &[],
            &[("COREAV_FALLBACK", dir), ("COREAV_FALLBACK_ACTIVE", value)],
        );
        assert_eq!(r.code, 4, "{value:?}: {}", r.stderr);
        assert!(r.stdout.is_empty(), "{value:?}");
        assert!(
            r.stderr.contains("led back to CoreAV"),
            "{value:?}: {}",
            r.stderr
        );
    }
}

#[test]
fn a_fallback_that_leads_back_to_coreav_is_stopped_by_the_guard() {
    let links = tmp("guard");
    symlink(bin(), links.join("ffmpeg")).unwrap();
    let log = links.join("fallback.log");
    let r = coreav(
        &["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"],
        &[],
        &[
            ("COREAV_FALLBACK", links.to_str().unwrap()),
            ("COREAV_FALLBACK_LOG", log.to_str().unwrap()),
        ],
    );
    assert_eq!(r.code, 4, "{}", r.stderr);
    assert!(r.stdout.is_empty());
    assert!(
        r.stderr
            .contains("the fallback to ffmpeg led back to CoreAV (COREAV_FALLBACK_ACTIVE is set)"),
        "{}",
        r.stderr
    );
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["ran"], format!("{}/ffmpeg", links.display()));
    assert_eq!(lines[1]["ran"], Value::Null);
    assert_eq!(lines[1]["not_run"], "recursion_guard");
}

#[test]
fn the_notice_is_printed_at_every_log_level() {
    let dir = stand_in().to_str().unwrap();
    for level in ["-v", "-loglevel"] {
        for value in ["quiet", "quiet+repeat", "-8", "error"] {
            let r = coreav(
                &["ffprobe", level, value, "-show_data", "in.mp4"],
                &[],
                &[("COREAV_FALLBACK", dir)],
            );
            assert_eq!(r.code, 7);
            assert!(
                r.stderr.starts_with(&format!(
                    "coreav: ran {dir}/ffprobe instead (not native: -show_data)\n"
                )),
                "{level} {value}: {}",
                r.stderr
            );
        }
    }
}

#[test]
fn the_log_gets_one_redacted_line_per_command_that_is_not_native() {
    let dir = stand_in().to_str().unwrap();
    let work = tmp("log");
    let log = work.join("fallback.log");
    let log = log.to_str().unwrap();
    let input = format!("{}/MARKER-in.mp4", work.display());
    let source = if cfg!(feature = "net") {
        "https://example.com/a.mp4?token=MARKER"
    } else {
        input.as_str()
    };
    let commands: [&[&str]; 3] = [
        &[
            "ffmpeg",
            "-headers",
            "Authorization: Bearer MARKER",
            "-i",
            source,
            "-metadata",
            "title=MARKER",
            "-map",
            "0:m:comment:MARKER",
            "-c:v",
            "copy",
            "/home/MARKER/out.mp4",
        ],
        &[
            "ffprobe",
            "-v",
            "quiet",
            "-show_entries",
            "format_tags=MARKER:stream_group=MARKER",
            "-of",
            "json",
            &input,
        ],
        &[
            "ffprobe",
            "-select_streams",
            "MARKER",
            "-show_streams",
            &input,
        ],
    ];
    for args in commands {
        assert_eq!(
            coreav(
                args,
                &[],
                &[("COREAV_FALLBACK", dir), ("COREAV_FALLBACK_LOG", log)]
            )
            .code,
            7
        );
    }
    let refused = coreav(
        &["ffprobe", "-show_data", &input],
        &[],
        &[("COREAV_STRICT_SAFE", "1"), ("COREAV_FALLBACK_LOG", log)],
    );
    assert_eq!(refused.code, 4);
    let text = std::fs::read_to_string(log).unwrap();
    assert!(!text.contains("MARKER"), "{text}");
    let lines = log_lines(Path::new(log));
    assert_eq!(lines.len(), 4);
    assert_eq!(
        lines[0]["args"],
        serde_json::json!([
            "-headers",
            "VALUE",
            "-i",
            "INPUT",
            "-metadata",
            "VALUE",
            "-map",
            "VALUE",
            "-c:v",
            "copy",
            "OUTPUT"
        ])
    );
    assert_eq!(lines[0]["tool"], "ffmpeg");
    assert_eq!(lines[0]["reason"], "-headers");
    assert_eq!(lines[0]["ran"], format!("{dir}/ffmpeg"));
    assert_eq!(lines[1]["reason"], "-show_entries stream_group");
    assert_eq!(
        lines[1]["args"],
        serde_json::json!([
            "-v",
            "quiet",
            "-show_entries",
            "format_tags=KEY:stream_group=KEY",
            "-of",
            "json",
            "INPUT"
        ])
    );
    assert_eq!(lines[2]["reason"], "-select_streams VALUE");
    assert_eq!(lines[3]["ran"], Value::Null);
    assert_eq!(lines[3]["not_run"], "strict_safe");
    for line in &lines {
        assert_eq!(line["coreav"], env!("CARGO_PKG_VERSION"));
        let time = line["time"].as_str().unwrap();
        assert!(time.len() == 20 && time.ends_with('Z'), "{time}");
    }
}

#[test]
fn a_log_that_cannot_be_written_costs_one_line_and_never_stops_the_command() {
    let dir = stand_in().to_str().unwrap();
    let r = coreav(
        &["ffmpeg", "-i", "in.mp4", "-vf", "x", "out.mp4"],
        &[],
        &[("COREAV_FALLBACK", dir), ("COREAV_FALLBACK_LOG", dir)],
    );
    assert_eq!(r.code, 7);
    assert_eq!(r.stderr.lines().count(), 3, "{}", r.stderr);
    assert!(
        r.stderr
            .starts_with(&format!("coreav ffmpeg: COREAV_FALLBACK_LOG={dir}: "))
    );
}

#[test]
fn native_commands_and_rejected_files_are_never_handed_on() {
    let dir = stand_in().to_str().unwrap();
    let work = tmp("native");
    let log = work.join("fallback.log");
    let env = [
        ("COREAV_FALLBACK", dir),
        ("COREAV_FALLBACK_LOG", log.to_str().unwrap()),
    ];
    let path = movie(&work);
    let native = coreav(
        &["ffprobe", "-v", "quiet", "-show_streams", &path],
        &[],
        &env,
    );
    assert_eq!(native.code, 0, "{}", native.stderr);
    assert!(native.stdout.contains("[STREAM]"));
    assert!(native.stderr.is_empty());
    let garbage = work.join("garbage.mp4");
    std::fs::write(&garbage, b"not a movie at all").unwrap();
    let rejected = coreav(
        &["ffprobe", "-show_streams", garbage.to_str().unwrap()],
        &[],
        &env,
    );
    assert_eq!(rejected.code, 1, "{}", rejected.stderr);
    let malformed = coreav(&["ffprobe", "-show_streams"], &[], &env);
    assert_eq!(malformed.code, 2);
    assert!(!log.exists());
}

#[test]
fn coreav_ffprobe_help_is_its_own_and_help_on_a_topic_is_ffprobes() {
    let dir = stand_in().to_str().unwrap();
    let work = tmp("ffprobe-help");
    let log = work.join("fallback.log");
    let env = [
        ("COREAV_FALLBACK", dir),
        ("COREAV_FALLBACK_LOG", log.to_str().unwrap()),
    ];
    for help in ["-h", "-?", "-help", "--help"] {
        let r = coreav(&["ffprobe", help], &[stand_in()], &env);
        assert_eq!(r.code, 0, "{help}: {}", r.stderr);
        assert_eq!(r.stdout, coreav_cli::ffprobe::USAGE, "{help}");
        assert!(r.stderr.is_empty(), "{help}: {}", r.stderr);
    }
    assert!(!log.exists(), "help wrote a fallback log line");
    let r = coreav(&["ffprobe", "-h", "demuxer=mov"], &[stand_in()], &env);
    assert_eq!(r.code, 7, "{}", r.stderr);
    assert!(
        r.stdout.starts_with("args: -hide_banner -h demuxer=mov\n"),
        "{}",
        r.stdout
    );
    assert_eq!(log_lines(&log).len(), 1);
}

#[test]
fn coreav_ffmpeg_runs_ffmpeg_for_what_it_refuses_once_the_input_is_read() {
    let dir = stand_in().to_str().unwrap();
    let work = tmp("ffmpeg-native");
    let log = work.join("fallback.log");
    let env = [
        ("COREAV_FALLBACK", dir),
        ("COREAV_FALLBACK_LOG", log.to_str().unwrap()),
    ];
    let input = movie(&work);
    let output = work.join("out.mp4");
    let o = output.to_str().unwrap();
    let native = coreav(&["ffmpeg", "-i", &input, "-c", "copy", o], &[], &env);
    assert_eq!(native.code, 0, "{}", native.stderr);
    assert!(native.stdout.is_empty());
    assert!(native.stderr.is_empty(), "{}", native.stderr);
    let written = std::fs::read(&output).unwrap();
    let garbage = work.join("garbage.mp4");
    std::fs::write(&garbage, b"not a movie at all").unwrap();
    let rejected = coreav(
        &[
            "ffmpeg",
            "-i",
            garbage.to_str().unwrap(),
            "-c",
            "copy",
            "-y",
            o,
        ],
        &[],
        &env,
    );
    assert_eq!(rejected.code, 1, "{}", rejected.stderr);
    let malformed = coreav(
        &["ffmpeg", "-y", "-n", "-i", &input, "-c", "copy", o],
        &[],
        &env,
    );
    assert_eq!(malformed.code, 2, "{}", malformed.stderr);
    assert!(!log.exists());
    // The output exists and neither -y nor -n is given: ffmpeg would ask.
    let ask = ["ffmpeg", "-i", &input, "-c", "copy", o];
    let ran = coreav(&ask, &[], &env);
    assert_eq!(ran.code, 7, "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        format!("args: -hide_banner -i {input} -c copy {o}\n")
    );
    let words = "asking whether to overwrite an existing output (pass -y or -n)";
    assert!(
        ran.stderr.starts_with(&format!(
            "coreav: ran {dir}/ffmpeg instead (not native: {words})\n"
        )),
        "{}",
        ran.stderr
    );
    let strict = coreav(
        &ask,
        &[],
        &[("COREAV_FALLBACK", dir), ("COREAV_STRICT_SAFE", "1")],
    );
    assert_eq!(strict.code, 4, "{}", strict.stderr);
    assert!(strict.stdout.is_empty());
    assert_eq!(
        strict.stderr,
        format!("coreav ffmpeg: not supported by coreav ffmpeg: {words}\n")
    );
    assert_eq!(std::fs::read(&output).unwrap(), written);
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["tool"], "ffmpeg");
    assert_eq!(lines[0]["reason"], words);
    assert_eq!(lines[0]["ran"], format!("{dir}/ffmpeg"));
}

#[test]
fn coreav_ffmpeg_runs_ffmpeg_for_a_name_with_a_protocol() {
    let dir = stand_in().to_str().unwrap();
    for args in [
        [
            "ffmpeg",
            "-i",
            "subfile,,start,0,end,99999,,:in.mp4",
            "-c",
            "copy",
            "out.mp4",
        ],
        ["ffmpeg", "-i", "in.mp4", "-c", "copy", ":out.mp4"],
    ] {
        let r = coreav(&args, &[], &[("COREAV_FALLBACK", dir)]);
        assert_eq!(r.code, 7, "{args:?}: {}", r.stderr);
        assert_eq!(
            r.stdout,
            format!("args: -hide_banner {}\n", args[1..].join(" "))
        );
        assert!(
            r.stderr.starts_with(&format!(
                "coreav: ran {dir}/ffmpeg instead (not native: an "
            )),
            "{}",
            r.stderr
        );
    }
}

/// Without the libopus feature a `-c:a libopus` line is not native
/// (`docs/design/compat-cli.md`, "Fallback to FFmpeg"): outside strict-safe
/// mode it runs the operator's `ffmpeg` with the same arguments, and in
/// strict-safe mode it is refused and nothing runs. Default builds only:
/// with `transcode`, strict-safe mode asks for an `x264` program first.
#[cfg(not(feature = "transcode"))]
#[test]
fn coreav_ffmpeg_hands_libopus_lines_on_without_the_libopus_feature() {
    let dir = stand_in().to_str().unwrap();
    let work = tmp("ffmpeg-libopus");
    let input = movie(&work);
    let output = work.join("out.mp4");
    let o = output.to_str().unwrap();
    let encoded = [
        "ffmpeg", "-i", &input, "-c:v", "libx264", "-c:a", "libopus", "-b:a", "48k", o,
    ];
    let copied = ["ffmpeg", "-i", &input, "-c:v", "copy", "-c:a", "libopus", o];
    for (line, words) in [
        (
            &encoded[..],
            "encoding with libopus, in a build without the libopus feature",
        ),
        (
            &copied[..],
            "encoding audio with libopus without encoding the video with libx264",
        ),
    ] {
        let ran = coreav(line, &[], &[("COREAV_FALLBACK", dir)]);
        assert_eq!(ran.code, 7, "{line:?}: {}", ran.stderr);
        assert_eq!(
            ran.stdout,
            format!("args: -hide_banner {}\n", line[1..].join(" "))
        );
        assert_eq!(
            ran.stderr,
            format!("coreav: ran {dir}/ffmpeg instead (not native: {words})\nguard=1\n")
        );
        let strict = coreav(
            line,
            &[],
            &[("COREAV_FALLBACK", dir), ("COREAV_STRICT_SAFE", "1")],
        );
        assert_eq!(strict.code, 4, "{line:?}: {}", strict.stderr);
        assert!(strict.stdout.is_empty(), "{line:?}: ran {}", strict.stdout);
        assert_eq!(
            strict.stderr,
            format!("coreav ffmpeg: not supported by coreav ffmpeg: {words}\n")
        );
        assert!(!output.exists());
    }
}

/// A `-c:a aac` line the pipeline does not take is not native
/// (`docs/design/compat-cli.md`, "Fallback to FFmpeg"): outside strict-safe
/// mode it runs the operator's `ffmpeg` with the same arguments, and in
/// strict-safe mode it is refused and nothing runs. Default builds only:
/// with `transcode`, strict-safe mode refuses every `-c:a aac` line first.
#[cfg(not(feature = "transcode"))]
#[test]
fn coreav_ffmpeg_hands_aac_lines_it_does_not_take_on() {
    let dir = stand_in().to_str().unwrap();
    let work = tmp("ffmpeg-aac");
    let input = movie(&work);
    let output = work.join("out.mp4");
    let o = output.to_str().unwrap();
    let encoded = [
        "ffmpeg", "-i", &input, "-c:v", "libx264", "-c:a", "aac", "-b:a", "96k", o,
    ];
    let copied = ["ffmpeg", "-i", &input, "-c:v", "copy", "-c:a", "aac", o];
    for (line, words) in [
        (
            &encoded[..],
            "encoding audio with aac from a codec this build does not decode",
        ),
        (
            &copied[..],
            "encoding audio with aac without encoding the video with libx264",
        ),
    ] {
        let ran = coreav(line, &[], &[("COREAV_FALLBACK", dir)]);
        assert_eq!(ran.code, 7, "{line:?}: {}", ran.stderr);
        assert_eq!(
            ran.stdout,
            format!("args: -hide_banner {}\n", line[1..].join(" "))
        );
        assert_eq!(
            ran.stderr,
            format!("coreav: ran {dir}/ffmpeg instead (not native: {words})\nguard=1\n")
        );
        let strict = coreav(
            line,
            &[],
            &[("COREAV_FALLBACK", dir), ("COREAV_STRICT_SAFE", "1")],
        );
        assert_eq!(strict.code, 4, "{line:?}: {}", strict.stderr);
        assert!(strict.stdout.is_empty(), "{line:?}: ran {}", strict.stdout);
        assert_eq!(
            strict.stderr,
            format!("coreav ffmpeg: not supported by coreav ffmpeg: {words}\n")
        );
        assert!(!output.exists());
    }
}
