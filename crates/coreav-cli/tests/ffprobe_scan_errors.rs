//! Optional ISO report and edit scans retain source-limit failures.

#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod common;
#[path = "../../coreav-isobmff/tests/common/text_movie.rs"]
mod text_movie;

use coreav_cli::error::Failure;
use coreav_cli::ffprobe::model::Value;
use coreav_cli::ffprobe::{self, Probed};
use coreav_isobmff::{Error, LimitKind, Limits, SliceSource};

fn probe(bytes: Vec<u8>, limits: &Limits) -> Result<Probed<'static>, Failure> {
    let bytes = Box::leak(bytes.into_boxed_slice());
    let source = Box::leak(Box::new(SliceSource::new(bytes)));
    ffprobe::probe_limited(source, bytes.len() as u64, "scan", None, limits)
}

fn limited() -> Limits {
    Limits {
        max_packet_bytes: 50,
        ..Limits::default()
    }
}

fn packet_limit<T>(result: Result<T, Failure>) {
    match result {
        Err(Failure::Library(Error::LimitExceeded {
            limit: LimitKind::PacketBytes,
            ..
        })) => {}
        Err(error) => panic!("unexpected failure: {}", error.to_json()),
        Ok(_) => panic!("expected the packet-size limit error"),
    }
}

fn video(oversized: usize, edits: Option<Vec<(u32, i32)>>) -> Vec<u8> {
    let mut sizes = vec![10; 128];
    sizes[oversized] = 100;
    common::movie(&common::Spec {
        sizes,
        composition: Vec::new(),
        ctts: Some(Vec::new()),
        sync: Vec::new(),
        elst: edits,
        ..common::Spec::default()
    })
}

fn count(probed: &mut Probed<'_>) -> (Result<(), Failure>, usize) {
    let mut count = 0;
    let payloads = vec![false; probed.report.streams.len()];
    let result = probed.packets.visit(&payloads, &mut |_| {
        count += 1;
        Ok(true)
    });
    (result, count)
}

#[test]
fn a_first_packets_limit_fails_the_report() {
    assert!(probe(video(40, None), &Limits::default()).is_ok());
    packet_limit(probe(video(40, None), &limited()));
}

#[test]
fn a_malformed_sample_still_ends_the_first_packets_scan_quietly() {
    let mut bytes = video(40, None);
    let at = bytes.windows(4).position(|w| w == b"stsz").unwrap() + 16 + 4 * 40;
    bytes[at..at + 4].copy_from_slice(&1_000_000u32.to_be_bytes());
    assert!(probe(bytes, &Limits::default()).is_ok());
}

#[test]
fn an_audio_priming_scan_limit_fails_the_report() {
    let mut samples = vec![(vec![0; 10], 10); 300];
    samples[80].0.resize(100, 0);
    let track = text_movie::TrackSpec {
        handler: *b"soun",
        entry: common::aac_entry(),
        elst: Some(vec![(500, 2500)]),
        ..text_movie::TrackSpec::tx3g(1, samples)
    };
    let bytes = text_movie::movie(&[track]);
    let default = probe(bytes.clone(), &Limits::default()).ok().unwrap();
    assert_eq!(
        default.report.streams[0]
            .entries
            .iter()
            .find(|(name, _)| *name == "initial_padding")
            .map(|(_, value)| value),
        Some(&Value::Int(48_000))
    );
    packet_limit(probe(bytes, &limited()));
}

#[test]
fn a_simple_edit_scan_limit_fails_before_the_first_packet() {
    let bytes = video(100, Some(vec![(4500, 0)]));
    let mut default = probe(bytes.clone(), &Limits::default()).ok().unwrap();
    let (result, total) = count(&mut default);
    assert!(result.is_ok());
    assert_eq!(total, 113);
    let mut probed = probe(bytes, &limited()).ok().unwrap();
    let (result, emitted) = count(&mut probed);
    packet_limit(result);
    assert_eq!(emitted, 0);
}

#[test]
fn a_multiple_edit_scan_limit_fails_before_the_first_packet() {
    let bytes = video(100, Some(vec![(2000, 0), (2000, 25600)]));
    let mut default = probe(bytes.clone(), &Limits::default()).ok().unwrap();
    let (result, total) = count(&mut default);
    assert!(result.is_ok());
    assert_eq!(total, 100);
    let mut probed = probe(bytes, &limited()).ok().unwrap();
    let (result, emitted) = count(&mut probed);
    packet_limit(result);
    assert_eq!(emitted, 0);
}

#[test]
fn splice_work_exhaustion_keeps_the_library_listing() {
    let bytes = common::movie(&common::Spec {
        sizes: vec![10; 128],
        composition: Vec::new(),
        ctts: Some(Vec::new()),
        sync: Vec::new(),
        elst: Some(vec![(5120, 0); 4]),
        ..common::Spec::default()
    });
    let mut default = probe(bytes.clone(), &Limits::default()).ok().unwrap();
    let (result, total) = count(&mut default);
    assert!(result.is_ok());
    assert_eq!(total, 512);
    let limits = Limits {
        max_sample_work: 512,
        ..Limits::default()
    };
    let mut probed = probe(bytes, &limits).ok().unwrap();
    let mut times = Vec::new();
    let result = probed.packets.visit(&[false], &mut |packet| {
        times.push(packet.dts);
        Ok(true)
    });
    assert!(result.is_ok());
    assert_eq!(times, vec![None; 128]);
}
