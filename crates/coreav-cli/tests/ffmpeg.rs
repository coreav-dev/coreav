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

use common::{aac_entry, av_movie_with};
use coreav_cli::ffmpeg::{Reason, Refusal, Stop};
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

fn ffmpeg(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("ffmpeg")
        .args(args)
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
        (vec!["-i", i, "-c:v", "libx265", o], "libx265"),
        (
            vec!["-i", i, "-vf", "scale=320:-2", "-c:a", "copy", o],
            "-vf",
        ),
        (vec!["-f", "concat", "-i", i, "-c", "copy", o], "concat"),
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
        refusal(&["-i", "in.mp4", "-c:v", "libx265", "out.mp4"]),
        Reason::Option("-c:v".to_owned(), Some("libx265".to_owned()))
    );
    assert_eq!(
        refusal(&["-f", "concat", "-i", "list.txt", "-c", "copy", "out.mp4"]),
        Reason::Option("-f".to_owned(), Some("concat".to_owned()))
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

/// The file extensions of each container family the command set names.
const FAMILIES: [(&str, &[&str]); 2] = [
    ("mp4-mov", &["mp4", "mov", "m4a", "3gp", "m4v"]),
    ("matroska-webm", &["mkv", "webm"]),
];

/// The entries that run on no fixture, and what their refusal names: a job
/// no writer takes yet (joining from a `-f concat` list, a clip of audio
/// alone).
const NOT_NATIVE: [(&str, &str); 3] = [
    ("C02", "concat"),
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
/// `out%03d.mp4` for segments. A command a file cannot take (a `-map` that
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
                let input = fixtures.join(name);
                if stream_kinds(&input)
                    .is_some_and(|kinds| kinds.iter().any(|k| without.contains(k)))
                {
                    continue;
                }
                let work = out.join(format!("{id}.{n}.{name}"));
                std::fs::create_dir_all(&work).unwrap();
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
                    "source": input.display().to_string(),
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
