//! Keyframe-aligned clips to MP4 by stream copy, written from
//! ISO/IEC 14496-12 (boxes, edit lists, composition offsets), ISO/IEC
//! 14496-14 (MP4), ISO/IEC 14496-15 (AVC and HEVC sample entries), the AV1
//! and VP codec ISO media file format bindings, and ISO/IEC 14496-3 (AAC).

use crate::boxes::Node;
use crate::copy::{COPY_BUFFER, Shared, Window};
use crate::cut::{CutPoints, latest_accepted};
use crate::m4a::{audio_sample_entry, unsupported};
use crate::moov::{
    EditEntry, Media, MovieBox, Ties, TrackBox, TrackFlags, UNIT_RATE, ftyp, interleave, language,
    overflow, rescale, write_head,
};
use crate::rational::{add, ceil, int, lt, max, min, round, scale, sub, to_i64, to_u64};
use crate::tables::Tables;
use crate::{
    AudioChoice, AudioClipOptions, AudioClipSummary, ClipOptions, ClipSummary, ClipTrack, LeftOut,
    LeftOutReason, TrackChoice,
};
use crate::{ExtractError, VideoChoice};
use core::cell::{Cell, RefCell};
use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Error, ItunesGapless, Limits, Movie, Packets, Rational,
    Source, Track, TrackKind, probe,
};
use std::collections::VecDeque;
use std::io::Write;

/// Output track IDs: the video is track 1, the audio track 2; audio alone
/// is track 1.
const VIDEO_ID: u32 = 1;
const AUDIO_ID: u32 = 2;
const AUDIO_ALONE_ID: u32 = 1;

/// Cuts a keyframe-aligned clip of `source` into an MP4 file written to
/// `out`.
///
/// The source is probed with `limits` and `allowlist`. The clip keeps one
/// video track (H.264 `avc1`, HEVC `hvc1`, AV1 or VP9) and at most one AAC
/// track, chosen by `options`. It starts at the latest video sync sample
/// presented at or before `options.start` that loses no frame of the range,
/// and ends at `options.end` exactly, through edit lists. The
/// source is listed three times: once to find the start and the end with
/// counters only, once to build the output tables from the start, and once
/// to copy the samples, so `out` receives `ftyp`, `moov` and `mdat` in that
/// order without seeking. `out` gets many small writes; give it a buffered
/// writer.
///
/// Fails before writing anything when the file cannot be probed or clipped
/// (an unsupported codec, sample entry, track or edit list); fails with a
/// partial output when the source cannot be
/// read later on, changes between the listings, or the writer fails.
pub fn clip_mp4<S: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &ClipOptions,
    mut out: W,
) -> Result<ClipSummary, ExtractError> {
    let mut source = source;
    let movie = probe(&mut source, limits, allowlist)?;
    let video = select_video(&movie, options.video)?;
    let video_entry = video_sample_entry(video)?;
    let selected_audio = select_audio(&movie, options.audio)?;
    let audio_entry = selected_audio.map(audio_sample_entry).transpose()?;
    let start = nonnegative(options.start)?;
    let end = options.end.map(nonnegative).transpose()?;
    if let Some(end) = end {
        if !lt(start, end)? {
            return Err(unsupported("the clip ends at or before its start").into());
        }
    }
    let mut budget = Budget::new(limits);

    // Listing 1: where the clip starts and ends, with counters only.
    let video_timeline = Timeline::with_several(video, &movie, None)?;
    let start_v = scale(start, video.timescale, 1)?;
    let end_v = end.map(|e| scale(e, video.timescale, 1)).transpose()?;
    let plan = plan_video(
        &movie,
        video,
        &video_timeline,
        start_v,
        end_v,
        &mut source,
        limits,
        &mut budget,
    )?;
    let audio_plan = match selected_audio {
        Some(track) => {
            let gapless = gapless(&movie, track, &mut source)?;
            let timeline = Timeline::with_several(track, &movie, gapless)?;
            let ta = track.timescale;
            plan_audio(
                &movie,
                track,
                timeline,
                scale(plan.origin, ta, video.timescale)?,
                Some(scale(plan.end, ta, video.timescale)?),
                &mut source,
                limits,
                &mut budget,
            )?
        }
        None => None,
    };

    // Listing 2: the output tables of the kept runs.
    let video_tables = build_tables(
        &movie,
        video,
        plan.first,
        plan.last,
        true,
        plan.marked.then(|| CutPoints::of(&movie, video)).as_ref(),
        &mut source,
        limits,
        &mut budget,
    )?;
    let audio_planned = audio_plan.is_some();
    let audio = match (audio_plan, selected_audio, audio_entry) {
        (Some(plan), Some(track), Some(entry)) => Some(CutAudio {
            track,
            entry,
            tables: build_tables(
                &movie,
                track,
                plan.first,
                plan.last,
                false,
                None,
                &mut source,
                limits,
                &mut budget,
            )?,
            plan,
            keep_identity: false,
        }),
        _ => None,
    };
    let cut = Cut {
        movie: &movie,
        video,
        video_entry,
        video_timeline,
        plan: &plan,
        video_tables,
        audio,
    };
    let mut cut = cut;
    let head = write_cut_head(&mut cut, &mut out)?;

    // Listing 3: the samples, in the order of the chunks in `mdat`.
    let shared = RefCell::new(source);
    let remaining = limits.max_sample_work.saturating_sub(budget.used());
    let copy_limits = Limits {
        max_sample_work: remaining,
        ..*limits
    };
    let mut video_budget = Budget::new(&copy_limits);
    let mut audio_budget = Budget::new(&copy_limits);
    // The runs and the layout are kept on the stack: the copy pass
    // allocates nothing beyond its buffer.
    let mut video_packets = movie.packets(
        video.index,
        Shared(&shared),
        &copy_limits,
        &mut video_budget,
    )?;
    let mut audio_packets = match &cut.audio {
        Some(a) => Some(movie.packets(
            a.track.index,
            Shared(&shared),
            &copy_limits,
            &mut audio_budget,
        )?),
        None => None,
    };
    let video_run = Run::new(&mut video_packets, plan.first, &cut.video_tables.0)?;
    let audio_run = match (&cut.audio, audio_packets.as_mut()) {
        (Some(a), Some(packets)) => Some(Run::new(packets, a.plan.first, &a.tables.0)?),
        _ => None,
    };
    let mut window = Window::new(COPY_BUFFER);
    copy_cut(&shared, video_run, audio_run, &mut window, &mut out)?;
    // Only the tables stay allocated from here on: the summary below is
    // built after the copy buffer is freed, so the clip's memory stays
    // bounded by its output tables and the one read window.
    drop((video_packets, audio_packets, window));
    if video_budget.used().saturating_add(audio_budget.used()) > remaining {
        return Err(Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }
    out.flush()?;

    let mut left_out = Vec::new();
    for track in &movie.tracks {
        if track.index == video.index || selected_audio.is_some_and(|a| a.index == track.index) {
            if !audio_planned && selected_audio.is_some_and(|a| a.index == track.index) {
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
    let tv = video.timescale;
    let seconds = |ticks: Rational| scale(ticks, 1, tv);
    Ok(ClipSummary {
        video: head.video,
        audio: head.audio,
        start,
        snapped_start: seconds(plan.zero)?,
        end,
        written_end: seconds(add(plan.zero, head.presented)?)?,
        bytes_written: head.data_start.saturating_add(head.data_bytes),
        left_out,
        boxes_left_out: video.description.other_boxes.clone(),
    })
}

/// Cuts a clip of one AAC track of `source`, a file without video or one
/// whose video is not wanted, into an MP4 file written to `out`.
///
/// The clip presents `options.start` to `options.end` (the end of the
/// track without one) exactly, through an edit list in the audio's media
/// timescale, which is also the movie timescale: it keeps the samples
/// presented in that range and one sample of pre-roll before them, as
/// [`clip_mp4`] keeps its audio. Its time 0 is `options.start`. The source
/// is listed three times, as for [`clip_mp4`], so `out` receives `ftyp`,
/// `moov` and `mdat` in that order without seeking; give it a buffered
/// writer.
///
/// Fails before writing anything when the file cannot be probed or clipped
/// (not AAC, an unsupported sample entry or edit list, no audio presented
/// in the range); fails with a partial output when the source cannot be
/// read later on, changes between the listings, or the writer fails.
pub fn clip_audio_mp4<S: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &AudioClipOptions,
    mut out: W,
) -> Result<AudioClipSummary, ExtractError> {
    let mut source = source;
    let movie = probe(&mut source, limits, allowlist)?;
    let choice = match options.audio {
        TrackChoice::FirstAudio => AudioChoice::First,
        TrackChoice::Stream(index) => AudioChoice::Stream(index),
    };
    let track =
        select_audio(&movie, choice)?.ok_or_else(|| no_allowed_track(&movie, TrackKind::Audio))?;
    let entry = audio_sample_entry(track)?;
    let start = nonnegative(options.start)?;
    let end = options.end.map(nonnegative).transpose()?;
    if let Some(end) = end {
        if !lt(start, end)? {
            return Err(unsupported("the clip ends at or before its start").into());
        }
    }
    let mut budget = Budget::new(limits);
    let ta = track.timescale;

    // Listing 1: the samples presented in the range, with counters only.
    let gapless = gapless(&movie, track, &mut source)?;
    let timeline = Timeline::with_several(track, &movie, gapless)?;
    let Some(plan) = plan_audio(
        &movie,
        track,
        timeline,
        scale(start, ta, 1)?,
        end.map(|e| scale(e, ta, 1)).transpose()?,
        &mut source,
        limits,
        &mut budget,
    )?
    else {
        return Err(unsupported("the clip presents no audio").into());
    };

    // Listing 2: the output tables of the kept run.
    let tables = build_tables(
        &movie,
        track,
        plan.first,
        plan.last,
        false,
        None,
        &mut source,
        limits,
        &mut budget,
    )?;
    let first = plan.first;
    let mut audio = CutAudio {
        track,
        entry,
        plan,
        tables,
        keep_identity: false,
    };
    let (trak, kept) = audio_track(&movie, &mut audio, ta, AUDIO_ALONE_ID)?;
    let presented = trak.duration;
    let (data_start, data_bytes) = write_head(
        &mut out,
        ftyp(b"isom", &[b"isom", b"mp42"]),
        &MovieBox {
            timescale: ta,
            created: movie.creation_time,
            duration: presented,
            extra: Vec::new(),
        },
        vec![trak],
        Ties::TrackOrder,
    )?;

    // Listing 3: the samples.
    let shared = RefCell::new(source);
    let remaining = limits.max_sample_work.saturating_sub(budget.used());
    let copy_limits = Limits {
        max_sample_work: remaining,
        ..*limits
    };
    let mut copy_budget = Budget::new(&copy_limits);
    let mut packets =
        movie.packets(track.index, Shared(&shared), &copy_limits, &mut copy_budget)?;
    let run = Run::new(&mut packets, first, &audio.tables.0)?;
    let mut window = Window::new(COPY_BUFFER);
    copy_cut(&shared, run, None, &mut window, &mut out)?;
    drop((packets, window));
    if copy_budget.used() > remaining {
        return Err(Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }
    out.flush()?;

    let mut left_out: Vec<LeftOut> = movie
        .tracks
        .iter()
        .filter(|t| t.index != track.index)
        .map(|t| LeftOut {
            stream_index: t.stream_index,
            track_id: t.track_id,
            reason: LeftOutReason::NotSelected,
        })
        .chain(movie.dropped.iter().map(|d| LeftOut {
            stream_index: d.stream_index,
            track_id: d.track_id,
            reason: LeftOutReason::CodecNotAllowed,
        }))
        .collect();
    left_out.sort_by_key(|l| l.stream_index);
    Ok(AudioClipSummary {
        audio: kept,
        start,
        end,
        written_end: add(
            start,
            scale(
                int(i64::try_from(presented).map_err(|_| overflow())?),
                1,
                ta,
            )?,
        )?,
        bytes_written: data_start.saturating_add(data_bytes),
        left_out,
    })
}

/// A planned cut of one source: the kept runs, their output tables (from
/// listing 2) and where they are presented. Shared by clips, segments
/// (`crate::split`) and joins.
pub(crate) struct Cut<'a> {
    pub(crate) movie: &'a Movie,
    pub(crate) video: &'a Track,
    /// The `stsd` payload of the video track, moved into the head by
    /// [`write_cut_head`] (the copy needs only the tables: its memory is
    /// bounded by them and the window).
    pub(crate) video_entry: Vec<u8>,
    /// Its window table is released once the edits are computed
    /// ([`write_cut_head`]).
    pub(crate) video_timeline: Timeline<'a>,
    pub(crate) plan: &'a VideoPlan,
    /// The video tables, with the first sample's decode and composition
    /// times.
    pub(crate) video_tables: (Tables, i64, i64),
    pub(crate) audio: Option<CutAudio<'a>>,
}

/// The audio of a [`Cut`].
pub(crate) struct CutAudio<'a> {
    pub(crate) track: &'a Track,
    /// Its `stsd` payload, moved into the head as the video's is.
    pub(crate) entry: Vec<u8>,
    pub(crate) plan: AudioPlan<'a>,
    pub(crate) tables: (Tables, i64, i64),
    /// Write the audio edit also when it is the identity (`crate::split`);
    /// a clip leaves it out.
    pub(crate) keep_identity: bool,
}

/// What [`write_cut_head`] wrote.
pub(crate) struct Head {
    pub(crate) video: ClipTrack,
    pub(crate) audio: Option<ClipTrack>,
    /// How long the video's edits present it, in video ticks: the edit's
    /// length rounded up to a whole tick, or with several edits their
    /// exact sum, which ends where a window does when the output ends
    /// there (clip decision 4).
    pub(crate) presented: Rational,
    /// Offset of the first sample byte and the number of sample bytes.
    pub(crate) data_start: u64,
    pub(crate) data_bytes: u64,
}

/// Writes `ftyp`, `moov` and the `mdat` header of `cut`: the edits,
/// durations and track headers computed from the plans.
pub(crate) fn write_cut_head<W: Write>(
    cut: &mut Cut<'_>,
    out: &mut W,
) -> Result<Head, ExtractError> {
    let (movie, video, plan) = (cut.movie, cut.video, cut.plan);
    let (video_tables, first_dts, first_cts) = &cut.video_tables;
    let (first_dts, first_cts) = (*first_dts, *first_cts);
    let tv = video.timescale;
    // The movie timescale: a common multiple of the video's and the audio's
    // media timescales when it fits 32 bits, so that both edits are exact;
    // else the video's.
    let base = cut
        .audio
        .as_ref()
        .map_or(tv, |a| movie_timescale(tv, a.track.timescale));
    let segment = to_u64(ceil(sub(plan.end, plan.zero)?)?)?;
    // With several edits, the windows inside the clip, each a media edit,
    // and the movie timescale in which each lasts whole ticks
    // (docs/design/edit-list-writers.md, clip decisions 4 and 5).
    let several = if cut.video_timeline.has_windows() {
        let to = if plan.exact_end {
            plan.end
        } else {
            add(plan.zero, Rational::from_int(i128::from(segment)))?
        };
        Some(WindowEdits::new(
            &cut.video_timeline,
            plan.zero,
            to,
            first_dts,
        ))
    } else {
        None
    };
    let several_edits = several.is_some();
    // The empty edit before the video: the gap a segment's cut falls after
    // (split decision 3), or the lead the first segment's earlier audio
    // gives it; none for a clip.
    let lead = sub(plan.zero, plan.origin)?;
    let tm = match &several {
        Some(parts) => exact_timescale(base, tv, lead, parts.clone())?,
        None => base,
    };
    let per_video_tick = u64::from(tm / tv);
    // The media time presented at the clip's start, in the output's media
    // time: the snapped sync sample's composition offset when the source
    // presents all of it (a whole number of ticks either way).
    let media_time = to_i64(round(sub(
        cut.video_timeline.media(plan.zero)?,
        int(first_dts),
    )?))?;
    if media_time < 0 || first_cts < first_dts {
        return Err(unsupported("the first sync sample is presented before it is decoded").into());
    }
    // Never shorter than the edit: where frames are presented later than
    // their decode durations add up to (variable frame rate), the declared
    // duration covers the clip, as the source's does.
    let video_media = media_duration(movie, video, video_tables, plan.keeps_last)?.max(segment);
    // Always written, also when it looks like the identity: samples whose
    // composition offsets put them past the clip (unsigned offsets of a
    // version 0 table, say) are then still left unpresented, as the
    // source's edit leaves them.
    let segment_ticks = segment.checked_mul(per_video_tick).ok_or_else(overflow)?;
    let mut video_edits = Vec::new();
    // A segment whose audio starts before its video, or whose cut falls
    // after a gap (`crate::split`), starts the video with an empty edit; a
    // clip starts at its video. With several edits the movie timescale
    // makes it whole, as it makes every other edit whole.
    let scaled_lead = scale(lead, tm, tv)?;
    if several_edits && scaled_lead.den != 1 {
        return Err(overflow().into());
    }
    let video_empty = to_u64(round(scaled_lead))?;
    if video_empty > 0 {
        video_edits.push(EditEntry {
            duration: video_empty,
            media_time: -1,
            rate: UNIT_RATE,
        });
    }
    match several {
        Some(parts) => {
            for part in parts {
                let (duration, media) = part?;
                // Whole by the choice of `tm`.
                let exact = scale(duration, tm, tv)?;
                if exact.den != 1 {
                    return Err(overflow().into());
                }
                let ticks = to_u64(exact.num)?;
                video_edits.push(EditEntry {
                    duration: ticks,
                    media_time: match media {
                        Some(m) => to_i64(round(m))?,
                        None => -1,
                    },
                    rate: UNIT_RATE,
                });
            }
        }
        None => video_edits.push(EditEntry {
            duration: segment_ticks,
            media_time,
            rate: UNIT_RATE,
        }),
    }
    let video_duration = video_edits
        .iter()
        .try_fold(0u64, |sum, e| sum.checked_add(e.duration))
        .ok_or_else(overflow)?;
    // With several edits, the edits end where the clip's last window part
    // does, which need not be a whole video tick.
    let presented = if several_edits {
        let shown = video_duration
            .checked_sub(video_empty)
            .ok_or_else(overflow)?;
        scale(Rational::from_int(i128::from(shown)), tv, tm)?
    } else {
        Rational::from_int(i128::from(segment))
    };
    let mut tracks = vec![TrackBox {
        track_id: VIDEO_ID,
        header: TrackFlags::DEFAULT,
        media: Media::Video {
            matrix: video.matrix,
            size: video.presentation_size,
        },
        created: video.creation_time,
        media_created: video.media_creation_time,
        language: language(video.language),
        media_timescale: tv,
        media_duration: video_media,
        duration: video_duration,
        edits: video_edits.into(),
        stsd: std::mem::take(&mut cut.video_entry),
        tables: video_tables,
        extra: Vec::new(),
        stbl_extra: Vec::new(),
    }];
    let mut movie_duration = video_duration;
    let mut audio_summary = None;
    if let Some(audio) = &mut cut.audio {
        let (track, summary) = audio_track(movie, audio, tm, AUDIO_ID)?;
        movie_duration = movie_duration.max(track.duration);
        audio_summary = Some(summary);
        tracks.push(track);
    }
    // The window tables of several edits are not needed past the edits
    // above: released before the head is built and the samples are copied,
    // so that the clip's memory is the output's tables and the copy buffer,
    // whatever the source's edit count.
    cut.video_timeline.release();
    let brands: Vec<&[u8; 4]> = match video.description.codec {
        Codec::H264 => vec![b"isom", b"mp42", b"avc1"],
        Codec::Av1 => vec![b"isom", b"mp42", b"av01"],
        _ => vec![b"isom", b"mp42"],
    };
    let (data_start, data_bytes) = write_head(
        out,
        ftyp(b"isom", &brands),
        &MovieBox {
            timescale: tm,
            created: movie.creation_time,
            duration: movie_duration,
            extra: Vec::new(),
        },
        tracks,
        Ties::TrackOrder,
    )?;
    Ok(Head {
        video: ClipTrack {
            stream_index: video.stream_index,
            track_id: video.track_id,
            codec: video.description.codec,
            first_sample: plan.first,
            samples: video_tables.samples(),
            timescale: tv,
            duration: video_tables.duration,
        },
        audio: audio_summary,
        presented,
        data_start,
        data_bytes,
    })
}

/// The audio track of a cut in the movie timescale `tm`, as track
/// `track_id`: its edits present `[start, end)` of its plan from the
/// output's time 0, rounded to whole movie ticks (exact when `tm` is a
/// multiple of the audio's timescale), with the plan's pre-roll sample
/// before them. Returns the track and its summary.
pub(crate) fn audio_track<'t>(
    movie: &Movie,
    audio: &'t mut CutAudio<'_>,
    tm: u32,
    track_id: u32,
) -> Result<(TrackBox<'t>, ClipTrack), ExtractError> {
    let CutAudio {
        track,
        entry,
        plan: a,
        tables: (tables, a_first_dts, _),
        keep_identity,
    } = audio;
    let track = *track;
    let ta = track.timescale;
    let media_duration = media_duration(movie, track, tables, a.keeps_last)?;
    let several = a.timeline.has_windows();
    // The empty edit is rounded to the movie timescale (exact when it is a
    // common multiple of both timescales);
    // the media time is then that of the rounded start, so that audio
    // stays within half an audio tick of its source times.
    // Rounded up instead when rounding down would start before the
    // first kept sample. With several edits the exact media time
    // decides, not the rounded one: the edits below take the exact
    // one, and a start half a movie tick before the run's first
    // sample would give them a negative media time. A segment's audio
    // has no pre-roll (docs/design/split.md), so its run starts at the
    // first sample it presents and rounding down always dips below.
    let exact_empty = scale(sub(a.start, a.clip_start)?, tm, ta)?;
    let mut empty = 0;
    let mut start = a.start;
    let mut media_time = -1;
    let mut placed = false;
    for rounded in [round(exact_empty), ceil(exact_empty)?] {
        empty = to_u64(rounded)?;
        start = add(a.clip_start, scale(int(to_i64(rounded)?), ta, tm)?)?;
        let exact = sub(a.timeline.media(start)?, int(*a_first_dts))?;
        media_time = to_i64(round(exact))?;
        placed = if several {
            exact.num >= 0
        } else {
            media_time >= 0
        };
        if placed {
            break;
        }
    }
    if !placed {
        return Err(overflow().into());
    }
    let segment = to_u64(round(scale(sub(a.end, start)?, tm, ta)?))?;
    // Audio that overlaps the clip by less than half a movie tick would
    // get a media edit of duration 0, which presents nothing in a file
    // without fragments; the clip is refused rather than written so.
    // Only possible when the movie timescale is not a multiple of the
    // audio's (their least common multiple does not fit 32 bits).
    if segment == 0 {
        return Err(unsupported(
            "the audio overlaps the clip by less than one tick of the movie timescale",
        )
        .into());
    }
    let identity = !several
        && !*keep_identity
        && empty == 0
        && media_time == 0
        && segment == rescale(media_duration, ta, tm)?;
    let mut edits = Vec::new();
    if several {
        // The audio's windows inside the clip, each a media edit, rounded
        // to the nearest tick of the movie timescale, as a single audio
        // edit is.
        if empty > 0 {
            edits.push(EditEntry {
                duration: empty,
                media_time: -1,
                rate: UNIT_RATE,
            });
        }
        // Each edit ends at its exact end rounded, so that rounding does
        // not add up over many edits; an edit shorter than half a movie
        // tick is left out (a media edit of duration 0 is outside the
        // edit model).
        let mut exact_at = int(0);
        let mut rounded_at = 0u64;
        for part in WindowEdits::new(&a.timeline, start, a.end, *a_first_dts) {
            let (duration, media) = part?;
            exact_at = add(exact_at, duration)?;
            let boundary = to_u64(round(scale(exact_at, tm, ta)?))?;
            let ticks = boundary.checked_sub(rounded_at).ok_or_else(overflow)?;
            rounded_at = boundary;
            if ticks == 0 {
                continue;
            }
            edits.push(EditEntry {
                duration: ticks,
                media_time: match media {
                    Some(m) => to_i64(round(m))?,
                    None => -1,
                },
                rate: UNIT_RATE,
            });
        }
    } else if !identity {
        if empty > 0 {
            edits.push(EditEntry {
                duration: empty,
                media_time: -1,
                rate: UNIT_RATE,
            });
        }
        edits.push(EditEntry {
            duration: segment,
            media_time,
            rate: UNIT_RATE,
        });
    }
    a.timeline.release();
    let duration = if identity {
        rescale(media_duration, ta, tm)?
    } else if several {
        edits
            .iter()
            .try_fold(0u64, |sum, e| sum.checked_add(e.duration))
            .ok_or_else(overflow)?
    } else {
        empty.checked_add(segment).ok_or_else(overflow)?
    };
    let summary = ClipTrack {
        stream_index: track.stream_index,
        track_id: track.track_id,
        codec: track.description.codec,
        first_sample: a.first,
        samples: tables.samples(),
        timescale: ta,
        duration: tables.duration,
    };
    let track_box = TrackBox {
        track_id,
        media: Media::Audio,
        header: TrackFlags::DEFAULT,
        created: track.creation_time,
        media_created: track.media_creation_time,
        language: language(track.language),
        media_timescale: ta,
        media_duration,
        duration,
        edits: edits.into(),
        stsd: std::mem::take(entry),
        tables,
        extra: Vec::new(),
        stbl_extra: Vec::new(),
    };
    Ok((track_box, summary))
}

/// Listing 3 of a cut: copies the samples of the `first` track (the video,
/// or the audio of a clip without video) and the `second` in the order of
/// the chunks in `mdat`, through `window`. Fails with
/// [`ExtractError::SourceChanged`] when a run holds other samples than its
/// tables.
pub(crate) fn copy_cut<'p, 'a, S: Source, W: Write>(
    shared: &'a RefCell<S>,
    mut first: Run<'p, 'a, S>,
    mut second: Option<Run<'p, 'a, S>>,
    window: &mut Window,
    out: &mut W,
) -> Result<(), ExtractError> {
    let both;
    let one;
    let layout: &[&Tables] = match &second {
        Some(run) => {
            both = [first.tables, run.tables];
            &both
        }
        None => {
            one = [first.tables];
            &one
        }
    };
    // Tell the source where the copy starts and how many bytes it takes,
    // so that a remote source can read them ahead in large requests rather
    // than ramping up from small ones.
    let extents = [first.extent(), second.as_ref().and_then(Run::extent)];
    if let Some(start) = extents.iter().flatten().map(|(offset, _)| *offset).min() {
        let bytes = extents
            .iter()
            .flatten()
            .fold(0u64, |sum, (_, bytes)| sum.saturating_add(*bytes));
        if let Ok(mut source) = shared.try_borrow_mut() {
            source.will_read(start, bytes);
        }
    }
    interleave(
        layout,
        Ties::TrackOrder,
        |track, chunk| -> Result<(), ExtractError> {
            let run = match track {
                0 => &mut first,
                _ => second.as_mut().ok_or_else(overflow)?,
            };
            for _ in 0..chunk.samples {
                let (offset, size) = run.next()?;
                window.copy(&mut Shared(shared), offset, size, out)?;
            }
            Ok(())
        },
    )?;
    let complete = |run: &Run<'_, '_, S>| run.copied == run.tables.samples();
    if !complete(&first) || second.as_ref().is_some_and(|run| !complete(run)) {
        return Err(ExtractError::SourceChanged);
    }
    Ok(())
}

/// The least common multiple of the two timescales when it fits 32 bits,
/// else `video`. Both are at least 1 (checked when the tracks are chosen).
pub(crate) fn movie_timescale(video: u32, audio: u32) -> u32 {
    let (mut a, mut b) = (u64::from(video), u64::from(audio));
    while b != 0 {
        (a, b) = (b, a % b);
    }
    u64::from(video)
        .checked_div(a)
        .and_then(|q| q.checked_mul(u64::from(audio)))
        .and_then(|l| u32::try_from(l).ok())
        .unwrap_or(video)
}

/// The edits that present `[from, to)` of `timeline`'s presentation
/// timeline, which has several edits: for each window inside it, in order,
/// a media edit `(duration, Some(media time))`, its media time relative to
/// `first_dts` (the output's decode times start at 0), and an empty edit
/// `(duration, None)` for each gap between two windows; in media ticks,
/// exact (docs/design/edit-list-writers.md, clip decision 4). An iterator,
/// so that a list of thousands of edits is not copied: the movie timescale
/// is chosen in one pass and the edits written in another.
#[derive(Clone)]
pub(crate) struct WindowEdits<'t> {
    timeline: &'t Timeline<'t>,
    windows: WindowsIter<'t>,
    from: Rational,
    to: Rational,
    first_dts: i64,
    previous: Option<Rational>,
    pending: Option<(Rational, Option<Rational>)>,
    done: bool,
}

impl<'t> WindowEdits<'t> {
    pub(crate) fn new(
        timeline: &'t Timeline<'t>,
        from: Rational,
        to: Rational,
        first_dts: i64,
    ) -> Self {
        Self {
            timeline,
            // Windows that end before `from` show nothing of the range.
            windows: timeline
                .windows
                .iter_near(from)
                .unwrap_or_else(|_| timeline.windows.iter()),
            from,
            to,
            first_dts,
            previous: None,
            pending: None,
            done: false,
        }
    }

    fn step(&mut self) -> Result<Option<(Rational, Option<Rational>)>, Error> {
        if let Some(edit) = self.pending.take() {
            return Ok(Some(edit));
        }
        let w = &self.timeline.windows;
        for window in self.windows.by_ref() {
            let (m0, m1, at) = window?;
            let start = w.ticks(at)?;
            let end = add(start, w.ticks(m1.checked_sub(m0).ok_or_else(overflow)?)?)?;
            let media_time = m0.checked_div(w.unit).ok_or_else(overflow)?;
            if !lt(start, self.to)? {
                // This window and every later one start at or after `to`.
                break;
            }
            let (a, b) = (max(start, self.from)?, min(end, self.to)?);
            if !lt(a, b)? {
                continue;
            }
            let media = sub(add(int(media_time), sub(a, start)?)?, int(self.first_dts))?;
            if media.num < 0 {
                return Err(unsupported(
                    "the first sync sample is presented before it is decoded",
                ));
            }
            let edit = (sub(b, a)?, Some(media));
            // Nothing presented before this window: from the range's start
            // (when it falls in a gap) or from the previous window's end.
            let since = self.previous.replace(b).unwrap_or(self.from);
            if lt(since, a)? {
                self.pending = Some(edit);
                return Ok(Some((sub(a, since)?, None)));
            }
            return Ok(Some(edit));
        }
        if self.previous.is_none() {
            return Err(unsupported("no video is presented in the clip's range"));
        }
        Ok(None)
    }
}

impl Iterator for WindowEdits<'_> {
    type Item = Result<(Rational, Option<Rational>), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let item = self.step().transpose();
        if matches!(item, Some(Err(_)) | None) {
            // Nothing after an error or the end.
            self.done = true;
            self.pending = None;
        }
        item
    }
}

/// The smallest multiple of `base` (itself a multiple of `video`) in which
/// `lead` and every duration of `parts`, in `video` ticks, are whole
/// numbers of ticks (docs/design/edit-list-writers.md, clip decision 5).
/// `lead` is the empty edit before the first window, which a segment gets
/// for the gap a cut falls after (split decision 3); 0 for a clip.
pub(crate) fn exact_timescale(
    base: u32,
    video: u32,
    lead: Rational,
    parts: WindowEdits<'_>,
) -> Result<u32, Error> {
    let too_large =
        || unsupported("edit list whose edit durations need a movie timescale above 32 bits");
    let gcd64 = |mut a: u64, mut b: u64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    };
    let mut factor = lead.den.max(1);
    for part in parts {
        let (duration, _) = part?;
        factor = factor
            .checked_div(gcd64(factor, duration.den))
            .and_then(|f| f.checked_mul(duration.den))
            .ok_or_else(too_large)?;
    }
    let needed = u64::from(video).checked_mul(factor).ok_or_else(too_large)?;
    let base = u64::from(base);
    // A multiple of the audio's timescale too where it fits; else the video's
    // alone, the audio edits rounded (clip decision 5).
    match base
        .checked_div(gcd64(base, needed))
        .and_then(|b| b.checked_mul(needed))
        .and_then(|tm| u32::try_from(tm).ok())
    {
        Some(tm) => Ok(tm),
        None => u32::try_from(needed).map_err(|_| too_large()),
    }
}

fn nonnegative(time: Rational) -> Result<Rational, Error> {
    if time.num < 0 {
        return Err(unsupported("a negative clip time"));
    }
    Ok(time)
}

/// The video track `choice` names: the first video track the allowlist
/// keeps by default.
pub(crate) fn select_video(movie: &Movie, choice: VideoChoice) -> Result<&Track, Error> {
    let track = match choice {
        VideoChoice::First => match movie.tracks.iter().find(|t| t.kind == TrackKind::Video) {
            Some(track) => track,
            None => return Err(no_allowed_track(movie, TrackKind::Video)),
        },
        VideoChoice::Stream(index) => by_stream(movie, index)?,
    };
    if track.kind != TrackKind::Video {
        return Err(unsupported("the stream is not a video track"));
    }
    usable(track)?;
    Ok(track)
}

/// The audio track `choice` names, `None` for no audio: the first audio
/// track the allowlist keeps by default.
pub(crate) fn select_audio(movie: &Movie, choice: AudioChoice) -> Result<Option<&Track>, Error> {
    let track = match choice {
        AudioChoice::None => return Ok(None),
        AudioChoice::First => match movie.tracks.iter().find(|t| t.kind == TrackKind::Audio) {
            Some(track) => track,
            None => return Ok(None),
        },
        AudioChoice::Stream(index) => by_stream(movie, index)?,
    };
    if track.kind != TrackKind::Audio {
        return Err(unsupported("the stream is not an audio track"));
    }
    if track.description.codec != Codec::Aac {
        return Err(unsupported("only AAC audio can be copied into a clip"));
    }
    usable(track)?;
    Ok(Some(track))
}

/// Why no track of `kind` can be chosen: a codec that is not allowed when
/// the allowlist dropped one (an encrypted `encv`, for example), else no
/// such track.
pub(crate) fn no_allowed_track(movie: &Movie, kind: TrackKind) -> Error {
    match movie.dropped.iter().find(|d| d.kind == kind) {
        Some(dropped) => Error::CodecNotAllowed {
            track_id: dropped.track_id,
            fourcc: dropped.fourcc,
        },
        None => Error::NoSuchTrack,
    }
}

pub(crate) fn by_stream(movie: &Movie, index: usize) -> Result<&Track, Error> {
    if let Some(track) = movie.tracks.iter().find(|t| t.stream_index == index) {
        return Ok(track);
    }
    match movie.dropped.iter().find(|d| d.stream_index == index) {
        Some(dropped) => Err(Error::CodecNotAllowed {
            track_id: dropped.track_id,
            fourcc: dropped.fourcc,
        }),
        None => Err(Error::NoSuchTrack),
    }
}

pub(crate) fn usable(track: &Track) -> Result<(), Error> {
    if track.descriptions != 1 {
        return Err(unsupported("more than one sample description"));
    }
    if track.external_data {
        return Err(unsupported("the track's data is in another file"));
    }
    if !track.tables_valid {
        return Err(unsupported("the track's sample tables are invalid"));
    }
    if track.timescale == 0 {
        return Err(unsupported("media timescale 0"));
    }
    match track.edit_list {
        EditList::Unsupported => Err(unsupported("edit list outside the supported model")),
        // Several edits: a clip applies them (docs/design/edit-list-writers.md);
        // the other writers refuse them where they read the edit list.
        EditList::None | EditList::Simple { .. } | EditList::Multiple => Ok(()),
    }
}

/// Refuses a stream whose coded data its codec's specification does not
/// allow, so that no writer copies it: an HEVC stream whose conformance
/// window leaves no picture (ITU-T H.265 7.4.3.2.1), which no conforming
/// decoder has to decode (C.5.1).
pub(crate) fn conforming(track: &Track) -> Result<(), Error> {
    match &track.description.codec_config {
        Some(config)
            if &config.kind.0 == b"hvcC" && coreav_isobmff::hevc_window_empty(&config.data) =>
        {
            Err(Error::InvalidStream {
                track_id: track.track_id,
                reason: "the HEVC conformance window leaves no picture (ITU-T H.265 7.4.3.2.1)",
            })
        }
        _ => Ok(()),
    }
}

/// The visual sample entry (ISO/IEC 14496-12 12.1.3): the source's coded
/// width and height, then its codec configuration box and its `pasp`,
/// `clap`, `colr` and `btrt` boxes as stored; its other children are left
/// out. A stream [`conforming`] refuses has none.
pub(crate) fn video_sample_entry(track: &Track) -> Result<Vec<u8>, Error> {
    conforming(track)?;
    let description = &track.description;
    let expected: &[u8; 4] = match (description.codec, &description.fourcc.0) {
        (Codec::H264, b"avc1") => b"avcC",
        (Codec::Hevc, b"hvc1") => b"hvcC",
        (Codec::Av1, b"av01") => b"av1C",
        (Codec::Vp9, b"vp09") => b"vpcC",
        _ => {
            return Err(unsupported(
                "video other than H.264 avc1, HEVC hvc1, AV1 or VP9",
            ));
        }
    };
    let config = description
        .codec_config
        .as_ref()
        .filter(|c| &c.kind.0 == expected)
        .ok_or(unsupported(
            "the video sample entry has no codec configuration",
        ))?;
    let (Some(width), Some(height)) = (description.width, description.height) else {
        return Err(unsupported("the video sample entry has no dimensions"));
    };
    let mut entry = Vec::with_capacity(78usize.saturating_add(config.data.len()));
    entry.extend_from_slice(&[0; 6]);
    entry.extend_from_slice(&1u16.to_be_bytes()); // data_reference_index
    entry.extend_from_slice(&[0; 16]); // pre_defined, reserved
    entry.extend_from_slice(&width.to_be_bytes());
    entry.extend_from_slice(&height.to_be_bytes());
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes()); // 72 dpi
    entry.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    entry.extend_from_slice(&[0; 4]); // reserved
    entry.extend_from_slice(&1u16.to_be_bytes()); // frame_count
    entry.extend_from_slice(&[0; 32]); // compressorname
    entry.extend_from_slice(&0x0018u16.to_be_bytes()); // depth
    entry.extend_from_slice(&(-1i16).to_be_bytes()); // pre_defined
    let mut children = vec![Node::leaf(&config.kind.0, config.data.clone())];
    for child in &description.entry_boxes {
        children.push(Node::leaf(&child.kind.0, child.data.clone()));
    }
    for child in children {
        child.write(&mut entry).map_err(|_| overflow())?;
    }
    let mut stsd = 1u32.to_be_bytes().to_vec();
    Node::leaf(&description.fourcc.0, entry)
        .write(&mut stsd)
        .map_err(|_| overflow())?;
    Ok(stsd)
}

/// A track's media-to-presentation mapping under the simple edit model
/// (at most one empty edit, then one media edit at rate 1.0), in media
/// ticks: a sample at media time `t` is presented
/// at `t - media_time + offset`, and the edit presents media times in
/// `[media_time, media_time + segment)`. With several edits
/// ([`Timeline::with_several`]), each media edit's window maps the samples
/// it owns (`docs/design/edit-lists.md`, decision 2).
#[derive(Clone)]
pub(crate) struct Timeline<'a> {
    media_time: i64,
    offset: Rational,
    /// `None` without an edit, or for an edit that lasts to the end of the
    /// media.
    segment: Option<Rational>,
    /// Whether an edit (or the gapless item) maps the media; without one
    /// every sample is presented.
    edited: bool,
    /// The media edits of a list of several; empty otherwise.
    windows: Windows<'a>,
}

/// A window of a list of several edits, `(media start, media end,
/// presented at)` in units, and whether it holds the media time looked up.
type Owner = (EditWindow, bool);

/// A window of a list of several edits: `(media start, media end,
/// presented at)` in units.
type EditWindow = (i64, i64, i64);

/// Windows between two checkpoints of [`Windows`].
const WINDOW_CHECKPOINT: usize = 256;

/// Most edit list entries one step of [`Windows`] walks: a longer run of
/// empty edits between two media edits is refused.
const MAX_EMPTY_RUN: usize = 4096;

/// The media edits of a list of several, as windows `(media start, media
/// end, presented at)` in units of `1/unit` media ticks, read from the
/// track's own edit list entries when needed rather than copied: a list can
/// hold thousands of edits, and the clip's memory is bounded by its output.
/// Kept: a checkpoint every 256 windows, and the
/// window of the latest lookup, from which the next lookup, usually for a
/// nearby time (samples come in decode order), walks a few entries.
#[derive(Clone, Default)]
pub(crate) struct Windows<'a> {
    /// The track whose edit list entries these are; `None` without
    /// windows.
    track: Option<&'a Track>,
    media_timescale: u32,
    movie_timescale: u32,
    unit: i64,
    /// Every 256th window: its entry and where the entry starts, in movie
    /// ticks.
    checkpoints: Vec<(usize, u64)>,
    /// The window of the latest lookup, as a checkpoint.
    cursor: Cell<(usize, u64)>,
    /// That window in units, once computed, by its entry.
    cached: Cell<Option<(usize, EditWindow)>>,
}

impl<'a> Windows<'a> {
    /// The windows of `track`'s list of several edits, every one checked to
    /// fit in 64-bit units.
    fn new(track: &'a Track, movie_timescale: u32) -> Result<Self, Error> {
        if movie_timescale == 0 {
            return Err(unsupported("edit list with movie timescale 0"));
        }
        let (mt, ms) = (u128::from(track.timescale), u128::from(movie_timescale));
        let gcd128 = |mut a: u128, mut b: u128| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a.max(1)
        };
        // The denominator of `movie_ticks` in media ticks.
        let den = |movie_ticks: u64| -> Result<i64, Error> {
            let num = u128::from(movie_ticks)
                .checked_mul(mt)
                .ok_or_else(overflow)?;
            i64::try_from(ms / gcd128(num, ms)).map_err(|_| overflow())
        };
        let mut unit = 1i64;
        let mut checkpoints = Vec::new();
        let mut count = 0usize;
        let mut at = 0u64;
        for (i, edit) in track.edits.iter().enumerate() {
            if edit.media_time != -1 {
                unit = lcm(lcm(unit, den(at)?)?, den(edit.segment_duration)?)?;
                if count % WINDOW_CHECKPOINT == 0 {
                    checkpoints.push((i, at));
                }
                count = count.saturating_add(1);
            }
            at = at.checked_add(edit.segment_duration).ok_or_else(overflow)?;
        }
        let first = *checkpoints.first().ok_or_else(overflow)?;
        let windows = Self {
            track: Some(track),
            media_timescale: track.timescale,
            movie_timescale,
            unit,
            checkpoints,
            cursor: Cell::new(first),
            cached: Cell::new(None),
        };
        // Every window in units, once, so that later steps cannot overflow.
        let mut next = Some(first);
        while let Some(c) = next {
            windows.window(c)?;
            next = windows.step_forward(c)?;
        }
        Ok(windows)
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    /// Entry `i` of the edit list: its duration in movie ticks and its
    /// media time.
    fn entry(&self, i: usize) -> Option<(u64, i64)> {
        let edit = self.track?.edits.get(i)?;
        Some((edit.segment_duration, edit.media_time))
    }

    /// `movie_ticks` in units (exact by the choice of `unit`).
    fn units(&self, movie_ticks: u64) -> Result<i64, Error> {
        let num = i128::from(movie_ticks)
            .checked_mul(i128::from(self.media_timescale))
            .and_then(|n| n.checked_mul(i128::from(self.unit)))
            .ok_or_else(overflow)?;
        to_i64(num / i128::from(self.movie_timescale))
    }

    /// The window of the media edit at entry `c.0`, starting at `c.1`.
    fn window(&self, c: (usize, u64)) -> Result<(i64, i64, i64), Error> {
        let (duration, media_time) = self.entry(c.0).ok_or_else(overflow)?;
        let from = media_time.checked_mul(self.unit).ok_or_else(overflow)?;
        let to = from
            .checked_add(self.units(duration)?)
            .ok_or_else(overflow)?;
        Ok((from, to, self.units(c.1)?))
    }

    /// Where the media edit at entry `c.0` starts in media units; checked
    /// by [`Windows::new`].
    fn media_start(&self, c: (usize, u64)) -> i64 {
        self.entry(c.0).map_or(i64::MAX, |(_, media_time)| {
            media_time.saturating_mul(self.unit)
        })
    }

    /// The next media edit after `c`, if any.
    fn step_forward(&self, c: (usize, u64)) -> Result<Option<(usize, u64)>, Error> {
        let (mut i, mut at) = c;
        for _ in 0..=MAX_EMPTY_RUN {
            let Some((duration, _)) = self.entry(i) else {
                return Ok(None);
            };
            at = at.checked_add(duration).ok_or_else(overflow)?;
            i = i.saturating_add(1);
            match self.entry(i) {
                None => return Ok(None),
                Some((_, media_time)) if media_time != -1 => return Ok(Some((i, at))),
                Some(_) => {}
            }
        }
        Err(unsupported("edit list with a long run of empty edits"))
    }

    /// The media edit before `c`, if any.
    fn step_back(&self, c: (usize, u64)) -> Result<Option<(usize, u64)>, Error> {
        // Nothing before the first window but empty edits.
        if self.checkpoints.first() == Some(&c) {
            return Ok(None);
        }
        let (mut i, mut at) = c;
        for _ in 0..=MAX_EMPTY_RUN {
            let Some(j) = i.checked_sub(1) else {
                return Ok(None);
            };
            let (duration, media_time) = self.entry(j).ok_or_else(overflow)?;
            at = at.checked_sub(duration).ok_or_else(overflow)?;
            i = j;
            if media_time != -1 {
                return Ok(Some((i, at)));
            }
        }
        Err(unsupported("edit list with a long run of empty edits"))
    }

    /// The last window whose media starts at or before `t` (units), else
    /// the first.
    fn locate(&self, t: i64) -> Result<(usize, u64), Error> {
        let mut c = self.cursor.get();
        // Near the latest lookup: a few steps either way.
        for _ in 0..8 {
            if self.media_start(c) <= t {
                match self.step_forward(c)? {
                    Some(n) if self.media_start(n) <= t => c = n,
                    _ => {
                        self.cursor.set(c);
                        return Ok(c);
                    }
                }
            } else {
                match self.step_back(c)? {
                    Some(p) => c = p,
                    None => {
                        self.cursor.set(c);
                        return Ok(c);
                    }
                }
            }
        }
        // Farther: from the checkpoint before `t`, at most 256 windows.
        let k = self
            .checkpoints
            .partition_point(|&cp| self.media_start(cp) <= t);
        let mut c = *k
            .checked_sub(1)
            .and_then(|k| self.checkpoints.get(k))
            .or_else(|| self.checkpoints.first())
            .ok_or_else(overflow)?;
        for _ in 0..WINDOW_CHECKPOINT {
            match self.step_forward(c)? {
                Some(n) if self.media_start(n) <= t => c = n,
                _ => break,
            }
        }
        self.cursor.set(c);
        Ok(c)
    }

    /// The window that maps media time `t` (in units): the one whose media
    /// interval holds it, else the next one, else the last
    /// (`docs/design/edit-lists.md`, decision 2); and whether `t` is inside
    /// it. `None` without windows.
    #[inline]
    fn owner(&self, t: i64) -> Result<Option<Owner>, Error> {
        if self.is_empty() {
            return Ok(None);
        }
        let c = self.locate(t)?;
        let w = match self.cached.get() {
            Some((i, w)) if i == c.0 => w,
            _ => {
                let w = self.window(c)?;
                self.cached.set(Some((c.0, w)));
                w
            }
        };
        if t < w.0 {
            return Ok(Some((w, false)));
        }
        if t < w.1 {
            return Ok(Some((w, true)));
        }
        Ok(Some(match self.step_forward(c)? {
            Some(n) => (self.window(n)?, false),
            None => (w, false),
        }))
    }

    /// Where media `[from, to)` (units) is presented: from where the
    /// first window that shows part of it starts showing it to where the
    /// last one stops, in units; a window holding `from` shows an empty
    /// interval. `None` when no window shows any of it. The windows'
    /// media intervals increase, so those that show part of it are
    /// consecutive.
    fn hull(&self, from: i64, to: i64) -> Result<Option<(i64, i64)>, Error> {
        let mut next = Some(self.locate(from)?);
        let mut hull: Option<(i64, i64)> = None;
        while let Some(c) = next {
            let w = self.window(c)?;
            if w.0 > to {
                break;
            }
            let (lo, hi) = (from.max(w.0), to.min(w.1));
            let shown = if from == to {
                w.0 <= from && from < w.1
            } else {
                lo < hi
            };
            if shown {
                let at = |t: i64| t.checked_sub(w.0).and_then(|v| v.checked_add(w.2));
                let (p, p_end) = (at(lo).ok_or_else(overflow)?, at(hi).ok_or_else(overflow)?);
                hull = Some(hull.map_or((p, p_end), |(a, b)| (a.min(p), b.max(p_end))));
            }
            next = self.step_forward(c)?;
        }
        Ok(hull)
    }

    /// The windows in order.
    fn iter(&self) -> WindowsIter<'_> {
        WindowsIter {
            windows: self,
            next: self.checkpoints.first().copied(),
        }
    }

    /// The windows in order from the second checkpoint that starts at or
    /// before presentation time `p` (media ticks), back from it: every
    /// window left out ends before the one that starts there, which starts
    /// before `p` (the start of a walk that only needs windows that end
    /// after `p`, or the last window that starts before it).
    fn iter_near(&self, p: Rational) -> Result<WindowsIter<'_>, Error> {
        let den = i128::from(p.den);
        let t = p
            .num
            .checked_mul(i128::from(self.unit))
            .ok_or_else(overflow)?
            .div_euclid(den.max(1));
        let k = self
            .checkpoints
            .partition_point(|&(_, at)| self.units(at).is_ok_and(|u| i128::from(u) <= t));
        let next = self
            .checkpoints
            .get(k.saturating_sub(2))
            .or_else(|| self.checkpoints.first())
            .copied();
        Ok(WindowsIter {
            windows: self,
            next,
        })
    }

    /// The last window.
    fn last(&self) -> Result<Option<(i64, i64, i64)>, Error> {
        let Some(&from) = self.checkpoints.last() else {
            return Ok(None);
        };
        let mut c = from;
        while let Some(n) = self.step_forward(c)? {
            c = n;
        }
        self.window(c).map(Some)
    }

    /// A value in units as exact media ticks.
    fn ticks(&self, units: i64) -> Result<Rational, Error> {
        Rational::new(
            i128::from(units),
            u64::try_from(self.unit).map_err(|_| overflow())?,
        )
        .ok_or_else(overflow)
    }
}

/// The windows of [`Windows::iter`], in order.
#[derive(Clone)]
pub(crate) struct WindowsIter<'w> {
    windows: &'w Windows<'w>,
    next: Option<(usize, u64)>,
}

impl Iterator for WindowsIter<'_> {
    type Item = Result<(i64, i64, i64), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let c = self.next.take()?;
        let item = self.windows.window(c).and_then(|w| {
            self.next = self.windows.step_forward(c)?;
            Ok(w)
        });
        Some(item)
    }
}

impl<'a> Timeline<'a> {
    /// From the track's edit list, or from the `iTunSMPB` item when it
    /// stands for one (see [`gapless`]).
    pub(crate) fn of(
        track: &Track,
        movie: &Movie,
        gapless: Option<ItunesGapless>,
    ) -> Result<Self, Error> {
        match (track.edit_list, gapless) {
            (EditList::None, Some(g)) => Ok(Self {
                media_time: i64::from(g.encoder_delay),
                offset: int(0),
                segment: Some(Rational::from_int(i128::from(g.original_samples))),
                edited: true,
                windows: Windows::default(),
            }),
            (EditList::None, None) => Ok(Self {
                media_time: 0,
                offset: int(0),
                segment: None,
                edited: false,
                windows: Windows::default(),
            }),
            (
                EditList::Simple {
                    media_time,
                    segment,
                    ..
                },
                _,
            ) => {
                let missing = || unsupported("edit list with movie timescale 0");
                Ok(Self {
                    media_time,
                    offset: track
                        .presentation_offset(movie.timescale)
                        .ok_or_else(missing)?,
                    // A media edit of duration 0 lasts to the end.
                    segment: if segment == 0 {
                        None
                    } else {
                        Some(
                            track
                                .media_edit_duration(movie.timescale)
                                .ok_or_else(missing)?,
                        )
                    },
                    edited: true,
                    windows: Windows::default(),
                })
            }
            (EditList::Multiple, _) => Err(unsupported(
                "edit list with several edits (multi-edit), not supported here yet",
            )),
            (EditList::Unsupported, _) => Err(unsupported("edit list outside the supported model")),
        }
    }

    /// As [`Timeline::of`], and a list of several edits is applied
    /// exactly, each media edit a window (`docs/design/edit-list-writers.md`,
    /// clip section).
    pub(crate) fn with_several(
        track: &'a Track,
        movie: &Movie,
        gapless: Option<ItunesGapless>,
    ) -> Result<Self, Error> {
        if track.edit_list != EditList::Multiple {
            return Self::of(track, movie, gapless);
        }
        let windows = Windows::new(track, movie.timescale)?;
        let (from, to, at) = windows.iter().next().ok_or_else(overflow)??;
        Ok(Self {
            media_time: from.checked_div(windows.unit).ok_or_else(overflow)?,
            offset: windows.ticks(at)?,
            segment: Some(windows.ticks(to.checked_sub(from).ok_or_else(overflow)?)?),
            edited: true,
            windows,
        })
    }

    /// Drops what the timeline keeps of a list of several edits, once
    /// nothing reads the windows any more ([`write_cut_head`], after the
    /// edits).
    pub(crate) fn release(&mut self) {
        self.windows = Windows::default();
    }

    /// Whether the track has a list of several edits.
    pub(crate) fn has_windows(&self) -> bool {
        !self.windows.is_empty()
    }

    /// The media edits of a list of several, in order, from a little
    /// before presentation time `p` ([`Windows::iter_near`]): enough for any
    /// window that ends after `p` and for the last one that starts before
    /// it. Each one's presentation interval `[start, end)` in media ticks
    /// (exact) and its first media time.
    pub(crate) fn windows_near(
        &self,
        p: Rational,
    ) -> Result<impl Iterator<Item = Result<(Rational, Rational, i64), Error>> + Clone + '_, Error>
    {
        Ok(self.exact(self.windows.iter_near(p)?))
    }

    fn exact<'w>(
        &'w self,
        windows: WindowsIter<'w>,
    ) -> impl Iterator<Item = Result<(Rational, Rational, i64), Error>> + Clone + 'w {
        let w = &self.windows;
        windows.map(move |window| {
            let (from, to, at) = window?;
            let start = w.ticks(at)?;
            let end = add(start, w.ticks(to.checked_sub(from).ok_or_else(overflow)?)?)?;
            Ok((start, end, from.checked_div(w.unit).ok_or_else(overflow)?))
        })
    }

    /// Media time of presentation time `p`. With several edits, through
    /// the window presented at `p`, or the next one when `p` falls between
    /// two (the clip then starts where that window does).
    pub(crate) fn media(&self, p: Rational) -> Result<Rational, Error> {
        if !self.has_windows() {
            return add(sub(p, self.offset)?, int(self.media_time));
        }
        let mut chosen = None;
        for w in self.windows_near(p)? {
            let w = w?;
            let end = w.1;
            chosen = Some(w);
            if lt(p, end)? {
                break;
            }
        }
        let (start, _, media_time) = chosen.ok_or_else(overflow)?;
        add(sub(max(p, start)?, start)?, int(media_time))
    }

    /// Where presentation up to `e` ends: with several edits, at the end
    /// of the last window that starts before `e` when `e` falls after it,
    /// in the gap before the next one; `e` otherwise.
    pub(crate) fn presented_until(&self, e: Rational) -> Result<Rational, Error> {
        let mut until = e;
        for w in self.windows_near(e)? {
            let (start, end, _) = w?;
            if !lt(start, e)? {
                break;
            }
            until = min(e, end)?;
        }
        Ok(until)
    }

    /// Presentation interval of the edits, `[start, end)`; no end without an
    /// edit.
    pub(crate) fn window(&self) -> Result<(Rational, Option<Rational>), Error> {
        if let Some((from, to, at)) = self.windows.last()? {
            let end = add(
                self.windows.ticks(at)?,
                self.windows
                    .ticks(to.checked_sub(from).ok_or_else(overflow)?)?,
            )?;
            return Ok((self.offset, Some(end)));
        }
        let end = self.segment.map(|s| add(self.offset, s)).transpose()?;
        Ok((self.offset, end))
    }
}

/// A [`Timeline`] in whole units of `1/d` media ticks, `d` being the
/// denominator of its edit's offset and length (1 unless an empty edit is
/// not a whole number of media ticks). The first listing compares every
/// sample's times with these integers, never with exact rationals, whose
/// every operation reduces by a 128-bit greatest common divisor.
pub(crate) struct Scaled<'a> {
    d: i64,
    media_time: i64,
    offset: i64,
    /// The edit's media interval, `[start, end)` in units of `1/d` tick.
    window: Option<(i64, i64)>,
    /// With several edits, each window's media interval and presentation
    /// start, `(media start, media end, presented at)` in units: the
    /// timeline's windows, with a lookup cursor of their own.
    windows: Windows<'a>,
}

impl<'a> Scaled<'a> {
    pub(crate) fn of(timeline: &Timeline<'a>) -> Result<Self, Error> {
        let den = |r: Rational| i64::try_from(r.den).map_err(|_| overflow());
        let mut d = den(timeline.offset)?;
        if let Some(segment) = timeline.segment {
            d = lcm(d, den(segment)?)?;
        }
        // With several edits, the offset and segment are the first
        // window's, so the windows' unit is the scale.
        if timeline.has_windows() {
            let unit = timeline.windows.unit;
            d = lcm(d, unit)?;
            if d != unit {
                return Err(overflow());
            }
        }
        let units = |r: Rational| -> Result<i64, Error> {
            let factor = d.checked_div(den(r)?).ok_or_else(overflow)?;
            to_i64(r.num.checked_mul(i128::from(factor)).ok_or_else(overflow)?)
        };
        let start = timeline.media_time.checked_mul(d).ok_or_else(overflow)?;
        let window = match timeline.segment {
            Some(segment) => Some((
                start,
                start.checked_add(units(segment)?).ok_or_else(overflow)?,
            )),
            // An edit that lasts to the end of the media.
            None if timeline.edited => Some((start, i64::MAX)),
            None => None,
        };
        let windows = timeline.windows.clone();
        Ok(Self {
            d,
            media_time: timeline.media_time,
            offset: units(timeline.offset)?,
            window,
            windows,
        })
    }

    /// With several edits, the window that maps media time `t` (in units):
    /// the one whose media interval holds it, else the next one, else the
    /// last (`docs/design/edit-lists.md`, decision 2); and whether `t` is
    /// inside it.
    #[inline]
    fn owner(&self, t: i64) -> Result<Option<Owner>, Error> {
        self.windows.owner(t)
    }

    /// Where the window that maps media time `t` starts on the
    /// presentation timeline, in units: the edit's start without several.
    #[inline]
    pub(crate) fn owner_start(&self, t: i64) -> Result<i64, Error> {
        let at = t.checked_mul(self.d).ok_or_else(overflow)?;
        Ok(self.owner(at)?.map_or(self.offset, |(w, _)| w.2))
    }

    /// Presentation time of media time `t`.
    #[inline]
    pub(crate) fn present(&self, t: i64) -> Result<i64, Error> {
        if !self.windows.is_empty() {
            let at = t.checked_mul(self.d).ok_or_else(overflow)?;
            let (w, _) = self.owner(at)?.ok_or_else(overflow)?;
            return at
                .checked_sub(w.0)
                .and_then(|v| v.checked_add(w.2))
                .ok_or_else(overflow);
        }
        t.checked_sub(self.media_time)
            .and_then(|v| v.checked_mul(self.d))
            .and_then(|v| v.checked_add(self.offset))
            .ok_or_else(overflow)
    }

    /// A duration in media ticks.
    #[inline]
    pub(crate) fn span(&self, ticks: u32) -> Result<i64, Error> {
        i64::from(ticks).checked_mul(self.d).ok_or_else(overflow)
    }

    /// Whether the edit presents any part of `[t, t + duration)`; with
    /// several edits, whether any window does (a zero-duration sample when
    /// a window holds `t`).
    #[inline]
    pub(crate) fn presents(&self, t: i64, duration: u32) -> Result<bool, Error> {
        if !self.windows.is_empty() {
            let from = t.checked_mul(self.d).ok_or_else(overflow)?;
            let to = from
                .checked_add(self.span(duration)?)
                .ok_or_else(overflow)?;
            return Ok(match self.owner(from)? {
                Some((_, true)) => true,
                Some((w, false)) => duration > 0 && from < w.0 && w.0 < to,
                None => false,
            });
        }
        let Some((start, end)) = self.window else {
            return Ok(true);
        };
        let from = t.checked_mul(self.d).ok_or_else(overflow)?;
        let to = from
            .checked_add(self.span(duration)?)
            .ok_or_else(overflow)?;
        Ok(from < end && start < to)
    }

    /// Where the sample at media time `t` lasting `duration` is presented,
    /// `[start, end)` in units, when any edit presents part of it. With
    /// several edits, every window that shows part of it counts, from
    /// where the first one starts showing it to where the last one stops
    /// (a sample can be shown by more than one).
    pub(crate) fn presented(&self, t: i64, duration: u32) -> Result<Option<(i64, i64)>, Error> {
        if !self.windows.is_empty() {
            let from = t.checked_mul(self.d).ok_or_else(overflow)?;
            let to = from
                .checked_add(self.span(duration)?)
                .ok_or_else(overflow)?;
            return self.windows.hull(from, to);
        }
        if !self.presents(t, duration)? {
            return Ok(None);
        }
        let p = self.present(t)?;
        Ok(Some((
            p,
            p.checked_add(self.span(duration)?).ok_or_else(overflow)?,
        )))
    }

    /// The largest whole unit at most `r` media ticks.
    pub(crate) fn floor(&self, r: Rational) -> Result<i64, Error> {
        let scaled = scale(r, u32::try_from(self.d).map_err(|_| overflow())?, 1)?;
        let neg = Rational::new(scaled.num.checked_neg().ok_or_else(overflow)?, scaled.den)
            .ok_or_else(overflow)?;
        to_i64(ceil(neg)?.checked_neg().ok_or_else(overflow)?)
    }

    /// The smallest whole unit at least `r` media ticks.
    pub(crate) fn ceil(&self, r: Rational) -> Result<i64, Error> {
        to_i64(ceil(scale(
            r,
            u32::try_from(self.d).map_err(|_| overflow())?,
            1,
        )?)?)
    }

    /// Units back to media ticks.
    pub(crate) fn ticks(&self, units: i64) -> Result<Rational, Error> {
        Rational::new(i128::from(units), self.d.unsigned_abs()).ok_or_else(overflow)
    }
}

fn lcm(a: i64, b: i64) -> Result<i64, Error> {
    a.checked_div(gcd(a, b))
        .and_then(|q| q.checked_mul(b))
        .ok_or_else(overflow)
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a.checked_rem(b).unwrap_or(0));
    }
    a.max(1)
}

/// The `iTunSMPB` item of an audio track without an edit list, when it
/// stands for one: the track starts at decode time 0, is the movie's only
/// audio track, and the item's delay, padding and content length add up
/// exactly to its decode durations.
pub(crate) fn gapless<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
) -> Result<Option<ItunesGapless>, Error> {
    if track.edit_list != EditList::None
        || movie.has_fragments()
        || movie
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .count()
            != 1
    {
        return Ok(None);
    }
    let Some(item) = movie.itunes_gapless(source)? else {
        return Ok(None);
    };
    let total = u64::from(item.encoder_delay)
        .checked_add(u64::from(item.padding))
        .and_then(|t| t.checked_add(item.original_samples));
    Ok((item.original_samples > 0 && total == Some(track.sample_duration)).then_some(item))
}

/// Where the video run starts and ends.
pub(crate) struct VideoPlan {
    /// Sample numbers of the snapped sync sample and the last kept sample.
    pub(crate) first: u64,
    pub(crate) last: u64,
    /// The output's time 0 on the source's presentation timeline, in video
    /// media ticks: `zero` for a clip; earlier for a segment whose audio
    /// starts before its video (`crate::split`).
    pub(crate) origin: Rational,
    /// The video's presentation interval `[zero, end)` in video media ticks.
    pub(crate) zero: Rational,
    pub(crate) end: Rational,
    /// The run ends with the track's last sample.
    pub(crate) keeps_last: bool,
    /// The container marks every sample of the run as a sync sample (no
    /// sync sample table, one that lists every sample, or fragment
    /// defaults): the output then marks those the coded data agrees with
    /// (docs/design/cut-points.md, "The output's sync flags").
    pub(crate) marked: bool,
    /// With several edits, the edits end at `end` exactly rather than at
    /// `end` rounded up to a whole video tick: the segments of a split
    /// tile the source's presentation, so one must not present past the
    /// next one's start (docs/design/edit-list-writers.md, split decision
    /// 2). A clip rounds its end up to a whole video tick.
    pub(crate) exact_end: bool,
}

/// What the start search keeps of a sync sample before the start: enough to
/// read its frame.
#[derive(Clone, Copy)]
struct StartCandidate {
    sample: u64,
    offset: u64,
    size: u32,
}

/// Bytes the start search may hold for its candidates, within the 64 KiB a
/// clip may use beyond its output tables (docs/design/cut-points.md,
/// "Cost").
const START_CANDIDATE_BYTES: usize = 24 << 10;

/// Most sync samples before the start kept to verify, newest first: 1,024.
const START_CANDIDATES: usize = START_CANDIDATE_BYTES / core::mem::size_of::<StartCandidate>();

// Checked when compiling: the candidates fit their byte budget, which takes
// at most half of the 64 KiB allowance; and their count is a power of two,
// which a VecDeque growing by doubling (as std's does today, though it does
// not promise to) reaches without a larger buffer. While it grows, the old
// and the new buffer are briefly both held: at most 36 KiB, still below the
// copy pass's 64 KiB.
const _: () = {
    assert!(START_CANDIDATES * core::mem::size_of::<StartCandidate>() <= START_CANDIDATE_BYTES);
    assert!(START_CANDIDATE_BYTES <= 32 << 10);
    assert!(START_CANDIDATES.is_power_of_two());
};

/// Which sync samples may start the clip in [`scan_video`].
#[derive(Clone, Copy)]
enum Gate {
    /// The container's flag (no bitstream view for the track).
    Raw,
    /// Before the start, only this sample, verified already; after it, the
    /// samples the coded data agrees with.
    Chosen(u64),
    /// None before the start (each was checked); after it, the samples the
    /// coded data agrees with.
    After,
    /// Every sample the coded data agrees with (too many sync samples
    /// before the start to keep).
    Verify,
}

/// Listing 1 for the video. The start is the latest
/// sync sample the coded data agrees with (docs/design/cut-points.md): a
/// pass over the sample tables, reading no sample data, finds the sync
/// samples that could start the clip, they are checked from the latest
/// back ([`latest_accepted`]), and the listing then runs with the one
/// chosen, so that only the frames near the start and the end are read.
#[allow(clippy::too_many_arguments)]
fn plan_video<S: Source>(
    movie: &Movie,
    track: &Track,
    timeline: &Timeline,
    start: Rational,
    end: Option<Rational>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<VideoPlan, Error> {
    let cuts = CutPoints::of(movie, track);
    if !cuts.reads() {
        return scan_video(
            movie,
            track,
            timeline,
            start,
            end,
            source,
            limits,
            budget,
            &cuts,
            Gate::Raw,
        );
    }
    let scaled = Scaled::of(timeline)?;
    let start_floor = scaled.floor(start)?;
    // Within the table budget too, for limits below the default.
    let table_bytes = usize::try_from(limits.max_table_bytes).unwrap_or(usize::MAX);
    let capacity = START_CANDIDATES.min(table_bytes / core::mem::size_of::<StartCandidate>());
    let mut candidates: VecDeque<StartCandidate> = VecDeque::new();
    let mut complete = true;
    let mut latest: Option<i64> = None;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        // Past a sample presented after the start, no later one can start
        // at or before it: it would have to be presented after that one.
        if latest.is_some_and(|l| l > start_floor) {
            break;
        }
        let p = scaled.present(packet.cts)?;
        if packet.keyframe && latest.is_none_or(|l| l < p) && p <= start_floor {
            if candidates.len() >= capacity {
                candidates.pop_front();
                complete = false;
            }
            // No room at all under a table budget below one candidate.
            if candidates.len() < capacity {
                candidates.push_back(StartCandidate {
                    sample: packet.sample,
                    offset: packet.offset,
                    size: packet.size,
                });
            }
        }
        if scaled.presents(packet.cts, packet.duration)? {
            latest = Some(latest.map_or(p, |l| l.max(p)));
        }
    }
    let mut listing = movie.packets(track.index, &mut *source, limits, budget)?;
    let chosen = latest_accepted(0, candidates.len(), &mut |i| match candidates.get(i) {
        Some(c) => cuts.allows_sync(&mut listing, c.sample, c.offset, c.size),
        None => Ok(false),
    })?
    .and_then(|i| candidates.get(i))
    .map(|c| c.sample);
    drop(listing);
    let gate = match chosen {
        Some(sample) => Gate::Chosen(sample),
        None if complete => Gate::After,
        None => Gate::Verify,
    };
    scan_video(
        movie, track, timeline, start, end, source, limits, budget, &cuts, gate,
    )
}

/// The listing of [`plan_video`] with the sync samples `gate` allows.
#[allow(clippy::too_many_arguments)]
fn scan_video<S: Source>(
    movie: &Movie,
    track: &Track,
    timeline: &Timeline,
    start: Rational,
    end: Option<Rational>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
    cuts: &CutPoints,
    gate: Gate,
) -> Result<VideoPlan, Error> {
    /// A candidate start and what follows it, in units of the scaled
    /// timeline.
    struct Candidate {
        sample: u64,
        presented_at: i64,
        /// Where the edit window that presents it starts, in units.
        window_at: i64,
        last: u64,
        presented_end: Option<i64>,
        syncs_after_end: u32,
        /// The first sample from this one on that is not marked sync.
        unmarked: Option<u64>,
    }
    let scaled = Scaled::of(timeline)?;
    // The clip ends at the requested end, the edit's end or the stream's
    // end as CoreAV reports it, whichever is first. The
    // stream's end matters without an edit list: a sample whose composition
    // offset is a large unsigned value of a version 0 table is presented
    // long after the stream ends. The run stops before this end too, so a
    // sample presented at the stream's end (a last sample of duration 0) is
    // not copied.
    let (window_start, window_end) = timeline.window()?;
    let mut end = end;
    if let Some(window_end) = window_end {
        end = Some(end.map_or(Ok(window_end), |e| min(e, window_end))?);
    }
    // An end between two windows: the clip ends where the earlier one does
    // (docs/design/edit-list-writers.md, clip decision 4).
    if timeline.has_windows() {
        end = end.map(|e| timeline.presented_until(e)).transpose()?;
    }
    // With several edits, the windows' end above is where the stream ends
    // (`docs/design/edit-lists.md`, decision 5).
    if let (Some(duration), false) = (track.duration, timeline.has_windows()) {
        let stream_end = add(window_start, duration)?;
        end = Some(end.map_or(Ok(stream_end), |e| min(e, stream_end))?);
    }
    // A sample presented at whole unit `p` is at or before `start` when
    // `p <= floor(start)`, and before `end` when `p < ceil(end)`.
    let start_floor = scaled.floor(start)?;
    let end_ceil = end.map(|e| scaled.ceil(e)).transpose()?;
    let before_end = |p: i64| end_ceil.is_none_or(|e| p < e);
    let mut candidate: Option<Candidate> = None;
    let mut fixed = false;
    let mut latest: Option<i64> = None;
    let mut last_sample = None;
    let mut exhausted = true;
    let mut packets = movie.packets(track.index, &mut *source, limits, budget)?;
    while let Some(packet) = packets.next() {
        let packet = packet?;
        last_sample = Some(packet.sample);
        let presented = scaled.presents(packet.cts, packet.duration)?;
        let p = scaled.present(packet.cts)?;
        // Whether the sample may start the clip, or count as a sync sample
        // after its end; the coded data is read only where one of the two
        // decisions below needs it.
        let before = !fixed && p <= start_floor;
        let needed = !fixed || (presented && !before_end(p));
        // Past the start, once a start is chosen, a sync sample cannot
        // start the clip: the first one only ends the search for a start,
        // and later ones decide where the listing stops. The table is
        // believed for the first when it is presented before the end (one
        // at or after the end also counts towards where the listing stops),
        // and for the others in a run with samples the container does not
        // mark, so that no frame away from the start is read before the
        // copy; a run marked sync throughout is checked.
        let believed = match candidate.as_ref() {
            Some(c) if fixed => c.unmarked.is_some(),
            Some(_) => p > start_floor && before_end(p),
            None => false,
        };
        let keyframe = packet.keyframe
            && match gate {
                Gate::Raw => true,
                Gate::Chosen(sample) if before => packet.sample == sample,
                Gate::After if before => false,
                _ => needed && (believed || cuts.allows(&mut packets, &packet)?),
            };
        let p_end = p
            .checked_add(scaled.span(packet.duration)?)
            .ok_or_else(overflow)?;
        // A sync sample the edit does not present (before an edit that
        // trims the start, say) can still start the clip: the clip then
        // starts where the edit does.
        if keyframe && !fixed {
            // Every sample before it in decode order is presented before
            // it, so starting here loses no frame of the range.
            if latest.is_none_or(|l| l < p) {
                let after_start = p > start_floor;
                if !after_start || candidate.is_none() {
                    candidate = Some(Candidate {
                        sample: packet.sample,
                        presented_at: p,
                        window_at: scaled.owner_start(packet.cts)?,
                        last: packet.sample,
                        presented_end: None,
                        syncs_after_end: 0,
                        unmarked: None,
                    });
                }
                fixed = after_start;
            }
        }
        if presented {
            latest = Some(latest.map_or(p, |l| l.max(p)));
        }
        if let Some(c) = candidate.as_mut() {
            if !packet.keyframe && c.unmarked.is_none() {
                c.unmarked = Some(packet.sample);
            }
            if presented {
                if before_end(p) {
                    c.last = packet.sample;
                }
                c.presented_end = Some(c.presented_end.map_or(p_end, |e| e.max(p_end)));
                // Leading samples of the next sync sample at or after the
                // end may still be presented before it; past the second
                // such sync sample, none are.
                if fixed && keyframe && packet.sample != c.sample && !before_end(p) {
                    c.syncs_after_end = c.syncs_after_end.saturating_add(1);
                    if c.syncs_after_end >= 2 {
                        exhausted = false;
                        break;
                    }
                }
            }
        }
    }
    let c = candidate.ok_or(unsupported("no video sync sample to start from"))?;
    // A sync sample no edit presents starts the clip where its window does
    // (with several edits, the window that maps it).
    let zero = max(scaled.ticks(c.presented_at)?, scaled.ticks(c.window_at)?)?;
    // When the listing reached the track's end, the clip ends no later
    // than its last presented sample does.
    let mut clip_end = end;
    if exhausted {
        if let Some(presented_end) = c.presented_end {
            let presented_end = scaled.ticks(presented_end)?;
            clip_end = Some(clip_end.map_or(Ok(presented_end), |e| min(e, presented_end))?);
        }
    }
    let clip_end = clip_end.ok_or(unsupported("no video sample is presented"))?;
    if !lt(zero, clip_end)? || !lt(start, clip_end)? {
        // A start at or after the video's end, or a range that ends at or
        // before the first sync sample it could start from.
        return Err(unsupported("no video is presented in the clip's range"));
    }
    Ok(VideoPlan {
        first: c.sample,
        last: c.last,
        origin: zero,
        zero,
        end: clip_end,
        keeps_last: exhausted && last_sample == Some(c.last),
        marked: c.unmarked.is_none_or(|u| u > c.last),
        exact_end: false,
    })
}

/// Where the audio run starts and ends, in audio media ticks.
#[derive(Clone)]
pub(crate) struct AudioPlan<'a> {
    pub(crate) first: u64,
    pub(crate) last: u64,
    /// The output's time 0 on the audio's presentation timeline.
    pub(crate) clip_start: Rational,
    /// Where the audio is presented in the output: `[start, end)`.
    pub(crate) start: Rational,
    pub(crate) end: Rational,
    pub(crate) timeline: Timeline<'a>,
    pub(crate) keeps_last: bool,
}

/// Listing 1 for the audio: the samples presented in the clip
/// `[clip_start, clip_end)`, in audio media ticks (to the end of the track
/// without `clip_end`), one sample of pre-roll before them. `None` when no
/// audio is presented in the clip.
#[allow(clippy::too_many_arguments)]
fn plan_audio<'a, S: Source>(
    movie: &Movie,
    track: &Track,
    timeline: Timeline<'a>,
    clip_start: Rational,
    clip_end: Option<Rational>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Option<AudioPlan<'a>>, Error> {
    let scaled = Scaled::of(&timeline)?;
    // A sample at whole unit `q` starts at or after the clip's end when
    // `q >= ceil(end)`, and ends after its start when `q_end > floor(start)`.
    let end_ceil = clip_end.map(|end| scaled.ceil(end)).transpose()?;
    let start_floor = scaled.floor(clip_start)?;
    let mut first: Option<(u64, i64)> = None;
    let mut last: Option<(u64, i64)> = None;
    let mut previous = None;
    let mut last_sample = None;
    let mut exhausted = true;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        if packet.cts != packet.dts {
            return Err(unsupported("composition offsets in an audio track"));
        }
        last_sample = Some(packet.sample);
        let q = scaled.present(packet.dts)?;
        if end_ceil.is_some_and(|end| q >= end) {
            exhausted = false;
            break;
        }
        let q_end = q
            .checked_add(scaled.span(packet.duration)?)
            .ok_or_else(overflow)?;
        if scaled.presents(packet.dts, packet.duration)? && q_end > start_floor {
            if first.is_none() {
                // One sample of pre-roll before the first presented one.
                let from = previous.unwrap_or(packet.sample);
                first = Some((from, q));
            }
            last = Some((packet.sample, q_end));
        }
        previous = Some(packet.sample);
    }
    let (Some((first, first_at)), Some((last, last_end))) = (first, last) else {
        return Ok(None);
    };
    let (first_at, last_end) = (scaled.ticks(first_at)?, scaled.ticks(last_end)?);
    // Presented from the clip's start, or later when the audio's edit or
    // its first presented sample starts later.
    let (window_start, window_end) = timeline.window()?;
    let start = max(max(clip_start, window_start)?, first_at)?;
    let mut end = match clip_end {
        Some(clip_end) => min(clip_end, last_end)?,
        None => last_end,
    };
    if let Some(window_end) = window_end {
        end = min(end, window_end)?;
    }
    if !lt(start, end)? {
        return Ok(None);
    }
    // With several edits, the range may fall between two windows.
    if timeline.has_windows() {
        let mut shown = false;
        for w in timeline.windows_near(start)? {
            let (a, b, _) = w?;
            if !lt(a, end)? {
                break;
            }
            if lt(max(a, start)?, min(b, end)?)? {
                shown = true;
                break;
            }
        }
        if !shown {
            return Ok(None);
        }
    }
    Ok(Some(AudioPlan {
        first,
        last,
        clip_start,
        start,
        end,
        timeline,
        keeps_last: exhausted && last_sample == Some(last),
    }))
}

/// Listing 2: the output tables of samples `first..=last` of `track`, with
/// the first sample's decode and composition times. With `verify`, a
/// sample is a sync sample only where that cut rule allows it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_tables<S: Source>(
    movie: &Movie,
    track: &Track,
    first: u64,
    last: u64,
    video: bool,
    verify: Option<&CutPoints>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<(Tables, i64, i64), ExtractError> {
    let mut packets = movie.packets(track.index, &mut *source, limits, budget)?;
    tables_of(&mut packets, track, first, last, video, verify, limits)
}

/// The output tables of samples `first..=last` from `packets`, which stops
/// right after sample `last`, so that a listing can go on with the next
/// run (`crate::split`). A sample is written as a sync sample when the
/// container marks it and, with `verify`, that cut rule allows it
/// (docs/design/cut-points.md, "The output's sync flags").
pub(crate) fn tables_of<S: Source>(
    packets: &mut Packets<'_, S>,
    track: &Track,
    first: u64,
    last: u64,
    video: bool,
    verify: Option<&CutPoints>,
    limits: &Limits,
) -> Result<(Tables, i64, i64), ExtractError> {
    let mut tables = Tables::new(track.timescale, limits.max_table_bytes);
    let mut first_times = None;
    let mut next_dts = None;
    if first <= last {
        while let Some(packet) = packets.next() {
            let packet = packet?;
            if packet.sample < first {
                continue;
            }
            if packet.sample > last {
                break;
            }
            // The output stores durations only, so each sample must start
            // where the previous one ends; fragments set their own base
            // decode time.
            if next_dts.is_some_and(|next| next != packet.dts) {
                return Err(unsupported("a gap or an overlap between decode times").into());
            }
            next_dts = Some(
                packet
                    .dts
                    .checked_add(i64::from(packet.duration))
                    .ok_or_else(overflow)?,
            );
            first_times.get_or_insert((packet.dts, packet.cts));
            let offset = if video {
                packet.cts.checked_sub(packet.dts).ok_or_else(overflow)?
            } else {
                0
            };
            let sync = match verify {
                Some(cuts) => cuts.allows(packets, &packet)?,
                None => packet.keyframe,
            };
            tables.push_sample(packet.size, packet.duration, sync, offset)?;
            if packet.sample == last {
                break;
            }
        }
    }
    tables.finish();
    let expected = last
        .checked_sub(first)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(overflow)?;
    if u64::from(tables.samples()) != expected {
        return Err(ExtractError::SourceChanged);
    }
    let (dts, cts) = first_times.ok_or(ExtractError::SourceChanged)?;
    Ok((tables, dts, cts))
}

/// The output media duration: the kept decode durations, plus, when the
/// run ends with the source track's last sample of a movie without
/// fragments that declares a media duration, the source's difference
/// between that duration and its own decode durations, as audio extraction
/// copies a declared media duration.
pub(crate) fn media_duration(
    movie: &Movie,
    track: &Track,
    tables: &Tables,
    keeps_last: bool,
) -> Result<u64, Error> {
    if !keeps_last || movie.has_fragments() || track.mdhd_duration == 0 {
        return Ok(tables.duration);
    }
    let difference = i128::from(track.mdhd_duration) - i128::from(track.sample_duration);
    let carried = i128::from(tables.duration).saturating_add(difference);
    Ok(if carried > 0 {
        to_u64(carried)?
    } else {
        tables.duration
    })
}

/// One track's kept run in the copy pass.
pub(crate) struct Run<'p, 'a, S: Source> {
    packets: &'p mut Packets<'a, Shared<'a, S>>,
    tables: &'p Tables,
    copied: u32,
    /// The run's first sample, read ahead so that the pass can advise the
    /// source where it starts.
    first: Option<(u64, u32)>,
}

impl<'p, 'a, S: Source> Run<'p, 'a, S> {
    /// The run of `tables`, after skipping the next `skip` packets of
    /// `packets` without reading sample data.
    pub(crate) fn new(
        packets: &'p mut Packets<'a, Shared<'a, S>>,
        skip: u64,
        tables: &'p Tables,
    ) -> Result<Self, Error> {
        let mut skipped = 0u64;
        while skipped < skip {
            match packets.next() {
                Some(packet) => {
                    packet?;
                }
                None => return Err(unsupported("the source changed during the clip")),
            }
            skipped = skipped.saturating_add(1);
        }
        let first = match tables.samples() {
            0 => None,
            _ => {
                let packet = packets
                    .next()
                    .ok_or(unsupported("the source changed during the clip"))??;
                Some((packet.offset, packet.size))
            }
        };
        Ok(Self {
            packets,
            tables,
            copied: 0,
            first,
        })
    }

    /// Where the run's first sample starts, and the bytes of all its
    /// samples.
    fn extent(&self) -> Option<(u64, u64)> {
        let (offset, _) = self.first?;
        let bytes = (0..self.tables.samples())
            .filter_map(|index| self.tables.size(index))
            .fold(0u64, |sum, size| sum.saturating_add(u64::from(size)));
        Some((offset, bytes))
    }

    /// Offset and size of the next sample, checked against the tables.
    fn next(&mut self) -> Result<(u64, u32), ExtractError> {
        let (offset, size) = match self.first.take() {
            Some(first) => first,
            None => {
                let packet = self.packets.next().ok_or(ExtractError::SourceChanged)??;
                (packet.offset, packet.size)
            }
        };
        if self.tables.size(self.copied) != Some(size) {
            return Err(ExtractError::SourceChanged);
        }
        self.copied = self
            .copied
            .checked_add(1)
            .ok_or(ExtractError::SourceChanged)?;
        Ok((offset, size))
    }
}
