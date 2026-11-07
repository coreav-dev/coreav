#![no_main]
#![deny(unsafe_code)]

use coreav_h264_encoder::{Error, Result};
use libfuzzer_sys::fuzz_target;

#[path = "../../src/kernels.rs"]
mod kernels;
#[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
#[path = "../../src/simd.rs"]
mod simd;
#[path = "../../src/transform.rs"]
mod transform;

fuzz_target!(|data: &[u8]| {
    let byte = |i| data.get(i).copied().unwrap_or(0);
    let width = match byte(0) % 4 {
        0 => 4,
        1 => 8,
        2 => 16,
        _ => usize::from(byte(1) % 20),
    };
    let stride = width + usize::from(byte(2) % 32);
    let offset = usize::from(byte(3) % 32);
    let extent = width.saturating_sub(1) * stride + width;
    let len = offset + extent;
    let a: Vec<_> = (0..len).map(|i| byte(6 + i)).collect();
    let b: Vec<_> = (0..len).map(|i| byte(6 + len + i)).collect();
    let a = a
        .get(..len.saturating_sub(usize::from(byte(4) % 2)))
        .unwrap_or_default();
    let b = b
        .get(..len.saturating_sub(usize::from(byte(5) % 2)))
        .unwrap_or_default();
    let want = if matches!(width, 4 | 8 | 16) && a.len() == len && b.len() == len {
        let mut sum = Ok(0u64);
        for y in (0..width).step_by(4) {
            for x in (0..width).step_by(4) {
                sum = sum.and_then(|s| {
                    transform::satd(a, b, offset + y * stride + x, stride).map(|v| s + v)
                });
            }
        }
        sum
    } else {
        Err(Error::Internal)
    };
    assert_eq!(kernels::satd(a, b, offset, stride, width), want);
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    if let Some(simd) = simd::Avx2::detect() {
        let direct = a
            .get(offset..)
            .zip(b.get(offset..))
            .and_then(|(a, b)| simd.satd(a, b, stride, width));
        assert_eq!(direct, want.ok());
    }
});
