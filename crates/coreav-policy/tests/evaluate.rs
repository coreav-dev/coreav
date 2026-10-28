//! Every rule of `coreav.policy/1` against hand-written `coreav.probe/1`
//! documents: accept, each kind of reason, exact comparisons at the limit,
//! unknown values, and the order of reasons.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

use coreav_policy::{Code, Policy, Verdict, evaluate};
use serde_json::{Value, json};

fn policy(value: Value) -> Policy {
    let mut value = value;
    value["schema"] = json!("coreav.policy/1");
    Policy::from_value(&value).unwrap()
}

fn time(num: i64, den: u64, timescale: u64) -> Value {
    #[allow(clippy::cast_precision_loss)]
    let seconds = num as f64 / (den as f64 * timescale as f64);
    json!({"ticks": {"num": num, "den": den}, "timescale": timescale, "seconds": seconds})
}

/// A 10-second MP4 file: H.264 1920×1080 at 5 Mbit/s and stereo AAC at
/// 48 kHz and 128 kbit/s.
fn probe() -> Value {
    json!({
        "schema": "coreav.probe/1",
        "file": {"size": 6_500_000},
        "container": {"major_brand": "isom", "fragmented": false, "timescale": 1000,
                      "creation_time": null},
        "duration": time(10_000, 1, 1000),
        "streams": [
            {
                "index": 0, "stream_index": 0, "track_id": 1, "kind": "video",
                "codec": "h264", "fourcc": "avc1",
                "duration": time(300_000, 1, 30_000),
                "bit_rate": {"exact": {"num": 5_000_000, "den": 1}, "bits_per_second": 5e6},
                "sample_descriptions": 1, "tables_valid": true, "external_data": false,
                "video": {"width": 1920, "height": 1080, "rotation": 0,
                          "parameter_set_size": {"width": 1920, "height": 1080},
                          "scanned_size": {"width": 1920, "height": 1080}},
                "audio": null
            },
            {
                "index": 1, "stream_index": 1, "track_id": 2, "kind": "audio",
                "codec": "aac", "fourcc": "mp4a",
                "duration": time(480_000, 1, 48_000),
                "bit_rate": {"exact": {"num": 128_000, "den": 1}, "bits_per_second": 128e3},
                "sample_descriptions": 1, "tables_valid": true, "external_data": false,
                "video": null,
                "audio": {"channels": 2, "sample_rate": 48_000}
            }
        ],
        "dropped_streams": [],
        "warnings": []
    })
}

fn permissive() -> Value {
    json!({"streams": {"video": {}, "audio": {}}})
}

/// The `(code, rule, path)` of every reason.
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

#[test]
fn a_matching_file_is_accepted() {
    let policy = policy(json!({
        "name": "web",
        "file": {"max_size": 6_500_000, "max_bit_rate": 5_200_000},
        "container": {"formats": ["mp4"], "fragmented": false},
        "duration": {"max_seconds": 10},
        "streams": {
            "max_count": 2,
            "video": {"codecs": ["h264", "hevc"], "min_count": 1, "max_count": 1,
                      "max_width": 1920, "max_height": 1080, "max_pixels": 2_073_600,
                      "max_bit_rate": 5_000_000, "rotations": [0, 90]},
            "audio": {"codecs": ["aac"], "max_count": 1, "max_bit_rate": 128_000,
                      "max_channels": 2, "max_sample_rate": 48_000}
        }
    }));
    let verdict = evaluate(&policy, &probe());
    assert!(verdict.accepted(), "{:#?}", verdict.reasons);
    assert_eq!(
        verdict.to_json(),
        json!({"schema": "coreav.verdict/1", "policy": "web", "verdict": "accept", "reasons": []})
    );
}

#[test]
fn every_limit_rejects_one_unit_over() {
    let policy = policy(json!({
        "file": {"max_size": 6_499_999, "max_bit_rate": 5_199_999},
        "container": {"formats": ["mov", "3gp"]},
        "duration": {"max_seconds": 9.999},
        "streams": {
            "max_count": 1,
            "video": {"codecs": ["hevc"], "min_count": 2, "max_width": 1919,
                      "max_height": 1079, "max_pixels": 2_073_599,
                      "max_bit_rate": 4_999_999, "rotations": [90]},
            "audio": {"codecs": [], "max_count": 0, "max_bit_rate": 127_999,
                      "max_channels": 1, "max_sample_rate": 44_100}
        }
    }));
    let verdict = evaluate(&policy, &probe());
    assert_eq!(
        reasons(&verdict),
        vec![
            ("above_maximum", "/file/max_size".into(), p("/file/size")),
            ("above_maximum", "/file/max_bit_rate".into(), None),
            (
                "not_allowed",
                "/container/formats".into(),
                p("/container/major_brand")
            ),
            (
                "above_maximum",
                "/duration/max_seconds".into(),
                p("/duration")
            ),
            ("above_maximum", "/streams/max_count".into(), None),
            ("below_minimum", "/streams/video/min_count".into(), None),
            ("above_maximum", "/streams/audio/max_count".into(), None),
            (
                "not_allowed",
                "/streams/video/codecs".into(),
                p("/streams/0/codec")
            ),
            (
                "above_maximum",
                "/streams/video/max_bit_rate".into(),
                p("/streams/0/bit_rate")
            ),
            (
                "above_maximum",
                "/streams/video/max_width".into(),
                p("/streams/0/video/width")
            ),
            (
                "above_maximum",
                "/streams/video/max_height".into(),
                p("/streams/0/video/height")
            ),
            ("above_maximum", "/streams/video/max_pixels".into(), None),
            (
                "not_allowed",
                "/streams/video/rotations".into(),
                p("/streams/0/video/rotation")
            ),
            (
                "not_allowed",
                "/streams/audio/codecs".into(),
                p("/streams/1/codec")
            ),
            (
                "above_maximum",
                "/streams/audio/max_bit_rate".into(),
                p("/streams/1/bit_rate")
            ),
            (
                "above_maximum",
                "/streams/audio/max_channels".into(),
                p("/streams/1/audio/channels")
            ),
            (
                "above_maximum",
                "/streams/audio/max_sample_rate".into(),
                p("/streams/1/audio/sample_rate")
            ),
        ]
    );
    let json = verdict.to_json();
    assert_eq!(json["verdict"], "reject");
    let size = &json["reasons"][0];
    assert_eq!(size["value"], 6_500_000);
    assert_eq!(size["limit"], 6_499_999);
    assert_eq!(size["stream_index"], Value::Null);
    assert_eq!(
        size["message"],
        "file: size 6500000 bytes is above the maximum 6499999 bytes"
    );
    // 6,500,000 bytes × 8 / 10 s, exactly.
    let rate = &json["reasons"][1];
    assert_eq!(rate["exact"], json!({"num": 5_200_000, "den": 1}));
    assert_eq!(rate["value"], 5_200_000.0);
    let container = &json["reasons"][2];
    assert_eq!(container["value"], "mp4");
    assert_eq!(container["limit"], json!(["mov", "3gp"]));
    assert_eq!(
        container["message"],
        "file: container format \"mp4\" (major brand \"isom\") is not allowed; allowed: mov, 3gp"
    );
    let duration = &json["reasons"][3];
    assert_eq!(duration["exact"], json!({"num": 10, "den": 1}));
    assert_eq!(duration["value"], 10.0);
    assert_eq!(duration["limit"], 9.999);
    let codec = &json["reasons"][7];
    assert_eq!(codec["stream_index"], 0);
    assert_eq!(codec["value"], "h264");
    assert_eq!(codec["limit"], json!(["hevc"]));
    assert_eq!(
        codec["message"],
        "stream 0 (video): codec \"h264\" is not allowed; allowed: hevc"
    );
    let empty = &json["reasons"][13];
    assert_eq!(
        empty["message"],
        "stream 1 (audio): codec \"aac\" is not allowed; allowed: none"
    );
    for reason in json["reasons"].as_array().unwrap() {
        assert_eq!(reason["error"], Value::Null);
        assert!(reason["message"].as_str().unwrap().len() > 10);
    }
}

#[test]
fn limits_are_compared_exactly() {
    // 29.97 fps: 1001/30000 s per frame, 300 frames = 10.01 s.
    let mut doc = probe();
    doc["duration"] = time(10_010, 1, 1000);
    doc["streams"][0]["duration"] = time(300_300, 1, 30_000);
    let at = |doc: &Value, limit: Value| {
        let verdict = evaluate(
            &policy(
                json!({"duration": {"max_seconds": limit}, "streams": {"video": {}, "audio": {}}}),
            ),
            doc,
        );
        reasons(&verdict).len()
    };
    assert_eq!(at(&doc, json!(10.01)), 0);
    assert_eq!(at(&doc, json!(10.009_999_999)), 1);
    assert_eq!(at(&doc, json!(1001e-2)), 0);
    // A duration whose ticks are a fraction: 1/3 tick over 10 s at 1 Hz.
    doc["duration"] = time(31, 3, 1);
    assert_eq!(at(&doc, json!(10.333_333_333)), 1);
    assert_eq!(at(&doc, json!(10.333_333_334)), 0);
}

#[test]
fn duration_is_the_longest_of_the_file_and_its_streams() {
    let rules =
        policy(json!({"duration": {"max_seconds": 10}, "streams": {"video": {}, "audio": {}}}));
    let mut doc = probe();
    assert!(evaluate(&rules, &doc).accepted());
    // The movie header says 10 s, but the audio lasts 10.5 s.
    doc["streams"][1]["duration"] = time(504_000, 1, 48_000);
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![(
            "above_maximum",
            "/duration/max_seconds".into(),
            p("/streams/1/duration")
        )]
    );
    assert_eq!(verdict.reasons[0].stream_index, Some(1));
    assert_eq!(verdict.reasons[0].exact, Some((21, 2)));
    assert_eq!(
        verdict.reasons[0].message,
        "stream 1 (audio): duration 10.5 s is above the maximum 10 s"
    );
    // An unknown stream duration cannot be verified.
    doc["streams"][0]["duration"] = Value::Null;
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![(
            "unknown",
            "/duration/max_seconds".into(),
            p("/streams/0/duration")
        )]
    );
}

#[test]
fn kinds_without_rules_are_not_allowed() {
    let mut doc = probe();
    doc["streams"].as_array_mut().unwrap().push(json!({
        "index": 2, "stream_index": 3, "kind": "subtitle", "codec": "timed_text",
        "duration": time(10, 1, 1), "bit_rate": null,
        "sample_descriptions": 1, "tables_valid": true, "external_data": false, "video": null, "audio": null
    }));
    let verdict = evaluate(&policy(json!({"streams": {"video": {}}})), &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "not_allowed",
                "/streams/audio".into(),
                p("/streams/1/codec")
            ),
            (
                "not_allowed",
                "/streams/subtitle".into(),
                p("/streams/2/codec")
            ),
        ]
    );
    let subtitle = &verdict.reasons[1];
    assert_eq!(subtitle.stream_index, Some(3));
    assert_eq!(subtitle.value, json!("subtitle"));
    assert_eq!(subtitle.limit, json!(["video"]));
    assert_eq!(
        subtitle.message,
        "stream 3 (subtitle): subtitle streams are not allowed; allowed: video"
    );
    let none = evaluate(&policy(json!({"streams": {}})), &doc);
    assert_eq!(none.reasons.len(), 3);
    assert_eq!(none.reasons[0].limit, json!([]));
    assert!(none.reasons[0].message.ends_with("allowed: none"));
}

#[test]
fn unidentified_and_malformed_streams() {
    let mut doc = probe();
    doc["streams"][1]["tables_valid"] = json!(false);
    doc["streams"][0]["external_data"] = json!(true);
    doc["dropped_streams"] = json!([
        {"stream_index": 2, "track_id": 3, "fourcc": "mebx", "reason": "codec_not_allowed"}
    ]);
    let verdict = evaluate(&policy(permissive()), &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "not_allowed",
                "/streams/external_data".into(),
                p("/streams/0/external_data")
            ),
            (
                "not_allowed",
                "/streams/invalid_tables".into(),
                p("/streams/1/tables_valid")
            ),
            (
                "not_allowed",
                "/streams/unidentified".into(),
                p("/dropped_streams/0/fourcc")
            ),
        ]
    );
    let dropped = &verdict.reasons[2];
    assert_eq!(dropped.stream_index, Some(2));
    assert_eq!(dropped.value, json!("mebx"));
    let ignore = policy(json!({"streams": {
        "video": {}, "audio": {},
        "unidentified": "ignore", "invalid_tables": "ignore", "external_data": "ignore"
    }}));
    assert!(evaluate(&ignore, &doc).accepted());
    // Dropped streams count towards the total.
    let total = policy(json!({"streams": {"max_count": 2, "video": {}, "audio": {},
                                          "unidentified": "ignore"}}));
    let verdict = evaluate(&total, &doc);
    assert_eq!(
        verdict.reasons[0].rule.as_deref(),
        Some("/streams/max_count")
    );
    assert_eq!(verdict.reasons[0].value, json!(3));
}

#[test]
fn rules_follow_the_codec_not_the_handler() {
    // A second H.264 track under a handler that is not `vide`: the probe
    // says `data` and prints no video object. It still gets the video rules:
    // the count rejects it, and its size is unknown.
    let mut doc = probe();
    doc["streams"].as_array_mut().unwrap().push(json!({
        "index": 2, "stream_index": 2, "kind": "data", "codec": "h264",
        "duration": time(10, 1, 1), "bit_rate": null,
        "sample_descriptions": 1, "tables_valid": true, "external_data": false, "video": null, "audio": null
    }));
    let rules = policy(json!({"streams": {
        "video": {"max_count": 1, "max_width": 4096}, "audio": {}, "data": {}
    }}));
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            ("above_maximum", "/streams/video/max_count".into(), None),
            (
                "unknown",
                "/streams/video/max_width".into(),
                p("/streams/2/video/width")
            ),
        ]
    );
    assert_eq!(
        verdict.reasons[1].message,
        "stream 2 (video): width is unknown"
    );
    // Without video rules, the stream is a video stream that is not allowed.
    let no_video = policy(json!({"streams": {"audio": {}, "data": {}}}));
    let verdict = evaluate(&no_video, &doc);
    assert_eq!(verdict.reasons.len(), 2);
    assert_eq!(verdict.reasons[1].path.as_deref(), Some("/streams/2/codec"));
    assert_eq!(verdict.reasons[1].value, json!("video"));
}

#[test]
fn unidentified_streams_by_sample_entry_type() {
    let mut doc = probe();
    doc["dropped_streams"] = json!([
        {"stream_index": 2, "track_id": 3, "fourcc": "mebx", "reason": "codec_not_allowed"},
        {"stream_index": 3, "track_id": 4, "fourcc": "mp4v", "reason": "codec_not_allowed"}
    ]);
    let listed = policy(json!({"streams": {"video": {}, "audio": {}, "unidentified": ["mebx"]}}));
    let verdict = evaluate(&listed, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![(
            "not_allowed",
            "/streams/unidentified".into(),
            p("/dropped_streams/1/fourcc")
        )]
    );
    assert_eq!(verdict.reasons[0].value, json!("mp4v"));
    assert_eq!(verdict.reasons[0].limit, json!(["mebx"]));
    let reject = policy(json!({"streams": {"video": {}, "audio": {}}}));
    let verdict = evaluate(&reject, &doc);
    assert_eq!(verdict.reasons.len(), 2);
    assert_eq!(verdict.reasons[0].limit, json!([]));
    let ignore = policy(json!({"streams": {"video": {}, "audio": {}, "unidentified": "ignore"}}));
    assert!(evaluate(&ignore, &doc).accepted());
}

#[test]
fn resolution_uses_the_larger_of_declared_and_verified() {
    // The sample entry says 1920×1080; the scanned samples say 7680×4320.
    let mut doc = probe();
    doc["streams"][0]["video"]["scanned_size"] = json!({"width": 7680, "height": 4320});
    let limits = |source: Option<&str>| {
        let mut video = json!({"max_width": 4096, "max_height": 4096, "max_pixels": 8_912_896});
        if let Some(source) = source {
            video["size_source"] = json!(source);
        }
        policy(json!({"streams": {"video": video, "audio": {}}}))
    };
    let verdict = evaluate(&limits(None), &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "above_maximum",
                "/streams/video/max_width".into(),
                p("/streams/0/video/scanned_size/width")
            ),
            (
                "above_maximum",
                "/streams/video/max_height".into(),
                p("/streams/0/video/scanned_size/height")
            ),
            ("above_maximum", "/streams/video/max_pixels".into(), None),
        ]
    );
    assert_eq!(verdict.reasons[0].value, json!(7680));
    assert_eq!(verdict.reasons[2].value, json!(7680 * 4320));
    // The configuration's parameter sets agree with the entry: accepted
    // under "configuration", and under "declared".
    assert!(evaluate(&limits(Some("configuration")), &doc).accepted());
    assert!(evaluate(&limits(Some("declared")), &doc).accepted());
    // A verified size smaller than declared does not lower the declared one.
    doc["streams"][0]["video"]["scanned_size"] = json!({"width": 640, "height": 360});
    let tight = policy(json!({"streams": {"video": {"max_width": 1000}, "audio": {}}}));
    let verdict = evaluate(&tight, &doc);
    assert_eq!(
        verdict.reasons[0].path.as_deref(),
        Some("/streams/0/video/width")
    );
    // A verified size the source cannot give is unknown.
    doc["streams"][0]["video"]["scanned_size"] = Value::Null;
    let verdict = evaluate(&limits(None), &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "unknown",
                "/streams/video/max_width".into(),
                p("/streams/0/video/scanned_size/width")
            ),
            (
                "unknown",
                "/streams/video/max_height".into(),
                p("/streams/0/video/scanned_size/height")
            ),
            ("unknown", "/streams/video/max_pixels".into(), None),
        ]
    );
    assert!(evaluate(&limits(Some("declared")), &doc).accepted());
}

#[test]
fn more_than_one_sample_description() {
    let mut doc = probe();
    doc["streams"][0]["sample_descriptions"] = json!(2);
    let rules = policy(json!({"streams": {"video": {}, "audio": {}}}));
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![(
            "not_allowed",
            "/streams/multiple_descriptions".into(),
            p("/streams/0/sample_descriptions")
        )]
    );
    assert_eq!(verdict.reasons[0].value, json!(2));
    assert_eq!(verdict.reasons[0].limit, json!(1));
    let ignore = policy(json!({"streams": {"video": {}, "audio": {},
                                           "multiple_descriptions": "ignore"}}));
    assert!(evaluate(&ignore, &doc).accepted());
}

#[test]
fn rotation_and_fragmentation() {
    let mut doc = probe();
    doc["streams"][0]["video"]["rotation"] = Value::Null;
    doc["container"]["fragmented"] = json!(true);
    let rules = policy(json!({
        "container": {"fragmented": false},
        "streams": {"video": {"rotations": [0, 90, 180, 270]}, "audio": {}}
    }));
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            (
                "not_allowed",
                "/container/fragmented".into(),
                p("/container/fragmented")
            ),
            (
                "not_allowed",
                "/streams/video/rotations".into(),
                p("/streams/0/video/rotation")
            ),
        ]
    );
    assert_eq!(verdict.reasons[1].value, Value::Null);
    assert_eq!(
        verdict.reasons[1].message,
        "stream 0 (video): the display matrix is not a quarter-turn rotation"
    );
    doc["streams"][0]["video"]["rotation"] = json!(270);
    let verdict = evaluate(&rules, &doc);
    assert_eq!(verdict.reasons.len(), 1);
}

#[test]
fn containers_by_major_brand() {
    let mut doc = probe();
    let mov_only =
        policy(json!({"container": {"formats": ["mov"]}, "streams": {"video": {}, "audio": {}}}));
    for (brand, accepted) in [
        (json!("qt  "), true),
        (Value::Null, true),
        (json!("isom"), false),
        (json!("3gp4"), false),
    ] {
        doc["container"]["major_brand"] = brand.clone();
        assert_eq!(evaluate(&mov_only, &doc).accepted(), accepted, "{brand}");
    }
    doc["container"]["major_brand"] = Value::Null;
    let mp4_only =
        policy(json!({"container": {"formats": ["mp4"]}, "streams": {"video": {}, "audio": {}}}));
    let verdict = evaluate(&mp4_only, &doc);
    assert_eq!(
        verdict.reasons[0].message,
        "file: container format \"mov\" (no ftyp box) is not allowed; allowed: mp4"
    );
}

#[test]
fn unknown_values_reject() {
    let mut doc = probe();
    doc["duration"] = Value::Null;
    doc["streams"][0]["bit_rate"] = Value::Null;
    doc["streams"][0]["video"]["width"] = Value::Null;
    doc["streams"][1]["duration"] = time(0, 1, 48_000);
    let rules = policy(json!({
        "file": {"max_bit_rate": 1_000_000_000},
        "duration": {"max_seconds": 3600},
        "streams": {"video": {"max_bit_rate": 1, "max_width": 1, "max_pixels": 1}, "audio": {}}
    }));
    let verdict = evaluate(&rules, &doc);
    assert_eq!(
        reasons(&verdict),
        vec![
            ("unknown", "/file/max_bit_rate".into(), None),
            ("unknown", "/duration/max_seconds".into(), p("/duration")),
            (
                "unknown",
                "/streams/video/max_bit_rate".into(),
                p("/streams/0/bit_rate")
            ),
            (
                "unknown",
                "/streams/video/max_width".into(),
                p("/streams/0/video/width")
            ),
            ("unknown", "/streams/video/max_pixels".into(), None),
        ]
    );
    assert!(verdict.reasons.iter().all(|r| r.value.is_null()));
    assert_eq!(verdict.reasons[1].message, "file: duration is unknown");
    // A zero duration gives no overall bit rate.
    doc["duration"] = time(0, 1, 1000);
    let rate = policy(json!({"file": {"max_bit_rate": 1}, "streams": {"video": {}, "audio": {}}}));
    assert_eq!(evaluate(&rate, &doc).reasons[0].code, Code::Unknown);
}

#[test]
fn documents_that_are_not_probe_output() {
    let rules = policy(permissive());
    for doc in [
        json!({}),
        json!([]),
        json!({"schema": "coreav.clip/1"}),
        Value::Null,
    ] {
        let verdict = evaluate(&rules, &doc);
        assert_eq!(
            reasons(&verdict),
            vec![("unknown", String::new(), p("/schema"))]
        );
    }
    // A probe document missing keys: every rule that needs one is unknown,
    // nothing panics.
    let bare = json!({"schema": "coreav.probe/1", "streams": [{}], "dropped_streams": [{}]});
    let all = policy(json!({
        "file": {"max_size": 1, "max_bit_rate": 1},
        "container": {"formats": ["mp4"], "fragmented": false},
        "duration": {"max_seconds": 1},
        "streams": {"video": {"codecs": ["h264"]}}
    }));
    let verdict = evaluate(&all, &bare);
    assert!(
        verdict
            .reasons
            .iter()
            .all(|r| r.code == Code::Unknown || r.code == Code::NotAllowed)
    );
    assert!(!verdict.accepted());
}

#[test]
fn unreadable_files() {
    let error = json!({"schema": "coreav.error/1", "kind": "truncated_structure", "limit": null,
                       "offset": 32, "message": "truncated structure at offset 32"});
    let verdict = Verdict::unreadable(Some("web".into()), error.clone());
    let json = verdict.to_json();
    assert_eq!(json["verdict"], "reject");
    assert_eq!(
        json["reasons"],
        json!([{
            "code": "unreadable", "rule": null, "path": null, "stream_index": null,
            "value": null, "exact": null, "limit": null,
            "message": "the file could not be read: truncated structure at offset 32",
            "error": error
        }])
    );
}

#[test]
fn verdicts_are_deterministic() {
    let rules = policy(json!({
        "file": {"max_size": 1},
        "streams": {"video": {"max_width": 1}, "audio": {"max_channels": 1}}
    }));
    let doc = probe();
    let first = serde_json::to_string(&evaluate(&rules, &doc).to_json()).unwrap();
    for _ in 0..100 {
        assert_eq!(
            serde_json::to_string(&evaluate(&rules, &doc).to_json()).unwrap(),
            first
        );
    }
}
