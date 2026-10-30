//! Building the index: one listing of each packaged track.

use super::codecs::{aac_channels, codec_string};
use super::deferred;
use super::index::{Built, Edit, IndexTrack, SegmentEntry, encode_records, serialize};
use super::nal::Units;
use super::playlist::target_duration;
use super::segment::{Plan, Sample};
use super::{HlsOptions, MAX_SOURCE_TAG, Rendition};
use crate::clip::{gapless, select_audio, select_video, video_sample_entry};
use crate::copy::Shared;
use crate::m4a::{audio_sample_entry, unsupported};
use crate::moov::{language, overflow};
use crate::rational::{round, to_i64, to_u64};
use crate::{ExtractError, VideoChoice};
use core::cell::RefCell;
use coreav_isobmff::{
    Allowlist, Budget, EditList, Error, LimitKind, Limits, Movie, Rational, Source, Track,
    TrackKind, probe,
};

/// Memory charged per buffered sample of the open segment.
const SAMPLE_CHARGE: u64 = 32;
/// Most consecutive segments one peak bit rate window spans.
const MAX_WINDOW: usize = 64;

/// Lists `source` once per packaged track and returns the index (design
/// note `docs/design/hls.md`).
///
/// The source is probed with `limits` and `allowlist`. The video track
/// (H.264 `avc1`, HEVC `hvc1`, AV1 or VP9) and the AAC track are chosen as
/// for [`clip_mp4`](crate::clip_mp4); by default, a source without a video
/// track (kept or dropped by the allowlist) is packaged as audio alone.
/// Video segments start at the first sync sample at or after each multiple
/// of `options.segment_duration` that every earlier sample in decode order
/// is presented before, so that the segments partition the presentation
/// timeline; audio segments start at the first audio sync sample presented
/// at or after each video segment's start. Samples after the last presented
/// one are left out, and so is the video before the sync sample that an
/// edit starting inside the media needs; audio keeps its leading samples.
///
/// Memory is bounded by the index itself (a few bytes per sample, counted
/// against `limits.max_table_bytes`) and one segment's samples.
pub fn build_index<S: Source>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &HlsOptions,
) -> Result<Vec<u8>, ExtractError> {
    let mut source = source;
    if options.segment_duration.num <= 0 || options.segment_duration.den == 0 {
        return Err(unsupported("a segment duration of 0 or less").into());
    }
    if options.source_tag.len() > MAX_SOURCE_TAG {
        return Err(unsupported("a source tag longer than 255 bytes").into());
    }
    let movie = probe(&mut source, limits, allowlist)?;
    // The default packages the audio alone when the source has no video
    // track, kept or dropped. Other dropped tracks (a QuickTime chapter
    // text track, say) do not count, nor do dropped chapter images (a
    // video track a `chap` reference names, as audiobooks carry).
    let no_video = !movie.tracks.iter().any(|t| t.kind == TrackKind::Video)
        && !movie
            .dropped
            .iter()
            .any(|d| d.kind == TrackKind::Video && !movie.chapter_tracks.contains(&d.track_id));
    let video = match options.video {
        Some(VideoChoice::First) if no_video => None,
        Some(choice) => Some(select_video(&movie, choice)?),
        None => None,
    };
    let video_entry = video.map(video_sample_entry).transpose()?;
    let audio = select_audio(&movie, options.audio)?;
    let audio_entry = audio.map(audio_sample_entry).transpose()?;
    let source_len = source.len()?;
    let mut budget = Budget::new(limits);
    // A fragmented source whose segment index lists every subsegment is
    // indexed from that index alone, and a request reads the one `moof`
    // of the segment it serves (design note, "Sources with a segment
    // index"). Anything the index cannot say lists the source as usual.
    let mut renditions: Vec<(&Track, Rendition, &[u8])> = Vec::with_capacity(2);
    if let (Some(track), Some(stsd)) = (video, video_entry.as_deref()) {
        renditions.push((track, Rendition::Video, stsd));
    }
    if let (Some(track), Some(stsd)) = (audio, audio_entry.as_deref()) {
        renditions.push((track, Rendition::Audio, stsd));
    }
    if renditions.is_empty() {
        return Err(Error::NoSuchTrack.into());
    }
    if let Some(mut built) = deferred::plan(
        &movie,
        &renditions,
        &mut source,
        source_len,
        limits,
        options,
        &mut budget,
    )? {
        return Ok(serialize(
            source_len,
            &options.source_tag,
            options.segment_duration,
            &mut built,
        )?);
    }
    drop(renditions);
    let mut memory = Memory {
        used: 0,
        limit: limits.max_table_bytes,
    };
    let mut built = Vec::with_capacity(2);
    let mut video_starts = None;
    if let (Some(track), Some(stsd)) = (video, video_entry.clone()) {
        let cuts = Cuts::Grid(options.segment_duration);
        let listed = list(
            &movie,
            track,
            Rendition::Video,
            &cuts,
            &mut source,
            limits,
            &mut budget,
            &mut memory,
        )?;
        video_starts = Some((
            listed
                .track
                .segments
                .iter()
                .map(|s| s.start)
                .collect::<Vec<_>>(),
            track.timescale,
        ));
        built.push(finish(listed, track, stsd, options.segment_duration)?);
    }
    if let (Some(track), Some(stsd)) = (audio, audio_entry.clone()) {
        let cuts = match &video_starts {
            Some((starts, timescale)) => Cuts::Follow {
                starts,
                timescale: *timescale,
            },
            None => Cuts::Grid(options.segment_duration),
        };
        let listed = list(
            &movie,
            track,
            Rendition::Audio,
            &cuts,
            &mut source,
            limits,
            &mut budget,
            &mut memory,
        )?;
        built.push(finish(listed, track, stsd, options.segment_duration)?);
    }
    if built.is_empty() {
        return Err(Error::NoSuchTrack.into());
    }
    Ok(serialize(
        source_len,
        &options.source_tag,
        options.segment_duration,
        &mut built,
    )?)
}

/// The target-duration grid, as one comparison per sample: the multiple a
/// cut waits for, and the smallest presentation time that reaches it.
struct Grid {
    /// The target duration in media ticks, numerator.
    unit: i128,
    /// Its denominator.
    den: i128,
    /// The next multiple, counted in targets.
    next: i128,
    /// Smallest presentation time at or after it; `None` when no time of
    /// the track reaches it.
    point: Option<i64>,
}

impl Grid {
    fn new(target: Rational, timescale: u32) -> Result<Self, Error> {
        let mut grid = Self {
            unit: target
                .num
                .checked_mul(i128::from(timescale))
                .ok_or_else(overflow)?,
            den: i128::from(target.den),
            next: 1,
            point: None,
        };
        grid.set(1)?;
        Ok(grid)
    }

    /// Waits for the `next`-th multiple of the target duration.
    fn set(&mut self, next: i128) -> Result<(), Error> {
        // p × den ≥ next × unit, with den above 0: p ≥ ⌈next × unit / den⌉.
        let limit = next.checked_mul(self.unit).ok_or_else(overflow)?;
        let whole = limit.checked_div_euclid(self.den).ok_or_else(overflow)?;
        let point = match limit.checked_rem_euclid(self.den) {
            Some(0) => whole,
            _ => whole.checked_add(1).ok_or_else(overflow)?,
        };
        self.next = next;
        self.point = i64::try_from(point).ok().or_else(|| {
            // Below every presentation time, or above all of them.
            (point < 0).then_some(i64::MIN)
        });
        Ok(())
    }

    fn reached(&self, p: i64) -> bool {
        self.point.is_some_and(|point| p >= point)
    }

    /// Waits for the first multiple after the cut at `p`, so that a GOP
    /// longer than the target gives one segment per GOP.
    fn pass(&mut self, p: i64) -> Result<(), Error> {
        let scaled = i128::from(p).checked_mul(self.den).ok_or_else(overflow)?;
        let next = scaled
            .checked_div(self.unit)
            .and_then(|k| k.checked_add(1))
            .ok_or_else(overflow)?;
        self.set(next)
    }
}

/// One video segment start on an audio track's timeline.
struct FollowPoint {
    /// Where the audio segment starts, in the audio track's ticks.
    at: i64,
    /// Smallest presentation time of the audio track at or after the video
    /// segment's start.
    crossed_at: i64,
}

/// The video's segment starts in a track of `timescale` ticks.
fn follow_points(
    starts: &[u64],
    video_timescale: u32,
    timescale: u32,
) -> Result<Vec<FollowPoint>, Error> {
    starts
        .iter()
        .map(|start| {
            let ticks = Rational::new(
                i128::from(*start)
                    .checked_mul(i128::from(timescale))
                    .ok_or_else(overflow)?,
                u64::from(video_timescale),
            )
            .ok_or_else(overflow)?;
            let whole = ticks
                .num
                .checked_div_euclid(i128::from(ticks.den))
                .ok_or_else(overflow)?;
            let crossed_at = match ticks.num.checked_rem_euclid(i128::from(ticks.den)) {
                Some(0) => whole,
                _ => whole.checked_add(1).ok_or_else(overflow)?,
            };
            Ok(FollowPoint {
                at: to_i64(round(ticks))?,
                crossed_at: i64::try_from(crossed_at).unwrap_or(i64::MAX),
            })
        })
        .collect()
}

/// Where segments start.
enum Cuts<'a> {
    /// At the first eligible sample at or after each multiple of this many
    /// seconds.
    Grid(Rational),
    /// At the first sample presented at or after each of these starts, in
    /// ticks of `timescale` (the video's).
    Follow { starts: &'a [u64], timescale: u32 },
}

/// Charges for the index under construction.
struct Memory {
    used: u64,
    limit: u64,
}

impl Memory {
    #[inline]
    fn charge(&mut self, bytes: u64) -> Result<(), Error> {
        self.used = self.used.checked_add(bytes).ok_or_else(overflow)?;
        if self.used > self.limit {
            return Err(Error::LimitExceeded {
                limit: LimitKind::TableBytes,
                offset: 0,
            });
        }
        Ok(())
    }

    fn release(&mut self, bytes: u64) {
        self.used = self.used.saturating_sub(bytes);
    }
}

/// How a track's times map to the output (design note, "Timing").
pub(super) struct Timing {
    /// The source edit's leading empty edit, rounded to a whole tick.
    pub(super) empty: i64,
    /// The source edit's media time.
    pub(super) media_time: i64,
    /// Whether the source has an edit (an edit list, or a gapless item
    /// standing for one).
    edited: bool,
    /// The source edit's media duration, rounded to a whole tick; `None`
    /// without an edit, or for a media edit that lasts to the end of the
    /// media (a stored duration of 0).
    segment: Option<i64>,
}

impl Timing {
    /// From the track's edit list, or, for an audio track without one, from
    /// its `iTunSMPB` item when the item stands for one (as for clips and
    /// extraction).
    pub(super) fn of<S: Source>(
        movie: &Movie,
        track: &Track,
        source: &mut S,
    ) -> Result<Self, Error> {
        match track.edit_list {
            EditList::None => Ok(match gapless(movie, track, source)? {
                Some(item) => Self {
                    empty: 0,
                    media_time: i64::from(item.encoder_delay),
                    edited: true,
                    segment: Some(i64::try_from(item.original_samples).map_err(|_| overflow())?),
                },
                None => Self {
                    empty: 0,
                    media_time: 0,
                    edited: false,
                    segment: None,
                },
            }),
            EditList::Simple {
                media_time,
                segment,
                ..
            } => {
                let missing = || unsupported("edit list with movie timescale 0");
                let empty = track
                    .presentation_offset(movie.timescale)
                    .ok_or_else(missing)?;
                // 0: the media edit lasts to the end of the media, as
                // fragmented recordings often store it (14496-12 8.6.6).
                let segment = match segment {
                    0 => None,
                    _ => {
                        let duration = track
                            .media_edit_duration(movie.timescale)
                            .ok_or_else(missing)?;
                        Some(to_i64(round(duration))?)
                    }
                };
                Ok(Self {
                    empty: to_i64(round(empty))?,
                    media_time,
                    edited: true,
                    segment,
                })
            }
            // The pinned ffmpeg applies at most the first media edit of
            // a fragmented rendition, and one media edit has nowhere to
            // hide a sample a presented sample needs (both measured in
            // docs/design/edit-list-writers.md, "The HLS writers").
            EditList::Multiple => Err(unsupported(
                "edit list with several edits (multi-edit), not supported here yet",
            )),
            EditList::Unsupported => Err(unsupported("edit list outside the supported model")),
        }
    }

    /// Whether the edit presents any part of the sample presented at `p`
    /// for `duration` ticks (a sample of duration 0 when `p` lies in it).
    #[inline]
    fn presents(&self, p: i64, duration: u32) -> Result<bool, Error> {
        if !self.edited {
            return Ok(true);
        }
        let before_end = self.window_end().is_none_or(|end| p < end);
        if duration == 0 {
            return Ok(self.empty <= p && before_end);
        }
        let p_end = p.checked_add(i64::from(duration)).ok_or_else(overflow)?;
        Ok(before_end && self.empty < p_end)
    }

    /// Presentation time of composition time `cts`, in whole ticks.
    #[inline]
    fn present(&self, cts: i64) -> Result<i64, Error> {
        cts.checked_sub(self.media_time)
            .and_then(|t| t.checked_add(self.empty))
            .ok_or_else(overflow)
    }

    /// End of the edit's presentation window, if it has one (not for an
    /// edit that lasts to the end of the media).
    pub(super) fn window_end(&self) -> Option<i64> {
        self.empty.checked_add(self.segment?)
    }

    /// The output's edit list: the source's, in media ticks; a duration of
    /// 0 for a media edit that lasts to the end of the media.
    pub(super) fn edit(&self) -> Result<Option<Edit>, Error> {
        if !self.edited {
            return Ok(None);
        }
        Ok(Some(Edit {
            empty: to_u64(i128::from(self.empty))?,
            media_time: self.media_time,
            duration: match self.segment {
                Some(segment) => to_u64(i128::from(segment))?,
                None => 0,
            },
        }))
    }
}

/// The presented samples' durations, for the maximum frame rate.
#[derive(Default)]
struct Rate {
    /// Shortest duration above 0 of every presented sample but the last.
    shortest: Option<u32>,
    /// The last presented sample's duration, which no later decode time
    /// constrains.
    last: Option<u32>,
    count: u64,
    sum: u64,
}

impl Rate {
    #[inline]
    fn add(&mut self, duration: u32) {
        if let Some(previous) = self.last.replace(duration).filter(|d| *d > 0) {
            self.shortest = Some(self.shortest.map_or(previous, |s| s.min(previous)));
            self.count = self.count.saturating_add(1);
            self.sum = self.sum.saturating_add(u64::from(previous));
        }
    }

    /// The shortest duration, or 0 when it is under half the mean, which a
    /// stray sample of a tick or two gives rather than a frame rate.
    fn shortest(&self) -> u32 {
        match self.shortest {
            Some(s) if u64::from(s).saturating_mul(2).saturating_mul(self.count) >= self.sum => s,
            _ => 0,
        }
    }
}

/// A listed track before its header is finished.
struct Listed {
    track: IndexTrack,
    records: Vec<u8>,
    /// Bytes of each segment, header included.
    sizes: Vec<u64>,
}

/// A segment being filled.
struct Open {
    /// Presentation time of its first sample, the cut.
    cut: i64,
    /// Earliest presentation time of its samples.
    earliest: i64,
    /// Latest presentation time of every sample before it.
    latest_before: Option<i64>,
    /// The grid position before the cut, restored when the cut is undone.
    grid_before: i128,
    samples: Vec<Sample>,
    /// Source decode time of the first sample.
    base_decode_time: i64,
    next_dts: i64,
}

impl Open {
    /// `samples` is an empty buffer, handed on from the segment before
    /// so that one allocation serves a whole track.
    fn new(
        p: i64,
        dts: i64,
        latest_before: Option<i64>,
        grid_before: i128,
        samples: Vec<Sample>,
    ) -> Self {
        Self {
            cut: p,
            earliest: p,
            latest_before,
            grid_before,
            samples,
            base_decode_time: dts,
            next_dts: dts,
        }
    }

    /// Adds a sample decoded at `dts` and, if presented, presented at `p`.
    #[inline]
    fn push(
        &mut self,
        sample: Sample,
        p: Option<i64>,
        dts: i64,
        memory: &mut Memory,
    ) -> Result<(), ExtractError> {
        // Durations are all a fragment stores, so each sample must start
        // where the previous one ends.
        if self.next_dts != dts {
            return Err(unsupported("a gap or an overlap between decode times").into());
        }
        self.next_dts = dts
            .checked_add(i64::from(sample.duration))
            .ok_or_else(overflow)?;
        if let Some(p) = p {
            self.earliest = self.earliest.min(p);
        }
        memory.charge(SAMPLE_CHARGE)?;
        self.samples.push(sample);
        Ok(())
    }

    /// Whether every sample is presented after every sample before the
    /// segment, so that segments partition the presentation timeline.
    fn partitions(&self) -> bool {
        self.latest_before.is_none_or(|l| self.earliest > l)
    }
}

#[allow(clippy::too_many_arguments)]
fn list<S: Source>(
    movie: &Movie,
    track: &Track,
    rendition: Rendition,
    cuts: &Cuts<'_>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
    memory: &mut Memory,
) -> Result<Listed, ExtractError> {
    let timing = Timing::of(movie, track, &mut *source)?;
    let ts = track.timescale;
    let mut listed = Listed {
        track: header_of(track, rendition)?,
        records: Vec::new(),
        sizes: Vec::new(),
    };
    // The segment being filled, and a video segment whose cut waits for
    // the next sync sample: samples decoded after a sync sample and
    // presented before it (leading samples of an open GOP) come before it.
    let mut open: Option<Open> = None;
    let mut pending: Option<Open> = None;
    // Grid: the next multiple of the target duration, counted in targets.
    let mut grid = Grid::new(
        match cuts {
            Cuts::Grid(target) => *target,
            Cuts::Follow { .. } => Rational::from_int(1),
        },
        ts,
    )?;
    // Follow: the video's segment starts in this track's ticks, each with
    // the smallest presentation time that crosses it, so that crossing one
    // is one comparison per sample; and the next start to cross.
    let follow = match cuts {
        Cuts::Grid(_) => Vec::new(),
        Cuts::Follow { starts, timescale } => follow_points(starts, *timescale, ts)?,
    };
    let mut next_start = 1usize;
    let mut latest: Option<i64> = None;
    let mut end: Option<i64> = None;
    let mut leading = false;
    // A sample presented during the leading empty edit.
    let mut in_empty = false;
    // Unpresented samples since the last presented one.
    let mut tail = 0usize;
    // Before the first presented sample: the latest presentation time of
    // every sample so far.
    let mut hidden_latest: Option<i64> = None;
    let mut any_presented = false;
    let mut rate = Rate::default();
    // Sync flags that say nothing (every sample so far marked sync) are
    // checked against the bitstream at a cut, for H.264 and HEVC.
    let units = Units::of(track).filter(|_| rendition == Rendition::Video);
    let mut all_sync = true;
    let shared = RefCell::new(&mut *source);
    let mut packets = movie.packets(track.index, Shared(&shared), limits, budget)?;
    // The finished segment's buffer, handed to the next one: one segment's
    // samples are held at a time, so one allocation serves the track.
    let mut spare: Vec<Sample> = Vec::new();
    // Every sample is visited, so the listing takes them sample by sample:
    // the walk needs no record per sample of its own, and it
    // charges one visit each, as listing them packet by packet charged
    // them. The listing needs the stored values alone, not
    // the presentation times each packet carries.
    packets.for_each_sample(|s| -> Result<(), ExtractError> {
        {
            let keyframe = s.keyframe;
            let duration = s.duration;
            let size = s.size;
            let cto = s.composition_offset;
            if !(i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&cto) {
                return Err(unsupported("a composition offset outside 32 bits").into());
            }
            // Offsets of 2^31 and more come from version 0 tables whose
            // writers meant them as negative, as players read them: they
            // are copied as stored, but segments and durations are planned
            // with them read as signed.
            let signed = if cto > i64::from(i32::MAX) {
                cto.checked_sub(1 << 32).ok_or_else(overflow)?
            } else {
                cto
            };
            {
                let offset = s.offset;
                let dts = s.dts;
                all_sync &= keyframe;
                let p = timing.present(dts.checked_add(signed).ok_or_else(overflow)?)?;
                let presented = timing.presents(p, duration)?;
                let sample = Sample {
                    offset,
                    size,
                    duration,
                    cto,
                    sync: keyframe,
                };
                // An edit that starts inside the media hides what comes
                // before it: the video is copied from the latest sync
                // sample at or before the edit's start that every earlier
                // sample is presented before; nothing presented depends on
                // what is left out. The first presented sample can restart
                // the video too: a sync sample presented at or across the
                // edit's start is the latest one at or before it.
                // Audio keeps every sample (design note, "Timing").
                if !any_presented {
                    let mut restart = match rendition {
                        Rendition::Video => {
                            keyframe && p <= timing.empty && hidden_latest.is_some_and(|l| p > l)
                        }
                        Rendition::Audio => false,
                    };
                    // Sync flags that say nothing: the bitstream decides
                    // here too.
                    if restart && all_sync {
                        if let Some(units) = units {
                            let mut cell = shared.try_borrow_mut().map_err(|_| overflow())?;
                            restart = units.starts_sequence(&mut **cell, offset, size)?;
                        }
                    }
                    hidden_latest = Some(hidden_latest.map_or(p, |l| l.max(p)));
                    if restart {
                        if let Some(dropped) = open.take() {
                            let count = u64::try_from(dropped.samples.len()).unwrap_or(u64::MAX);
                            memory.release(count.saturating_mul(SAMPLE_CHARGE));
                        }
                    }
                }
                any_presented |= presented;
                in_empty |= (0..timing.empty).contains(&p);
                if presented {
                    let p_end = p.checked_add(i64::from(duration)).ok_or_else(overflow)?;
                    end = Some(end.map_or(p_end, |e| e.max(p_end)));
                    rate.add(duration);
                }
                if keyframe {
                    if let Some(next) = pending.take() {
                        resolve(
                            next,
                            &mut open,
                            &mut listed,
                            &mut grid,
                            &mut leading,
                            memory,
                            &mut spare,
                        )?;
                    }
                }
                // Whether a segment starts here, and where it is presented
                // from.
                let cut = match cuts {
                    Cuts::Grid(_) => {
                        // A segment starts only at a sync sample presented
                        // after every sample before it in decode order.
                        let mut eligible = keyframe && presented && latest.is_none_or(|l| p > l);
                        if eligible && all_sync && open.is_some() {
                            if let Some(units) = units {
                                let mut cell = shared.try_borrow_mut().map_err(|_| overflow())?;
                                eligible = units.starts_sequence(&mut **cell, offset, size)?;
                            }
                        }
                        if eligible && grid.reached(p) && open.is_some() {
                            let before = grid.next;
                            grid.pass(p)?;
                            Some((p, before))
                        } else {
                            None
                        }
                    }
                    // Audio starts at a sync sample; a sample a source
                    // marks otherwise joins the segment before.
                    Cuts::Follow { .. } if !keyframe || !presented => None,
                    Cuts::Follow { .. } => {
                        let mut crossed = None;
                        while let Some(start) = follow.get(next_start) {
                            if p < start.crossed_at {
                                break;
                            }
                            crossed = Some(start.at);
                            next_start = next_start.saturating_add(1);
                        }
                        match (crossed, &open) {
                            (Some(start), Some(_)) => Some((start, grid.next)),
                            _ => None,
                        }
                    }
                };
                if let Some((at, grid_before)) = cut {
                    let mut next =
                        Open::new(at, dts, latest, grid_before, core::mem::take(&mut spare));
                    next.earliest = p;
                    if rendition == Rendition::Video {
                        pending = Some(next);
                    } else {
                        // Every audio sample is a sync sample presented in
                        // decode order: the cut needs no confirmation.
                        if let Some(segment) = open.replace(next) {
                            spare = close(segment, &mut listed, memory)?;
                        }
                    }
                }
                // Samples the edit leaves unpresented (encoder priming,
                // say) take no part in where segments start.
                let shown = presented.then_some(p);
                tail = if presented { 0 } else { tail.saturating_add(1) };
                if let Some(p) = shown {
                    latest = Some(latest.map_or(p, |l| l.max(p)));
                }
                let segment = match (pending.as_mut(), open.as_mut()) {
                    (Some(segment), _) | (None, Some(segment)) => segment,
                    (None, None) => {
                        if !keyframe {
                            return Err(
                                unsupported("the track does not start with a sync sample").into()
                            );
                        }
                        let mut first =
                            Open::new(0, dts, None, grid.next, core::mem::take(&mut spare));
                        first.earliest = p;
                        open.insert(first)
                    }
                };
                segment.push(sample, shown, dts, memory)?;
            }
        }
        Ok(())
    })?;
    // Samples decoded after the last presented one are never presented,
    // and no presented sample depends on them: they are left out, as
    // readers that apply the edit list drop them. They all lie in the last
    // segment, which starts with a presented sample.
    if let Some(last) = pending.as_mut().or(open.as_mut()) {
        let kept = last.samples.len().saturating_sub(tail);
        last.samples.truncate(kept);
        memory.release(
            u64::try_from(tail)
                .unwrap_or(0)
                .saturating_mul(SAMPLE_CHARGE),
        );
    }
    if let Some(next) = pending.take() {
        resolve(
            next,
            &mut open,
            &mut listed,
            &mut grid,
            &mut leading,
            memory,
            &mut spare,
        )?;
    }
    if let Some(segment) = open.take() {
        close(segment, &mut listed, memory)?;
    }
    let mut end = end.ok_or(unsupported("no sample of the track is presented"))?;
    if let Some(window_end) = timing.window_end() {
        end = end.min(window_end);
    }
    let last = listed.track.segments.last().map_or(0, |s| s.start);
    let t = &mut listed.track;
    t.end = u64::try_from(end).unwrap_or(0).max(last);
    t.independent = !leading;
    t.min_sample_duration = rate.shortest();
    t.edit = match timing.edit()? {
        // A leading empty edit becomes a decode time shift, which players
        // that skip empty edits keep, when nothing it would expose is
        // presented: the media edit then starts at 0 and lasts that much
        // longer.
        Some(edit) if edit.empty > 0 && !in_empty => {
            for entry in &mut t.segments {
                entry.base_decode_time = entry
                    .base_decode_time
                    .checked_add(edit.empty)
                    .ok_or_else(overflow)?;
            }
            Some(Edit {
                empty: 0,
                media_time: edit.media_time,
                // A media edit to the end of the media stays one.
                duration: match edit.duration {
                    0 => 0,
                    d => d.checked_add(edit.empty).ok_or_else(overflow)?,
                },
            })
        }
        edit => edit,
    };
    Ok(listed)
}

/// Settles a video cut at the next sync sample: kept when its samples are
/// all presented after every earlier sample, else undone by merging them
/// into the segment before.
#[allow(clippy::too_many_arguments)]
fn resolve(
    next: Open,
    open: &mut Option<Open>,
    listed: &mut Listed,
    grid: &mut Grid,
    leading: &mut bool,
    memory: &mut Memory,
    spare: &mut Vec<Sample>,
) -> Result<(), Error> {
    if next.partitions() {
        // Presented from its earliest sample, which a leading sample of an
        // open GOP puts before the cut.
        *leading |= next.earliest < next.cut;
        if let Some(segment) = open.replace(next) {
            *spare = close(segment, listed, memory)?;
        }
        return Ok(());
    }
    grid.set(next.grid_before)?;
    let segment = open.as_mut().ok_or_else(overflow)?;
    segment.earliest = segment.earliest.min(next.earliest);
    segment.next_dts = next.next_dts;
    segment.samples.extend(next.samples);
    Ok(())
}

/// Appends a finished segment's entry and records. The first segment
/// starts at 0, every other one at its earliest presentation time.
fn close(
    mut segment: Open,
    listed: &mut Listed,
    memory: &mut Memory,
) -> Result<Vec<Sample>, Error> {
    let start = match listed.track.segments.last() {
        None => 0,
        Some(previous) => u64::try_from(segment.earliest.min(segment.cut))
            .unwrap_or(0)
            .max(previous.start),
    };
    // Segment numbers (the `mfhd` sequence, counted from 1) are 32 bits.
    u32::try_from(listed.track.segments.len())
        .ok()
        .and_then(|n| n.checked_add(1))
        .ok_or_else(overflow)?;
    let base_decode_time = u64::try_from(segment.base_decode_time)
        .map_err(|_| unsupported("a negative decode time"))?;
    // The fragment is planned, not written: building the index needs its
    // size for the bit rates (RFC 8216 4.3.4.2), and a request writes it.
    let plan = Plan::of(&segment.samples)?;
    let data_bytes = plan.data_bytes;
    let size = plan.len()?;
    let records = u64::try_from(listed.records.len()).map_err(|_| overflow())?;
    encode_records(&segment.samples, &mut listed.records);
    let records_len = u64::try_from(listed.records.len())
        .ok()
        .and_then(|n| n.checked_sub(records))
        .ok_or_else(overflow)?;
    let count = u64::try_from(segment.samples.len()).map_err(|_| overflow())?;
    memory.release(count.saturating_mul(SAMPLE_CHARGE));
    memory.charge(records_len.checked_add(40).ok_or_else(overflow)?)?;
    let t = &mut listed.track;
    t.samples = t.samples.checked_add(count).ok_or_else(overflow)?;
    t.data_bytes = t.data_bytes.checked_add(data_bytes).ok_or_else(overflow)?;
    t.segments.push(SegmentEntry {
        start,
        base_decode_time,
        samples: u32::try_from(count).map_err(|_| overflow())?,
        data_bytes,
        records,
        records_len: u32::try_from(records_len).map_err(|_| overflow())?,
    });
    listed.sizes.push(size);
    segment.samples.clear();
    Ok(segment.samples)
}

/// The track header without segments, from the source track.
pub(super) fn header_of(track: &Track, rendition: Rendition) -> Result<IndexTrack, Error> {
    let description = &track.description;
    // The display size: the picture a decoder configured from the codec
    // configuration's parameter sets produces, else the sample entry's
    // size, widened by the pixel aspect ratio (`pasp`); else the track
    // header's size.
    let coded = match description.parameter_set_size {
        Some((w, h)) => (
            u16::try_from(w).unwrap_or(u16::MAX),
            u16::try_from(h).unwrap_or(u16::MAX),
        ),
        None => (
            description.width.unwrap_or(0),
            description.height.unwrap_or(0),
        ),
    };
    let display = match description.pixel_aspect {
        Some((h, v)) if h > 0 && v > 0 && coded.0 > 0 => (
            u16::try_from((u64::from(coded.0) * u64::from(h) + u64::from(v) / 2) / u64::from(v))
                .unwrap_or(u16::MAX),
            coded.1,
        ),
        _ => {
            let (w, h) = (
                track.presentation_size.0 >> 16,
                track.presentation_size.1 >> 16,
            );
            match (u16::try_from(w), u16::try_from(h)) {
                (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
                _ => coded,
            }
        }
    };
    Ok(IndexTrack {
        rendition,
        stream_index: u32::try_from(track.stream_index).map_err(|_| overflow())?,
        track_id: track.track_id,
        codecs: codec_string(track)?,
        timescale: track.timescale,
        language: language(track.language),
        created: track.creation_time,
        media_created: track.media_creation_time,
        matrix: track.matrix,
        presentation_size: track.presentation_size,
        width: display.0,
        height: display.1,
        // The configuration's channel count (players decode it), not the
        // sample entry's.
        channels: aac_channels(track).unwrap_or(0),
        sample_rate: description.sample_rate.unwrap_or(0),
        samples: 0,
        data_bytes: 0,
        independent: true,
        deferred: None,
        edit: None,
        end: 0,
        peak_bit_rate: 0,
        average_bit_rate: 0,
        min_sample_duration: 0,
        stsd: Vec::new(),
        segments: Vec::new(),
        records_offset: 0,
        records_len: 0,
    })
}

/// Fills in the sample entry and the bit rates.
fn finish(listed: Listed, track: &Track, stsd: Vec<u8>, target: Rational) -> Result<Built, Error> {
    let Listed {
        track: mut index_track,
        records,
        sizes,
        ..
    } = listed;
    let ts = track.timescale;
    let durations: Vec<u64> = (0..index_track.segments.len())
        .map(|n| {
            let start = index_track.segments.get(n).map_or(0, |s| s.start);
            index_track
                .segment_end(n)
                .unwrap_or(start)
                .saturating_sub(start)
        })
        .collect();
    // RFC 8216 4.3.4.2 measures the peak against the playlist's own target
    // duration, which long GOPs can make longer than the requested one.
    let target = target_duration(&index_track).map_or(target, Rational::from_int);
    let (peak, average) = bit_rates(&durations, &sizes, ts, target)?;
    index_track.peak_bit_rate = peak;
    index_track.average_bit_rate = average;
    index_track.stsd = stsd;
    Ok(Built {
        track: index_track,
        records,
    })
}

/// The peak segment bit rate of RFC 8216 4.3.4.2 (the largest bit rate of
/// any run of consecutive segments lasting between half and one and a half
/// target durations; the whole rendition when no run does) and the
/// average, in bits per second, rounded up.
pub(super) fn bit_rates(
    durations: &[u64],
    sizes: &[u64],
    timescale: u32,
    target: Rational,
) -> Result<(u64, u64), Error> {
    let rate = |bytes: u128, ticks: u128| -> u64 {
        if ticks == 0 {
            return 0;
        }
        let bits = bytes
            .saturating_mul(8)
            .saturating_mul(u128::from(timescale));
        u64::try_from(bits.div_ceil(ticks)).unwrap_or(u64::MAX)
    };
    // Twice a run's ticks times den, against target × timescale.
    let unit = u128::try_from(target.num)
        .ok()
        .and_then(|n| n.checked_mul(u128::from(timescale)))
        .ok_or_else(overflow)?;
    let den = u128::from(target.den);
    let mut peak = None;
    for first in 0..durations.len() {
        let (mut ticks, mut bytes) = (0u128, 0u128);
        for (d, z) in durations.iter().zip(sizes).skip(first).take(MAX_WINDOW) {
            ticks = ticks.saturating_add(u128::from(*d));
            bytes = bytes.saturating_add(u128::from(*z));
            let twice = ticks.saturating_mul(2).saturating_mul(den);
            if twice > unit.saturating_mul(3) {
                break;
            }
            if twice >= unit {
                peak = Some(peak.unwrap_or(0).max(rate(bytes, ticks)));
            }
        }
    }
    let total_ticks = durations.iter().map(|d| u128::from(*d)).sum::<u128>();
    let total_bytes = sizes.iter().map(|z| u128::from(*z)).sum::<u128>();
    let average = rate(total_bytes, total_ticks);
    Ok((peak.unwrap_or(average), average))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_over_runs_near_the_target() {
        let target = Rational::from_int(6);
        // Three six-second segments and a short last one at 1 Hz ticks.
        let durations = [6, 6, 6, 1];
        let sizes = [600, 1200, 600, 1000];
        let (peak, average) = bit_rates(&durations, &sizes, 1, target).unwrap();
        // The last segment alone is too short; with its predecessor, 1,600
        // bytes over 7 s, above the second segment's 1,600 bits per second.
        assert_eq!(peak, 1_829);
        assert_eq!(average, (3_400 * 8_u64).div_ceil(19));
        // A rendition shorter than half the target: the average.
        let (peak, average) = bit_rates(&[1], &[100], 1, target).unwrap();
        assert_eq!((peak, average), (800, 800));
    }
}
