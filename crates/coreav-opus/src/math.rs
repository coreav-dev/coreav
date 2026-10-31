//! The few functions of real numbers the decoder needs in `f32`, without
//! the C math library that `std`'s would link into every process
//! (`docs/design/audio-decode.md`, MP3 "Tables").

use core::f32::consts::LN_2;

/// 2^x to within a few units of the last place; 0 below 2^-126.
pub(crate) fn exp2(x: f32) -> f32 {
    if x.is_nan() || x <= -126.0 {
        return 0.0;
    }
    if x >= 128.0 {
        return f32::INFINITY;
    }
    // x = whole + f with f in [-1/2, 1/2].
    let mut whole = x as i32;
    if whole as f32 > x {
        whole -= 1;
    }
    let mut f = x - whole as f32;
    if f > 0.5 {
        whole += 1;
        f -= 1.0;
    }
    // e^(f ln 2) by its Taylor series; the next term is below 2^-27.
    let t = f * LN_2;
    let series = 1.0
        + t * (1.0
            + t * (1.0 / 2.0
                + t * (1.0 / 6.0
                    + t * (1.0 / 24.0
                        + t * (1.0 / 120.0 + t * (1.0 / 720.0 + t * (1.0 / 5040.0)))))));
    match u32::try_from(whole + 127) {
        Ok(biased) if biased > 0 => series * f32::from_bits(biased << 23),
        _ => 0.0,
    }
}

/// cos(pi/2 x) for x in [0, 1], as the spreading rotation takes it, by the
/// Taylor series of the cosine below 1/2 and of the sine of the complement
/// above, whose arguments stay within pi/4.
pub(crate) fn cos_half_pi(x: f32) -> f32 {
    let x = f64::from(x.clamp(0.0, 1.0));
    let r = core::f64::consts::FRAC_PI_2 * if x <= 0.5 { x } else { 1.0 - x };
    let r2 = r * r;
    // 1/2!, 1/4!, ... 1/12! and 1/3!, 1/5!, ... 1/13!, alternating in sign.
    let horner = |c: [f64; 6]| c.iter().rev().fold(0.0, |acc, &c| (acc + c) * r2);
    if x <= 0.5 {
        let even = [
            -1.0 / 2.0,
            1.0 / 24.0,
            -1.0 / 720.0,
            1.0 / 40_320.0,
            -1.0 / 3_628_800.0,
            1.0 / 479_001_600.0,
        ];
        (1.0 + horner(even)) as f32
    } else {
        let odd = [
            -1.0 / 6.0,
            1.0 / 120.0,
            -1.0 / 5_040.0,
            1.0 / 362_880.0,
            -1.0 / 39_916_800.0,
            1.0 / 6_227_020_800.0,
        ];
        (r * (1.0 + horner(odd))) as f32
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn exp2_matches_std() {
        for i in -1300..=400 {
            let x = i as f32 * 0.0937;
            let (got, want) = (super::exp2(x), x.exp2());
            assert!(
                (got - want).abs() <= 4.0 * f32::EPSILON * want,
                "{x}: {got} {want}"
            );
        }
        assert_eq!(super::exp2(-200.0), 0.0);
        assert_eq!(super::exp2(0.0), 1.0);
    }

    #[test]
    fn cos_half_pi_matches_std() {
        for i in 0..=1000 {
            let x = i as f32 / 1000.0;
            let want = (core::f32::consts::FRAC_PI_2 * x).cos();
            assert!((super::cos_half_pi(x) - want).abs() < 2e-7, "{x}");
        }
    }
}
