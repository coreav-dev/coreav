#![allow(unsafe_code)]

use super::Avx2;
use crate::transform::{Forward, Quant};
use std::arch::x86_64::*;

impl Avx2 {
    pub(crate) fn forward(
        self,
        source: &[u8],
        predicted: &[u8],
        at: usize,
        stride: usize,
    ) -> Option<[i32; 16]> {
        let extent = stride.checked_mul(3)?.checked_add(4)?;
        let source = source.get(at..)?.get(..extent)?;
        let predicted = predicted.get(at..)?.get(..extent)?;
        Some(unsafe { forward4(source, predicted, stride) })
    }

    pub(crate) fn zero(self, coefficients: &Forward, quant: &Quant, separate_dc: bool) -> bool {
        let (zero, _, _, _) = quant.parameters();
        unsafe { zero4(coefficients.coefficients(), zero, separate_dc) }
    }

    pub(crate) fn levels(
        self,
        coefficients: &Forward,
        quant: &Quant,
        separate_dc: bool,
    ) -> [i32; 16] {
        let (_, bias, reciprocal, _) = quant.parameters();
        let mut out = unsafe { levels4(coefficients.coefficients(), bias, reciprocal) };
        if separate_dc && let Some(dc) = out.first_mut() {
            *dc = 0;
        }
        out
    }

    pub(crate) fn scaled(self, levels: &[i32; 16], scale: &[i32; 16]) -> [i32; 16] {
        unsafe { scaled4(levels, scale) }
    }

    pub(crate) fn inverse(self, coefficients: &[i32; 16]) -> Option<[i32; 16]> {
        let [dc, rest @ ..] = coefficients;
        if rest.iter().all(|&c| c == 0) {
            return (-32768..=32735).contains(dc).then_some([*dc; 16]);
        }
        unsafe { inverse4(coefficients) }
    }
}

#[target_feature(enable = "avx2")]
fn combine(a: __m256i, b: __m256i, c: __m256i, d: __m256i) -> __m256i {
    _mm256_blend_epi32::<0xcc>(
        _mm256_blend_epi32::<0xaa>(a, b),
        _mm256_blend_epi32::<0xaa>(c, d),
    )
}

#[target_feature(enable = "avx2")]
fn transpose(a: __m256i, b: __m256i) -> (__m256i, __m256i) {
    let even = _mm256_permute2x128_si256::<0x20>(a, b);
    let odd = _mm256_permute2x128_si256::<0x31>(a, b);
    let low = _mm256_unpacklo_epi32(even, odd);
    let high = _mm256_unpackhi_epi32(even, odd);
    let upper = _mm256_permute2x128_si256::<0x20>(low, high);
    let lower = _mm256_permute2x128_si256::<0x31>(low, high);
    let even = _mm256_unpacklo_epi64(upper, lower);
    let odd = _mm256_unpackhi_epi64(upper, lower);
    (
        _mm256_permute2x128_si256::<0x20>(even, odd),
        _mm256_permute2x128_si256::<0x31>(even, odd),
    )
}

#[target_feature(enable = "avx2")]
fn forward_line(input: __m256i) -> __m256i {
    let a = _mm256_shuffle_epi32::<0x00>(input);
    let b = _mm256_shuffle_epi32::<0x55>(input);
    let c = _mm256_shuffle_epi32::<0xaa>(input);
    let d = _mm256_shuffle_epi32::<0xff>(input);
    let p = _mm256_add_epi32(a, d);
    let q = _mm256_add_epi32(b, c);
    let r = _mm256_sub_epi32(a, d);
    let s = _mm256_sub_epi32(b, c);
    combine(
        _mm256_add_epi32(p, q),
        _mm256_add_epi32(_mm256_add_epi32(r, r), s),
        _mm256_sub_epi32(p, q),
        _mm256_sub_epi32(r, _mm256_add_epi32(s, s)),
    )
}

#[target_feature(enable = "avx2")]
unsafe fn forward4(source: &[u8], predicted: &[u8], stride: usize) -> [i32; 16] {
    let mut rows = [_mm_setzero_si128(); 4];
    for (j, row) in rows.iter_mut().enumerate() {
        let a = unsafe {
            source
                .as_ptr()
                .add(j * stride)
                .cast::<i32>()
                .read_unaligned()
        };
        let b = unsafe {
            predicted
                .as_ptr()
                .add(j * stride)
                .cast::<i32>()
                .read_unaligned()
        };
        *row = _mm_sub_epi32(
            _mm_cvtepu8_epi32(_mm_cvtsi32_si128(a)),
            _mm_cvtepu8_epi32(_mm_cvtsi32_si128(b)),
        );
    }
    let [a, b, c, d] = rows;
    let a = forward_line(_mm256_set_m128i(b, a));
    let b = forward_line(_mm256_set_m128i(d, c));
    let (a, b) = transpose(a, b);
    let (a, b) = transpose(forward_line(a), forward_line(b));
    let mut out = [0; 16];
    unsafe {
        _mm256_storeu_si256(out.as_mut_ptr().cast(), a);
        _mm256_storeu_si256(out.as_mut_ptr().add(8).cast(), b);
    }
    out
}

#[target_feature(enable = "avx2")]
unsafe fn zero4(coefficients: &[i32; 16], zero: &[u32; 16], separate_dc: bool) -> bool {
    for i in 0..2 {
        let values = unsafe { _mm256_loadu_si256(coefficients.as_ptr().add(i * 8).cast()) };
        let bounds = unsafe { _mm256_loadu_si256(zero.as_ptr().add(i * 8).cast()) };
        let exceeded = _mm256_cmpgt_epi32(_mm256_abs_epi32(values), bounds);
        let mut mask = _mm256_movemask_epi8(exceeded);
        if separate_dc && i == 0 {
            mask &= !15;
        }
        if mask != 0 {
            return false;
        }
    }
    true
}

#[target_feature(enable = "avx2")]
unsafe fn levels4(coefficients: &[i32; 16], bias: &[u64; 16], reciprocal: &[u64; 16]) -> [i32; 16] {
    let mut out = [0; 16];
    for i in 0..4 {
        let c = unsafe { _mm_loadu_si128(coefficients.as_ptr().add(i * 4).cast()) };
        let bias = unsafe { _mm256_loadu_si256(bias.as_ptr().add(i * 4).cast()) };
        let reciprocal = unsafe { _mm256_loadu_si256(reciprocal.as_ptr().add(i * 4).cast()) };
        let magnitude = _mm256_cvtepu32_epi64(_mm_abs_epi32(c));
        let numerator = _mm256_add_epi64(_mm256_slli_epi64::<7>(magnitude), bias);
        let magnitude = _mm256_srli_epi64::<40>(_mm256_mul_epu32(numerator, reciprocal));
        let magnitude = _mm256_castsi256_si128(_mm256_permutevar8x32_epi32(
            magnitude,
            _mm256_setr_epi32(0, 2, 4, 6, 0, 0, 0, 0),
        ));
        let sign = _mm_srai_epi32::<31>(c);
        let levels = _mm_sub_epi32(_mm_xor_si128(magnitude, sign), sign);
        unsafe { _mm_storeu_si128(out.as_mut_ptr().add(i * 4).cast(), levels) };
    }
    out
}

#[target_feature(enable = "avx2")]
unsafe fn scaled4(levels: &[i32; 16], scale: &[i32; 16]) -> [i32; 16] {
    let mut out = [0; 16];
    for i in 0..2 {
        let levels = unsafe { _mm256_loadu_si256(levels.as_ptr().add(i * 8).cast()) };
        let scale = unsafe { _mm256_loadu_si256(scale.as_ptr().add(i * 8).cast()) };
        unsafe {
            _mm256_storeu_si256(
                out.as_mut_ptr().add(i * 8).cast(),
                _mm256_mullo_epi32(levels, scale),
            );
        }
    }
    out
}

#[target_feature(enable = "avx2")]
fn outside(input: __m256i) -> __m256i {
    _mm256_or_si256(
        _mm256_cmpgt_epi32(input, _mm256_set1_epi32(32767)),
        _mm256_cmpgt_epi32(_mm256_set1_epi32(-32768), input),
    )
}

#[target_feature(enable = "avx2")]
fn inverse_line(input: __m256i) -> (__m256i, __m256i) {
    let a = _mm256_shuffle_epi32::<0x00>(input);
    let b = _mm256_shuffle_epi32::<0x55>(input);
    let c = _mm256_shuffle_epi32::<0xaa>(input);
    let d = _mm256_shuffle_epi32::<0xff>(input);
    let p = _mm256_add_epi32(a, c);
    let q = _mm256_sub_epi32(a, c);
    let r = _mm256_sub_epi32(_mm256_srai_epi32::<1>(b), d);
    let s = _mm256_add_epi32(b, _mm256_srai_epi32::<1>(d));
    let out = combine(
        _mm256_add_epi32(p, s),
        _mm256_add_epi32(q, r),
        _mm256_sub_epi32(q, r),
        _mm256_sub_epi32(p, s),
    );
    let low = _mm256_min_epi32(_mm256_min_epi32(p, q), _mm256_min_epi32(r, s));
    let high = _mm256_max_epi32(_mm256_max_epi32(p, q), _mm256_max_epi32(r, s));
    let invalid = _mm256_or_si256(
        _mm256_cmpgt_epi32(_mm256_set1_epi32(-32768), _mm256_min_epi32(low, out)),
        _mm256_cmpgt_epi32(_mm256_max_epi32(high, out), _mm256_set1_epi32(32767)),
    );
    (out, invalid)
}

#[target_feature(enable = "avx2")]
unsafe fn inverse4(coefficients: &[i32; 16]) -> Option<[i32; 16]> {
    let a = unsafe { _mm256_loadu_si256(coefficients.as_ptr().cast()) };
    let b = unsafe { _mm256_loadu_si256(coefficients.as_ptr().add(8).cast()) };
    if _mm256_movemask_epi8(_mm256_or_si256(outside(a), outside(b))) != 0 {
        return None;
    }
    let (a, invalid_a) = inverse_line(a);
    let (b, invalid_b) = inverse_line(b);
    let (a, b) = transpose(a, b);
    let (a, invalid_c) = inverse_line(a);
    let (b, invalid_d) = inverse_line(b);
    let invalid = _mm256_or_si256(
        _mm256_or_si256(invalid_a, invalid_b),
        _mm256_or_si256(invalid_c, invalid_d),
    );
    let round_limit = _mm256_or_si256(
        _mm256_cmpgt_epi32(a, _mm256_set1_epi32(32735)),
        _mm256_cmpgt_epi32(b, _mm256_set1_epi32(32735)),
    );
    if _mm256_movemask_epi8(_mm256_or_si256(invalid, round_limit)) != 0 {
        return None;
    }
    let (a, b) = transpose(a, b);
    let mut out = [0; 16];
    unsafe {
        _mm256_storeu_si256(out.as_mut_ptr().cast(), a);
        _mm256_storeu_si256(out.as_mut_ptr().add(8).cast(), b);
    }
    Some(out)
}
