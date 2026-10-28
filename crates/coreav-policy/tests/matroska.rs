//! The rules of `coreav.policy/1` against hand-written
//! `coreav.probe-matroska/1` documents: the mapping of
//! `docs/design/policy-matroska.md`, rule by rule.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

use coreav_policy::{Code, Policy, Verdict, evaluate};
use serde_json::{Value, json};

fn policy(value: Value) -> Policy {
    let mut value = value;
    value["schema"] = json!("coreav.policy/1");
    Policy::from_value(&value).unwrap()
}

/// A time in milliseconds (the default `TimestampScale`).
fn ms(ms: i64) -> Value {
    #[allow(clippy::cast_precision_loss)]
    let seconds = ms as f64 / 1000.0;
    json!({"ticks": {"num": ms, "den": 1}, "timescale": 1000, "seconds": seconds})
}

/// A 10-second WebM file from a browser: VP8 1280×720 at 2 Mbit/s and
/// stereo Opus at 48 kHz and 96 kbit/s, scanned.
fn probe() -> Value {
    json!({
        "schema": "coreav.probe-matroska/1",
        "file": {"size": 2_620_000},
        "container": {"doc_type": "webm", "live": false, "timestamp_scale": 1_000_000},
        "duration": ms(10_000),
        "streams": [
            {
                "index": 0, "stream_index": 0, "track_number": 1, "kind": "video",
                "codec": "vp8", "codec_id": "V_VP8", "encoding": "none",
                "packets_available": true, "frames_listed": true,
                "duration": ms(10_000), "frame_count": 300,
                "bit_rate": {"exact": {"num": 2_000_000, "den": 1}, "bits_per_second": 2e6},
                "video": {"width": 1280, "height": 720, "rotation": 0,
                          "parameter_set_size": null,
                          "scanned_size": {"width": 1280, "height": 720}},
                "audio": null
            },
            {
                "index": 1, "stream_index": 1, "track_number": 2, "kind": "audio",
                "codec": "opus", "codec_id": "A_OPUS", "encoding": "none",
                "packets_available": true, "frames_listed": true,
                "duration": ms(9_990), "frame_count": 500,
                "bit_rate": {"exact": {"num": 96_000, "den": 1}, "bits_per_second": 96e3},
                "video": null,
                "audio": {"channels": 2, "sample_rate": 48_000}
            }
        ],
        "dropped_streams": [],
        "warnings": []
    })
}

fn reasons(verdict: &Verdict) -> Vec<(&'static str, String, Option<String>)> {
    verdict
        .reasons
        .iter()
        .map(|r| {
            (
                r.code.name(),
                r.rule.clone().unwrap_or_default(),
                r.path.clone(),
            )
        })
        .collect()
}

fn p(path: &str) -> Option<String> {
    Some(path.to_owned())
}

fn strict() -> Policy {
    policy(json!({
        "file": {"max_size": 10_000_000, "max_bit_rate": 3_000_000},
        "container": {"formats": ["webm", "mp4"], "fragmented": false},
        "duration": {"max_seconds": 60},
        "streams": {
            "max_count": 2,
            "video": {"codecs": ["vp8", "vp9"], "max_count": 1, "max_bit_rate": 4_000_000,
                      "max_width": 1920, "max_height": 1080, "rotations": [0]},
            "audio": {"codecs": ["opus", "vorbis"], "max_channels": 2,
                      "max_sample_rate": 48_000, "max_bit_rate": 256_000}
        }
    }))
}

#[test]
fn a_matching_webm_file_is_accepted() {
    let verdict = evaluate(&strict(), &probe());
    assert!(verdict.accepted(), "{:?}", reasons(&verdict));
}

#[test]
fn container_rules_read_the_doc_type_and_live() {
    let mut doc = probe();
    doc["container"]["doc_type"] = json!("matroska");
    doc["container"]["live"] = json!(true);
    assert_eq!(
        reasons(&evaluate(&strict(), &doc)),
        vec![
            (
                "not_allowed",
                "/container/formats".to_owned(),
                p("/container/doc_type")
            ),
            (
                "not_allowed",
                "/container/fragmented".to_owned(),
                p("/container/live")
            ),
        ]
    );
    doc["container"]["doc_type"] = json!("something");
    doc["container"]["live"] = Value::Null;
    let verdict = evaluate(&strict(), &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "unknown",
                "/container/formats".to_owned(),
                p("/container/doc_type")
            ),
            (
                "unknown",
                "/container/fragmented".to_owned(),
                p("/container/live")
            ),
        ]
    );
}

#[test]
fn duration_takes_the_longest_and_a_missing_duration_is_live() {
    // A stream longer than Duration.
    let mut doc = probe();
    doc["streams"][1]["duration"] = ms(61_000);
    let rules = policy(json!({"duration": {"max_seconds": 60},
                              "streams": {"video": {}, "audio": {}}}));
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "above_maximum",
            "/duration/max_seconds".to_owned(),
            p("/streams/1/duration")
        )]
    );
    // No Duration (live): the streams' durations decide.
    let mut doc = probe();
    doc["duration"] = Value::Null;
    assert!(evaluate(&rules, &doc).accepted());
    // No Duration and a stream that was not scanned: unknown.
    doc["streams"][0]["duration"] = Value::Null;
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "unknown",
            "/duration/max_seconds".to_owned(),
            p("/duration")
        )]
    );
    // Duration present, a stream unscanned: unknown at the stream.
    let mut doc = probe();
    doc["streams"][0]["duration"] = Value::Null;
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "unknown",
            "/duration/max_seconds".to_owned(),
            p("/streams/0/duration")
        )]
    );
}

#[test]
fn overall_bit_rate_without_duration_uses_the_longest_stream() {
    let rules = policy(json!({"file": {"max_bit_rate": 2_000_000},
                              "streams": {"video": {}, "audio": {}}}));
    let mut doc = probe();
    // 2,620,000 bytes over 10 s: 2,096,000 bit/s.
    assert_eq!(evaluate(&rules, &doc).reasons[0].code, Code::AboveMaximum);
    doc["duration"] = Value::Null;
    doc["streams"][0]["duration"] = ms(20_000);
    assert!(evaluate(&rules, &doc).accepted());
    doc["streams"][1]["duration"] = Value::Null;
    assert_eq!(evaluate(&rules, &doc).reasons[0].code, Code::Unknown);
}

#[test]
fn matroska_codecs_have_their_family() {
    // A subtitle track labelled as video by its TrackType still gets the
    // subtitle rules from its codec.
    let mut doc = probe();
    doc["streams"][1]["codec"] = json!("webvtt");
    doc["streams"][1]["kind"] = json!("video");
    let rules = policy(json!({"streams": {"video": {}, "audio": {}}}));
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "not_allowed",
            "/streams/subtitle".to_owned(),
            p("/streams/1/codec")
        )]
    );
    let mut doc = probe();
    doc["streams"][1]["codec"] = json!("vorbis");
    let rules = policy(json!({"streams": {"video": {}, "audio": {"codecs": ["opus"]}}}));
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "not_allowed",
            "/streams/audio/codecs".to_owned(),
            p("/streams/1/codec")
        )]
    );
}

#[test]
fn verified_sizes_and_rotation() {
    let mut doc = probe();
    doc["streams"][0]["video"]["scanned_size"] = json!({"width": 3840, "height": 2160});
    let rules = policy(json!({"streams": {
        "video": {"max_width": 1920, "max_height": 1080, "rotations": [0]}, "audio": {}}}));
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![
            (
                "above_maximum",
                "/streams/video/max_width".to_owned(),
                p("/streams/0/video/scanned_size/width")
            ),
            (
                "above_maximum",
                "/streams/video/max_height".to_owned(),
                p("/streams/0/video/scanned_size/height")
            ),
        ]
    );
    let mut doc = probe();
    doc["streams"][0]["video"]["scanned_size"] = Value::Null;
    doc["streams"][0]["video"]["rotation"] = Value::Null;
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "unknown",
                "/streams/video/max_width".to_owned(),
                p("/streams/0/video/scanned_size/width")
            ),
            (
                "unknown",
                "/streams/video/max_height".to_owned(),
                p("/streams/0/video/scanned_size/height")
            ),
            (
                "not_allowed",
                "/streams/video/rotations".to_owned(),
                p("/streams/0/video/rotation")
            ),
        ]
    );
    assert!(verdict.reasons[2].message.contains("projection pose"));
}

#[test]
fn encrypted_and_unidentified_tracks() {
    let mut doc = probe();
    doc["streams"][1]["encoding"] = json!("encrypted");
    doc["dropped_streams"] = json!([{"stream_index": 2, "track_number": 3,
                                     "codec_id": "V_QUICKTIME", "codec": "other",
                                     "reason": "codec_not_allowed"}]);
    let rules = |unidentified: Value| {
        policy(json!({"streams": {"video": {}, "audio": {}, "unidentified": unidentified}}))
    };
    assert_eq!(
        reasons(&evaluate(&rules(json!("reject")), &doc)),
        vec![
            (
                "not_allowed",
                "/streams/unidentified".to_owned(),
                p("/streams/1/encoding")
            ),
            (
                "not_allowed",
                "/streams/unidentified".to_owned(),
                p("/dropped_streams/0/codec_id")
            ),
        ]
    );
    // A list names CodecIDs; it cannot allow an encrypted track.
    assert_eq!(
        reasons(&evaluate(&rules(json!(["V_QUICKTIME", "A_OPUS"])), &doc)),
        vec![(
            "not_allowed",
            "/streams/unidentified".to_owned(),
            p("/streams/1/encoding")
        )]
    );
    assert!(evaluate(&rules(json!("ignore")), &doc).accepted());
}

#[test]
fn frames_listed_stands_for_valid_tables() {
    let mut doc = probe();
    doc["streams"][1]["encoding"] = json!("unsupported");
    doc["streams"][1]["packets_available"] = json!(false);
    doc["streams"][1]["frames_listed"] = json!(false);
    let rules = policy(json!({"streams": {"video": {}, "audio": {}, "unidentified": "ignore"}}));
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "not_allowed",
            "/streams/invalid_tables".to_owned(),
            p("/streams/1/frames_listed")
        )]
    );
    // Not scanned: unknown.
    let mut doc = probe();
    doc["streams"][0]["frames_listed"] = Value::Null;
    assert_eq!(
        reasons(&evaluate(&rules, &doc)),
        vec![(
            "unknown",
            "/streams/invalid_tables".to_owned(),
            p("/streams/0/frames_listed")
        )]
    );
    // External data and sample descriptions do not apply to Matroska.
    let doc = probe();
    assert!(doc["streams"][0].get("external_data").is_none());
    assert!(evaluate(&rules, &doc).accepted());
}

#[test]
fn stream_counts_exclude_attachments() {
    let mut doc = probe();
    doc["container"]["attachments"] = json!([{"name": "cover.jpg", "media_type": "image/jpeg",
                                              "size": 1000}]);
    let rules = policy(json!({"streams": {"max_count": 2, "video": {}, "audio": {}}}));
    assert!(evaluate(&rules, &doc).accepted());
}
