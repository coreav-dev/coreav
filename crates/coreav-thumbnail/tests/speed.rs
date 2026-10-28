//! Timing of the image encoders (`cargo test --release -p coreav-thumbnail
//! --test speed -- --ignored --nocapture`); not a gate.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Instant;

use coreav_frame::{Picture, PixelFormat};
use coreav_thumbnail::{BuiltinEncoder, ImageEncoder, ImageFormat};

#[test]
#[ignore = "timing"]
fn encoder_timing() {
    let mut p = Picture::zeroed(320, 180, PixelFormat::Rgb24).expect("picture");
    for y in 0..180 {
        for (x, b) in p.row_mut(0, y).expect("row").iter_mut().enumerate() {
            *b = ((x * 7 + y * 3 + (x * y) % 13) % 251) as u8;
        }
    }
    let flat = Picture::zeroed(320, 180, PixelFormat::Rgb24).expect("flat");
    let jpeg = BuiltinEncoder {
        format: ImageFormat::Jpeg,
        quality: 91,
    };
    let mut best = f64::MAX;
    for _ in 0..20 {
        let t = Instant::now();
        let _ = jpeg.encode(&flat).expect("encode");
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    println!("jpeg flat: {best:.3} ms");
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp] {
        let encoder = BuiltinEncoder {
            format,
            quality: 91,
        };
        let mut best = f64::MAX;
        let mut size = 0;
        for _ in 0..20 {
            let t = Instant::now();
            size = encoder.encode(&p).expect("encode").len();
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
        }
        println!("{}: {best:.3} ms, {size} bytes", format.name());
    }
}
