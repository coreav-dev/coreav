//! Hostile input: every limit, size bombs, truncation, mutation, and a
//! pinned large virtual file.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::*;
use coreav_mkv::{
    Allowlist, BufferedSource, Error, LimitKind, Limits, MAX_LACED_FRAMES, Segment, SliceSource,
    Source, probe,
};
use std::time::Instant;

fn probe_with(data: &[u8], limits: &Limits) -> Result<Segment, Error> {
    probe(SliceSource::new(data), limits, &Allowlist::all())
}

/// Lists every packet and builds the index; returns the first error.
fn drain(segment: &Segment, data: &[u8], limits: &Limits) -> Result<u64, Error> {
    let mut count = 0;
    for packet in segment.packets(SliceSource::new(data), limits) {
        let packet = packet?;
        assert!(packet.offset + packet.stored_size <= data.len() as u64);
        count += 1;
    }
    segment.index(SliceSource::new(data), limits)?;
    Ok(count)
}

fn tracks() -> Vec<u8> {
    el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[]))
}

#[test]
fn huge_sizes_are_rejected_without_allocating() {
    // Info claims 2^50 bytes in a tiny file.
    let mut bomb = ebml_header("webm");
    bomb.extend(id_bytes(SEGMENT));
    bomb.extend([0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    bomb.extend(id_bytes(INFO));
    bomb.extend([0x01, 0x04, 0, 0, 0, 0, 0, 0]);
    let started = Instant::now();
    assert!(matches!(
        probe_with(&bomb, &Limits::default()),
        Err(Error::Truncated { .. })
    ));
    assert!(started.elapsed().as_millis() < 100);

    // A real 2 MiB Info is over max_element_bytes.
    let big = file("webm", &[el(INFO, &vec![0xEC; 2 << 20]), tracks()]);
    assert_eq!(
        probe_with(&big, &Limits::default()).unwrap_err(),
        Error::LimitExceeded {
            limit: LimitKind::ElementBytes,
            offset: segment_data_offset("webm", big.len() - ebml_header("webm").len() - 12) as u64,
        }
    );
}

#[test]
fn element_count_is_bounded() {
    // 100,000 empty Void elements inside Info.
    let voids = [0xEC, 0x80].repeat(100_000);
    let data = file("webm", &[el(INFO, &voids), tracks()]);
    let limits = Limits {
        max_elements: 1_000,
        ..Limits::default()
    };
    assert!(matches!(
        probe_with(&data, &limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::Elements,
            ..
        })
    ));
    assert!(probe_with(&data, &Limits::default()).is_ok());
}

#[test]
fn deep_nesting_is_skipped_by_size() {
    // A Tags element nested 50,000 levels deep (SimpleTag inside SimpleTag)
    // is skipped by its size; nesting costs no work.
    let mut nested = string(0x45A3, "TITLE");
    for _ in 0..50_000 {
        nested = el(0x67C8, &nested);
    }
    let data = file(
        "webm",
        &[info(None), tracks(), el(0x1254_C367, &el(0x7373, &nested))],
    );
    let started = Instant::now();
    let segment = probe_with(&data, &Limits::default()).unwrap();
    assert!(segment.has_tags);
    assert!(started.elapsed().as_millis() < 100);
}

#[test]
fn track_and_retained_byte_limits() {
    let entries: Vec<u8> = (1..=40)
        .flat_map(|n| audio_track(n, "A_OPUS", 48_000.0, 2, &el(0x63A2, &vec![0; 900 << 10])))
        .collect();
    let data = file("webm", &[info(None), el(TRACKS, &entries)]);
    assert!(matches!(
        probe_with(&data, &Limits::default()),
        Err(Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            ..
        })
    ));
    let limits = Limits {
        max_tracks: 10,
        ..Limits::default()
    };
    assert!(matches!(
        probe_with(&data, &limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::Tracks,
            ..
        })
    ));
}

#[test]
fn block_work_and_packet_size_are_bounded() {
    let blocks: Vec<Vec<u8>> = (0..1_000)
        .map(|i| simple_block(1, i, true, &[0; 10]))
        .collect();
    let data = file("webm", &[info(None), tracks(), cluster(0, &blocks)]);
    let segment = probe_with(&data, &Limits::default()).unwrap();
    assert_eq!(drain(&segment, &data, &Limits::default()), Ok(1_000));
    let limits = Limits {
        max_block_work: 100,
        ..Limits::default()
    };
    assert!(matches!(
        drain(&segment, &data, &limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::BlockWork,
            ..
        })
    ));
    let limits = Limits {
        max_packet_bytes: 9,
        ..Limits::default()
    };
    assert!(matches!(
        drain(&segment, &data, &limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::PacketBytes,
            ..
        })
    ));
}

#[test]
fn the_largest_lace_is_bounded_by_construction() {
    // 256 empty frames in one Xiph lace: the header is the count and 255
    // zero sizes.
    let mut block = vec![0x81, 0, 0, 0x82, 255];
    block.extend(vec![0u8; 255]);
    let data = file(
        "webm",
        &[
            info(None),
            tracks(),
            cluster(0, &[el(SIMPLE_BLOCK, &block)]),
        ],
    );
    let segment = probe_with(&data, &Limits::default()).unwrap();
    assert_eq!(
        drain(&segment, &data, &Limits::default()),
        Ok(MAX_LACED_FRAMES as u64)
    );
}

#[test]
fn structural_errors() {
    // Info with an unknown size.
    let data = cat(&[
        ebml_header("webm"),
        el(SEGMENT, &cat(&[el_unknown(INFO, &[]), tracks()])),
    ]);
    assert!(matches!(
        probe_with(&data, &Limits::default()),
        Err(Error::InvalidElement { .. })
    ));
    // A five-octet element ID and a reserved one.
    for id in [&[0x08u8, 1, 2, 3, 4, 0x80][..], &[0xFF, 0x80]] {
        let data = cat(&[
            ebml_header("webm"),
            el(SEGMENT, &cat(&[id.to_vec(), info(None), tracks()])),
        ]);
        assert!(matches!(
            probe_with(&data, &Limits::default()),
            Err(Error::InvalidElement { .. })
        ));
    }
    // A block before the Cluster Timestamp.
    let cluster = el(
        CLUSTER,
        &cat(&[simple_block(1, 0, true, &[1]), uint(0xE7, 0)]),
    );
    let data = file("webm", &[info(None), tracks(), cluster]);
    let segment = probe_with(&data, &Limits::default()).unwrap();
    assert!(matches!(
        drain(&segment, &data, &Limits::default()),
        Err(Error::InvalidElement { .. })
    ));
    // A lace whose sizes exceed the block.
    let data = file(
        "webm",
        &[
            info(None),
            tracks(),
            cluster_of(&[el(SIMPLE_BLOCK, &[0x81, 0, 0, 0x82, 1, 200, 1])]),
        ],
    );
    let segment = probe_with(&data, &Limits::default()).unwrap();
    assert!(matches!(
        drain(&segment, &data, &Limits::default()),
        Err(Error::InvalidElement { .. })
    ));
}

fn cluster_of(blocks: &[Vec<u8>]) -> Vec<u8> {
    cluster(0, blocks)
}

#[test]
fn a_second_ebml_document_ends_the_listing() {
    // An unknown-sized Segment, then a second EBML document: the first
    // Segment's packets, and the listing stops there.
    let first = live_file(
        "webm",
        &[
            info(None),
            tracks(),
            el_unknown(
                CLUSTER,
                &cat(&[uint(0xE7, 0), simple_block(1, 0, true, &[1])]),
            ),
        ],
    );
    let data = cat(&[first, small_webm()]);
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let limits = Limits {
        max_block_work: 50,
        ..Limits::default()
    };
    assert_eq!(drain(&segment, &data, &limits), Ok(1));
}

#[test]
fn header_stripping_counts_against_retained_bytes() {
    let strip = el(
        0x6D80,
        &el(
            0x6240,
            &el(
                0x5034,
                &cat(&[uint(0x4254, 3), el(0x4255, &vec![7; 900 << 10])]),
            ),
        ),
    );
    let entries: Vec<u8> = (1..=20)
        .flat_map(|n| audio_track(n, "A_OPUS", 48_000.0, 2, &strip))
        .collect();
    let data = file("webm", &[info(None), el(TRACKS, &entries)]);
    assert!(matches!(
        probe_with(&data, &Limits::default()),
        Err(Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            ..
        })
    ));
}

#[test]
fn every_truncation_is_an_error_or_a_valid_result() {
    let data = small_webm();
    for len in 0..data.len() {
        let cut = &data[..len];
        if let Ok(segment) = probe_with(cut, &Limits::default()) {
            let _ = drain(&segment, cut, &Limits::default());
        }
    }
}

#[test]
fn deterministic_mutations_never_panic() {
    let data = small_webm();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..20_000 {
        let mut copy = data.clone();
        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let at = (state as usize) % copy.len();
            copy[at] = (state >> 32) as u8;
        }
        if let Ok(segment) = probe_with(&copy, &Limits::default()) {
            let _ = drain(&segment, &copy, &Limits::default());
        }
    }
}

/// A live recording of `clusters` unknown-sized Clusters, each with one
/// 110,000-byte Opus frame, generated on demand.
struct VirtualLive {
    head: Vec<u8>,
    cluster_len: u64,
    clusters: u64,
}

const FRAME: u64 = 110_000;

impl VirtualLive {
    fn new(clusters: u64) -> Self {
        let head = cat(&[
            ebml_header("webm"),
            id_bytes(SEGMENT),
            vec![0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            info(None),
            tracks(),
        ]);
        let cluster_len = Self::cluster_header(0).len() as u64 + FRAME;
        Self {
            head,
            cluster_len,
            clusters,
        }
    }

    /// Everything of a Cluster but the frame bytes.
    fn cluster_header(index: u64) -> Vec<u8> {
        let mut out = id_bytes(CLUSTER);
        out.extend([0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        // Timestamp with a fixed 8-octet value.
        out.extend([0xE7, 0x88]);
        out.extend((index * 20).to_be_bytes());
        out.push(0xA3);
        out.extend(size_bytes(FRAME + 4)[..].iter().copied().chain([]));
        out.extend([0x81, 0, 0, 0x80]);
        out
    }
}

impl Source for VirtualLive {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        Ok(self.head.len() as u64 + self.cluster_len * self.clusters)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        if offset + buf.len() as u64 > self.len()? {
            return Err(coreav_isobmff::Error::Truncated { offset });
        }
        let head = self.head.len() as u64;
        let mut cache: Option<(u64, Vec<u8>)> = None;
        for (i, byte) in buf.iter_mut().enumerate() {
            let at = offset + i as u64;
            if at < head {
                *byte = self.head[at as usize];
                continue;
            }
            let index = (at - head) / self.cluster_len;
            let within = ((at - head) % self.cluster_len) as usize;
            if cache.as_ref().is_none_or(|(i, _)| *i != index) {
                cache = Some((index, Self::cluster_header(index)));
            }
            *byte = cache
                .as_ref()
                .unwrap()
                .1
                .get(within)
                .copied()
                .unwrap_or(0x55);
        }
        Ok(())
    }
}

/// Counts the bytes read from the inner source.
struct Counting<S> {
    inner: S,
    bytes: u64,
}

impl<S: Source> Source for Counting<S> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.inner.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.bytes += buf.len() as u64;
        self.inner.read_exact_at(offset, buf)
    }
}

#[test]
fn probe_reads_do_not_grow_with_clusters() {
    let mut read = Vec::new();
    for clusters in [3, 3_000] {
        let mut counting = Counting {
            inner: VirtualLive::new(clusters),
            bytes: 0,
        };
        let segment = probe(&mut counting, &Limits::default(), &Allowlist::default()).unwrap();
        assert!(segment.size.is_none());
        read.push(counting.bytes);
    }
    assert_eq!(read[0], read[1]);
    assert!(read[0] < 4096, "probe read {} bytes", read[0]);
}

#[test]
#[ignore = "lists 20,000 packets of a 2.2 GB virtual file; run with --ignored --release"]
fn a_large_live_recording_streams_with_bounded_reads() {
    let clusters = 20_000;
    let mut buffered = BufferedSource::new(VirtualLive::new(clusters));
    let total = buffered.len().unwrap();
    assert!(total > 2_200_000_000);
    let limits = Limits::default();
    let segment = probe(&mut buffered, &limits, &Allowlist::default()).unwrap();
    let mut count = 0u64;
    let mut last = -1;
    for packet in segment.packets(&mut buffered, &limits) {
        let packet = packet.unwrap();
        assert_eq!(packet.stored_size, FRAME);
        assert!(packet.block_timestamp > last);
        last = packet.block_timestamp;
        count += 1;
    }
    assert_eq!(count, clusters);
    assert!(buffered.largest_read() <= coreav_mkv::MAX_READ);
    let index = segment.index(&mut buffered, &limits).unwrap();
    assert_eq!(index.points.len() as u64, clusters);
}

/// The hostile suite's lacing cases (tools/hostile/generate.py,
/// part `matroska`), as SimpleBlocks of track 1 after the
/// track number, timestamp and flags: each lists every frame or fails with
/// a structured error, never past the block.
#[test]
fn lacing_edge_cases() {
    let xiph = |sizes: &[usize]| -> Vec<u8> {
        sizes
            .iter()
            .flat_map(|&s| {
                let mut run = vec![255u8; s / 255];
                run.push((s % 255) as u8);
                run
            })
            .collect()
    };
    let block = |flags: u8, body: &[u8]| cat(&[vec![0x81, 0, 0, flags], body.to_vec()]);
    let cases: Vec<(&str, Vec<u8>, Result<u64, ()>)> = vec![
        // Runs of 255 for a 70,000-byte frame, then 1 and 300 bytes.
        (
            "xiph long runs",
            block(
                0x82,
                &cat(&[vec![3], xiph(&[70_000, 1, 300]), vec![0u8; 70_301 + 20]]),
            ),
            Ok(4),
        ),
        (
            "xiph run past the block",
            block(0x82, &cat(&[vec![1], vec![255; 40]])),
            Err(()),
        ),
        (
            "xiph sizes past the block",
            block(0x82, &cat(&[vec![2], xiph(&[200, 200]), vec![0; 100]])),
            Err(()),
        ),
        (
            "ebml negative size",
            block(0x86, &cat(&[vec![2, 0x8a, 0x80], vec![0; 40]])),
            Err(()),
        ),
        (
            "ebml size of 2^44",
            block(
                0x86,
                &cat(&[vec![1, 0x01, 0, 0, 0x10, 0, 0, 0, 0], vec![0; 40]]),
            ),
            Err(()),
        ),
        (
            "ebml reserved vints",
            block(
                0x86,
                &cat(&[
                    vec![3, 0xff],
                    [0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff].repeat(2),
                    vec![0; 40],
                ]),
            ),
            Err(()),
        ),
        (
            "fixed not dividing",
            block(0x84, &cat(&[vec![2], vec![0; 10]])),
            Err(()),
        ),
        ("fixed 256 empty", block(0x84, &[255]), Ok(256)),
        ("lace count missing", block(0x82, &[]), Err(())),
    ];
    for (name, body, expected) in cases {
        let data = file(
            "webm",
            &[info(None), tracks(), cluster(0, &[el(SIMPLE_BLOCK, &body)])],
        );
        let segment = probe_with(&data, &Limits::default()).unwrap();
        let got = drain(&segment, &data, &Limits::default());
        match expected {
            Ok(frames) => assert_eq!(got, Ok(frames), "{name}"),
            Err(()) => assert!(
                matches!(got, Err(Error::InvalidElement { .. })),
                "{name}: {got:?}"
            ),
        }
    }
}

fn zlib_track(algo: u64, chain: bool) -> Vec<u8> {
    let encoding = |order: u64| {
        el(
            0x6240,
            &cat(&[
                uint(0x5031, order),
                uint(0x5032, 1),
                uint(0x5033, 0),
                el(0x5034, &uint(0x4254, algo)),
            ]),
        )
    };
    let encodings = if chain {
        cat(&[encoding(0), encoding(1)])
    } else {
        encoding(0)
    };
    el(
        TRACKS,
        &audio_track(1, "A_OPUS", 48_000.0, 2, &el(0x6D80, &encodings)),
    )
}

fn zlib_file(frames: &[Vec<u8>], algo: u64, chain: bool) -> Vec<u8> {
    let blocks: Vec<Vec<u8>> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| simple_block(1, i as i16, true, f))
        .collect();
    file(
        "webm",
        &[info(None), zlib_track(algo, chain), cluster(0, &blocks)],
    )
}

/// The sizes a listing that inflates gives each packet (`None` for a zlib
/// frame that did not inflate within the limits), or its first error.
fn inflated_sizes(data: &[u8], limits: &Limits) -> Result<Vec<Option<u64>>, Error> {
    let segment = probe_with(data, limits).unwrap();
    segment
        .packets(SliceSource::new(data), limits)
        .with_inflated_sizes()
        .map(|p| p.map(|p| p.inflated_size))
        .collect()
}

/// zlib-compressed frames: sized by inflating them only when
/// the listing asks, within `max_packet_bytes` per frame and
/// `max_inflated_bytes` per listing (inflated bytes plus 4 KiB per frame);
/// a frame that does not inflate within them has an unknown size and never
/// stops the listing; other compression leaves the track without packets.
#[test]
fn zlib_frames_are_sized_within_the_limits() {
    let deflate = |data: &[u8]| miniz_oxide::deflate::compress_to_vec_zlib(data, 9);
    let frames = vec![deflate(b"hello"), deflate(&[1u8; 70_000])];
    let data = zlib_file(&frames, 0, false);
    assert_eq!(
        inflated_sizes(&data, &Limits::default()),
        Ok(vec![Some(5), Some(70_000)])
    );
    // The default listing inflates nothing: stored ranges only.
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let plain: Vec<(Option<u64>, u64)> = segment
        .packets(SliceSource::new(&data), &Limits::default())
        .map(|p| p.map(|p| (p.inflated_size, p.size())).unwrap())
        .collect();
    assert_eq!(
        plain,
        [
            (None, frames[0].len() as u64),
            (None, frames[1].len() as u64)
        ]
    );

    // A frame that inflates to 64 MiB, against a 1 MiB packet limit, then a
    // good frame: the bomb's size is unknown, the next frame's is known.
    let bomb = deflate(&vec![0u8; 64 << 20]);
    let data = zlib_file(&[bomb, deflate(b"after")], 0, false);
    let limits = Limits {
        max_packet_bytes: 1 << 20,
        ..Limits::default()
    };
    let started = Instant::now();
    assert_eq!(inflated_sizes(&data, &limits), Ok(vec![None, Some(5)]));
    assert!(started.elapsed().as_secs() < 5);

    // Frames past the listing's budget have unknown sizes.
    let data = zlib_file(&vec![deflate(&vec![0u8; 1 << 20]); 40], 0, false);
    let limits = Limits {
        max_inflated_bytes: 16 << 20,
        ..Limits::default()
    };
    let sizes = inflated_sizes(&data, &limits).unwrap();
    let known = sizes.iter().filter(|s| s.is_some()).count();
    assert!((14..=15).contains(&known), "{known}");
    assert!(sizes[known..].iter().all(Option::is_none));

    // Broken, truncated or empty streams: unknown sizes, not errors.
    let broken = [
        b"not zlib".to_vec(),
        deflate(&[7u8; 1000])[..10].to_vec(),
        Vec::new(),
    ];
    let data = zlib_file(&broken, 0, false);
    assert_eq!(inflated_sizes(&data, &Limits::default()), Ok(vec![None; 3]));

    // bzlib, lzo and chains of encodings: no packets.
    for (algo, chain) in [(1, false), (2, false), (0, true)] {
        let data = zlib_file(&frames, algo, chain);
        let segment = probe_with(&data, &Limits::default()).unwrap();
        assert!(!segment.tracks[0].packets_available());
        assert_eq!(drain(&segment, &data, &Limits::default()), Ok(0));
    }
}

/// A flood of empty zlib frames (8 bytes each, inflating to nothing) is
/// bounded by the per-frame charge, and costs nothing to a listing that
/// does not ask for sizes.
#[test]
fn a_flood_of_empty_zlib_frames_is_bounded() {
    let empty = miniz_oxide::deflate::compress_to_vec_zlib(b"", 9);
    // Fixed-size lacing: 256 empty zlib frames per block, 2,000 blocks.
    let mut block = vec![0x81, 0, 0, 0x84, 255];
    for _ in 0..256 {
        block.extend(&empty);
    }
    let clusters: Vec<Vec<u8>> = (0..20)
        .map(|c| cluster(c, &vec![el(SIMPLE_BLOCK, &block); 100]))
        .collect();
    let data = file(
        "webm",
        &[&[info(None), zlib_track(0, false)][..], &clusters].concat(),
    );
    let limits = Limits::default();
    let started = Instant::now();
    let sizes = inflated_sizes(&data, &limits).unwrap();
    assert_eq!(sizes.len(), 512_000);
    // The budget charges 4 KiB a frame: 256 MiB covers 65,536 frames.
    assert_eq!(sizes.iter().filter(|s| s.is_some()).count(), 65_536);
    let segment = probe_with(&data, &limits).unwrap();
    assert_eq!(drain(&segment, &data, &limits), Ok(512_000));
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}

/// A zlib frame that does not inflate affects only its own track: the
/// other tracks are listed, and the scan marks only it as not listed.
#[test]
fn a_bad_zlib_frame_stays_in_its_track() {
    let encodings = el(
        0x6D80,
        &el(
            0x6240,
            &cat(&[
                uint(0x5032, 1),
                uint(0x5033, 0),
                el(0x5034, &uint(0x4254, 0)),
            ]),
        ),
    );
    let tracks = el(
        TRACKS,
        &cat(&[
            audio_track(1, "A_OPUS", 48_000.0, 2, &[]),
            audio_track(2, "A_OPUS", 48_000.0, 2, &encodings),
        ]),
    );
    let blocks = vec![
        simple_block(1, 0, true, &[0xFC, 1, 2]),
        simple_block(2, 0, true, b"not zlib"),
        simple_block(1, 20, true, &[0xFC, 3, 4]),
    ];
    let data = file("webm", &[info(None), tracks, cluster(0, &blocks)]);
    assert_eq!(
        inflated_sizes(&data, &Limits::default()),
        Ok(vec![None, None, None])
    );
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let scan = segment
        .scan_tracks(SliceSource::new(&data), &Limits::default())
        .unwrap();
    assert!(scan.tracks[0].listed);
    assert_eq!(scan.tracks[0].frames, 2);
    assert!(!scan.tracks[1].listed);

    // Two zlib tracks: broken frames in the first charge only what they
    // inflated, so the second keeps its sizes.
    let tracks = el(
        TRACKS,
        &cat(&[
            audio_track(1, "A_OPUS", 48_000.0, 2, &encodings),
            audio_track(2, "A_OPUS", 48_000.0, 2, &encodings),
        ]),
    );
    let good = miniz_oxide::deflate::compress_to_vec_zlib(b"hello", 9);
    let mut blocks: Vec<Vec<u8>> = (0..4)
        .map(|i| simple_block(1, i, true, b"not zlib"))
        .collect();
    blocks.extend((0..5).map(|i| simple_block(2, i, true, &good)));
    let data = file("webm", &[info(None), tracks, cluster(0, &blocks)]);
    let mut expected = vec![None; 4];
    expected.extend(vec![Some(5); 5]);
    assert_eq!(inflated_sizes(&data, &Limits::default()), Ok(expected));
    let segment = probe_with(&data, &Limits::default()).unwrap();
    let scan = segment
        .scan_tracks(SliceSource::new(&data), &Limits::default())
        .unwrap();
    assert!(!scan.tracks[0].listed);
    assert!(scan.tracks[1].listed);
}

/// Many zlib tracks of bombs: the listing's budget is shared by its tracks,
/// so the work does not grow with their number.
#[test]
fn many_tracks_of_bombs_share_one_budget() {
    let bomb = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; 16 << 20], 9);
    let encodings = el(
        0x6D80,
        &el(
            0x6240,
            &cat(&[
                uint(0x5032, 1),
                uint(0x5033, 0),
                el(0x5034, &uint(0x4254, 0)),
            ]),
        ),
    );
    let entries: Vec<Vec<u8>> = (1..=40)
        .map(|n| audio_track(n, "A_OPUS", 48_000.0, 2, &encodings))
        .collect();
    let blocks: Vec<Vec<u8>> = (1..=40u64)
        .flat_map(|n| {
            [
                simple_block(n, 0, true, &bomb),
                simple_block(n, 1, true, &bomb),
            ]
        })
        .collect();
    let data = file(
        "webm",
        &[info(None), el(TRACKS, &cat(&entries)), cluster(0, &blocks)],
    );
    let limits = Limits {
        max_inflated_bytes: 64 << 20,
        ..Limits::default()
    };
    let sizes = inflated_sizes(&data, &limits).unwrap();
    // 64 MiB covers three 16 MiB frames with their charges; the rest of
    // the 80 frames, of every track, are not inflated.
    assert_eq!(sizes.len(), 80);
    assert_eq!(sizes.iter().filter(|s| s.is_some()).count(), 3);
}
