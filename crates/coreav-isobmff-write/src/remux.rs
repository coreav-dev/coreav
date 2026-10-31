//! Full-file remux of an MP4/MOV source to MP4 by stream copy
//! (`docs/design/remux.md`): every stream that MP4 can carry, all of its
//! samples, the movie box first. Written from ISO/IEC 14496-12 (boxes,
//! sample tables, edit lists), ISO/IEC 14496-14 (MP4), ISO/IEC 14496-15
//! (AVC and HEVC sample entries), the AV1 and VP codec ISO media file
//! format bindings, the Opus in ISOBMFF encapsulation, the FLAC-in-ISOBMFF
//! mapping, Apple's ALAC magic cookie description, ETSI TS 102 366 Annex F
//! (AC-3 and E-AC-3) and the QuickTime File Format (its sound sample entry
//! versions and language codes).

use crate::audio;
use crate::copy::Shared;
use crate::moov::UNITY;
use crate::moov::{TrackFlags, language, overflow, rescale};
use crate::mux::{
    CopyWindow, Edit, FileType, MovieHeader, Muxer, RawBox, Sample, TrackHeader, TrackMedia,
    audio_sample_entry, visual_sample_entry,
};
use crate::replace::{
    AudioReplacement, ReplacementSample, Replacements, audio_edits, audio_edits_exact, durations,
    roll_boxes, validate, validate_audio,
};
use crate::sanitize::{AUDIO_KEPT, EntryBox, Removed, VISUAL_KEPT, describe, entry_box, rewrite};
use crate::scan::{FileScan, MAX_PATH, Path, Skipped, TrakScan, read_elst, scan};
use crate::{
    BoxDropReason, Cleared, ClearedField, DroppedBox, DroppedTrack, ExtractError, Keep, Metadata,
    MetadataKind, OnUnsupported, RemuxOptions, RemuxSummary, RemuxTrack, TrackDropReason,
};
use core::cell::RefCell;
use coreav_isobmff::{
    Allowlist, BoxHeader, Budget, Codec, EditList, Error, FourCc, LimitKind, Limits, Movie,
    Packets, Parsed, SliceSource, Source, Track, TrackKind, parse_box_header, probe,
};
use std::io::Write;

/// Reads at least this large are handed straight to the caller's buffer by
/// a `BufferedSource` (`coreav_isobmff::BufferedSource`).
const DIRECT_READ: usize = 64 << 10;

/// Boxes this small are read through a buffered source's window as is.
const SMALL_BOX: usize = 4 << 10;

/// Most dropped boxes listed in the summary.
const MAX_REPORTED: usize = 256;

/// Rewrites `source` as an MP4 file written to `out`, by stream copy.
///
/// The source is probed with `limits` and `allowlist`. Every stream that
/// MP4 can carry is kept, or those `options` names: H.264, HEVC, AV1 and
/// VP9 video, AAC, MP3, Opus, FLAC, ALAC, AC-3 and E-AC-3 audio. Other streams (ProRes, PCM, timed
/// text, timecode, data) are dropped and listed in the summary, or fail
/// the remux under [`OnUnsupported::Fail`]. Every sample of a kept stream
/// is copied unchanged, with its duration, composition offset and sync
/// flag; the edit list, the media and movie timescales, the declared media
/// duration, the language, the track header flags and matrix, the sample
/// entry's child boxes and the movie's and tracks' `udta` and `meta` boxes
/// are copied as stored, less the cover pictures `options` leaves out. A
/// fragmented source becomes an unfragmented file.
/// Every other box is listed in the summary as dropped.
///
/// The source is listed twice per kept stream, once for the sample tables
/// and once to copy the samples, so `out` receives `ftyp`, `moov` and
/// `mdat` in that order without seeking. Memory is the output tables plus
/// one 64 KiB copy window, plus the copied metadata and edit lists
/// (bounded by `max_retained_bytes`). `out` gets many small writes; give it
/// a buffered writer.
///
/// Fails before writing anything when the file cannot be probed or no
/// stream can be copied; fails with a partial output when the source cannot
/// be read later on, changes between the passes, or the writer fails.
pub fn remux_mp4<S: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &RemuxOptions,
    out: W,
) -> Result<RemuxSummary, ExtractError> {
    let none = Replacements::<SliceSource<'static>> {
        video: None,
        audio: None,
    };
    remux(source, limits, allowlist, options, none, out)
}

/// [`remux_mp4`] with one video stream of the source, one audio stream, or
/// one of each, replaced by the samples of `replacements`
/// (`docs/design/remux.md`, "Replaced streams"); every other stream is
/// written exactly as [`remux_mp4`] writes it. A replaced audio stream is
/// written as [`AudioReplacement`] describes, on a timeline of its own; the
/// rest of this description is the video stream's.
///
/// The replaced stream keeps the source track's place in the track order,
/// its language, matrix (rotation), flags, layer, alternate group,
/// creation times and `udta`/`meta` boxes (or what `options.metadata`
/// keeps of them), and its media timescale. It takes the replacement's
/// sample entry, presentation width and height (`tkhd`) and samples; its
/// media duration is the sum of the replacement's durations, and its track
/// duration the sum of its edits. The source track's per-sample boxes
/// (`sdtp`, `sgpd`, `sbgp`, ...) describe the old samples: they are left
/// out and listed as dropped (`not_carried`). None of the source track's
/// samples is read, so its codec need not be one MP4 carries.
///
/// Sample times are media ticks of the source track on its media timeline
/// ([`Replacement`]). The source track's edit list must be of the simple
/// model (at most one empty edit, then one media edit at rate 1) or absent;
/// any other fails with `Unsupported`. The output's decode times start at
/// 0, so the edit list's media time moves by the replacement's first
/// decode time instead of the source's, and a frame with presentation time
/// `P` is presented at the movie time at which [`remux_mp4`] presents a
/// source sample with composition time `P`. A source media edit that lasts
/// to the end of the media (duration 0) lasts to the end of the
/// replacement's. Where the output needs an empty edit longer than the
/// source's (the replacement's first decode time after the edit's media
/// time, or above 0 without an edit list), the movie timescale becomes a
/// common multiple of the source's and the media timescale when one fits
/// 32 bits, so that the edit stays exact; otherwise it is rounded to the
/// movie timescale. Without an edit list and with a first decode time of
/// 0, none is written.
///
/// Where an audio replacement's edits would round in the movie timescale,
/// it becomes a common multiple of it and the replacement's timescale, as
/// for the video stream's empty edit.
///
/// Fails before writing anything when a replacement is invalid
/// ([`Replacement`]: a sample entry box of a known type, decode times
/// strictly increasing, presentation at or after decode, every sample
/// within `replacement.data`; [`AudioReplacement`]), when a
/// `stream_index` is not a stream of its kind in the source (or the
/// allowlist dropped it, or `options.streams` leaves it out), when nothing
/// of the video replacement would be presented, or when the output tables
/// exceed `limits`; later failures are those of [`remux_mp4`], and a read
/// error of a replacement's data.
pub fn remux_mp4_replacing<S: Source, R: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &RemuxOptions,
    replacements: Replacements<'_, R>,
    out: W,
) -> Result<RemuxSummary, ExtractError> {
    remux(source, limits, allowlist, options, replacements, out)
}

/// Which replacement a stream's samples are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Replaced {
    Video,
    Audio,
}

/// The full-file remux, with or without replaced streams.
fn remux<S: Source, R: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &RemuxOptions,
    replacements: Replacements<'_, R>,
    mut out: W,
) -> Result<RemuxSummary, ExtractError> {
    let Replacements {
        video: mut replacement,
        mut audio,
    } = replacements;
    let replaced_codec = match replacement.as_mut() {
        Some(r) => Some(validate(r, limits)?),
        None => None,
    };
    let audio_codec = match audio.as_mut() {
        Some(a) => Some(validate_audio(a, limits)?),
        None => None,
    };
    let mut source = source;
    let movie = probe(&mut source, limits, allowlist)?;
    // The boxes this copies are read from the file's movie box, which in a
    // compressed movie holds only the compressed data
    // (docs/design/compressed-movie.md).
    if movie.compressed {
        return Err(Error::Unsupported {
            reason: "compressed movie box (cmov): its boxes cannot be copied",
        }
        .into());
    }
    let file = scan(&mut source, limits)?;
    let streams = movie.tracks.len().saturating_add(movie.dropped.len());
    if file.traks.len() != streams {
        return Err(Error::InvalidBox {
            offset: 0,
            kind: FourCc(*b"moov"),
            reason: "track boxes that probe did not count",
        }
        .into());
    }
    if let Some(wanted) = &options.streams {
        if u64::try_from(wanted.len()).unwrap_or(u64::MAX) > u64::from(limits.max_tracks) {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Tracks,
                offset: 0,
            }
            .into());
        }
        for index in wanted {
            if *index >= streams {
                return Err(Error::NoSuchTrack.into());
            }
            if let Some(d) = movie.dropped.iter().find(|d| d.stream_index == *index) {
                return Err(Error::CodecNotAllowed {
                    track_id: d.track_id,
                    fourcc: d.fourcc,
                }
                .into());
            }
        }
    }
    if let Some(r) = &replacement {
        check_replaced(&movie, r.stream_index, TrackKind::Video, options)?;
    }
    if let Some(a) = &audio {
        check_replaced(&movie, a.stream_index, TrackKind::Audio, options)?;
        if replacement
            .as_ref()
            .is_some_and(|r| r.stream_index == a.stream_index)
        {
            return Err(unsupported("one stream replaced twice").into());
        }
    }
    let mut retained = Retained::new(limits);
    let mut dropped_tracks = Vec::new();
    let drop_unsupported =
        options.on_unsupported == OnUnsupported::Drop && options.streams.is_none();
    // The output's tracks, in the order named: a stream named twice is
    // written twice.
    let order: Vec<&Track> = match &options.streams {
        None => movie.tracks.iter().collect(),
        Some(wanted) => {
            for track in &movie.tracks {
                if !wanted.contains(&track.stream_index) {
                    dropped_tracks.push(dropped(track, TrackDropReason::NotSelected));
                }
            }
            if (replacement.is_some() || audio.is_some())
                && wanted
                    .iter()
                    .enumerate()
                    .any(|(n, i)| wanted.iter().skip(n.saturating_add(1)).any(|j| j == i))
            {
                return Err(unsupported("a stream named twice beside a replaced stream").into());
            }
            wanted
                .iter()
                .map(|i| {
                    movie
                        .tracks
                        .iter()
                        .find(|t| t.stream_index == *i)
                        .ok_or(Error::NoSuchTrack)
                })
                .collect::<Result<_, _>>()?
        }
    };
    // What every kept stream needs (its sample entry and edit list) is read
    // first; the optional metadata gets what `max_retained_bytes` leaves.
    let mut plans = Vec::new();
    for track in order {
        let trak = file.traks.get(track.stream_index).ok_or_else(overflow)?;
        let before = retained.left;
        let replace = match (&replacement, &audio) {
            (Some(r), _) if r.stream_index == track.stream_index => Some(Replace::Video {
                entry: r.sample_entry,
                width: r.width,
                height: r.height,
            }),
            (_, Some(a)) if a.stream_index == track.stream_index => Some(Replace::Audio {
                entry: a.sample_entry,
                timescale: a.timescale,
            }),
            _ => None,
        };
        match plan(
            track,
            trak,
            options.metadata,
            &mut source,
            limits,
            &mut retained,
            replace,
        ) {
            Ok(plan) => plans.push(plan),
            Err(Error::Unsupported { reason }) if drop_unsupported && replace.is_none() => {
                retained.left = before;
                dropped_tracks.push(dropped(track, TrackDropReason::Unsupported { reason }));
            }
            Err(error) => return Err(error.into()),
        }
    }
    for d in &movie.dropped {
        dropped_tracks.push(DroppedTrack {
            stream_index: d.stream_index,
            track_id: d.track_id,
            reason: TrackDropReason::CodecNotAllowed,
        });
    }

    // Pass 1: the output tables, one stream at a time.
    let mut cleared = Vec::new();
    let movie_created = match options.metadata {
        Metadata::Sanitize(keep) if !keep.creation_times => {
            if movie.creation_time != 0 {
                cleared.push(Cleared {
                    stream_index: None,
                    field: ClearedField::CreationTime,
                });
            }
            0
        }
        _ => movie.creation_time,
    };
    let mut muxer = Muxer::new(
        FileType {
            major: FourCc(*b"isom"),
            minor: 0,
            compatible: Vec::new(),
        },
        MovieHeader {
            timescale: movie.timescale,
            creation_time: movie_created,
            boxes: Vec::new(),
        },
        limits,
    )?;
    let mut budget = Budget::new(limits);
    let mut kept: Vec<Kept<'_>> = Vec::new();
    for plan in plans {
        let before = budget.used();
        let index = muxer.add_track(plan.header)?;
        let listed = match (plan.replaced, &replacement, &audio) {
            (Some(Replaced::Video), Some(r), _) => {
                list_replaced(&mut muxer, index, r.samples, r.last_duration).map(|first| (first, 0))
            }
            (Some(Replaced::Audio), _, Some(a)) => {
                list_audio(&mut muxer, index, a).map(|()| (0, 0))
            }
            (Some(_), _, _) => Err(overflow().into()),
            (None, _, _) => list(
                &movie,
                plan.track,
                &plan.edits,
                plan.declared,
                &mut muxer,
                index,
                &mut source,
                limits,
                &mut budget,
            ),
        };
        match listed {
            Ok((first_dts, composition_end)) => {
                cleared.extend(plan.cleared);
                let codec = match (plan.replaced, replaced_codec) {
                    (Some(Replaced::Video), Some(codec)) => codec.isobmff(),
                    (Some(Replaced::Audio), _) => {
                        audio_codec.map_or(plan.track.description.codec, |c| c.isobmff())
                    }
                    _ => plan.track.description.codec,
                };
                kept.push(Kept {
                    track: plan.track,
                    trak: plan.trak,
                    work: budget.used().saturating_sub(before),
                    gone: plan.gone,
                    gone_more: 0,
                    edits: plan.edits,
                    first_dts,
                    composition_end,
                    codec,
                    replaced: plan.replaced,
                    declared: plan.declared.filter(|_| plan.replaced.is_none()),
                });
            }
            Err(ExtractError::Source(Error::Unsupported { reason }))
                if drop_unsupported && plan.replaced.is_none() =>
            {
                muxer.pop_track();
                retained.left = retained.left.saturating_add(plan.retained);
                dropped_tracks.push(dropped(plan.track, TrackDropReason::Unsupported { reason }));
            }
            Err(error) => return Err(error),
        }
    }
    if kept.is_empty() {
        // The reason of the first stream that could not be copied, if any.
        let reason = dropped_tracks
            .iter()
            .find_map(|d| match d.reason {
                TrackDropReason::Unsupported { reason } => Some(reason),
                _ => None,
            })
            .unwrap_or("no stream can be copied to MP4");
        return Err(Error::Unsupported { reason }.into());
    }
    dropped_tracks.sort_by_key(|d| d.stream_index);

    // The edit lists, in the movie timescale. A fragmented source's track
    // durations and some of its edits are computed from media ticks; where
    // one would round in the source's movie timescale, the movie timescale
    // becomes a common multiple of it and that stream's media timescale,
    // when one fits 32 bits. Most files keep theirs (and 32-bit headers).
    let mut movie_timescale = movie.timescale;
    if movie.has_fragments() {
        for (index, k) in kept
            .iter()
            .enumerate()
            .filter(|(_, k)| k.replaced.is_none())
        {
            let sum = muxer.duration(index).ok_or_else(overflow)?;
            let computed = computed_ticks(&k.edits, k.first_dts, sum, k.composition_end);
            let rounds = computed.iter().flatten().any(|ticks| {
                u128::from(*ticks)
                    .saturating_mul(u128::from(movie_timescale))
                    .checked_rem(u128::from(k.track.timescale))
                    .is_some_and(|r| r != 0)
            });
            if rounds {
                movie_timescale =
                    lcm(movie_timescale, k.track.timescale).unwrap_or(movie_timescale);
            }
        }
    }
    // The replaced stream's edits, in media ticks where they are computed;
    // where one would round, the movie timescale grows the same way.
    let mut replaced_edits = None;
    for (index, k) in kept
        .iter()
        .enumerate()
        .filter(|(_, k)| k.replaced == Some(Replaced::Video))
    {
        let Some(r) = replacement.as_ref() else {
            continue;
        };
        // The end of the frame presented last (the last of them on a tie).
        let end = durations(r.samples, r.last_duration)
            .max_by_key(|(s, _)| s.presentation_time)
            .and_then(|(s, duration)| {
                s.presentation_time
                    .checked_add(i64::from(duration))?
                    .checked_sub(k.first_dts)
            })
            .and_then(|end| u64::try_from(end).ok())
            .ok_or_else(overflow)?;
        let p_min = r
            .samples
            .iter()
            .map(|s| s.presentation_time)
            .min()
            .ok_or_else(overflow)?;
        let source_first = match k.track.edit_list {
            EditList::Simple { .. } => {
                first_composition(&movie, k.track, &mut source, limits, &mut budget)?
            }
            _ => None,
        };
        let planned = ReplacedEdits::plan(k.track, k.first_dts, p_min, source_first, end)?;
        // A switch would move the computed values of a fragmented source's
        // other tracks that round in the movie timescale (the plain remux
        // rounds them there): it is left out then, and the replaced
        // stream's empty edit rounds instead.
        let others_round = others_round(&movie, &kept, &muxer, movie_timescale);
        if !others_round
            && planned
                .as_ref()
                .is_some_and(|p| p.rounds(movie_timescale, k.track.timescale))
        {
            movie_timescale = lcm(movie_timescale, k.track.timescale).unwrap_or(movie_timescale);
        }
        replaced_edits = Some((index, planned));
    }
    // The audio replacement's edits, the same way.
    if let Some(a) = &audio {
        if !audio_edits_exact(a, movie_timescale)
            && !others_round(&movie, &kept, &muxer, movie_timescale)
        {
            movie_timescale = lcm(movie_timescale, a.timescale).unwrap_or(movie_timescale);
        }
    }
    let factor = movie_timescale / movie.timescale.max(1);
    muxer.set_movie_timescale(movie_timescale)?;
    if factor != 1 && !movie.has_fragments() {
        // The declared track durations, set in the source's movie ticks
        // (only a replaced stream changes an unfragmented source's movie
        // timescale).
        for (index, k) in kept.iter().enumerate() {
            if let Some(declared) = k.declared {
                let scaled = declared
                    .checked_mul(u64::from(factor))
                    .ok_or_else(overflow)?;
                muxer.set_duration(index, Some(scaled))?;
            }
        }
    }
    for (index, k) in kept.iter().enumerate() {
        if k.replaced == Some(Replaced::Audio) {
            let edits = match &audio {
                Some(a) => audio_edits(a, movie_timescale)?,
                None => Vec::new(),
            };
            muxer.set_edits(index, edits)?;
            continue;
        }
        if k.replaced.is_some() {
            let edits = match &replaced_edits {
                Some((i, Some(planned))) if *i == index => {
                    planned.edits(factor, movie_timescale, k.track.timescale)?
                }
                _ => Vec::new(),
            };
            muxer.set_edits(index, edits)?;
            continue;
        }
        let sum = muxer.duration(index).ok_or_else(overflow)?;
        let source_edits = k
            .edits
            .iter()
            .map(|e| {
                Ok(Edit {
                    duration: e
                        .duration
                        .checked_mul(u64::from(factor))
                        .ok_or_else(overflow)?,
                    ..*e
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let edits = edits(
            &movie,
            k.track,
            movie_timescale,
            &source_edits,
            k.first_dts,
            sum,
            k.composition_end,
        );
        muxer.set_edits(index, edits?)?;
    }

    // The optional boxes of the kept streams and of the movie.
    let fragmented = movie.has_fragments();
    let sanitize = match options.metadata {
        Metadata::Copy => None,
        Metadata::Sanitize(keep) => Some(keep),
    };
    let mut described = file.boxes;
    for (index, k) in kept.iter_mut().enumerate() {
        let stream = Some(k.track.stream_index);
        let trak_path = Path::top().with(FourCc(*b"moov")).with(FourCc(*b"trak"));
        if let Some(keep) = sanitize {
            let rewritten = describe(
                &mut source,
                limits,
                keep,
                &mut described,
                trak_path,
                &k.trak.tags,
                &mut |r| note(&mut k.gone, &mut k.gone_more, Gone::removed(r)),
            )?;
            muxer.set_boxes(index, rewrite(&rewritten)?.into_iter().collect())?;
        } else {
            let tags = copy_boxes(stream, trak_path, &k.trak.tags, &mut source, &mut retained)?;
            muxer.set_boxes(index, tags)?;
        }
        // Per-sample information stays valid when every sample is copied in
        // order; a fragmented source has more of it in its fragments, which
        // the output does not carry, so there it is left out.
        let stbl_path = trak_path
            .with(FourCc(*b"mdia"))
            .with(FourCc(*b"minf"))
            .with(FourCc(*b"stbl"));
        // A replaced stream's per-sample information describes the old
        // samples.
        if fragmented || k.replaced.is_some() {
            for header in &k.trak.sample_info {
                k.gone.push(Gone::not_carried(stbl_path, *header));
            }
            if let (Some(Replaced::Audio), Some(a)) = (k.replaced, &audio) {
                let samples = muxer.samples(index).ok_or_else(overflow)?;
                muxer.set_sample_table_boxes(index, roll_boxes(samples, a.roll))?;
            }
        } else {
            let info = copy_boxes(
                stream,
                stbl_path,
                &k.trak.sample_info,
                &mut source,
                &mut retained,
            )?;
            muxer.set_sample_table_boxes(index, info)?;
        }
    }
    let (mut movie_gone, mut movie_more) = (Vec::new(), 0u64);
    if let Some(keep) = sanitize {
        let rewritten = describe(
            &mut source,
            limits,
            keep,
            &mut described,
            Path::top().with(FourCc(*b"moov")),
            &file.tags,
            &mut |r| note(&mut movie_gone, &mut movie_more, Gone::removed(r)),
        )?;
        muxer.set_movie_boxes(rewrite(&rewritten)?.into_iter().collect())?;
    } else {
        let moov = Path::top().with(FourCc(*b"moov"));
        let left_out = &options.left_out_covers;
        let mut movie_tags = Vec::new();
        let mut found = Vec::new();
        for header in &file.tags {
            for tag in copy_boxes(
                None,
                moov,
                core::slice::from_ref(header),
                &mut source,
                &mut retained,
            )? {
                let at = header.payload_offset();
                let edited = if left_out.iter().any(|o| at < *o && *o < header.end()) {
                    without_covers(
                        Level::of(tag.kind),
                        &tag.payload,
                        at,
                        moov.with(tag.kind),
                        left_out,
                        &mut found,
                        &mut |g| note(&mut movie_gone, &mut movie_more, g),
                    )?
                } else {
                    Edited::Same
                };
                movie_tags.push(match edited {
                    Edited::Changed(payload) => RawBox {
                        kind: tag.kind,
                        payload,
                    },
                    Edited::Same | Edited::Removed => tag,
                });
            }
        }
        // A cover inside a box too large to copy is left out with it.
        let too_large = |offset: u64| {
            retained.too_large.iter().any(|(stream, s)| {
                stream.is_none() && s.header.offset < offset && offset < s.header.end()
            })
        };
        if left_out
            .iter()
            .any(|o| !found.contains(o) && !too_large(*o))
        {
            return Err(Error::NoSuchTrack.into());
        }
        muxer.set_movie_boxes(movie_tags)?;
    }
    muxer.set_file_type(file_type(&kept))?;

    // The copy pass lists every kept stream again, with the work the first
    // listing took; both passes together stay within `max_sample_work`.
    let copy_work = kept
        .iter()
        .try_fold(0u64, |sum, k| sum.checked_add(k.work))
        .ok_or_else(overflow)?;
    if copy_work > limits.max_sample_work.saturating_sub(budget.used()) {
        return Err(Error::LimitExceeded {
            limit: LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }
    let (dropped_boxes, not_listed) = report(&file, &movie_gone, &kept, &retained);
    let not_listed = kept
        .iter()
        .fold(not_listed.saturating_add(movie_more), |n, k| {
            n.saturating_add(k.gone_more)
        });
    drop(retained);

    let head = muxer.write_head(&mut out)?;

    // Pass 2: the samples, in the order of the chunks in `mdat`.
    let shared = RefCell::new(source);
    let mut budgets: Vec<(Limits, Budget)> = Vec::new();
    budgets
        .try_reserve_exact(kept.len())
        .map_err(|_| overflow())?;
    for k in &kept {
        let copy_limits = Limits {
            max_sample_work: k.work,
            ..*limits
        };
        budgets.push((copy_limits, Budget::new(&copy_limits)));
    }
    if replacement.is_some() || audio.is_some() {
        // Its own loop, so that the plain remux's stays as it was.
        copy_replaced(
            &movie,
            &kept,
            &muxer,
            &shared,
            &mut budgets,
            replacement.map(|r| r.data),
            audio.map(|a| a.data),
            &mut out,
        )?;
    } else {
        let mut cursors: Vec<Cursor<'_, S>> = Vec::new();
        cursors
            .try_reserve_exact(kept.len())
            .map_err(|_| overflow())?;
        for (k, (copy_limits, budget)) in kept.iter().zip(budgets.iter_mut()) {
            cursors.push(Cursor {
                packets: movie.packets(k.track.index, Shared(&shared), copy_limits, budget)?,
                copied: 0,
            });
        }
        let mut window = CopyWindow::new();
        muxer.for_each_chunk(|track, samples| -> Result<(), ExtractError> {
            let cursor = cursors.get_mut(track).ok_or_else(overflow)?;
            for _ in 0..samples {
                let packet = cursor.packets.next().ok_or(ExtractError::SourceChanged)??;
                if muxer.sample_size(track, cursor.copied) != Some(packet.size) {
                    return Err(ExtractError::SourceChanged);
                }
                window.copy(&mut Shared(&shared), packet.offset, packet.size, &mut out)?;
                cursor.copied = cursor
                    .copied
                    .checked_add(1)
                    .ok_or(ExtractError::SourceChanged)?;
            }
            Ok(())
        })?;
        for (index, cursor) in cursors.iter().enumerate() {
            if muxer.samples(index) != Some(cursor.copied) {
                return Err(ExtractError::SourceChanged);
            }
        }
        drop((cursors, window));
    }
    out.flush()?;

    let mut tracks = Vec::new();
    for (index, k) in kept.iter().enumerate() {
        tracks.push(RemuxTrack {
            stream_index: k.track.stream_index,
            source_track_id: k.track.track_id,
            track_id: u32::try_from(index)
                .ok()
                .and_then(|i| i.checked_add(1))
                .ok_or_else(overflow)?,
            kind: k.track.kind,
            codec: k.codec,
            samples: muxer.samples(index).ok_or_else(overflow)?,
            timescale: muxer.header(index).ok_or_else(overflow)?.timescale,
            duration: muxer.duration(index).ok_or_else(overflow)?,
            replaced: k.replaced.is_some(),
        });
    }
    Ok(RemuxSummary {
        tracks,
        dropped_tracks,
        dropped_boxes,
        dropped_boxes_not_listed: not_listed,
        cleared,
        source_moov_first: file.moov_first,
        bytes_written: head.total(),
    })
}

/// Rewrites `source` as an MP4 file written to `out` keeping only
/// allowlisted boxes (`docs/design/sanitize.md`): [`remux_mp4`] with
/// [`Metadata::Sanitize`]. No `udta` or `meta` box of the movie or a track is
/// written, only allowlisted sample entry children, no compressor name, and
/// the header fields `keep` does not keep are cleared. The summary lists
/// every box removed, with what a metadata item describes, and every field
/// cleared. Every stream that MP4 can carry is kept, with every sample.
pub fn sanitize_mp4<S: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    keep: Keep,
    out: W,
) -> Result<RemuxSummary, ExtractError> {
    let options = RemuxOptions {
        metadata: Metadata::Sanitize(keep),
        ..RemuxOptions::default()
    };
    remux_mp4(source, limits, allowlist, &options, out)
}

fn dropped(track: &Track, reason: TrackDropReason) -> DroppedTrack {
    DroppedTrack {
        stream_index: track.stream_index,
        track_id: track.track_id,
        reason,
    }
}

/// One kept stream's listing in the copy pass.
struct Cursor<'a, S: Source> {
    packets: Packets<'a, Shared<'a, S>>,
    copied: u32,
}

/// A replacement's data in the copy pass: its next sample, that sample's
/// offset, and a window of its own.
struct Feed<R> {
    data: R,
    copied: u32,
    offset: u64,
    window: CopyWindow,
}

/// Pass 2 with replaced streams: the source's kept streams are listed
/// again as in the plain remux; each replacement's samples are read from
/// its data (`video`, `audio`) in order, through a window of their own.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn copy_replaced<S: Source, R: Source, W: Write>(
    movie: &Movie,
    kept: &[Kept<'_>],
    muxer: &Muxer,
    shared: &RefCell<S>,
    budgets: &mut [(Limits, Budget)],
    video: Option<R>,
    audio: Option<R>,
    out: &mut W,
) -> Result<(), ExtractError> {
    let feed = |data| Feed {
        data,
        copied: 0,
        offset: 0,
        window: CopyWindow::new(),
    };
    let (mut video, mut audio) = (video.map(feed), audio.map(feed));
    // `None` for a replaced stream.
    let mut cursors: Vec<Option<Cursor<'_, S>>> = Vec::new();
    cursors
        .try_reserve_exact(kept.len())
        .map_err(|_| overflow())?;
    for (k, (copy_limits, budget)) in kept.iter().zip(budgets.iter_mut()) {
        cursors.push(if k.replaced.is_some() {
            None
        } else {
            Some(Cursor {
                packets: movie.packets(k.track.index, Shared(shared), copy_limits, budget)?,
                copied: 0,
            })
        });
    }
    let mut window = CopyWindow::new();
    muxer.for_each_chunk(|track, samples| -> Result<(), ExtractError> {
        let cursor = cursors.get_mut(track).ok_or_else(overflow)?;
        for _ in 0..samples {
            let Some(cursor) = cursor.as_mut() else {
                let feed = match kept.get(track).and_then(|k| k.replaced) {
                    Some(Replaced::Video) => video.as_mut(),
                    Some(Replaced::Audio) => audio.as_mut(),
                    None => None,
                }
                .ok_or(ExtractError::SourceChanged)?;
                let size = muxer
                    .sample_size(track, feed.copied)
                    .ok_or(ExtractError::SourceChanged)?;
                feed.window
                    .copy(&mut feed.data, feed.offset, size, &mut *out)?;
                feed.offset = feed
                    .offset
                    .checked_add(u64::from(size))
                    .ok_or(ExtractError::SourceChanged)?;
                feed.copied = feed
                    .copied
                    .checked_add(1)
                    .ok_or(ExtractError::SourceChanged)?;
                continue;
            };
            let packet = cursor.packets.next().ok_or(ExtractError::SourceChanged)??;
            if muxer.sample_size(track, cursor.copied) != Some(packet.size) {
                return Err(ExtractError::SourceChanged);
            }
            window.copy(&mut Shared(shared), packet.offset, packet.size, &mut *out)?;
            cursor.copied = cursor
                .copied
                .checked_add(1)
                .ok_or(ExtractError::SourceChanged)?;
        }
        Ok(())
    })?;
    for (index, cursor) in cursors.iter().enumerate() {
        let copied = match (cursor, kept.get(index).and_then(|k| k.replaced)) {
            (Some(c), _) => Some(c.copied),
            (None, Some(Replaced::Video)) => video.as_ref().map(|f| f.copied),
            (None, Some(Replaced::Audio)) => audio.as_ref().map(|f| f.copied),
            (None, None) => None,
        };
        if copied.is_none() || muxer.samples(index) != copied {
            return Err(ExtractError::SourceChanged);
        }
    }
    Ok(())
}

/// A stream that will be copied, before its samples are listed.
struct Plan<'m> {
    track: &'m Track,
    trak: &'m TrakScan,
    header: TrackHeader,
    /// The source's edit list as stored.
    edits: Vec<Edit>,
    /// The track header duration as stored, when it is used.
    declared: Option<u64>,
    /// Metadata bytes charged for the plan, refunded if the stream is
    /// dropped later.
    retained: u64,
    /// Boxes of the sample entry that the output leaves out.
    gone: Vec<Gone>,
    /// Header fields that sanitization clears.
    cleared: Vec<Cleared>,
    /// The stream's samples are a replacement's.
    replaced: Option<Replaced>,
}

/// A box of the source that the output leaves out, and why.
struct Gone {
    skipped: Skipped,
    reason: BoxDropReason,
    kind: Option<MetadataKind>,
    name: Option<String>,
}

impl Gone {
    fn not_carried(parent: Path, header: BoxHeader) -> Self {
        Self {
            skipped: Skipped { parent, header },
            reason: BoxDropReason::NotCarried,
            kind: None,
            name: None,
        }
    }

    fn removed(r: Removed) -> Self {
        Self {
            skipped: r.skipped,
            reason: BoxDropReason::Sanitized,
            kind: r.kind,
            name: r.name,
        }
    }

    fn sanitized(parent: Path, header: BoxHeader) -> Self {
        Self {
            skipped: Skipped { parent, header },
            reason: BoxDropReason::Sanitized,
            kind: Some(MetadataKind::Other),
            name: None,
        }
    }
}

/// Adds `gone` to `list` while it is shorter than [`MAX_REPORTED`], else
/// counts it in `more`: the summary lists no more than that, and a source
/// with many metadata items keeps no more in memory.
fn note(list: &mut Vec<Gone>, more: &mut u64, gone: Gone) {
    if list.len() < MAX_REPORTED {
        list.push(gone);
    } else {
        *more = more.saturating_add(1);
    }
}

/// A stream whose tables are built.
struct Kept<'m> {
    track: &'m Track,
    trak: &'m TrakScan,
    /// Sample work of its listing.
    work: u64,
    gone: Vec<Gone>,
    /// Removed metadata beyond those in `gone`.
    gone_more: u64,
    /// The source's edit list as stored, and the first decode time.
    edits: Vec<Edit>,
    first_dts: i64,
    /// Where the frame presented last ends, in media ticks from the first
    /// decode time (0 for a replacement's samples).
    composition_end: u64,
    /// The output's codec: the source's, or the replacement's.
    codec: Codec,
    /// The stream's samples are a replacement's.
    replaced: Option<Replaced>,
    /// The track header duration set from the source, in its movie ticks.
    declared: Option<u64>,
}

/// Metadata bytes kept so far, against `max_retained_bytes`, and the
/// optional boxes left out for size.
struct Retained {
    left: u64,
    too_large: Vec<(Option<usize>, Skipped)>,
}

impl Retained {
    fn new(limits: &Limits) -> Self {
        Self {
            left: limits.max_retained_bytes,
            too_large: Vec::new(),
        }
    }

    /// Charges `bytes`, or fails when they exceed what is left.
    fn charge(&mut self, bytes: u64, offset: u64) -> Result<(), Error> {
        self.left = self.left.checked_sub(bytes).ok_or(Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            offset,
        })?;
        Ok(())
    }

    /// The payload of `header` (with a `uuid` box's extended type), or
    /// `None` when it is larger than what is left.
    fn read<S: Source>(
        &mut self,
        source: &mut S,
        header: &BoxHeader,
    ) -> Result<Option<Vec<u8>>, Error> {
        let uuid = if header.kind.0 == *b"uuid" { 16 } else { 0 };
        let start = header.payload_offset().saturating_sub(uuid);
        let len = header.end().saturating_sub(start);
        if len > self.left {
            return Ok(None);
        }
        let size = usize::try_from(len).map_err(|_| overflow())?;
        // A read of at least 64 KiB, or to the end of the source, goes
        // straight to this buffer through a `BufferedSource`; a smaller one
        // would grow its window and leave a second copy of the box there
        // for the rest of the remux. So a box above 4 KiB is read with that
        // much and trimmed, before the copy window exists.
        let available = source.len()?.saturating_sub(start);
        let read = if size > SMALL_BOX {
            usize::try_from(available.min(DIRECT_READ as u64))
                .map_err(|_| overflow())?
                .max(size)
        } else {
            size
        };
        let mut payload = Vec::new();
        if payload.try_reserve_exact(read).is_err() {
            return Ok(None);
        }
        payload.resize(read, 0);
        source.read_exact_at(start, &mut payload)?;
        payload.truncate(size);
        payload.shrink_to_fit();
        self.left = self.left.saturating_sub(len);
        Ok(Some(payload))
    }
}

/// Boxes as stored; a box larger than what `max_retained_bytes` leaves is
/// listed as dropped instead.
fn copy_boxes<S: Source>(
    stream: Option<usize>,
    parent: Path,
    tags: &[BoxHeader],
    source: &mut S,
    retained: &mut Retained,
) -> Result<Vec<RawBox>, Error> {
    let mut boxes = Vec::new();
    for header in tags {
        match retained.read(source, header)? {
            Some(payload) => boxes.push(RawBox {
                kind: header.kind,
                payload,
            }),
            None => retained.too_large.push((
                stream,
                Skipped {
                    parent,
                    header: *header,
                },
            )),
        }
    }
    Ok(boxes)
}

/// Where a box sits in the item list layout that [`without_covers`] walks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// A user data box (`udta`, ISO/IEC 14496-12 8.10.1).
    UserData,
    /// A metadata box (`meta`, 8.11.1, or QuickTime's without version and
    /// flags).
    Meta,
    /// An item list (`ilst`).
    List,
    /// One entry of an item list: its `data` boxes and others.
    Item,
    /// Anything else, copied as stored.
    Other,
}

impl Level {
    /// The level of a child of the movie box.
    fn of(kind: FourCc) -> Self {
        match &kind.0 {
            b"udta" => Self::UserData,
            b"meta" => Self::Meta,
            _ => Self::Other,
        }
    }

    /// The level of a child of `kind` at this level.
    fn child(self, kind: FourCc) -> Self {
        match (self, &kind.0) {
            (Self::UserData, b"meta") => Self::Meta,
            (Self::UserData | Self::Meta, b"ilst") => Self::List,
            (Self::List, kind) if kind != b"uuid" => Self::Item,
            _ => Self::Other,
        }
    }
}

/// What [`without_covers`] did to a box.
enum Edited {
    Same,
    Removed,
    Changed(Vec<u8>),
}

/// The payload of a box at `level` that starts at `at` in the file
/// (under `path`, its own type included), without the item list `data`
/// boxes whose values start at an offset of `left_out`, and without an
/// item left with no `data` box. Each offset it leaves out is added to
/// `found`, and each box to `gone`: the item, or the `data` box of an
/// item that stays. A child list that does not parse ends the walk, the
/// bytes after it kept as stored.
fn without_covers(
    level: Level,
    payload: &[u8],
    at: u64,
    path: Path,
    left_out: &[u64],
    found: &mut Vec<u64>,
    gone: &mut dyn FnMut(Gone),
) -> Result<Edited, Error> {
    let start = match level {
        Level::Meta if payload.get(4..8) != Some(b"hdlr".as_slice()) => 4,
        Level::UserData | Level::Meta | Level::List | Level::Item => 0,
        Level::Other => return Ok(Edited::Same),
    };
    let mut out = payload.get(..start).ok_or_else(overflow)?.to_vec();
    let mut removed = Vec::new();
    let (mut changed, mut data_kept) = (false, false);
    let mut pos = start;
    while let Some(child) = payload.get(pos..).and_then(|rest| {
        let offset = at.checked_add(u64::try_from(pos).ok()?)?;
        let room = u64::try_from(rest.len()).ok()?;
        match parse_box_header(rest, offset, room) {
            Ok(Parsed::Done(header)) => Some(header),
            _ => None,
        }
    }) {
        let end = usize::try_from(child.size)
            .ok()
            .and_then(|size| pos.checked_add(size))
            .ok_or_else(overflow)?;
        let bytes = payload.get(pos..end).ok_or_else(overflow)?;
        pos = end;
        let edited = if level == Level::Item && child.kind.0 == *b"data" {
            let value = child.payload_offset().saturating_add(8);
            if left_out.contains(&value) {
                found.push(value);
                Edited::Removed
            } else {
                data_kept = true;
                Edited::Same
            }
        } else {
            let body = bytes
                .get(usize::from(child.header_len)..)
                .ok_or_else(overflow)?;
            without_covers(
                level.child(child.kind),
                body,
                child.payload_offset(),
                path.with(child.kind),
                left_out,
                found,
                gone,
            )?
        };
        changed |= !matches!(edited, Edited::Same);
        match edited {
            Edited::Same => out.extend_from_slice(bytes),
            Edited::Removed => removed.push(child),
            Edited::Changed(body) => {
                let size = u32::try_from(body.len().saturating_add(8)).map_err(|_| overflow())?;
                out.extend(size.to_be_bytes());
                out.extend(child.kind.0);
                out.extend(body);
            }
        }
    }
    if !changed {
        return Ok(Edited::Same);
    }
    if level == Level::Item && !data_kept {
        return Ok(Edited::Removed);
    }
    for header in removed {
        gone(Gone::not_carried(path, header));
    }
    out.extend_from_slice(payload.get(pos..).unwrap_or_default());
    Ok(Edited::Changed(out))
}

/// A required box's payload as stored (a sample entry child): it is copied
/// whole or the remux fails.
fn required<S: Source>(
    source: &mut S,
    header: &BoxHeader,
    retained: &mut Retained,
) -> Result<Vec<u8>, Error> {
    retained.read(source, header)?.ok_or(Error::LimitExceeded {
        limit: LimitKind::RetainedBytes,
        offset: header.offset,
    })
}

/// Checks that `track` can be copied to MP4 and builds its header from what
/// it needs: its sample entry and edit list.
fn plan<'m, S: Source>(
    track: &'m Track,
    trak: &'m TrakScan,
    policy: Metadata,
    source: &mut S,
    limits: &Limits,
    retained: &mut Retained,
    replace: Option<Replace<'_>>,
) -> Result<Plan<'m>, Error> {
    let start = retained.left;
    let keep = match policy {
        Metadata::Copy => None,
        Metadata::Sanitize(keep) => Some(keep),
    };
    let mut cleared = Vec::new();
    let mut clear = |field| {
        cleared.push(Cleared {
            stream_index: Some(track.stream_index),
            field,
        });
    };
    // None of a replaced stream's samples or sample entry is read.
    if replace.is_none() {
        crate::clip::conforming(track)?;
        if track.descriptions != 1 {
            return Err(unsupported("more than one sample description"));
        }
        if track.external_data {
            return Err(unsupported("the track's data is in another file"));
        }
        if !track.tables_valid {
            return Err(unsupported("the track's sample tables are invalid"));
        }
    }
    if track.timescale == 0 {
        return Err(unsupported("media timescale 0"));
    }
    if replace.is_none()
        && (trak.entry_children.len() >= crate::scan::MAX_LISTED
            || trak.wave_children.len() >= crate::scan::MAX_LISTED)
    {
        return Err(unsupported("too many boxes in the sample entry"));
    }
    let description = &track.description;
    let flags = trak.flags.unwrap_or(TrackFlags::DEFAULT);
    let mut gone = Vec::new();
    let entry_path = Path::top()
        .with(FourCc(*b"moov"))
        .with(FourCc(*b"trak"))
        .with(FourCc(*b"mdia"))
        .with(FourCc(*b"minf"))
        .with(FourCc(*b"stbl"))
        .with(FourCc(*b"stsd"))
        .with(description.fourcc);
    let (media, entry) = match (replace, track.kind) {
        (
            Some(Replace::Video {
                entry,
                width,
                height,
            }),
            TrackKind::Video,
        ) => (
            TrackMedia::Video {
                matrix: replaced_matrix(track.matrix, track.presentation_size, (width, height))?,
                width,
                height,
            },
            entry.to_vec(),
        ),
        (Some(Replace::Audio { entry, .. }), TrackKind::Audio) => (
            TrackMedia::Audio {
                volume: flags.volume,
            },
            entry.to_vec(),
        ),
        (Some(Replace::Video { .. }), _) => {
            return Err(unsupported("the replaced stream is not a video stream"));
        }
        (Some(Replace::Audio { .. }), _) => {
            return Err(unsupported("the replaced stream is not an audio stream"));
        }
        (None, TrackKind::Video) => {
            let config: &[u8; 4] = match (description.codec, &description.fourcc.0) {
                (Codec::H264, b"avc1" | b"avc3") => b"avcC",
                (Codec::Hevc, b"hvc1" | b"hev1") => b"hvcC",
                (Codec::Av1, b"av01") => b"av1C",
                (Codec::Vp9, b"vp09") => b"vpcC",
                (Codec::ProRes, _) => {
                    return Err(unsupported("ProRes has no MP4 sample entry"));
                }
                _ => {
                    return Err(unsupported(
                        "video other than H.264, HEVC, AV1 or VP9 in their MP4 sample entries",
                    ));
                }
            };
            if !description
                .codec_config
                .as_ref()
                .is_some_and(|c| &c.kind.0 == config)
            {
                return Err(unsupported(
                    "the video sample entry has no codec configuration",
                ));
            }
            let (Some(width), Some(height)) = (description.width, description.height) else {
                return Err(unsupported("the video sample entry has no dimensions"));
            };
            // Every child as stored: the codec configuration and whatever
            // else describes the coded video (HDR, Dolby Vision, field
            // order, ...); when sanitizing, only the allowlisted ones.
            let mut children = Vec::new();
            for child in &trak.entry_children {
                if keep.is_some() && !VISUAL_KEPT.contains(&&child.kind.0) {
                    gone.push(Gone::sanitized(entry_path, *child));
                    continue;
                }
                let mut payload = required(source, child, retained)?;
                if keep.is_some() {
                    // Allowlisted boxes written with only their defined fields.
                    match entry_box(child.kind, payload) {
                        EntryBox::Write(out, field) => {
                            if let Some(field) = field {
                                clear(field);
                            }
                            payload = out;
                        }
                        EntryBox::Remove => {
                            gone.push(Gone::sanitized(entry_path, *child));
                            continue;
                        }
                        EntryBox::Unsupported(reason) => return Err(unsupported(reason)),
                    }
                }
                children.push(RawBox {
                    kind: child.kind,
                    payload,
                });
            }
            if !children.iter().any(|c| &c.kind.0 == config) {
                return Err(unsupported(
                    "the codec configuration is not among the sample entry's boxes",
                ));
            }
            let mut compressor = trak.compressor_name.unwrap_or([0; 32]);
            if keep.is_some() && compressor != [0; 32] {
                // The compressor name often names the device or encoder.
                compressor = [0; 32];
                clear(ClearedField::CompressorName);
            }
            let depth = trak.depth.unwrap_or(0x0018);
            let entry = visual_sample_entry(
                description.fourcc,
                width,
                height,
                &compressor,
                depth,
                &children,
            )?;
            let media = TrackMedia::Video {
                matrix: track.matrix,
                width: track.presentation_size.0,
                height: track.presentation_size.1,
            };
            (media, entry)
        }
        (None, TrackKind::Audio) => {
            if description.codec == Codec::Pcm {
                return Err(unsupported("PCM audio is not copied to MP4"));
            }
            let fourcc = description.fourcc;
            let config = audio::config_kind(fourcc)
                .filter(|_| description.codec != Codec::Other)
                .ok_or(unsupported(
                    "audio other than AAC, MP3, Opus, FLAC, ALAC, AC-3 or E-AC-3 in their MP4 \
                     sample entries",
                ))?;
            let codec_config = description
                .codec_config
                .as_ref()
                .filter(|c| c.kind == config)
                .ok_or(unsupported(
                    "the audio sample entry has no codec configuration",
                ))?;
            let channels = description
                .channels
                .and_then(|c| u16::try_from(c).ok())
                .ok_or(unsupported(
                    "channel count outside the version 0 sound entry",
                ))?;
            let mut rate = description
                .sample_rate
                .ok_or(unsupported("the audio sample entry has no sample rate"))?;
            if matches!(description.codec, Codec::Flac | Codec::Alac) {
                rate = audio::entry_rate(rate);
            }
            // An ISO version 0 entry: the configuration box first, then the
            // other children as stored. QuickTime's `wave` wrapper holds the
            // configuration box (and its format, `frma`); anything else in
            // it is reported, as is QuickTime's channel layout `chan`, which
            // MP4 does not define (ISO/IEC 14496-12 has `chnl`).
            retained.charge(
                u64::try_from(codec_config.data.len()).unwrap_or(u64::MAX),
                0,
            )?;
            let mut payload = codec_config.data.clone();
            if keep.is_some() && config.0 == *b"dfLa" {
                let (only, changed) = audio::dfla_streaminfo_only(&payload)
                    .ok_or(unsupported("the FLAC configuration does not parse"))?;
                if changed {
                    clear(ClearedField::SampleEntryExtras);
                }
                payload = only;
            }
            let mut children = vec![RawBox {
                kind: config,
                payload,
            }];
            for child in &trak.entry_children {
                match &child.kind.0 {
                    kind if *kind == config.0 => {}
                    b"wave" => {
                        let wave_path = entry_path.with(child.kind);
                        for inner in &trak.wave_children {
                            if !matches!(&inner.kind.0, b"frma" | b"esds" | b"\0\0\0\0")
                                && inner.kind != description.fourcc
                            {
                                gone.push(Gone::not_carried(wave_path, *inner));
                            }
                        }
                    }
                    b"chan" => gone.push(Gone::not_carried(entry_path, *child)),
                    kind if keep.is_some() && !AUDIO_KEPT.contains(&kind) => {
                        gone.push(Gone::sanitized(entry_path, *child));
                    }
                    _ => {
                        let mut payload = required(source, child, retained)?;
                        if keep.is_some() {
                            match entry_box(child.kind, payload) {
                                EntryBox::Write(out, field) => {
                                    if let Some(field) = field {
                                        clear(field);
                                    }
                                    payload = out;
                                }
                                EntryBox::Remove => {
                                    gone.push(Gone::sanitized(entry_path, *child));
                                    continue;
                                }
                                EntryBox::Unsupported(reason) => return Err(unsupported(reason)),
                            }
                        }
                        children.push(RawBox {
                            kind: child.kind,
                            payload,
                        });
                    }
                }
            }
            let entry = audio_sample_entry(
                fourcc,
                channels,
                audio::sample_size(description)
                    .ok_or(unsupported("the FLAC or ALAC configuration does not parse"))?,
                rate,
                &children,
            )?;
            (
                TrackMedia::Audio {
                    volume: flags.volume,
                },
                entry,
            )
        }
        (None, TrackKind::Subtitle) => return Err(unsupported("timed text is not copied to MP4")),
        (None, TrackKind::Timecode) => {
            return Err(unsupported("timecode tracks are not copied to MP4"));
        }
        (None, TrackKind::Data) => return Err(unsupported("data tracks are not copied to MP4")),
    };
    // The source's, or for a replaced stream its translation for the new
    // size (`replaced_matrix`).
    let mut matrix = match media {
        TrackMedia::Video { matrix, .. } => matrix,
        TrackMedia::Audio { .. } => track.matrix,
    };
    let mut language = track_language(track, trak);
    let (mut created, mut media_created) = (track.creation_time, track.media_creation_time);
    if let Some(keep) = keep {
        if !keep.rotation && matrix != UNITY {
            matrix = UNITY;
            clear(ClearedField::Rotation);
        }
        if !keep.language && language != *b"und" {
            language = *b"und";
            clear(ClearedField::Language);
        }
        if !keep.creation_times && (created != 0 || media_created != 0) {
            (created, media_created) = (0, 0);
            clear(ClearedField::CreationTime);
        }
    }
    let media = match media {
        TrackMedia::Video { width, height, .. } => TrackMedia::Video {
            matrix,
            width,
            height,
        },
        audio @ TrackMedia::Audio { .. } => audio,
    };
    // A replaced stream's edits are computed from the probe's model of the
    // source's (`ReplacedEdits`).
    let edits = match &trak.elst {
        Some(elst) if replace.is_none() => read_elst(source, elst, limits, &mut |bytes| {
            retained.charge(bytes, elst.offset)
        })?,
        _ => Vec::new(),
    };
    Ok(Plan {
        track,
        trak,
        header: TrackHeader {
            media,
            timescale: match replace {
                Some(Replace::Audio { timescale, .. }) => timescale,
                _ => track.timescale,
            },
            language,
            creation_time: created,
            media_creation_time: media_created,
            flags: flags.flags,
            layer: flags.layer,
            alternate_group: flags.alternate_group,
            edits: Vec::new(),
            duration: None,
            media_duration: None,
            sample_entry: entry,
            sample_table_boxes: Vec::new(),
            boxes: Vec::new(),
        },
        edits,
        declared: trak.duration.filter(|d| *d != 0),
        retained: start.saturating_sub(retained.left),
        gone,
        cleared,
        replaced: replace.map(|r| match r {
            Replace::Video { .. } => Replaced::Video,
            Replace::Audio { .. } => Replaced::Audio,
        }),
    })
}

/// What a replacement gives a stream's header.
#[derive(Clone, Copy, Debug)]
enum Replace<'r> {
    Video {
        entry: &'r [u8],
        width: u32,
        height: u32,
    },
    Audio {
        entry: &'r [u8],
        timescale: u32,
    },
}

/// Checks that `stream_index` names a stream of `kind` of `movie` that a
/// replacement can take the place of: one the allowlist keeps, among
/// `options.streams` when they are named; a video stream with an edit list
/// of the simple model or none.
fn check_replaced(
    movie: &Movie,
    stream_index: usize,
    kind: TrackKind,
    options: &RemuxOptions,
) -> Result<(), Error> {
    if let Some(d) = movie
        .dropped
        .iter()
        .find(|d| d.stream_index == stream_index)
    {
        return Err(Error::CodecNotAllowed {
            track_id: d.track_id,
            fourcc: d.fourcc,
        });
    }
    let track = movie
        .tracks
        .iter()
        .find(|t| t.stream_index == stream_index)
        .ok_or(Error::NoSuchTrack)?;
    if track.kind != kind {
        return Err(unsupported(if kind == TrackKind::Video {
            "the replaced stream is not a video stream"
        } else {
            "the replaced stream is not an audio stream"
        }));
    }
    if options
        .streams
        .as_ref()
        .is_some_and(|s| !s.contains(&stream_index))
    {
        return Err(unsupported(
            "the replaced stream is not among the selected streams",
        ));
    }
    if kind == TrackKind::Audio {
        return Ok(());
    }
    match track.edit_list {
        EditList::None | EditList::Simple { .. } => Ok(()),
        EditList::Multiple | EditList::Unsupported => Err(unsupported(
            "the replaced stream's edit list is outside the simple model \
             (at most one empty edit, then one media edit at rate 1)",
        )),
    }
}

/// The smallest composition time of `track`'s samples, from its sample
/// tables (no sample is read), charged against `budget` like any listing;
/// `None` when the tables are invalid or the data is in another file, so
/// that the edit's media time stands for it. A failure of the listing (a
/// limit, a read error) fails the remux.
fn first_composition<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Option<i64>, Error> {
    if !track.tables_valid || track.external_data {
        return Ok(None);
    }
    let mut first: Option<i64> = None;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let cts = packet?.cts;
        first = Some(first.map_or(cts, |f| f.min(cts)));
    }
    Ok(first)
}

/// The replaced stream's matrix: the source track's, whose translation
/// (`x`, `y`) places the picture for the source's presentation size. When
/// the size changes and the matrix is a quarter turn or a flip (`a`, `b`,
/// `c`, `d` each 0 or ±1, no perspective) whose translation is the one
/// that keeps that size's picture in the positive quadrant (ISO/IEC
/// 14496-12 6.2.2: what a rotation tag writes), the translation is
/// computed again for the new size; any other translated matrix fails
/// with `Unsupported`, as its meaning for another size is unknown.
fn replaced_matrix(matrix: [i32; 9], old: (u32, u32), new: (u32, u32)) -> Result<[i32; 9], Error> {
    let [a, b, u, c, d, v, x, y, w] = matrix;
    if old == new || (x == 0 && y == 0) {
        return Ok(matrix);
    }
    const ONE: i32 = 0x0001_0000;
    let unit = |m: i32| m == 0 || m == ONE || m == -ONE;
    // A coefficient of -1 maps that axis to negative coordinates, which the
    // translation moves back by that axis's size (16.16 fixed point).
    let translation = |(width, height): (u32, u32)| -> Option<(i32, i32)> {
        let back = |m: i32, size: u32| if m < 0 { u64::from(size) } else { 0 };
        let tx = back(a, width).checked_add(back(c, height))?;
        let ty = back(b, width).checked_add(back(d, height))?;
        Some((i32::try_from(tx).ok()?, i32::try_from(ty).ok()?))
    };
    let axis_aligned = [a, b, c, d].into_iter().all(unit) && u == 0 && v == 0 && w == 0x4000_0000;
    if axis_aligned && translation(old) == Some((x, y)) {
        if let Some((x, y)) = translation(new) {
            return Ok([a, b, u, c, d, v, x, y, w]);
        }
    }
    Err(unsupported(
        "the replaced stream's matrix has a translation that does not follow a new size",
    ))
}

/// Pass 1 for the replaced stream: pushes the replacement's samples, each
/// lasting up to the next one's decode time; returns the first decode time.
fn list_replaced(
    muxer: &mut Muxer,
    index: usize,
    samples: &[ReplacementSample],
    last_duration: u32,
) -> Result<i64, ExtractError> {
    for (sample, duration) in durations(samples, last_duration) {
        muxer.push_sample(
            index,
            Sample {
                size: sample.size,
                duration,
                sync: sample.sync,
                composition_offset: sample
                    .presentation_time
                    .checked_sub(sample.decode_time)
                    .ok_or_else(overflow)?,
            },
        )?;
    }
    muxer.finish_track(index);
    Ok(samples
        .first()
        .ok_or(unsupported("the replacement has no samples"))?
        .decode_time)
}

/// Pass 1 for the replaced audio stream: pushes its samples, every one a
/// sync sample.
fn list_audio<R>(
    muxer: &mut Muxer,
    index: usize,
    audio: &AudioReplacement<'_, R>,
) -> Result<(), ExtractError> {
    for sample in audio.samples {
        muxer.push_sample(
            index,
            Sample {
                size: sample.size,
                duration: sample.duration,
                sync: true,
                composition_offset: 0,
            },
        )?;
    }
    muxer.finish_track(index);
    Ok(())
}

/// A media edit's duration: the source's, in its movie ticks, or computed,
/// in media ticks.
#[derive(Clone, Copy, Debug)]
enum Span {
    Movie(u64),
    Media(u64),
}

/// The replaced stream's edit list before it is put in movie ticks: the
/// source's empty edit (movie ticks) plus `extra` media ticks, then one
/// media edit at rate 1 from `media_time` (output media ticks).
#[derive(Clone, Copy, Debug)]
struct ReplacedEdits {
    empty: u64,
    extra: u64,
    media_time: i64,
    segment: Span,
}

impl ReplacedEdits {
    /// The edits that present the replacement's time `P` (output media
    /// time `P - first`) at the movie time where the source's edit list
    /// presents its media time `P`; `None` for no edit list, when the
    /// output's media time is the source's (`first` is 0).
    ///
    /// A frame at `P` is presented at the empty edit plus `P` less the
    /// media edit's start, so the start may move by any amount the empty
    /// edit grows by. It moves to keep the gap the source's own edit leaves
    /// before its first frame: the source's media edit starts at media time
    /// `M`, its first frame is presented at `s0` (its smallest composition
    /// time `source_first` from the sample tables, or `M` when that is
    /// later or unknown), and the replacement's media edit starts as far
    /// before its first frame `p_min` as that, `M + max(0, p_min - s0)`, but
    /// never before its first decode time `first`. A player that shows the
    /// first frame of a gap at the edit's start (`ffmpeg`, observed as a
    /// black box) then shows the replacement where it shows the source;
    /// the identity replacement gives the source's edit list. Without an
    /// edit list there is no such gap (media time 0 is movie time 0), and
    /// the media edit starts at the first frame.
    ///
    /// `end` is where the replacement's presentation ends, in output media
    /// ticks: the presentation time of the frame presented last plus its
    /// duration, less `first`; a media edit computed here lasts to it, so
    /// that every frame is presented as without an edit list.
    fn plan(
        track: &Track,
        first: i64,
        p_min: i64,
        source_first: Option<i64>,
        end: u64,
    ) -> Result<Option<Self>, Error> {
        let nothing = || unsupported("nothing of the replacement is presented");
        let ticks = |v: i64| u64::try_from(v).map_err(|_| overflow());
        let (empty, media_time, segment, s0) = match track.edit_list {
            // Media time is movie time.
            EditList::None if first == 0 => return Ok(None),
            EditList::None => (0, 0, 0, 0),
            EditList::Simple {
                empty,
                media_time,
                segment,
            } => (
                empty,
                media_time,
                segment,
                source_first.map_or(media_time, |c| c.max(media_time)),
            ),
            EditList::Multiple | EditList::Unsupported => {
                return Err(unsupported(
                    "the replaced stream's edit list is outside the simple model",
                ));
            }
        };
        let later = p_min.checked_sub(s0).ok_or_else(overflow)?.max(0);
        let start = media_time
            .checked_add(later)
            .ok_or_else(overflow)?
            .max(first);
        let extra = ticks(start.checked_sub(media_time).ok_or_else(overflow)?)?;
        let media_time = start.checked_sub(first).ok_or_else(overflow)?;
        let segment = if segment == 0 {
            // To the end of the presentation.
            let rest = end
                .checked_sub(ticks(media_time)?)
                .filter(|r| *r > 0)
                .ok_or_else(nothing)?;
            Span::Media(rest)
        } else {
            Span::Movie(segment)
        };
        Ok(Some(Self {
            empty,
            extra,
            media_time,
            segment,
        }))
    }

    /// Whether a value computed in media ticks rounds in `movie_timescale`.
    fn rounds(&self, movie_timescale: u32, timescale: u32) -> bool {
        let media = match self.segment {
            Span::Media(m) => Some(m),
            Span::Movie(_) => None,
        };
        [Some(self.extra), media].into_iter().flatten().any(|v| {
            u128::from(v)
                .saturating_mul(u128::from(movie_timescale))
                .checked_rem(u128::from(timescale))
                .is_some_and(|r| r != 0)
        })
    }

    /// The edit list in `movie_timescale` ticks, `factor` times the
    /// source's.
    fn edits(&self, factor: u32, movie_timescale: u32, timescale: u32) -> Result<Vec<Edit>, Error> {
        let extra = rescale(self.extra, timescale, movie_timescale)?;
        let empty = self
            .empty
            .checked_mul(u64::from(factor))
            .and_then(|e| e.checked_add(extra))
            .ok_or_else(overflow)?;
        let segment = match self.segment {
            Span::Media(m) => rescale(m, timescale, movie_timescale)?,
            Span::Movie(s) => s
                .checked_mul(u64::from(factor))
                .ok_or_else(overflow)?
                .checked_sub(extra)
                .filter(|s| *s > 0)
                .ok_or(unsupported("nothing of the replacement is presented"))?,
        };
        let mut list = Vec::with_capacity(2);
        if empty > 0 {
            list.push(Edit {
                duration: empty,
                media_time: -1,
                rate: Edit::UNIT_RATE,
            });
        }
        list.push(Edit {
            duration: segment,
            media_time: self.media_time,
            rate: Edit::UNIT_RATE,
        });
        Ok(list)
    }
}

/// Pass 1 for one stream: pushes every sample to the muxer and sets the
/// declared durations; returns the first decode time and where the frame
/// presented last ends, in media ticks from it.
#[allow(clippy::too_many_arguments)]
fn list<S: Source>(
    movie: &Movie,
    track: &Track,
    source_edits: &[Edit],
    declared: Option<u64>,
    muxer: &mut Muxer,
    index: usize,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<(i64, u64), ExtractError> {
    let mut first_dts = None;
    let mut next_dts: Option<i64> = None;
    // The latest composition end, a frame's composition time plus its
    // duration (ISO/IEC 14496-12 8.6.1.3).
    let mut end: Option<i64> = None;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        // The output stores durations only, so each sample must start where
        // the previous one ends; fragments set their own base decode time.
        if next_dts.is_some_and(|next| next != packet.dts) {
            return Err(unsupported("a gap or an overlap between decode times").into());
        }
        next_dts = Some(
            packet
                .dts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        first_dts.get_or_insert(packet.dts);
        let ends = packet
            .cts
            .checked_add(i64::from(packet.duration))
            .ok_or_else(overflow)?;
        end = Some(end.map_or(ends, |e| e.max(ends)));
        muxer.push_sample(
            index,
            Sample {
                size: packet.size,
                duration: packet.duration,
                sync: packet.keyframe,
                composition_offset: packet.cts.checked_sub(packet.dts).ok_or_else(overflow)?,
            },
        )?;
    }
    muxer.finish_track(index);
    let first = first_dts.ok_or(unsupported("the track has no samples"))?;
    if first < 0 {
        return Err(unsupported("a negative first decode time").into());
    }
    // The edits are written once every stream is listed, in the movie
    // timescale chosen then; whether they fit this stream is known now.
    let sum = muxer.duration(index).ok_or_else(overflow)?;
    // Frames that all end before the first decode time (offsets below
    // the durations) present nothing; the decode durations' sum stands in
    // rather than a media edit of length 0.
    let composition_end = end
        .and_then(|e| e.checked_sub(first))
        .and_then(|e| u64::try_from(e).ok())
        .filter(|e| *e > 0)
        .unwrap_or(sum);
    edits(
        movie,
        track,
        movie.timescale,
        source_edits,
        first,
        sum,
        composition_end,
    )?;
    // The declared media and track durations are copied, also when they
    // disagree with the sample durations or the edits, as audio extraction
    // does; with fragments the movie header durations cover
    // only part of the samples, so they are computed.
    if !movie.has_fragments() {
        if track.mdhd_duration != 0 {
            muxer.set_media_duration(index, Some(track.mdhd_duration))?;
        }
        muxer.set_duration(index, declared)?;
    }
    Ok((first, composition_end))
}

/// The media durations a fragmented stream's output turns into movie ticks
/// ([`edits`] and the track duration): the first decode time and the
/// composition end without an edit list, else what a last edit of
/// duration 0 presents.
fn computed_ticks(
    edits: &[Edit],
    first_dts: i64,
    sum: u64,
    composition_end: u64,
) -> [Option<u64>; 2] {
    let first = u64::try_from(first_dts).unwrap_or(0);
    match edits.last() {
        // Decoded from 0: no edit list, and the track lasts the media.
        None if first == 0 => [Some(0), Some(sum)],
        None => [Some(first), Some(composition_end)],
        Some(last) if last.duration == 0 && last.media_time >= 0 => {
            let media = u64::try_from(last.media_time).unwrap_or(0);
            [Some(sum.saturating_sub(media.saturating_sub(first))), None]
        }
        Some(_) => [None, None],
    }
}

/// The least common multiple of two timescales, when it fits 32 bits.
/// Whether a fragmented source's copied streams have computed edits that
/// round in `movie_timescale` (the plain remux rounds them there, so the
/// movie timescale must not change for a replaced stream).
fn others_round(movie: &Movie, kept: &[Kept<'_>], muxer: &Muxer, movie_timescale: u32) -> bool {
    movie.has_fragments()
        && kept
            .iter()
            .enumerate()
            .filter(|(_, o)| o.replaced.is_none())
            .any(|(i, o)| {
                let sum = muxer.duration(i).unwrap_or(0);
                computed_ticks(&o.edits, o.first_dts, sum, o.composition_end)
                    .iter()
                    .flatten()
                    .any(|ticks| {
                        u128::from(*ticks)
                            .saturating_mul(u128::from(movie_timescale))
                            .checked_rem(u128::from(o.track.timescale))
                            .is_some_and(|r| r != 0)
                    })
            })
}

pub(crate) fn lcm(a: u32, b: u32) -> Option<u32> {
    let (mut x, mut y) = (u64::from(a), u64::from(b));
    while y != 0 {
        (x, y) = (y, x % y);
    }
    u64::from(a)
        .checked_div(x)?
        .checked_mul(u64::from(b))
        .and_then(|l| u32::try_from(l).ok())
        .filter(|l| *l > 0)
}

/// The output edit list, in `movie_timescale` ticks: the source's (already
/// in those ticks), with media times moved by the first decode time
/// (output decode times start at 0). Without an edit list, a first decode
/// time above 0 becomes an empty edit of that length, rounded to the movie
/// timescale, then a media edit from media time 0 to `composition_end`,
/// where the frame presented last ends (from the first decode time): the
/// source presents every frame at its composition time, and with B-frames
/// that runs past the sum of the decode durations. A last edit of
/// duration 0 in a source with fragments, which lasts to the end of the
/// media, gets that length.
fn edits(
    movie: &Movie,
    track: &Track,
    movie_timescale: u32,
    entries: &[Edit],
    first_dts: i64,
    sample_sum: u64,
    composition_end: u64,
) -> Result<Vec<Edit>, Error> {
    let first = u64::try_from(first_dts).map_err(|_| overflow())?;
    if entries.is_empty() {
        if first == 0 {
            return Ok(Vec::new());
        }
        return Ok(vec![
            Edit {
                duration: rescale(first, track.timescale, movie_timescale)?,
                media_time: -1,
                rate: Edit::UNIT_RATE,
            },
            Edit {
                duration: rescale(composition_end, track.timescale, movie_timescale)?,
                media_time: 0,
                rate: Edit::UNIT_RATE,
            },
        ]);
    }
    let last = entries.len().saturating_sub(1);
    let mut out = Vec::new();
    out.try_reserve_exact(entries.len())
        .map_err(|_| overflow())?;
    for (i, e) in entries.iter().enumerate() {
        if e.media_time == -1 {
            out.push(*e);
            continue;
        }
        let media_time = e
            .media_time
            .checked_sub(first_dts)
            .filter(|t| *t >= 0)
            .ok_or(unsupported("an edit starts before the first sample"))?;
        let duration = if i == last && e.duration == 0 && movie.has_fragments() {
            let rest =
                sample_sum.saturating_sub(u64::try_from(media_time).map_err(|_| overflow())?);
            rescale(rest, track.timescale, movie_timescale)?
        } else {
            e.duration
        };
        out.push(Edit {
            duration,
            media_time,
            rate: e.rate,
        });
    }
    Ok(out)
}

/// `ftyp`: major brand `isom`, compatible `isom`, `iso2` and `mp41`, plus
/// the brands that the kept codecs' bindings define.
fn file_type(kept: &[Kept<'_>]) -> FileType {
    let mut compatible = vec![FourCc(*b"isom"), FourCc(*b"iso2"), FourCc(*b"mp41")];
    let has = |codec| kept.iter().any(|k| k.codec == codec);
    if has(Codec::H264) {
        compatible.push(FourCc(*b"avc1"));
    }
    if has(Codec::Av1) {
        compatible.push(FourCc(*b"av01"));
    }
    if has(Codec::Opus) {
        compatible.push(FourCc(*b"Opus"));
    }
    FileType {
        major: FourCc(*b"isom"),
        minor: 0x200,
        compatible,
    }
}

/// The summary's dropped boxes: those outside the tracks (with the movie's
/// sanitized metadata), then each kept source stream's once, in source
/// order (a stream written twice has its boxes listed once), then the
/// metadata boxes too large to copy, each once; each group in file order;
/// at most [`MAX_REPORTED`].
fn report(
    file: &FileScan,
    movie_gone: &[Gone],
    kept: &[Kept<'_>],
    retained: &Retained,
) -> (Vec<DroppedBox>, u64) {
    let mut listed = Vec::new();
    let mut more = file.skipped.more;
    let mut add = |stream: Option<usize>,
                   s: &Skipped,
                   reason: BoxDropReason,
                   kind: Option<MetadataKind>,
                   name: Option<&String>| {
        if listed.len() >= MAX_REPORTED {
            more = more.saturating_add(1);
            return;
        }
        let mut path = Vec::with_capacity(MAX_PATH.saturating_add(1));
        path.extend_from_slice(s.parent.kinds());
        path.push(s.header.kind);
        listed.push(DroppedBox {
            stream_index: stream,
            path,
            offset: s.header.offset,
            size: s.header.size,
            reason,
            kind,
            name: name.cloned(),
        });
    };
    let mut outside: Vec<(
        &Skipped,
        BoxDropReason,
        Option<MetadataKind>,
        Option<&String>,
    )> = file
        .skipped
        .listed
        .iter()
        .map(|s| (s, BoxDropReason::NotCarried, None, None))
        .collect();
    outside.extend(
        movie_gone
            .iter()
            .map(|g| (&g.skipped, g.reason, g.kind, g.name.as_ref())),
    );
    outside.sort_by_key(|(s, ..)| s.header.offset);
    for (s, reason, kind, name) in outside {
        add(None, s, reason, kind, name);
    }
    let mut sources: Vec<&Kept<'_>> = Vec::new();
    for k in kept {
        if !sources
            .iter()
            .any(|s| s.track.stream_index == k.track.stream_index)
        {
            sources.push(k);
        }
    }
    sources.sort_by_key(|k| k.track.stream_index);
    let mut trak_more = 0u64;
    for k in sources {
        let stream = Some(k.track.stream_index);
        let mut mine: Vec<(
            &Skipped,
            BoxDropReason,
            Option<MetadataKind>,
            Option<&String>,
        )> = Vec::new();
        if let Some(trak) = file.traks.get(k.track.stream_index) {
            mine.extend(
                trak.skipped
                    .listed
                    .iter()
                    .map(|s| (s, BoxDropReason::NotCarried, None, None)),
            );
            trak_more = trak_more.saturating_add(trak.skipped.more);
        }
        mine.extend(
            k.gone
                .iter()
                .map(|g| (&g.skipped, g.reason, g.kind, g.name.as_ref())),
        );
        mine.sort_by_key(|(s, ..)| s.header.offset);
        for (s, reason, kind, name) in mine {
            add(stream, s, reason, kind, name);
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for (stream, s) in &retained.too_large {
        if (stream.is_none() || kept.iter().any(|k| Some(k.track.stream_index) == *stream))
            && seen.insert((*stream, s.header.offset))
        {
            add(*stream, s, BoxDropReason::TooLarge, None, None);
        }
    }
    (listed, more.saturating_add(trak_more))
}

/// The track's language as an ISO 639-2/T code: the media header's code,
/// or for a QuickTime Macintosh language code (below `0x400`) the code of
/// that language, else `und`.
fn track_language(track: &Track, trak: &TrakScan) -> [u8; 3] {
    if track.language.is_some() {
        return language(track.language);
    }
    trak.language
        .and_then(macintosh_language)
        .unwrap_or(*b"und")
}

/// ISO 639-2/T codes of the QuickTime Macintosh language codes (QuickTime
/// File Format, "Language code values").
fn macintosh_language(code: u16) -> Option<[u8; 3]> {
    const CODES: [&[u8; 3]; 95] = [
        b"eng", b"fra", b"deu", b"ita", b"nld", b"swe", b"spa", b"dan", b"por", b"nor", // 0
        b"heb", b"jpn", b"ara", b"fin", b"ell", b"isl", b"mlt", b"tur", b"hrv", b"zho", // 10
        b"urd", b"hin", b"tha", b"kor", b"lit", b"pol", b"hun", b"est", b"lav", b"sme", // 20
        b"fao", b"fas", b"rus", b"zho", b"nld", b"gle", b"sqi", b"ron", b"ces", b"slk", // 30
        b"slv", b"yid", b"srp", b"mkd", b"bul", b"ukr", b"bel", b"uzb", b"kaz", b"aze", // 40
        b"aze", b"hye", b"kat", b"ron", b"kir", b"tgk", b"tuk", b"mon", b"mon", b"pus", // 50
        b"kur", b"kas", b"snd", b"bod", b"nep", b"san", b"mar", b"ben", b"asm", b"guj", // 60
        b"pan", b"ori", b"mal", b"kan", b"tam", b"tel", b"sin", b"mya", b"khm", b"lao", // 70
        b"vie", b"ind", b"tgl", b"msa", b"msa", b"amh", b"tir", b"orm", b"som", b"swa", // 80
        b"kin", b"run", b"nya", b"mlg", b"epo", // 90
    ];
    const HIGH: [&[u8; 3]; 11] = [
        b"cym", b"eus", b"cat", b"lat", b"que", b"grn", b"aym", b"tat", b"uig", b"dzo", b"jav",
    ];
    let index = usize::from(code);
    match index {
        0..=94 => CODES.get(index).map(|c| **c),
        128..=138 => HIGH.get(index.saturating_sub(128)).map(|c| **c),
        _ => None,
    }
}

fn unsupported(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}
