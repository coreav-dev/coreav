//! Exact source-clock admission, separate from the native settings cadence.

use std::{mem::size_of, num::NonZeroU64, time::Instant};

use coreav_decode::{Error, ErrorKind};
use coreav_isobmff::Rational;
use coreav_thumbnail::StreamPacket;

use crate::encoder::FrameRate;

/// Immutable expected presentation times with one checked nominal settings rate.
/// The settings rate is the exact presented average, not a conversion to CFR.
#[derive(Debug)]
pub struct Timeline {
    times: Vec<i64>,
    next: usize,
    rate: FrameRate,
    last_duration: NonZeroU64,
    deadline: Instant,
}

impl Timeline {
    /// Plans original presented times in order without repairing duplicates.
    /// Final duration comes from the last presented packet, else its preceding
    /// presentation interval. A single frame without a duration is refused.
    /// The memory bound charges the allocated timestamp capacity, not payloads.
    pub fn new(
        packets: &[StreamPacket],
        clock: Rational,
        max_frames: u64,
        max_metadata_bytes: u64,
        deadline: Instant,
    ) -> Result<Self, Error> {
        check_deadline(deadline)?;
        if max_frames == 0 || max_metadata_bytes < size_of::<i64>() as u64 {
            return Err(Error::new(ErrorKind::Limit, "invalid AV1 timeline limits"));
        }
        let clock_num = u128::try_from(clock.num)
            .ok()
            .filter(|num| *num > 0)
            .ok_or_else(|| {
                Error::new(ErrorKind::Unsupported, "AV1 source clock must be positive")
            })?;
        if clock.den == 0 {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "AV1 source clock has a zero denominator",
            ));
        }
        let mut count = 0usize;
        let mut last: Option<(i64, Option<u64>)> = None;
        for packet in packets {
            check_deadline(deadline)?;
            if !packet.presented {
                continue;
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| Error::new(ErrorKind::Limit, "AV1 timeline count overflows"))?;
            if count as u128 > u128::from(max_frames) {
                return Err(Error::new(
                    ErrorKind::Limit,
                    "AV1 source timeline exceeds the frame limit",
                ));
            }
            if packet.duration == Some(0) {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "AV1 source duration is zero",
                ));
            }
            if last.is_none_or(|(pts, _)| packet.pts > pts) {
                last = Some((packet.pts, packet.duration));
            }
        }
        if count == 0 {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "the source presents no AV1 input frame",
            ));
        }
        if count as u128 * size_of::<i64>() as u128 > u128::from(max_metadata_bytes) {
            return Err(Error::new(
                ErrorKind::Limit,
                "AV1 timeline exceeds its metadata budget",
            ));
        }
        let mut times = Vec::new();
        times
            .try_reserve_exact(count)
            .map_err(|_| Error::new(ErrorKind::Limit, "cannot store AV1 source timeline"))?;
        if times.capacity() as u128 * size_of::<i64>() as u128 > u128::from(max_metadata_bytes) {
            return Err(Error::new(
                ErrorKind::Limit,
                "AV1 timeline capacity exceeds its metadata budget",
            ));
        }
        for packet in packets {
            check_deadline(deadline)?;
            if packet.presented {
                times.push(packet.pts);
            }
        }
        check_deadline(deadline)?;
        times.sort_unstable();
        check_deadline(deadline)?;
        if times
            .windows(2)
            .any(|pair| matches!(pair, [a, b] if a == b))
        {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "AV1 source presentation times repeat",
            ));
        }
        if times.windows(2).any(
            |pair| matches!(pair, [a, b] if i128::from(*b) - i128::from(*a) > i128::from(u32::MAX)),
        ) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "AV1 source interval exceeds MP4 duration fields",
            ));
        }
        let duration = last
            .and_then(|(_, duration)| duration)
            .map(u128::from)
            .or_else(|| match times.as_slice() {
                [.., a, b] => u128::try_from(i128::from(*b) - i128::from(*a)).ok(),
                _ => None,
            })
            .and_then(|duration| u32::try_from(duration).ok())
            .filter(|duration| *duration > 0)
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::Unsupported,
                    "AV1 final source duration is unknown or out of range",
                )
            })?;
        let (&first, &last) = times
            .first()
            .zip(times.last())
            .ok_or_else(|| Error::new(ErrorKind::Unsupported, "the AV1 timeline is empty"))?;
        let ticks = u128::try_from(i128::from(last) - i128::from(first))
            .ok()
            .and_then(|span| span.checked_add(u128::from(duration)))
            .ok_or_else(|| Error::new(ErrorKind::Limit, "AV1 timeline span overflows"))?;
        let mut numerators = [count as u128, u128::from(clock.den)];
        let mut denominators = [ticks, clock_num];
        for numerator in &mut numerators {
            for denominator in &mut denominators {
                let divisor = gcd(*numerator, *denominator);
                *numerator /= divisor;
                *denominator /= divisor;
            }
        }
        let term = |factors: [u128; 2]| {
            factors
                .into_iter()
                .try_fold(1u128, u128::checked_mul)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::Unsupported,
                        "exact AV1 cadence exceeds native rational fields",
                    )
                })
        };
        let rate = FrameRate {
            num: term(numerators)?,
            den: term(denominators)?,
        };
        let last_duration = NonZeroU64::new(u64::from(duration)).ok_or_else(|| {
            Error::new(ErrorKind::Unsupported, "AV1 final source duration is zero")
        })?;
        check_deadline(deadline)?;
        Ok(Self {
            times,
            next: 0,
            rate,
            last_duration,
            deadline,
        })
    }

    /// Exact presented-average cadence, without snapping or a fallback rate.
    pub fn rate(&self) -> FrameRate {
        self.rate
    }

    /// Number of expected presented frames, excluding reference-only packets.
    pub fn frames(&self) -> u64 {
        self.times.len() as u64
    }

    /// Final sample duration in the unchanged source clock.
    pub fn last_duration(&self) -> NonZeroU64 {
        self.last_duration
    }

    pub(crate) fn contains(&self, pts: i64) -> bool {
        self.times.binary_search(&pts).is_ok()
    }

    pub(crate) fn metadata_bytes(&self) -> u64 {
        self.times.capacity() as u64 * size_of::<i64>() as u64
    }

    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub(super) fn admit(&mut self, pts: i64) -> Result<(), Error> {
        check_deadline(self.deadline)?;
        if self.times.get(self.next) != Some(&pts) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "AV1 frame differs from the next source presentation time",
            ));
        }
        self.next += 1;
        Ok(())
    }

    pub(super) fn complete(self, duration: NonZeroU64) -> Result<(), Error> {
        check_deadline(self.deadline)?;
        if self.next != self.times.len() || duration != self.last_duration {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "AV1 source timeline or final duration is incomplete",
            ));
        }
        Ok(())
    }
}

fn check_deadline(deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= deadline {
        return Err(Error::new(
            ErrorKind::Timeout,
            "AV1 timeline deadline expired",
        ));
    }
    Ok(())
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
