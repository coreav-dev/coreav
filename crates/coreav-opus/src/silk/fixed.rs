//! The integer arithmetic SILK's reconstruction is specified in (RFC 6716
//! Appendix A, the specification): products kept to
//! their high bits, rounding shifts, saturation, and the reference's two
//! approximate divisions, whose exact results the output depends on.
//!
//! Products are formed in `i64` and truncated to `i32`, which gives the
//! two's complement result of the reference's 32-bit forms for every
//! input, overflowing ones included.

/// `(a * b) >> 16`, rounded down.
pub(crate) fn mul16(a: i32, b: i32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> 16) as i32
}

/// `(a * b) >> 16` with `b` taken as its low 16 bits, sign-extended.
pub(crate) fn mul16_low(a: i32, b: i32) -> i32 {
    mul16(a, i32::from(b as i16))
}

/// `a / 2**shift`, rounded half up; `shift` from 1 to 31.
pub(crate) fn round_shift(a: i32, shift: u32) -> i32 {
    (((i64::from(a) >> (shift - 1)) + 1) >> 1) as i32
}

/// `a` clamped to the `i16` range.
pub(crate) fn sat16(a: i32) -> i16 {
    a.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// `a << shift`, saturated to the `i32` range first; `shift` below 32.
fn shift_left_saturating(a: i32, shift: u32) -> i32 {
    a.clamp(i32::MIN >> shift, i32::MAX >> shift) << shift
}

/// The bits of headroom of `a`: how far it shifts left before its sign
/// bit would change.
fn headroom(a: i32) -> u32 {
    a.unsigned_abs().leading_zeros().saturating_sub(1)
}

/// About `2**61 / b` scaled to the Q of `b`'s headroom: 14 bits from one
/// division, then one Newton step. Returns the estimate and that headroom.
fn reciprocal(b: i32) -> (i32, u32) {
    let shift = headroom(b);
    let normalized = b << shift;
    // Q(29 + 16 - shift); 0 only for b = 0, which callers never pass.
    let inverse = (i32::MAX >> 2).checked_div(normalized >> 16).unwrap_or(0);
    (inverse, shift)
}

/// The reference's approximation of `(1 << q) / b`, `q` above 0.
pub(crate) fn inverse(b: i32, q: u32) -> i32 {
    let (inverse, shift) = reciprocal(b);
    let normalized = b << shift;
    let first = inverse << 16;
    let error = ((1 << 29) - mul16_low(normalized, inverse)) << 3;
    let refined = first.wrapping_add(mul16(error, inverse));
    rescale(refined, 61 - i64::from(shift) - i64::from(q))
}

/// The reference's approximation of `(a << q) / b`.
pub(crate) fn divide(a: i32, b: i32, q: u32) -> i32 {
    let a_shift = headroom(a);
    let mut a_normalized = a << a_shift;
    let (inverse, b_shift) = reciprocal(b);
    let b_normalized = b << b_shift;
    let first = mul16_low(a_normalized, inverse);
    let product = ((i64::from(b_normalized) * i64::from(first)) >> 32) as i32;
    a_normalized = a_normalized.wrapping_sub(product << 3);
    let refined = first.wrapping_add(mul16_low(a_normalized, inverse));
    rescale(
        refined,
        29 + i64::from(a_shift) - i64::from(b_shift) - i64::from(q),
    )
}

/// `value` shifted right by `shift`, or left with saturation when
/// `shift` is negative; 0 when every bit shifts out.
fn rescale(value: i32, shift: i64) -> i32 {
    match u32::try_from(shift) {
        Ok(right) if right < 32 => value >> right,
        Ok(_) => 0,
        Err(_) => shift_left_saturating(value, u32::try_from(-shift).unwrap_or(31).min(31)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_and_shifts_round_as_specified() {
        assert_eq!(mul16(-1, 1), -1);
        assert_eq!(mul16(1 << 20, 3 << 15), 3 << 19);
        assert_eq!(mul16_low(1 << 16, 0x1_8000), -32768);
        assert_eq!(round_shift(3, 1), 2);
        assert_eq!(round_shift(-3, 1), -1);
        assert_eq!(round_shift(0x180, 8), 2);
        assert_eq!(sat16(40000), 32767);
        assert_eq!(sat16(-40000), -32768);
    }

    #[test]
    fn divisions_approximate_their_quotients() {
        for b in [81_920, 65_536, 1_000_003, 1_686_110_208] {
            let exact = (1i64 << 47) as f64 / f64::from(b);
            let got = f64::from(inverse(b, 47));
            assert!(
                (got - exact).abs() <= exact * 1e-6 + 1.0,
                "{b}: {got} {exact}"
            );
            for a in [81_920, 123_456_789, 1_686_110_208] {
                let exact = f64::from(a) * 65536.0 / f64::from(b);
                let got = f64::from(divide(a, b, 16));
                let exact = exact.min(f64::from(i32::MAX));
                assert!(
                    (got - exact).abs() <= exact * 1e-4 + 2.0,
                    "{a}/{b}: {got} {exact}"
                );
            }
        }
    }
}
