#![no_main]

use coreav_backend_hevc::NativeHevc;
use coreav_decode::{Backend, Codec, CodecConfig, ConfigKind, Limits, Packet, StreamConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }
    let Some((&mode, mut remaining)) = data.split_first() else {
        return;
    };
    let mut stream = StreamConfig::new(Codec::Hevc);
    if mode & 1 != 0 {
        let Some((&length, tail)) = remaining.split_first() else {
            return;
        };
        let length = usize::from(length).min(tail.len());
        let (config, tail) = tail.split_at(length);
        stream.config = Some(CodecConfig {
            kind: ConfigKind::Hvcc,
            data: config.to_vec(),
        });
        remaining = tail;
    }
    let limits = Limits {
        max_width: 256,
        max_height: 256,
        max_pixels: 65_536,
        max_frame_bytes: 196_608,
        max_decoded_bytes: 786_432,
        max_pending_bytes: 393_216,
        max_packet_bytes: 65_536,
        max_memory_bytes: 2 * 1024 * 1024,
        max_reference_frames: 4,
        timeout: std::time::Duration::from_millis(100),
    };
    let Ok(mut decoder) = NativeHevc::new().open(&stream, &limits) else {
        return;
    };
    let mut failed = false;
    let mut delivered = Vec::new();
    let mut origins = Vec::new();
    for _ in 0..32 {
        let Some((&operation, tail)) = remaining.split_first() else {
            break;
        };
        remaining = tail;
        let result = match operation & 3 {
            0 => {
                let Some(prefix) = remaining.first_chunk::<2>() else {
                    break;
                };
                let length = usize::from(u16::from_be_bytes(*prefix)).min(remaining.len() - 2);
                let Some(tail) = remaining.get(2..) else {
                    break;
                };
                let (packet, tail) = tail.split_at(length);
                remaining = tail;
                let pts = (operation & 16 != 0).then_some(i64::from(operation >> 5));
                if operation & 8 == 0 {
                    origins.push(pts);
                }
                decoder.send_packet(&Packet {
                    data: packet,
                    pts,
                    dts: None,
                    keyframe: operation & 4 != 0,
                    discard: operation & 8 != 0,
                })
            }
            1 => decoder
                .receive_frame()
                .map(|frame| delivered.extend(frame.map(|frame| frame.pts))),
            2 => decoder.send_eof(),
            _ => break,
        };
        if failed {
            assert!(result.is_err());
        }
        failed |= result.is_err();
    }
    if failed {
        assert!(decoder.receive_frame().is_err());
        assert!(decoder.send_eof().is_err());
    } else {
        let _ = decoder.send_eof();
        for _ in 0..16 {
            match decoder.receive_frame() {
                Ok(Some(frame)) => delivered.push(frame.pts),
                _ => break,
            }
        }
    }
    assert!(delivered.len() <= origins.len());
    assert!(delivered.iter().all(|pts| origins.contains(pts)));
});
