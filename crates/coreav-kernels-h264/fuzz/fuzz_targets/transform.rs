//! Bar C3 for the inverse transforms, over coefficients in the range
//! clauses 8.5.10 to 8.5.13 give them.
#![no_main]

use coreav_kernels_h264::{BlockMut, Kernels, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

/// A coefficient from two bytes, inside -2^15 to 2^15 - 1.
fn coefficient(bytes: &[u8], i: usize) -> i32 {
    let low = bytes.get(2 * i).copied().unwrap_or(0);
    let high = bytes.get(2 * i + 1).copied().unwrap_or(0);
    i32::from(i16::from_le_bytes([low, high]))
}

/// A plane of `n` samples from the input past the 64 coefficients, the
/// bytes repeated; a fixed pattern where there are none. Samples that
/// differ from row to row and column to column catch a row or column
/// put back in the wrong place.
fn samples(rest: &[u8], n: usize) -> Vec<u8> {
    let tail = rest.get(128..).unwrap_or_default();
    (0..n)
        .map(|i| match tail.len() {
            0 => (i * 37) as u8,
            len => tail.get(i % len).copied().unwrap_or(0),
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, rest)) = data.split_at_checked(2) else {
        return;
    };
    let eight = head[0] % 2 == 0;
    let stride = if eight { 8 } else { 4 } + usize::from(head[1] % 8);
    let size = if eight { 8 } else { 4 };
    let plane = samples(rest, stride * size + 16);
    let mut want = plane.clone();
    let mut got = plane;
    if eight {
        let mut coeffs = [0i32; 64];
        for (i, c) in coeffs.iter_mut().enumerate() {
            *c = coefficient(rest, i);
        }
        let Ok(block) = BlockMut::new(&mut want, 0, stride, 8, 8) else {
            return;
        };
        Scalar.idct8x8_add(block, &coeffs);
        let Ok(block) = BlockMut::new(&mut got, 0, stride, 8, 8) else {
            return;
        };
        simd.idct8x8_add(block, &coeffs);
    } else {
        let mut coeffs = [0i32; 16];
        for (i, c) in coeffs.iter_mut().enumerate() {
            *c = coefficient(rest, i);
        }
        let Ok(block) = BlockMut::new(&mut want, 0, stride, 4, 4) else {
            return;
        };
        Scalar.idct4x4_add(block, &coeffs);
        let Ok(block) = BlockMut::new(&mut got, 0, stride, 4, 4) else {
            return;
        };
        simd.idct4x4_add(block, &coeffs);
        // The DC-only form takes the same path.
        let Ok(block) = BlockMut::new(&mut want, 0, stride, 4, 4) else {
            return;
        };
        Scalar.idct4x4_dc_add(block, coeffs[0]);
        let Ok(block) = BlockMut::new(&mut got, 0, stride, 4, 4) else {
            return;
        };
        simd.idct4x4_dc_add(block, coeffs[0]);
    }
    assert_eq!(got, want);
    // Four 4x4 blocks in one call, against the reference one block at a
    // time; the mask from the first byte.
    let stride = 8 + usize::from(head[1] % 8);
    let plane = samples(rest, stride * 8 + 16);
    let mut want = plane.clone();
    let mut got = plane;
    let mut coeffs = [[0i32; 16]; 4];
    for (i, c) in coeffs.as_flattened_mut().iter_mut().enumerate() {
        *c = coefficient(rest, i);
    }
    let nz = head[0] >> 4;
    for (k, block) in coeffs.iter().enumerate() {
        if nz & (1 << k) != 0 {
            let at = (k / 2) * 4 * stride + (k % 2) * 4;
            let Ok(view) = BlockMut::new(&mut want, at, stride, 4, 4) else {
                return;
            };
            Scalar.idct4x4_add(view, block);
        }
    }
    let Ok(view) = BlockMut::new(&mut got, 0, stride, 8, 8) else {
        return;
    };
    simd.idct4x4_add_quad(view, &coeffs, nz);
    assert_eq!(got, want);
});
