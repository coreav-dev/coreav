//! The `coreav` binary end to end: schema, compat output,
//! fragment scanning, allowlist, errors and exit codes.
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
#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod text_movie;
#[path = "../../coreav-mpegts/tests/common/mod.rs"]
mod ts;

use common::{aac_entry, av_movie_with, bx, fragmented_movie};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA: &str = include_str!("../../../docs/schema/probe-v1.schema.json");
const CLIP_SCHEMA: &str = include_str!("../../../docs/schema/clip-v1.schema.json");
const SPLIT_SCHEMA: &str = include_str!("../../../docs/schema/split-v1.schema.json");
const HLS_INDEX_SCHEMA: &str = include_str!("../../../docs/schema/hls-index-v2.schema.json");
const REMUX_SCHEMA: &str = include_str!("../../../docs/schema/remux-v1.schema.json");
const EXTRACT_SCHEMA: &str = include_str!("../../../docs/schema/extract-audio-v1.schema.json");
const SUBTITLES_SCHEMA: &str =
    include_str!("../../../docs/schema/extract-subtitles-v1.schema.json");
const CHAPTERS_SCHEMA: &str = include_str!("../../../docs/schema/chapters-v1.schema.json");
const JOIN_SCHEMA: &str = include_str!("../../../docs/schema/join-v1.schema.json");

const MATROSKA_SCHEMA: &str = include_str!("../../../docs/schema/probe-matroska-v1.schema.json");
const MPEGTS_SCHEMA: &str = include_str!("../../../docs/schema/probe-mpegts-v1.schema.json");

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_coreav")
}

/// Writes `bytes` to a file under Cargo's per-test temporary directory.
fn file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], path: &Path) -> Run {
    let out = Command::new(bin())
        .arg("probe")
        .args(args)
        .arg(path)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

// ------------------------------------------------- a small schema validator
//
// Supports the subset `docs/schema/probe-v1.schema.json` uses: type, const,
// enum, minimum, properties, required, additionalProperties: false, items,
// anyOf and local $ref.

fn type_ok(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        other => panic!("schema type {other} not supported"),
    }
}

fn validate(value: &Value, schema: &Value, root: &Value, at: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.strip_prefix("#/$defs/").unwrap();
        return validate(value, &root["$defs"][name], root, at);
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        let errors: Vec<String> = options
            .iter()
            .filter_map(|s| validate(value, s, root, at).err())
            .collect();
        if errors.len() == options.len() {
            return Err(format!("{at}: no anyOf branch matches: {errors:?}"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if value != expected {
            return Err(format!("{at}: {value} is not {expected}"));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{at}: {value} not in {allowed:?}"));
        }
    }
    if let Some(types) = schema.get("type") {
        let ok = match types {
            Value::String(name) => type_ok(value, name),
            Value::Array(names) => names.iter().any(|n| type_ok(value, n.as_str().unwrap())),
            _ => panic!("bad type in schema"),
        };
        if !ok {
            return Err(format!("{at}: {value} is not of type {types}"));
        }
    }
    if let (Some(minimum), Some(number)) = (schema.get("minimum"), value.as_f64()) {
        if number < minimum.as_f64().unwrap() {
            return Err(format!("{at}: {value} below {minimum}"));
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        for key in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = key.as_str().unwrap();
            if !object.contains_key(key) {
                return Err(format!("{at}: missing key {key}"));
            }
        }
        for (key, item) in object {
            match properties.and_then(|p| p.get(key)) {
                Some(sub) => validate(item, sub, root, &format!("{at}.{key}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{at}: undocumented key {key}"));
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            validate(item, items, root, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

/// Validates against the schema the document names.
fn assert_any_schema(document: &Value) {
    match document["schema"].as_str() {
        Some("coreav.probe-matroska/1") => assert_matroska_schema(document),
        Some("coreav.probe-mpegts/1") => assert_matches(document, MPEGTS_SCHEMA),
        _ => assert_schema(document),
    }
}

fn assert_matroska_schema(document: &Value) {
    let root = json(MATROSKA_SCHEMA);
    if let Err(error) = validate(document, &root, &root, "$") {
        panic!("schema violation: {error}\n{document:#}");
    }
}

fn assert_schema(document: &Value) {
    assert_matches(document, SCHEMA);
}

fn assert_matches(document: &Value, schema: &str) {
    let root = json(schema);
    if let Err(error) = validate(document, &root, &root, "$") {
        panic!("schema violation: {error}\n{document:#}");
    }
}

fn assert_error(run: &Run, code: i32, kind: &str) -> Value {
    assert_eq!(run.code, code, "stderr: {}", run.stderr);
    assert!(run.stdout.is_empty());
    let error = json(&run.stderr);
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["kind"], kind, "{error}");
    for key in ["limit", "offset", "message"] {
        assert!(error.get(key).is_some(), "missing {key}: {error}");
    }
    error
}

// -------------------------------------------------------------------- tests

#[test]
fn the_validator_catches_violations() {
    let root = json(SCHEMA);
    let path = file("av-validator.mp4", &av_movie_with(aac_entry(), false));
    let good = json(&run(&[], &path).stdout);
    assert!(validate(&good, &root, &root, "$").is_ok());
    let mut missing = good.clone();
    missing["streams"][0]
        .as_object_mut()
        .unwrap()
        .remove("codec");
    assert!(validate(&missing, &root, &root, "$").is_err());
    let mut extra = good.clone();
    extra["container"]["surprise"] = Value::Bool(true);
    assert!(validate(&extra, &root, &root, "$").is_err());
    let mut wrong = good;
    wrong["streams"][1]["kind"] = Value::from("sound");
    assert!(validate(&wrong, &root, &root, "$").is_err());
}

#[test]
fn native_output_matches_the_schema() {
    let path = file("av.mp4", &av_movie_with(aac_entry(), false));
    let run = run(&[], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_schema(&doc);
    assert_eq!(doc["schema"], "coreav.probe/1");
    assert_eq!(doc["container"]["major_brand"], "isom");
    let streams = doc["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["kind"], "video");
    assert_eq!(streams[0]["codec"], "h264");
    assert!(streams[0]["audio"].is_null());
    assert_eq!(streams[0]["video"]["width"], 320);
    assert_eq!(streams[1]["codec"], "aac");
    assert!(streams[1]["video"].is_null());
    assert_eq!(streams[1]["audio"]["sample_rate"], 48_000);
    assert_eq!(streams[1]["sample_count"], 4);
    // Exact ticks first, seconds as a copy.
    let duration = &doc["duration"];
    assert_eq!(duration["timescale"], 1000);
    assert_eq!(duration["ticks"]["den"], 1);
}

#[test]
fn compat_output_uses_ffprobe_names() {
    let path = file("av-compat.mp4", &av_movie_with(aac_entry(), false));
    let run = run(&["--ffprobe-compat"], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    let format = &doc["format"];
    assert_eq!(format["format_name"], "mov,mp4,m4a,3gp,3g2,mj2");
    assert_eq!(format["nb_streams"], 2);
    assert_eq!(
        format["size"],
        std::fs::metadata(&path).unwrap().len().to_string()
    );
    let video = &doc["streams"][0];
    assert_eq!(video["codec_type"], "video");
    assert_eq!(video["codec_tag_string"], "avc1");
    assert_eq!(video["codec_tag"], "0x31637661");
    assert_eq!(video["time_base"], "1/12800");
    let audio = &doc["streams"][1];
    assert_eq!(audio["codec_name"], "aac");
    assert_eq!(audio["sample_rate"], "48000");
    // Stream bit rate is not checked against ffprobe, so it is not printed.
    assert!(audio.get("bit_rate").is_none());
}

#[test]
fn fragments_are_read_only_on_request() {
    let path = file("fragmented.mp4", &fragmented_movie(None, None));
    let plain = run(&[], &path);
    assert_eq!(plain.code, 0, "{}", plain.stderr);
    let doc = json(&plain.stdout);
    assert_schema(&doc);
    assert_eq!(doc["container"]["fragmented"], true);
    assert_eq!(doc["streams"][0]["duration_source"], "unknown");
    assert!(doc["streams"][0]["sample_count"].is_null());
    assert_eq!(doc["warnings"].as_array().unwrap().len(), 1);

    let scanned = json(&run(&["--scan-fragments"], &path).stdout);
    assert_schema(&scanned);
    assert_eq!(scanned["streams"][0]["duration_source"], "observed");
    assert_eq!(scanned["streams"][0]["sample_count"], 4);
    assert!(scanned["warnings"].as_array().unwrap().is_empty());

    // Compat mode scans by default, unless told not to.
    let compat = json(&run(&["--ffprobe-compat"], &path).stdout);
    assert_eq!(compat["streams"][0]["duration_ts"], 2048);
    let unscanned = json(&run(&["--ffprobe-compat", "--no-scan-fragments"], &path).stdout);
    assert!(unscanned["streams"][0].get("duration_ts").is_none());
}

#[test]
fn allowlist_drops_or_rejects() {
    let path = file("av-allow.mp4", &av_movie_with(aac_entry(), false));
    let dropped = run(&["--allow", "h264"], &path);
    assert_eq!(dropped.code, 0, "{}", dropped.stderr);
    let doc = json(&dropped.stdout);
    assert_schema(&doc);
    assert_eq!(doc["streams"].as_array().unwrap().len(), 1);
    assert_eq!(doc["dropped_streams"][0]["stream_index"], 1);
    assert_eq!(doc["dropped_streams"][0]["reason"], "codec_not_allowed");

    let compat = json(&run(&["--allow", "h264", "--ffprobe-compat"], &path).stdout);
    assert_eq!(compat["format"]["nb_streams"], 2);
    assert_eq!(compat["streams"].as_array().unwrap().len(), 1);

    let strict = run(&["--allow", "h264", "--strict"], &path);
    assert_error(&strict, 1, "codec_not_allowed");
}

#[test]
fn rejected_files_exit_1_with_a_json_error() {
    let mut nested = bx(b"ftyp", b"isom\0\0\x02\0");
    let levels = 10_000usize;
    for i in 0..levels {
        let size = (8 * (levels - i)) as u32;
        nested.extend_from_slice(&size.to_be_bytes());
        nested.extend_from_slice(if i == 0 { b"moov" } else { b"trak" });
    }
    let path = file("nested.mp4", &nested);
    let deep = run(&[], &path);
    assert_eq!(deep.code, 1, "{}", deep.stderr);
    let error = json(&deep.stderr);
    assert!(
        matches!(error["kind"].as_str(), Some("limit" | "invalid_structure")),
        "{error}"
    );

    let riff = file("not-a-movie.avi", b"RIFF\x24\x00\x00\x00AVI LIST");
    assert_error(&run(&[], &riff), 1, "unsupported_container");
    assert_error(
        &run(&["--ffprobe-compat"], &riff),
        1,
        "unsupported_container",
    );
}

#[test]
fn io_and_usage_errors_have_their_own_codes() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("does-not-exist.mp4");
    assert_error(&run(&[], &missing), 3, "io");

    let path = file("av-usage.mp4", &av_movie_with(aac_entry(), false));
    let unknown = run(&["--allow", "magicyuv"], &path);
    assert_eq!(unknown.code, 2);
    assert!(unknown.stdout.is_empty());
    // `other` is a codec name in the output but can never be allowed.
    let other = run(&["--allow", "h264,other"], &path);
    assert_eq!(other.code, 2);
    assert!(
        other.stderr.contains("unknown codec 'other'"),
        "{}",
        other.stderr
    );
    let conflict = run(&["--scan-fragments", "--no-scan-fragments"], &path);
    assert_eq!(conflict.code, 2);
}

/// Every generated fixture through the binary: success matches the schema,
/// rejection prints a valid error. Run by the fixtures workflow with
/// `COREAV_FIXTURES` set to the generated directory.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixtures_match_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut checked = 0;
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
    {
        // Also with the scan, whose values (stream durations and bit rates
        // of Matroska files, scanned sizes) are `null` without it.
        for args in [
            &["--scan-fragments"][..],
            &["--scan-fragments", "--scan-frame-sizes"],
        ] {
            let result = run(args, &dir.join(name));
            match result.code {
                0 => assert_any_schema(&json(&result.stdout)),
                1 => {
                    let error = json(&result.stderr);
                    assert_eq!(error["schema"], "coreav.error/1", "{name}");
                }
                other => panic!("{name}: exit code {other}: {}", result.stderr),
            }
        }
        checked += 1;
    }
    assert!(checked > 0, "no fixtures listed");
}

// --------------------------------------------------------------- coreav clip

fn clip(args: &[&str]) -> Run {
    let out = Command::new(bin()).arg("clip").args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// A fresh output path under Cargo's per-test temporary directory.
fn output(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn clip_writes_a_file_and_a_summary() {
    let input = file("clip_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("clip_out.mp4");
    let result = clip(&[
        "--start",
        "0",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, CLIP_SCHEMA);
    assert_eq!(summary["schema"], "coreav.clip/1");
    let written = std::fs::read(&out).unwrap();
    assert_eq!(summary["output"]["bytes"], written.len());
    assert_eq!(&written[4..8], b"ftyp");
    assert_eq!(summary["streams"][0]["kind"], "video");
    assert_eq!(summary["streams"][0]["codec"], "h264");
    assert_eq!(summary["streams"][1]["codec"], "aac");
    assert_eq!(
        summary["start"]["requested"],
        serde_json::json!({"num": 0, "den": 1})
    );
    // The clip's own probe is a valid movie with both streams.
    let probe = run(&[], &out);
    assert_eq!(probe.code, 0, "{}", probe.stderr);
    assert_eq!(json(&probe.stdout)["streams"].as_array().unwrap().len(), 2);
    // Without audio.
    let video_only = output("clip_video_only.mp4");
    let result = clip(&[
        "--start",
        "0",
        "--audio",
        "none",
        input.to_str().unwrap(),
        video_only.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_eq!(summary["streams"].as_array().unwrap().len(), 1);
    assert_eq!(summary["left_out"][0]["reason"], "not_selected");
}

#[test]
fn clip_never_overwrites_without_force() {
    let input = file("clip_force_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("clip_force_out.mp4");
    std::fs::write(&out, b"keep me").unwrap();
    let args = [
        "--start",
        "0",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ];
    let refused = clip(&args);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    let mut forced = vec!["--force"];
    forced.extend(args);
    let result = clip(&forced);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_ne!(std::fs::read(&out).unwrap(), b"keep me");
}

#[test]
fn clip_never_writes_over_its_input() {
    let bytes = av_movie_with(aac_entry(), false);
    let input = file("clip_self_in.mp4", &bytes);
    let refused = clip(&[
        "--force",
        "--start",
        "0",
        input.to_str().unwrap(),
        input.to_str().unwrap(),
    ]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert_eq!(std::fs::read(&input).unwrap(), bytes);
    // Another hard link to the input.
    #[cfg(unix)]
    {
        let link = output("clip_self_link.mp4");
        std::fs::hard_link(&input, &link).unwrap();
        let refused = clip(&[
            "--force",
            "--start",
            "0",
            input.to_str().unwrap(),
            link.to_str().unwrap(),
        ]);
        assert_eq!(refused.code, 2, "{}", refused.stderr);
        assert_eq!(std::fs::read(&input).unwrap(), bytes);
    }
}

#[test]
fn a_failed_forced_clip_keeps_the_existing_output() {
    let input = file("clip_keep_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("clip_keep_out.mp4");
    std::fs::write(&out, b"keep me").unwrap();
    // A start past the end of the 0.16 s movie.
    let result = clip(&[
        "--force",
        "--start",
        "100",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    let parts = std::fs::read_dir(out.parent().unwrap())
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains("clip_keep_out.mp4") && name.ends_with(".part")
        })
        .count();
    assert_eq!(parts, 0, "a temporary file was left behind");
}

#[test]
fn a_failed_clip_leaves_no_file() {
    let input = file("clip_fail_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("clip_fail_out.mp4");
    // The movie lasts 0.16 s: a start at 100 s is past its end.
    let result = clip(&[
        "--start",
        "100",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_error(&result, 1, "unsupported_feature");
    assert!(!out.exists());
    // Not a movie at all.
    let junk = file("clip_junk.mp4", b"not a movie");
    let result = clip(&[
        "--start",
        "0",
        junk.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    assert!(!out.exists());
    // A missing input is an I/O error.
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clip_missing.mp4");
    let result = clip(&[
        "--start",
        "0",
        missing.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_error(&result, 3, "io");
    assert!(!out.exists());
}

#[test]
fn clip_to_standard_output() {
    let input = file("clip_stdout_in.mp4", &av_movie_with(aac_entry(), false));
    let out = Command::new(bin())
        .args(["clip", "--start", "0", input.to_str().unwrap(), "-"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(&out.stdout[4..8], b"ftyp");
    let summary = json(&String::from_utf8(out.stderr).unwrap());
    assert_matches(&summary, CLIP_SCHEMA);
    assert_eq!(summary["output"]["bytes"], out.stdout.len());
}

#[test]
fn clip_usage_errors_exit_2() {
    let input = file("clip_usage_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("clip_usage_out.mp4");
    for args in [
        vec!["--start", "1.5s"],
        vec!["--start", "-1"],
        vec!["--start", "0", "--end", "x"],
        vec!["--start", "0", "--audio", "first"],
        vec!["--start", "0", "--allow", "h264,nope"],
    ] {
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
        let result = clip(&all);
        assert_eq!(result.code, 2, "{args:?}: {}", result.stderr);
        assert!(!out.exists());
    }
}

#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixture_clips_match_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut clipped = 0;
    let mut candidates = 0;
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .filter(|name| name.ends_with(".mp4") || name.ends_with(".mov"))
    {
        candidates += 1;
        let out = output(&format!("fixture_clip_{name}.mp4"));
        let input = dir.join(name);
        let result = clip(&[
            "--start",
            "0.5",
            "--end",
            "1.5",
            input.to_str().unwrap(),
            out.to_str().unwrap(),
        ]);
        match result.code {
            0 => {
                let summary = json(&result.stdout);
                assert_matches(&summary, CLIP_SCHEMA);
                assert_eq!(
                    summary["output"]["bytes"],
                    std::fs::metadata(&out).unwrap().len(),
                    "{name}"
                );
                clipped += 1;
            }
            1 => {
                let error = json(&result.stderr);
                assert_eq!(error["schema"], "coreav.error/1", "{name}");
                assert!(!out.exists(), "{name}: a failed clip left a file");
            }
            other => panic!("{name}: exit code {other}: {}", result.stderr),
        }
    }
    // A fixture set without MP4 or MOV files (the Matroska fixtures) has
    // nothing to clip.
    assert!(candidates == 0 || clipped > 0, "no fixture was clipped");
}

// ------------------------------------------------------ coreav extract-audio

fn extract(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("extract-audio")
        .args(args)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// The library's output for the same request, to compare bytes with.
fn extract_in_memory(input: &Path, choice: coreav_isobmff_write::TrackChoice) -> Vec<u8> {
    let mut out = Vec::new();
    let data = std::fs::read(input).unwrap();
    let allowlist = coreav_isobmff::Allowlist::default();
    assert!(coreav_cli::extract::run_slice(&data, choice, &allowlist, &mut out).is_ok());
    out
}

#[test]
fn extract_audio_writes_a_file_and_a_summary() {
    let input = file("extract_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("extract_out.m4a");
    let result = extract(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, EXTRACT_SCHEMA);
    let written = std::fs::read(&out).unwrap();
    assert_eq!(summary["output"]["bytes"], written.len());
    assert_eq!(&written[4..12], b"ftypM4A ");
    assert_eq!(
        written,
        extract_in_memory(&input, coreav_isobmff_write::TrackChoice::FirstAudio)
    );
    assert_eq!(summary["stream"]["stream_index"], 1);
    assert_eq!(summary["stream"]["track_id"], 2);
    assert_eq!(summary["stream"]["codec"], "aac");
    assert_eq!(summary["stream"]["samples"], 4);
    // The output probes as one AAC stream.
    let probe = run(&[], &out);
    assert_eq!(probe.code, 0, "{}", probe.stderr);
    let streams = json(&probe.stdout)["streams"].clone();
    assert_eq!(streams.as_array().unwrap().len(), 1);
    assert_eq!(streams[0]["codec"], "aac");
    // The same stream named by its index.
    let named = output("extract_named.m4a");
    let result = extract(&[
        "--stream",
        "1",
        input.to_str().unwrap(),
        named.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(std::fs::read(&named).unwrap(), written);
}

#[test]
fn extract_audio_refusals_exit_1_and_leave_no_file() {
    let input = file("extract_refuse_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("extract_refuse_out.m4a");
    for (args, kind) in [
        // Stream 0 is the video stream.
        (vec!["--stream", "0"], "unsupported_feature"),
        (vec!["--stream", "7"], "unsupported_feature"),
        // The allowlist dropped the only audio track: its codec is not
        // allowed, whether chosen by default or by index.
        (vec!["--allow", "h264"], "codec_not_allowed"),
        (
            vec!["--allow", "h264", "--stream", "1"],
            "codec_not_allowed",
        ),
        (vec!["--allow", "h264", "--strict"], "codec_not_allowed"),
    ] {
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
        let result = extract(&all);
        assert_error(&result, 1, kind);
        assert!(!out.exists(), "{args:?}");
    }
    // Not AAC.
    let mp3 = file("extract_mp3.mp4", &common::av_movie(b".mp3"));
    let result = extract(&[mp3.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 1, "unsupported_feature");
    assert!(!out.exists());
    // Not a movie; a missing input is an I/O error.
    let junk = file("extract_junk.mp4", b"not a movie");
    let result = extract(&[junk.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 1, "unsupported_container");
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("extract_missing.mp4");
    let result = extract(&[missing.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 3, "io");
    assert!(!out.exists());
}

#[test]
fn extract_audio_keeps_existing_files() {
    let bytes = av_movie_with(aac_entry(), false);
    let input = file("extract_keep_in.mp4", &bytes);
    let out = output("extract_keep_out.m4a");
    std::fs::write(&out, b"keep me").unwrap();
    let refused = extract(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    // A failed forced extraction keeps the file and leaves no temporary one.
    let refused = extract(&[
        "--force",
        "--stream",
        "0",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(refused.code, 1, "{}", refused.stderr);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    let parts = std::fs::read_dir(out.parent().unwrap())
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains("extract_keep_out.m4a") && name.ends_with(".part")
        })
        .count();
    assert_eq!(parts, 0, "a temporary file was left behind");
    // Never over the input, even with --force.
    let refused = extract(&["--force", input.to_str().unwrap(), input.to_str().unwrap()]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert_eq!(std::fs::read(&input).unwrap(), bytes);
    let result = extract(&["--force", input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_ne!(std::fs::read(&out).unwrap(), b"keep me");
}

#[test]
fn extract_audio_output_errors_exit_3() {
    let input = file("extract_io_in.mp4", &av_movie_with(aac_entry(), false));
    // A directory cannot be replaced by the output file: refused before
    // any work, as a usage error.
    let dir = output("extract_io_dir.m4a");
    let _ = std::fs::remove_dir(&dir);
    std::fs::create_dir(&dir).unwrap();
    let result = extract(&["--force", input.to_str().unwrap(), dir.to_str().unwrap()]);
    assert_eq!(result.code, 2, "{}", result.stderr);
    assert!(
        result.stderr.contains("is a directory"),
        "{}",
        result.stderr
    );
    assert!(dir.is_dir());
    let parts = std::fs::read_dir(dir.parent().unwrap())
        .unwrap()
        .filter(|e| {
            let name = e.as_ref().unwrap().file_name();
            let name = name.to_string_lossy();
            name.contains("extract_io_dir.m4a") && name.ends_with(".part")
        })
        .count();
    assert_eq!(parts, 0, "a temporary file was left behind");
    // A missing directory.
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no_such_dir/out.m4a");
    let result = extract(&[input.to_str().unwrap(), missing.to_str().unwrap()]);
    assert_error(&result, 3, "io");
    // Standard output on a full device.
    #[cfg(target_os = "linux")]
    {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let out = Command::new(bin())
            .args(["extract-audio", input.to_str().unwrap(), "-"])
            .stdout(full)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
        let error = json(&String::from_utf8(out.stderr).unwrap());
        assert_eq!(error["kind"], "io");
    }
}

#[test]
fn extract_audio_to_standard_output() {
    let input = file("extract_stdout_in.mp4", &av_movie_with(aac_entry(), false));
    let out = Command::new(bin())
        .args(["extract-audio", input.to_str().unwrap(), "-"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        out.stdout,
        extract_in_memory(&input, coreav_isobmff_write::TrackChoice::FirstAudio)
    );
    let summary = json(&String::from_utf8(out.stderr).unwrap());
    assert_matches(&summary, EXTRACT_SCHEMA);
    assert_eq!(summary["output"]["bytes"], out.stdout.len());
}

#[test]
fn extract_audio_usage_errors_exit_2() {
    let input = file("extract_usage_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("extract_usage_out.m4a");
    for args in [
        vec!["--stream", "first"],
        vec!["--stream", "-1"],
        vec!["--allow", "aac,nope"],
        vec!["--start", "0"],
    ] {
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
        let result = extract(&all);
        assert_eq!(result.code, 2, "{args:?}: {}", result.stderr);
        assert!(!out.exists());
    }
}

/// Every AAC stream of every MP4, MOV and M4A fixture through the binary:
/// the summary matches its schema and the output equals the library's
/// bytes. With `COREAV_EXTRACT_CLI_OUT`, the outputs and a manifest for
/// `scripts/extract_check.py` are written there.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixture_extractions_match_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let out_dir = std::env::var("COREAV_EXTRACT_CLI_OUT")
        .ok()
        .map(PathBuf::from);
    if let Some(out_dir) = &out_dir {
        std::fs::create_dir_all(out_dir).unwrap();
    }
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut entries = Vec::new();
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .filter(|name| [".mp4", ".mov", ".m4a"].iter().any(|e| name.ends_with(e)))
    {
        let input = dir.join(name);
        let probe = run(&[], &input);
        if probe.code != 0 {
            continue;
        }
        let probed = json(&probe.stdout);
        for stream in probed["streams"].as_array().unwrap() {
            if stream["codec"] != "aac" {
                continue;
            }
            let index = stream["stream_index"].as_u64().unwrap();
            let out = match &out_dir {
                Some(out_dir) => {
                    let path = out_dir.join(format!("{name}.{index}.m4a"));
                    let _ = std::fs::remove_file(&path);
                    path
                }
                None => output(&format!("fixture_extract_{name}.{index}.m4a")),
            };
            let result = extract(&[
                "--stream",
                &index.to_string(),
                input.to_str().unwrap(),
                out.to_str().unwrap(),
            ]);
            assert_eq!(result.code, 0, "{name} stream {index}: {}", result.stderr);
            let summary = json(&result.stdout);
            assert_matches(&summary, EXTRACT_SCHEMA);
            let written = std::fs::read(&out).unwrap();
            assert_eq!(summary["output"]["bytes"], written.len(), "{name}");
            assert_eq!(summary["stream"]["stream_index"], index, "{name}");
            let choice = coreav_isobmff_write::TrackChoice::Stream(index as usize);
            assert!(
                written == extract_in_memory(&input, choice),
                "{name} stream {index}: the binary's output differs from the library's"
            );
            entries.push(serde_json::json!({
                "source": input.display().to_string(),
                "stream_index": index,
                "output": out.display().to_string(),
            }));
        }
    }
    // A fixture set without MP4, MOV or M4A files (the Matroska fixtures)
    // has nothing to extract.
    let candidates = manifest.lines().any(|line| {
        line.strip_prefix("generated ")
            .is_some_and(|n| n.ends_with(".mp4") || n.ends_with(".mov") || n.ends_with(".m4a"))
    });
    assert!(
        !candidates || !entries.is_empty(),
        "no fixture was extracted"
    );
    if let Some(out_dir) = &out_dir {
        let text = serde_json::to_string_pretty(&Value::Array(entries)).unwrap();
        std::fs::write(out_dir.join("manifest.json"), text).unwrap();
    }
}

// ------------------------------- coreav extract-subtitles and coreav chapters

fn command(name: &str, args: &[&str]) -> Run {
    let out = Command::new(bin()).arg(name).args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// Video (track 1) naming a QuickTime chapter track (track 3), and a 3GPP
/// timed text track (track 2) in Korean.
fn text_movie_bytes() -> Vec<u8> {
    use text_movie::{TrackSpec, movie, styl, text_sample};
    let video = TrackSpec {
        chap: vec![3],
        ..TrackSpec::video(1, &[1000, 1000])
    };
    let subtitles = TrackSpec::tx3g(
        2,
        vec![
            (text_sample(b"", &[]), 500),
            (text_sample(b"Hi & <bye>", &[styl(0, 2)]), 1000),
        ],
    );
    let chapters = TrackSpec::qt_text(
        3,
        vec![
            (text_sample(b"One", &[]), 800),
            (text_sample(b"Two", &[]), 1200),
        ],
    );
    let mut data = movie(&[video, subtitles, chapters]);
    // The Korean language code in the timed text track's `mdhd`.
    let at = data
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == b"mdhd")
        .nth(1)
        .unwrap()
        .0;
    let packed: u16 =
        (u16::from(b'k' - 0x60) << 10) | (u16::from(b'o' - 0x60) << 5) | u16::from(b'r' - 0x60);
    data[at + 24..at + 26].copy_from_slice(&packed.to_be_bytes());
    data
}

#[test]
fn compat_sizes_only_the_text_tracks_ffprobe_lists_as_subtitles() {
    use text_movie::{TrackSpec, movie, text_sample};
    // A 400x60 subtitle track, and a 160x160 `tx3g` chapter track (as in an
    // M4A podcast), named by an enabled or a disabled track: ffprobe lists
    // the chapter track as `bin_data`, with no size (KD-CHAPTER-TEXT).
    let samples = || vec![(text_sample(b"One", &[]), 1000)];
    for enabled in [true, false] {
        let video = TrackSpec {
            chap: vec![3],
            enabled,
            ..TrackSpec::video(1, &[1000])
        };
        let subtitles = TrackSpec {
            size: (400 << 16, 60 << 16),
            ..TrackSpec::tx3g(2, samples())
        };
        let chapters = TrackSpec {
            size: (160 << 16, 160 << 16),
            ..TrackSpec::tx3g(3, samples())
        };
        let path = file(
            &format!("chapter-size-{enabled}.mp4"),
            &movie(&[video, subtitles, chapters]),
        );
        let result = run(&["--ffprobe-compat"], &path);
        assert_eq!(result.code, 0, "{}", result.stderr);
        let streams = &json(&result.stdout)["streams"];
        assert_eq!(streams[1]["width"], 400);
        assert_eq!(streams[1]["height"], 60);
        assert!(streams[2].get("width").is_none(), "{}", streams[2]);
        assert!(streams[2].get("height").is_none(), "{}", streams[2]);
    }
}

#[test]
fn extract_subtitles_writes_webvtt_and_a_summary() {
    let input = file("text_in.mp4", &text_movie_bytes());
    let out = output("text_out.vtt");
    let result = command(
        "extract-subtitles",
        &[input.to_str().unwrap(), out.to_str().unwrap()],
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, SUBTITLES_SCHEMA);
    let written = std::fs::read_to_string(&out).unwrap();
    assert_eq!(
        written,
        "WEBVTT\n\n00:00:00.500 --> 00:00:01.500\nHi &amp; &lt;bye&gt;\n\n"
    );
    assert_eq!(summary["output"]["bytes"], written.len());
    assert_eq!(summary["stream"]["stream_index"], 1);
    assert_eq!(summary["stream"]["track_id"], 2);
    assert_eq!(summary["stream"]["codec"], "timed_text");
    assert_eq!(summary["stream"]["language"], "kor");
    assert_eq!(summary["samples"], 2);
    assert_eq!(summary["cues"], 1);
    assert_eq!(summary["dropped"]["empty"], 1);
    assert_eq!(summary["dropped"]["styling"]["styl"], 1);
    // The chapter track by index, to standard output.
    let out = Command::new(bin())
        .args([
            "extract-subtitles",
            "--stream",
            "2",
            input.to_str().unwrap(),
            "-",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(out.stdout.clone()).unwrap(),
        "WEBVTT\n\n00:00:00.000 --> 00:00:00.800\nOne\n\n00:00:00.800 --> 00:00:02.000\nTwo\n\n"
    );
    let summary = json(&String::from_utf8(out.stderr).unwrap());
    assert_matches(&summary, SUBTITLES_SCHEMA);
    assert_eq!(summary["stream"]["codec"], "quicktime_text");
    assert_eq!(summary["output"]["bytes"], out.stdout.len());
}

#[test]
fn extract_subtitles_errors() {
    let input = file("text_err_in.mp4", &text_movie_bytes());
    let out = output("text_err_out.vtt");
    let (i, o) = (input.to_str().unwrap(), out.to_str().unwrap());
    // Not a text stream; no such stream; no subtitle track.
    assert_error(
        &command("extract-subtitles", &["--stream", "0", i, o]),
        1,
        "unsupported_feature",
    );
    assert_error(
        &command("extract-subtitles", &["--stream", "9", i, o]),
        1,
        "unsupported_feature",
    );
    let audio = file("text_err_audio.mp4", &av_movie_with(aac_entry(), false));
    assert_error(
        &command("extract-subtitles", &[audio.to_str().unwrap(), o]),
        1,
        "unsupported_feature",
    );
    let junk = file("text_err_junk.mp4", b"not a movie");
    assert_error(
        &command("extract-subtitles", &[junk.to_str().unwrap(), o]),
        1,
        "unsupported_container",
    );
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("text_missing.mp4");
    assert_error(
        &command("extract-subtitles", &[missing.to_str().unwrap(), o]),
        3,
        "io",
    );
    assert!(!out.exists());
    // Usage errors; an existing output needs --force.
    for args in [vec!["--stream", "x"], vec!["--allow", "h264"]] {
        let mut all = args.clone();
        all.extend([i, o]);
        assert_eq!(command("extract-subtitles", &all).code, 2, "{args:?}");
    }
    std::fs::write(&out, b"keep me").unwrap();
    assert_eq!(command("extract-subtitles", &[i, o]).code, 2);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    assert_eq!(command("extract-subtitles", &["--force", i, o]).code, 0);
    assert_eq!(command("extract-subtitles", &["--force", i, i]).code, 2);
}

/// Chapter tracks merged by `ffprobe` keep to the title bytes limit
/// together: the earlier track's longer list keeps its tail, so a later
/// track that would take the whole over the limit is left out.
#[test]
fn ffprobe_chapters_keep_the_title_limit_across_tracks() {
    use text_movie::{TrackSpec, movie, text_sample};
    let long = vec![b'a'; 65_500];
    let mut first: Vec<_> = (0..64).map(|_| (text_sample(b"", &[]), 1000)).collect();
    first.extend((0..64).map(|_| (text_sample(&long, &[]), 1000)));
    let second = (0..64).map(|_| (text_sample(&long, &[]), 1000)).collect();
    let video = TrackSpec {
        chap: vec![2, 3],
        ..TrackSpec::video(1, &[1000; 128])
    };
    let path = file(
        "chapter-title-limit.mp4",
        &movie(&[
            video,
            TrackSpec::qt_text(2, first),
            TrackSpec::qt_text(3, second),
        ]),
    );
    let result = command(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_chapters",
            "-of",
            "json",
            path.to_str().unwrap(),
        ],
    );
    assert_eq!(result.code, 0, "{}", result.stderr);
    let chapters = json(&result.stdout)["chapters"].as_array().unwrap().clone();
    let titles: Vec<usize> = chapters
        .iter()
        .map(|c| c["tags"]["title"].as_str().map_or(0, str::len))
        .collect();
    assert_eq!(titles.len(), 128);
    assert!(titles.iter().sum::<usize>() <= 4 << 20);
    assert_eq!(titles[..64], [0; 64]);
    assert_eq!(titles[64..], [65_500; 64]);
}

#[test]
fn chapters_print_the_list() {
    let input = file("chapters_in.mp4", &text_movie_bytes());
    let result = command("chapters", &[input.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let document = json(&result.stdout);
    assert_matches(&document, CHAPTERS_SCHEMA);
    assert_eq!(document["stream"]["track_id"], 3);
    assert_eq!(document["stream"]["stream_index"], 2);
    assert_eq!(document["stream"]["codec"], "quicktime_text");
    let chapters = document["chapters"].as_array().unwrap();
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[1]["title"], "Two");
    assert_eq!(
        chapters[1]["start"]["ticks"],
        serde_json::json!({"num": 800, "den": 1})
    );
    assert_eq!(chapters[1]["end"]["seconds"], 2.0);
    // Without a chapter track.
    let plain = file("chapters_none.mp4", &av_movie_with(aac_entry(), false));
    let result = command("chapters", &[plain.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let document = json(&result.stdout);
    assert_matches(&document, CHAPTERS_SCHEMA);
    assert!(document["stream"].is_null());
    assert!(document["chapters"].as_array().unwrap().is_empty());
    // Errors.
    let junk = file("chapters_junk.mp4", b"not a movie");
    assert_error(
        &command("chapters", &[junk.to_str().unwrap()]),
        1,
        "unsupported_container",
    );
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("chapters_missing.mp4");
    assert_error(&command("chapters", &[missing.to_str().unwrap()]), 3, "io");
}

/// Every text stream and every MP4, MOV and M4A fixture through the binary:
/// the subtitle summaries and chapter documents match their schemas.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixture_text_matches_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let (mut subtitles, mut chapters) = (0, 0);
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .filter(|name| [".mp4", ".mov", ".m4a"].iter().any(|e| name.ends_with(e)))
    {
        let input = dir.join(name);
        let result = command("chapters", &[input.to_str().unwrap()]);
        match result.code {
            0 => {
                let document = json(&result.stdout);
                assert_matches(&document, CHAPTERS_SCHEMA);
                chapters += usize::from(!document["chapters"].as_array().unwrap().is_empty());
            }
            1 => assert_eq!(json(&result.stderr)["schema"], "coreav.error/1", "{name}"),
            other => panic!("{name}: exit code {other}: {}", result.stderr),
        }
        let out = output(&format!("fixture_text_{name}.vtt"));
        let result = command(
            "extract-subtitles",
            &[input.to_str().unwrap(), out.to_str().unwrap()],
        );
        match result.code {
            0 => {
                let summary = json(&result.stdout);
                assert_matches(&summary, SUBTITLES_SCHEMA);
                assert_eq!(
                    summary["output"]["bytes"],
                    std::fs::metadata(&out).unwrap().len()
                );
                subtitles += 1;
            }
            1 => {
                assert_eq!(json(&result.stderr)["schema"], "coreav.error/1", "{name}");
                assert!(!out.exists(), "{name}: a failed extraction left a file");
            }
            other => panic!("{name}: exit code {other}: {}", result.stderr),
        }
    }
    let any = manifest
        .lines()
        .any(|l| l.ends_with(".mp4") || l.ends_with(".mov"));
    assert!(
        !any || (subtitles > 0 && chapters > 0),
        "no subtitles or chapters extracted"
    );
}

// ------------------------------------------------ MPEG transport streams

/// A small transport stream: one program, H.264-less, with one ADTS AAC
/// stream of three frames at 48 kHz, and 77 bytes of other data first, so
/// that the reader has to find the first packet.
fn small_ts(prefix: usize) -> Vec<u8> {
    let mut mux = ts::Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux.pes(0x101, 0xC0, 90_000, &ts::adts(100), true);
    mux.pes(
        0x101,
        0xC0,
        91_920,
        &[ts::adts(120), ts::adts(110)].concat(),
        true,
    );
    let mut out: Vec<u8> = (0..prefix).map(|i| ((i * 37 + 11) % 251) as u8).collect();
    out.extend(mux.out);
    out
}

#[test]
fn transport_streams_print_their_own_schema() {
    let path = file("small.ts", &small_ts(0));
    let run = run(&[], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_matches(&doc, MPEGTS_SCHEMA);
    assert_eq!(doc["schema"], "coreav.probe-mpegts/1");
    assert_eq!(doc["container"]["packet_size"], 188);
    assert_eq!(doc["container"]["first_packet"], 0);
    assert_eq!(doc["container"]["scanned"], false);
    assert_eq!(doc["container"]["programs"][0]["number"], 1);
    assert_eq!(doc["container"]["programs"][0]["pmt_pid"], 0x1000);
    assert_eq!(doc["start"]["ticks"], 90_000);
    assert_eq!(doc["start"]["timescale"], 90_000);
    let streams = doc["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["codec"], "aac");
    assert_eq!(streams[0]["kind"], "audio");
    assert_eq!(streams[0]["pid"], 0x101);
    assert_eq!(streams[0]["stream_type"], 0x0F);
    assert_eq!(streams[0]["index"], 0);
    assert_eq!(streams[0]["stream_index"], 0);
    assert_eq!(streams[0]["audio"]["sample_rate"], 48_000);
    assert_eq!(streams[0]["audio"]["channels"], 2);
    assert_eq!(streams[0]["profile"]["name"], "LC");
    assert_eq!(streams[0]["scan"], Value::Null);
    assert_eq!(doc["dropped_streams"].as_array().unwrap().len(), 0);
}

/// The reader finds the first packet itself, so a stream that starts after
/// other bytes is read; the ISO base media and Matroska readers decline the
/// file first.
#[test]
fn a_transport_stream_after_other_bytes_is_found() {
    let path = file("prefix.ts", &small_ts(77));
    let run = run(&[], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_eq!(doc["schema"], "coreav.probe-mpegts/1");
    assert_eq!(doc["container"]["first_packet"], 77);
    assert_eq!(doc["streams"].as_array().unwrap().len(), 1);
}

/// A file no reader recognises keeps the ISO base media rejection, even
/// when sync bytes happen to fall in it: the transport stream reader is
/// offered the file only when it holds stored packets.
#[test]
fn a_file_that_is_no_container_is_still_unsupported() {
    let plain = vec![0x11u8; 4096];
    // Sync bytes scattered about, including near the end, where fewer than
    // three packets could follow one.
    let mut sprinkled = plain.clone();
    for at in [7usize, 500, 1000, 1188, 2000, 3800, 3900, 4000, 4090] {
        sprinkled[at] = 0x47;
    }
    // Two sync bytes at a packet stride, one short of what confirms a
    // packet size.
    let mut two = plain.clone();
    two[64] = 0x47;
    two[64 + 188] = 0x47;
    for (name, data) in [
        ("none.ts", plain),
        ("sprinkled.ts", sprinkled),
        ("two-sync.ts", two),
    ] {
        let path = file(name, &data);
        let run = run(&[], &path);
        assert_eq!(run.code, 1, "{name}: {}", run.stdout);
        let error = json(&run.stderr);
        assert_eq!(error["kind"], "unsupported_container", "{name}");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("ISO base media"),
            "{name}: {error}"
        );
    }
}

/// A stream whose type names no codec the reader knows is listed with
/// `codec` `other`, in both output shapes.
#[test]
fn unnamed_transport_streams_are_listed() {
    let mut mux = ts::Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101), (0xA1, 0x102), (0x06, 0x103)]);
    mux.pes(0x101, 0xC0, 90_000, &ts::adts(100), true);
    let path = file("unnamed.ts", &mux.out);
    let doc = json(&run(&[], &path).stdout);
    assert_matches(&doc, MPEGTS_SCHEMA);
    let streams = doc["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 3);
    assert_eq!(streams[1]["codec"], "other");
    assert_eq!(streams[1]["stream_type"], 0xA1);
    assert_eq!(streams[2]["codec"], "other");
    assert_eq!(doc["dropped_streams"].as_array().unwrap().len(), 0);
    let compat = json(&run(&["--ffprobe-compat"], &path).stdout);
    assert_eq!(compat["format"]["nb_streams"], 3);
    let out = compat["streams"].as_array().unwrap();
    assert_eq!(out.len(), 3);
    // `ffprobe` leaves an unknown stream's media type out, and reads an
    // unnamed private stream (type 0x06) as binary data.
    assert!(out[1].get("codec_type").is_none(), "{}", out[1]);
    assert!(out[1].get("codec_name").is_none(), "{}", out[1]);
    assert_eq!(out[2]["codec_type"], "data");
    assert_eq!(out[2]["codec_name"], "bin_data");
}

/// The listing reads the whole file, so its warnings reach the document;
/// the probe alone reads only the head and the tail.
#[test]
fn the_scan_reports_what_the_listing_met() {
    let mut mux = ts::Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101)]);
    mux.pes(0x101, 0xC0, 90_000, &ts::adts(100), true);
    let mut data = mux.out.clone();
    // A long run without a sync byte, then the stream resumes.
    data.extend(std::iter::repeat_n(0x11u8, 40_000));
    let mut tail = ts::Mux {
        counters: mux.counters.clone(),
        ..ts::Mux::default()
    };
    tail.pes(0x101, 0xC0, 91_920, &ts::adts(100), true);
    data.extend(&tail.out);
    let path = file("lost-sync.ts", &data);
    let scanned = json(&run(&["--scan-frame-sizes"], &path).stdout);
    let warnings = scanned["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w
            .as_str()
            .is_some_and(|w| w.contains("without a sync byte"))),
        "{warnings:?}"
    );
    assert_eq!(scanned["streams"][0]["scan"]["packets"], 2);
}

#[test]
fn the_scan_lists_every_access_unit_of_a_transport_stream() {
    let path = file("scan.ts", &small_ts(0));
    let run = run(&["--scan-frame-sizes"], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_matches(&doc, MPEGTS_SCHEMA);
    assert_eq!(doc["container"]["scanned"], true);
    let scan = &doc["streams"][0]["scan"];
    assert_eq!(scan["packets"], 3);
    assert_eq!(scan["bytes"], 330);
    assert_eq!(scan["first_pts"]["ticks"], 90_000);
    // 1,024 samples at 48 kHz are 1,920 ticks; the third frame follows the
    // second's time.
    assert_eq!(scan["last_pts"]["ticks"], 93_840);
    assert_eq!(scan["span"]["ticks"], 3_840);
}

#[test]
fn transport_stream_compat_output_uses_ffprobe_names() {
    let path = file("compat.ts", &small_ts(0));
    let run = run(&["--ffprobe-compat"], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_eq!(doc["format"]["format_name"], "mpegts");
    assert_eq!(doc["format"]["nb_streams"], 1);
    let audio = &doc["streams"][0];
    assert_eq!(audio["index"], 0);
    assert_eq!(audio["codec_name"], "aac");
    assert_eq!(audio["codec_type"], "audio");
    // No registration descriptor: the stream type as a little-endian code.
    assert_eq!(audio["codec_tag_string"], "[15][0][0][0]");
    assert_eq!(audio["codec_tag"], "0x000f");
    assert_eq!(audio["time_base"], "1/90000");
    assert_eq!(audio["sample_rate"], "48000");
    assert_eq!(audio["channels"], 2);
    assert_eq!(audio["profile"], "LC");
    // A transport stream carries no display size or rotation of its own.
    assert!(audio.get("sample_aspect_ratio").is_none());
    assert!(audio.get("side_data_list").is_none());
}

/// `--allow` drops a stream and counts it in `nb_streams`, as `ffprobe`
/// numbers its streams; `--strict` rejects the file instead.
#[test]
fn allow_and_strict_apply_to_transport_streams() {
    let path = file("allow.ts", &small_ts(0));
    let dropped = run(&["--allow", "h264"], &path);
    assert_eq!(dropped.code, 0, "{}", dropped.stderr);
    let doc = json(&dropped.stdout);
    assert_matches(&doc, MPEGTS_SCHEMA);
    assert_eq!(doc["streams"].as_array().unwrap().len(), 0);
    let dropped_streams = doc["dropped_streams"].as_array().unwrap();
    assert_eq!(dropped_streams.len(), 1);
    assert_eq!(dropped_streams[0]["codec"], "aac");
    assert_eq!(dropped_streams[0]["stream_index"], 0);
    assert_eq!(dropped_streams[0]["reason"], "codec_not_allowed");
    let compat = json(&run(&["--allow", "h264", "--ffprobe-compat"], &path).stdout);
    assert_eq!(compat["format"]["nb_streams"], 1);
    assert_eq!(compat["streams"].as_array().unwrap().len(), 0);
    assert_error(
        &run(&["--allow", "h264", "--strict"], &path),
        1,
        "codec_not_allowed",
    );
    // A name only the transport stream reader knows is accepted.
    assert_eq!(run(&["--allow", "eac3"], &path).code, 0);
    assert_eq!(run(&["--allow", "nope"], &path).code, 2);
}

// ------------------------------------------------------- Matroska and WebM

#[test]
fn matroska_files_print_their_own_schema() {
    let path = file("small.webm", &mkv::small_webm());
    let run = run(&[], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_matroska_schema(&doc);
    assert_eq!(doc["schema"], "coreav.probe-matroska/1");
    assert_eq!(doc["container"]["doc_type"], "webm");
    assert_eq!(doc["container"]["cues"], true);
    assert_eq!(doc["duration"]["ticks"]["num"], 80);
    assert_eq!(doc["duration"]["timescale"], 1000);
    let streams = doc["streams"].as_array().unwrap();
    assert_eq!(streams[0]["codec"], "vp9");
    assert_eq!(streams[0]["video"]["width"], 320);
    assert_eq!(streams[0]["video"]["rotation"], 0);
    assert_eq!(streams[0]["video"]["color"]["matrix"], 1);
    assert_eq!(streams[0]["profile"]["name"], "Profile 0");
    assert_eq!(streams[1]["codec"], "opus");
    assert_eq!(streams[1]["codec_delay_ns"], 6_500_000);
    assert_eq!(streams[1]["audio"]["sample_rate"], 48_000);
}

#[test]
fn the_validator_rejects_a_broken_matroska_document() {
    let path = file("small-validator.webm", &mkv::small_webm());
    let good = json(&run(&[], &path).stdout);
    let root = json(MATROSKA_SCHEMA);
    assert!(validate(&good, &root, &root, "$").is_ok());
    let mut extra = good.clone();
    extra["streams"][0]["surprise"] = Value::Bool(true);
    assert!(validate(&extra, &root, &root, "$").is_err());
    let mut wrong = good;
    wrong["streams"][0]["codec"] = Value::from("h266");
    assert!(validate(&wrong, &root, &root, "$").is_err());
}

#[test]
fn matroska_compat_output_uses_ffprobe_names() {
    let path = file("small-compat.webm", &mkv::small_webm());
    let run = run(&["--ffprobe-compat"], &path);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let doc = json(&run.stdout);
    assert_eq!(doc["format"]["format_name"], "matroska,webm");
    assert_eq!(doc["format"]["duration"], "0.080000");
    let video = &doc["streams"][0];
    assert_eq!(video["codec_tag"], "0x0000");
    assert_eq!(video["codec_tag_string"], "[0][0][0][0]");
    assert_eq!(video["time_base"], "1/1000");
    assert_eq!(video["profile"], "Profile 0");
    assert_eq!(video["color_space"], "bt709");
    assert!(video.get("duration_ts").is_none());
    assert_eq!(doc["streams"][1]["sample_rate"], "48000");
}

#[test]
fn allow_names_cover_both_containers() {
    let webm = file("allow.webm", &mkv::small_webm());
    let opus_only = json(&run(&["--allow", "opus,h264"], &webm).stdout);
    assert_eq!(opus_only["streams"].as_array().unwrap().len(), 1);
    assert_eq!(opus_only["dropped_streams"][0]["codec"], "vp9");
    let strict = run(&["--allow", "opus", "--strict"], &webm);
    assert_error(&strict, 1, "codec_not_allowed");
    // A Matroska-only name narrows an MP4 file's list without an error.
    let mp4 = file("allow-vp8.mp4", &av_movie_with(aac_entry(), false));
    let narrowed = run(&["--allow", "vp8,aac"], &mp4);
    assert_eq!(narrowed.code, 0, "{}", narrowed.stderr);
    assert_eq!(
        json(&narrowed.stdout)["streams"].as_array().unwrap().len(),
        1
    );
    let unknown = run(&["--allow", "vp7"], &webm);
    assert_eq!(unknown.code, 2);
    assert!(unknown.stderr.contains("vp8"), "{}", unknown.stderr);
}

#[test]
fn malformed_matroska_is_rejected_with_a_json_error() {
    let mut data = mkv::small_webm();
    data.truncate(40);
    let error = assert_error(
        &run(&[], &file("cut.webm", &data)),
        1,
        "truncated_structure",
    );
    assert!(error["offset"].is_u64());
    let header_only = mkv::ebml_header("mka");
    assert_error(
        &run(&[], &file("doctype.mka", &header_only)),
        1,
        "unsupported_container",
    );
}

// ------------------------------------------- long output names

/// An output name of exactly 255 bytes, the usual file-name limit: the
/// temporary file next to it must fit as well. Each command writes it, and
/// leaves no temporary file behind. The three share `output::to_file`, so
/// without the fix the first one already fails.
#[test]
fn outputs_with_255_byte_names() {
    let input = file("long_in.mp4", &av_movie_with(aac_entry(), false));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("long_names");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, extension, args) in [
        ("clip", "mp4", vec!["--start", "0"]),
        ("extract-audio", "m4a", vec![]),
        ("remux", "mp4", vec![]),
    ] {
        let out = dir.join(format!(
            "{}.{extension}",
            "a".repeat(255 - 1 - extension.len())
        ));
        assert_eq!(out.file_name().unwrap().len(), 255);
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
        let result = Command::new(bin()).arg(name).args(&all).output().unwrap();
        assert_eq!(
            result.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(&std::fs::read(&out).unwrap()[4..8], b"ftyp", "{name}");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty(), "{name}: {leftovers:?}");
        std::fs::remove_file(&out).unwrap();
    }
}

// ------------------------------------------------- coreav remux (docs/design/remux.md)

fn remux(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("remux")
        .args(args)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

#[test]
fn remux_writes_a_file_and_a_summary() {
    let input = file("remux_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("remux_out.mp4");
    let result = remux(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["schema"], "coreav.remux/1");
    let written = std::fs::read(&out).unwrap();
    assert_eq!(summary["output"]["bytes"], written.len());
    assert_eq!(&written[4..8], b"ftyp");
    let moov = written.windows(4).position(|w| w == b"moov").unwrap();
    let mdat = written.windows(4).position(|w| w == b"mdat").unwrap();
    assert!(moov < mdat);
    assert_eq!(summary["streams"][0]["kind"], "video");
    assert_eq!(summary["streams"][0]["codec"], "h264");
    assert_eq!(summary["streams"][1]["codec"], "aac");
    assert_eq!(summary["streams"][1]["track_id"], 2);
    let probe = run(&[], &out);
    assert_eq!(probe.code, 0, "{}", probe.stderr);
    assert_eq!(json(&probe.stdout)["streams"].as_array().unwrap().len(), 2);
    // One stream only.
    let video_only = output("remux_video_only.mp4");
    let result = remux(&[
        "--streams",
        "0",
        input.to_str().unwrap(),
        video_only.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["streams"].as_array().unwrap().len(), 1);
    assert_eq!(summary["dropped_streams"][0]["reason"], "not_selected");
    assert_eq!(summary["dropped_streams"][0]["message"], Value::Null);
    // Overwriting needs --force.
    let refused = remux(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    let forced = remux(&["--force", input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(forced.code, 0, "{}", forced.stderr);
}

#[test]
fn remux_refuses_its_input_and_keeps_an_existing_output() {
    let source = av_movie_with(aac_entry(), false);
    // Its own directory, where no other test writes temporary files.
    let own = Path::new(env!("CARGO_TARGET_TMPDIR")).join("remux_outputs");
    let _ = std::fs::remove_dir_all(&own);
    std::fs::create_dir_all(&own).unwrap();
    let path = own.join("remux_input.mp4");
    std::fs::write(&path, &source).unwrap();
    // The input itself is refused, with or without --force.
    for args in [vec![], vec!["--force"]] {
        let mut all = args.clone();
        all.extend([path.to_str().unwrap(), path.to_str().unwrap()]);
        let refused = remux(&all);
        assert_eq!(refused.code, 2, "{}", refused.stderr);
        assert_eq!(std::fs::read(&path).unwrap(), source);
    }
    // An existing output is refused without --force; a failure with --force
    // leaves it as it was and no temporary file; a success replaces it.
    let out = own.join("remux_existing.mp4");
    std::fs::write(&out, b"keep me").unwrap();
    let refused = remux(&[path.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    let failed = remux(&[
        "--force",
        "--streams",
        "7",
        path.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(failed.code, 1, "{}", failed.stderr);
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    let leftovers: Vec<_> = std::fs::read_dir(&own)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".coreav-"))
        .collect();
    assert!(leftovers.is_empty());
    let result = remux(&["--force", path.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let written = std::fs::read(&out).unwrap();
    assert_eq!(json(&result.stdout)["output"]["bytes"], written.len());
    assert_eq!(&written[4..8], b"ftyp");
    assert_eq!(run(&[], &out).code, 0);
}

#[test]
fn sanitize_writes_a_file_without_metadata() {
    // The movie with a user data box holding a location.
    let mut source = av_movie_with(aac_entry(), false);
    let udta = bx(b"udta", &bx(b"\xa9xyz", b"\0\x11\0\0+37.5+127.0/"));
    let moov = source.windows(4).position(|w| w == b"moov").unwrap() - 4;
    let size = u32::from_be_bytes(source[moov..moov + 4].try_into().unwrap()) as usize;
    source.splice(moov + size..moov + size, udta.iter().copied());
    source[moov..moov + 4].copy_from_slice(&((size + udta.len()) as u32).to_be_bytes());
    // (The samples come before the movie box, so no offset moves.)
    let input = file("sanitize_in.mp4", &source);
    let remuxed = output("sanitize_copy.mp4");
    let copy = remux(&[input.to_str().unwrap(), remuxed.to_str().unwrap()]);
    assert_eq!(copy.code, 0, "{}", copy.stderr);
    let summary = json(&copy.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["metadata"]["mode"], "copy");
    assert!(
        std::fs::read(&remuxed)
            .unwrap()
            .windows(4)
            .any(|w| w == b"udta")
    );
    let out = output("sanitize_out.mp4");
    let result = Command::new(bin())
        .args(["sanitize", input.to_str().unwrap(), out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    let summary = json(&String::from_utf8(result.stdout).unwrap());
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["metadata"]["mode"], "sanitize");
    assert_eq!(
        summary["metadata"]["keep"],
        serde_json::json!(["rotation", "language", "gapless", "kinds"])
    );
    let location = summary["dropped_boxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["path"] == "moov/udta/\\xa9xyz")
        .unwrap();
    assert_eq!(location["reason"], "sanitized");
    assert_eq!(location["kind"], "location");
    assert!(
        !std::fs::read(&out)
            .unwrap()
            .windows(4)
            .any(|w| w == b"udta")
    );
    // --keep none, and an unknown field.
    let none = output("sanitize_none.mp4");
    let result = Command::new(bin())
        .args([
            "sanitize",
            "--keep",
            "none",
            input.to_str().unwrap(),
            none.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(
        json(&String::from_utf8(result.stdout).unwrap())["metadata"]["keep"],
        serde_json::json!([])
    );
    // Matroska and WebM are remuxed but not sanitized.
    let webm = file("sanitize_in.webm", &mkv::small_webm());
    let webm_out = output("sanitize_webm.mp4");
    let refused = Command::new(bin())
        .args([
            "sanitize",
            webm.to_str().unwrap(),
            webm_out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(!webm_out.exists());
    let bad = Command::new(bin())
        .args([
            "sanitize",
            "--keep",
            "title",
            input.to_str().unwrap(),
            output("sanitize_bad.mp4").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
}

#[test]
fn remux_to_standard_output() {
    let input = file("remux_stdout_in.mp4", &av_movie_with(aac_entry(), false));
    let out = Command::new(bin())
        .args(["remux", input.to_str().unwrap(), "-"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(&out.stdout[4..8], b"ftyp");
    let summary = json(&String::from_utf8(out.stderr).unwrap());
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["output"]["bytes"], out.stdout.len());
}

#[test]
fn a_failed_remux_leaves_no_file() {
    let out = output("remux_fail_out.mp4");
    let junk = file("remux_junk.mp4", b"not a movie");
    let result = remux(&[junk.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    assert!(!out.exists());
    // A stream that cannot be copied, with --fail-on-unsupported.
    let input = file("remux_fail_in.mp4", &av_movie_with(aac_entry(), false));
    let result = remux(&[
        "--streams",
        "7",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    assert!(!out.exists());
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("remux_missing.mp4");
    let result = remux(&[missing.to_str().unwrap(), out.to_str().unwrap()]);
    assert_error(&result, 3, "io");
    assert!(!out.exists());
}

#[test]
fn remux_usage_errors_exit_2() {
    let input = file("remux_usage_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("remux_usage_out.mp4");
    for args in [
        vec!["--streams", "x"],
        vec!["--streams", "-1"],
        vec!["--allow", "h264,nope"],
    ] {
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), out.to_str().unwrap()]);
        let result = remux(&all);
        assert_eq!(result.code, 2, "{args:?}: {}", result.stderr);
        assert!(!out.exists());
    }
}

#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh or gen-fixtures-mkv.sh); set COREAV_FIXTURES"]
fn fixture_remuxes_match_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut remuxed = 0;
    let mut candidates = 0;
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .filter(|name| {
            [".mp4", ".mov", ".m4a", ".mkv", ".webm"]
                .iter()
                .any(|ext| name.ends_with(ext))
        })
    {
        candidates += 1;
        let out = output(&format!("fixture_remux_{name}.mp4"));
        let input = dir.join(name);
        let result = remux(&[input.to_str().unwrap(), out.to_str().unwrap()]);
        match result.code {
            0 => {
                let summary = json(&result.stdout);
                assert_matches(&summary, REMUX_SCHEMA);
                assert_eq!(
                    summary["output"]["bytes"],
                    std::fs::metadata(&out).unwrap().len(),
                    "{name}"
                );
                remuxed += 1;
            }
            1 => {
                let error = json(&result.stderr);
                assert_eq!(error["schema"], "coreav.error/1", "{name}");
                assert!(!out.exists(), "{name}: a failed remux left a file");
            }
            other => panic!("{name}: exit code {other}: {}", result.stderr),
        }
    }
    // A fixture set without MP4, MOV or M4A files (the Matroska fixtures)
    // has nothing to remux.
    assert!(candidates == 0 || remuxed > 0, "no fixture was remuxed");
}

#[test]
fn remux_webm_to_mp4() {
    let input = file("remux_in.webm", &mkv::small_webm());
    let out = output("remux_webm_out.mp4");
    let result = remux(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    let written = std::fs::read(&out).unwrap();
    assert_eq!(summary["output"]["bytes"], written.len());
    assert_eq!(&written[4..8], b"ftyp");
    assert_eq!(summary["source"]["moov_first"], false);
    assert_eq!(summary["source"]["truncated_at"], Value::Null);
    assert_eq!(summary["streams"][0]["codec"], "vp9");
    assert_eq!(summary["streams"][0]["source_track_id"], 1);
    assert_eq!(summary["streams"][1]["codec"], "opus");
    assert_eq!(summary["streams"][1]["samples"], 2);
    // The output is an MP4 file with both streams.
    let probe = run(&[], &out);
    assert_eq!(probe.code, 0, "{}", probe.stderr);
    let doc = json(&probe.stdout);
    assert_eq!(doc["schema"], "coreav.probe/1");
    assert_eq!(doc["streams"].as_array().unwrap().len(), 2);
    // Codecs outside --allow are dropped and reported.
    let video_only = output("remux_webm_video_only.mp4");
    let result = remux(&[
        "--allow",
        "vp9",
        input.to_str().unwrap(),
        video_only.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["dropped_streams"][0]["stream_index"], 1);
    assert_eq!(summary["dropped_streams"][0]["reason"], "codec_not_allowed");
    // A cut-off recording: every complete frame, and where it ends.
    let mut cut = mkv::small_webm();
    let cues = cut
        .windows(4)
        .rposition(|w| w == [0x1C, 0x53, 0xBB, 0x6B])
        .unwrap();
    cut.truncate(cues - 2);
    let input = file("remux_cut.webm", &cut);
    let cut_out = output("remux_cut.mp4");
    let result = remux(&[input.to_str().unwrap(), cut_out.to_str().unwrap()]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    assert!(summary["source"]["truncated_at"].is_u64());
    assert_eq!(summary["streams"][0]["samples"], 1);
}

#[test]
fn remux_of_a_webm_without_mp4_streams_fails() {
    const VP8_REASON: &str = "VP8 in MP4 does not play in Safari; keep the WebM, which plays in \
                              Chrome, Firefox and Safari, or re-encode to VP9, H.264 or AV1 \
                              outside CoreAV";
    let mut data = mkv::small_webm();
    // V_VP9 becomes V_VP8, A_OPUS stays: --streams names the VP8 stream.
    let at = data.windows(5).position(|w| w == b"V_VP9").unwrap();
    data[at + 4] = b'8';
    let input = file("remux_vp8.webm", &data);
    let out = output("remux_vp8.mp4");
    let result = remux(&[
        "--streams",
        "0",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    let error = json(&result.stderr);
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["message"], format!("unsupported: {VP8_REASON}"));
    assert!(!out.exists());
    // Without --streams the remux still fails: the video would be lost.
    let result = remux(&[input.to_str().unwrap(), out.to_str().unwrap()]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    let error = json(&result.stderr);
    assert_eq!(error["kind"], "unsupported_feature");
    assert_eq!(
        error["message"],
        format!(
            "unsupported: video stream 0 cannot be copied to MP4: {VP8_REASON}; \
             --drop-unsupported writes the other streams without it"
        )
    );
    assert!(!out.exists());
    // It cannot be combined with --fail-on-unsupported.
    let result = remux(&[
        "--drop-unsupported",
        "--fail-on-unsupported",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 2, "{}", result.stderr);
    // With --drop-unsupported the audio is written and the VP8 stream is
    // reported with its reason.
    let result = remux(&[
        "--drop-unsupported",
        input.to_str().unwrap(),
        out.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, REMUX_SCHEMA);
    assert_eq!(summary["dropped_streams"][0]["reason"], "unsupported");
    assert_eq!(summary["dropped_streams"][0]["message"], VP8_REASON);
}

// ------------------------------------------------ coreav split (docs/design/split.md)

fn split(args: &[&str]) -> Run {
    let out = Command::new(bin())
        .arg("split")
        .args(args)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// A fresh directory under Cargo's per-test temporary directory.
fn directory(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn split_writes_segments_and_a_summary() {
    // Four video samples of 40 ms, every one a sync sample.
    let input = file("split_in.mp4", &av_movie_with(aac_entry(), false));
    let dir = directory("split_out");
    let pattern = dir.join("seg-%03d.mp4");
    let result = split(&[
        "--every",
        "0.08",
        input.to_str().unwrap(),
        pattern.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, SPLIT_SCHEMA);
    assert_eq!(summary["schema"], "coreav.split/1");
    assert_eq!(listing(&dir), ["seg-000.mp4", "seg-001.mp4"]);
    let segments = summary["segments"].as_array().unwrap();
    let mut samples = 0;
    for (index, segment) in segments.iter().enumerate() {
        assert_eq!(segment["index"], index);
        let path = PathBuf::from(segment["output"]["path"].as_str().unwrap());
        assert_eq!(path, dir.join(format!("seg-{index:03}.mp4")));
        assert_eq!(
            segment["output"]["bytes"],
            std::fs::metadata(&path).unwrap().len()
        );
        assert_eq!(segment["streams"][0]["first_sample"], samples);
        samples += segment["streams"][0]["samples"].as_u64().unwrap();
        // Each segment is a valid movie.
        let probe = run(&[], &path);
        assert_eq!(probe.code, 0, "{}", probe.stderr);
    }
    assert_eq!(samples, 4);
    assert_eq!(segments[1]["video_start"]["seconds"], 0.08);
    // At given times, without audio.
    let dir = directory("split_at");
    let result = split(&[
        "--at",
        "0.04,0.1",
        "--audio",
        "none",
        input.to_str().unwrap(),
        dir.join("%d.mp4").to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, SPLIT_SCHEMA);
    assert_eq!(listing(&dir), ["0.mp4", "1.mp4", "2.mp4"]);
    assert_eq!(summary["left_out"][0]["reason"], "not_selected");
}

#[test]
fn split_never_overwrites_without_force() {
    let input = file("split_force_in.mp4", &av_movie_with(aac_entry(), false));
    let dir = directory("split_force");
    std::fs::write(dir.join("seg-1.mp4"), b"keep me").unwrap();
    let pattern = dir.join("seg-%d.mp4");
    let args = [
        "--every",
        "0.08",
        input.to_str().unwrap(),
        pattern.to_str().unwrap(),
    ];
    let refused = split(&args);
    assert_eq!(refused.code, 2, "{}", refused.stderr);
    assert!(
        refused.stderr.contains("seg-1.mp4 exists"),
        "{}",
        refused.stderr
    );
    // The segment written before the refusal is removed.
    assert_eq!(listing(&dir), ["seg-1.mp4"]);
    assert_eq!(std::fs::read(dir.join("seg-1.mp4")).unwrap(), b"keep me");
    let mut forced = vec!["--force"];
    forced.extend(args);
    let result = split(&forced);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(listing(&dir), ["seg-0.mp4", "seg-1.mp4"]);
    assert_ne!(std::fs::read(dir.join("seg-1.mp4")).unwrap(), b"keep me");
    // Even with --force, a segment is never written over the input.
    let own = dir.join("0.mp4");
    std::fs::copy(&input, &own).unwrap();
    let before = std::fs::read(&own).unwrap();
    let result = split(&[
        "--force",
        "--every",
        "0.08",
        own.to_str().unwrap(),
        dir.join("%d.mp4").to_str().unwrap(),
    ]);
    assert_eq!(result.code, 2, "{}", result.stderr);
    assert!(result.stderr.contains("is the input"), "{}", result.stderr);
    assert_eq!(std::fs::read(&own).unwrap(), before);
}

#[test]
fn a_failed_split_leaves_no_file() {
    let dir = directory("split_fail");
    let pattern = dir.join("seg-%d.mp4");
    let junk = file("split_junk.mp4", b"not a movie");
    let result = split(&[
        "--every",
        "1",
        junk.to_str().unwrap(),
        pattern.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 1, "{}", result.stderr);
    assert!(listing(&dir).is_empty());
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("split_missing.mp4");
    let result = split(&[
        "--every",
        "1",
        missing.to_str().unwrap(),
        pattern.to_str().unwrap(),
    ]);
    assert_error(&result, 3, "io");
    assert!(listing(&dir).is_empty());
}

#[test]
fn split_usage_errors_exit_2() {
    let input = file("split_usage_in.mp4", &av_movie_with(aac_entry(), false));
    let dir = directory("split_usage");
    let pattern = dir.join("seg-%d.mp4");
    for (args, output) in [
        (vec![], pattern.to_str().unwrap()),
        (vec!["--every", "1", "--at", "1"], pattern.to_str().unwrap()),
        (vec!["--every", "0"], pattern.to_str().unwrap()),
        (vec!["--every", "-1"], pattern.to_str().unwrap()),
        (vec!["--at", "2,1"], pattern.to_str().unwrap()),
        (vec!["--at", "1,1"], pattern.to_str().unwrap()),
        (vec!["--at", ""], pattern.to_str().unwrap()),
        (
            vec!["--every", "1", "--audio", "first"],
            pattern.to_str().unwrap(),
        ),
        (vec!["--every", "1"], "seg.mp4"),
        (vec!["--every", "1"], "seg-%s.mp4"),
        (vec!["--every", "1"], "seg-%d-%d.mp4"),
    ] {
        let mut all = args.clone();
        all.extend([input.to_str().unwrap(), output]);
        let result = split(&all);
        assert_eq!(result.code, 2, "{args:?} {output}: {}", result.stderr);
        assert!(listing(&dir).is_empty());
    }
}

#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixture_splits_match_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut split_files = 0;
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .filter(|name| name.ends_with(".mp4") || name.ends_with(".mov"))
    {
        let out = directory(&format!("fixture_split_{name}"));
        let input = dir.join(name);
        let result = split(&[
            "--every",
            "0.5",
            input.to_str().unwrap(),
            out.join("%d.mp4").to_str().unwrap(),
        ]);
        match result.code {
            0 => {
                let summary = json(&result.stdout);
                assert_matches(&summary, SPLIT_SCHEMA);
                let segments = summary["segments"].as_array().unwrap();
                assert_eq!(listing(&out).len(), segments.len(), "{name}");
                split_files += 1;
            }
            1 => {
                let error = json(&result.stderr);
                assert_eq!(error["schema"], "coreav.error/1", "{name}");
                assert!(
                    listing(&out).is_empty(),
                    "{name}: a failed split left a file"
                );
            }
            other => panic!("{name}: exit code {other}: {}", result.stderr),
        }
    }
    assert!(split_files > 0, "no fixture was split");
}

/// Every command, with the arguments that make it succeed: the commands that
/// print a document, and the writing commands with a file output (their
/// summary goes to standard output). Files are named after `test`, since
/// tests run in parallel.
fn succeeding_commands(test: &str) -> Vec<Vec<String>> {
    let movie = file(
        &format!("{test}_in.mp4"),
        &av_movie_with(aac_entry(), false),
    );
    let text = file(&format!("{test}_text.mp4"), &text_movie_bytes());
    let movie = movie.to_str().unwrap().to_owned();
    let text = text.to_str().unwrap().to_owned();
    let policy = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/schema/policy-examples/web-upload.json"
    );
    let out = |name: &str| {
        output(&format!("{test}{name}"))
            .to_str()
            .unwrap()
            .to_owned()
    };
    // `hls get` reads an index built here; `hls package` needs a new
    // directory. This movie's audio is not one HLS packages.
    let index = out(".cavhls");
    let built = Command::new(bin())
        .args(["hls", "index", "--audio", "none", &movie, &index])
        .output()
        .unwrap();
    assert_eq!(built.status.code(), Some(0));
    let package = out("_hls");
    let _ = std::fs::remove_dir_all(&package);
    let commands: [&[&str]; 14] = [
        &["--help"],
        &["--version"],
        &["probe", &movie],
        &["chapters", &text],
        &["check", "--policy", policy, &movie],
        &["remux", "--force", &movie, &out(".mp4")],
        &["sanitize", "--force", &movie, &out("_s.mp4")],
        &["clip", "--force", "--start", "0", &movie, &out("_c.mp4")],
        &["extract-audio", "--force", &movie, &out(".m4a")],
        &["extract-subtitles", "--force", &text, &out(".vtt")],
        &[
            "split",
            "--force",
            "--every",
            "1",
            &movie,
            &out("_segment%d.mp4"),
        ],
        &[
            "hls",
            "index",
            "--audio",
            "none",
            "--force",
            &movie,
            &out("_2.cavhls"),
        ],
        &["hls", "get", &index, &movie, "master.m3u8", "-"],
        &["hls", "package", "--audio", "none", &movie, &package],
    ];
    commands
        .iter()
        .map(|c| c.iter().map(|a| (*a).to_owned()).collect())
        .collect()
}

#[test]
fn every_command_succeeds_on_a_working_standard_output() {
    for args in succeeding_commands("stdout_ok") {
        let out = Command::new(bin()).args(&args).output().unwrap();
        let code = out.status.code();
        // `check` exits 1 for a rejected file; this one is accepted or not,
        // but its verdict was written.
        assert!(
            code == Some(0) || (args[0] == "check" && code == Some(1)),
            "{args:?}: {code:?}"
        );
        assert!(!out.stdout.is_empty(), "{args:?}");
    }
}

/// A document that cannot be written (a full device) is an I/O error, never
/// a success.
#[cfg(target_os = "linux")]
#[test]
fn a_full_standard_output_exits_3() {
    for args in succeeding_commands("stdout_full") {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap();
        let out = Command::new(bin())
            .args(&args)
            .stdout(full)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3), "{args:?}");
        let error = json(&String::from_utf8(out.stderr).unwrap());
        assert_eq!(error["kind"], "io", "{args:?}");
        assert_eq!(
            error["message"], "write error: no storage space",
            "{args:?}"
        );
    }
    // The summary of an output written to standard output goes to standard
    // error; a summary that cannot be written is a failure too.
    let movie = file(
        "stdout_errors_summary.mp4",
        &av_movie_with(aac_entry(), false),
    );
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let out = Command::new(bin())
        .args(["remux", movie.to_str().unwrap(), "-"])
        .stderr(full)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(!out.stdout.is_empty());
}

/// Media written to `-` go through a duplicate of the standard output
/// descriptor rather than `Stdout`'s line buffer: the bytes are the
/// ones written to a file, whether standard output is a pipe or a file.
#[test]
fn media_on_standard_output_are_the_file_bytes() {
    let movie = file("stdout_same_in.mp4", &av_movie_with(aac_entry(), false));
    let text = file("stdout_same_text.mp4", &text_movie_bytes());
    let (movie, text) = (movie.to_str().unwrap(), text.to_str().unwrap());
    let index = output("stdout_same.cavhls");
    let index = index.to_str().unwrap();
    let built = hls(&["index", "--audio", "none", movie, index]);
    assert_eq!(built.code, 0, "{}", built.stderr);
    // `OUT` stands for the output argument.
    let commands: [&[&str]; 9] = [
        &["remux", movie, "OUT"],
        &["sanitize", movie, "OUT"],
        &["clip", "--start", "0", movie, "OUT"],
        &["extract-audio", movie, "OUT"],
        &["extract-subtitles", text, "OUT"],
        &["hls", "index", "--audio", "none", movie, "OUT"],
        &["hls", "get", index, movie, "video-init.mp4", "OUT"],
        &["hls", "get", index, movie, "video-0.m4s", "OUT"],
        &["join", "-o", "OUT", movie, movie],
    ];
    for (n, command) in commands.iter().enumerate() {
        let with = |out: &str| -> Vec<String> {
            command
                .iter()
                .map(|a| if *a == "OUT" { out } else { a }.to_owned())
                .collect()
        };
        let to_file = output(&format!("stdout_same_{n}.out"));
        let done = Command::new(bin())
            .args(with(to_file.to_str().unwrap()))
            .output()
            .unwrap();
        assert_eq!(done.status.code(), Some(0), "{command:?}");
        let expected = std::fs::read(&to_file).unwrap();
        assert!(!expected.is_empty(), "{command:?}");
        // Standard output a pipe.
        let piped = Command::new(bin()).args(with("-")).output().unwrap();
        assert_eq!(piped.status.code(), Some(0), "{command:?}");
        assert!(piped.stdout == expected, "{command:?}: pipe");
        // Standard output a file.
        let redirected = output(&format!("stdout_same_{n}.redirected"));
        let done = Command::new(bin())
            .args(with("-"))
            .stdout(std::fs::File::create(&redirected).unwrap())
            .output()
            .unwrap();
        assert_eq!(done.status.code(), Some(0), "{command:?}");
        assert!(
            std::fs::read(&redirected).unwrap() == expected,
            "{command:?}: redirected"
        );
    }
}

/// A standard output whose reader is gone: a socket whose other end is
/// closed fails every write with `EPIPE`, like a pipe after `head` exits,
/// without depending on when the reader exits.
#[cfg(unix)]
fn closed_reader() -> std::process::Stdio {
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    drop(theirs);
    std::process::Stdio::from(std::os::fd::OwnedFd::from(ours))
}

/// A closed pipe ends the command with 141, as `SIGPIPE` would, without a
/// message.
#[cfg(unix)]
#[test]
fn a_closed_standard_output_exits_141_quietly() {
    let mut commands = succeeding_commands("stdout_pipe");
    let movie = file("stdout_errors_pipe.mp4", &av_movie_with(aac_entry(), false));
    let movie = movie.to_str().unwrap().to_owned();
    for name in ["remux", "sanitize", "extract-audio"] {
        commands.push(vec![name.to_owned(), movie.clone(), "-".to_owned()]);
    }
    commands.push(
        ["clip", "--start", "0", &movie, "-"]
            .iter()
            .map(|a| (*a).to_owned())
            .collect(),
    );
    // HLS: the index, and a media segment copied from the source's ranges.
    let index = output("stdout_errors_pipe.cavhls");
    let index = index.to_str().unwrap().to_owned();
    let built = hls(&["index", "--audio", "none", &movie, &index]);
    assert_eq!(built.code, 0, "{}", built.stderr);
    for args in [
        vec!["hls", "index", "--audio", "none", &movie, "-"],
        vec!["hls", "get", &index, &movie, "video-0.m4s", "-"],
    ] {
        commands.push(args.iter().map(|a| (*a).to_owned()).collect());
    }
    for args in commands {
        let out = Command::new(bin())
            .args(&args)
            .stdout(closed_reader())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(141), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "", "{args:?}");
    }
}

// ------------------------------------------------- coreav hls (docs/design/hls.md)

fn hls(args: &[&str]) -> Run {
    let out = Command::new(bin()).arg("hls").args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// A fresh path, with nothing at it, under Cargo's per-test temporary directory.
fn fresh_path(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&path);
    path
}

#[test]
fn hls_package_index_and_get_agree() {
    // The synthetic AAC entry has no decoder configuration, so no codec
    // string: package the video alone.
    let input = file("hls_in.mp4", &av_movie_with(aac_entry(), false));
    let dir = fresh_path("hls_package");
    let input_arg = input.to_str().unwrap();
    let result = hls(&[
        "package",
        "--audio",
        "none",
        input_arg,
        dir.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, HLS_INDEX_SCHEMA);
    assert_eq!(summary["renditions"][0]["rendition"], "video");
    assert_eq!(summary["renditions"][0]["codecs"], "avc1.64001E");
    for name in [
        "master.m3u8",
        "video.m3u8",
        "video-init.mp4",
        "video-0.m4s",
        "index.cavhls",
    ] {
        assert!(dir.join(name).exists(), "{name}");
    }
    let master = std::fs::read_to_string(dir.join("master.m3u8")).unwrap();
    assert!(master.starts_with("#EXTM3U\n"));
    // `get` cuts the same bytes from the index the package wrote.
    let index = dir.join("index.cavhls");
    let index_arg = index.to_str().unwrap();
    let segment = output("hls_segment.m4s");
    let result = hls(&[
        "get",
        index_arg,
        input_arg,
        "video-0.m4s",
        segment.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(
        std::fs::read(&segment).unwrap(),
        std::fs::read(dir.join("video-0.m4s")).unwrap()
    );
    // A separately built index is the same.
    let rebuilt = output("hls_rebuilt.cavhls");
    let result = hls(&[
        "index",
        "--audio",
        "none",
        input_arg,
        rebuilt.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(
        std::fs::read(&rebuilt).unwrap(),
        std::fs::read(&index).unwrap()
    );
    // Never overwritten without --force.
    let result = hls(&[
        "index",
        "--audio",
        "none",
        input_arg,
        rebuilt.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 2);
    // A non-empty package directory is refused.
    let result = hls(&[
        "package",
        "--audio",
        "none",
        input_arg,
        dir.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 2);
}

#[test]
fn hls_errors_have_their_codes() {
    let input = file("hls_err_in.mp4", &av_movie_with(aac_entry(), false));
    let input_arg = input.to_str().unwrap();
    // The default audio has no readable codec configuration: rejected.
    let result = hls(&[
        "index",
        input_arg,
        output("hls_err.cavhls").to_str().unwrap(),
    ]);
    assert_error(&result, 1, "unsupported_feature");
    // Not an index.
    let result = hls(&["get", input_arg, input_arg, "master.m3u8", "-"]);
    assert_error(&result, 1, "invalid_index");
    // Unknown resource names and bad options are usage errors.
    let index = output("hls_ok.cavhls");
    let result = hls(&[
        "index",
        "--audio",
        "none",
        input_arg,
        index.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let index_arg = index.to_str().unwrap();
    for name in ["video-01.m4s", "../video.m3u8", "subtitles.m3u8"] {
        assert_eq!(
            hls(&["get", index_arg, input_arg, name, "-"]).code,
            2,
            "{name}"
        );
    }
    let result = hls(&["index", "--segment-duration", "0", input_arg, "-"]);
    assert_eq!(result.code, 2);
    let result = hls(&[
        "index", "--video", "none", "--audio", "none", input_arg, "-",
    ]);
    assert_eq!(result.code, 2);
    // A segment past the end, and a rendition the index lacks.
    let result = hls(&["get", index_arg, input_arg, "video-99.m4s", "-"]);
    assert_eq!(result.code, 1);
    let result = hls(&["get", index_arg, input_arg, "audio-init.mp4", "-"]);
    assert_eq!(result.code, 1);
    // The playlist goes to standard output.
    let result = hls(&["get", index_arg, input_arg, "video.m3u8", "-"]);
    assert_eq!(result.code, 0);
    assert!(result.stdout.contains("#EXT-X-MAP:URI=\"video-init.mp4\""));
}

/// Found by review: a failed request never destroys or truncates files.
#[test]
fn hls_failures_leave_files_alone() {
    let source = av_movie_with(aac_entry(), false);
    let input = file("hls_keep_in.mp4", &source);
    let input_arg = input.to_str().unwrap();
    let index = output("hls_keep.cavhls");
    let index_arg = index.to_str().unwrap();
    let result = hls(&["index", "--audio", "none", input_arg, index_arg]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    // A source of another length: the existing output stays as it was.
    let shorter = file("hls_keep_short.mp4", &source[..source.len() - 1]);
    let out = output("hls_keep_out.m4s");
    std::fs::write(&out, b"keep me").unwrap();
    let result = hls(&[
        "get",
        "--force",
        index_arg,
        shorter.to_str().unwrap(),
        "video-0.m4s",
        out.to_str().unwrap(),
    ]);
    assert_error(&result, 1, "source_mismatch");
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    // The input or the index as the output is refused.
    for target in [input_arg, index_arg] {
        let result = hls(&[
            "get",
            "--force",
            index_arg,
            input_arg,
            "video-0.m4s",
            target,
        ]);
        assert_eq!(result.code, 2);
    }
    assert_eq!(std::fs::read(&input).unwrap(), source);
    // A rejected input leaves no package directory behind.
    let dir = fresh_path("hls_keep_package");
    let result = hls(&["package", input_arg, dir.to_str().unwrap()]);
    assert_eq!(result.code, 1);
    assert!(!dir.exists());
    let parent = dir.parent().unwrap();
    assert!(
        std::fs::read_dir(parent).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("partial"))
    );
}

/// `-` is standard output to the other writing commands; a package cannot
/// be one stream, so it is a usage error and no directory named `-` is made.
/// `./-` still names such a directory.
#[test]
fn hls_package_refuses_standard_output() {
    let input = file("hls_dash_in.mp4", &av_movie_with(aac_entry(), false));
    let cwd = fresh_path("hls_dash_cwd");
    std::fs::create_dir(&cwd).unwrap();
    let package = |dir: &str| {
        Command::new(bin())
            .current_dir(&cwd)
            .args(["hls", "package", "--audio", "none"])
            .arg(&input)
            .arg(dir)
            .output()
            .unwrap()
    };
    let out = package("-");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("must be a directory"));
    assert!(out.stdout.is_empty());
    assert!(!cwd.join("-").exists());
    let out = package("./-");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(cwd.join("-").join("master.m3u8").exists());
}

/// Found by review: an existing empty directory, a symlink to one included,
/// is packaged in place and kept.
#[test]
fn hls_package_into_an_existing_empty_directory() {
    let input = file("hls_dir_in.mp4", &av_movie_with(aac_entry(), false));
    let input_arg = input.to_str().unwrap();
    let real = fresh_path("hls_dir_real");
    std::fs::create_dir(&real).unwrap();
    let link = fresh_path("hls_dir_link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let result = hls(&[
        "package",
        "--audio",
        "none",
        input_arg,
        link.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert!(link.is_symlink());
    assert!(real.join("master.m3u8").exists());
    // A rejected input leaves the existing directory empty, and in place.
    let empty = fresh_path("hls_dir_empty");
    std::fs::create_dir(&empty).unwrap();
    let result = hls(&["package", input_arg, empty.to_str().unwrap()]);
    assert_eq!(result.code, 1);
    assert!(empty.is_dir());
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
}

/// `-` writes the index on standard output and the summary on standard
/// error, as for the other commands.
#[test]
fn hls_index_to_standard_output() {
    let input = file("hls_stdout_in.mp4", &av_movie_with(aac_entry(), false));
    let input_arg = input.to_str().unwrap();
    let out = Command::new(bin())
        .args(["hls", "index", "--audio", "none", input_arg, "-"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.starts_with(b"CAVHLSIX"));
    let summary = json(&String::from_utf8(out.stderr).unwrap());
    assert_matches(&summary, HLS_INDEX_SCHEMA);
    assert_eq!(summary["index"]["bytes"], out.stdout.len());
    // The same bytes as an index written to a file.
    let path = output("hls_stdout.cavhls");
    let result = hls(&[
        "index",
        "--audio",
        "none",
        input_arg,
        path.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_eq!(std::fs::read(&path).unwrap(), out.stdout);
}

#[test]
fn sanitize_takes_its_input_then_its_output() {
    // Its arguments are remux's with some help replaced; the replacement must
    // not reorder them, and the hidden --drop-unsupported stays accepted.
    let input = file("sanitize_order_in.mp4", &av_movie_with(aac_entry(), false));
    let before = std::fs::read(&input).unwrap();
    let out = output("sanitize_order_out.mp4");
    let result = Command::new(bin())
        .args(["sanitize", "--drop-unsupported"])
        .arg(&input)
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(&std::fs::read(&out).unwrap()[4..8], b"ftyp");
    assert_eq!(std::fs::read(&input).unwrap(), before);
    let help = Command::new(bin())
        .args(["sanitize", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("<INPUT> <OUTPUT>"), "{help}");
    assert!(!help.contains("Matroska"), "{help}");
}

// -------------------------------------------------- coreav join (docs/design/join.md)

fn join(args: &[&str]) -> Run {
    let out = Command::new(bin()).arg("join").args(args).output().unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

#[test]
fn split_then_join_gives_back_the_samples() {
    let input = file("join_src.mp4", &av_movie_with(aac_entry(), false));
    let dir = directory("join_parts");
    let result = split(&[
        "--every",
        "0.04",
        input.to_str().unwrap(),
        dir.join("%d.mp4").to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let parts: Vec<String> = (0..4)
        .map(|n| dir.join(format!("{n}.mp4")).to_string_lossy().into_owned())
        .collect();
    let out = output("join_out.mp4");
    let mut args = vec!["-o", out.to_str().unwrap()];
    args.extend(parts.iter().map(String::as_str));
    let result = join(&args);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let summary = json(&result.stdout);
    assert_matches(&summary, JOIN_SCHEMA);
    assert_eq!(summary["schema"], "coreav.join/1");
    assert_eq!(summary["inputs"].as_array().unwrap().len(), 4);
    assert_eq!(summary["edits"]["video"], 1);
    assert_eq!(
        summary["output"]["bytes"],
        std::fs::metadata(&out).unwrap().len()
    );
    let probe = run(&[], &out);
    assert_eq!(probe.code, 0, "{}", probe.stderr);
    // A join refuses to overwrite and leaves no file when it fails.
    let again = join(&args);
    assert_eq!(again.code, 2, "{}", again.stderr);
    let junk = file("join_junk.mp4", b"not a movie");
    let failed_out = output("join_failed.mp4");
    let result = join(&[
        "-o",
        failed_out.to_str().unwrap(),
        parts[0].as_str(),
        junk.to_str().unwrap(),
    ]);
    let error = assert_error(&result, 1, "unsupported_container");
    assert_eq!(error["input"], 1);
    assert!(!failed_out.exists());
}

#[test]
fn incompatible_inputs_name_the_input() {
    let with_audio = file("join_av.mp4", &av_movie_with(aac_entry(), false));
    let video_only_movie = {
        // The same movie without its audio: drop the second `trak`.
        let data = av_movie_with(aac_entry(), false);
        let moov = data.windows(4).position(|w| w == b"moov").unwrap() - 4;
        let second = data[moov + 8..]
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"trak")
            .map(|(i, _)| moov + 8 + i - 4)
            .nth(1)
            .unwrap();
        let size = u32::from_be_bytes(data[second..second + 4].try_into().unwrap()) as usize;
        let mut out = data.clone();
        out.drain(second..second + size);
        let moov_size = u32::from_be_bytes(out[moov..moov + 4].try_into().unwrap()) as usize - size;
        out[moov..moov + 4].copy_from_slice(&(moov_size as u32).to_be_bytes());
        out
    };
    let video_only = file("join_v.mp4", &video_only_movie);
    let out = output("join_mismatch.mp4");
    let result = join(&[
        "-o",
        out.to_str().unwrap(),
        with_audio.to_str().unwrap(),
        video_only.to_str().unwrap(),
    ]);
    let error = assert_error(&result, 1, "incompatible_inputs");
    assert_eq!(error["input"], 1);
    assert!(
        error["message"].as_str().unwrap().contains("input 1"),
        "{error}"
    );
    assert!(!out.exists());
    // Without audio they join.
    let result = join(&[
        "--audio",
        "none",
        "-o",
        out.to_str().unwrap(),
        with_audio.to_str().unwrap(),
        video_only.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 0, "{}", result.stderr);
    assert_matches(&json(&result.stdout), JOIN_SCHEMA);
}

#[test]
fn join_usage_errors_exit_2() {
    let input = file("join_usage_in.mp4", &av_movie_with(aac_entry(), false));
    let out = output("join_usage_out.mp4");
    for args in [
        vec![input.to_str().unwrap()],
        vec!["-o", out.to_str().unwrap()],
        vec![
            "--audio",
            "first",
            "-o",
            out.to_str().unwrap(),
            input.to_str().unwrap(),
        ],
        vec![
            "--allow",
            "nope",
            "-o",
            out.to_str().unwrap(),
            input.to_str().unwrap(),
        ],
    ] {
        let result = join(&args);
        assert_eq!(result.code, 2, "{args:?}: {}", result.stderr);
        assert!(!out.exists());
    }
}

#[test]
fn join_never_writes_over_an_input() {
    let data = av_movie_with(aac_entry(), false);
    let a = file("join_self_a.mp4", &data);
    let b = file("join_self_b.mp4", &data);
    // The output names the second input; --force does not allow it.
    let result = join(&[
        "--force",
        "-o",
        b.to_str().unwrap(),
        a.to_str().unwrap(),
        b.to_str().unwrap(),
    ]);
    assert_eq!(result.code, 2, "{}", result.stderr);
    assert!(
        result.stderr.contains("is the input file"),
        "{}",
        result.stderr
    );
    assert_eq!(std::fs::read(&b).unwrap(), data);
}

#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixture_joins_match_the_schema() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut joined = 0;
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
        .filter(|name| name.ends_with(".mp4") || name.ends_with(".mov"))
    {
        let out = output(&format!("fixture_join_{name}.mp4"));
        let input = dir.join(name);
        let result = join(&[
            "-o",
            out.to_str().unwrap(),
            input.to_str().unwrap(),
            input.to_str().unwrap(),
        ]);
        match result.code {
            0 => {
                let summary = json(&result.stdout);
                assert_matches(&summary, JOIN_SCHEMA);
                assert_eq!(
                    summary["output"]["bytes"],
                    std::fs::metadata(&out).unwrap().len(),
                    "{name}"
                );
                joined += 1;
            }
            1 => {
                let error = json(&result.stderr);
                assert_eq!(error["schema"], "coreav.error/1", "{name}");
                assert!(error.get("input").is_some(), "{name}");
                assert!(!out.exists(), "{name}: a failed join left a file");
            }
            other => panic!("{name}: exit code {other}: {}", result.stderr),
        }
    }
    assert!(joined > 0, "no fixture was joined");
}
