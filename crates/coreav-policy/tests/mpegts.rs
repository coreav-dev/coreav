//! Policy evaluation over the current MPEG-TS probe document.
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

use coreav_policy::{Code, MPEGTS_PROBE_SCHEMA, Policy, evaluate};
use serde_json::{Value, json};

fn policy() -> Value {
    json!({
        "schema": "coreav.policy/1",
        "container": {"formats": ["mpegts"]},
        "streams": {
            "video": {}, "audio": {}, "subtitle": {}, "data": {},
            "invalid_tables": "ignore", "external_data": "ignore",
            "multiple_descriptions": "ignore"
        }
    })
}

fn document() -> Value {
    json!({
        "schema": MPEGTS_PROBE_SCHEMA,
        "file": {"size": 1000}, "container": {"packet_size": 188},
        "duration": {"ticks": 900_900, "timescale": 90_000, "seconds": 10.01},
        "streams": [{
            "index": 0, "stream_index": 3, "codec": "aac", "kind": "audio",
            "duration": {"ticks": 900_900, "timescale": 90_000, "seconds": 10.01},
            "scan": {"bit_rate": {
                "exact": {"num": 1000, "den": 3}, "bits_per_second": 1000.0 / 3.0
            }},
            "audio": {"channels": 2, "sample_rate": 48_000}
        }],
        "dropped_streams": []
    })
}

#[test]
fn the_current_schema_identifies_transport_streams_without_a_brand() {
    let mut doc = document();
    let mut p = policy();
    for brand in [Value::Null, json!("qt  "), json!("isom")] {
        doc["container"]["major_brand"] = brand;
        assert!(evaluate(&Policy::from_value(&p).unwrap(), &doc).accepted());
    }
    p["container"]["formats"] = json!(["mov", "mp4"]);
    let verdict = evaluate(&Policy::from_value(&p).unwrap(), &doc);
    assert_eq!(verdict.reasons.len(), 1);
    assert_eq!(verdict.reasons[0].path.as_deref(), Some("/schema"));
    assert_eq!(verdict.reasons[0].value, "mpegts");
    doc["schema"] = json!("coreav.probe-mpegts/1");
    assert_eq!(
        evaluate(&Policy::from_value(&p).unwrap(), &doc).reasons[0].code,
        Code::Unknown
    );
}

#[test]
fn transport_codec_names_select_their_family_not_the_stated_kind() {
    for (codec, kind) in [
        ("vvc", "video"),
        ("aac_latm", "audio"),
        ("mpeg_audio", "audio"),
        ("bluray_pcm", "audio"),
        ("scte35", "data"),
        ("id3", "data"),
        ("epg", "data"),
        ("dvb_subtitle", "subtitle"),
        ("dvb_teletext", "subtitle"),
        ("arib_caption", "subtitle"),
    ] {
        let mut doc = document();
        doc["streams"][0]["codec"] = json!(codec);
        doc["streams"][0]["kind"] = json!("timecode");
        let mut p = policy();
        p["streams"][kind] = json!({"codecs": [codec], "min_count": 1});
        let parsed = Policy::from_value(&p).unwrap();
        assert!(evaluate(&parsed, &doc).accepted(), "{codec}");
    }
}

#[test]
fn integer_ticks_and_nested_rates_are_compared_exactly() {
    let mut p = policy();
    p["duration"] = json!({"max_seconds": 10.01});
    p["streams"]["audio"]["max_bit_rate"] = json!(334);
    assert!(evaluate(&Policy::from_value(&p).unwrap(), &document()).accepted());
    p["duration"]["max_seconds"] = json!(10.009999);
    p["streams"]["audio"]["max_bit_rate"] = json!(333);
    let verdict = evaluate(&Policy::from_value(&p).unwrap(), &document());
    assert_eq!(verdict.reasons.len(), 2);
    assert_eq!(verdict.reasons[0].exact, Some((1001, 100)));
    assert_eq!(
        verdict.reasons[1].path.as_deref(),
        Some("/streams/0/scan/bit_rate")
    );
    assert_eq!(verdict.reasons[1].exact, Some((1000, 3)));
    assert_eq!(verdict.reasons[1].stream_index, Some(3));
}

#[test]
fn missing_null_and_zero_times_cannot_supply_a_file_rate() {
    let mut p = policy();
    p["file"] = json!({"max_bit_rate": 1000});
    for duration in [
        Value::Null,
        json!({"ticks": 0, "timescale": 90_000, "seconds": 0}),
    ] {
        let mut doc = document();
        doc["duration"] = duration;
        let verdict = evaluate(&Policy::from_value(&p).unwrap(), &doc);
        assert_eq!(verdict.reasons.len(), 1);
        assert_eq!(verdict.reasons[0].code, Code::Unknown);
    }
    let mut doc = document();
    doc.as_object_mut().unwrap().remove("duration");
    assert_eq!(
        evaluate(&Policy::from_value(&p).unwrap(), &doc).reasons[0].code,
        Code::Unknown
    );
}

#[test]
fn unidentified_streams_count_only_towards_the_total() {
    let mut doc = document();
    doc["streams"][0]["codec"] = json!("other");
    doc["streams"][0]["duration"] = Value::Null;
    let mut p = policy();
    p["duration"] = json!({"max_seconds": 20});
    p["streams"]["audio"] = json!({"min_count": 1});
    p["streams"]["max_count"] = json!(0);
    p["streams"]["unidentified"] = json!("ignore");
    let verdict = evaluate(&Policy::from_value(&p).unwrap(), &doc);
    assert_eq!(verdict.reasons.len(), 2);
    assert_eq!(
        verdict.reasons[0].rule.as_deref(),
        Some("/streams/max_count")
    );
    assert_eq!(
        verdict.reasons[1].rule.as_deref(),
        Some("/streams/audio/min_count")
    );
}

#[test]
fn null_transport_rotation_is_unknown_rather_than_a_non_rotation_matrix() {
    let mut p = policy();
    p["streams"]["video"] = json!({"rotations": [0]});
    let mut doc = document();
    doc["streams"][0]["codec"] = json!("h264");
    doc["streams"][0]["video"] = json!({"rotation": null});
    let verdict = evaluate(&Policy::from_value(&p).unwrap(), &doc);
    assert_eq!(verdict.reasons.len(), 1);
    assert_eq!(verdict.reasons[0].code, Code::Unknown);
    assert_eq!(
        verdict.reasons[0].path.as_deref(),
        Some("/streams/0/video/rotation")
    );
}
