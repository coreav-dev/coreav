//! Aggregate admission for synthetic Matroska planning operations.
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use common::*;
use coreav_isobmff::PlanningBudget;
use coreav_mkv::{
    Allowlist, Codec, Error, IndexSource, LimitKind, Limits, SliceSource, probe,
    probe_with_planning,
};

fn fixture(cues: Vec<u8>) -> Vec<u8> {
    file(
        "matroska",
        &[
            info(None),
            el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[])),
            cues,
            cluster(0, &[simple_block(1, 0, true, &[0; 4])]),
            cluster(100, &[simple_block(1, 0, true, &[0; 4])]),
        ],
    )
}

fn planning_error(error: Error) {
    assert!(
        matches!(
            error,
            Error::LimitExceeded {
                limit: LimitKind::PlanningBytes,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn benign_probe_index_and_packets_keep_native_results() {
    let data = fixture(Vec::new());
    let limits = Limits::default();
    let allowlist = Allowlist::all();
    let native = probe(SliceSource::new(&data), &limits, &allowlist).unwrap();
    let planning = PlanningBudget::new(1 << 20);
    let bounded = probe_with_planning(
        SliceSource::new(&data),
        &limits,
        &allowlist,
        planning.clone(),
    )
    .unwrap();
    assert_eq!(bounded.tracks, native.tracks);
    assert_eq!(bounded.dropped, native.dropped);
    assert_eq!(bounded.muxing_app, native.muxing_app);
    assert_eq!(bounded.writing_app, native.writing_app);
    assert_eq!(bounded.warnings, native.warnings);
    let probe_bytes = planning.used();
    let native_index = native.index(SliceSource::new(&data), &limits).unwrap();
    assert_eq!(
        bounded.index(SliceSource::new(&data), &limits).unwrap(),
        native_index
    );
    assert!(planning.used() > probe_bytes);
    let index_bytes = planning.used();
    let native_packets: Vec<_> = native.packets(SliceSource::new(&data), &limits).collect();
    let bounded_packets: Vec<_> = bounded.packets(SliceSource::new(&data), &limits).collect();
    assert_eq!(bounded_packets, native_packets);
    assert!(planning.used() > index_bytes);
    assert!(!planning.failed());
}

#[test]
fn malformed_cues_fall_back_but_planning_exhaustion_does_not() {
    let data = fixture(el(CUES, &el(0xBB, &[0])));
    let limits = Limits::default();
    let allowlist = Allowlist::all();
    let generous = PlanningBudget::new(1 << 20);
    let segment = probe_with_planning(
        SliceSource::new(&data),
        &limits,
        &allowlist,
        generous.clone(),
    )
    .unwrap();
    let probe_bytes = generous.used();
    let index = segment.index(SliceSource::new(&data), &limits).unwrap();
    assert_eq!(index.source, IndexSource::ClusterScan);
    assert_eq!(index.points.len(), 2);
    let tiny = PlanningBudget::new(probe_bytes);
    let bounded =
        probe_with_planning(SliceSource::new(&data), &limits, &allowlist, tiny.clone()).unwrap();
    planning_error(bounded.index(SliceSource::new(&data), &limits).unwrap_err());
    assert_eq!(tiny.used(), probe_bytes);
    assert!(tiny.failed());
    planning_error(bounded.index(SliceSource::new(&data), &limits).unwrap_err());
    planning_error(
        bounded
            .packets(SliceSource::new(&data), &limits)
            .next()
            .unwrap()
            .unwrap_err(),
    );
}

#[test]
fn full_listing_and_repeated_indexes_share_the_probe_account() {
    let data = fixture(Vec::new());
    let limits = Limits::default();
    let allowlist = Allowlist::all();
    let generous = PlanningBudget::new(1 << 20);
    let segment = probe_with_planning(
        SliceSource::new(&data),
        &limits,
        &allowlist,
        generous.clone(),
    )
    .unwrap();
    let probe_bytes = generous.used();
    segment.index(SliceSource::new(&data), &limits).unwrap();
    let once = generous.used();
    segment.index(SliceSource::new(&data), &limits).unwrap();
    assert_eq!(generous.used() - once, once - probe_bytes);
    let tiny = PlanningBudget::new(probe_bytes);
    let bounded =
        probe_with_planning(SliceSource::new(&data), &limits, &allowlist, tiny.clone()).unwrap();
    let mut packets = bounded.packets(SliceSource::new(&data), &limits);
    planning_error(packets.next().unwrap().unwrap_err());
    assert!(packets.next().is_none());
    assert_eq!(tiny.used(), probe_bytes);
    assert!(tiny.failed());
}

#[test]
fn allowlist_refuses_before_owning_codec_capacity() {
    let planning = PlanningBudget::new(size_of::<Codec>() - 1);
    planning_error(Allowlist::new_with_planning(&[Codec::H264], &planning).unwrap_err());
    assert_eq!(planning.used(), 0);
    assert!(planning.failed());
    let planning = PlanningBudget::new(3 * size_of::<Codec>());
    let codecs = [Codec::H264, Codec::Other, Codec::H264];
    let bounded = Allowlist::new_with_planning(&codecs, &planning).unwrap();
    assert_eq!(bounded, Allowlist::new(&codecs));
    assert_eq!(planning.used(), 3 * size_of::<Codec>());
    let planning = PlanningBudget::new(1 << 20);
    assert_eq!(
        Allowlist::default_with_planning(&planning).unwrap(),
        Allowlist::default()
    );
}

#[test]
fn replaced_metadata_and_fallback_work_are_never_refunded() {
    let title = string(0x7BA9, "replaced");
    let tracks = el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[]));
    let once = file("matroska", &[el(INFO, &title), tracks.clone()]);
    let twice = file(
        "matroska",
        &[el(INFO, &cat(&[title.clone(), title.clone()])), tracks],
    );
    let limits = Limits::default();
    let allowlist = Allowlist::all();
    let first = PlanningBudget::new(1 << 20);
    probe_with_planning(SliceSource::new(&once), &limits, &allowlist, first.clone()).unwrap();
    let second = PlanningBudget::new(1 << 20);
    probe_with_planning(
        SliceSource::new(&twice),
        &limits,
        &allowlist,
        second.clone(),
    )
    .unwrap();
    assert_eq!(second.used() - first.used(), title.len() + "replaced".len());

    let cue = el(
        0xBB,
        &cat(&[
            uint(0xB3, 0),
            el(0xB7, &cat(&[uint(0xF7, 1), uint(0xF1, 0)])),
        ]),
    );
    let data = fixture(el(CUES, &cat(&[cue, el(0xBB, &[0])])));
    let generous = PlanningBudget::new(1 << 20);
    let segment = probe_with_planning(
        SliceSource::new(&data),
        &limits,
        &allowlist,
        generous.clone(),
    )
    .unwrap();
    let probe_bytes = generous.used();
    assert_eq!(
        segment
            .index(SliceSource::new(&data), &limits)
            .unwrap()
            .source,
        IndexSource::ClusterScan
    );
    let complete = generous.used();
    for max in probe_bytes..complete {
        let planning = PlanningBudget::new(max);
        let segment = probe_with_planning(
            SliceSource::new(&data),
            &limits,
            &allowlist,
            planning.clone(),
        )
        .unwrap();
        planning_error(segment.index(SliceSource::new(&data), &limits).unwrap_err());
        assert!(planning.failed());
        assert!(planning.used() <= max);
    }
}

#[test]
fn failed_shared_account_releases_no_pending_lace_packets() {
    let data = file(
        "matroska",
        &[
            info(None),
            el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[])),
            cluster(
                0,
                &[el(SIMPLE_BLOCK, &[0x81, 0, 0, 0x84, 2, 1, 2, 3, 4, 5, 6])],
            ),
        ],
    );
    let planning = PlanningBudget::new(1 << 20);
    let limits = Limits::default();
    let segment = probe_with_planning(
        SliceSource::new(&data),
        &limits,
        &Allowlist::all(),
        planning.clone(),
    )
    .unwrap();
    let mut packets = segment.packets(SliceSource::new(&data), &limits);
    assert_eq!(packets.next().unwrap().unwrap().frames_in_block, 3);
    assert!(planning.charge(usize::MAX).is_err());
    planning_error(packets.next().unwrap().unwrap_err());
    assert!(packets.next().is_none());
}

#[test]
fn best_effort_frame_header_scan_cannot_hide_planning_exhaustion() {
    let head = [
        info(None),
        el(TRACKS, &video_track(1, "V_MPEG4/ISO/AVC", 16, 16, &[])),
    ];
    let without_cluster = file("matroska", &head);
    let with_cluster = file(
        "matroska",
        &[
            head.to_vec(),
            vec![cluster(0, &[simple_block(1, 0, true, &[0; 4])])],
        ]
        .concat(),
    );
    let limits = Limits::default();
    let allowlist = Allowlist::all();
    let generous = PlanningBudget::new(1 << 20);
    probe_with_planning(
        SliceSource::new(&without_cluster),
        &limits,
        &allowlist,
        generous.clone(),
    )
    .unwrap();
    let tiny = PlanningBudget::new(generous.used());
    planning_error(
        probe_with_planning(
            SliceSource::new(&with_cluster),
            &limits,
            &allowlist,
            tiny.clone(),
        )
        .unwrap_err(),
    );
    assert!(tiny.failed());
    assert!(probe(SliceSource::new(&with_cluster), &limits, &allowlist).is_ok());
}
