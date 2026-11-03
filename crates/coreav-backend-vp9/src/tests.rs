use super::*;
use coreav_decode::CodecConfig;

const KEY: [u8; 15] = [
    0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
];
const BYTES: u64 = 64 * 48 + 2 * 32 * 24;

fn packet(data: &[u8], pts: i64) -> Packet<'_> {
    Packet {
        data,
        pts: Some(pts),
        dts: Some(pts - 1),
        keyframe: false,
        discard: false,
    }
}

fn open(limits: Limits) -> Box<dyn Decoder> {
    NativeVp9::new()
        .open(&StreamConfig::new(Codec::Vp9), &limits)
        .unwrap()
}

#[test]
fn declared_dimensions_are_checked_even_when_only_one_is_known() {
    for width in [true, false] {
        let mut stream = StreamConfig::new(Codec::Vp9);
        if width {
            stream.width = Some(16_385);
        } else {
            stream.height = Some(16_385);
        }
        assert!(NativeVp9::new().open(&stream, &Limits::default()).is_err());
        assert!(
            NativeVp9::new()
                .open(
                    &stream,
                    &Limits {
                        max_width: 65_536,
                        max_height: 65_536,
                        ..Limits::default()
                    }
                )
                .is_ok()
        );
    }
}

#[test]
fn configuration_scope_is_checked_in_selection_and_direct_open() {
    let backend = NativeVp9::new();
    let mut stream = StreamConfig::new(Codec::Vp9);
    assert_eq!(backend.kind(), BackendKind::Native);
    assert_eq!(backend.supports(&stream, &stream.traits()), Support::Yes);
    for config in [
        CodecConfig {
            kind: ConfigKind::Avcc,
            data: Vec::new(),
        },
        CodecConfig {
            kind: ConfigKind::Vpcc,
            data: vec![1, 0, 0, 0, 0],
        },
        CodecConfig {
            kind: ConfigKind::MatroskaPrivate,
            data: vec![1, 2, 0],
        },
        CodecConfig {
            kind: ConfigKind::MatroskaPrivate,
            data: vec![1, 1, 0, 1, 1, 0],
        },
        CodecConfig {
            kind: ConfigKind::MatroskaPrivate,
            data: vec![3, 1, 10],
        },
        CodecConfig {
            kind: ConfigKind::MatroskaPrivate,
            data: vec![4, 1, 2],
        },
        CodecConfig {
            kind: ConfigKind::Vpcc,
            data: vec![1, 0, 0, 0, 1, 10, 0x80, 2, 2, 2, 0, 0],
        },
    ] {
        stream.config = Some(config);
        assert!(matches!(
            backend.supports(&stream, &stream.traits()),
            Support::No(_)
        ));
        assert!(backend.open(&stream, &Limits::default()).is_err());
    }
    stream.config = Some(CodecConfig {
        kind: ConfigKind::Vpcc,
        data: vec![1, 0, 0, 0, 0, 10, 0x80, 2, 2, 2, 0, 0],
    });
    assert_eq!(backend.supports(&stream, &stream.traits()), Support::Yes);
    assert!(backend.open(&stream, &Limits::default()).is_ok());
    stream.codec = Codec::Vp8;
    assert!(matches!(
        backend.supports(&stream, &stream.traits()),
        Support::No(_)
    ));
    assert!(backend.open(&stream, &Limits::default()).is_err());
}

#[test]
fn frames_keep_packet_timestamps_and_discarded_keys_still_refresh_references() {
    let mut decoder = open(Limits::default());
    let mut discarded = packet(&KEY, 9);
    discarded.discard = true;
    decoder.send_packet(&discarded).unwrap();
    assert!(decoder.receive_frame().unwrap().is_none());
    decoder.send_packet(&packet(&[0x88], -41)).unwrap();
    let frame = decoder.receive_frame().unwrap().unwrap();
    assert_eq!(frame.pts, Some(-41));
    assert_eq!(frame.color.matrix, Some(1));
    assert_eq!(frame.color.full_range, Some(false));
    let Pixels::Cpu(picture) = frame.pixels else {
        panic!()
    };
    assert_eq!(
        (picture.width(), picture.height(), picture.format()),
        (64, 48, PixelFormat::YUV420P)
    );
    assert_eq!(decoder.info().isolation, Isolation::InProcess);
    assert!(!decoder.info().fell_back());
}

#[test]
fn hidden_frames_and_every_shown_superframe_member_use_their_packet_clock() {
    let mut decoder = open(Limits::default());
    let mut hidden = KEY;
    hidden[0] &= !2;
    decoder.send_packet(&packet(&hidden, 3)).unwrap();
    assert!(decoder.receive_frame().unwrap().is_none());
    decoder
        .send_packet(&packet(&[0x88, 0x88, 0xc1, 1, 1, 0xc1], 17))
        .unwrap();
    for _ in 0..2 {
        assert_eq!(decoder.receive_frame().unwrap().unwrap().pts, Some(17));
    }
    assert!(decoder.receive_frame().unwrap().is_none());
}

#[test]
fn output_limits_are_cumulative_and_pending_frames_are_not_drainable_after_failure() {
    for cumulative in [false, true] {
        let mut limits = Limits::default();
        if cumulative {
            limits.max_decoded_bytes = BYTES;
        } else {
            limits.max_pending_bytes = BYTES;
        }
        let mut decoder = open(limits);
        decoder.send_packet(&packet(&KEY, 0)).unwrap();
        if cumulative {
            decoder.receive_frame().unwrap();
        }
        assert_eq!(
            decoder.send_packet(&packet(&[0x88], 1)).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert_eq!(
            decoder.receive_frame().unwrap_err().kind,
            ErrorKind::InvalidData
        );
        assert_eq!(decoder.send_eof().unwrap_err().kind, ErrorKind::InvalidData);
    }
}

#[test]
fn each_geometry_packet_and_memory_limit_refuses_before_success() {
    for limits in [
        Limits {
            max_width: 63,
            ..Limits::default()
        },
        Limits {
            max_height: 47,
            ..Limits::default()
        },
        Limits {
            max_pixels: 64 * 48 - 1,
            ..Limits::default()
        },
        Limits {
            max_frame_bytes: BYTES - 1,
            ..Limits::default()
        },
        Limits {
            max_packet_bytes: 14,
            ..Limits::default()
        },
        Limits {
            max_memory_bytes: 0,
            ..Limits::default()
        },
    ] {
        let mut decoder = open(limits);
        assert_eq!(
            decoder.send_packet(&packet(&KEY, 0)).unwrap_err().kind,
            ErrorKind::Limit
        );
        assert!(decoder.receive_frame().is_err());
    }
    let backend = NativeVp9::new();
    assert!(
        backend
            .open(
                &StreamConfig::new(Codec::Vp9),
                &Limits {
                    max_reference_frames: 7,
                    ..Limits::default()
                }
            )
            .is_err()
    );
    let mut huge = StreamConfig::new(Codec::Vp9);
    huge.width = Some(u32::MAX);
    huge.height = Some(u32::MAX);
    assert!(backend.open(&huge, &Limits::default()).is_err());
}

#[test]
fn terminal_errors_discard_partial_superframes_and_eof_is_idempotent() {
    let mut decoder = open(Limits::default());
    let mut bad = KEY.to_vec();
    bad.extend_from_slice(&[0x80, 0xc1, 15, 1, 0xc1]);
    assert_eq!(
        decoder.send_packet(&packet(&bad, 0)).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    assert!(decoder.receive_frame().is_err());
    let mut decoder = open(Limits::default());
    decoder.send_packet(&packet(&KEY, 0)).unwrap();
    decoder.send_eof().unwrap();
    decoder.send_eof().unwrap();
    assert!(decoder.receive_frame().unwrap().is_some());
    assert!(decoder.receive_frame().unwrap().is_none());
    assert!(decoder.send_packet(&packet(&KEY, 1)).is_err());
}

#[test]
fn zero_timeout_is_checked_on_each_boundary_operation() {
    for operation in 0..3 {
        let mut decoder = open(Limits {
            timeout: std::time::Duration::ZERO,
            ..Limits::default()
        });
        let error = match operation {
            0 => decoder.send_packet(&packet(&KEY, 0)).unwrap_err(),
            1 => decoder.send_eof().unwrap_err(),
            _ => decoder.receive_frame().unwrap_err(),
        };
        assert_eq!(error.kind, ErrorKind::Timeout);
    }
}

#[test]
fn packet_profile_refusal_does_not_export_old_references() {
    let mut decoder = open(Limits::default());
    decoder.send_packet(&packet(&KEY, 0)).unwrap();
    assert_eq!(
        decoder.send_packet(&packet(&[0x98], 1)).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    assert!(decoder.receive_frame().is_err());
}

#[test]
fn keyframe_only_backend_drops_refreshed_keys_and_keeps_other_slots() {
    let mut decoder = NativeVp9::with_options(NativeOptions {
        keyframes_only: true,
    })
    .open(&StreamConfig::new(Codec::Vp9), &Limits::default())
    .unwrap();
    decoder.send_packet(&packet(&KEY, 0)).unwrap();
    decoder.receive_frame().unwrap();
    decoder.send_packet(&packet(&[0x86, 0, 0x40], 1)).unwrap();
    assert!(decoder.receive_frame().unwrap().is_none());
    decoder.send_packet(&packet(&[0x88], 2)).unwrap();
    assert!(decoder.receive_frame().unwrap().is_none());
    decoder.send_packet(&packet(&[0x89], 3)).unwrap();
    assert_eq!(decoder.receive_frame().unwrap().unwrap().pts, Some(3));
}

#[test]
fn pending_frame_capacities_reduce_the_next_codec_memory_allowance() {
    let limits = Limits {
        max_memory_bytes: 40_000,
        ..Limits::default()
    };
    let mut held = open(limits.clone());
    held.send_packet(&packet(&KEY, 0)).unwrap();
    assert_eq!(
        held.send_packet(&packet(&KEY, 1)).unwrap_err().kind,
        ErrorKind::Limit
    );
    let mut drained = open(limits);
    drained.send_packet(&packet(&KEY, 0)).unwrap();
    drained.receive_frame().unwrap();
    assert!(drained.send_packet(&packet(&KEY, 1)).is_ok());
}
