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
            r.stdout, "args: -i - -af volume=2 -f mpegts -\npiped bytes",
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
    let direct = coreav(&["ffprobe", "-show_frames", "in.mp4"], path, &[]);
    let multicall = run(
        &links.join("ffprobe"),
        &["-show_frames", "in.mp4"],
        path,
        &[],
        "",
    );
    for r in [direct, multicall] {
        assert_eq!(r.code, 7, "{}", r.stderr);
        assert_eq!(r.stdout, "args: -show_frames in.mp4\n");
        assert!(
            r.stderr
                .starts_with(&format!("{expected} (not native: -show_frames)\n")),
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
            &["ffmpeg", "-i", "in.mp4", "out.mkv"],
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
    let args = ["ffprobe", "-show_frames", "in.mp4"];
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
            "coreav ffprobe: option '-show_frames' is not supported: frames need a decoder\n"
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
        r.stdout.starts_with("args: -h encoder=libx264\n"),
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
                &["ffprobe", level, value, "-show_frames", "in.mp4"],
                &[],
                &[("COREAV_FALLBACK", dir)],
            );
            assert_eq!(r.code, 7);
            assert!(
                r.stderr.starts_with(&format!(
                    "coreav: ran {dir}/ffprobe instead (not native: -show_frames)\n"
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
    let commands: [&[&str]; 3] = [
        &[
            "ffmpeg",
            "-headers",
            "Authorization: Bearer MARKER",
            "-i",
            "https://example.com/a.mp4?token=MARKER",
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
            "format_tags=MARKER:frame=MARKER",
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
        &["ffprobe", "-show_frames", &input],
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
    assert_eq!(lines[1]["reason"], "-show_entries frame");
    assert_eq!(
        lines[1]["args"],
        serde_json::json!([
            "-v",
            "quiet",
            "-show_entries",
            "format_tags=KEY:frame=KEY",
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
        r.stdout.starts_with("args: -h demuxer=mov\n"),
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
    assert_eq!(ran.stdout, format!("args: -i {input} -c copy {o}\n"));
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
        ["ffmpeg", "-i", "file:in.mp4", "-c", "copy", "out.mp4"],
        ["ffmpeg", "-i", "in.mp4", "-c", "copy", "file:out.mp4"],
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
        assert_eq!(r.stdout, format!("args: {}\n", args[1..].join(" ")));
        assert!(
            r.stderr.starts_with(&format!(
                "coreav: ran {dir}/ffmpeg instead (not native: an "
            )),
            "{}",
            r.stderr
        );
    }
}
