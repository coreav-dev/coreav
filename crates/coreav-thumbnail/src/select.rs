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

/// The time at which `sample` may be given as a keyframe image: a sync
/// sample the container presents, at a known time. A keyframe whose time
/// is unknown is not one: the image would be reported at the time asked
/// for. The one rule for every search that offers a keyframe as the image
/// ([`keyframe`], the last resort of [`exact`], the window check of
/// `crate::window_holds`, and the Matroska listing check,
/// `mkv::cut_point_before`).
pub(crate) fn offered(sample: &Sample) -> Option<Rational> {
    if sample.keyframe && sample.presented {
        sample.time
    } else {
        None
    }
}

/// Whether `sample` is a keyframe [`offered`] at or before `at`.
pub(crate) fn offered_by(sample: &Sample, at: Rational) -> bool {
    offered(sample).is_some_and(|t| le(t, at))
}

/// Keyframe mode: the latest keyframe [`offered_by`] `at`. When none is
/// (`at` before the first presented keyframe, or an edit list that
/// presents no keyframe at all), the frame [`exact`] chooses.
/// `cut_point(i)` says whether a sync sample really starts decoding
/// (`docs/design/cut-points.md`); `forward` allows the last resort of
/// [`exact`].
pub(crate) fn keyframe(
    samples: &[Sample],
    at: Rational,
    forward: bool,
    cut_point: &mut dyn FnMut(usize) -> Result<bool, Error>,
) -> Result<Choice, Error> {
    check_end(samples, at)?;
    let mut candidates: Vec<(usize, Rational)> = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| offered_by(s, at))
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
    for (index, _) in candidates.iter().rev() {
        if cut_point(*index)? {
            return Ok(Choice {
                start: *index,
                target: *index,
            });
        }
    }
    exact(samples, at, forward, cut_point)
}

/// Exact mode: the first frame presented at or after `at`, decoded from
/// the latest keyframe before it in decode order that is not presented
/// after it. When no keyframe at or before it starts decoding and
/// `forward`, the first one after it that does, whose frame is the one a
/// decoder gives first there.
pub(crate) fn exact(
    samples: &[Sample],
    at: Rational,
    forward: bool,
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
    // Nothing at or before the frame starts decoding: a stream whose
    // first pictures precede its first parameter sets, or whose first
    // keyframe comes later. Its first frames are undecodable, as they are
    // for FFmpeg's decoder, which drops them and gives the frame of the
    // first picture that decodes; that frame is this one.
    if forward {
        // Every keyframe [`offered`] after the frame, in decode order: one
        // before it in decode order can be presented after it.
        for (index, sample) in samples.iter().enumerate() {
            let after = offered(sample).is_some_and(|t| lt(time, t));
            if after && cut_point(index)? {
                return Ok(Choice {
                    start: index,
                    target: index,
                });
            }
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
            keyframe(&s, r(5), true, &mut all).expect("k"),
            Choice {
                start: 0,
                target: 0
            }
        );
        assert_eq!(
            keyframe(&s, r(7), true, &mut all).expect("k"),
            Choice {
                start: 7,
                target: 7
            }
        );
        // A sync sample that does not start decoding is skipped.
        let mut not7 = |i| Ok(i != 7);
        assert_eq!(keyframe(&s, r(7), true, &mut not7).expect("k").target, 0);
        assert!(matches!(
            keyframe(&s, r(8), true, &mut all),
            Err(Error::TimeOutOfRange)
        ));
        // Before the first keyframe: the first frame, decoded from it.
        let before = Rational::new(-1, 1).expect("-1");
        assert_eq!(
            keyframe(&s, before, true, &mut all).expect("k"),
            Choice {
                start: 0,
                target: 0
            }
        );
        // An edit list that presents no keyframe: the frame at the time.
        let mut hidden = gop();
        hidden[0].presented = false;
        assert_eq!(
            keyframe(&hidden, r(2), true, &mut all).expect("k"),
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
            exact(&s, r(2), true, &mut all).expect("e"),
            Choice {
                start: 0,
                target: 3
            }
        );
        let half = Rational::new(9, 2).expect("4.5");
        assert_eq!(
            exact(&s, half, true, &mut all).expect("e"),
            Choice {
                start: 0,
                target: 6
            }
        );
        assert_eq!(
            exact(&s, r(7), true, &mut all).expect("e"),
            Choice {
                start: 7,
                target: 7
            }
        );
        let before = Rational::new(-1, 1).expect("-1");
        assert_eq!(exact(&s, before, true, &mut all).expect("e").target, 0);
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
            exact(&s, r(3), true, &mut all).expect("e"),
            Choice {
                start: 0,
                target: 3
            }
        );
    }

    #[test]
    fn nothing_before_the_frame_starts_decoding() {
        // Every block flagged a keyframe, only every fourth one a cut
        // point: a writer that muxed a byte stream whose parameter sets
        // and IDR pictures are in every fourth frame.
        let s: Vec<Sample> = (0..8)
            .map(|t| Sample {
                time: Some(r(t)),
                end: Some(r(t + 1)),
                presented: true,
                keyframe: true,
            })
            .collect();
        let mut every_fourth = |i: usize| Ok(i % 4 == 3);
        // The first cut point after a time before it.
        assert_eq!(
            keyframe(&s, r(0), true, &mut every_fourth).expect("k"),
            Choice {
                start: 3,
                target: 3
            }
        );
        assert_eq!(
            exact(&s, r(1), true, &mut every_fourth).expect("e"),
            Choice {
                start: 3,
                target: 3
            }
        );
        // The cut point at or before the time wins over it.
        assert_eq!(
            keyframe(&s, r(5), true, &mut every_fourth).expect("k"),
            Choice {
                start: 3,
                target: 3
            }
        );
        // Not taken from a window: a packet before it might be a cut
        // point.
        assert!(matches!(
            keyframe(&s, r(0), false, &mut every_fourth),
            Err(Error::Unsupported(_))
        ));
        // No cut point at all.
        let mut none = |_| Ok(false);
        assert!(matches!(
            exact(&s, r(0), true, &mut none),
            Err(Error::Unsupported(_))
        ));
    }
}
