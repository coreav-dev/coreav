//! Exact arithmetic on [`Rational`] for the clip writer's times; every
//! operation fails on overflow instead of wrapping.

use crate::moov::overflow;
use coreav_isobmff::{Error, Rational};

pub(crate) fn int(value: i64) -> Rational {
    Rational::from_int(i128::from(value))
}

pub(crate) fn add(a: Rational, b: Rational) -> Result<Rational, Error> {
    a.checked_add(b).ok_or_else(overflow)
}

pub(crate) fn sub(a: Rational, b: Rational) -> Result<Rational, Error> {
    let negated =
        Rational::new(b.num.checked_neg().ok_or_else(overflow)?, b.den).ok_or_else(overflow)?;
    add(a, negated)
}

pub(crate) fn lt(a: Rational, b: Rational) -> Result<bool, Error> {
    a.checked_lt(b).ok_or_else(overflow)
}

pub(crate) fn max(a: Rational, b: Rational) -> Result<Rational, Error> {
    Ok(if lt(a, b)? { b } else { a })
}

pub(crate) fn min(a: Rational, b: Rational) -> Result<Rational, Error> {
    Ok(if lt(b, a)? { b } else { a })
}

/// `a × mul / div`.
pub(crate) fn scale(a: Rational, mul: u32, div: u32) -> Result<Rational, Error> {
    let num = a.num.checked_mul(i128::from(mul)).ok_or_else(overflow)?;
    let den = a.den.checked_mul(u64::from(div)).ok_or_else(overflow)?;
    Rational::new(num, den).ok_or_else(overflow)
}

/// The smallest integer at least `a`.
pub(crate) fn ceil(a: Rational) -> Result<i128, Error> {
    let den = i128::from(a.den);
    let floor = a.num.checked_div_euclid(den).ok_or_else(overflow)?;
    let rem = a.num.checked_rem_euclid(den).ok_or_else(overflow)?;
    if rem == 0 {
        Ok(floor)
    } else {
        floor.checked_add(1).ok_or_else(overflow)
    }
}

/// The nearest integer, halves away from zero.
pub(crate) fn round(a: Rational) -> i128 {
    a.round()
}

/// An integer result as `u64`, failing when it is negative or too large.
pub(crate) fn to_u64(value: i128) -> Result<u64, Error> {
    u64::try_from(value).map_err(|_| overflow())
}

/// An integer result as `i64`.
pub(crate) fn to_i64(value: i128) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| overflow())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(num: i128, den: u64) -> Rational {
        Rational::new(num, den).unwrap()
    }

    #[test]
    fn exact_operations() {
        assert_eq!(sub(q(1, 2), q(1, 3)).unwrap(), q(1, 6));
        assert_eq!(scale(q(1, 7), 90_000, 1).unwrap(), q(90_000, 7));
        assert_eq!(ceil(q(90_000, 7)).unwrap(), 12_858);
        assert_eq!(ceil(q(-3, 2)).unwrap(), -1);
        assert_eq!(ceil(q(4, 2)).unwrap(), 2);
        assert!(!lt(q(1, 2), q(2, 4)).unwrap());
        assert_eq!(max(q(1, 3), q(1, 2)).unwrap(), q(1, 2));
        assert_eq!(min(q(1, 3), q(1, 2)).unwrap(), q(1, 3));
        assert!(sub(Rational::from_int(i128::MIN), q(1, 1)).is_err());
    }
}
