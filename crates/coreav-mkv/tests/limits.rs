//! Each field of `Limits` at its boundary, and the default values.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::*;
use coreav_mkv::{
    Allowlist, Error, IndexSource, LimitKind, Limits, Packet, SeekPoint, Segment, SliceSource,
    Warning, probe,
};

fn probe_with(data: &[u8], limits: &Limits) -> Result<Segment, Error> {
    probe(SliceSource::new(data), limits, &Allowlist::all())
}

fn list(data: &[u8], limits: &Limits) -> Result<Vec<Packet>, Error> {
    probe_with(data, &Limits::default())
        .unwrap()
        .packets(SliceSource::new(data), limits)
        .collect()
}

fn limit_of(result: Result<impl Sized, Error>) -> LimitKind {
    match result.err() {
        Some(Error::LimitExceeded { limit, .. }) => limit,
        other => panic!("expected a limit error, got {other:?}"),
    }
}

fn vint(data: &[u8], keep_marker: bool) -> (u64, usize) {
    let len = data[0].leading_zeros() as usize + 1;
    let first = if keep_marker {
        data[0]
    } else {
        data[0] & (0xFF >> len)
    };
    let value = data[1..len].iter().fold(u64::from(first), |value, byte| {
        value << 8 | u64::from(*byte)
    });
    (value, len)
}

/// The element headers in `data`, counted from the bytes alone, descending
/// into the masters named.
fn headers(data: &[u8], masters: &[u32]) -> u64 {
    let mut count = 0;
    let mut at = 0;
    while at < data.len() {
        let (id, id_len) = vint(&data[at..], true);
        let (size, size_len) = vint(&data[at + id_len..], false);
        let body = at + id_len + size_len;
        let end = body + size as usize;
        count += 1;
        if masters.contains(&(id as u32)) {
            count += headers(&data[body..end], masters);
        }
        at = end;
    }
    count
}

/// The strings `info(None)` holds: its muxing and writing applications.
const INFO_STRINGS: u64 = 2 * "coreav-tests".len() as u64;
/// The strings `tracks()` holds: its codec ID and the default language.
const TRACK_STRINGS: u64 = ("A_OPUS".len() + "eng".len()) as u64;

fn tracks() -> Vec<u8> {
    el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[]))
}

fn padded(len: usize, fixed: &[u8]) -> Vec<u8> {
    let pad = (0..len)
        .find(|pad| fixed.len() + el(VOID, &vec![0; *pad]).len() == len)
        .unwrap();
    [fixed, &el(VOID, &vec![0; pad])].concat()
}

fn blocks(count: usize) -> Vec<Vec<u8>> {
    (0..count)
        .map(|i| simple_block(1, (i % 1000) as i16, true, &[0; 4]))
        .collect()
}

fn cluster_file(clusters: usize) -> Vec<u8> {
    let clusters: Vec<Vec<u8>> = (0..clusters)
        .map(|i| cluster(i as u64, &[simple_block(1, 0, true, &[0; 4])]))
        .collect();
    file("webm", &[&[info(None), tracks()][..], &clusters].concat())
}

fn cue_file(cue_points: usize) -> (Vec<u8>, Vec<u8>) {
    let cues = |position: u64| {
        let points: Vec<Vec<u8>> = (0..cue_points)
            .map(|i| {
                el(
                    0xBB,
                    &cat(&[
                        uint(0xB3, i as u64),
                        el(
                            0xB7,
                            &cat(&[uint(0xF7, 1), el(0xF1, &position.to_be_bytes())]),
                        ),
                    ]),
                )
            })
            .collect();
        el(CUES, &points.concat())
    };
    let head_len = info(None).len() + tracks().len() + cues(0).len();
    let cues = cues(head_len as u64);
    let data = file(
        "webm",
        &[
            info(None),
            tracks(),
            cues.clone(),
            cluster(0, &[simple_block(1, 0, true, &[0; 4])]),
            cluster(1, &[simple_block(1, 0, true, &[0; 4])]),
        ],
    );
    (data, cues)
}

#[test]
fn defaults_match_the_documented_values() {
    assert_eq!(
        Limits::default(),
        Limits {
            max_elements: 1_000_000,
            max_element_bytes: 1 << 20,
            max_retained_bytes: 16 << 20,
            max_index_bytes: 256 << 20,
            max_packet_bytes: 128 << 20,
            max_block_work: 1 << 26,
            max_tracks: 1_024,
            max_inflated_bytes: 256 << 20,
        }
    );
}

#[test]
fn every_element_header_of_the_probe_counts_against_max_elements() {
    for voids in [0, 1_000] {
        let data = file("webm", &[el(INFO, &[0xEC, 0x80].repeat(voids)), tracks()]);
        let count = headers(&data, &[EBML, SEGMENT, INFO, TRACKS, TRACK_ENTRY, 0xE1]);
        let limits = |max_elements: u64| Limits {
            max_elements: max_elements as u32,
            ..Limits::default()
        };
        assert!(probe_with(&data, &limits(count)).is_ok(), "{voids} voids");
        assert_eq!(
            limit_of(probe_with(&data, &limits(count - 1))),
            LimitKind::Elements,
            "{voids} voids"
        );
    }
}

#[test]
fn cues_of_more_element_headers_than_max_elements_are_not_used_as_the_index() {
    for cue_points in [1, 41] {
        let (data, cues) = cue_file(cue_points);
        let count = headers(&cues, &[CUES, 0xBB, 0xB7]);
        let segment = probe_with(&data, &Limits::default()).unwrap();
        let index = |max_elements: u64| {
            let limits = Limits {
                max_elements: max_elements as u32,
                ..Limits::default()
            };
            segment.index(SliceSource::new(&data), &limits).unwrap()
        };
        let all = index(count);
        assert_eq!(all.source, IndexSource::Cues, "{cue_points} cue points");
        assert_eq!(all.points.len(), cue_points);
        let fewer = index(count - 1);
        assert_eq!(
            fewer.source,
            IndexSource::ClusterScan,
            "{cue_points} cue points"
        );
        assert!(fewer.warnings.contains(&Warning::InvalidCuePoint));
    }
}

#[test]
fn an_info_of_exactly_max_element_bytes_is_read_and_one_more_is_refused() {
    let limits = Limits {
        max_element_bytes: 64,
        ..Limits::default()
    };
    let info_of = |len| {
        let body = padded(len, &uint(0x2A_D7B1, 1_000_000));
        file("webm", &[el(INFO, &body), tracks()])
    };
    assert!(probe_with(&info_of(64), &limits).is_ok());
    assert_eq!(
        limit_of(probe_with(&info_of(65), &limits)),
        LimitKind::ElementBytes
    );
}

#[test]
fn a_track_entry_of_exactly_max_element_bytes_is_read_and_one_more_is_refused() {
    let limits = Limits {
        max_element_bytes: 200,
        ..Limits::default()
    };
    let fixed = cat(&[
        uint(0xD7, 1),
        uint(0x73C5, 1000),
        uint(0x83, 2),
        string(0x86, "A_OPUS"),
        el(0xE1, &cat(&[float(0xB5, 48_000.0), uint(0x9F, 2)])),
    ]);
    let entry_of = |len| {
        file(
            "webm",
            &[
                info(None),
                el(TRACKS, &el(TRACK_ENTRY, &padded(len, &fixed))),
            ],
        )
    };
    assert!(probe_with(&entry_of(200), &limits).is_ok());
    assert_eq!(
        limit_of(probe_with(&entry_of(201), &limits)),
        LimitKind::ElementBytes
    );
}

#[test]
fn a_block_header_of_exactly_max_element_bytes_is_read_and_one_more_is_refused() {
    let mut lace = vec![0x81, 0, 0, 0x82, 255];
    lace.extend(vec![0u8; 255]);
    let header = lace.len() as u64;
    let data = file(
        "webm",
        &[info(None), tracks(), cluster(0, &[el(SIMPLE_BLOCK, &lace)])],
    );
    let limit = |max_element_bytes| Limits {
        max_element_bytes,
        ..Limits::default()
    };
    assert_eq!(list(&data, &limit(header)).unwrap().len(), 256);
    assert_eq!(
        limit_of(list(&data, &limit(header - 1))),
        LimitKind::ElementBytes
    );
}

#[test]
fn codec_private_bytes_count_one_for_one_against_max_retained_bytes() {
    for private in [1_000, 5_000] {
        let track = audio_track(1, "A_OPUS", 48_000.0, 2, &el(0x63A2, &vec![0; private]));
        let data = file("webm", &[info(None), el(TRACKS, &track)]);
        let retained = INFO_STRINGS + TRACK_STRINGS + private as u64;
        let limits = |max_retained_bytes| Limits {
            max_retained_bytes,
            ..Limits::default()
        };
        assert!(probe_with(&data, &limits(retained)).is_ok(), "{private}");
        assert_eq!(
            limit_of(probe_with(&data, &limits(retained - 1))),
            LimitKind::RetainedBytes,
            "{private}"
        );
    }
}

#[test]
fn info_strings_count_one_for_one_against_max_retained_bytes() {
    for title in [1_000, 3_000] {
        let body = cat(&[uint(0x2A_D7B1, 1_000_000), el(0x7BA9, &vec![b'a'; title])]);
        let data = file("webm", &[el(INFO, &body), tracks()]);
        let retained = title as u64 + TRACK_STRINGS;
        let limits = |max_retained_bytes| Limits {
            max_retained_bytes,
            ..Limits::default()
        };
        assert!(probe_with(&data, &limits(retained)).is_ok(), "{title}");
        assert_eq!(
            limit_of(probe_with(&data, &limits(retained - 1))),
            LimitKind::RetainedBytes,
            "{title}"
        );
    }
}

#[test]
fn a_cluster_scan_keeps_at_most_max_index_bytes_of_seek_points() {
    let data = cluster_file(20);
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let point = std::mem::size_of::<SeekPoint>() as u64;
    let index = |max_index_bytes| {
        let limits = Limits {
            max_index_bytes,
            ..Limits::default()
        };
        segment.index(SliceSource::new(&data), &limits)
    };
    let all = index(20 * point).unwrap();
    assert_eq!(all.source, IndexSource::ClusterScan);
    assert_eq!(all.points.len(), 20);
    assert_eq!(limit_of(index(20 * point - 1)), LimitKind::IndexBytes);
}

#[test]
fn cues_past_max_index_bytes_are_not_used_as_the_index() {
    let (data, _) = cue_file(10);
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let point = std::mem::size_of::<SeekPoint>() as u64;
    let index = |max_index_bytes| {
        let limits = Limits {
            max_index_bytes,
            ..Limits::default()
        };
        segment.index(SliceSource::new(&data), &limits).unwrap()
    };
    let all = index(10 * point);
    assert_eq!(all.source, IndexSource::Cues);
    assert_eq!(all.points.len(), 10);
    let fewer = index(10 * point - 1);
    assert_eq!(fewer.source, IndexSource::ClusterScan);
    assert!(fewer.warnings.contains(&Warning::InvalidCuePoint));
}

#[test]
fn a_frame_of_exactly_max_packet_bytes_is_listed_and_one_more_is_refused() {
    let limits = Limits {
        max_packet_bytes: 10,
        ..Limits::default()
    };
    let with_frame = |len: usize| {
        file(
            "webm",
            &[
                info(None),
                tracks(),
                cluster(0, &[simple_block(1, 0, true, &vec![0; len])]),
            ],
        )
    };
    assert_eq!(list(&with_frame(10), &limits).unwrap().len(), 1);
    assert_eq!(
        limit_of(list(&with_frame(11), &limits)),
        LimitKind::PacketBytes
    );
}

#[test]
fn a_laced_frame_over_max_packet_bytes_refuses_the_whole_block() {
    let limits = Limits {
        max_packet_bytes: 10,
        ..Limits::default()
    };
    let block = |second: usize| {
        let mut body = vec![0x81, 0, 0, 0x82, 1, 10];
        body.extend(vec![0; 10]);
        body.extend(vec![0; second]);
        file(
            "webm",
            &[info(None), tracks(), cluster(0, &[el(SIMPLE_BLOCK, &body)])],
        )
    };
    assert_eq!(list(&block(10), &limits).unwrap().len(), 2);
    assert_eq!(limit_of(list(&block(11), &limits)), LimitKind::PacketBytes);
}

#[test]
fn every_element_header_of_a_listing_counts_against_max_block_work() {
    for count in [10, 1_010] {
        let clusters = cluster(0, &blocks(count));
        let work = headers(&clusters, &[CLUSTER]);
        let data = file("webm", &[info(None), tracks(), clusters]);
        let limits = |max_block_work| Limits {
            max_block_work,
            ..Limits::default()
        };
        assert_eq!(list(&data, &limits(work)).unwrap().len(), count);
        assert_eq!(
            limit_of(list(&data, &limits(work - 1))),
            LimitKind::BlockWork,
            "{count} blocks"
        );
    }
}

#[test]
fn a_cluster_scan_reads_two_element_headers_a_cluster_against_max_block_work() {
    for clusters in [50, 150] {
        let data = cluster_file(clusters);
        let segment = probe_with(&data, &Limits::default()).unwrap();
        let work = 2 * clusters as u64;
        let index = |max_block_work| {
            let limits = Limits {
                max_block_work,
                ..Limits::default()
            };
            segment.index(SliceSource::new(&data), &limits)
        };
        assert_eq!(index(work).unwrap().points.len(), clusters);
        assert_eq!(
            limit_of(index(work - 1)),
            LimitKind::BlockWork,
            "{clusters} clusters"
        );
    }
}

/// Each zlib frame is charged its inflated bytes plus 4 KiB; a frame the
/// rest of the budget cannot cover has an unknown size, not an error.
#[test]
fn zlib_frames_are_sized_while_their_charge_fits_max_inflated_bytes() {
    const FRAME_CHARGE: u64 = 4 << 10;
    let inflated = 1_000;
    let frame = miniz_oxide::deflate::compress_to_vec_zlib(&vec![7u8; inflated as usize], 9);
    let encoding = el(
        0x6240,
        &cat(&[
            uint(0x5031, 0),
            uint(0x5032, 1),
            uint(0x5033, 0),
            el(0x5034, &uint(0x4254, 0)),
        ]),
    );
    let track = audio_track(1, "A_OPUS", 48_000.0, 2, &el(0x6D80, &encoding));
    let data = file(
        "webm",
        &[
            info(None),
            el(TRACKS, &track),
            cluster(
                0,
                &[
                    simple_block(1, 0, true, &frame),
                    simple_block(1, 1, true, &frame),
                ],
            ),
        ],
    );
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let sizes = |max_inflated_bytes| {
        let limits = Limits {
            max_inflated_bytes,
            ..Limits::default()
        };
        segment
            .packets(SliceSource::new(&data), &limits)
            .with_inflated_sizes()
            .map(|packet| packet.unwrap().inflated_size)
            .collect::<Vec<_>>()
    };
    let one = FRAME_CHARGE + inflated;
    assert_eq!(sizes(2 * one), [Some(inflated), Some(inflated)]);
    assert_eq!(sizes(2 * one - 1), [Some(inflated), None]);
    assert_eq!(sizes(one), [Some(inflated), None]);
    assert_eq!(sizes(one - 1), [None, None]);
}

#[test]
fn exactly_max_tracks_are_read_and_one_more_is_refused() {
    let limits = Limits {
        max_tracks: 10,
        ..Limits::default()
    };
    let with_tracks = |count: u64| {
        let entries: Vec<u8> = (1..=count)
            .flat_map(|n| audio_track(n, "A_OPUS", 48_000.0, 2, &[]))
            .collect();
        file("webm", &[info(None), el(TRACKS, &entries)])
    };
    assert_eq!(
        probe_with(&with_tracks(10), &limits).unwrap().tracks.len(),
        10
    );
    assert_eq!(
        limit_of(probe_with(&with_tracks(11), &limits)),
        LimitKind::Tracks
    );
}
