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
/// negative maps have removed theirs: 20,000 `-map 0` on a file of two
/// streams would otherwise plan 40,000 outputs, and 513 `-map 0` less the
/// audio are 513 video tracks.
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
    for maps in [513, 20_000] {
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
            track(video, 3, true, b"avc1", b"avc1"),
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
            video.to_vec(),
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

#[test]
fn joins_ffmpeg_would_time_or_read_otherwise_are_refused() {
    let dir = scratch("ffmpeg-concat-not-native");
    movie(&dir);
    // The audio lasts 0.32 s, twice the video (its `stts`, `mdhd` and the
    // movie's duration): the concat demuxer places the next file at 0.32 s,
    // the join at the video's end.
    patched_movie(
        &dir,
        "long.mp4",
        &[
            (2, &[1, 4, 512], &[1, 4, 1024]),
            (2, &[12_800, 2048], &[12_800, 4096]),
            (1, &[1000, 160], &[1000, 320]),
        ],
    );
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
        (
            "file long.mp4\nfile long.mp4\n",
            "streams end at different times",
        ),
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
    let cases = ["file:", subfile, ":", "a,b:"].into_iter().flat_map(|p| {
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
                let (status, detail) = match coreav_cli::ffmpeg::run(&args, StrictSafe::new(false))
                {
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
                    Err(Stop::Refused(Refusal::Help)) => panic!("{label}: read as a help query"),
                    Err(Stop::Failed(failure)) => {
                        assert!(!failure.is_io(), "{label}: {}", failure.to_json());
                        ("rejected", failure.to_json())
                    }
                };
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
            assert!(
                native.contains(&(id, *family)),
                "{id}: no {family} fixture ran natively"
            );
        }
    }
    std::fs::write(
        out.join("records.json"),
        serde_json::to_string_pretty(&Value::Array(records)).unwrap(),
    )
    .unwrap();
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
/// `work`, which `tools/compat-corpus/copy_check.py` reads. Unlike
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
        String::from_utf8(out.stdout).unwrap()
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
