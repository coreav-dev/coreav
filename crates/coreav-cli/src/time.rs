//! Calendar and number formatting.

use coreav_isobmff::{Rational, unix_time};

/// A creation time stored as seconds since 1904-01-01 UTC, as an RFC 3339
/// UTC string (`2026-10-16T02:19:27Z`); `None` when the stored value is 0.
pub fn rfc3339(stored: u64) -> Option<String> {
    civil(stored).map(|(date, seconds)| format!("{date}T{seconds}Z"))
}

fn civil(stored: u64) -> Option<(String, String)> {
    let unix = unix_time(stored)?;
    let days = unix.div_euclid(86_400);
    let seconds = unix.rem_euclid(86_400);
    // Civil date from days since 1970-01-01, proleptic Gregorian calendar.
    let z = days.checked_add(719_468)?;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe.checked_add(era.checked_mul(400)?)? + i64::from(month <= 2);
    Some((
        format!("{year:04}-{month:02}-{day:02}"),
        format!(
            "{:02}:{:02}:{:02}",
            seconds / 3_600,
            seconds % 3_600 / 60,
            seconds % 60
        ),
    ))
}

/// A rational as a floating-point number.
pub fn to_f64(value: Rational) -> f64 {
    // Precision loss is the point: this is the convenience copy.
    #[allow(clippy::cast_precision_loss)]
    let (num, den) = (value.num as f64, value.den as f64);
    num / den
}

/// A rational rounded to the nearest integer, when it fits in 64 bits.
pub fn rounded(value: Rational) -> Option<i64> {
    i64::try_from(value.round()).ok()
}

/// `value` × `factor`, exactly.
pub fn scale(value: Rational, factor: u64) -> Option<Rational> {
    Rational::new(value.num.checked_mul(i128::from(factor))?, value.den)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_leap_day() {
        // 1904-01-01 is stored 0, which means "not set".
        assert_eq!(rfc3339(0), None);
        // 1970-01-01T00:00:00Z is 2,082,844,800 seconds after 1904-01-01.
        assert_eq!(
            rfc3339(2_082_844_800).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        // 2024-02-29T12:34:56Z.
        let leap = 2_082_844_800 + 1_709_210_096;
        assert_eq!(rfc3339(leap).as_deref(), Some("2024-02-29T12:34:56Z"));
    }

    #[test]
    fn rational_helpers() {
        let half = Rational::new(1, 2).unwrap();
        assert!((to_f64(half) - 0.5).abs() < f64::EPSILON);
        assert_eq!(scale(half, 4).map(|r| (r.num, r.den)), Some((2, 1)));
        assert_eq!(rounded(Rational::new(7, 2).unwrap()), Some(4));
    }
}
