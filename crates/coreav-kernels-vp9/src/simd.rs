#![allow(unsafe_code)]

use core::arch::x86_64::{
    _mm_cvtsi64_si128, _mm256_add_epi32, _mm256_cvtepu8_epi32, _mm256_extract_epi32,
    _mm256_mullo_epi32, _mm256_set1_epi32, _mm256_setzero_si256,
};

#[derive(Clone, Copy)]
pub(super) struct Avx2 {
    _private: (),
}

impl Avx2 {
    pub(super) fn detect() -> Option<Self> {
        std::is_x86_feature_detected!("avx2").then_some(Self { _private: () })
    }

    pub(super) fn horizontal(self, samples: &[u8; 15], weights: &[i16; 8]) -> [i32; 8] {
        unsafe { horizontal8_avx2(samples, weights) }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn horizontal8_avx2(samples: &[u8; 15], weights: &[i16; 8]) -> [i32; 8] {
    let mut sums = _mm256_setzero_si256();
    for (t, &weight) in weights.iter().enumerate() {
        let bytes = samples
            .get(t..t + 8)
            .and_then(|window| <&[u8; 8]>::try_from(window).ok())
            .copied()
            .unwrap_or_default();
        let input = _mm_cvtsi64_si128(u64::from_le_bytes(bytes) as i64);
        let values = _mm256_cvtepu8_epi32(input);
        let weights = _mm256_set1_epi32(i32::from(weight));
        sums = _mm256_add_epi32(sums, _mm256_mullo_epi32(values, weights));
    }
    [
        _mm256_extract_epi32::<0>(sums),
        _mm256_extract_epi32::<1>(sums),
        _mm256_extract_epi32::<2>(sums),
        _mm256_extract_epi32::<3>(sums),
        _mm256_extract_epi32::<4>(sums),
        _mm256_extract_epi32::<5>(sums),
        _mm256_extract_epi32::<6>(sums),
        _mm256_extract_epi32::<7>(sums),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::SUBPEL_FILTERS;

    #[test]
    fn detected_avx2_executes_exact_signed_sums_and_lane_order() {
        let Some(avx2) = Avx2::detect() else {
            assert!(
                std::env::var_os("COREAV_REQUIRE_AVX2").is_none(),
                "the CI arithmetic host must expose AVX2"
            );
            return;
        };
        let mut seed = 0x713b_99e1u32;
        let extremes = [
            [i16::MIN; 8],
            [i16::MAX; 8],
            [i16::MIN, i16::MAX, -1, 1, 0, 128, -128, 32767],
        ];
        for weights in SUBPEL_FILTERS.iter().flatten().chain(extremes.iter()) {
            for pattern in 0..128 {
                let samples: [u8; 15] = core::array::from_fn(|i| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    match pattern % 5 {
                        0 => 255,
                        1 => {
                            if i & 1 == 0 {
                                0
                            } else {
                                255
                            }
                        }
                        2 => (i * 17) as u8,
                        3 => {
                            if i == pattern / 5 {
                                255
                            } else {
                                0
                            }
                        }
                        _ => seed as u8,
                    }
                });
                let expected: [i32; 8] = core::array::from_fn(|c| {
                    samples
                        .iter()
                        .skip(c)
                        .zip(weights)
                        .map(|(&s, &w)| i32::from(s) * i32::from(w))
                        .sum()
                });
                assert_eq!(avx2.horizontal(&samples, weights), expected);
            }
        }
        eprintln!("executed the AVX2 horizontal arithmetic kernel");
    }
}
