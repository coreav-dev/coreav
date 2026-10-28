//! The samples' times from the source frames' presentation times and the
//! encoded pictures' presentation indices (`docs/design/transcode.md`,
//! "From Annex B to samples", "Timestamps").
//!
//! Frames go to the encoder in presentation order, so the `n`-th picture in
//! presentation order encodes the `n`-th frame sent, and its presentation
//! time is that frame's source time. Decode times are those same times in
//! their sorted order, shifted back by the largest reorder lead
//! `δ = max_k(T[k] − T[r(k)])`, which is never negative (the leads of a
//! permutation sum to 0): decode times then increase strictly and never
//! pass presentation times, so every composition offset is non-negative
//! (version 0 `ctts`), and a sample's duration is the distance to the next
//! one's decode time.
//!
//! Every value is computed with checked arithmetic; times that do not fit
//! an `i64` are an error.

use std::fmt;

/// One sample's times, in the source track's time scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleTime {
    /// Decode time.
    pub dts: i64,
    /// Presentation time.
    pub pts: i64,
    /// The next sample's decode time minus this one's; the last sample's is
    /// the caller's.
    pub duration: u64,
    /// `pts − dts`, never negative.
    pub composition_offset: u64,
}

/// Why sample times cannot be computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The two lists differ in length.
    LengthMismatch,
    /// The presentation times do not increase strictly.
    NotIncreasing,
    /// The presentation indices are not a permutation of `0..n`.
    NotAPermutation,
    /// A time does not fit an `i64`.
    Overflow,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LengthMismatch => "as many presentation indices as frames are needed",
            Self::NotIncreasing => "frame presentation times must increase strictly",
            Self::NotAPermutation => {
                "the pictures' presentation indices are not a permutation of the frames"
            }
            Self::Overflow => "a sample time is out of range",
        })
    }
}

impl std::error::Error for Error {}

/// The times of the samples in decode order.
///
/// `presented[i]` is the source presentation time of the `i`-th frame
/// sent (strictly increasing); `order[k]` is the presentation index of the
/// `k`-th picture in decode order. Sample `k` gets
/// `dts = presented[k] − δ`, `pts = presented[order[k]]`, the duration to
/// the next sample's decode time (`last_duration` for the last one), and
/// `pts − dts` as its composition offset.
pub fn sample_times(
    presented: &[i64],
    order: &[u64],
    last_duration: u64,
) -> Result<Vec<SampleTime>, Error> {
    if presented.len() != order.len() {
        return Err(Error::LengthMismatch);
    }
    if presented.windows(2).any(|w| matches!(w, [a, b] if a >= b)) {
        return Err(Error::NotIncreasing);
    }
    let mut seen = vec![false; presented.len()];
    let mut pts = Vec::with_capacity(presented.len());
    for &index in order {
        let slot = usize::try_from(index)
            .ok()
            .and_then(|i| seen.get_mut(i))
            .filter(|s| !**s)
            .ok_or(Error::NotAPermutation)?;
        *slot = true;
        let time = usize::try_from(index)
            .ok()
            .and_then(|i| presented.get(i))
            .ok_or(Error::NotAPermutation)?;
        pts.push(*time);
    }
    // The largest reorder lead; 0 for an empty list.
    let delta = presented
        .iter()
        .zip(&pts)
        .map(|(t, p)| i128::from(*t) - i128::from(*p))
        .max()
        .unwrap_or(0)
        .max(0);
    let mut out = Vec::with_capacity(presented.len());
    for (k, (&time, &pts)) in presented.iter().zip(&pts).enumerate() {
        let dts = i128::from(time) - delta;
        let duration = match presented.get(k + 1) {
            Some(&next) => {
                u64::try_from(i128::from(next) - i128::from(time)).map_err(|_| Error::Overflow)?
            }
            None => last_duration,
        };
        out.push(SampleTime {
            dts: i64::try_from(dts).map_err(|_| Error::Overflow)?,
            pts,
            duration,
            composition_offset: u64::try_from(i128::from(pts) - dts)
                .map_err(|_| Error::Overflow)?,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_reordering() {
        let times = sample_times(&[0, 10, 20], &[0, 1, 2], 10).unwrap();
        assert_eq!(
            times,
            vec![
                SampleTime {
                    dts: 0,
                    pts: 0,
                    duration: 10,
                    composition_offset: 0
                },
                SampleTime {
                    dts: 10,
                    pts: 10,
                    duration: 10,
                    composition_offset: 0
                },
                SampleTime {
                    dts: 20,
                    pts: 20,
                    duration: 10,
                    composition_offset: 0
                },
            ]
        );
    }

    #[test]
    fn b_frames_shift_decode_times_back() {
        // I P B B: decode order presents frames 0, 3, 1, 2.
        let presented = [100, 140, 160, 200];
        let times = sample_times(&presented, &[0, 3, 1, 2], 40).unwrap();
        // δ = max(100−100, 140−200, 160−140, 200−160) = 40.
        let dts: Vec<i64> = times.iter().map(|t| t.dts).collect();
        let pts: Vec<i64> = times.iter().map(|t| t.pts).collect();
        let offsets: Vec<u64> = times.iter().map(|t| t.composition_offset).collect();
        let durations: Vec<u64> = times.iter().map(|t| t.duration).collect();
        assert_eq!(dts, [60, 100, 120, 160]);
        assert_eq!(pts, [100, 200, 140, 160]);
        assert_eq!(offsets, [40, 100, 20, 0]);
        assert_eq!(durations, [40, 20, 40, 40]);
    }

    #[test]
    fn empty() {
        assert_eq!(sample_times(&[], &[], 5).unwrap(), vec![]);
    }

    #[test]
    fn errors() {
        assert_eq!(sample_times(&[0, 1], &[0], 1), Err(Error::LengthMismatch));
        assert_eq!(sample_times(&[0, 0], &[0, 1], 1), Err(Error::NotIncreasing));
        assert_eq!(sample_times(&[1, 0], &[0, 1], 1), Err(Error::NotIncreasing));
        assert_eq!(
            sample_times(&[0, 1], &[0, 0], 1),
            Err(Error::NotAPermutation)
        );
        assert_eq!(
            sample_times(&[0, 1], &[0, 2], 1),
            Err(Error::NotAPermutation)
        );
        assert_eq!(
            sample_times(&[0, 1], &[u64::MAX, 0], 1),
            Err(Error::NotAPermutation)
        );
        // A duration above i64::MAX still fits u64; the decode time does not.
        assert_eq!(
            sample_times(&[i64::MIN, i64::MAX], &[1, 0], 1),
            Err(Error::Overflow)
        );
        let wide = sample_times(&[i64::MIN, i64::MAX], &[0, 1], 1).unwrap();
        assert_eq!(wide[0].duration, u64::MAX);
    }
}
