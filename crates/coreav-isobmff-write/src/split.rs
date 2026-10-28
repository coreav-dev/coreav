//! Splitting a file at keyframes into MP4 segments by stream copy (use case
//! 7, `docs/design/split.md`), with the clip writer's selection, tables,
//! file head and copy pass (`crate::clip`).
//!
//! The segments partition the kept tracks: every video and audio sample of
//! the source is copied into exactly one segment, and every frame the source
//! presents is presented by exactly one segment, at its source time minus
//! the segment's start.

use crate::clip::{
    AudioPlan, Cut, CutAudio, Run, Scaled, Timeline, VideoPlan, WindowEdits, copy_cut,
    exact_timescale, gapless, select_audio, select_video, tables_of, video_sample_entry,
    write_cut_head,
};
use crate::copy::{COPY_BUFFER, Shared, Window};
use crate::cut::{CutPoints, latest_accepted};
use crate::m4a::{audio_sample_entry, unsupported};
use crate::moov::overflow;
use crate::rational::{add, ceil, int, lt, max, min, scale, sub};
use crate::{ExtractError, LeftOut, LeftOutReason, Segment, SplitAt, SplitOptions, SplitSummary};
use core::cell::RefCell;
use coreav_isobmff::{
    Allowlist, Budget, Error, LimitKind, Limits, Movie, Rational, Source, Track, probe,
};
use std::io::Write;

/// Bytes of table memory per clean cut point kept while the video is
/// listed, counted against `max_table_bytes`: its sample number and
/// presentation time, and the earliest presentation time before it.
const CANDIDATE_ENTRY: u64 = 40;

/// Bytes of memory per segment counted against `max_table_bytes`: its
/// video and audio plans, its audio boundary and the start of the audio
/// after it, and its summary, rounded up.
const SEGMENT_ENTRY: u64 = 320;

/// Splits `source` into keyframe-aligned MP4 segments, each written to the
/// writer that `open` returns for its index (0, 1, ...).
///
/// The source is probed with `limits` and `allowlist`; the video and audio
/// tracks are chosen and checked as for [`clip_mp4`](crate::clip_mp4). The
/// video is cut only at clean cut points (a sync sample that every sample
/// before it in decode order is presented before, and every sample after it
/// is presented after), chosen by `options.at`; each audio sample goes to
/// the segment in which its presentation starts. The first segment starts
/// with the source's first samples and the last ends with its last ones, so
/// the segments hold every sample of the kept tracks exactly once, each
/// segment's samples in the source's order.
///
/// The source is listed three times in all, as for one clip: once to find
/// the cut points with counters only, then once to build each segment's
/// tables and once to copy its samples, both listings going on from one
/// segment to the next. Memory holds one segment's tables, the cut points
/// and one 64 KiB copy window.
///
/// Fails before `open` is called when the file cannot be probed or split;
/// fails with the segments written so far when the source cannot be read
/// later on, changes between the listings, or a writer fails.
pub fn split_mp4<S: Source, W: Write, F: FnMut(usize) -> std::io::Result<W>>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &SplitOptions,
    mut open: F,
) -> Result<SplitSummary, ExtractError> {
    let mut source = source;
    let movie = probe(&mut source, limits, allowlist)?;
    let video = select_video(&movie, options.video)?;
    let video_entry = video_sample_entry(video)?;
    let selected_audio = select_audio(&movie, options.audio)?;
    let audio_entry = selected_audio.map(audio_sample_entry).transpose()?;
    check_targets(&options.at)?;
    let mut budget = Budget::new(limits);

    // Listing 1: the cut points and each segment's runs, counters only.
    let video_timeline = Timeline::with_several(video, &movie, None)?;
    let listed = list_video(
        &movie,
        video,
        &video_timeline,
        &mut source,
        limits,
        &mut budget,
    )?;
    // The cut points the coded data agrees with, read only for the sync
    // samples chosen (docs/design/cut-points.md).
    let rule = CutPoints::of(&movie, video);
    let cuts = {
        let mut listing = movie.packets(video.index, &mut source, limits, &mut budget)?;
        choose(
            &listed,
            &options.at,
            video.timescale,
            &mut |c: &Candidate| {
                if rule.reads() {
                    rule.allows_sync(&mut listing, c.sample, c.offset, c.size)
                } else {
                    Ok(true)
                }
            },
        )?
    };
    let mut plans = video_plans(&listed, &cuts, limits)?;
    // With several edits, a segment whose windows need a movie timescale
    // above 32 bits is refused here, while the source is listed, so that no
    // segment is opened first (docs/design/edit-list-writers.md, common
    // decision 1). The refusal depends on the video timescale and the
    // windows only, not on the audio's timescale or on the decode time the
    // media times are relative to.
    if video_timeline.has_windows() {
        for plan in &plans {
            exact_timescale(
                video.timescale,
                video.timescale,
                sub(plan.zero, plan.origin)?,
                WindowEdits::new(&video_timeline, plan.zero, plan.end, 0),
            )?;
        }
    }
    let audio = match selected_audio {
        Some(track) => {
            let gapless = gapless(&movie, track, &mut source)?;
            let timeline = Timeline::with_several(track, &movie, gapless)?;
            let planned = plan_audio(
                &movie,
                track,
                timeline,
                &mut plans,
                video.timescale,
                &mut source,
                limits,
                &mut budget,
            )?;
            planned.map(|(plans, timeline)| (track, plans, timeline))
        }
        None => None,
    };

    // Listings 2 and 3 visit the same samples as listing 1, so a source
    // whose three listings would exceed the work limit is refused here,
    // before any segment is opened.
    if budget.used().saturating_mul(3) > limits.max_sample_work {
        return Err(Error::LimitExceeded {
            limit: LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }

    // Listings 2 and 3, each going on from one segment to the next.
    let shared = RefCell::new(source);
    let remaining = limits.max_sample_work.saturating_sub(budget.used());
    let listing_limits = Limits {
        max_sample_work: remaining,
        ..*limits
    };
    let mut budgets = [Budget::new(&listing_limits); 4];
    let [
        video_table_budget,
        audio_table_budget,
        video_copy_budget,
        audio_copy_budget,
    ] = &mut budgets;
    let mut video_tables = movie.packets(
        video.index,
        Shared(&shared),
        &listing_limits,
        video_table_budget,
    )?;
    let mut video_copy = movie.packets(
        video.index,
        Shared(&shared),
        &listing_limits,
        video_copy_budget,
    )?;
    let mut audio_listings = match &audio {
        Some((track, _, _)) => Some((
            movie.packets(
                track.index,
                Shared(&shared),
                &listing_limits,
                audio_table_budget,
            )?,
            movie.packets(
                track.index,
                Shared(&shared),
                &listing_limits,
                audio_copy_budget,
            )?,
        )),
        None => None,
    };
    let mut window = Window::new(COPY_BUFFER);
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(plans.len())
        .map_err(|_| table_limit())?;
    let tv = video.timescale;
    let seconds = |ticks: Rational| scale(ticks, 1, tv);
    for (index, plan) in plans.iter().enumerate() {
        let video_built = tables_of(
            &mut video_tables,
            video,
            plan.first,
            plan.last,
            true,
            plan.marked.then_some(&rule),
            limits,
        )?;
        let audio_plan = audio
            .as_ref()
            .and_then(|(track, plans, timeline)| plans.get(index).map(|p| (*track, p, timeline)))
            .and_then(|(track, p, timeline)| p.as_ref().map(|p| (track, p, timeline)));
        let cut_audio = match (audio_plan, audio_listings.as_mut(), &audio_entry) {
            (Some((track, plan, timeline)), Some((tables, _)), Some(entry)) => Some(CutAudio {
                track,
                entry: entry.clone(),
                // The shared timeline, alive for this segment only.
                plan: AudioPlan {
                    timeline: timeline.clone(),
                    ..plan.clone()
                },
                tables: tables_of(tables, track, plan.first, plan.last, false, None, limits)?,
                // Without an edit list, ffprobe 7.0.2 reports an audio track
                // shorter than the movie at the movie's length.
                keep_identity: true,
            }),
            _ => None,
        };
        let cut = Cut {
            movie: &movie,
            video,
            video_entry: video_entry.clone(),
            video_timeline: video_timeline.clone(),
            plan,
            video_tables: video_built,
            audio: cut_audio,
        };
        let mut out = open(index)?;
        let mut cut = cut;
        let head = write_cut_head(&mut cut, &mut out)?;
        let video_run = Run::new(&mut video_copy, 0, &cut.video_tables.0)?;
        let audio_run = match (&cut.audio, audio_listings.as_mut()) {
            (Some(a), Some((_, copy))) => Some(Run::new(copy, 0, &a.tables.0)?),
            _ => None,
        };
        copy_cut(&shared, video_run, audio_run, &mut window, &mut out)?;
        out.flush()?;
        drop(out);
        segments.push(Segment {
            start: seconds(plan.origin)?,
            video_start: seconds(plan.zero)?,
            // The video's edits present `head.presented` ticks from the
            // segment's video start: with several edits, the exact sum of
            // its windows' parts, which need not be a whole video tick.
            end: seconds(add(plan.zero, head.presented)?)?,
            video: head.video,
            audio: head.audio,
            bytes_written: head.data_start.saturating_add(head.data_bytes),
        });
    }
    drop((video_tables, video_copy, audio_listings, window));
    let used = budgets
        .iter()
        .fold(0u64, |sum, b| sum.saturating_add(b.used()));
    if used > remaining {
        return Err(Error::LimitExceeded {
            limit: LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }

    let mut left_out = Vec::new();
    for track in &movie.tracks {
        if track.index == video.index {
            continue;
        }
        if selected_audio.is_some_and(|a| a.index == track.index) {
            if audio.is_none() {
                left_out.push(LeftOut {
                    stream_index: track.stream_index,
                    track_id: track.track_id,
                    reason: LeftOutReason::NoSamplesInClip,
                });
            }
            continue;
        }
        left_out.push(LeftOut {
            stream_index: track.stream_index,
            track_id: track.track_id,
            reason: LeftOutReason::NotSelected,
        });
    }
    for dropped in &movie.dropped {
        left_out.push(LeftOut {
            stream_index: dropped.stream_index,
            track_id: dropped.track_id,
            reason: LeftOutReason::CodecNotAllowed,
        });
    }
    left_out.sort_by_key(|l| l.stream_index);
    Ok(SplitSummary {
        segments,
        left_out,
        boxes_left_out: video.description.other_boxes.clone(),
    })
}

fn check_targets(at: &SplitAt) -> Result<(), Error> {
    match at {
        SplitAt::Every(step) => {
            if step.num <= 0 {
                return Err(unsupported("a split interval of 0 or less"));
            }
        }
        SplitAt::Times(times) => {
            let mut previous: Option<Rational> = None;
            for &time in times {
                if time.num < 0 {
                    return Err(unsupported("a negative split time"));
                }
                if let Some(previous) = previous {
                    if !lt(previous, time)? {
                        return Err(unsupported("split times that do not increase"));
                    }
                }
                previous = Some(time);
            }
        }
    }
    Ok(())
}

/// A clean cut point: a sync sample `sample` presented at `at`, after every
/// sample before it in decode order, the earliest of which is presented at
/// `first_before`. Presentation times are in units of the video's
/// [`Scaled`] timeline.
#[derive(Clone, Copy)]
struct Candidate {
    sample: u64,
    at: i64,
    first_before: i64,
    /// Where the sample is, to read its coded data if it is chosen
    /// (docs/design/cut-points.md).
    offset: u64,
    size: u32,
}

/// What listing 1 learns about the video.
struct Listed<'a> {
    /// The clean cut points after the first sample, by presentation time.
    candidates: Vec<Candidate>,
    /// Table memory charged for them.
    candidate_bytes: u64,
    /// Decode time of the first sample.
    first_dts: i64,
    timeline: Timeline<'a>,
    /// Samples in the track.
    samples: u64,
    /// The container marks every sample as a sync sample.
    marked: bool,
    /// The earliest presentation time of any sample, in scaled units.
    first_at: Option<i64>,
    /// Where the video's presentation starts and ends, in video ticks.
    start: Rational,
    end: Rational,
    scaled: Scaled<'a>,
}

/// Listing 1 for the video: every clean cut point. A sync sample becomes a
/// candidate when every earlier sample is presented before it; a later
/// sample presented at or before a candidate removes it. Candidates are
/// presented later than every earlier sample, so they are kept in
/// increasing order and a later sample removes a suffix of them.
fn list_video<'a, S: Source>(
    movie: &Movie,
    track: &Track,
    timeline: &Timeline<'a>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Listed<'a>, Error> {
    let scaled = Scaled::of(timeline)?;
    let windows = timeline.has_windows();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut table_bytes = 0u64;
    let mut latest: Option<i64> = None;
    let mut earliest: Option<i64> = None;
    let mut presented_end: Option<i64> = None;
    let mut samples = 0u64;
    let mut marked = true;
    let mut next_dts: Option<i64> = None;
    let mut first_dts = 0;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        marked &= packet.keyframe;
        // The refusals of the table listing, made here so that nothing is
        // written before them: the output stores decode durations only.
        if next_dts.is_some_and(|next| next != packet.dts) {
            return Err(unsupported("a gap or an overlap between decode times"));
        }
        next_dts = Some(
            packet
                .dts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        if samples == 0 {
            if packet.cts < packet.dts {
                return Err(unsupported(
                    "the first sync sample is presented before it is decoded",
                ));
            }
            first_dts = packet.dts;
        }
        samples = samples.saturating_add(1);
        // Where the sample's media maps, and when it is first shown: with
        // several edits, a sample whose media starts in a gap and reaches
        // into the next window is shown from that window's start, later
        // than the mapping places it (docs/design/edit-list-writers.md,
        // split decision 1).
        let at = scaled.present(packet.cts)?;
        let p = if windows {
            at.max(scaled.owner_start(packet.cts)?)
        } else {
            at
        };
        // Only presented samples order the cut points: a clean cut point is
        // a sync sample every presented sample before it in decode order is
        // shown before, and every one after it after. A sample a gap skips
        // is shown by no window, and the time the mapping gives it can lie
        // before samples decoded earlier.
        let presented = scaled.presents(packet.cts, packet.duration)?;
        if presented {
            while candidates.last().is_some_and(|c| c.at >= p) {
                candidates.pop();
            }
        }
        // A segment's video edit starts at its first sample's composition
        // time, so that sample must not be presented before it is decoded
        // (as for a clip's first sample).
        if presented
            && packet.keyframe
            && packet.sample > 0
            && packet.cts >= packet.dts
            && latest.is_some_and(|l| l < p)
        {
            table_bytes = table_bytes.saturating_add(CANDIDATE_ENTRY);
            if table_bytes > limits.max_table_bytes {
                return Err(table_limit());
            }
            candidates.try_reserve(1).map_err(|_| table_limit())?;
            candidates.push(Candidate {
                sample: packet.sample,
                at: p,
                first_before: earliest.unwrap_or(p),
                offset: packet.offset,
                size: packet.size,
            });
        }
        if presented {
            latest = Some(latest.map_or(p, |l| l.max(p)));
            earliest = Some(earliest.map_or(p, |e| e.min(p)));
            // It stops being shown where its first showing ends: a
            // sample the window that resumes shows again is shown there
            // only as far as the segment's own range reaches (split
            // decision 5).
            let p_end = at
                .checked_add(scaled.span(packet.duration)?)
                .ok_or_else(overflow)?;
            presented_end = Some(presented_end.map_or(p_end, |e| e.max(p_end)));
        }
    }
    // The video ends where the edit, the stream or its last
    // presented sample does, whichever is first, as a clip to the end does.
    let (start, window_end) = timeline.window()?;
    let mut end = window_end;
    // With several edits, the windows' end above is where the stream ends
    // (docs/design/edit-lists.md, decision 5).
    if let (Some(duration), false) = (track.duration, timeline.has_windows()) {
        let stream_end = add(start, duration)?;
        end = Some(end.map_or(Ok(stream_end), |e| min(e, stream_end))?);
    }
    let presented_end = presented_end.ok_or(unsupported("no video sample is presented"))?;
    let presented_end = scaled.ticks(presented_end)?;
    let mut end = end.map_or(Ok(presented_end), |e| min(e, presented_end))?;
    // An end between two windows: the last segment ends where the earlier
    // one does, so that its edits reach its end (clip decision 4).
    if timeline.has_windows() {
        end = timeline.presented_until(end)?;
    }
    Ok(Listed {
        candidates,
        candidate_bytes: table_bytes,
        first_dts,
        timeline: timeline.clone(),
        samples,
        marked,
        first_at: earliest,
        start,
        end,
        scaled,
    })
}

/// The chosen cut points, by increasing presentation time: for each target
/// time, the latest candidate presented at or before it, strictly inside the
/// video's presentation, that `verify` accepts (the coded data agrees, docs/
/// design/cut-points.md). `verify` is called only for candidates that a
/// target reaches, at most once each, so only the frames at the cuts are
/// read. The work is bounded by the candidates, which `max_table_bytes`
/// bounds, not by the number of targets: a target that reaches no candidate
/// not yet decided is skipped.
fn choose(
    listed: &Listed,
    at: &SplitAt,
    timescale: u32,
    verify: &mut dyn FnMut(&Candidate) -> Result<bool, Error>,
) -> Result<Vec<Candidate>, Error> {
    let scaled = &listed.scaled;
    let start = match listed.first_at {
        Some(first) => max(scaled.ticks(first)?, listed.start)?,
        None => listed.start,
    };
    // Candidates strictly inside the video's presentation.
    let start_units = scaled.floor(start)?;
    let end_units = scaled.ceil(listed.end)?;
    let mut usable: Vec<Candidate> = Vec::new();
    usable
        .try_reserve_exact(listed.candidates.len())
        .map_err(|_| table_limit())?;
    usable.extend(
        listed
            .candidates
            .iter()
            .filter(|c| c.at > start_units && c.at < end_units)
            .copied(),
    );
    let mut chosen: Vec<Candidate> = Vec::new();
    // Candidates below this index are decided: chosen, before a chosen one,
    // or refused. Targets increase, so a later one never needs them again.
    let mut decided = 0usize;
    // Chooses the latest accepted candidate presented at or before `limit`
    // (in scaled units) among those not decided; returns the index of the
    // first candidate presented after `limit`.
    let mut reach = |limit: i64, chosen: &mut Vec<Candidate>| -> Result<usize, Error> {
        let end = usable.partition_point(|c| c.at <= limit);
        let found = latest_accepted(decided, end, &mut |index| match usable.get(index) {
            Some(candidate) => verify(candidate),
            None => Ok(false),
        })?;
        if let Some(candidate) = found.and_then(|i| usable.get(i)) {
            chosen.push(*candidate);
        }
        decided = decided.max(end);
        Ok(end)
    };
    match at {
        SplitAt::Times(times) => {
            for &time in times {
                // Presented at or before `time`: at most its floor in units.
                let limit = scaled.floor(scale(time, timescale, 1)?)?;
                reach(limit, &mut chosen)?;
            }
        }
        SplitAt::Every(step) => {
            // A cut at the latest accepted candidate presented at or before
            // each multiple k * step (k >= 1) inside the video's
            // presentation.
            let step_ticks = scale(*step, timescale, 1)?;
            let step_num = u64::try_from(step_ticks.num).map_err(|_| overflow())?;
            let mut k: i128 = 1;
            loop {
                let multiple = Rational::new(
                    k.checked_mul(step_ticks.num).ok_or_else(overflow)?,
                    step_ticks.den,
                )
                .ok_or_else(overflow)?;
                if !lt(multiple, listed.end)? {
                    break;
                }
                let next = reach(scaled.floor(multiple)?, &mut chosen)?;
                // The next multiple that reaches the first candidate after
                // this one: the multiples between reach none not decided.
                let Some(candidate) = usable.get(next) else {
                    break;
                };
                let at = scaled.ticks(candidate.at)?;
                let steps = Rational::new(
                    at.num
                        .checked_mul(i128::from(step_ticks.den))
                        .ok_or_else(overflow)?,
                    at.den.checked_mul(step_num).ok_or_else(overflow)?,
                )
                .ok_or_else(overflow)?;
                k = k.checked_add(1).ok_or_else(overflow)?.max(ceil(steps)?);
            }
        }
    }
    Ok(chosen)
}

/// The video run and presentation of every segment.
fn video_plans(
    listed: &Listed,
    cuts: &[Candidate],
    limits: &Limits,
) -> Result<Vec<VideoPlan>, Error> {
    let scaled = &listed.scaled;
    let count = cuts.len().saturating_add(1);
    // Every segment's plans and summary count against the table limit,
    // with the cut points (docs/design/split.md, "Memory and work").
    let segment_bytes = u64::try_from(count)
        .map_err(|_| overflow())?
        .saturating_mul(SEGMENT_ENTRY);
    if listed.candidate_bytes.saturating_add(segment_bytes) > limits.max_table_bytes {
        return Err(table_limit());
    }
    let mut plans = Vec::new();
    plans.try_reserve_exact(count).map_err(|_| table_limit())?;
    let last_sample = listed
        .samples
        .checked_sub(1)
        .ok_or(unsupported("no video sample"))?;
    // With several edits, a cut that falls on a window's start comes after
    // the gap the previous window left: nothing is presented there, so the
    // segment before it ends where its last window does and the segment
    // after it opens with an empty edit for the gap. The segments then
    // still tile the source's presentation (docs/design/edit-list-writers.md,
    // split decision 3).
    let until = |at: i64| -> Result<Rational, Error> {
        let p = scaled.ticks(at)?;
        if listed.timeline.has_windows() {
            listed.timeline.presented_until(p)
        } else {
            Ok(p)
        }
    };
    for index in 0..count {
        let (first, zero, origin) = match index.checked_sub(1).and_then(|i| cuts.get(i)) {
            Some(c) => (c.sample, scaled.ticks(c.at)?, until(c.at)?),
            None => {
                // The first segment starts with the source's first sample,
                // presented from the video's start.
                let earliest = match cuts.first() {
                    Some(c) => c.first_before,
                    None => listed.first_at.ok_or(unsupported("no video sample"))?,
                };
                let zero = max(scaled.ticks(earliest)?, listed.start)?;
                (0, zero, zero)
            }
        };
        let (last, end) = match cuts.get(index) {
            Some(next) => (
                next.sample.checked_sub(1).ok_or_else(overflow)?,
                until(next.at)?,
            ),
            None => (last_sample, listed.end),
        };
        if !lt(zero, end)? {
            return Err(unsupported("no video sample is presented"));
        }
        if index == 0 {
            // The first segment's edit starts at the media time its video
            // starts, which must not lie before its first decode time (as
            // the file head requires; checked here, before any output).
            let media = sub(listed.timeline.media(zero)?, int(listed.first_dts))?;
            if media.num < 0 {
                return Err(unsupported(
                    "the first sync sample is presented before it is decoded",
                ));
            }
        }
        plans.push(VideoPlan {
            first,
            last,
            origin,
            zero,
            end,
            keeps_last: index.saturating_add(1) == count,
            marked: listed.marked,
            // The segments tile the source's presentation, so a segment's
            // edits end where the next one starts, exactly.
            exact_end: true,
        });
    }
    Ok(plans)
}

/// What [`plan_audio`] found: one plan per segment, and the timeline the
/// segment being written borrows its windows from (split decision 7).
type PlannedAudio<'a> = (Vec<Option<AudioPlan<'a>>>, Timeline<'a>);

/// One segment's audio while it is listed.
#[derive(Clone, Copy)]
struct Owned {
    first: u64,
    last: u64,
    /// Presentation of its presented samples, in scaled audio units.
    start: Option<i64>,
    end: Option<i64>,
}

/// Listing 1 for the audio: each sample goes to the segment in which its
/// presentation starts. Samples the source does not present go with their
/// presented neighbours: those before the edit with the segment where the
/// edit starts, those after it with the last segment that has audio.
/// Moves the first segment's origin back when its audio starts before its
/// video. `None` when the source presents no audio at all.
#[allow(clippy::too_many_arguments)]
fn plan_audio<'a, S: Source>(
    movie: &Movie,
    track: &Track,
    timeline: Timeline<'a>,
    plans: &mut [VideoPlan],
    video_timescale: u32,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Option<PlannedAudio<'a>>, Error> {
    let ta = track.timescale;
    let scaled = Scaled::of(&timeline)?;
    let windows = timeline.has_windows();
    let (window_start, window_end) = timeline.window()?;
    let window_start_units = scaled.ceil(window_start)?;
    // The boundary before segment i (i >= 1): where the segment starts, in
    // units. That is its video's start, except where its cut falls after a
    // gap: the gap belongs to the segment, which pads its video with an
    // empty edit for it (split decision 3), so audio presented in the gap
    // belongs to it too. Taking the video's start instead would leave that
    // audio in the segment before while the segment after it still counts
    // the gap, and the two together would last longer than the source.
    let mut boundaries = Vec::new();
    boundaries
        .try_reserve_exact(plans.len())
        .map_err(|_| table_limit())?;
    for plan in plans.iter().skip(1) {
        boundaries.push(scaled.ceil(scale(plan.origin, ta, video_timescale)?)?);
    }
    let mut owned: Vec<Option<Owned>> = Vec::new();
    owned
        .try_reserve_exact(plans.len())
        .map_err(|_| table_limit())?;
    owned.resize(plans.len(), None);
    let mut segment = 0usize;
    let mut presented_any = false;
    let mut next_dts: Option<i64> = None;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        if packet.cts != packet.dts {
            return Err(unsupported("composition offsets in an audio track"));
        }
        if next_dts.is_some_and(|next| next != packet.dts) {
            return Err(unsupported("a gap or an overlap between decode times"));
        }
        next_dts = Some(
            packet
                .dts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        // As for the video: where the sample's media maps, and when it is
        // first shown (split decision 1).
        let dts = scaled.present(packet.dts)?;
        let q = if windows {
            dts.max(scaled.owner_start(packet.dts)?)
        } else {
            dts
        };
        let presented = scaled.presents(packet.dts, packet.duration)?;
        // Samples after the edit stay with the segment of the last
        // presented one.
        if presented || !presented_any {
            let at = q.max(window_start_units);
            while boundaries.get(segment).is_some_and(|b| at >= *b) {
                segment = segment.saturating_add(1);
            }
        }
        let slot = owned.get_mut(segment).ok_or_else(overflow)?;
        let entry = slot.get_or_insert(Owned {
            first: packet.sample,
            last: packet.sample,
            start: None,
            end: None,
        });
        entry.last = packet.sample;
        if presented {
            presented_any = true;
            // To the end of its first showing (split decision 5).
            let q_end = dts
                .checked_add(scaled.span(packet.duration)?)
                .ok_or_else(overflow)?;
            entry.start.get_or_insert(q);
            entry.end = Some(entry.end.map_or(q_end, |e| e.max(q_end)));
        }
    }
    if !presented_any {
        return Ok(None);
    }
    let last_owner = owned.iter().rposition(Option::is_some);
    // Where the audio of the first segment after each one starts, in
    // scaled units: one backward pass, so the whole plan stays linear in
    // the number of segments. Only with several edits, where a segment's
    // audio can reach past the next segment's first window; without them
    // the two meet exactly and nothing is capped.
    let mut next_start: Vec<Option<i64>> = Vec::new();
    if timeline.has_windows() {
        next_start
            .try_reserve_exact(plans.len())
            .map_err(|_| table_limit())?;
        next_start.resize(plans.len(), None);
        let mut following: Option<i64> = None;
        for (slot, entry) in next_start.iter_mut().zip(&owned).rev() {
            *slot = following;
            if let Some(start) = entry.and_then(|e| e.start) {
                following = Some(start);
            }
        }
    }
    let mut audio_plans = Vec::new();
    audio_plans
        .try_reserve_exact(plans.len())
        .map_err(|_| table_limit())?;
    for (index, (plan, entry)) in plans.iter_mut().zip(&owned).enumerate() {
        let Some(entry) = entry else {
            audio_plans.push(None);
            continue;
        };
        let (Some(start), Some(end)) = (entry.start, entry.end) else {
            return Err(unsupported("audio samples that no segment presents"));
        };
        let mut start = max(scaled.ticks(start)?, window_start)?;
        let mut end = scaled.ticks(end)?;
        if let Some(window_end) = window_end {
            end = min(end, window_end)?;
        }
        // Never past the next segment's audio: a sample that straddles a
        // window's end is presented past it, and the windows the next
        // segment presents start there (docs/design/edit-list-writers.md,
        // split decision 5). Without several edits the two meet exactly,
        // the samples being contiguous.
        if let Some(next) = next_start.get(index).copied().flatten() {
            end = min(end, scaled.ticks(next)?)?;
        }
        if index == 0 {
            // The first segment starts where its first track does. Its video
            // then starts after an empty edit of whole video ticks, so that
            // the video stays exact; the audio, rounded to whole audio ticks
            // anyway, starts less than a video tick after the origin.
            let start_v = scale(start, video_timescale, ta)?;
            if lt(start_v, plan.origin)? {
                let lead = ceil(sub(plan.origin, start_v)?)?;
                plan.origin = sub(plan.origin, Rational::from_int(lead))?;
            }
        }
        let clip_start = scale(plan.origin, ta, video_timescale)?;
        start = max(start, clip_start)?;
        if !lt(start, end)? {
            return Err(unsupported("audio samples that no segment presents"));
        }
        // The windows are not kept per segment: a plan holds a released
        // timeline, and `split_mp4` puts the one shared timeline into the
        // segment it is writing, so only that one is alive at a time
        // (split decision 7). A source's edit count would otherwise add a
        // checkpoint table to every segment.
        let mut released = timeline.clone();
        released.release();
        audio_plans.push(Some(AudioPlan {
            first: entry.first,
            last: entry.last,
            clip_start,
            start,
            end,
            timeline: released,
            keeps_last: Some(index) == last_owner,
        }));
    }
    Ok(Some((audio_plans, timeline)))
}

fn table_limit() -> Error {
    Error::LimitExceeded {
        limit: LimitKind::TableBytes,
        offset: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioChoice, VideoChoice};

    fn q(num: i128, den: u64) -> Rational {
        Rational::new(num, den).unwrap_or(Rational::from_int(0))
    }

    #[test]
    fn targets_are_checked() {
        let every = |r| SplitOptions {
            video: VideoChoice::First,
            audio: AudioChoice::None,
            at: SplitAt::Every(r),
        };
        assert!(check_targets(&every(q(0, 1)).at).is_err());
        assert!(check_targets(&every(q(1, 10)).at).is_ok());
        assert!(check_targets(&SplitAt::Times(vec![q(1, 1), q(1, 1)])).is_err());
        assert!(check_targets(&SplitAt::Times(vec![q(-1, 1)])).is_err());
        assert!(check_targets(&SplitAt::Times(vec![q(0, 1), q(1, 2)])).is_ok());
    }
}
