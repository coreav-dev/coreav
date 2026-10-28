//! Bar C3 for the intra kernels: Intra_16x16, chroma, and the Intra_4x4
//! and Intra_8x8 macroblocks with their residuals, over every combination
//! of neighbours, every mode and some out of range, strides from the
//! narrowest a view allows, and coefficients over their whole range.
#![no_main]

use coreav_kernels_h264::{IntraAvail, IntraMut, Kernels, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

/// A coefficient from two bytes, inside -2^15 to 2^15 - 1.
fn coefficient(bytes: &[u8], i: usize) -> i32 {
    let low = bytes.get(2 * i).copied().unwrap_or(0);
    let high = bytes.get(2 * i + 1).copied().unwrap_or(0);
    i32::from(i16::from_le_bytes([low, high]))
}

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, rest)) = data.split_at_checked(5) else {
        return;
    };
    let kind = head[0] % 4;
    let bits = head[1];
    let avail = IntraAvail {
        left: bits & 1 != 0,
        top: bits & 2 != 0,
        top_right: bits & 4 != 0,
        top_left: bits & 8 != 0,
    };
    let n = if kind == 1 { 8 } else { 16 };
    let stride = n + usize::from(head[2] % 40);
    let x = 1 + usize::from(head[3]) % (stride - n + 1);
    let x = x.min(stride - n);
    let off = stride + x;
    // The plane ends at the block's last sample, or a little past it.
    let len = off + (n - 1) * stride + n + usize::from(head[4] % 4);
    // Coefficients and modes from the first bytes, samples from the rest.
    let samples = rest.get(16 * 32 + 16..).unwrap_or_default();
    let plane: Vec<u8> = (0..len)
        .map(|i| match samples.len() {
            0 => (i * 37) as u8,
            l => samples.get(i % l).copied().unwrap_or(0),
        })
        .collect();
    let mut want = plane.clone();
    let mut got = plane;
    let (Ok(a), Ok(b)) = (
        IntraMut::new(&mut want, off, stride, n, avail),
        IntraMut::new(&mut got, off, stride, n, avail),
    ) else {
        return;
    };
    let mode = bits >> 4;
    match kind {
        0 => {
            Scalar.intra16x16(a, mode);
            simd.intra16x16(b, mode);
        }
        1 => {
            Scalar.intra_chroma(a, mode);
            simd.intra_chroma(b, mode);
        }
        3 => {
            let coeffs: [[i32; 64]; 4] =
                core::array::from_fn(|k| core::array::from_fn(|i| coefficient(rest, k * 64 + i)));
            let at = |i: usize| rest.get(16 * 32 + i).copied().unwrap_or(0);
            let modes: [u8; 4] = core::array::from_fn(|k| at(k) % 10);
            let nz = (at(4) ^ mode) & 0xf;
            Scalar.intra8x8_mb(a, &modes, &coeffs, nz);
            simd.intra8x8_mb(b, &modes, &coeffs, nz);
        }
        _ => {
            let coeffs: [[i32; 16]; 16] =
                core::array::from_fn(|k| core::array::from_fn(|i| coefficient(rest, k * 16 + i)));
            let at = |i: usize| rest.get(16 * 32 + i).copied().unwrap_or(0);
            let modes: [u8; 16] = core::array::from_fn(|k| at(k) % 10);
            let nz = u16::from_le_bytes([at(0), at(1)]) ^ u16::from(mode) * 0x1111;
            Scalar.intra4x4_mb(a, &modes, &coeffs, nz);
            simd.intra4x4_mb(b, &modes, &coeffs, nz);
        }
    }
    assert_eq!(got, want);
});
