//! Arbitrary RGB pictures of arbitrary small sizes through the built-in
//! JPEG, PNG and WebP encoders, at arbitrary JPEG qualities. Every call
//! gives image bytes or an error, never a panic.
#![no_main]

use coreav_frame::{Picture, PixelFormat, Plane};
use coreav_thumbnail::{BuiltinEncoder, ImageEncoder, ImageFormat};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let [w, h, quality, rest @ ..] = data else {
        return;
    };
    let (w, h) = (u32::from(*w % 80) + 1, u32::from(*h % 80) + 1);
    let len = (w * h * 3) as usize;
    let pixels: Vec<u8> = if rest.is_empty() { vec![0; len] } else { rest.iter().copied().cycle().take(len).collect() };
    let Ok(picture) = Picture::new(w, h, PixelFormat::Rgb24, vec![Plane::new(pixels, (w * 3) as usize)]) else {
        return;
    };
    for format in [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp] {
        let encoder = BuiltinEncoder { format, quality: *quality };
        if let Ok(bytes) = encoder.encode(&picture) {
            assert!(!bytes.is_empty());
        }
    }
});
