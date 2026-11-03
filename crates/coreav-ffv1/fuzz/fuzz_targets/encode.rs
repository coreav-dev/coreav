#![no_main]

use coreav_ffv1::{Encoder, Limits, Settings};
use coreav_frame::{Chroma, Picture, PixelFormat};
use libfuzzer_sys::fuzz_target;
use std::time::{Duration, Instant};

fuzz_target!(|data: &[u8]| {
    let [w, h, format, packet, allocation, aspect, samples @ ..] = data else {
        return;
    };
    let format = match format % 5 {
        0 => PixelFormat::YUV420P,
        1 => PixelFormat::Yuv {
            chroma: Chroma::Cs422,
            depth: 8,
        },
        2 => PixelFormat::Yuv {
            chroma: Chroma::Cs444,
            depth: 8,
        },
        3 => PixelFormat::Yuv {
            chroma: Chroma::Cs420,
            depth: 10,
        },
        _ => PixelFormat::Rgb24,
    };
    let (width, height) = (u32::from(w % 64) + 1, u32::from(h % 64) + 1);
    let settings = Settings {
        width,
        height,
        format,
        sample_aspect: Some((u32::from(*aspect), 1)),
    };
    let limits = Limits {
        max_width: 64,
        max_height: 64,
        max_pixels: 4096,
        max_packet_bytes: usize::from(*packet) * 256,
        max_alloc_bytes: usize::from(*allocation) * 1024,
    };
    let Ok(mut encoder) = Encoder::new(settings, limits) else {
        return;
    };
    let Ok(mut picture) = Picture::zeroed(width, height, format) else {
        return;
    };
    let mut source = samples.iter().copied().cycle();
    for plane in 0..format.plane_count() {
        let Some((rows, _)) = picture.plane_rows(plane) else {
            return;
        };
        for y in 0..rows {
            let Some(row) = picture.row_mut(plane, y) else {
                return;
            };
            for sample in row {
                *sample = source.next().unwrap_or(0);
            }
        }
    }
    let end = Instant::now() + Duration::from_secs(1);
    if let Ok(packet) = encoder.encode(&picture, end) {
        assert!(packet.len() <= limits.max_packet_bytes);
        match encoder.encode(&picture, end) {
            Ok(next) => assert_eq!(next, packet),
            Err(coreav_ffv1::Error::Timeout) => {}
            Err(error) => panic!("unchanged picture failed: {error}"),
        }
    }
});
