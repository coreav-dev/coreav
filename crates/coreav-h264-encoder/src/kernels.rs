#![forbid(unsafe_code)]

use crate::{Error, Result};
use coreav_kernels_h264::Auto;

pub(crate) fn pixels() -> Auto {
    #[cfg(feature = "scalar")]
    {
        Auto::scalar()
    }
    #[cfg(not(feature = "scalar"))]
    {
        static PIXELS: std::sync::OnceLock<Auto> = std::sync::OnceLock::new();
        *PIXELS.get_or_init(|| Auto::detect().without_avx512())
    }
}

pub(crate) fn sad(a: &[u8], b: &[u8]) -> Result<u64> {
    if a.len() != b.len() || a.len() > 16 {
        return Err(Error::Internal);
    }
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    {
        static SAD: std::sync::OnceLock<Option<crate::simd::Avx2>> = std::sync::OnceLock::new();
        if let Some(simd) = SAD.get_or_init(crate::simd::Avx2::detect) {
            return Ok(simd.sad(a, b));
        }
    }
    Ok(a.iter()
        .zip(b)
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn sad_matches_scalar_on_extremes_random_unaligned_and_final_byte_rows() {
        let mut seed = 0x98f2d65b3a17c40du64;
        for len in 0..=16 {
            for offset in 0..32 {
                for _ in 0..32 {
                    let mut a = vec![0; offset + len];
                    let mut b = a.clone();
                    for v in a.iter_mut().chain(&mut b) {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        *v = seed as u8;
                    }
                    let a = &a[offset..];
                    let b = &b[offset..];
                    let want = a
                        .iter()
                        .zip(b)
                        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
                        .sum();
                    assert_eq!(sad(a, b), Ok(want));
                    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
                    if let Some(simd) = crate::simd::Avx2::detect() {
                        assert_eq!(simd.sad(a, b), want);
                    }
                }
            }
            for a in 0..=255u8 {
                for b in [0, 1, 127, 128, 254, 255] {
                    assert_eq!(
                        sad(&vec![a; len], &vec![b; len]),
                        Ok(u64::from(a.abs_diff(b)) * len as u64)
                    );
                }
            }
        }
        assert_eq!(sad(&[0; 17], &[0; 17]), Err(Error::Internal));
        assert_eq!(sad(&[0; 16], &[0; 15]), Err(Error::Internal));
    }

    #[test]
    fn scalar_feature_overrides_pixel_dispatch() {
        #[cfg(feature = "scalar")]
        assert!(!pixels().uses_simd());
        #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
        assert_eq!(pixels().uses_simd(), std::is_x86_feature_detected!("avx2"));
    }
}
