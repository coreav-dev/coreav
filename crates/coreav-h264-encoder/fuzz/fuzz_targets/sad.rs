#![no_main]
#![deny(unsafe_code)]

use coreav_h264_encoder::{Error, Result};
use libfuzzer_sys::fuzz_target;

#[path = "../../src/kernels.rs"]
mod kernels;
#[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
#[path = "../../src/simd.rs"]
mod simd;

fuzz_target!(|data: &[u8]| {
    let len = usize::from(data.first().copied().unwrap_or(0) % 20);
    let other_len = usize::from(data.get(1).copied().unwrap_or(0) % 20);
    let offset = usize::from(data.get(2).copied().unwrap_or(0) % 32);
    let a: Vec<_> = (0..offset + len)
        .map(|i| data.get(3 + i).copied().unwrap_or(0))
        .collect();
    let b: Vec<_> = (0..offset + other_len)
        .map(|i| data.get(3 + offset + len + i).copied().unwrap_or(255))
        .collect();
    let a = a.get(offset..).unwrap_or_default();
    let b = b.get(offset..).unwrap_or_default();
    let want = if len == other_len && len <= 16 {
        Ok(a.iter()
            .zip(b)
            .map(|(&a, &b)| u64::from(a.abs_diff(b)))
            .sum())
    } else {
        Err(Error::Internal)
    };
    assert_eq!(kernels::sad(a, b), want);
    if let Ok(want) = want {
        #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
        if let Some(simd) = simd::Avx2::detect() {
            assert_eq!(simd.sad(a, b), want);
        }
    }
});
