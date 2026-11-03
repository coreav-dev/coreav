//! Bounded packet, discard, drain and EOF transitions through the backend.
#![no_main]

use std::time::Duration;

use coreav_backend_vp9::{NativeOptions, NativeVp9};
use coreav_decode::{Codec, Limits, Packet, Policy, StreamConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&options, mut rest)) = data.split_first() else {
        return;
    };
    let limits = Limits {
        max_width: 256,
        max_height: 256,
        max_pixels: 256 * 256,
        max_frame_bytes: 256 * 256 * 2,
        max_decoded_bytes: 1 << 20,
        max_pending_bytes: 1 << 20,
        max_packet_bytes: 1024,
        max_memory_bytes: 32 << 20,
        timeout: Duration::from_millis(200),
        ..Limits::default()
    };
    let backend = NativeVp9::with_options(NativeOptions {
        keyframes_only: options & 1 != 0,
    });
    let Ok(opened) = coreav_decode::open(
        &StreamConfig::new(Codec::Vp9),
        &Policy::default(),
        &limits,
        &[&backend],
    ) else {
        return;
    };
    let mut decoder = opened.decoder;
    let mut failed = false;
    if options & 2 != 0 {
        failed = decoder
            .send_packet(&Packet {
                data: &[
                    0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00,
                    0x01, 0x00,
                ],
                pts: Some(-1),
                dts: None,
                keyframe: false,
                discard: false,
            })
            .is_err();
    }
    for pts in 0..64 {
        let Some((&operation, tail)) = rest.split_first() else {
            break;
        };
        rest = tail;
        match operation & 3 {
            0 | 1 => {
                let [low, high, tail @ ..] = rest else { break };
                let len = usize::from(u16::from_le_bytes([*low, *high])) % 4097;
                let Some((packet, tail)) = tail.split_at_checked(len) else {
                    break;
                };
                rest = tail;
                let result = decoder.send_packet(&Packet {
                    data: packet,
                    pts: Some(pts),
                    dts: None,
                    keyframe: operation & 8 != 0,
                    discard: operation & 4 != 0,
                });
                if failed {
                    assert!(result.is_err());
                }
                failed |= result.is_err();
            }
            2 => {
                let result = decoder.receive_frame();
                if failed {
                    assert!(result.is_err());
                }
                failed |= result.is_err();
            }
            _ => {
                let result = decoder.send_eof();
                if failed {
                    assert!(result.is_err());
                }
                failed |= result.is_err();
            }
        }
    }
    failed |= decoder.send_eof().is_err();
    for _ in 0..8 {
        let result = decoder.receive_frame();
        if failed {
            assert!(result.is_err());
        }
        failed |= result.is_err();
    }
});
