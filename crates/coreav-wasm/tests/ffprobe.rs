//! `probe` stays the native document, and `ffprobe` prints what
//! `coreav ffprobe -of json -show_format -show_streams` prints, on small
//! hand-built files of each container.

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

use coreav_cli::{ffprobe, native};
use coreav_isobmff::{BufferedSource, SliceSource};
use coreav_wasm::ops::{self, Failure, Operation, edge_limits};
use serde_json::Value;

const COMMAND: [&str; 5] = ["-of", "json", "-show_format", "-show_streams", "input"];

fn movie() -> Vec<u8> {
    common::av_movie_with(common::aac_entry(), false)
}

fn transport_stream() -> Vec<u8> {
    let mut mux = ts::Mux::default();
    mux.pat(&[(1, 0x1000)]);
    mux.pmt(0x1000, 1, &[(0x0F, 0x101), (0x0F, 0x102)]);
    for n in 0..12u64 {
        let pid = 0x101 + (n % 2) as u16;
        mux.pes(pid, 0xC0, 90_000 + n * 1920, &ts::adts(100), true);
    }
    mux.out
}

fn files() -> [(&'static str, Vec<u8>); 3] {
    [
        ("mp4", movie()),
        ("webm", mkv::small_webm()),
        ("ts", transport_stream()),
    ]
}

fn document(operation: &str, bytes: &[u8], options: &str) -> Result<String, Failure> {
    ops::run_document(operation, SliceSource::new(bytes), options)
}

fn usage(result: Result<String, Failure>) -> String {
    match result {
        Err(Failure::Usage(message)) => message,
        other => panic!("expected a usage error, got {other:?}"),
    }
}

fn error_kind(result: Result<String, Failure>) -> String {
    match result {
        Err(Failure::Error(error)) => error["kind"].as_str().unwrap().to_owned(),
        other => panic!("expected an error object, got {other:?}"),
    }
}

/// What `coreav ffprobe -of json -show_format -show_streams NAME` prints:
/// the command line parsed, the file probed under the library's limits and
/// the sections emitted.
fn command_stdout(bytes: &[u8], name: &str) -> String {
    let command = COMMAND.map(str::to_owned);
    let options = ffprobe::args::parse(&command).unwrap();
    let mut source = BufferedSource::new(SliceSource::new(bytes));
    let mut probed =
        ffprobe::probe_limited(&mut source, bytes.len() as u64, name, None, &edge_limits())
            .ok()
            .unwrap();
    let mut out = Vec::new();
    ffprobe::emit(&options, &mut probed, &mut out).ok().unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn ffprobe_prints_what_the_command_prints() {
    for (kind, bytes) in files() {
        let name = format!("movie.{kind}");
        let options = format!(r#"{{"filename": "{name}"}}"#);
        let got = document("ffprobe", &bytes, &options).ok().unwrap();
        assert_eq!(got, command_stdout(&bytes, &name), "{kind}");
        let value: Value = got.parse().unwrap();
        assert_eq!(value["format"]["filename"], name.as_str(), "{kind}");
        assert!(!value["streams"].as_array().unwrap().is_empty(), "{kind}");
    }
}

#[test]
fn ffprobe_names_the_input_dash_by_default() {
    for (kind, bytes) in files() {
        let got = document("ffprobe", &bytes, "").ok().unwrap();
        assert_eq!(got, command_stdout(&bytes, "-"), "{kind}");
        let value: Value = got.parse().unwrap();
        assert_eq!(value["format"]["filename"], "-", "{kind}");
    }
}

#[test]
fn ffprobe_lists_the_streams_of_the_movie() {
    let value: Value = document("ffprobe", &movie(), "{}")
        .ok()
        .unwrap()
        .parse()
        .unwrap();
    let streams = value["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0]["index"], 0);
    assert_eq!(streams[1]["codec_name"], "aac");
    assert!(value["format"]["format_name"].is_string());
}

#[test]
fn native_probe_is_unchanged() {
    let bytes = movie();
    let allowlist = coreav_cli::probe::allowlist(None, false).unwrap();
    let mut source = SliceSource::new(&bytes);
    let probed = coreav_cli::probe::run_source_limited(
        &mut source,
        bytes.len() as u64,
        &allowlist,
        false,
        false,
        &edge_limits(),
    )
    .ok()
    .unwrap();
    let expected = native::document(&probed).to_string();
    assert_eq!(document("probe", &bytes, "{}").ok().unwrap(), expected);
    assert_eq!(document("probe", &bytes, "").ok().unwrap(), expected);
    let value: Value = expected.parse().unwrap();
    assert_eq!(value["schema"], "coreav.probe/1");

    let webm = mkv::small_webm();
    let value: Value = document("probe", &webm, "{}")
        .ok()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(value["schema"], "coreav.probe-matroska/1");
}

#[test]
fn native_probe_keeps_its_options() {
    let bytes = movie();
    let options = r#"{"allow": ["h264", "aac"], "strict": true, "scanFragments": true, "scanFrameSizes": true, "limits": {"maxTableBytes": 4096}}"#;
    assert!(document("probe", &bytes, options).is_ok());
    assert!(document("probe", &bytes, r#"{"scanFragments": false}"#).is_ok());
    assert!(
        usage(document("probe", &bytes, r#"{"allow": ["nope"]}"#)).contains("nope"),
        "unknown codec"
    );
    assert_eq!(
        error_kind(document(
            "probe",
            &bytes,
            r#"{"allow": ["vp9"], "strict": true}"#
        )),
        "codec_not_allowed"
    );
    assert_eq!(
        error_kind(document("probe", &bytes, r#"{"limits": {"maxTracks": 1}}"#)),
        "limit"
    );
    let webm = mkv::small_webm();
    assert!(document("probe", &webm, r#"{"limits": {"maxTracks": 1}}"#).is_ok());
}

#[test]
fn native_probe_refuses_the_ffprobe_options() {
    let bytes = movie();
    for options in [
        r#"{"format": "ffprobe-compat"}"#,
        r#"{"format": "native"}"#,
        r#"{"format": "json"}"#,
        r#"{"filename": "movie.mp4"}"#,
    ] {
        let message = usage(document("probe", &bytes, options));
        assert!(message.contains("unknown field"), "{options}: {message}");
    }
}

#[test]
fn ffprobe_takes_only_a_filename_and_limits() {
    let empty = SliceSource::new(b"");
    for (options, field) in [
        (r#"{"bogus": 1}"#, "bogus"),
        (r#"{"format": "ffprobe-compat"}"#, "format"),
        (r#"{"allow": ["h264"]}"#, "allow"),
        (r#"{"strict": true}"#, "strict"),
        (r#"{"scanFragments": true}"#, "scanFragments"),
        (r#"{"scanFrameSizes": true}"#, "scanFrameSizes"),
        (r#"{"limits": {"bogus": 1}}"#, "bogus"),
        (r#"{"filename": 1}"#, "invalid type"),
    ] {
        let message = usage(ops::run_document("ffprobe", empty, options));
        assert!(message.contains(field), "{options}: {message}");
    }
}

#[test]
fn ffprobe_writes_no_output_stream() {
    let mut out = Vec::new();
    let result = ops::run_stream("ffprobe", SliceSource::new(b""), "{}", &mut out);
    assert!(usage(result).contains("no output"));
    assert!(out.is_empty());
    assert!(!Operation::Ffprobe.writes_output());
    assert_eq!(Operation::from_name("ffprobe"), Some(Operation::Ffprobe));
}

#[test]
fn ffprobe_limits_bound_every_container() {
    for (kind, bytes) in files() {
        assert!(document("ffprobe", &bytes, "{}").is_ok(), "{kind}");
        let tracks = document("ffprobe", &bytes, r#"{"limits": {"maxTracks": 1}}"#);
        assert_eq!(error_kind(tracks), "limit", "{kind}");
    }
}

#[test]
fn ffprobe_limits_start_from_the_edge_limits() {
    let bytes = movie();
    let tables = format!(
        r#"{{"limits": {{"maxTableBytes": {}}}}}"#,
        ops::EDGE_TABLE_BYTES + 1
    );
    assert!(document("ffprobe", &bytes, &tables).is_ok());
    assert_eq!(
        error_kind(document(
            "ffprobe",
            &bytes,
            r#"{"limits": {"maxTableBytes": 1}}"#
        )),
        "limit"
    );
}

#[test]
fn ffprobe_rejections_are_error_objects() {
    let not_a_movie = b"RIFF\x24\x00\x00\x00AVI LIST";
    assert_eq!(
        error_kind(document("ffprobe", not_a_movie, "{}")),
        "unsupported_container"
    );
    assert_eq!(
        error_kind(document("ffprobe", b"", "{}")),
        "unsupported_container"
    );
}
