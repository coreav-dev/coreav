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

fn least(highest: u64, accepted: impl Fn(u64) -> bool) -> u64 {
    assert!(accepted(highest), "{highest} is not enough");
    let (mut low, mut high) = (0, highest);
    while low < high {
        let mid = low + (high - low) / 2;
        if accepted(mid) {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low
}

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

fn cue_file(cue_points: usize) -> Vec<u8> {
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
    file(
        "webm",
        &[
            info(None),
            tracks(),
            cues(head_len as u64),
            cluster(0, &[simple_block(1, 0, true, &[0; 4])]),
            cluster(1, &[simple_block(1, 0, true, &[0; 4])]),
        ],
    )
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
fn each_element_header_in_the_probe_costs_one_of_max_elements() {
    let fewest = |voids: usize| {
        let data = file("webm", &[el(INFO, &[0xEC, 0x80].repeat(voids)), tracks()]);
        let fewest = least(10_000, |max| {
            let limits = Limits {
                max_elements: max as u32,
                ..Limits::default()
            };
            probe_with(&data, &limits).is_ok()
        });
        let below = Limits {
            max_elements: fewest as u32 - 1,
            ..Limits::default()
        };
        assert_eq!(
            limit_of(probe_with(&data, &below)),
            LimitKind::Elements,
            "{voids} voids"
        );
        fewest
    };
    assert_eq!(fewest(1_000), fewest(0) + 1_000);
}

#[test]
fn each_element_header_in_the_cues_costs_one_of_max_elements() {
    let fewest = |cue_points: usize| {
        let data = cue_file(cue_points);
        let segment = probe_with(&data, &Limits::default()).unwrap();
        let from_cues = |max: u64| {
            let limits = Limits {
                max_elements: max as u32,
                ..Limits::default()
            };
            let index = segment.index(SliceSource::new(&data), &limits).unwrap();
            index.source == IndexSource::Cues
        };
        let fewest = least(10_000, from_cues);
        let below = Limits {
            max_elements: fewest as u32 - 1,
            ..Limits::default()
        };
        let index = segment.index(SliceSource::new(&data), &below).unwrap();
        assert_eq!(index.source, IndexSource::ClusterScan);
        assert!(index.warnings.contains(&Warning::InvalidCuePoint));
        fewest
    };
    assert_eq!(fewest(41) - fewest(1), 40 * 5);
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
fn a_block_header_of_more_than_max_element_bytes_is_refused() {
    let mut lace = vec![0x81, 0, 0, 0x82, 255];
    lace.extend(vec![0u8; 255]);
    let data = file(
        "webm",
        &[info(None), tracks(), cluster(0, &[el(SIMPLE_BLOCK, &lace)])],
    );
    let limit = |max_element_bytes| Limits {
        max_element_bytes,
        ..Limits::default()
    };
    assert_eq!(list(&data, &limit(260)).unwrap().len(), 256);
    assert_eq!(limit_of(list(&data, &limit(100))), LimitKind::ElementBytes);
}

#[test]
fn codec_private_bytes_count_one_for_one_against_max_retained_bytes() {
    let fewest = |private: usize| {
        let track = audio_track(1, "A_OPUS", 48_000.0, 2, &el(0x63A2, &vec![0; private]));
        let data = file("webm", &[info(None), el(TRACKS, &track)]);
        let fewest = least(1 << 20, |max| {
            let limits = Limits {
                max_retained_bytes: max,
                ..Limits::default()
            };
            probe_with(&data, &limits).is_ok()
        });
        let below = Limits {
            max_retained_bytes: fewest - 1,
            ..Limits::default()
        };
        assert_eq!(
            limit_of(probe_with(&data, &below)),
            LimitKind::RetainedBytes
        );
        fewest
    };
    assert_eq!(fewest(5_000) - fewest(1_000), 4_000);
}

#[test]
fn info_strings_count_one_for_one_against_max_retained_bytes() {
    let fewest = |title: usize| {
        let body = cat(&[uint(0x2A_D7B1, 1_000_000), el(0x7BA9, &vec![b'a'; title])]);
        let data = file("webm", &[el(INFO, &body), tracks()]);
        least(1 << 20, |max| {
            let limits = Limits {
                max_retained_bytes: max,
                ..Limits::default()
            };
            probe_with(&data, &limits).is_ok()
        })
    };
    assert_eq!(fewest(3_000) - fewest(1_000), 2_000);
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
    let data = cue_file(10);
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
fn each_block_in_a_listing_costs_one_of_max_block_work() {
    let fewest = |count: usize| {
        let data = file("webm", &[info(None), tracks(), cluster(0, &blocks(count))]);
        let fewest = least(1 << 20, |max| {
            let limits = Limits {
                max_block_work: max,
                ..Limits::default()
            };
            list(&data, &limits).is_ok()
        });
        let below = Limits {
            max_block_work: fewest - 1,
            ..Limits::default()
        };
        assert_eq!(limit_of(list(&data, &below)), LimitKind::BlockWork);
        fewest
    };
    assert_eq!(fewest(1_010) - fewest(10), 1_000);
}

#[test]
fn each_cluster_in_an_index_scan_costs_two_of_max_block_work() {
    let fewest = |clusters: usize| {
        let data = cluster_file(clusters);
        let segment = probe_with(&data, &Limits::default()).unwrap();
        let fewest = least(1 << 20, |max| {
            let limits = Limits {
                max_block_work: max,
                ..Limits::default()
            };
            segment.index(SliceSource::new(&data), &limits).is_ok()
        });
        let below = Limits {
            max_block_work: fewest - 1,
            ..Limits::default()
        };
        assert_eq!(
            limit_of(segment.index(SliceSource::new(&data), &below)),
            LimitKind::BlockWork
        );
        fewest
    };
    assert_eq!(fewest(150) - fewest(50), 100 * 2);
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
