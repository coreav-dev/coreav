//! Synthetic serializer and schema checks, not native encoding evidence.

#![cfg(feature = "svt-av1")]
#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

mod schema;

use coreav_cli::transcode;
use coreav_decode::{BackendKind, Codec, DecoderInfo, Isolation};
use coreav_frame::{ChromaLocation, ColorInfo};
use coreav_isobmff_write::{RemuxSummary, RemuxTrack};
use coreav_transcode::encoder::FrameRate;
use coreav_transcode::{Av1Encoding, Av1Summary, Av1VideoSummary, Remuxed};
use serde_json::{Value, json};

const V1: &str = include_str!("../../../docs/schema/transcode-v1.schema.json");
const V2: &str = include_str!("../../../docs/schema/transcode-v2.schema.json");
const EXAMPLE: &str = include_str!("../../../docs/schema/transcode-v2.example.json");

fn summary() -> Av1Summary {
    Av1Summary {
        video: Av1VideoSummary {
            stream_index: 0,
            codec: Codec::H264,
            frames: 4,
            not_presented: 0,
            decoder: DecoderInfo {
                backend: BackendKind::Native,
                isolation: Isolation::InProcess,
                decoder: "synthetic".into(),
            },
            skipped: Vec::new(),
            width: 64,
            height: 64,
            sample_aspect: None,
            encoding: Av1Encoding {
                version: "4.2.0".into(),
                preset: 8,
                crf: 32,
                cadence: FrameRate { num: 120, den: 7 },
                color: ColorInfo {
                    primaries: Some(1),
                    transfer: Some(1),
                    matrix: Some(1),
                    full_range: Some(false),
                    chroma_location: Some(ChromaLocation::Left),
                },
                keyint: 85,
                retained_budget: 40_000,
                fixed_retained_bytes: 256,
                retained_picture_bytes: 6400,
                segment_frames: 6,
                segments: 1,
                address_space: 1 << 30,
                #[cfg(feature = "av1-measurement")]
                in_process: false,
            },
        },
        audio: None,
        remux: Remuxed::Mp4(RemuxSummary {
            tracks: vec![
                RemuxTrack {
                    stream_index: 0,
                    source_track_id: 1,
                    track_id: 1,
                    kind: coreav_isobmff::TrackKind::Video,
                    codec: coreav_isobmff::Codec::Av1,
                    samples: 4,
                    timescale: 30,
                    duration: 7,
                    replaced: true,
                },
                RemuxTrack {
                    stream_index: 1,
                    source_track_id: 2,
                    track_id: 2,
                    kind: coreav_isobmff::TrackKind::Audio,
                    codec: coreav_isobmff::Codec::Aac,
                    samples: 4,
                    timescale: 48_000,
                    duration: 4096,
                    replaced: false,
                },
            ],
            dropped_tracks: Vec::new(),
            dropped_boxes: Vec::new(),
            dropped_boxes_not_listed: 0,
            cleared: Vec::new(),
            source_moov_first: true,
            bytes_written: 4096,
        }),
    }
}

fn legacy_summary(av1: Av1Summary) -> coreav_transcode::Summary {
    coreav_transcode::Summary {
        video: coreav_transcode::VideoSummary {
            stream_index: av1.video.stream_index,
            codec: av1.video.codec,
            frames: av1.video.frames,
            dropped_frames: 2,
            not_presented: av1.video.not_presented,
            decoder: av1.video.decoder,
            skipped: av1.video.skipped,
            encoder: "x264",
            width: av1.video.width,
            height: av1.video.height,
            sample_aspect: Some((4, 3)),
            frame_rate: FrameRate { num: 30, den: 1 },
        },
        audio: av1.audio,
        remux: av1.remux,
    }
}

#[test]
fn av1_document_matches_the_illustrative_example_and_schema() {
    let document = transcode::av1::document(&summary());
    let expected: Value = serde_json::from_str(EXAMPLE).unwrap();
    assert_eq!(document, expected);
    schema::assert_matches(&document, V2);
    assert_eq!(document["video"]["codec"], "h264");
    assert_eq!(document["video"]["encoding"]["codec"], "av1");
    assert_eq!(document["video"]["encoding"]["thread_limit"], Value::Null);
    assert!(document["video"].get("dropped_frames").is_none());
}

#[test]
fn legacy_h264_document_keeps_its_exact_fields_and_schema() {
    let summary = legacy_summary(summary());
    let document = transcode::document(&summary);
    let expected = json!({
        "schema": "coreav.transcode/1",
        "output": {"bytes": 4096},
        "video": {
            "stream_index": 0, "codec": "h264", "frames": 4,
            "dropped_frames": 2, "not_presented": 0, "backend": "native",
            "encoder": "x264", "width": 64, "height": 64,
            "sample_aspect": "4:3", "frame_rate": {"num": 30, "den": 1},
        },
        "audio": [{"stream_index": 1, "codec": "aac"}],
        "encoded_audio": null, "other": [], "dropped": [],
    });
    assert_eq!(document, expected);
    assert_eq!(
        serde_json::to_vec(&document).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    schema::assert_matches(&document, V1);
}

#[test]
fn encoded_audio_fields_remain_shared_between_versions() {
    let mut summary = summary();
    summary.audio = Some(coreav_transcode::AudioSummary {
        stream_index: 1,
        codec: coreav_audio::Codec::Flac,
        sample_rate: 48_000,
        source_channels: 2,
        encoder: "aac",
        channels: 2,
        bitrate: None,
        samples: 4096,
        packets: 5,
        pre_skip: 1024,
        #[cfg(feature = "input-capture")]
        input_capture: None,
    });
    let v2 = transcode::av1::document(&summary);
    let v1 = transcode::document(&legacy_summary(summary));
    let expected = json!({
        "stream_index": 1, "codec": "flac", "sample_rate": 48000,
        "encoder": "aac", "channels": 2, "bit_rate": null,
        "samples": 4096, "pre_skip": 1024, "packets": 5,
    });
    assert_eq!(v2["encoded_audio"], expected);
    assert_eq!(v1["encoded_audio"], expected);
    schema::assert_matches(&v1, V1);
    schema::assert_matches(&v2, V2);
}

#[test]
#[cfg(feature = "input-capture")]
fn requested_capture_retains_its_existing_optional_fields() {
    let mut summary = summary();
    summary.audio = Some(coreav_transcode::AudioSummary {
        stream_index: 1,
        codec: coreav_audio::Codec::Flac,
        sample_rate: 48_000,
        source_channels: 2,
        encoder: "aac",
        channels: 2,
        bitrate: None,
        samples: 4096,
        packets: 5,
        pre_skip: 1024,
        input_capture: Some(coreav_transcode::audio::EncoderInput {
            format: "f32le",
            pcm: coreav_transcode::capture::Receipt {
                path: "synthetic.pcm".into(),
                bytes: 32768,
                sha256: "0".repeat(64),
            },
            encoder_sha256: "1".repeat(64),
            argv: vec!["synthetic".into()],
            launcher_argv: vec!["synthetic".into()],
            sample_rate: 48_000,
            channel_mask: 3,
            start_ticks: 0,
            sandboxed: true,
            address_space: Some(1 << 30),
            open_files: Some(32),
        }),
    });
    let v2 = transcode::av1::document(&summary);
    let v1 = transcode::document(&legacy_summary(summary));
    assert_eq!(v2["input_capture"], v1["input_capture"]);
    assert_eq!(v2["input_capture"]["pcm"]["bytes"], 32768);
    schema::assert_matches(&v2, V2);
    let contract: Value = serde_json::from_str(V2).unwrap();
    for (pointer, value) in [
        ("/input_capture/pcm/bytes", json!((64u64 << 20) + 1)),
        ("/input_capture/pcm/sha256", json!("short")),
        ("/input_capture/encoder_sha256", json!("short")),
    ] {
        let mut changed = v2.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            schema::validate(&changed, &contract, &contract, "$").is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn shared_remux_and_audio_contracts_match_version_one() {
    let v1: Value = serde_json::from_str(V1).unwrap();
    let v2: Value = serde_json::from_str(V2).unwrap();
    for key in ["output", "audio", "encoded_audio", "other", "dropped"] {
        assert_eq!(v2["properties"][key], v1["properties"][key], "{key}");
    }
    assert_eq!(
        v2["properties"]["video"]["properties"]["codec"],
        v1["properties"]["video"]["properties"]["codec"]
    );
}

#[test]
fn unsupported_or_misreported_encoder_facts_fail_the_schema() {
    let contract: Value = serde_json::from_str(V2).unwrap();
    let original = transcode::av1::document(&summary());
    for (pointer, value) in [
        ("/video/encoding/preset", json!(14)),
        ("/video/encoding/crf", json!(64)),
        ("/video/encoding/cadence/den", json!(0)),
        ("/video/encoding/parallelism_level", json!(0)),
        ("/video/encoding/thread_limit", json!(1)),
        ("/video/encoding/kind", json!("hardware")),
        ("/video/encoding/color/transfer", json!(16)),
        ("/video/encoding/color/full_range", Value::Null),
        ("/video/encoding/retained_budget", json!((1u64 << 30) + 1)),
        ("/video/encoding/segments", json!(0)),
        ("/video/width", json!(65)),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            schema::validate(&changed, &contract, &contract, "$").is_err(),
            "{pointer}"
        );
    }
    for key in ["version", "isolation", "segment_frames", "address_space"] {
        let mut changed = original.clone();
        changed["video"]["encoding"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(
            schema::validate(&changed, &contract, &contract, "$").is_err(),
            "{key}"
        );
    }
}
