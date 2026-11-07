#![allow(unsafe_code)]

use std::arch::x86_64::*;

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

    pub(crate) fn satd(self, a: &[u8], b: &[u8], stride: usize, width: usize) -> Option<u64> {
        if !matches!(width, 4 | 8 | 16) || stride < width {
            return None;
        }
        let extent = (width - 1).checked_mul(stride)?.checked_add(width)?;
        let a = a.get(..extent)?;
        let b = b.get(..extent)?;
        Some(unsafe { satd_square(a, b, stride, width) })
    }
}

#[target_feature(enable = "avx2")]
unsafe fn satd_square(a: &[u8], b: &[u8], stride: usize, width: usize) -> u64 {
    let mut total = _mm256_setzero_si256();
    for y in (0..width).step_by(4) {
        let mut rows = [_mm256_setzero_si256(); 4];
        for (j, row) in rows.iter_mut().enumerate() {
            let offset = (y + j) * stride;
            let a = unsafe { load_row(a.as_ptr().add(offset), width) };
            let b = unsafe { load_row(b.as_ptr().add(offset), width) };
            let residual = _mm256_sub_epi16(a, b);
            let swapped = _mm256_shufflelo_epi16::<0xb1>(residual);
            let swapped = _mm256_shufflehi_epi16::<0xb1>(swapped);
            let pairs = _mm256_blend_epi16::<0xaa>(
                _mm256_add_epi16(residual, swapped),
                _mm256_sub_epi16(residual, swapped),
            );
            let swapped = _mm256_shuffle_epi32::<0xb1>(pairs);
            *row = _mm256_blend_epi16::<0xcc>(
                _mm256_add_epi16(pairs, swapped),
                _mm256_sub_epi16(pairs, swapped),
            );
        }
        let [a, b, c, d] = rows;
        let p = _mm256_add_epi16(a, b);
        let q = _mm256_add_epi16(c, d);
        let r = _mm256_sub_epi16(a, b);
        let s = _mm256_sub_epi16(c, d);
        for coefficients in [
            _mm256_add_epi16(p, q),
            _mm256_sub_epi16(p, q),
            _mm256_sub_epi16(r, s),
            _mm256_add_epi16(r, s),
        ] {
            total = _mm256_add_epi32(
                total,
                _mm256_madd_epi16(_mm256_abs_epi16(coefficients), _mm256_set1_epi16(1)),
            );
        }
    }
    let mut lanes = [0i32; 8];
    unsafe { _mm256_storeu_si256(lanes.as_mut_ptr().cast(), total) };
    lanes.into_iter().map(|v| v as u64).sum::<u64>() / 2
}

#[target_feature(enable = "avx2")]
unsafe fn load_row(ptr: *const u8, width: usize) -> __m256i {
    let bytes = match width {
        4 => _mm_cvtsi32_si128(unsafe { ptr.cast::<i32>().read_unaligned() }),
        8 => _mm_cvtsi64_si128(unsafe { ptr.cast::<i64>().read_unaligned() }),
        _ => unsafe { _mm_loadu_si128(ptr.cast()) },
    };
    _mm256_cvtepu8_epi16(bytes)
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
