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
    let at = usize::from(byte(0) % 32);
    let stride = usize::from(byte(1) % 32);
    let extent = at + 3 * stride + 4;
    let a: Vec<_> = (0..extent).map(|i| byte(6 + i)).collect();
    let b: Vec<_> = (0..extent).map(|i| byte(6 + extent + i)).collect();
    let a = a
        .get(..extent.saturating_sub(usize::from(byte(2) % 2)))
        .unwrap_or_default();
    let b = b
        .get(..extent.saturating_sub(usize::from(byte(3) % 2)))
        .unwrap_or_default();
    let scalar = transform::forward_scalar(a, b, at, stride);
    assert_eq!(
        transform::forward(a, b, at, stride).map(|v| *v.coefficients()),
        scalar.as_ref().map(|v| *v.coefficients()).map_err(|e| *e)
    );
    let qp = byte(4) % 52;
    let separate_dc = byte(5) % 2 != 0;
    let Ok(quant) = transform::Quant::of(qp) else {
        return;
    };
    if let Ok(coefficients) = &scalar {
        assert_eq!(
            quant.zero(coefficients, separate_dc),
            quant.zero_scalar(coefficients, separate_dc)
        );
        assert_eq!(
            quant.levels(coefficients, separate_dc),
            quant.levels_scalar(coefficients, separate_dc)
        );
    }
    let levels =
        std::array::from_fn(|i| i32::from_le_bytes(std::array::from_fn(|j| byte(300 + i * 4 + j))));
    let scaled = quant.scaled_scalar(&levels);
    let inverse_input = std::array::from_fn(|i| {
        i32::from(i16::from_le_bytes([byte(400 + i * 2), byte(401 + i * 2)]))
    });
    let wide_inverse_input =
        std::array::from_fn(|i| i32::from_le_bytes(std::array::from_fn(|j| byte(400 + i * 4 + j))));
    let wide_inverse = transform::inverse_scalar(wide_inverse_input);
    if wide_inverse_input.iter().any(|&c| i16::try_from(c).is_err()) {
        assert!(wide_inverse.is_none());
    }
    #[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
    {
        let simd = simd::Avx2::detect();
        assert_eq!(simd.is_some(), std::is_x86_feature_detected!("avx2"));
        if let Some(simd) = simd {
            assert_eq!(
                simd.forward(a, b, at, stride),
                scalar.as_ref().ok().map(|v| *v.coefficients())
            );
            let (_, _, _, scale) = quant.parameters();
            assert_eq!(simd.scaled(&levels, scale), scaled);
            assert_eq!(
                simd.inverse(&inverse_input),
                transform::inverse_scalar(inverse_input)
            );
            assert_eq!(simd.inverse(&wide_inverse_input), wide_inverse);
        }
    }
    let mut actual: [u8; 16] = std::array::from_fn(|i| byte(450 + i));
    let mut expected = actual;
    let dc = if separate_dc {
        Some(inverse_input.first().copied().unwrap_or(0))
    } else {
        None
    };
    let mut coefficients = scaled;
    let want = if !coefficients.iter().all(|c| (-32768..=32767).contains(c)) {
        Err(Error::Coefficient)
    } else {
        if let (Some(dc), Some(first)) = (dc, coefficients.first_mut()) {
            *first = dc;
        }
        if coefficients.iter().all(|&c| c == 0) {
            Ok(())
        } else if let Some(residual) = transform::inverse_scalar(coefficients) {
            for (sample, r) in expected.iter_mut().zip(residual) {
                *sample = (i32::from(*sample) + ((r + 32) >> 6)).clamp(0, 255) as u8;
            }
            Ok(())
        } else {
            Err(Error::Coefficient)
        }
    };
    assert_eq!(quant.reconstruct(&levels, dc, &mut actual, 0, 4), want);
    assert_eq!(actual, expected);
});
