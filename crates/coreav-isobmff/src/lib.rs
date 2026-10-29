//! Memory-safe probing and packet listing for ISO base media files (MP4,
//! M4A, 3GP) and QuickTime movies (MOV).
//!
//! Every entry point takes [`Limits`] and an [`Allowlist`]; malformed or
//! hostile input produces an [`Error`], never a panic or unbounded work.
//! [`Limits::default`] gives the default bounds.
//!
//! ```
//! use coreav_isobmff::{probe, Allowlist, Limits, SliceSource};
//!
//! let not_a_movie = b"RIFF\x24\x00\x00\x00AVI LIST";
//! let result = probe(SliceSource::new(not_a_movie), &Limits::default(), &Allowlist::default());
//! assert!(result.is_err());
//! ```

pub mod access;
mod bitstream;
mod bytes;
mod codec;
mod error;
pub mod fragment;
pub mod frames;
mod gapless;
mod header;
mod limits;
pub mod metadata;
mod movie;
mod packets;
mod remote;
mod scan;
mod seek;
mod segment;
mod size;
mod source;
mod tables;

pub use codec::{
    Allowlist, Codec, CodecConfig, Color, ConfigRecord, DeclaredBitRate, DolbyVision,
    MAX_OTHER_BOXES, PcmFormat, ProResProfile, Profile, SampleDescription, avc_max_num_ref_frames,
    avc_timing, describe_audio_specific_config, describe_config_record, parameter_set_color,
};
pub use error::{Error, LimitKind, Warning};
pub use fragment::{FragmentSamples, TrackDefaults, subsegment_samples};
pub use gapless::{ItunesGapless, MAX_TAG_BOXES};
pub use header::{BoxHeader, MAX_HEADER_LEN, Parsed, parse_box_header};
pub use limits::Limits;
pub use movie::{
    DropReason, DroppedStream, DurationSource, EditList, IDENTITY_MATRIX, MAX_CHAPTER_REFERENCES,
    Movie, PresentationSpan, Track, TrackKind, probe, unix_time,
};
pub use packets::{Budget, Packet, Packets, SampleRun, StoredSample};
pub use remote::{
    ByteRange, DownloadReason, FragmentPlan, IndexRoute, IndexStatus, RangePolicy, RangeReader,
    plan_index,
};
pub use seek::Seeker;
pub use segment::{SegmentIndex, SegmentReference};
pub use size::Size;
pub use source::{BufferedSource, FileSource, MAX_READ, SliceSource, Source};

use core::fmt;

/// A four-character code: a box type or a sample entry type.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCc(pub [u8; 4]);

impl fmt::Display for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            if byte.is_ascii_graphic() || byte == b' ' {
                write!(f, "{}", char::from(byte))?;
            } else {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FourCc(\"{self}\")")
    }
}

/// An exact rational number with a positive denominator, in lowest terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    /// Numerator.
    pub num: i128,
    /// Denominator, never zero.
    pub den: u64,
}

impl Rational {
    /// Builds `num / den` in lowest terms; `None` when `den` is zero.
    pub fn new(num: i128, den: u64) -> Option<Self> {
        if den == 0 {
            return None;
        }
        let divisor = gcd(num.unsigned_abs(), u128::from(den));
        let num = num.checked_div(i128::try_from(divisor).ok()?)?;
        let den = u64::try_from(u128::from(den).checked_div(divisor)?).ok()?;
        Some(Self { num, den })
    }

    /// An integer value.
    pub fn from_int(value: i128) -> Self {
        Self { num: value, den: 1 }
    }

    /// Sum of two values, `None` on overflow.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        let num = self
            .num
            .checked_mul(i128::from(other.den))?
            .checked_add(other.num.checked_mul(i128::from(self.den))?)?;
        let den = self.den.checked_mul(other.den)?;
        Self::new(num, den)
    }

    /// Whether `self` is smaller than `other`, `None` on overflow.
    pub fn checked_lt(self, other: Self) -> Option<bool> {
        let left = self.num.checked_mul(i128::from(other.den))?;
        let right = other.num.checked_mul(i128::from(self.den))?;
        Some(left < right)
    }

    /// Rounds to the nearest integer, halves away from zero. Exact over the
    /// whole range; a zero denominator (reachable only by setting the fields
    /// directly) saturates by the sign of the numerator.
    pub fn round(self) -> i128 {
        let magnitude = self.num.unsigned_abs();
        let den = u128::from(self.den);
        let (Some(quotient), Some(remainder)) =
            (magnitude.checked_div(den), magnitude.checked_rem(den))
        else {
            return if self.num >= 0 { i128::MAX } else { i128::MIN };
        };
        // remainder < den, so den - remainder cannot underflow; a quotient
        // below den's magnitude bound cannot overflow when incremented.
        let rounded = if remainder >= den - remainder {
            quotient + 1
        } else {
            quotient
        };
        if self.num >= 0 {
            i128::try_from(rounded).unwrap_or(i128::MAX)
        } else {
            // The magnitude of i128::MIN is 2^127, one more than i128::MAX.
            i128::try_from(rounded).map_or(i128::MIN, |value| -value)
        }
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    if a == 0 { 1 } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_arithmetic() {
        let a = Rational::new(104_958, 10).unwrap();
        assert_eq!(
            a,
            Rational {
                num: 52_479,
                den: 5
            }
        );
        assert_eq!(a.round(), 10_496);
        assert_eq!(Rational::new(-3, 2).unwrap().round(), -2);
        assert_eq!(Rational::new(0, 9).unwrap(), Rational { num: 0, den: 1 });
        assert!(Rational::new(1, 0).is_none());
        let sum = a.checked_add(Rational::from_int(1)).unwrap();
        assert_eq!(
            sum,
            Rational {
                num: 52_484,
                den: 5
            }
        );
        assert_eq!(Rational::from_int(1).checked_lt(a), Some(true));
    }

    #[test]
    fn round_is_exact_at_the_edges() {
        assert_eq!(Rational::from_int(i128::MAX).round(), i128::MAX);
        assert_eq!(Rational::from_int(i128::MIN).round(), i128::MIN);
        assert_eq!(Rational::from_int(i128::MAX - 1).round(), i128::MAX - 1);
        assert_eq!(
            Rational::new(i128::MAX, 2).unwrap().round(),
            i128::MAX / 2 + 1
        );
        assert_eq!(
            Rational::new(i128::MIN + 1, 2).unwrap().round(),
            i128::MIN / 2
        );
        assert_eq!(Rational::new(i128::MAX, u64::MAX).unwrap().round(), {
            let den = i128::from(u64::MAX);
            let (q, r) = (i128::MAX / den, i128::MAX % den);
            if r >= den - r { q + 1 } else { q }
        });
        for (num, den, expected) in [
            (1, 2, 1),
            (-1, 2, -1),
            (5, 2, 3),
            (-5, 2, -3),
            (1, 3, 0),
            (2, 3, 1),
            (-2, 3, -1),
        ] {
            assert_eq!(
                Rational::new(num, den).unwrap().round(),
                expected,
                "{num}/{den}"
            );
        }
        assert_eq!(Rational { num: 7, den: 0 }.round(), i128::MAX);
        assert_eq!(Rational { num: -7, den: 0 }.round(), i128::MIN);
    }
}
