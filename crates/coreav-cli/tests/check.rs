//! `coreav check` end to end: verdicts against `docs/schema/verdict-v1.schema.json`,
//! reasons that point at the probe output, unreadable files, policy errors,
//! exit codes, the example policies and determinism.
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
mod schema;

use common::{aac_entry, av_movie_with, bx, fragmented_movie};
use schema::{assert_matches, validate};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const VERDICT_SCHEMA: &str = include_str!("../../../docs/schema/verdict-v1.schema.json");
const POLICY_SCHEMA: &str = include_str!("../../../docs/schema/policy-v1.schema.json");
const EXAMPLES: [(&str, &str); 3] = [
    (
        "web-upload",
        include_str!("../../../docs/schema/policy-examples/web-upload.json"),
    ),
    (
        "strict-gate",
        include_str!("../../../docs/schema/policy-examples/strict-gate.json"),
    ),
    (
        "transcription",
        include_str!("../../../docs/schema/policy-examples/transcription.json"),
    ),
];

fn file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn policy_file(name: &str, policy: &Value) -> PathBuf {
    file(name, policy.to_string().as_bytes())
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

fn coreav(args: &[&std::ffi::OsStr]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(args)
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

fn check(policy: &Path, input: &Path) -> Run {
    coreav(&[
        "check".as_ref(),
        "--policy".as_ref(),
        policy.as_os_str(),
        input.as_os_str(),
    ])
}

fn probe(input: &Path) -> Run {
    coreav(&[
        "probe".as_ref(),
        "--scan-fragments".as_ref(),
        input.as_os_str(),
    ])
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// The verdict of a run that printed one, checked against the schema and
/// the exit code.
fn verdict(run: &Run) -> Value {
    assert!(
        matches!(run.code, 0 | 1),
        "exit {}: {}",
        run.code,
        run.stderr
    );
    assert!(run.stderr.is_empty(), "{}", run.stderr);
    let verdict = json(&run.stdout);
    assert_matches(&verdict, VERDICT_SCHEMA);
    let accepted = verdict["verdict"] == "accept";
    assert_eq!(accepted, run.code == 0);
    assert_eq!(accepted, verdict["reasons"].as_array().unwrap().is_empty());
    verdict
}

fn av_file(name: &str) -> PathBuf {
    file(name, &av_movie_with(aac_entry(), false))
}

#[test]
fn accept_and_reject() {
    let input = av_file("check-av.mp4");
    let doc = json(&probe(&input).stdout);
    let video = doc["streams"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["kind"] == "video")
        .unwrap();
    let codec = video["codec"].as_str().unwrap();

    let permissive = policy_file(
        "permissive.json",
        &json!({"schema": "coreav.policy/1", "name": "any", "streams": {"video": {}, "audio": {}}}),
    );
    let accepted = verdict(&check(&permissive, &input));
    assert_eq!(
        accepted,
        json!({"schema": "coreav.verdict/1", "policy": "any", "verdict": "accept", "reasons": []})
    );

    let strict = policy_file(
        "strict.json",
        &json!({
            "schema": "coreav.policy/1",
            "file": {"max_size": 1},
            "container": {"formats": ["mov"]},
            "streams": {"video": {"codecs": ["av1"], "max_width": 1, "max_height": 1,
                                  "size_source": "declared"},
                        "audio": {"max_channels": 0}}
        }),
    );
    let run = check(&strict, &input);
    assert_eq!(run.code, 1);
    let rejected = verdict(&run);
    let rules: Vec<&str> = rejected["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert_eq!(
        rules,
        [
            "/file/max_size",
            "/container/formats",
            "/streams/video/codecs",
            "/streams/video/max_width",
            "/streams/video/max_height",
            "/streams/audio/max_channels",
        ]
    );
    // Each reason's value is what `coreav probe` prints at its path.
    for reason in rejected["reasons"].as_array().unwrap() {
        let path = reason["path"].as_str().unwrap();
        let at = doc
            .pointer(path)
            .unwrap_or_else(|| panic!("{path} not in the probe output"));
        if reason["rule"] == "/container/formats" {
            assert_eq!(at, "isom");
            assert_eq!(reason["value"], "mp4");
        } else {
            assert_eq!(&reason["value"], at, "{path}");
        }
    }
    assert_eq!(rejected["reasons"][2]["value"], codec);
    assert_eq!(
        rejected["reasons"][2]["stream_index"],
        video["stream_index"]
    );
}

#[test]
fn unreadable_files_carry_the_probe_error() {
    let policy = policy_file(
        "unreadable.json",
        &json!({"schema": "coreav.policy/1", "streams": {"video": {}, "audio": {}}}),
    );
    let mut nested = bx(b"ftyp", b"isom\0\0\x02\0");
    let levels = 10_000usize;
    for i in 0..levels {
        let size = u32::try_from(8 * (levels - i)).unwrap();
        nested.extend_from_slice(&size.to_be_bytes());
        nested.extend_from_slice(if i == 0 { b"moov" } else { b"trak" });
    }
    let mut truncated = av_movie_with(aac_entry(), false);
    truncated.truncate(truncated.len() / 2);
    for (name, bytes) in [
        ("not-a-movie.avi", b"RIFF\x24\x00\x00\x00AVI LIST".to_vec()),
        ("nested-check.mp4", nested),
        ("truncated-check.mp4", truncated),
        ("empty.mp4", Vec::new()),
    ] {
        let input = file(name, &bytes);
        let probed = probe(&input);
        assert_eq!(probed.code, 1, "{name}: {}", probed.stdout);
        let run = check(&policy, &input);
        assert_eq!(run.code, 1, "{name}");
        let verdict = verdict(&run);
        let reasons = verdict["reasons"].as_array().unwrap();
        assert_eq!(reasons.len(), 1, "{name}");
        assert_eq!(reasons[0]["code"], "unreadable");
        assert_eq!(reasons[0]["error"], json(&probed.stderr), "{name}");
    }
}

#[test]
fn fragmented_files_are_scanned() {
    let input = file("check-fragmented.mp4", &fragmented_movie(None, None));
    let policy = policy_file(
        "fragments.json",
        &json!({"schema": "coreav.policy/1", "duration": {"max_seconds": 3600},
                "streams": {"video": {}, "audio": {}, "unidentified": "ignore"}}),
    );
    // Without a scan the duration of a fragmented stream would be unknown.
    let verdict = verdict(&check(&policy, &input));
    assert_eq!(verdict["verdict"], "accept", "{verdict:#}");
    let no_fragments = policy_file(
        "no-fragments.json",
        &json!({"schema": "coreav.policy/1", "container": {"fragmented": false},
                "streams": {"video": {}, "audio": {}}}),
    );
    let run = check(&no_fragments, &input);
    assert_eq!(verdict_rules(&run), ["/container/fragmented"]);
}

fn verdict_rules(run: &Run) -> Vec<String> {
    verdict(run)["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn policy_errors_exit_2() {
    let input = av_file("check-policy-errors.mp4");
    for (name, text, expected) in [
        ("bad-json.json", "{".to_owned(), "not JSON"),
        (
            "misspelled.json",
            json!({"schema": "coreav.policy/1", "streams": {"video": {"max_widht": 1}}})
                .to_string(),
            "/streams/video/max_widht: unknown key",
        ),
        (
            "no-streams.json",
            json!({"schema": "coreav.policy/1"}).to_string(),
            "/streams: missing",
        ),
        (
            "too-big.json",
            " ".repeat(coreav_policy::MAX_POLICY_BYTES + 1),
            "larger than",
        ),
    ] {
        let run = check(&file(name, text.as_bytes()), &input);
        assert_eq!(run.code, 2, "{name}");
        assert!(run.stdout.is_empty(), "{name}");
        assert!(run.stderr.contains(expected), "{name}: {}", run.stderr);
    }
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-policy.json");
    let run = check(&missing, &input);
    assert_eq!(run.code, 2);
    assert!(run.stderr.contains("cannot read policy"), "{}", run.stderr);
    let no_policy = coreav(&["check".as_ref(), input.as_os_str()]);
    assert_eq!(no_policy.code, 2);
}

#[test]
fn unreadable_input_exits_3() {
    let policy = policy_file(
        "io.json",
        &json!({"schema": "coreav.policy/1", "streams": {}}),
    );
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("does-not-exist-check.mp4");
    let run = check(&policy, &missing);
    assert_eq!(run.code, 3);
    assert!(run.stdout.is_empty());
    let error = json(&run.stderr);
    assert_eq!(error["schema"], "coreav.error/1");
    assert_eq!(error["kind"], "io");
}

#[test]
fn examples_match_the_policy_schema_and_parse() {
    let input = av_file("check-examples.mp4");
    for (name, text) in EXAMPLES {
        let policy = json(text);
        assert_matches(&policy, POLICY_SCHEMA);
        assert_eq!(policy["name"], name);
        let path = file(&format!("{name}.json"), text.as_bytes());
        let verdict = verdict(&check(&path, &input));
        assert_eq!(verdict["policy"], name);
    }
}

#[test]
fn the_schema_and_the_parser_refuse_the_same_policies() {
    let root = json(POLICY_SCHEMA);
    for good in [
        json!({"schema": "coreav.policy/1", "streams": {"video": {"rotations": [90]}}}),
        json!({"schema": "coreav.policy/1", "container": {"formats": ["webm", "matroska"]},
               "streams": {"video": {"codecs": ["vp8"]}, "unidentified": ["V_QUICKTIME"]}}),
    ] {
        assert!(validate(&good, &root, &root, "$").is_ok(), "{good}");
        assert!(coreav_policy::Policy::from_value(&good).is_ok(), "{good}");
    }
    for bad in [
        json!({"schema": "coreav.policy/1"}),
        json!({"schema": "coreav.policy/2", "streams": {}}),
        json!({"schema": "coreav.policy/1", "streams": {}, "extra": 1}),
        json!({"schema": "coreav.policy/1", "streams": {"video": {"rotations": [45]}}}),
        json!({"schema": "coreav.policy/1", "streams": {"audio": {"max_width": 1}}}),
        json!({"schema": "coreav.policy/1", "streams": {"audio": {"codecs": ["other"]}}}),
        json!({"schema": "coreav.policy/1", "streams": {"unidentified": "drop"}}),
        json!({"schema": "coreav.policy/1", "file": {"max_size": -1}, "streams": {}}),
        json!({"schema": "coreav.policy/1", "file": {"max_size": 0.5}, "streams": {}}),
        json!({"schema": "coreav.policy/1", "duration": {"max_seconds": -1}, "streams": {}}),
        json!({"schema": "coreav.policy/1", "container": {"formats": ["mkv"]}, "streams": {}}),
        json!({"schema": "coreav.policy/1", "streams": {"unidentified": [""]}}),
    ] {
        assert!(
            validate(&bad, &root, &root, "$").is_err(),
            "schema accepts {bad}"
        );
        assert!(
            coreav_policy::Policy::from_value(&bad).is_err(),
            "parser accepts {bad}"
        );
    }
}

#[test]
fn verdicts_are_byte_identical_across_runs() {
    let input = av_file("check-determinism.mp4");
    for (name, text) in EXAMPLES {
        let path = file(&format!("det-{name}.json"), text.as_bytes());
        let first = check(&path, &input);
        for _ in 0..3 {
            let again = check(&path, &input);
            assert_eq!(again.stdout, first.stdout, "{name}");
            assert_eq!(again.code, first.code, "{name}");
        }
    }
}

/// Every generated fixture through every example policy: a valid verdict,
/// the same bytes twice, and an `unreadable` reason exactly when `coreav
/// probe` rejects the file. Run by the fixtures workflow with
/// `COREAV_FIXTURES` set to the generated directory.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn fixtures_give_valid_verdicts() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let manifest = std::fs::read_to_string(dir.join("MANIFEST")).unwrap();
    let mut checked = 0;
    for name in manifest
        .lines()
        .filter_map(|line| line.strip_prefix("generated "))
    {
        let input = dir.join(name);
        let probed = probe(&input);
        for (policy_name, text) in EXAMPLES {
            let policy = file(&format!("fixture-{policy_name}.json"), text.as_bytes());
            let run = check(&policy, &input);
            let verdict = verdict(&run);
            assert_eq!(check(&policy, &input).stdout, run.stdout, "{name}");
            let unreadable = verdict["reasons"][0]["code"] == "unreadable";
            assert_eq!(unreadable, probed.code == 1, "{name} {policy_name}");
            if probed.code == 1 {
                assert_eq!(verdict["reasons"][0]["error"], json(&probed.stderr));
            }
            checked += 1;
        }
    }
    assert!(checked > 0, "no fixtures listed");
}

/// The generated files whose pictures are larger than their sample entry
/// says (`docs/design/frame-sizes.md`): rejected under the default size
/// source, with the scanned size as the reason, and accepted when the
/// policy trusts the declared size. Run by the fixtures workflow.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures.sh); set COREAV_FIXTURES"]
fn understated_sizes_are_rejected() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    // Each limit sits between the declared size and the decoded one.
    let limit = |source: &str, (width, height): (u32, u32)| {
        let policy = json!({"schema": "coreav.policy/1", "streams": {
            "video": {"max_width": width, "max_height": height, "size_source": source},
            "audio": {}, "timecode": {}}});
        policy_file(&format!("size-{source}-{width}x{height}.json"), &policy)
    };
    for (name, max) in [
        ("size_entry_understated.mp4", (640, 640)),
        ("size_prores_understated.mov", (200, 200)),
        ("size_inband_sps.mp4", (640, 640)),
        ("size_vp9_resize.mp4", (640, 640)),
        ("size_left_crop.mp4", (1250, 1250)),
        ("size_hevc_window.mp4", (1260, 1260)),
        ("size_hevc_mixed_window.mp4", (1300, 700)),
    ] {
        let input = dir.join(name);
        let rejected = verdict(&check(&limit("scan", max), &input));
        let paths: Vec<&str> = rejected["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["path"].as_str())
            .collect();
        assert_eq!(rejected["verdict"], "reject", "{name}");
        assert!(
            paths.iter().any(|p| p.contains("/video/scanned_size/")),
            "{name}: {paths:?}"
        );
        let trusted = verdict(&check(&limit("declared", max), &input));
        assert_eq!(trusted["verdict"], "accept", "{name}");
    }
}

/// The generated Matroska and WebM files whose headers understate what
/// their blocks hold (`docs/design/policy-matroska.md`): rejected with the
/// scanned value as the reason, and accepted when the policy trusts the
/// header. Run by the Matroska fixtures workflow.
#[test]
#[ignore = "needs generated fixtures (scripts/gen-fixtures-mkv.sh); set COREAV_FIXTURES"]
fn matroska_understatements_are_rejected() {
    let dir = PathBuf::from(std::env::var("COREAV_FIXTURES").expect("set COREAV_FIXTURES"));
    let size = |source: &str| {
        let policy = json!({"schema": "coreav.policy/1", "streams": {
            "video": {"max_width": 200, "max_height": 200, "size_source": source},
            "audio": {}}});
        policy_file(&format!("mkv-size-{source}.json"), &policy)
    };
    for name in [
        "size_vp8_understated.webm",
        "size_h264_understated.mkv",
        "size_h264_annexb_understated.mkv",
        "size_vp9_resize.webm",
    ] {
        let input = dir.join(name);
        let rejected = verdict(&check(&size("scan"), &input));
        let paths: Vec<&str> = rejected["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["path"].as_str())
            .collect();
        assert!(
            !paths.is_empty() && paths.iter().all(|p| p.contains("/video/scanned_size/")),
            "{name}: {paths:?}"
        );
        if name != "size_vp9_resize.webm" {
            let trusted = verdict(&check(&size("declared"), &input));
            assert_eq!(trusted["verdict"], "accept", "{name}");
        }
    }
    let duration = policy_file(
        "mkv-duration.json",
        &json!({"schema": "coreav.policy/1", "duration": {"max_seconds": 1.5},
                "streams": {"video": {}, "audio": {}}}),
    );
    let rejected = verdict(&check(&duration, &dir.join("mkv_duration_understated.mkv")));
    assert_eq!(rejected["reasons"][0]["rule"], "/duration/max_seconds");
    assert_eq!(rejected["reasons"][0]["path"], "/streams/1/duration");
}

/// Matroska files through `coreav check` with a policy that scans the
/// blocks (`docs/design/policy-matroska.md`; its VP9 frame is too short
/// for a verified size, so the policy trusts the declared one): the WebM
/// file is accepted,
/// and every truncation and every byte overwritten in it gives a valid
/// verdict, never a crash.
#[test]
fn matroska_files_get_verdicts_whatever_their_bytes() {
    let policy = policy_file(
        "mkv-hostile.json",
        &json!({"schema": "coreav.policy/1", "container": {"formats": ["webm"]},
                "duration": {"max_seconds": 60}, "file": {"max_bit_rate": 100000000},
                "streams": {"video": {"max_width": 640, "max_bit_rate": 100000000,
                                      "size_source": "declared"},
                            "audio": {"max_bit_rate": 1000000}}}),
    );
    let data = mkv::small_webm();
    let accepted = verdict(&check(&policy, &file("check-small.webm", &data)));
    assert_eq!(accepted["verdict"], "accept", "{accepted:#}");
    let input = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check-mutated.webm");
    for at in (0..data.len()).step_by(3) {
        let mut mutated = data.clone();
        mutated[at] = !mutated[at];
        std::fs::write(&input, &mutated).unwrap();
        verdict(&check(&policy, &input));
        std::fs::write(&input, &data[..at]).unwrap();
        verdict(&check(&policy, &input));
    }
}

/// A Matroska file whose second `Tracks` element hides a stream from the
/// probe (docs/design/policy-matroska.md, decision 8): rejected even by a
/// policy with no rule that needs the scanned values, through
/// `invalid_tables`.
#[test]
fn matroska_structures_that_hide_frames_are_rejected() {
    use mkv::{TRACKS, audio_track, cluster, el, info, simple_block, video_track};
    let policy = policy_file(
        "mkv-codecs-only.json",
        &json!({"schema": "coreav.policy/1",
                "streams": {"max_count": 1, "video": {"codecs": ["vp8"]}}}),
    );
    let tracks = |entry: Vec<u8>| el(TRACKS, &entry);
    let data = mkv::file(
        "webm",
        &[
            info(Some(40.0)),
            tracks(video_track(1, "V_VP8", 320, 240, &[])),
            tracks(audio_track(2, "A_OPUS", 48_000.0, 2, &[])),
            cluster(
                0,
                &[
                    simple_block(
                        1,
                        0,
                        true,
                        &[0x50, 0x2a, 0, 0x9d, 1, 0x2a, 0x40, 1, 0xf0, 0],
                    ),
                    simple_block(2, 0, true, &[0xfc, 1, 2]),
                ],
            ),
        ],
    );
    let input = file("check-two-tracks.webm", &data);
    let rejected = verdict(&check(&policy, &input));
    let rules: Vec<&str> = rejected["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["/streams/invalid_tables"], "{rejected:#}");
    assert_eq!(rejected["reasons"][0]["path"], "/streams/0/frames_listed");
}
