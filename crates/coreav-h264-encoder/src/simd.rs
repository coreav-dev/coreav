#![allow(unsafe_code)]

use std::arch::x86_64::*;

mod mc;

#[derive(Clone, Copy)]
pub(crate) struct Avx2(());

impl Avx2 {
    pub(crate) fn detect() -> Option<Self> {
        std::is_x86_feature_detected!("avx2").then_some(Self(()))
    }

    pub(crate) fn sad(self, a: &[u8], b: &[u8]) -> u64 {
        match (a.len(), b.len()) {
            (16, 16) => {
                if let (Some(a), Some(b)) = (a.first_chunk::<16>(), b.first_chunk::<16>()) {
                    return unsafe { sad16(a, b) };
                }
            }
            (8, 8) => {
                if let (Some(a), Some(b)) = (a.first_chunk::<8>(), b.first_chunk::<8>()) {
                    return unsafe { sad8(*a, *b) };
                }
            }
            (4, 4) => {
                if let (Some(a), Some(b)) = (a.first_chunk::<4>(), b.first_chunk::<4>()) {
                    return unsafe { sad4(*a, *b) };
                }
            }
            _ => {}
        }
        a.iter()
            .zip(b)
            .map(|(&a, &b)| u64::from(a.abs_diff(b)))
            .sum()
    }
}

#[target_feature(enable = "avx2")]
unsafe fn sad16(a: &[u8; 16], b: &[u8; 16]) -> u64 {
    let a = unsafe { _mm_loadu_si128(a.as_ptr().cast()) };
    let b = unsafe { _mm_loadu_si128(b.as_ptr().cast()) };
    let sum = _mm_sad_epu8(a, b);
    _mm_cvtsi128_si64(sum) as u64 + _mm_extract_epi64::<1>(sum) as u64
}

#[target_feature(enable = "avx2")]
unsafe fn sad8(a: [u8; 8], b: [u8; 8]) -> u64 {
    let a = _mm_cvtsi64_si128(i64::from_le_bytes(a));
    let b = _mm_cvtsi64_si128(i64::from_le_bytes(b));
    _mm_cvtsi128_si64(_mm_sad_epu8(a, b)) as u64
}

#[target_feature(enable = "avx2")]
unsafe fn sad4(a: [u8; 4], b: [u8; 4]) -> u64 {
    let a = _mm_cvtsi32_si128(i32::from_le_bytes(a));
    let b = _mm_cvtsi32_si128(i32::from_le_bytes(b));
    _mm_cvtsi128_si64(_mm_sad_epu8(a, b)) as u64
}
