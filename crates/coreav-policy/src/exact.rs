//! Exact non-negative fractions: the values rules compare and the decimal
//! limits of a policy.

use core::cmp::Ordering;

/// A non-negative fraction `num / den` with `den > 0`, not necessarily in
/// lowest terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frac {
    pub(crate) num: u128,
    pub(crate) den: u128,
}

impl Frac {
    /// `num / den`; `None` when `den` is zero.
    pub(crate) fn new(num: u128, den: u128) -> Option<Self> {
        (den != 0).then_some(Self { num, den })
    }

    /// An integer.
    pub(crate) fn int(value: u64) -> Self {
        Self {
            num: u128::from(value),
            den: 1,
        }
    }

    /// Compares two fractions exactly, without overflow (Euclid's algorithm
    /// on the continued fractions).
    pub(crate) fn cmp_exact(self, other: Self) -> Ordering {
        let (mut a, mut b, mut c, mut d) = (self.num, self.den, other.num, other.den);
        // Each step compares a/b with c/d; flipping swaps the sense.
        let mut flipped = false;
        loop {
            // b and d are never zero here: they start non-zero and are
            // replaced only by non-zero remainders.
            let (Some(qa), Some(qc)) = (a.checked_div(b), c.checked_div(d)) else {
                return Ordering::Equal;
            };
            if qa != qc {
                let order = qa.cmp(&qc);
                return if flipped { order.reverse() } else { order };
            }
            let (ra, rc) = (a - qa * b, c - qc * d);
            match (ra == 0, rc == 0) {
                (true, true) => return Ordering::Equal,
                (true, false) => {
                    let order = Ordering::Less;
                    return if flipped { order.reverse() } else { order };
                }
                (false, true) => {
                    let order = Ordering::Greater;
                    return if flipped { order.reverse() } else { order };
                }
                (false, false) => {
                    // a/b = q + ra/b; compare ra/b with rc/d, that is d/rc
                    // with b/ra reversed.
                    (a, b, c, d) = (b, ra, d, rc);
                    flipped = !flipped;
                }
            }
        }
    }

    /// The fraction in lowest terms as a `{num, den}` pair, when both fit in
    /// 64 bits.
    pub(crate) fn reduced(self) -> Option<(u64, u64)> {
        let divisor = gcd(self.num, self.den);
        let num = u64::try_from(self.num.checked_div(divisor)?).ok()?;
        let den = u64::try_from(self.den.checked_div(divisor)?).ok()?;
        Some((num, den))
    }

    /// The nearest binary64 value, as a convenience copy.
    pub(crate) fn to_f64(self) -> f64 {
        // Precision loss is the point: this is the convenience copy.
        #[allow(clippy::cast_precision_loss)]
        let (num, den) = (self.num as f64, self.den as f64);
        num / den
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    if a == 0 { 1 } else { a }
}

/// Most digits of a decimal limit.
const MAX_DIGITS: usize = 30;

/// Parses a non-negative decimal as JSON writes it (`600`, `0.5`, `1e3`,
/// `2.5E-1`) into an exact fraction: the decimal the policy states, not the
/// nearest binary64. `None` for anything else, including a negative value,
/// more than 30 significant digits or an exponent that leaves more than 30
/// digits on either side of the point.
pub(crate) fn parse_decimal(text: &str) -> Option<Frac> {
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(at) => {
            let exponent = text.get(at + 1..)?;
            let exponent = exponent.strip_prefix('+').unwrap_or(exponent);
            (text.get(..at)?, exponent.parse::<i32>().ok()?)
        }
        None => (text, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !all_digits(whole) || !all_digits(fraction) {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let significant = digits.trim_start_matches('0');
    if significant.len() > MAX_DIGITS {
        return None;
    }
    let mut num: u128 = if significant.is_empty() {
        0
    } else {
        significant.parse().ok()?
    };
    let scale = i32::try_from(fraction.len()).ok()?.checked_sub(exponent)?;
    let den = if scale >= 0 {
        10u128.checked_pow(u32::try_from(scale).ok().filter(|s| *s <= 30)?)?
    } else {
        let shift = u32::try_from(-scale).ok().filter(|s| *s <= 30)?;
        num = num.checked_mul(10u128.checked_pow(shift)?)?;
        1
    };
    Frac::new(num, den)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(num: u128, den: u128) -> Frac {
        Frac::new(num, den).unwrap()
    }

    #[test]
    fn comparison_is_exact() {
        assert_eq!(f(1, 3).cmp_exact(f(1, 3)), Ordering::Equal);
        assert_eq!(f(2, 6).cmp_exact(f(1, 3)), Ordering::Equal);
        assert_eq!(f(1, 3).cmp_exact(f(1, 2)), Ordering::Less);
        assert_eq!(f(7, 2).cmp_exact(f(3, 1)), Ordering::Greater);
        assert_eq!(f(0, 5).cmp_exact(f(0, 1)), Ordering::Equal);
        assert_eq!(f(3, 1).cmp_exact(f(7, 2)), Ordering::Less);
        // Products of these overflow 128 bits; the comparison does not.
        let big = u128::MAX;
        assert_eq!(
            f(big, big - 1).cmp_exact(f(big - 1, big - 2)),
            Ordering::Less
        );
        assert_eq!(
            f(big - 1, big).cmp_exact(f(big - 2, big - 1)),
            Ordering::Greater
        );
        assert_eq!(f(big, big).cmp_exact(f(1, 1)), Ordering::Equal);
        // 600.0000001 s against a 600 s limit.
        assert_eq!(
            f(6_000_000_001, 10_000_000).cmp_exact(Frac::int(600)),
            Ordering::Greater
        );
    }

    #[test]
    fn decimals() {
        assert_eq!(parse_decimal("600"), Some(f(600, 1)));
        assert_eq!(parse_decimal("0.1"), Some(f(1, 10)));
        assert_eq!(parse_decimal("600.5"), Some(f(6005, 10)));
        assert_eq!(parse_decimal("1e3"), Some(f(1000, 1)));
        assert_eq!(parse_decimal("2.5E-1"), Some(f(25, 100)));
        assert_eq!(parse_decimal("1e+2"), Some(f(100, 1)));
        assert_eq!(parse_decimal("0"), Some(f(0, 1)));
        assert_eq!(parse_decimal("00012.50"), Some(f(1250, 100)));
        for bad in ["", "-1", ".5", "1.2.3", "1e", "e5", "abc", "1e400", "1e-31"] {
            assert_eq!(parse_decimal(bad), None, "{bad}");
        }
        assert_eq!(parse_decimal(&"9".repeat(31)), None);
        assert!(parse_decimal(&"9".repeat(30)).is_some());
    }

    #[test]
    fn reduction() {
        assert_eq!(f(6005, 10).reduced(), Some((1201, 2)));
        assert_eq!(f(0, 7).reduced(), Some((0, 1)));
        assert_eq!(f(u128::MAX, 1).reduced(), None);
    }
}
