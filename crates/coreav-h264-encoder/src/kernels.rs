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
        if let Some(simd) = arithmetic() {
            return Ok(simd.sad(a, b));
        }
    }
    Ok(a.iter()
        .zip(b)
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum())
}

#[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
fn arithmetic() -> Option<crate::simd::Avx2> {
    static SIMD: std::sync::OnceLock<Option<crate::simd::Avx2>> = std::sync::OnceLock::new();
    *SIMD.get_or_init(crate::simd::Avx2::detect)
}

pub(crate) fn satd(
    source: &[u8],
    predicted: &[u8],
    at: usize,
    stride: usize,
    width: usize,
) -> Result<u64> {
    if !matches!(width, 4 | 8 | 16) || stride < width {
        return Err(Error::Internal);
    }
    let extent = (width - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(width))
        .ok_or(Error::Internal)?;
    let source = source
        .get(at..)
        .and_then(|s| s.get(..extent))
        .ok_or(Error::Internal)?;
    let predicted = predicted
        .get(at..)
        .and_then(|s| s.get(..extent))
        .ok_or(Error::Internal)?;
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    if let Some(simd) = arithmetic() {
        return simd
            .satd(source, predicted, stride, width)
            .ok_or(Error::Internal);
    }
    let mut sum = 0;
    for y in (0..width).step_by(4) {
        for x in (0..width).step_by(4) {
            sum += crate::transform::satd(source, predicted, y * stride + x, stride)?;
        }
    }
    Ok(sum)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn satd_matches_scalar_for_every_full_swing_4x4_pattern() -> Result<()> {
        let b = [0; 16];
        for bits in 0..=u16::MAX {
            let a = std::array::from_fn::<_, 16, _>(|i| if bits & (1 << i) == 0 { 0 } else { 255 });
            assert_eq!(satd(&a, &b, 0, 4, 4), crate::transform::satd(&a, &b, 0, 4));
        }
        Ok(())
    }

    #[test]
    fn satd_matches_scalar_for_square_sizes_strides_and_extents() -> Result<()> {
        let mut seed = 0xcad28563147e9b0fu64;
        for width in [4, 8, 16] {
            for padding in [0, 1, 5, 31] {
                let stride = width + padding;
                for offset in 0..32 {
                    let extent = offset + (width - 1) * stride + width;
                    for pattern in 0..20 {
                        let mut a = vec![0; extent];
                        let mut b = vec![0; extent];
                        for (i, (a, b)) in a.iter_mut().zip(&mut b).enumerate() {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            *a = match pattern {
                                0 => 0,
                                1 => 255,
                                2 => {
                                    if i % 2 == 0 {
                                        255
                                    } else {
                                        0
                                    }
                                }
                                3 => {
                                    if i / stride % 2 == 0 {
                                        255
                                    } else {
                                        0
                                    }
                                }
                                _ => seed as u8,
                            };
                            *b = if pattern < 4 {
                                255 - *a
                            } else {
                                (seed >> 32) as u8
                            };
                        }
                        let mut want = 0;
                        for y in (0..width).step_by(4) {
                            for x in (0..width).step_by(4) {
                                want += crate::transform::satd(
                                    &a,
                                    &b,
                                    offset + y * stride + x,
                                    stride,
                                )?;
                            }
                        }
                        assert_eq!(satd(&a, &b, offset, stride, width), Ok(want));
                        #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
                        if let Some(simd) = arithmetic() {
                            assert_eq!(
                                simd.satd(
                                    a.get(offset..).ok_or(Error::Internal)?,
                                    b.get(offset..).ok_or(Error::Internal)?,
                                    stride,
                                    width
                                ),
                                Some(want)
                            );
                        }
                        assert_eq!(
                            satd(
                                a.get(..extent - 1).ok_or(Error::Internal)?,
                                &b,
                                offset,
                                stride,
                                width
                            ),
                            Err(Error::Internal)
                        );
                        assert_eq!(
                            satd(
                                &a,
                                b.get(..extent - 1).ok_or(Error::Internal)?,
                                offset,
                                stride,
                                width
                            ),
                            Err(Error::Internal)
                        );
                    }
                }
            }
        }
        for (at, stride, width) in [
            (0, 4, 0),
            (0, 4, 3),
            (0, 3, 4),
            (usize::MAX, 4, 4),
            (0, usize::MAX, 4),
        ] {
            assert_eq!(
                satd(&[0; 256], &[0; 256], at, stride, width),
                Err(Error::Internal)
            );
            #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
            if let Some(simd) = arithmetic() {
                assert_eq!(simd.satd(&[], &[], stride, width), None);
            }
        }
        Ok(())
    }

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
