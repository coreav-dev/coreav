//! Which frame a time asks for, and which packets decode it
//! (`docs/design/thumbnails.md`, "Which frame").

use coreav_isobmff::Rational;

use crate::Error;

/// What selection needs of one packet of the video stream, in decode
/// order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sample {
    /// Presentation time in seconds on the file's timeline; `None` when it
    /// cannot be known.
    pub time: Option<Rational>,
    /// The end of its presentation in seconds, when known.
    pub end: Option<Rational>,
    /// The container presents it (an edit list may not).
    pub presented: bool,
    /// The container marks it as a sync sample.
    pub keyframe: bool,
}

/// The packets to decode, as indices into the samples, and the one whose
/// frame is the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Choice {
    /// First packet to decode (a keyframe).
    pub start: usize,
    /// The packet whose frame is wanted; packets `start..=target` are
    /// decoded.
    pub target: usize,
}

fn lt(a: Rational, b: Rational) -> bool {
    a.checked_lt(b).unwrap_or(false)
}

fn le(a: Rational, b: Rational) -> bool {
    !lt(b, a)
}

/// Fails when `at` is at or after the end of the stream's presentation.
fn check_end(samples: &[Sample], at: Rational) -> Result<(), Error> {
    let end = samples
        .iter()
        .filter(|s| s.presented)
        .filter_map(|s| s.end.or(s.time))
        .fold(None, |max: Option<Rational>, e| match max {
            Some(m) if le(e, m) => Some(m),
            _ => Some(e),
        });
    match end {
        Some(end) if le(end, at) => Err(Error::TimeOutOfRange),
        None => Err(Error::Unsupported(
            "the video stream has no presented frame with a known time".into(),
        )),
        _ => Ok(()),
    }
}

/// Keyframe mode: the latest keyframe presented at or before `at`. When
/// none is (`at` before the first presented keyframe, or an edit list that
/// presents no keyframe at all), the frame [`exact`] chooses. `cut_point(i)`
/// says whether a sync sample really starts decoding
/// (`docs/design/cut-points.md`).
pub(crate) fn keyframe(
    samples: &[Sample],
    at: Rational,
    cut_point: &mut dyn FnMut(usize) -> Result<bool, Error>,
) -> Result<Choice, Error> {
    check_end(samples, at)?;
    let mut candidates: Vec<(usize, Rational)> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.keyframe && s.presented)
        .filter_map(|(i, s)| s.time.map(|t| (i, t)))
        .collect();
    candidates.sort_by(|a, b| {
        if lt(a.1, b.1) {
            std::cmp::Ordering::Less
        } else if lt(b.1, a.1) {
            std::cmp::Ordering::Greater
        } else {
            a.0.cmp(&b.0)
        }
    });
    let split = candidates.partition_point(|(_, t)| le(*t, at));
    let before = candidates.get(..split).unwrap_or(&[]);
    for (index, _) in before.iter().rev() {
        if cut_point(*index)? {
            return Ok(Choice {
                start: *index,
                target: *index,
            });
        }
    }
    exact(samples, at, cut_point)
}

/// Exact mode: the first frame presented at or after `at`, decoded from
/// the latest keyframe before it in decode order that is not presented
/// after it.
pub(crate) fn exact(
    samples: &[Sample],
    at: Rational,
    cut_point: &mut dyn FnMut(usize) -> Result<bool, Error>,
) -> Result<Choice, Error> {
    check_end(samples, at)?;
    let target = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.presented)
        .filter_map(|(i, s)| s.time.map(|t| (i, t)))
        .filter(|(_, t)| le(at, *t))
        .fold(None, |best: Option<(usize, Rational)>, (i, t)| match best {
            Some((_, b)) if le(b, t) => best,
            _ => Some((i, t)),
        });
    // A time before the first frame gives the first frame.
    let (target, time) = target.ok_or(Error::TimeOutOfRange)?;
    for index in (0..=target).rev() {
        let Some(sample) = samples.get(index) else {
            continue;
        };
        let presented_before = sample.time.is_some_and(|t| le(t, time));
        if sample.keyframe && presented_before && cut_point(index)? {
            return Ok(Choice {
                start: index,
                target,
            });
        }
    }
    Err(Error::Unsupported(
        "no keyframe before the frame at that time".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i128) -> Rational {
        Rational::from_int(n)
    }

    /// I P B B P B B (decode order) with times 0 3 1 2 6 4 5, then I at 7.
    fn gop() -> Vec<Sample> {
        [
            (0, true),
            (3, false),
            (1, false),
            (2, false),
            (6, false),
            (4, false),
            (5, false),
            (7, true),
        ]
        .iter()
        .map(|&(t, k)| Sample {
            time: Some(r(t)),
            end: Some(r(t + 1)),
            presented: true,
            keyframe: k,
        })
        .collect()
    }

    #[test]
    fn keyframe_mode() {
        let s = gop();
        let mut all = |_| Ok(true);
        assert_eq!(
            keyframe(&s, r(5), &mut all).expect("k"),
            Choice {
                start: 0,
                target: 0
            }
        );
        assert_eq!(
            keyframe(&s, r(7), &mut all).expect("k"),
            Choice {
                start: 7,
                target: 7
            }
        );
        // A sync sample that does not start decoding is skipped.
        let mut not7 = |i| Ok(i != 7);
        assert_eq!(keyframe(&s, r(7), &mut not7).expect("k").target, 0);
        assert!(matches!(
            keyframe(&s, r(8), &mut all),
            Err(Error::TimeOutOfRange)
        ));
        // Before the first keyframe: the first frame, decoded from it.
        let before = Rational::new(-1, 1).expect("-1");
        assert_eq!(
            keyframe(&s, before, &mut all).expect("k"),
            Choice {
                start: 0,
                target: 0
            }
        );
        // An edit list that presents no keyframe: the frame at the time.
        let mut hidden = gop();
        hidden[0].presented = false;
        assert_eq!(
            keyframe(&hidden, r(2), &mut all).expect("k"),
            Choice {
                start: 0,
                target: 3
            }
        );
    }

    #[test]
    fn exact_mode() {
        let s = gop();
        let mut all = |_| Ok(true);
        // Time 2 is the third B frame in decode order (index 3).
        assert_eq!(
            exact(&s, r(2), &mut all).expect("e"),
            Choice {
                start: 0,
                target: 3
            }
        );
        let half = Rational::new(9, 2).expect("4.5");
        assert_eq!(
            exact(&s, half, &mut all).expect("e"),
            Choice {
                start: 0,
                target: 6
            }
        );
        assert_eq!(
            exact(&s, r(7), &mut all).expect("e"),
            Choice {
                start: 7,
                target: 7
            }
        );
        let before = Rational::new(-1, 1).expect("-1");
        assert_eq!(exact(&s, before, &mut all).expect("e").target, 0);
    }

    #[test]
    fn open_gop_leading_frame_decodes_from_the_previous_keyframe() {
        // I0 (t0) P (t2) I (t4) B (t3, leading the second I).
        let s: Vec<Sample> = [(0, true), (2, false), (4, true), (3, false)]
            .iter()
            .map(|&(t, k)| Sample {
                time: Some(r(t)),
                end: Some(r(t + 1)),
                presented: true,
                keyframe: k,
            })
            .collect();
        let mut all = |_| Ok(true);
        assert_eq!(
            exact(&s, r(3), &mut all).expect("e"),
            Choice {
                start: 0,
                target: 3
            }
        );
    }
}
