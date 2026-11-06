//! Bounded transport copy checked against the pinned black-box oracle.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

#[path = "../../../test-support/oracle.rs"]
mod test_oracle;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct FixtureDir(PathBuf);

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tools() -> Option<(PathBuf, PathBuf)> {
    let pin = test_oracle::require();
    Some((pin.ffmpeg().to_path_buf(), pin.ffprobe().to_path_buf()))
}

fn scratch(name: &str) -> FixtureDir {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ffmpeg-mpegts-{}-{name}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    FixtureDir(path)
}

fn run(command: &mut Command) -> Vec<u8> {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

fn fixture(ffmpeg: &Path, path: &Path, rate: &str, bframes: &str, edits: bool, frames: u32) {
    run(Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size=96x64:rate={rate}:duration=2"))
        .args([
            "-an",
            "-c:v",
            "libx264",
            "-threads:v",
            "1",
            "-bf",
            bframes,
            "-g",
            "15",
            "-keyint_min",
            "15",
            "-sc_threshold",
            "0",
            "-use_editlist",
            if edits { "1" } else { "0" },
        ])
        .arg("-frames:v")
        .arg(frames.to_string())
        .arg(path));
}

fn core(args: &[&str], fallback: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_coreav"));
    command.arg("ffmpeg").args(args).env_clear();
    match fallback {
        None => {
            command
                .env("COREAV_STRICT_SAFE", "1")
                .env("COREAV_FALLBACK", "off");
        }
        Some(program) => {
            command
                .env("COREAV_FALLBACK", program)
                .env("PATH", program.parent().unwrap());
        }
    }
    command.output().unwrap()
}

fn success(result: Output) {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn fields(ffprobe: &Path, path: &Path) -> Value {
    let mut value: Value = serde_json::from_slice(&run(Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-show_streams",
            "-show_programs",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)))
    .unwrap();
    for packet in value["packets"].as_array_mut().unwrap() {
        packet.as_object_mut().unwrap().remove("pos");
    }
    for key in ["filename", "size", "bit_rate", "probe_score"] {
        value["format"].as_object_mut().unwrap().remove(key);
    }
    value
}

fn decoded(ffmpeg: &Path, path: &Path) -> Vec<u8> {
    run(Command::new(ffmpeg)
        .args(["-v", "error", "-copyts", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "framemd5",
            "-",
        ]))
}

#[test]
fn automatic_and_explicit_avc_copy_keep_every_packet_clock_and_frame() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let owner = scratch("exact");
    let dir = &owner.0;
    for rate in ["24", "30", "30000/1001"] {
        for bframes in ["0", "2"] {
            let input = dir.join("in.mp4");
            fixture(
                &ffmpeg,
                &input,
                rate,
                bframes,
                false,
                if rate == "24" { 48 } else { 60 },
            );
            for explicit in [false, true] {
                let output = dir.join(if explicit { "out.bin" } else { "out.ts" });
                let reference = dir.join("reference.ts");
                let mut args = vec!["-y", "-i", input.to_str().unwrap(), "-an", "-c", "copy"];
                if explicit {
                    args.extend([
                        "-c:v",
                        "libx264",
                        "-c:v:0",
                        "copy",
                        "-map",
                        "0:v:0",
                        "-bsf:v",
                        "h264_mp4toannexb",
                        "-f",
                        "mpegts",
                    ]);
                }
                args.push(output.to_str().unwrap());
                success(core(&args, None));
                run(Command::new(&ffmpeg)
                    .args(["-nostdin", "-v", "error"])
                    .args(&args[..args.len() - 1])
                    .arg(&reference));
                let actual = fields(&ffprobe, &output);
                let expected = fields(&ffprobe, &reference);
                assert_eq!(actual, expected, "{rate}, B={bframes}, explicit={explicit}");
                assert_eq!(
                    actual["packets"].as_array().unwrap().len(),
                    if rate == "24" { 48 } else { 60 }
                );
                assert_eq!(decoded(&ffmpeg, &output), decoded(&ffmpeg, &reference));
                for key in ["data_hash", "pts", "dts", "duration"] {
                    let mut changed = expected.clone();
                    changed["packets"][17][key] = Value::Null;
                    assert_ne!(actual, changed, "interior {key} must enter comparison");
                }
                let mut changed = expected.clone();
                changed["programs"][0]["tags"]["service_name"] = Value::String("changed".into());
                assert_ne!(actual, changed);
            }
            std::fs::remove_file(input).unwrap();
        }
    }
}

#[derive(PartialEq)]
struct FileState {
    bytes: Vec<u8>,
    modified: std::time::SystemTime,
    readonly: bool,
    identity: u64,
}

fn snapshot(dir: &Path) -> std::collections::BTreeMap<PathBuf, FileState> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                metadata.ino()
            };
            #[cfg(not(unix))]
            let identity = 0;
            (
                path,
                FileState {
                    bytes,
                    modified: metadata.modified().unwrap(),
                    readonly: metadata.permissions().readonly(),
                    identity,
                },
            )
        })
        .collect()
}

#[test]
fn unqualified_transport_options_and_sources_preserve_destinations_and_fallback() {
    let Some((ffmpeg, _)) = tools() else {
        return;
    };
    let owner = scratch("refusal");
    let dir = &owner.0;
    let input = dir.join("input with spaces.mp4");
    fixture(&ffmpeg, &input, "30", "2", false, 60);
    let edited = dir.join("edited.mp4");
    fixture(&ffmpeg, &edited, "30", "2", true, 60);
    let audiovisual = dir.join("audiovisual.mp4");
    run(Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&input)
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000:duration=2",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-use_editlist",
            "0",
        ])
        .arg(&audiovisual));
    let output = dir.join("out.ts");
    let sentinel = dir.join(".out.ts.coreav-sentinel.part");
    std::fs::write(&sentinel, b"preserve neighboring staging").unwrap();
    for (source, prefix, options, name) in [
        (&input, vec![], vec!["-movflags", "+faststart"], "out.ts"),
        (
            &input,
            vec![],
            vec!["-max_muxing_queue_size", "32"],
            "out.ts",
        ),
        (&input, vec!["-fflags", "+genpts"], vec![], "out.ts"),
        (&input, vec![], vec!["-bsf:v", "null"], "out.ts"),
        (&edited, vec![], vec![], "out.ts"),
        (&audiovisual, vec![], vec![], "out.ts"),
        (&input, vec![], vec!["-f", "mpegts"], "out.m2ts"),
    ] {
        let output = dir.join(name);
        let mut args = vec!["-y", "-v", "error"];
        args.extend(prefix);
        args.extend(["-i", source.to_str().unwrap(), "-an", "-c:v", "copy"]);
        args.extend(options);
        args.push(output.to_str().unwrap());
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing destination").unwrap();
            }
            let before = snapshot(dir);
            let result = core(&args, None);
            assert_eq!(
                result.status.code(),
                Some(4),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(result.stdout.is_empty());
            assert!(snapshot(dir) == before, "refusal changed neighboring files");
        }
        let reference = dir.join(format!(
            "reference.{}",
            output.extension().unwrap().to_str().unwrap()
        ));
        run(Command::new(&ffmpeg)
            .args(["-nostdin"])
            .args(&args[..args.len() - 1])
            .arg(&reference));
        let result = core(&args, Some(&ffmpeg));
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            std::fs::read(&output).unwrap() == std::fs::read(&reference).unwrap(),
            "complete fallback bytes differ for {name}"
        );
        std::fs::remove_file(&output).unwrap();
        std::fs::remove_file(&reference).unwrap();
    }
    let mut bytes = std::fs::read(&input).unwrap();
    let limits = coreav_isobmff::Limits::default();
    let movie = coreav_isobmff::probe(
        coreav_isobmff::SliceSource::new(&bytes),
        &limits,
        &coreav_isobmff::Allowlist::default(),
    )
    .unwrap();
    let mut budget = coreav_isobmff::Budget::new(&limits);
    let last = movie
        .packets(
            0,
            coreav_isobmff::SliceSource::new(&bytes),
            &limits,
            &mut budget,
        )
        .unwrap()
        .last()
        .unwrap()
        .unwrap();
    bytes[last.offset as usize..last.offset as usize + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    let malformed = dir.join("late-malformed.mp4");
    std::fs::write(&malformed, bytes).unwrap();
    let args = [
        "-y",
        "-i",
        malformed.to_str().unwrap(),
        "-an",
        "-c",
        "copy",
        output.to_str().unwrap(),
    ];
    for existing in [false, true] {
        if existing {
            std::fs::write(&output, b"existing destination").unwrap();
        }
        let before = snapshot(dir);
        let result = core(&args, None);
        assert_eq!(
            result.status.code(),
            Some(4),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.stdout.is_empty());
        assert!(
            snapshot(dir) == before,
            "late malformed input changed neighboring files"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("ffmpeg");
        std::fs::write(&program, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 19\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let result = core(&args, Some(&program));
        assert_eq!(result.status.code(), Some(19));
        let expected: Vec<_> = std::iter::once("-hide_banner")
            .chain(args)
            .flat_map(|arg| arg.bytes().chain([0]))
            .collect();
        assert_eq!(result.stdout, expected);
    }
}

#[test]
fn short_transport_outputs_keep_the_qualified_three_sample_boundary() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let owner = scratch("short");
    let dir = &owner.0;
    let input = dir.join("input.mp4");
    let output = dir.join("output.ts");
    let reference = dir.join("reference.ts");
    for rate in ["24", "30", "30000/1001"] {
        for bframes in ["0", "2"] {
            for count in [1, 2, 3] {
                fixture(&ffmpeg, &input, rate, bframes, false, count);
                let args = [
                    "-y",
                    "-i",
                    input.to_str().unwrap(),
                    "-an",
                    "-c:v",
                    "copy",
                    output.to_str().unwrap(),
                ];
                let before = snapshot(dir);
                let result = core(&args, None);
                if count < 3 {
                    assert_eq!(
                        result.status.code(),
                        Some(4),
                        "{}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                    assert!(result.stdout.is_empty());
                    assert!(snapshot(dir) == before);
                } else {
                    success(result);
                    run(Command::new(&ffmpeg)
                        .args(["-nostdin", "-v", "error"])
                        .args(&args[..args.len() - 1])
                        .arg(&reference));
                    assert_eq!(fields(&ffprobe, &output), fields(&ffprobe, &reference));
                    assert_eq!(decoded(&ffmpeg, &output), decoded(&ffmpeg, &reference));
                }
                std::fs::remove_file(&input).unwrap();
            }
        }
    }
}
