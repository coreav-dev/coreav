//! Probing, packets and the seek index on hand-built files (RFC 8794, RFC
//! 9559).
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::*;
use coreav_mkv::{
    Allowlist, Codec, DocType, Encoding, Error, IndexSource, Limits, Packet, Segment, SliceSource,
    TrackKind, Warning, probe, probe_with_metadata,
};

fn probe_bytes(data: &[u8]) -> Result<Segment, Error> {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
}

fn packets(segment: &Segment, data: &[u8]) -> Vec<Packet> {
    segment
        .packets(SliceSource::new(data), &Limits::default())
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn probes_a_small_webm_file() {
    let data = small_webm();
    let segment = probe_bytes(&data).unwrap();
    assert_eq!(segment.doc_type, DocType::Webm);
    assert_eq!(segment.timestamp_scale, 1_000_000);
    assert_eq!(segment.duration_ns(), Some(80_000_000));
    assert!(segment.cues.is_some());
    assert_eq!(segment.tracks.len(), 2);
    let video = &segment.tracks[0];
    assert_eq!(
        (video.kind, video.codec, video.number),
        (TrackKind::Video, Codec::Vp9, 1)
    );
    assert_eq!(video.video.unwrap().pixel_width, Some(320));
    assert_eq!(video.video.unwrap().rotation(), Some(0));
    // From the first key frame header: profile 0, BT.709, studio range.
    assert_eq!(video.frame_profile, Some(0));
    assert_eq!(video.color.matrix, Some(1));
    assert_eq!(video.color.full_range, Some(false));
    let audio = &segment.tracks[1];
    assert_eq!((audio.codec, audio.codec_delay), (Codec::Opus, 6_500_000));
    assert_eq!(audio.audio.unwrap().sample_rate(), Some(48_000));
    assert_eq!(audio.audio.unwrap().channels, 2);
}

/// A source whose `fail_at`-th read fails with an I/O error.
struct FailingAt<'a> {
    data: SliceSource<'a>,
    reads: usize,
    fail_at: usize,
}

impl coreav_mkv::Source for FailingAt<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.reads += 1;
        if self.reads == self.fail_at {
            return Err(coreav_isobmff::Error::Io(
                std::io::ErrorKind::ConnectionReset,
            ));
        }
        self.data.read_exact_at(offset, buf)
    }
}

/// A read that fails is not the end of the data: wherever the
/// failure falls, including the reads of the first key frame's header,
/// which are best effort for a damaged file, the probe fails with it.
#[test]
fn a_read_failure_fails_the_probe_wherever_it_falls() {
    let data = small_webm();
    let mut clean = FailingAt {
        data: SliceSource::new(&data),
        reads: 0,
        fail_at: 0,
    };
    probe(&mut clean, &Limits::default(), &Allowlist::default()).unwrap();
    assert!(clean.reads > 2, "{}", clean.reads);
    for fail_at in 1..=clean.reads {
        let mut source = FailingAt {
            data: SliceSource::new(&data),
            reads: 0,
            fail_at,
        };
        let result = probe(&mut source, &Limits::default(), &Allowlist::default());
        assert_eq!(
            result.err(),
            Some(Error::Io(std::io::ErrorKind::ConnectionReset)),
            "read {fail_at} of {}",
            clean.reads
        );
    }
}

#[test]
fn packets_carry_timestamps_after_the_codec_delay() {
    let data = small_webm();
    let segment = probe_bytes(&data).unwrap();
    let list = packets(&segment, &data);
    assert_eq!(list.len(), 4);
    assert_eq!(
        (list[0].track, list[0].pts, list[0].dts, list[0].keyframe),
        (0, Some(0), Some(0), true)
    );
    // 6.5 ms of codec delay rounds to 7 ticks.
    assert_eq!((list[1].track, list[1].pts), (1, Some(-7)));
    assert_eq!(list[1].pts_ns(&segment), Some(-6_500_000));
    assert_eq!(list[2].pts, Some(13));
    assert_eq!((list[3].pts, list[3].keyframe), (Some(40), false));
    for packet in &list {
        let start = packet.offset as usize;
        assert!(start + packet.size() as usize <= data.len());
    }
    assert_eq!(&data[list[0].offset as usize..][..8], &VP9_KEY_FRAME);
}

#[test]
fn the_index_uses_cues_or_a_cluster_scan() {
    let data = small_webm();
    let segment = probe_bytes(&data).unwrap();
    let index = segment
        .index(SliceSource::new(&data), &Limits::default())
        .unwrap();
    assert_eq!(index.source, IndexSource::Cues);
    let cluster = index.cluster_for(Some(1), 50).unwrap();
    assert_eq!(cluster, segment.first_cluster.unwrap());
    let from = segment
        .packets_from(SliceSource::new(&data), &Limits::default(), cluster)
        .count();
    assert_eq!(from, 4);

    // Without Cues: a live recording with unknown-sized Clusters.
    let tracks = el(TRACKS, &video_track(1, "V_VP8", 160, 120, &[]));
    let key = [0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a];
    let clusters: Vec<Vec<u8>> = (0..3)
        .map(|i| {
            el_unknown(
                CLUSTER,
                &cat(&[uint(0xE7, i * 1000), simple_block(1, 0, true, &key)]),
            )
        })
        .collect();
    let data = live_file("webm", &[vec![info(None), tracks], clusters].concat());
    let segment = probe_bytes(&data).unwrap();
    assert!(segment.size.is_none() && segment.duration.is_none() && segment.cues.is_none());
    assert_eq!(segment.tracks[0].frame_profile, Some(0));
    let index = segment
        .index(SliceSource::new(&data), &Limits::default())
        .unwrap();
    assert_eq!(index.source, IndexSource::ClusterScan);
    assert_eq!(
        index.points.iter().map(|p| p.time).collect::<Vec<_>>(),
        [0, 1000, 2000]
    );
    let second = index.cluster_for(None, 1500).unwrap();
    let list: Vec<Packet> = segment
        .packets_from(SliceSource::new(&data), &Limits::default(), second)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        list.iter().map(|p| p.pts).collect::<Vec<_>>(),
        [Some(1000), Some(2000)]
    );
    assert_eq!(packets(&segment, &data).len(), 3);
}

/// Every lacing of RFC 9559 10.3 gives the same frames.
#[test]
fn laced_frames_are_split() {
    let frames: [&[u8]; 3] = [&[1; 300], &[2; 20], &[3; 1000]];
    let xiph = [vec![2, 255, 45, 20], frames.concat()].concat();
    let mut ebml = vec![2, 0x41, 0x2C];
    // 20 - 300 = -280 as a two-octet signed VINT: -280 + 8191 + 0x4000.
    ebml.extend_from_slice(&(0x4000u16 + 8191 - 280).to_be_bytes());
    ebml.extend_from_slice(&frames.concat());
    let fixed = [vec![2], [[7u8; 50], [8; 50], [9; 50]].concat()].concat();
    for (flags, body, sizes) in [
        (0x82u8, xiph, vec![300, 20, 1000]),
        (0x86, ebml, vec![300, 20, 1000]),
        (0x84, fixed, vec![50, 50, 50]),
    ] {
        let mut block = vec![0x81, 0, 10, flags];
        block.extend(body);
        let tracks = el(
            TRACKS,
            &audio_track(1, "A_VORBIS", 48_000.0, 2, &uint(0x23_E383, 21_333_333)),
        );
        let data = file(
            "matroska",
            &[
                info(None),
                tracks,
                cluster(100, &[el(SIMPLE_BLOCK, &block)]),
            ],
        );
        let segment = probe_bytes(&data).unwrap();
        let list = packets(&segment, &data);
        assert_eq!(list.iter().map(|p| p.size()).collect::<Vec<_>>(), sizes);
        assert!(
            list.iter()
                .all(|p| p.frames_in_block == 3 && p.block_offset == list[0].block_offset)
        );
        // 110 ticks, then 21.33 ms and 42.67 ms later, truncated to ticks.
        assert_eq!(
            list.iter().map(|p| p.pts).collect::<Vec<_>>(),
            [Some(110), Some(131), Some(152)]
        );
        assert_eq!(list[2].pts_ns(&segment), Some(110_000_000 + 42_666_666));
        for (packet, byte) in list.iter().zip([1u8, 2, 3]) {
            if flags != 0x84 {
                assert_eq!(data[packet.offset as usize], byte);
            }
        }
    }
}

#[test]
fn lace_without_default_duration_has_unknown_later_timestamps() {
    let block = [vec![0x81, 0, 0, 0x84, 1], vec![5u8; 20]].concat();
    let tracks = el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[]));
    let data = file(
        "webm",
        &[info(None), tracks, cluster(0, &[el(SIMPLE_BLOCK, &block)])],
    );
    let segment = probe_bytes(&data).unwrap();
    let list = packets(&segment, &data);
    assert_eq!(
        list.iter().map(|p| p.pts).collect::<Vec<_>>(),
        [Some(0), None]
    );
    assert_eq!(list[1].dts, None);
}

#[test]
fn block_groups_reference_blocks_and_durations() {
    let block = |ts: i16| {
        let mut b = vec![0x81];
        b.extend(ts.to_be_bytes());
        b.push(0);
        b.extend([0u8, 0, 0, 1, 0x65]);
        b
    };
    let groups = vec![
        el(BLOCK_GROUP, &cat(&[el(0xA1, &block(0)), uint(0x9B, 40)])),
        el(BLOCK_GROUP, &cat(&[el(0xA1, &block(80)), sint(0xFB, -80)])),
        el(
            BLOCK_GROUP,
            &cat(&[el(0xA1, &block(40)), sint(0xFB, -40), sint(0xFB, 40)]),
        ),
    ];
    let tracks = el(TRACKS, &video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &[]));
    let data = file("matroska", &[info(None), tracks, cluster(0, &groups)]);
    let segment = probe_bytes(&data).unwrap();
    let list = packets(&segment, &data);
    assert_eq!(
        list.iter().map(|p| p.keyframe).collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(
        list.iter().map(|p| p.pts).collect::<Vec<_>>(),
        [Some(0), Some(80), Some(40)]
    );
    // H.264 can reorder frames: no decode times in Matroska.
    assert!(list.iter().all(|p| p.dts.is_none()));
    assert_eq!(list[0].block_duration, Some(40));
}

#[test]
fn header_stripping_is_reported_with_each_packet() {
    let encodings = el(
        0x6D80,
        &el(
            0x6240,
            &cat(&[
                uint(0x5032, 1),
                uint(0x5033, 0),
                el(0x5034, &cat(&[uint(0x4254, 3), el(0x4255, &[0, 0, 0, 1])])),
            ]),
        ),
    );
    let tracks = el(
        TRACKS,
        &video_track(1, "V_MPEG4/ISO/AVC", 64, 64, &encodings),
    );
    let data = file(
        "matroska",
        &[
            info(None),
            tracks,
            cluster(0, &[simple_block(1, 0, true, &[0x65, 0x88])]),
        ],
    );
    let segment = probe_bytes(&data).unwrap();
    assert_eq!(
        segment.tracks[0].encoding,
        Encoding::HeaderStripping(vec![0, 0, 0, 1])
    );
    let list = packets(&segment, &data);
    assert_eq!(
        (list[0].stored_size, list[0].stripped, list[0].size()),
        (2, 4, 6)
    );
}

#[test]
fn compressed_tracks_are_sized_or_have_no_packets_and_encrypted_ones_are_listed() {
    let encoding =
        |kind: u64, inner: Vec<u8>| el(0x6D80, &el(0x6240, &cat(&[uint(0x5033, kind), inner])));
    let zlib = encoding(0, el(0x5034, &uint(0x4254, 0)));
    let bzlib = encoding(0, el(0x5034, &uint(0x4254, 1)));
    let aes = encoding(1, el(0x5035, &uint(0x47E1, 5)));
    let tracks = el(
        TRACKS,
        &cat(&[
            audio_track(1, "A_OPUS", 48_000.0, 2, &bzlib),
            audio_track(2, "A_OPUS", 48_000.0, 2, &aes),
            audio_track(3, "A_OPUS", 48_000.0, 2, &zlib),
        ]),
    );
    let inflated = [0xFCu8; 40];
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&inflated, 6);
    let blocks = vec![
        simple_block(1, 0, true, &[1]),
        simple_block(2, 0, true, &[2]),
        simple_block(3, 0, true, &compressed),
    ];
    let data = file("webm", &[info(None), tracks, cluster(0, &blocks)]);
    let segment = probe_bytes(&data).unwrap();
    assert_eq!(segment.tracks[0].encoding, Encoding::Unsupported);
    assert!(
        segment.tracks[0]
            .warnings
            .contains(&Warning::UnsupportedEncoding)
    );
    assert_eq!(segment.tracks[1].encoding, Encoding::Encrypted);
    assert_eq!(segment.tracks[2].encoding, Encoding::Zlib);
    assert!(
        segment.tracks[2]
            .warnings
            .contains(&Warning::ZlibCompressed)
    );
    let list = packets(&segment, &data);
    assert_eq!(list.len(), 2);
    assert_eq!(
        (list[0].track, list[0].size(), list[0].inflated_size),
        (1, 1, None)
    );
    // The zlib frame: its stored range; the size a decoder sees when the
    // listing inflates.
    assert_eq!(list[1].track, 2);
    assert_eq!(list[1].stored_size, compressed.len() as u64);
    assert_eq!(list[1].inflated_size, None);
    let inflated: Vec<Packet> = segment
        .packets(SliceSource::new(&data), &Limits::default())
        .with_inflated_sizes()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        (inflated[1].size(), inflated[1].inflated_size),
        (40, Some(40))
    );
}

#[test]
fn metadata_after_the_clusters_is_found_through_the_seek_head() {
    let tracks = el(TRACKS, &video_track(1, "V_VP9", 64, 48, &[]));
    let clusters = cluster(0, &[simple_block(1, 0, true, &VP9_KEY_FRAME)]);
    // SeekHead (fixed size), Info, Cluster, then Tracks.
    let seek = |position: u64| {
        el(
            SEEK_HEAD,
            &el(
                0x4DBB,
                &cat(&[
                    el(0x53AB, &[0x16, 0x54, 0xAE, 0x6B]),
                    el(0x53AC, &position.to_be_bytes()),
                ]),
            ),
        )
    };
    let info = info(Some(40.0));
    let position = (seek(0x10000).len() + info.len() + clusters.len()) as u64;
    let data = file("webm", &[seek(position), info, clusters, tracks]);
    let segment = probe_bytes(&data).unwrap();
    assert_eq!(segment.tracks.len(), 1);
    assert_eq!(segment.tracks[0].video.unwrap().pixel_height, Some(48));
    assert_eq!(packets(&segment, &data).len(), 1);
}

#[test]
fn colour_projection_and_codec_private() {
    let colour = el(
        0x55B0,
        &cat(&[
            uint(0x55B1, 9),
            uint(0x55BA, 16),
            uint(0x55BB, 9),
            uint(0x55B9, 1),
        ]),
    );
    let projection = el(0x7670, &float(0x7675, -90.0));
    let video = el(
        0xE0,
        &cat(&[uint(0xB0, 640), uint(0xBA, 480), colour, projection]),
    );
    let entry = el(
        TRACK_ENTRY,
        &cat(&[uint(0xD7, 1), uint(0x83, 1), string(0x86, "V_AV1"), video]),
    );
    let data = file("matroska", &[info(None), el(TRACKS, &entry)]);
    let segment = probe_bytes(&data).unwrap();
    let track = &segment.tracks[0];
    assert_eq!(
        (
            track.color.matrix,
            track.color.transfer,
            track.color.primaries,
            track.color.full_range
        ),
        (Some(9), Some(16), Some(9), Some(false))
    );
    // -90 degrees counter-clockwise is 90 degrees clockwise.
    assert_eq!(track.video.unwrap().rotation(), Some(90));
    assert!(segment.first_cluster.is_none());
}

#[test]
fn the_allowlist_drops_or_rejects() {
    let tracks = el(
        TRACKS,
        &cat(&[
            video_track(1, "V_VP9", 64, 64, &[]),
            audio_track(2, "A_AC3", 48_000.0, 6, &[]),
            audio_track(3, "A_REAL/COOK", 44_100.0, 2, &[]),
        ]),
    );
    let blocks = vec![
        simple_block(2, 0, true, &[0x0b, 0x77]),
        simple_block(1, 0, true, &VP9_KEY_FRAME),
    ];
    let data = file("matroska", &[info(None), tracks, cluster(0, &blocks)]);
    let segment = probe_bytes(&data).unwrap();
    assert_eq!(segment.tracks.len(), 1);
    assert_eq!(
        segment
            .dropped
            .iter()
            .map(|d| (d.stream_index, d.codec))
            .collect::<Vec<_>>(),
        [(1, Codec::Ac3), (2, Codec::Other)]
    );
    // Blocks of dropped tracks never come out.
    assert_eq!(packets(&segment, &data).len(), 1);
    let all = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    assert_eq!(all.tracks.len(), 2);
    let strict = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::default().strict(),
    );
    assert_eq!(
        strict.unwrap_err(),
        Error::CodecNotAllowed {
            track_number: 2,
            codec: Codec::Ac3
        }
    );
}

#[test]
fn malformed_headers_are_errors() {
    assert_eq!(
        probe_bytes(b"\x00\x00\x00\x18ftypisom").unwrap_err(),
        Error::NotMatroska
    );
    assert_eq!(
        probe_bytes(&ebml_header("mka2")).unwrap_err(),
        Error::NotMatroska
    );
    let future = el(EBML, &cat(&[string(0x4282, "matroska"), uint(0x4285, 5)]));
    assert!(matches!(
        probe_bytes(&future),
        Err(Error::Unsupported { .. })
    ));
    let reader = el(EBML, &cat(&[uint(0x42F7, 2), string(0x4282, "matroska")]));
    assert!(matches!(
        probe_bytes(&reader),
        Err(Error::Unsupported { .. })
    ));
    // No Tracks.
    assert!(matches!(
        probe_bytes(&file("webm", &[info(None)])),
        Err(Error::InvalidElement { .. })
    ));
    // Duplicate track numbers.
    let tracks = el(
        TRACKS,
        &cat(&[
            video_track(1, "V_VP9", 8, 8, &[]),
            video_track(1, "V_VP8", 8, 8, &[]),
        ]),
    );
    assert!(matches!(
        probe_bytes(&file("webm", &[info(None), tracks])),
        Err(Error::InvalidElement {
            reason: "duplicate TrackNumber",
            ..
        })
    ));
}

#[test]
fn a_truncated_file_yields_its_complete_frames_then_an_error() {
    let tracks = el(TRACKS, &audio_track(1, "A_OPUS", 48_000.0, 2, &[]));
    let blocks: Vec<Vec<u8>> = (0..10)
        .map(|i| simple_block(1, i * 20, true, &[i as u8; 100]))
        .collect();
    let data = file("webm", &[info(None), tracks, cluster(0, &blocks)]);
    let cut = &data[..data.len() - 50];
    let segment = probe_bytes(cut).unwrap();
    assert!(segment.warnings.contains(&Warning::TruncatedSegment));
    let results: Vec<_> = segment
        .packets(SliceSource::new(cut), &Limits::default())
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 9);
    assert!(matches!(results.last(), Some(Err(Error::Truncated { .. }))));
}

#[test]
fn tags_chapters_attachments_and_track_flags_are_read() {
    let track = video_track(
        1,
        "V_VP9",
        64,
        48,
        &cat(&[uint(0x55AB, 1), uint(0x55AF, 1)]),
    );
    let simple =
        |name: &str, value: &str| el(0x67C8, &cat(&[string(0x45A3, name), string(0x4487, value)]));
    let tags = el(
        0x1254_C367,
        &cat(&[
            el(0x7373, &cat(&[el(0x63C0, &[]), simple("ENCODER", "x")])),
            el(
                0x7373,
                &cat(&[
                    el(0x63C0, &uint(0x63C5, 1000)),
                    el(
                        0x67C8,
                        &cat(&[
                            string(0x45A3, "BPS"),
                            string(0x447A, "eng"),
                            string(0x4487, "100"),
                            simple("CHILD", "c"),
                        ]),
                    ),
                ]),
            ),
        ]),
    );
    let chapters = el(
        0x1043_A770,
        &el(
            0x45B9,
            &cat(&[
                uint(0x45BC, 7),
                el(
                    0xB6,
                    &cat(&[
                        uint(0x73C4, 11),
                        uint(0x91, 0),
                        uint(0x92, 1_000_000_000),
                        el(0x80, &string(0x85, "One")),
                    ]),
                ),
            ]),
        ),
    );
    let attachments = el(
        0x1941_A469,
        &el(
            0x61A7,
            &cat(&[
                string(0x466E, "a.ttf"),
                string(0x4660, "application/x-truetype-font"),
                string(0x467E, "font"),
                el(0x465C, b"abcd"),
                uint(0x46AE, 42),
            ]),
        ),
    );
    let data = file(
        "matroska",
        &[
            info(Some(40.0)),
            el(TRACKS, &track),
            tags,
            chapters,
            attachments,
            cluster(0, &[simple_block(1, 0, true, &VP9_KEY_FRAME)]),
        ],
    );
    // The plain probe only notes the metadata elements.
    let plain = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    assert!(plain.has_tags && plain.has_chapters);
    assert!(plain.tags.is_empty() && plain.editions.is_empty());
    assert_eq!(
        (
            plain.attachments[0].uid,
            plain.attachments[0].description.as_ref()
        ),
        (None, None)
    );
    let segment = probe_with_metadata(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::all(),
    )
    .unwrap();
    let track = &segment.tracks[0];
    assert!(track.hearing_impaired && track.commentary && !track.visual_impaired);
    assert_eq!(track.video.unwrap().field_order, 2);
    assert_eq!(segment.tags.len(), 2);
    assert!(segment.tags[0].global());
    let bps = &segment.tags[1].simple_tags[0];
    assert_eq!(segment.tags[1].track_uids, vec![1000]);
    assert_eq!(
        (bps.name.as_str(), bps.language.as_deref()),
        ("BPS", Some("eng"))
    );
    assert_eq!(bps.children[0].string.as_deref(), Some("c"));
    let chapter = &segment.editions[0].chapters[0];
    assert_eq!(
        (chapter.uid, chapter.start, chapter.end),
        (Some(11), Some(0), Some(1_000_000_000))
    );
    let attachment = &segment.attachments[0];
    assert_eq!((attachment.uid, attachment.size), (Some(42), 4));
    assert_eq!(attachment.description.as_deref(), Some("font"));
    assert_eq!(&data[attachment.data_offset as usize..][..4], b"abcd");
}
