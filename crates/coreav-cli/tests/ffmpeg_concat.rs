//! Complete native H.264 concat output and atomic refusals against the pinned oracle.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "support/concat_output.rs"]
mod concat_output;

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
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

struct Run {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
}

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

#[test]
fn concat_audio_gaps_are_refused_with_exact_pinned_fallback() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-audio-gaps");
    for (name, duration, rate) in [("short", "3", "24"), ("fractional", "3.25", "30000/1001")] {
        let edited = dir.join(format!("{name}-edited.mp4"));
        let source = dir.join(format!("{name}.mp4"));
        let generated = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("testsrc2=size=64x48:rate={rate}:duration=3.25"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency=751:sample_rate=44100:duration={duration}"
            ))
            .args([
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-g",
                "24",
                "-bf",
                "2",
                "-c:a",
                "aac",
            ])
            .arg(&edited)
            .output()
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let unedited = Command::new(&ffmpeg_bin)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&edited)
            .args(["-c", "copy", "-use_editlist", "0"])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            unedited.status.success(),
            "{}",
            String::from_utf8_lossy(&unedited.stderr)
        );
        let movie = coreav_isobmff::probe(
            coreav_isobmff::FileSource::open(&source).unwrap(),
            &Default::default(),
            &coreav_isobmff::Allowlist::listing(),
        )
        .unwrap();
        assert!(movie.tracks.iter().all(|track| track.edits.is_empty()));
        let path = std::env::join_paths(
            std::iter::once(ffmpeg_bin.parent().unwrap().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ),
        )
        .unwrap();
        for inputs in [2, 3] {
            let list = dir.join("list.txt");
            std::fs::write(&list, format!("file '{name}.mp4'\n").repeat(inputs)).unwrap();
            for (safe, options) in [
                (true, vec!["-c", "copy"]),
                (false, vec!["-c", "copy"]),
                (true, vec!["-map", "0", "-c", "copy"]),
                (true, vec!["-c", "copy", "-movflags", "+faststart"]),
            ] {
                let output = dir.join("output.mp4");
                let reference = dir.join("reference.mp4");
                let mut args = vec!["-y", "-f", "concat"];
                if safe {
                    args.extend(["-safe", "0"]);
                }
                args.extend(["-i", list.to_str().unwrap()]);
                args.extend(&options);
                args.push(output.to_str().unwrap());
                for existing in [false, true] {
                    if existing {
                        std::fs::write(&output, b"existing output").unwrap();
                    }
                    let before = std::fs::read_dir(&dir).unwrap().count();
                    let result = ffmpeg(&args);
                    assert_eq!(
                        result.code, 4,
                        "{name}, {inputs}, {options:?}: {}",
                        result.stderr
                    );
                    assert!(
                        result.stderr.contains("several audio edits"),
                        "{}",
                        result.stderr
                    );
                    assert!(result.stdout.is_empty());
                    if existing {
                        assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
                    } else {
                        assert!(!output.exists());
                    }
                    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), before);
                }
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
                let fallback = Command::new(bin())
                    .arg("ffmpeg")
                    .args(&args)
                    .env("PATH", &path)
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
                std::fs::remove_file(&output).unwrap();
            }
            let video = dir.join("video.mp4");
            let result = ffmpeg(&[
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                list.to_str().unwrap(),
                "-an",
                "-c",
                "copy",
                video.to_str().unwrap(),
            ]);
            assert_eq!(result.code, 0, "{}", result.stderr);
            assert!(result.stdout.is_empty());
            let reference = dir.join("video-reference.mp4");
            let reference_run = Command::new(&ffmpeg_bin)
                .args([
                    "-nostdin", "-v", "error", "-y", "-f", "concat", "-safe", "0",
                ])
                .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
                .arg("-i")
                .arg(&list)
                .args(["-an", "-c", "copy"])
                .arg(&reference)
                .output()
                .unwrap();
            assert!(
                reference_run.status.success(),
                "{}",
                String::from_utf8_lossy(&reference_run.stderr)
            );
            concat_output::compare(
                &ffmpeg_bin,
                &ffprobe_bin,
                &video,
                &reference,
                &list,
                &format!("gap-{name}-{inputs}-video"),
            );
            std::fs::remove_file(video).unwrap();
        }
    }
}

#[test]
fn lossless_audiovisual_concat_preserves_complete_native_output() {
    let Some((reference, probe)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-lossless");
    for codec in ["flac", "alac"] {
        let source = dir.join(format!("{codec}.mp4"));
        let mut command = Command::new(&reference);
        command
            .args(["-nostdin", "-v", "error", "-f", "lavfi"])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .args([
                "-i",
                "testsrc2=size=320x240:rate=25:duration=2",
                "-f",
                "lavfi",
            ])
            .args(include_str!("../../../scripts/oracle-audio-args.txt").split_whitespace())
            .args([
                "-i",
                "sine=frequency=440:duration=2:sample_rate=48000",
                "-c:v",
                "libx264",
                "-threads:v",
                "1",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "25",
                "-ac",
                "2",
                "-c:a",
                codec,
            ]);
        if codec == "alac" {
            command.args(["-sample_fmt", "s16p"]);
        }
        let generated = command.arg("-shortest").arg(&source).output().unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let list = dir.join("list.txt");
        std::fs::write(&list, format!("file '{codec}.mp4'\n").repeat(2)).unwrap();
        let output = dir.join("native.mp4");
        let expected = dir.join("reference.mp4");
        let args = [
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            list.to_str().unwrap(),
            "-c",
            "copy",
        ];
        let copied = Command::new(&reference)
            .args(["-nostdin", "-v", "error"])
            .args(args)
            .arg(&expected)
            .output()
            .unwrap();
        assert!(
            copied.status.success(),
            "{}",
            String::from_utf8_lossy(&copied.stderr)
        );
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let copied = ffmpeg(&[args.as_slice(), &[output.to_str().unwrap()]].concat());
            assert_eq!(copied.code, 0, "{}", copied.stderr);
            assert!(copied.stdout.is_empty());
            concat_output::compare(
                &reference,
                &probe,
                &output,
                &expected,
                &list,
                &format!("audio-copy-{codec}"),
            );
            std::fs::remove_file(&output).unwrap();
        }
        let hostile = dir.join("inband.txt");
        std::fs::write(&hostile, "file 'reference.mp4'\nfile 'reference.mp4'\n").unwrap();
        let hostile_args = [
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            hostile.to_str().unwrap(),
            "-c",
            "copy",
        ];
        let fallback_reference = dir.join("fallback-reference.mp4");
        let pin = Command::new(&reference)
            .args(["-nostdin", "-v", "error"])
            .args(hostile_args)
            .arg(&fallback_reference)
            .output()
            .unwrap();
        assert!(
            pin.status.success(),
            "{}",
            String::from_utf8_lossy(&pin.stderr)
        );
        for existing in [false, true] {
            if existing {
                std::fs::write(&output, b"existing output").unwrap();
            }
            let entries = std::fs::read_dir(&dir).unwrap().count();
            let strict = ffmpeg(&[hostile_args.as_slice(), &[output.to_str().unwrap()]].concat());
            assert_eq!(strict.code, 4, "{}", strict.stderr);
            assert!(
                strict
                    .stderr
                    .contains("AVC parameter sets carried inside avc1 samples")
            );
            assert!(strict.stdout.is_empty());
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), entries);
            if existing {
                assert_eq!(std::fs::read(&output).unwrap(), b"existing output");
            } else {
                assert!(!output.exists());
            }
            let fallback = Command::new(bin())
                .arg("ffmpeg")
                .args(hostile_args)
                .arg(&output)
                .env("COREAV_FALLBACK", &reference)
                .env_remove("COREAV_STRICT_SAFE")
                .env_remove("COREAV_FALLBACK_ACTIVE")
                .output()
                .unwrap();
            assert_eq!(fallback.status.code(), pin.status.code());
            assert_eq!(fallback.stdout, pin.stdout);
            assert_eq!(
                std::fs::read(&output).unwrap(),
                std::fs::read(&fallback_reference).unwrap()
            );
            std::fs::remove_file(&output).unwrap();
        }
    }
}

#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn c02_guard_concat_preserves_complete_native_output() {
    let Some((reference, probe)) = oracle() else {
        return;
    };
    let fixtures = std::env::var_os("COREAV_FIXTURES").expect("set COREAV_FIXTURES");
    let dir = oracle_scratch("ffmpeg-concat-c02-complete");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::join_paths(
        std::iter::once(reference.parent().unwrap().to_path_buf()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .unwrap();
    let prepared = Command::new("python3")
        .args(["-I", "-c", r#"
import json
import pathlib
import sys
sys.path.insert(0, str(pathlib.Path(sys.argv[1]) / 'scripts'))
import bench_guard
coreav = sys.argv[2]
fixtures = pathlib.Path(sys.argv[3])
work = pathlib.Path(sys.argv[4])
bench_guard.derive(fixtures, work)
fixture, template = bench_guard.CASES['ffmpeg-C02-join']
source = work / fixture
assert source.is_file(), 'C02 derived fixture is missing'
prepared = {}
for arg in template:
    if arg.startswith('{inputs:'):
        prepared[arg] = bench_guard.inputs_of(coreav, source, arg[len('{inputs:'):-1], work)
assert None not in prepared.values(), 'C02 input preparation failed'
output = work / 'native'
output.mkdir()
command = []
for arg in template:
    command.extend(prepared[arg] if arg in prepared else [bench_guard.argument(arg, source, None, str(output))])
print(json.dumps(command))
"#])
        .arg(&root).arg(bin()).arg(fixtures).arg(&*dir)
        .env("PATH", path)
        .env("COREAV_STRICT_SAFE", "1")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .output().unwrap();
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let command: Vec<String> = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(command[0], "ffmpeg");
    let candidate = Path::new(command.last().unwrap());
    let copied = Command::new(bin())
        .args(&command)
        .env("COREAV_STRICT_SAFE", "1")
        .env_remove("COREAV_FALLBACK_ACTIVE")
        .output()
        .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stderr)
    );
    assert!(copied.stdout.is_empty());
    let expected = dir.join("reference.mp4");
    let pin = Command::new(&reference)
        .args(["-nostdin", "-v", "error"])
        .args(&command[1..command.len() - 1])
        .arg(&expected)
        .output()
        .unwrap();
    assert!(
        pin.status.success(),
        "{}",
        String::from_utf8_lossy(&pin.stderr)
    );
    concat_output::compare(
        &reference,
        &probe,
        candidate,
        &expected,
        Path::new(&command[4]),
        "guard-C02",
    );
}

#[test]
fn unedited_audiovisual_concat_matches_the_pinned_ffmpeg() {
    let Some((ffmpeg_bin, ffprobe_bin)) = oracle() else {
        return;
    };
    let dir = oracle_scratch("ffmpeg-concat-unedited-oracle");
    let audio_input: Vec<&str> = include_str!("../../../scripts/oracle-audio-args.txt")
        .split_whitespace()
        .collect();
    let make = |args: &[&str]| {
        let mut command = Command::new(&ffmpeg_bin);
        command.args(["-nostdin", "-v", "error"]);
        for arg in args {
            if *arg == "-i" {
                command.args(&audio_input);
            }
            command.arg(arg);
        }
        let result = command.current_dir(&dir).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    make(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=96x64:rate=24:duration=3.25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=733:sample_rate=44100:duration=3.25",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-threads:v",
        "1",
        "-g",
        "24",
        "-bf",
        "2",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-ac",
        "2",
        "-b:a",
        "96k",
        "-shortest",
        "original.mp4",
    ]);
    make(&[
        "-i",
        "original.mp4",
        "-c",
        "copy",
        "-use_editlist",
        "0",
        "source.mp4",
    ]);
    let source = dir.join("source.mp4");
    let parsed = coreav_isobmff::probe(
        coreav_isobmff::FileSource::open(&source).unwrap(),
        &Default::default(),
        &coreav_isobmff::Allowlist::listing(),
    )
    .unwrap();
    assert!(parsed.tracks.iter().all(|track| track.edits.is_empty()));
    let list = dir.join("list.txt");
    std::fs::write(&list, "file source.mp4\nfile source.mp4\n").unwrap();
    let candidate = dir.join("candidate.mp4");
    let r = ffmpeg(&[
        "-f",
        "concat",
        "-i",
        list.to_str().unwrap(),
        "-c",
        "copy",
        candidate.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stdout.is_empty());
    make(&[
        "-f",
        "concat",
        "-i",
        "list.txt",
        "-c",
        "copy",
        "reference.mp4",
    ]);
    concat_output::compare(
        &ffmpeg_bin,
        &ffprobe_bin,
        &candidate,
        &dir.join("reference.mp4"),
        &list,
        "01-unedited-av",
    );
    let mut changed = std::fs::read(&candidate).unwrap();
    let bitrate = changed
        .windows(4)
        .position(|bytes| bytes == b"btrt")
        .unwrap()
        + 12;
    changed[bitrate..bitrate + 4].copy_from_slice(&12345u32.to_be_bytes());
    std::fs::write(&candidate, changed).unwrap();
    assert!(
        std::panic::catch_unwind(|| concat_output::compare(
            &ffmpeg_bin,
            &ffprobe_bin,
            &candidate,
            &dir.join("reference.mp4"),
            &list,
            "01-unedited-av",
        ))
        .is_err(),
        "changed copied btrt must fail complete-output qualification"
    );
}
