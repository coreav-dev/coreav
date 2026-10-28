//! Arbitrary pictures (format, size, strides and plane bytes from the
//! input) through validation and every frame operation, with arbitrary
//! arguments: crop, rotate, resize with both chroma sitings, conversion to
//! RGB in both chroma modes, and the semi-planar conversions. Every call
//! returns a picture or an error, never panics or reads outside a plane.
#![no_main]

use coreav_frame::{
    Chroma, ChromaSiting, ChromaUpsampling, ColorInfo, Filter, Picture, PixelFormat, Plane, Rect,
};
use libfuzzer_sys::fuzz_target;

fn format(selector: u8, depth: u8) -> PixelFormat {
    let depth = 8 + depth % 9;
    match selector % 8 {
        0 => PixelFormat::Yuv { chroma: Chroma::Cs420, depth },
        1 => PixelFormat::Yuv { chroma: Chroma::Cs422, depth },
        2 => PixelFormat::Yuv { chroma: Chroma::Cs444, depth },
        3 => PixelFormat::Yuv { chroma: Chroma::Mono, depth },
        4 => PixelFormat::SemiPlanar { depth },
        5 => PixelFormat::Gbr { depth },
        6 => PixelFormat::Rgb24,
        _ => PixelFormat::Rgba,
    }
}

fuzz_target!(|data: &[u8]| {
    let [sel, depth, w, h, tw, th, pad, op, cx, cy, matrix, rest @ ..] = data else {
        return;
    };
    let format = format(*sel, *depth);
    let (w, h) = (u32::from(*w % 48), u32::from(*h % 48));
    let mut bytes = rest.iter().copied().cycle();
    let mut planes = Vec::new();
    for plane in 0..format.plane_count() {
        let Some(row) = format.row_bytes(plane, w.max(1), h.max(1)) else {
            return;
        };
        let (_, rows) = format.plane_size(plane, w.max(1), h.max(1)).unwrap_or((0, 0));
        // The stride is the row plus padding, or shorter when `pad` says so.
        let stride = if *pad == 0xFF { row.saturating_sub(1) } else { row + usize::from(*pad % 8) };
        let len = stride * rows as usize;
        let len = if *pad == 0xFE { len.saturating_sub(1) } else { len };
        let data: Vec<u8> = if rest.is_empty() { vec![0; len] } else { (&mut bytes).take(len).collect() };
        planes.push(Plane::new(data, stride));
    }
    let Ok(picture) = Picture::new(w, h, format, planes) else {
        return;
    };
    let (tw, th) = (u32::from(*tw % 40) + 1, u32::from(*th % 40) + 1);
    let color = ColorInfo {
        matrix: Some(*matrix % 16),
        full_range: Some(*matrix & 0x80 != 0),
        ..ColorInfo::default()
    };
    let filter = [Filter::Bilinear, Filter::Bicubic, Filter::Area][usize::from(*op % 3)];
    let _ = picture.crop(Rect {
        x: u32::from(*cx % 48),
        y: u32::from(*cy % 48),
        width: tw,
        height: th,
    });
    let _ = picture.rotate(*op >> 2);
    let _ = picture.to_planar();
    for siting in [ChromaSiting::Location(None), ChromaSiting::FfmpegCompatible] {
        if let Ok(resized) = picture.resize_with(tw, th, filter, siting) {
            assert_eq!((resized.width(), resized.height()), (tw, th));
            assert_eq!(resized.format(), picture.format());
        }
    }
    for chroma in [ChromaUpsampling::Full, ChromaUpsampling::FfmpegCompatible] {
        if let Ok(rgb) = picture.resize_to_rgb(tw, th, filter, &color, chroma) {
            assert_eq!(rgb.format(), PixelFormat::Rgb24);
        }
    }
    let _ = picture.to_rgb(&color);
});
