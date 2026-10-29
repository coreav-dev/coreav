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

use common::{Spec, aac_entry, av_movie_with};
use coreav_cli::fallback::Reason;
use coreav_cli::ffmpeg::{Refusal, Stop};
use coreav_cli::ffprobe::StreamKind;
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
fn commands_outside_the_vocabulary_are_refused_by_name() {
    let dir = scratch("ffmpeg-refused");
    let input = movie(&dir);
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let i = input.as_str();
    for (args, named) in [
        (vec!["-i", i, "-c:v", "libx264", o], "libx264"),
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
        (vec!["-h"], "-h"),
        (vec!["--help"], "--help"),
    ] {
        let r = ffmpeg(&args);
        assert_eq!(r.code, 4, "{args:?}: {}", r.stderr);
        assert!(r.stdout.is_empty(), "{args:?}");
        assert!(r.stderr.contains(named), "{args:?}: {}", r.stderr);
        assert!(!output.exists(), "{args:?}");
    }
}

#[test]
fn a_refusal_carries_its_reason() {
    let refusal = |list: &[&str]| {
        let args: Vec<String> = list.iter().map(|a| (*a).to_owned()).collect();
        match coreav_cli::ffmpeg::run(&args) {
            Err(Stop::Refused(Refusal::NotNative(refusal))) => refusal.reason,
            _ => panic!("{list:?} was not refused"),
        }
    };
    assert_eq!(
        refusal(&["-i", "in.mp4", "-c:v", "libx264", "out.mp4"]),
        Reason::Option("-c:v".to_owned(), Some("libx264".to_owned()))
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
fn a_concat_list_is_joined() {
    let dir = scratch("ffmpeg-concat");
    let input = movie(&dir);
    let list = dir.join("list.txt");
    std::fs::write(&list, "ffconcat version 1.0\nfile in.mp4\nfile 'in.mp4'\n").unwrap();
    let output = dir.join("out.mp4");
    let o = output.to_str().unwrap();
    let l = list.to_str().unwrap();
    let args: Vec<String> = ["-f", "concat", "-i", l, "-c", "copy", o]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let Ok(summary) = coreav_cli::ffmpeg::run(&args) else {
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
    let Ok(summary) = coreav_cli::ffmpeg::run(&args) else {
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

/// The entries that run on no fixture, and what their refusal names: a job
/// no writer takes yet (a clip of audio alone).
const NOT_NATIVE: [(&str, &str); 2] = [
    ("C15", "a clip or a segment without video"),
    ("C19", "a clip or a segment without video"),
];

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
/// natively on at least one fixture of each of its families, but those of
/// [`NOT_NATIVE`], which run on none and are refused for their reason
/// wherever the file is not rejected first; a refusal or a
/// rejection leaves no file behind. With `COREAV_FFMPEG_OUT` the outputs and
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
        let not_native = NOT_NATIVE
            .iter()
            .find(|(e, _)| *e == id)
            .map(|(_, why)| *why);
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
                let (status, detail) = match coreav_cli::ffmpeg::run(&args) {
                    Ok(summary) => {
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
                    Err(Stop::Failed(failure)) => {
                        assert!(!failure.is_io(), "{label}: {}", failure.to_json());
                        ("rejected", failure.to_json())
                    }
                };
                let files = std::fs::read_dir(&work).unwrap().count();
                if status == "written" {
                    assert!(files > 0, "{label}: no output");
                } else {
                    assert_eq!(files, 0, "{label}: {status}, but a file was left");
                }
                if let Some(why) = not_native {
                    assert!(
                        status != "written"
                            && (status != "refused"
                                || detail.as_str().is_some_and(|m| m.contains(why))),
                        "{label}: {status}: {detail}"
                    );
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
        if not_native.is_none() {
            for family in families.iter().filter(|f| present.contains(*f)) {
                assert!(
                    native.contains(&(id, *family)),
                    "{id}: no {family} fixture ran natively"
                );
            }
        }
    }
    std::fs::write(
        out.join("records.json"),
        serde_json::to_string_pretty(&Value::Array(records)).unwrap(),
    )
    .unwrap();
}
