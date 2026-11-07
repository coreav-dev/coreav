#![forbid(unsafe_code)]

use crate::{Error, Result};
use coreav_kernels_h264::Auto;
use coreav_kernels_h264::{Block, BlockMut, Kernels};

#[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
pub(crate) fn arithmetic() -> Option<crate::simd::Avx2> {
    static SIMD: std::sync::OnceLock<Option<crate::simd::Avx2>> = std::sync::OnceLock::new();
    *SIMD.get_or_init(crate::simd::Avx2::detect)
}

pub(crate) fn chroma_mc(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    if let Some(simd) = arithmetic() {
        return simd.chroma_mc(dst, src, xf, yf);
    }
    pixels().chroma_mc(dst, src, xf, yf);
}

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

    #[test]
    fn interpolation_matches_scalar_at_every_phase_shape_and_borrowed_boundary() {
        use coreav_kernels_h264::Scalar;
        let mut seed = 0x285d5ba9c13fu64;
        for chroma in [false, true] {
            let (phases, extra) = if chroma { (8, 1) } else { (4, 5) };
            for w in [2, 4, 8, 16] {
                for h in [1, 2, 4, 8, 16] {
                    for offset in [0, 1, 15, 31] {
                        for pattern in 0..6 {
                            let stride = w + extra + offset % 3;
                            let mut data = vec![0; offset + (h + extra - 1) * stride + w + extra];
                            for (i, v) in data.iter_mut().enumerate() {
                                seed ^= seed << 13;
                                seed ^= seed >> 7;
                                seed ^= seed << 17;
                                *v = match pattern {
                                    0 => 0,
                                    1 => 255,
                                    2 => {
                                        if i % 2 == 0 {
                                            0
                                        } else {
                                            255
                                        }
                                    }
                                    3 => {
                                        if i % stride < stride / 2 {
                                            0
                                        } else {
                                            255
                                        }
                                    }
                                    _ => seed as u8,
                                };
                            }
                            let src =
                                Block::new(&data, offset, stride, w + extra, h + extra).unwrap();
                            for yf in 0..phases {
                                for xf in 0..phases {
                                    let mut actual = vec![93; offset + (h - 1) * (w + 3) + w];
                                    let mut expected = actual.clone();
                                    let dst =
                                        BlockMut::new(&mut actual, offset, w + 3, w, h).unwrap();
                                    let scalar =
                                        BlockMut::new(&mut expected, offset, w + 3, w, h).unwrap();
                                    if chroma {
                                        chroma_mc(dst, src, xf, yf);
                                        Scalar.chroma_mc(scalar, src, xf, yf);
                                    } else {
                                        pixels().luma_mc(dst, src, xf, yf);
                                        Scalar.luma_mc(scalar, src, xf, yf);
                                    }
                                    assert_eq!(
                                        actual, expected,
                                        "chroma={chroma} {w}x{h} phase={xf},{yf} pattern={pattern}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
