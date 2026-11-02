//! Packet-only output, read back through the public probe, packet and seek APIs.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_mkv::write::{CodecId, Media, Options, Packet, Track, plan};
use coreav_mkv::{Allowlist, DocType, Error, IndexSource, Limits, SliceSource, probe};

fn video(codec: CodecId) -> Track {
    Track {
        codec,
        private: if codec == CodecId::H264 {
            vec![1, 66, 0, 30, 255, 224, 0]
        } else {
            Vec::new()
        },
        media: Media::Video {
            width: 64,
            height: 48,
            display: Some((80, 48)),
        },
        default_duration_ns: Some(40_000_000),
        codec_delay_ns: 0,
        seek_pre_roll_ns: 0,
        language: "und".to_owned(),
        name: Some("Video".to_owned()),
        default: true,
    }
}

fn opus() -> Track {
    let mut private = b"OpusHead".to_vec();
    private.extend([1, 2]);
    private.extend(312u16.to_le_bytes());
    private.extend(48_000u32.to_le_bytes());
    private.extend([0, 0, 0]);
    Track {
        codec: CodecId::Opus,
        private,
        media: Media::Audio {
            sample_rate: 48_000,
            channels: 2,
            bit_depth: None,
        },
        default_duration_ns: None,
        codec_delay_ns: 6_500_000,
        seek_pre_roll_ns: 80_000_000,
        language: "eng".to_owned(),
        name: None,
        default: true,
    }
}

fn packet(track: usize, pts_ns: i64, keyframe: bool) -> Packet {
    Packet {
        track,
        pts_ns,
        duration_ns: Some(40_000_000),
        size: 3,
        keyframe,
        invisible: false,
        discard_padding_ns: None,
    }
}

fn output(tracks: &[Track], packets: &[Packet], options: Options) -> Vec<u8> {
    let layout = plan(tracks, packets, options, &Limits::default()).unwrap();
    let mut bytes = Vec::new();
    let size = layout
        .write(&mut bytes, |index, out| {
            out.write_all(&[index as u8, 42, 99])
                .map_err(|e| Error::Io(e.kind()))
        })
        .unwrap();
    assert_eq!(size, layout.bytes());
    assert_eq!(bytes.len() as u64, size);
    bytes
}

#[test]
fn presentation_times_and_payloads_keep_decode_order() {
    let packets = [
        packet(0, 0, true),
        packet(0, 120_000_000, false),
        packet(0, 40_000_000, false),
        packet(0, 80_000_000, false),
        packet(0, 160_000_000, true),
    ];
    let data = output(&[video(CodecId::H264)], &packets, Options::default());
    let limits = Limits::default();
    let segment = probe(SliceSource::new(&data), &limits, &Allowlist::any()).unwrap();
    assert!(segment.warnings.is_empty(), "{:?}", segment.warnings);
    assert_eq!(segment.doc_type_version, 4);
    assert_eq!(segment.timestamp_scale, 1_000_000);
    assert_eq!(segment.duration, Some(200.0));
    assert_eq!(segment.tracks[0].default_duration, Some(40_000_000));
    assert_eq!(
        segment.tracks[0].video.as_ref().unwrap().display_aspect(),
        Some((80, 48))
    );
    let listed: Vec<_> = segment
        .packets(SliceSource::new(&data), &limits)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        listed.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(),
        [0, 120, 40, 80, 160]
    );
    for (i, packet) in listed.iter().enumerate() {
        assert_eq!(
            &data[packet.offset as usize..(packet.offset + packet.stored_size) as usize],
            &[i as u8, 42, 99]
        );
        assert_eq!(packet.keyframe, i == 0 || i == 4);
    }
    let index = segment.index(SliceSource::new(&data), &limits).unwrap();
    assert_eq!(index.source, IndexSource::Cues);
    assert!(index.warnings.is_empty());
    assert_eq!(
        index.points.iter().map(|p| p.time).collect::<Vec<_>>(),
        [0, 160]
    );
    assert!(segment.cues.unwrap() < segment.first_cluster.unwrap());
    for point in index.points {
        let first = segment
            .packets_from(SliceSource::new(&data), &limits, point.cluster)
            .next()
            .unwrap()
            .unwrap();
        assert!(first.keyframe);
        assert_eq!(first.pts, Some(point.time as i64));
    }
}

#[test]
fn opus_delay_and_both_padding_signs_roundtrip() {
    let mut packets = [packet(0, -6_500_000, true), packet(0, 13_500_000, true)];
    for packet in &mut packets {
        packet.duration_ns = Some(20_000_000);
    }
    packets[0].discard_padding_ns = Some(-1_000_000);
    packets[1].discard_padding_ns = Some(5_000_000);
    let data = output(
        &[opus()],
        &packets,
        Options {
            doc_type: DocType::Webm,
            timestamp_scale_ns: 1_000,
        },
    );
    let limits = Limits::default();
    let segment = probe(SliceSource::new(&data), &limits, &Allowlist::any()).unwrap();
    assert_eq!(segment.doc_type, DocType::Webm);
    assert_eq!(segment.tracks[0].codec_delay, 6_500_000);
    assert_eq!(segment.tracks[0].seek_pre_roll, 80_000_000);
    let listed: Vec<_> = segment
        .packets(SliceSource::new(&data), &limits)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        listed.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(),
        [-6500, 13500]
    );
    assert_eq!(
        listed.iter().map(|p| p.block_timestamp).collect::<Vec<_>>(),
        [0, 20000]
    );
    assert_eq!(listed[0].discard_padding, Some(-1_000_000));
    assert_eq!(listed[1].discard_padding, Some(5_000_000));
    assert_eq!(listed[1].block_duration, Some(15000));
    assert!(listed.iter().all(|p| p.keyframe));
}

#[test]
fn variable_durations_do_not_turn_inter_frames_into_keyframes() {
    let mut track = video(CodecId::H264);
    track.default_duration_ns = None;
    let packets = [packet(0, 0, true), packet(0, 60_000_000, false)];
    let data = output(&[track], &packets, Options::default());
    let limits = Limits::default();
    let segment = probe(SliceSource::new(&data), &limits, &Allowlist::any()).unwrap();
    let listed: Vec<_> = segment
        .packets(SliceSource::new(&data), &limits)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(listed[0].block_duration, Some(40));
    assert_eq!(listed[1].block_duration, Some(40));
    assert!(!listed[1].keyframe);
    assert_eq!(segment.tracks[0].default_duration, None);
}

#[test]
fn cluster_boundaries_keep_large_and_negative_relative_times() {
    let mut track = video(CodecId::Vp9);
    track.default_duration_ns = Some(1000);
    let mut packets = [
        packet(0, -1000, true),
        packet(0, 40_000_000, false),
        packet(0, 1_000_000, false),
    ];
    for p in &mut packets {
        p.duration_ns = Some(1000);
    }
    let data = output(
        &[track],
        &packets,
        Options {
            timestamp_scale_ns: 1000,
            ..Options::default()
        },
    );
    let limits = Limits::default();
    let segment = probe(SliceSource::new(&data), &limits, &Allowlist::any()).unwrap();
    let listed: Vec<_> = segment
        .packets(SliceSource::new(&data), &limits)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        listed.iter().map(|p| p.pts.unwrap()).collect::<Vec<_>>(),
        [-1, 40000, 1000]
    );
}

#[test]
fn webm_refuses_wrong_codecs_and_nonmonotonic_times() {
    for codec in [CodecId::H264, CodecId::Aac] {
        let track = if codec == CodecId::Aac {
            Track { codec, ..opus() }
        } else {
            video(codec)
        };
        assert!(
            plan(
                &[track],
                &[packet(0, 0, true)],
                Options {
                    doc_type: DocType::Webm,
                    ..Options::default()
                },
                &Limits::default()
            )
            .is_err()
        );
    }
    assert!(
        plan(
            &[video(CodecId::Vp9)],
            &[packet(0, 40_000_000, true), packet(0, 0, false)],
            Options {
                doc_type: DocType::Webm,
                ..Options::default()
            },
            &Limits::default()
        )
        .is_err()
    );
}

#[test]
fn metadata_limits_and_payload_lengths_are_enforced() {
    let tracks = [video(CodecId::Vp9)];
    let packets = [packet(0, 0, true)];
    for limits in [
        Limits {
            max_tracks: 0,
            ..Limits::default()
        },
        Limits {
            max_block_work: 0,
            ..Limits::default()
        },
        Limits {
            max_index_bytes: 0,
            ..Limits::default()
        },
        Limits {
            max_packet_bytes: 2,
            ..Limits::default()
        },
        Limits {
            max_retained_bytes: 0,
            ..Limits::default()
        },
    ] {
        assert!(matches!(
            plan(&tracks, &packets, Options::default(), &limits),
            Err(Error::LimitExceeded { .. })
        ));
    }
    let layout = plan(&tracks, &packets, Options::default(), &Limits::default()).unwrap();
    assert!(layout.write(Vec::new(), |_, _| Ok(())).is_err());
    assert!(
        layout
            .write(Vec::new(), |_, out| out
                .write_all(&[0; 4])
                .map_err(|e| Error::Io(e.kind())))
            .is_err()
    );
    assert!(
        layout
            .write(Vec::new(), |_, _| Err(Error::Io(std::io::ErrorKind::Other)))
            .is_err()
    );
}

#[test]
fn unknown_end_and_missing_keyframes_do_not_invent_metadata() {
    let mut track = video(CodecId::Vp9);
    track.default_duration_ns = None;
    let mut p = packet(0, 0, false);
    p.duration_ns = None;
    let data = output(&[track], &[p], Options::default());
    let segment = probe(
        SliceSource::new(&data),
        &Limits::default(),
        &Allowlist::any(),
    )
    .unwrap();
    assert_eq!(segment.duration, None);
    assert_eq!(segment.cues, None);
}

#[test]
fn nonpositive_presentation_ends_omit_duration_without_losing_packets() {
    for (pts_ns, padding) in [(-1_000_000, None), (0, Some(1_000_000))] {
        let mut track = video(CodecId::Vp9);
        track.default_duration_ns = None;
        let mut p = packet(0, pts_ns, true);
        p.duration_ns = Some(1_000_000);
        p.discard_padding_ns = padding;
        let data = output(&[track], &[p], Options::default());
        assert!(!data.windows(3).any(|b| b == [0x44, 0x89, 0x88]));
        let limits = Limits::default();
        let segment = probe(SliceSource::new(&data), &limits, &Allowlist::any()).unwrap();
        assert_eq!(segment.duration, None);
        let packets: Vec<_> = segment
            .packets(SliceSource::new(&data), &limits)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].pts, Some(pts_ns / 1_000_000));
        assert_eq!(packets[0].discard_padding, padding);
    }
}

#[test]
fn seek_head_obeys_the_element_byte_limit() {
    let tracks = [video(CodecId::Vp9)];
    let packets = [packet(0, 0, true)];
    let limits = Limits {
        max_element_bytes: 60,
        ..Limits::default()
    };
    assert!(matches!(
        plan(&tracks, &packets, Options::default(), &limits),
        Err(Error::LimitExceeded {
            limit: coreav_mkv::LimitKind::ElementBytes,
            ..
        })
    ));
}

#[test]
fn multibyte_track_numbers_and_interleaved_tracks_roundtrip() {
    let tracks = vec![video(CodecId::Vp9); 128];
    let packets: Vec<_> = (0..128).rev().map(|track| packet(track, 0, true)).collect();
    let data = output(&tracks, &packets, Options::default());
    let limits = Limits::default();
    let segment = probe(SliceSource::new(&data), &limits, &Allowlist::any()).unwrap();
    assert_eq!(segment.tracks.len(), 128);
    let listed: Vec<_> = segment
        .packets(SliceSource::new(&data), &limits)
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        listed.iter().map(|p| p.track).collect::<Vec<_>>(),
        (0..128).rev().collect::<Vec<_>>()
    );
    let index = segment.index(SliceSource::new(&data), &limits).unwrap();
    assert_eq!(index.points.len(), 128);
    assert!(index.warnings.is_empty());
}
