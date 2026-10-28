//! When a text sample is shown: the parts of its composition interval the
//! track's edits present, on the movie's presentation timeline (ISO/IEC
//! 14496-12 8.6.6, the edit list model of `docs/design/edit-lists.md` and,
//! for several media edits, `docs/design/edit-list-writers.md`).

use coreav_isobmff::{EditList, Error, Packet, Rational, Track};

/// The presented part of a track's media, in media ticks on the
/// presentation timeline; no bounds for a track without an edit list. With
/// several media edits every edit that shows part of a sample shows it
/// (ISO/IEC 14496-12 8.6.6: the entries play one after another, each
/// presenting its own window of the media), which
/// `docs/design/edit-list-writers.md` ("`coreav-text` timing") settles.
pub(crate) enum Edit<'a> {
    /// No edit list, or one media edit: `(start, end)` on the presentation
    /// timeline, `None` for a track without an edit list.
    One {
        start: Option<Rational>,
        end: Option<Rational>,
    },
    /// Several media edits, walked with the samples.
    Several(Windows<'a>),
}

impl<'a> Edit<'a> {
    /// The edit of `track`; fails for an edit list outside the supported
    /// model, whose presentation times are unknown.
    pub(crate) fn of(track: &'a Track, movie_timescale: u32) -> Result<Self, Error> {
        match track.edit_list {
            EditList::None => Ok(Self::One {
                start: None,
                end: None,
            }),
            EditList::Simple { .. } => {
                let start = track.presentation_offset(movie_timescale);
                let end = start
                    .zip(track.media_edit_duration(movie_timescale))
                    .and_then(|(start, length)| start.checked_add(length));
                match (start, end) {
                    (Some(start), Some(end)) => Ok(Self::One {
                        start: Some(start),
                        end: Some(end),
                    }),
                    _ => Err(unsupported("an edit that cannot be represented")),
                }
            }
            EditList::Multiple => Ok(Self::Several(Windows::new(track, movie_timescale)?)),
            EditList::Unsupported => Err(unsupported("an edit list outside the supported model")),
        }
    }

    /// The presented intervals of `packet` in media ticks, in order: one
    /// per run of media edits that show part of it with nothing between
    /// them, so an edit list that cuts the media into pieces and plays
    /// them back to back shows a cue once, over its whole length, and a
    /// splice inside a cue shows it on both sides of the splice. `out` is
    /// cleared first and is empty when no edit presents the sample.
    ///
    /// `None` when a time cannot be represented; the caller then counts
    /// the sample as not presented, as it does for an empty result.
    pub(crate) fn intervals(
        &mut self,
        packet: &Packet,
        out: &mut Vec<(Rational, Rational)>,
    ) -> Option<()> {
        out.clear();
        if !packet.presented {
            return Some(());
        }
        match self {
            Self::One { start, end } => {
                let mut from = packet.pts?;
                let mut to = from.checked_add(Rational::from_int(i128::from(packet.duration)))?;
                if let Some(edit_start) = *start {
                    if from.checked_lt(edit_start)? {
                        from = edit_start;
                    }
                }
                if let Some(edit_end) = *end {
                    if edit_end.checked_lt(to)? {
                        to = edit_end;
                    }
                }
                push(out, from, to)
            }
            Self::Several(windows) => windows.shown(packet.cts, packet.duration, out),
        }
    }
}

/// Adds `[from, to)` to `out`, with times before the timeline's origin cut
/// away and an interval that touches the one before it merged into it.
fn push(out: &mut Vec<(Rational, Rational)>, from: Rational, to: Rational) -> Option<()> {
    let zero = Rational::from_int(0);
    let from = if from.checked_lt(zero)? { zero } else { from };
    if to.checked_lt(from)? {
        return Some(());
    }
    match out.last_mut() {
        Some(last) if !last.1.checked_lt(from)? && !from.checked_lt(last.1)? => last.1 = to,
        _ => out.push((from, to)),
    }
    Some(())
}

/// The media edits of a list of several, walked in order with the samples.
/// Text samples come in composition order — a track with a composition
/// offset stops the extraction (`docs/design/text.md`, decision 7) — so one
/// cursor over the track's own entries serves every sample, and no table of
/// windows is built: a list of thousands of edits costs nothing beyond the
/// entries the parser already keeps.
pub(crate) struct Windows<'a> {
    track: &'a Track,
    movie_timescale: u32,
    /// The media edit the walk is on: its entry, and where that entry
    /// starts on the presentation timeline in movie ticks.
    at: (usize, u64),
}

impl<'a> Windows<'a> {
    fn new(track: &'a Track, movie_timescale: u32) -> Result<Self, Error> {
        if movie_timescale == 0 {
            return Err(unsupported("an edit list with movie timescale 0"));
        }
        let mut at = 0u64;
        for (i, edit) in track.edits.iter().enumerate() {
            if edit.media_time != -1 {
                return Ok(Self {
                    track,
                    movie_timescale,
                    at: (i, at),
                });
            }
            at = at.checked_add(edit.segment_duration).ok_or_else(entries)?;
        }
        Err(unsupported("an edit list with no media edit"))
    }

    /// `movie_ticks` in media ticks, exactly.
    fn ticks(&self, movie_ticks: u64) -> Option<Rational> {
        Rational::new(
            i128::from(movie_ticks).checked_mul(i128::from(self.track.timescale))?,
            u64::from(self.movie_timescale),
        )
    }

    /// The media edit at `c`: its media interval `[start, end)` and where
    /// it starts on the presentation timeline, in media ticks.
    fn window(&self, c: (usize, u64)) -> Option<(Rational, Rational, Rational)> {
        let edit = self.track.edits.get(c.0)?;
        let length = self.ticks(edit.segment_duration)?;
        let media_start = Rational::from_int(i128::from(edit.media_time));
        Some((
            media_start,
            media_start.checked_add(length)?,
            self.ticks(c.1)?,
        ))
    }

    /// The media edit after `c`, if any; the empty edits between them only
    /// move the presentation timeline on.
    fn after(&self, c: (usize, u64)) -> Option<(usize, u64)> {
        let (mut i, mut at) = c;
        loop {
            at = at.checked_add(self.track.edits.get(i)?.segment_duration)?;
            i = i.checked_add(1)?;
            match self.track.edits.get(i) {
                None => return None,
                Some(edit) if edit.media_time != -1 => return Some((i, at)),
                Some(_) => {}
            }
        }
    }

    /// The parts of the sample whose composition interval is
    /// `[cts, cts + duration)` that the media edits present, in order,
    /// appended to `out` (ISO/IEC 14496-12 8.6.6: each entry presents its
    /// own window of the media, one after another). A run of edits with
    /// nothing between them becomes one interval.
    ///
    /// The walk starts at the media edit that maps `cts` and never goes
    /// back: the samples come in composition order, and a later sample
    /// starts at or after this one.
    fn shown(
        &mut self,
        cts: i64,
        duration: u32,
        out: &mut Vec<(Rational, Rational)>,
    ) -> Option<()> {
        let first = Rational::from_int(i128::from(cts));
        let last = Rational::from_int(i128::from(cts).checked_add(i128::from(duration))?);
        // The first media edit whose media ends after `cts`, else the last.
        loop {
            let (_, media_end, _) = self.window(self.at)?;
            if first.checked_lt(media_end)? {
                break;
            }
            match self.after(self.at) {
                Some(next) => self.at = next,
                None => break,
            }
        }
        let mut c = self.at;
        loop {
            let (media_start, media_end, at) = self.window(c)?;
            // Past the sample: no later edit shows any of it either, since
            // the media edits go forward without overlap. A sample of no
            // duration is shown only by the edit whose media holds it, so
            // the first edit that starts after it ends the walk too — else
            // one such sample would cross the whole list.
            let past = if duration > 0 {
                !media_start.checked_lt(last)?
            } else {
                first.checked_lt(media_start)?
            };
            if past {
                return Some(());
            }
            let shows = if duration > 0 {
                first.checked_lt(media_end)? && media_start.checked_lt(last)?
            } else {
                !first.checked_lt(media_start)? && first.checked_lt(media_end)?
            };
            if shows {
                let from = if first.checked_lt(media_start)? {
                    media_start
                } else {
                    first
                };
                let to = if media_end.checked_lt(last)? {
                    media_end
                } else {
                    last
                };
                let shift = at.checked_add(negate(media_start)?)?;
                push(out, from.checked_add(shift)?, to.checked_add(shift)?)?;
            }
            match self.after(c) {
                Some(next) => c = next,
                None => return Some(()),
            }
        }
    }
}

/// `-value`.
fn negate(value: Rational) -> Option<Rational> {
    Rational::new(value.num.checked_neg()?, value.den)
}

fn entries() -> Error {
    unsupported("an edit list whose entries do not add up")
}

/// `ticks` media ticks in whole milliseconds, rounded to the nearest
/// (halves away from zero); `None` when negative or out of range.
pub(crate) fn milliseconds(ticks: Rational, timescale: u32) -> Option<u64> {
    let scaled = Rational::new(
        ticks.num.checked_mul(1000)?,
        ticks.den.checked_mul(u64::from(timescale))?,
    )?;
    u64::try_from(scaled.round()).ok()
}

pub(crate) fn unsupported(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milliseconds_round_to_the_nearest() {
        let t = |n| Rational::from_int(n);
        assert_eq!(milliseconds(t(500), 1000), Some(500));
        assert_eq!(milliseconds(t(1), 3000), Some(0));
        assert_eq!(milliseconds(t(2), 3000), Some(1));
        assert_eq!(milliseconds(t(3), 2000), Some(2));
        assert_eq!(milliseconds(Rational::new(1, 2).unwrap(), 1000), Some(1));
        assert_eq!(milliseconds(t(-1), 1), None);
    }
}
