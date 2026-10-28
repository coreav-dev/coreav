//! Timing of the thumbnail path (`cargo test --release -p coreav-frame
//! --test speed -- --ignored --nocapture`); not a gate.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Instant;

use coreav_frame::{ChromaUpsampling, ColorInfo, Filter, Picture, PixelFormat};

fn picture(w: u32, h: u32, format: PixelFormat) -> Picture {
    let mut p = Picture::zeroed(w, h, format).expect("picture");
    for plane in 0..format.plane_count() {
        let (rows, _) = p.plane_rows(plane).expect("rows");
        for y in 0..rows {
            for (x, b) in p.row_mut(plane, y).expect("row").iter_mut().enumerate() {
                *b = ((x * 7 + y * 3) % 251) as u8;
            }
        }
    }
    p
}

#[test]
#[ignore = "timing"]
fn resize_to_rgb_timing() {
    for (w, h) in [(1280, 720), (1920, 1080), (3840, 2160)] {
        let p = picture(w, h, PixelFormat::YUV420P);
        let mut best = f64::MAX;
        for _ in 0..10 {
            let t = Instant::now();
            let rgb = p
                .resize_to_rgb(
                    320,
                    180,
                    Filter::Bicubic,
                    &ColorInfo::default(),
                    ChromaUpsampling::FfmpegCompatible,
                )
                .expect("rgb");
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
            assert_eq!(rgb.width(), 320);
        }
        println!("{w}x{h} -> 320x180: {best:.2} ms");
    }
}
