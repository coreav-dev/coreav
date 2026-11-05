#![no_main]

use coreav_kernels_vp9::{
    BitDepth, InterpFilter, PredictionMut, ReferencePlane, Sampling, predict_inter,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((header, bytes)) = data.split_first_chunk::<16>() else {
        return;
    };
    let byte = |i: usize| header.get(i).copied().unwrap_or(0);
    let width = usize::from(byte(0) % 96 + 1);
    let height = usize::from(byte(1) % 96 + 1);
    let stride = width + usize::from(byte(2) & 7);
    let source: Vec<u8> = (0..(height - 1) * stride + width)
        .map(|i| {
            if bytes.is_empty() {
                0
            } else {
                bytes.get(i % bytes.len()).copied().unwrap_or(0)
            }
        })
        .collect();
    let scalar_source: Vec<u16> = source.iter().map(|&v| u16::from(v)).collect();
    let block_width = byte(3) % 64 + 1;
    let block_height = byte(4) % 64 + 1;
    let block_stride = usize::from(block_width) + usize::from(byte(5) & 7);
    let offset = usize::from(byte(14) & 7);
    let x = if byte(15) & 1 == 0 {
        (usize::from(byte(6)) % width * 16) as i32 + i32::from(byte(8) & 15)
    } else {
        i32::from(i16::from_le_bytes([byte(6), byte(8)]))
    };
    let y = if byte(15) & 1 == 0 {
        (usize::from(byte(7)) % height * 16) as i32 + i32::from(byte(9) & 15)
    } else {
        i32::from(i16::from_le_bytes([byte(7), byte(9)]))
    };
    let step_x = if byte(15) & 2 == 0 {
        16
    } else {
        byte(10) % 80 + 1
    };
    let step_y = if byte(15) & 2 == 0 {
        16
    } else {
        byte(11) % 80 + 1
    };
    let Some(sampling) = Sampling::new(x, y, step_x, step_y) else {
        return;
    };
    let filter = match byte(12) & 3 {
        0 => InterpFilter::EightTap,
        1 => InterpFilter::EightTapSmooth,
        2 => InterpFilter::EightTapSharp,
        _ => InterpFilter::Bilinear,
    };
    let compound = byte(15) & 4 != 0;
    let len = offset + (usize::from(block_height) - 1) * block_stride + usize::from(block_width);
    let mut automatic = vec![byte(13); len];
    let mut scalar = vec![u16::from(byte(13)); len];
    let Some(src) = ReferencePlane::new(&source, stride, width as u32, height as u32) else {
        return;
    };
    let Some(scalar_src) = ReferencePlane::new(&scalar_source, stride, width as u32, height as u32)
    else {
        return;
    };
    let Some(dst) = PredictionMut::new(
        &mut automatic,
        offset,
        block_stride,
        block_width,
        block_height,
    ) else {
        return;
    };
    let Some(scalar_dst) =
        PredictionMut::new(&mut scalar, offset, block_stride, block_width, block_height)
    else {
        return;
    };
    predict_inter(dst, src, sampling, filter, BitDepth::Eight, compound);
    predict_inter(
        scalar_dst,
        scalar_src,
        sampling,
        filter,
        BitDepth::Eight,
        compound,
    );
    assert!(
        automatic
            .iter()
            .zip(&scalar)
            .all(|(&a, &b)| u16::from(a) == b)
    );
});
