//! Joining MP4/MOV files that share a codec configuration into one MP4 by
//! stream copy (use case 7, `docs/design/join.md`), with the clip writer's
//! track selection, sample entries, timelines, tables, file head and copy
//! window (`crate::clip`, `crate::moov`).
//!
//! Every sample of each input's kept tracks is copied once, in order. Each
//! input keeps its own presentation: its edit lists (B-frame delay, audio
//! priming and padding, frames outside a clip) become one edit per media
//! edit of each input in the output's edit list, with the empty edits
//! between them, and the inputs follow one another on the video's
//! timeline. Edits of inputs that meet exactly are merged, so the segments
//! of a split join back into one edit per track.

use crate::clip::{
    Scaled, Timeline, gapless, media_duration, movie_timescale, select_audio, select_video,
    video_sample_entry,
};
use crate::copy::{COPY_BUFFER, Shared, Window};
use crate::m4a::{audio_sample_entry, unsupported};
use crate::moov::{
    EditEntry, Media, MovieBox, Ties, TrackBox, TrackFlags, UNIT_RATE, ftyp, interleave, language,
    overflow, rescale, write_head,
};
use crate::rational::{add, ceil, int, lt, max, min, round, scale, sub, to_i64, to_u64};
use crate::tables::Tables;
use crate::{
    AudioChoice, ClipTrack, ExtractError, JoinAudio, JoinError, JoinOptions, JoinSummary,
    JoinTrack, JoinedInput, LeftOut, LeftOutReason, Mismatch, VideoChoice,
};
use core::cell::RefCell;
use coreav_isobmff::{
    Allowlist, Budget, Codec, Error, LimitKind, Limits, Movie, Packets, Rational, Source, Track,
    probe,
};
use std::io::Write;

/// Output track IDs, as in a clip.
const VIDEO_ID: u32 = 1;
const AUDIO_ID: u32 = 2;

/// Joins `inputs`, in order, into one MP4 file written to `out`.
///
/// Each input is probed with `limits` and `allowlist`; its first video
/// track (H.264 `avc1`, HEVC `hvc1`, AV1 or VP9) and, with
/// [`JoinAudio::First`], its first audio track (AAC) are kept, checked as a
/// clip checks them. Every input must match the first: the same codecs,
/// sample entry types, codec configurations, coded size, `pasp`, `clap` and
/// `colr` boxes, channels, sample rate, media timescales, track matrix and
/// display size, and an audio track in all or none. Anything else fails with
/// [`JoinError::Mismatch`], naming the input and the difference, before
/// anything is written.
///
/// Each input keeps its own presentation, edit lists included: the output
/// presents what each input presents, the inputs one after another, each
/// starting where the previous input's video ends (the first at its own
/// time 0). The output's edit list holds one edit per media edit of each
/// input and track, merged where two edits meet exactly. Where an input's frames are
/// presented past the media time where the next input's start (a clip that
/// ends inside a group of pictures, say), its last video sample's decode
/// duration is extended so that they are not; [`JoinedInput::extended`]
/// reports it.
///
/// An input after the first must start its video with a sync sample that
/// no later frame is presented before (no open-GOP leading frames): those
/// frames refer to frames before the input, which in the output would be
/// the previous input's.
///
/// Each input is listed three times: to find its presentation, to build the
/// output tables, and to copy its samples through one 64 KiB window, so
/// that `out` receives `ftyp`, `moov` and `mdat` in order without seeking.
/// Memory holds the inputs' parsed indexes, the output tables and the
/// window.
pub fn join_mp4<S: Source, W: Write>(
    inputs: Vec<S>,
    limits: &Limits,
    allowlist: &Allowlist,
    options: &JoinOptions,
    mut out: W,
) -> Result<JoinSummary, JoinError> {
    if inputs.is_empty() {
        return Err(JoinError::NoInputs);
    }
    let sources: Vec<RefCell<S>> = inputs.into_iter().map(RefCell::new).collect();
    let mut movies = Vec::new();
    movies
        .try_reserve_exact(sources.len())
        .map_err(|_| JoinError::Output(table_limit()))?;
    for (input, source) in sources.iter().enumerate() {
        let movie = probe(Shared(source), limits, allowlist).map_err(|e| at(input, e))?;
        movies.push(movie);
        // Every input stays open for the whole join; what its source holds
        // for reads is freed between the passes that read it.
        Shared(source).release();
    }
    let mut picks = Vec::new();
    for (input, movie) in movies.iter().enumerate() {
        picks.push(Pick::of(movie, options.audio).map_err(|e| at(input, e))?);
    }
    let first = picks.first().ok_or(JoinError::NoInputs)?;
    for (input, pick) in picks.iter().enumerate().skip(1) {
        if let Some(mismatch) = compare(first, pick) {
            return Err(JoinError::Mismatch { input, mismatch });
        }
    }
    let tv = first.video.timescale;
    let ta = first.audio.as_ref().map(|a| a.track.timescale);

    // Listing 1: each input's samples and presentation.
    let mut spans = Vec::new();
    let mut budgets = Vec::new();
    for (input, (movie, pick)) in movies.iter().zip(&picks).enumerate() {
        let source = sources
            .get(input)
            .ok_or_else(|| JoinError::Output(overflow()))?;
        let mut budget = Budget::new(limits);
        let video = video_span(movie, pick.video, &mut Shared(source), limits, &mut budget)
            .map_err(|e| at(input, e))?;
        if input > 0 && !video.starts_with_sync {
            return Err(at(
                input,
                unsupported("the video does not start with a sync sample"),
            ));
        }
        // Its leading frames refer to frames before the input, which in
        // the output are the previous input's (docs/design/join.md).
        if input > 0 && video.leading {
            return Err(at(
                input,
                unsupported(
                    "the video starts at an open-GOP sync sample whose leading frames refer to frames before it",
                ),
            ));
        }
        let audio = match pick.audio.as_ref() {
            Some(a) => Some(
                audio_span(movie, a.track, &mut Shared(source), limits, &mut budget)
                    .map_err(|e| at(input, e))?,
            ),
            None => None,
        };
        // Listing 2 and the copy visit the same samples as listing 1, so an
        // input whose three listings would exceed the work limit is
        // refused here, before anything is written (as for a split).
        if budget.used().saturating_mul(3) > limits.max_sample_work {
            return Err(at(
                input,
                Error::LimitExceeded {
                    limit: LimitKind::SampleWork,
                    offset: 0,
                },
            ));
        }
        spans.push(Spans { video, audio });
        budgets.push(budget);
        Shared(source).release();
    }

    // Where each input goes.
    let tm = ta.map_or(tv, |ta| movie_timescale(tv, ta));
    let layout = place(&spans, tv, ta).map_err(JoinError::Output)?;

    // Listing 2: the output tables.
    let mut video_tables = Tables::new(tv, limits.max_table_bytes);
    let mut audio_tables = ta.map(|ta| Tables::new(ta, limits.max_table_bytes));
    for (input, ((movie, pick), budget)) in movies.iter().zip(&picks).zip(&mut budgets).enumerate()
    {
        let source = sources
            .get(input)
            .ok_or_else(|| JoinError::Output(overflow()))?;
        let (span, placed) = spans
            .get(input)
            .zip(layout.inputs.get(input))
            .ok_or_else(|| JoinError::Output(overflow()))?;
        append(
            &mut video_tables,
            movie,
            pick.video,
            &span.video.listed,
            placed.extended,
            true,
            &mut Shared(source),
            limits,
            budget,
        )
        .map_err(|e| at_extract(input, e))?;
        if let (Some(tables), Some(a), Some(audio)) =
            (audio_tables.as_mut(), pick.audio.as_ref(), &span.audio)
        {
            append(
                tables,
                movie,
                a.track,
                &audio.listed,
                0,
                false,
                &mut Shared(source),
                limits,
                budget,
            )
            .map_err(|e| at_extract(input, e))?;
        }
        Shared(source).release();
    }
    video_tables.finish();
    if let Some(tables) = audio_tables.as_mut() {
        tables.finish();
    }

    // The file head.
    let last = movies
        .len()
        .checked_sub(1)
        .and_then(|i| movies.get(i).zip(picks.get(i)))
        .ok_or(JoinError::NoInputs)?;
    let video_edits = edits(&layout.video, tv, tm).map_err(JoinError::Output)?;
    let video_media = media_duration(last.0, last.1.video, &video_tables, true)
        .map_err(JoinError::Output)?
        .max(media_end(&video_edits, tv, tm).map_err(JoinError::Output)?);
    let video_duration = track_duration(&video_edits);
    let first_movie = movies.first().ok_or(JoinError::NoInputs)?;
    let mut tracks = vec![TrackBox {
        track_id: VIDEO_ID,
        header: TrackFlags::DEFAULT,
        media: Media::Video {
            matrix: first.video.matrix,
            size: first.video.presentation_size,
        },
        created: first.video.creation_time,
        media_created: first.video.media_creation_time,
        language: language(first.video.language),
        media_timescale: tv,
        media_duration: video_media,
        duration: video_duration,
        edits: video_edits.into(),
        stsd: first.video_entry.clone(),
        tables: &video_tables,
        extra: Vec::new(),
        stbl_extra: Vec::new(),
    }];
    let mut movie_duration = video_duration;
    let mut audio_edit_count = 0;
    if let (Some(a), Some(tables), Some(ta)) = (first.audio.as_ref(), audio_tables.as_ref(), ta) {
        let mut audio_edits = edits(&layout.audio, ta, tm).map_err(JoinError::Output)?;
        if audio_edits.iter().all(|e| e.media_time < 0) {
            return Err(at(0, unsupported("no input presents its audio")));
        }
        let last_audio = last
            .1
            .audio
            .as_ref()
            .map(|l| l.track)
            .ok_or(JoinError::NoInputs)?;
        let media = media_duration(last.0, last_audio, tables, true)
            .map_err(JoinError::Output)?
            .max(media_end(&audio_edits, ta, tm).map_err(JoinError::Output)?);
        // As in a clip, an edit that presents all of the media from its
        // start is left out.
        if let [only] = audio_edits.as_slice() {
            if only.media_time == 0
                && only.duration == rescale(media, ta, tm).map_err(JoinError::Output)?
            {
                audio_edits.clear();
            }
        }
        let duration = if audio_edits.is_empty() {
            rescale(media, ta, tm).map_err(JoinError::Output)?
        } else {
            track_duration(&audio_edits)
        };
        audio_edit_count = audio_edits.len();
        movie_duration = movie_duration.max(duration);
        tracks.push(TrackBox {
            track_id: AUDIO_ID,
            header: TrackFlags::DEFAULT,
            media: Media::Audio,
            created: a.track.creation_time,
            media_created: a.track.media_creation_time,
            language: language(a.track.language),
            media_timescale: ta,
            media_duration: media,
            duration,
            edits: audio_edits.into(),
            stsd: a.entry.clone(),
            tables,
            extra: Vec::new(),
            stbl_extra: Vec::new(),
        });
    }
    let video_edit_count = tracks.first().map_or(0, |t| t.edits.len());
    let brands: Vec<&[u8; 4]> = match first.video.description.codec {
        Codec::H264 => vec![b"isom", b"mp42", b"avc1"],
        Codec::Av1 => vec![b"isom", b"mp42", b"av01"],
        _ => vec![b"isom", b"mp42"],
    };
    let (data_start, data_bytes) = write_head(
        &mut out,
        ftyp(b"isom", &brands),
        &MovieBox {
            timescale: tm,
            created: first_movie.creation_time,
            duration: movie_duration,
            extra: Vec::new(),
        },
        tracks,
        Ties::TrackOrder,
    )
    .map_err(output)?;

    // Listing 3: the samples, in the order of the chunks in `mdat`.
    let copy_limits: Vec<Limits> = budgets
        .iter()
        .map(|b| Limits {
            max_sample_work: limits.max_sample_work.saturating_sub(b.used()),
            ..*limits
        })
        .collect();
    let mut video_budgets: Vec<Budget> = copy_limits.iter().map(Budget::new).collect();
    let mut audio_budgets: Vec<Budget> = copy_limits.iter().map(Budget::new).collect();
    let counts = |audio: bool| -> Vec<u64> {
        spans
            .iter()
            .map(|s| {
                if audio {
                    s.audio.as_ref().map_or(0, |a| a.listed.samples)
                } else {
                    s.video.listed.samples
                }
            })
            .collect()
    };
    let video_tracks: Vec<usize> = picks.iter().map(|p| p.video.index).collect();
    let audio_tracks: Vec<usize> = picks
        .iter()
        .map(|p| p.audio.as_ref().map_or(0, |a| a.track.index))
        .collect();
    let mut video_run = Chain::new(
        &movies,
        &sources,
        &copy_limits,
        video_budgets.iter_mut(),
        video_tracks,
        counts(false),
        &video_tables,
    );
    let mut audio_run = audio_tables.as_ref().map(|tables| {
        Chain::new(
            &movies,
            &sources,
            &copy_limits,
            audio_budgets.iter_mut(),
            audio_tracks,
            counts(true),
            tables,
        )
    });
    let both;
    let one;
    let layout_tables: &[&Tables] = match &audio_run {
        Some(run) => {
            both = [&video_tables, run.tables];
            &both
        }
        None => {
            one = [&video_tables];
            &one
        }
    };
    let mut window = Window::new(COPY_BUFFER);
    let mut current = None;
    interleave(
        layout_tables,
        Ties::TrackOrder,
        |track, chunk| -> Result<(), JoinError> {
            let run = match track {
                0 => &mut video_run,
                _ => audio_run
                    .as_mut()
                    .ok_or_else(|| JoinError::Output(overflow()))?,
            };
            for _ in 0..chunk.samples {
                let (input, offset, size) = run.next()?;
                if current != Some(input) {
                    window.clear();
                    if let Some(previous) = current.and_then(|p| sources.get(p)) {
                        Shared(previous).release();
                    }
                    current = Some(input);
                }
                let source = sources
                    .get(input)
                    .ok_or_else(|| JoinError::Output(overflow()))?;
                window
                    .copy(&mut Shared(source), offset, size, &mut out)
                    .map_err(|e| match e {
                        // The output failed, not the input.
                        ExtractError::Write(kind) => JoinError::Write(kind),
                        e => at_extract(input, e),
                    })?;
            }
            Ok(())
        },
    )?;
    video_run.finish()?;
    if let Some(run) = audio_run.as_mut() {
        run.finish()?;
    }
    drop((video_run, audio_run, window));
    for (input, ((limit, v), a)) in copy_limits
        .iter()
        .zip(&video_budgets)
        .zip(&audio_budgets)
        .enumerate()
    {
        if v.used().saturating_add(a.used()) > limit.max_sample_work {
            return Err(at(
                input,
                Error::LimitExceeded {
                    limit: LimitKind::SampleWork,
                    offset: 0,
                },
            ));
        }
    }
    out.flush().map_err(|e| JoinError::Write(e.kind()))?;

    // The summary.
    let mut joined = Vec::new();
    for ((movie, pick), (span, placed)) in movies
        .iter()
        .zip(&picks)
        .zip(spans.iter().zip(&layout.inputs))
    {
        let track = |t: &Track, listed: &Listed| ClipTrack {
            stream_index: t.stream_index,
            track_id: t.track_id,
            codec: t.description.codec,
            first_sample: 0,
            samples: u32::try_from(listed.samples).unwrap_or(u32::MAX),
            timescale: t.timescale,
            duration: listed.duration,
        };
        let mut left_out = Vec::new();
        for t in &movie.tracks {
            if t.index != pick.video.index
                && pick.audio.as_ref().is_none_or(|a| a.track.index != t.index)
            {
                left_out.push(LeftOut {
                    stream_index: t.stream_index,
                    track_id: t.track_id,
                    reason: LeftOutReason::NotSelected,
                });
            }
        }
        for dropped in &movie.dropped {
            left_out.push(LeftOut {
                stream_index: dropped.stream_index,
                track_id: dropped.track_id,
                reason: LeftOutReason::CodecNotAllowed,
            });
        }
        left_out.sort_by_key(|l| l.stream_index);
        joined.push(JoinedInput {
            video: track(pick.video, &span.video.listed),
            audio: pick
                .audio
                .as_ref()
                .zip(span.audio.as_ref())
                .map(|(a, s)| track(a.track, &s.listed)),
            offset: placed.offset,
            video_start: placed.video.0,
            video_end: placed.video.1,
            extended: placed.extended,
            left_out,
        });
    }
    Ok(JoinSummary {
        inputs: joined,
        video_edits: video_edit_count,
        audio_edits: audio_edit_count,
        duration: layout.end,
        bytes_written: data_start.saturating_add(data_bytes),
        boxes_left_out: first.video.description.other_boxes.clone(),
    })
}

fn at(input: usize, error: Error) -> JoinError {
    JoinError::Input {
        input,
        error: ExtractError::Source(error),
    }
}

fn at_extract(input: usize, error: ExtractError) -> JoinError {
    JoinError::Input { input, error }
}

fn output(error: ExtractError) -> JoinError {
    match error {
        ExtractError::Source(error) => JoinError::Output(error),
        ExtractError::Write(kind) => JoinError::Write(kind),
        ExtractError::SourceChanged => JoinError::Output(overflow()),
        // Only HLS indexes are read as such; a join writes none.
        ExtractError::InvalidIndex { reason } => JoinError::Output(unsupported(reason)),
    }
}

fn table_limit() -> Error {
    Error::LimitExceeded {
        limit: LimitKind::TableBytes,
        offset: 0,
    }
}

/// The kept tracks of one input and their sample entries.
struct Pick<'a> {
    video: &'a Track,
    video_entry: Vec<u8>,
    audio: Option<PickAudio<'a>>,
}

struct PickAudio<'a> {
    track: &'a Track,
    entry: Vec<u8>,
}

impl<'a> Pick<'a> {
    fn of(movie: &'a Movie, audio: JoinAudio) -> Result<Self, Error> {
        let video = select_video(movie, VideoChoice::First)?;
        let video_entry = video_sample_entry(video)?;
        let audio = match audio {
            JoinAudio::None => None,
            JoinAudio::First => select_audio(movie, AudioChoice::First)?,
        };
        let audio = match audio {
            Some(track) => Some(PickAudio {
                track,
                entry: audio_sample_entry(track)?,
            }),
            None => None,
        };
        Ok(Self {
            video,
            video_entry,
            audio,
        })
    }
}

/// How `this` differs from `first`, if it does (`docs/design/join.md`):
/// everything a decoder or a reader of the one sample entry and track
/// header of the output depends on. Declared bit rates (`btrt`, the bit
/// rates of `esds`) are not compared; the output keeps the first input's.
fn compare(first: &Pick<'_>, this: &Pick<'_>) -> Option<Mismatch> {
    let (a, b) = (&first.video.description, &this.video.description);
    let video = JoinTrack::Video;
    if a.codec != b.codec {
        return Some(Mismatch::Codec {
            track: video,
            first: a.codec,
            this: b.codec,
        });
    }
    if a.fourcc != b.fourcc {
        return Some(Mismatch::EntryType {
            track: video,
            first: a.fourcc,
            this: b.fourcc,
        });
    }
    if a.codec_config != b.codec_config {
        return Some(Mismatch::Configuration {
            track: video,
            kind: a
                .codec_config
                .as_ref()
                .or(b.codec_config.as_ref())
                .map_or(a.fourcc, |c| c.kind),
        });
    }
    if (a.width, a.height) != (b.width, b.height) {
        return Some(Mismatch::CodedSize {
            first: (a.width.unwrap_or(0), a.height.unwrap_or(0)),
            this: (b.width.unwrap_or(0), b.height.unwrap_or(0)),
        });
    }
    for kind in [*b"pasp", *b"clap", *b"colr"] {
        let find = |d: &coreav_isobmff::SampleDescription| {
            d.entry_boxes.iter().find(|x| x.kind.0 == kind).cloned()
        };
        if find(a) != find(b) {
            return Some(Mismatch::EntryBox {
                kind: coreav_isobmff::FourCc(kind),
            });
        }
    }
    if first.video.timescale != this.video.timescale {
        return Some(Mismatch::Timescale {
            track: video,
            first: first.video.timescale,
            this: this.video.timescale,
        });
    }
    if first.video.matrix != this.video.matrix {
        return Some(Mismatch::Matrix);
    }
    if first.video.presentation_size != this.video.presentation_size {
        return Some(Mismatch::DisplaySize {
            first: first.video.presentation_size,
            this: this.video.presentation_size,
        });
    }
    let (fa, ta) = match (&first.audio, &this.audio) {
        (None, None) => return None,
        (Some(_), None) => return Some(Mismatch::AudioTrack { present: false }),
        (None, Some(_)) => return Some(Mismatch::AudioTrack { present: true }),
        (Some(fa), Some(ta)) => (fa, ta),
    };
    let audio = JoinTrack::Audio;
    let (a, b) = (&fa.track.description, &ta.track.description);
    if a.codec != b.codec {
        return Some(Mismatch::Codec {
            track: audio,
            first: a.codec,
            this: b.codec,
        });
    }
    if a.channels != b.channels {
        return Some(Mismatch::Channels {
            first: a.channels.unwrap_or(0),
            this: b.channels.unwrap_or(0),
        });
    }
    if a.sample_rate != b.sample_rate {
        return Some(Mismatch::SampleRate {
            first: a.sample_rate.unwrap_or(0),
            this: b.sample_rate.unwrap_or(0),
        });
    }
    let config = |d: &coreav_isobmff::SampleDescription| {
        d.codec_config.as_ref().map(|c| decoder_config(&c.data))
    };
    if config(a) != config(b) {
        return Some(Mismatch::Configuration {
            track: audio,
            kind: coreav_isobmff::FourCc(*b"esds"),
        });
    }
    if fa.track.timescale != ta.track.timescale {
        return Some(Mismatch::Timescale {
            track: audio,
            first: fa.track.timescale,
            this: ta.track.timescale,
        });
    }
    None
}

/// The decoder configuration of an `esds` payload (ISO/IEC 14496-1 7.2.6.6
/// and 7.2.6.7, ISO/IEC 14496-14 5.6): the object type and the decoder
/// specific information, without the ES identifier, buffer size and bit
/// rates. The whole payload when it cannot be read that way.
fn decoder_config(esds: &[u8]) -> Vec<u8> {
    fn descriptor(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let (&tag, mut rest) = data.split_first()?;
        let mut size = 0usize;
        for _ in 0..4 {
            let (&byte, tail) = rest.split_first()?;
            rest = tail;
            size = size
                .checked_mul(128)?
                .checked_add(usize::from(byte & 0x7f))?;
            if byte & 0x80 == 0 {
                let body = rest.get(..size)?;
                return Some((tag, body, rest.get(size..)?));
            }
        }
        None
    }
    let parsed = (|| -> Option<Vec<u8>> {
        // Version and flags of the full box.
        let (tag, es, _) = descriptor(esds.get(4..)?)?;
        if tag != 3 {
            return None;
        }
        let flags = *es.get(2)?;
        let mut at = 3usize;
        if flags & 0x80 != 0 {
            at = at.checked_add(2)?;
        }
        if flags & 0x40 != 0 {
            let len = usize::from(*es.get(at)?);
            at = at.checked_add(1)?.checked_add(len)?;
        }
        if flags & 0x20 != 0 {
            at = at.checked_add(2)?;
        }
        let (tag, config, _) = descriptor(es.get(at..)?)?;
        if tag != 4 {
            return None;
        }
        let object_type = *config.first()?;
        let (tag, specific, _) = descriptor(config.get(13..)?)?;
        if tag != 5 {
            return None;
        }
        let mut out = vec![object_type];
        out.extend_from_slice(specific);
        Some(out)
    })();
    parsed.unwrap_or_else(|| esds.to_vec())
}

/// What listing 1 counted of a track.
struct Listed {
    samples: u64,
    duration: u64,
    first_dts: i64,
}

/// A composition offset of this many ticks or more: a version 0 `ctts`
/// offset that ISO/IEC 14496-12 reads as unsigned, where the muxer most
/// likely meant a negative one (`KD-C-CTTS-UNSIGNED`). Such a sample is
/// composed some 2^31 ticks or more after its decode, past every edit, so
/// it is never presented; extending decode durations to keep it before the
/// next input would move every later input by that much.
const FAR_OFFSET: i64 = 1 << 31;

/// The video of one input, in its media ticks.
struct VideoSpan<'a> {
    listed: Listed,
    starts_with_sync: bool,
    /// Some sample after the first in decode order is composed before it:
    /// the video starts at an open-GOP sync sample with leading frames.
    leading: bool,
    /// Composition interval of its samples (by their durations), but for
    /// far ones.
    comp: (i64, i64),
    /// The latest composition time of a sample, but for far ones.
    last_cts: i64,
    /// The earliest composition time of a far sample: one composed
    /// [`FAR_OFFSET`] ticks or more after it is decoded.
    far: Option<i64>,
    /// Where its video is presented on its own timeline, `[start, end)`.
    start: Rational,
    end: Rational,
    timeline: Timeline<'a>,
}

/// The audio of one input, in its media ticks. `None` presentation when it
/// presents nothing.
struct AudioSpan<'a> {
    listed: Listed,
    presented: Option<(Rational, Rational)>,
    timeline: Timeline<'a>,
}

struct Spans<'a> {
    video: VideoSpan<'a>,
    audio: Option<AudioSpan<'a>>,
}

/// What an input presents of a track in `[from, to)` of its own timeline
/// (media ticks): `(start, end, media time of start)` per media edit, a
/// single one without a list of several edits.
fn shown(
    timeline: &Timeline<'_>,
    from: Rational,
    to: Rational,
) -> Result<Vec<(Rational, Rational, Rational)>, Error> {
    if !lt(from, to)? {
        return Ok(Vec::new());
    }
    if !timeline.has_windows() {
        return Ok(vec![(from, to, timeline.media(from)?)]);
    }
    let mut out = Vec::new();
    for window in timeline.windows_near(from)? {
        let (start, end, media_time) = window?;
        if !lt(start, to)? {
            break;
        }
        let (s, e) = (max(start, from)?, min(end, to)?);
        if lt(s, e)? {
            out.push((s, e, add(sub(s, start)?, int(media_time))?));
        }
    }
    Ok(out)
}

/// Listing 1 for an input's video.
fn video_span<'a, S: Source>(
    movie: &'a Movie,
    track: &'a Track,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<VideoSpan<'a>, Error> {
    let timeline = Timeline::with_several(track, movie, None)?;
    let scaled = Scaled::of(&timeline)?;
    let mut listed = Listed {
        samples: 0,
        duration: 0,
        first_dts: 0,
    };
    let mut starts_with_sync = false;
    let mut first_cts = None;
    let mut leading = false;
    let mut comp: Option<(i64, i64)> = None;
    let mut far: Option<i64> = None;
    let mut last_cts: Option<i64> = None;
    let mut presented: Option<(i64, i64)> = None;
    // The duration of the presented sample that ends last, and the two
    // latest presentation times (the last frame's spacing from the one
    // before it).
    let mut last_frame = 0;
    let mut latest: (Option<i64>, Option<i64>) = (None, None);
    let mut next_dts = None;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        if listed.samples == 0 {
            listed.first_dts = packet.dts;
            starts_with_sync = packet.keyframe;
        }
        match first_cts {
            None => first_cts = Some(packet.cts),
            Some(first) => leading |= packet.cts < first,
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
        listed.samples = listed.samples.saturating_add(1);
        listed.duration = listed
            .duration
            .checked_add(u64::from(packet.duration))
            .ok_or_else(overflow)?;
        let end = packet
            .cts
            .checked_add(i64::from(packet.duration))
            .ok_or_else(overflow)?;
        if packet.cts.saturating_sub(packet.dts) >= FAR_OFFSET {
            far = Some(far.map_or(packet.cts, |f| f.min(packet.cts)));
        } else {
            comp = Some(comp.map_or((packet.cts, end), |(lo, hi)| {
                (lo.min(packet.cts), hi.max(end))
            }));
            last_cts = Some(last_cts.map_or(packet.cts, |c: i64| c.max(packet.cts)));
        }
        if scaled.presents(packet.cts, packet.duration)? {
            let p = scaled.present(packet.cts)?;
            let p_end = p
                .checked_add(scaled.span(packet.duration)?)
                .ok_or_else(overflow)?;
            if presented.is_none_or(|(_, hi)| p_end >= hi) {
                last_frame = scaled.span(packet.duration)?;
            }
            latest = match latest {
                (Some(top), _) if p > top => (Some(p), Some(top)),
                (Some(top), second) if p == top => (Some(top), second),
                (Some(top), second) => (Some(top), Some(second.map_or(p, |s| s.max(p)))),
                (None, _) => (Some(p), None),
            };
            presented = Some(presented.map_or((p, p_end), |(lo, hi)| (lo.min(p), hi.max(p_end))));
        }
    }
    let (Some(comp), Some(last_cts), Some((first, last))) = (comp, last_cts, presented) else {
        return Err(unsupported("no video sample is presented"));
    };
    // As for a clip to the end: the edit, the stream or the
    // last presented sample, whichever ends first; but the edit where the
    // input's last frame is shown until it ends (ISO/IEC 14496-12: a sample
    // is shown until the next one's composition time). A segment or clip
    // ends where its source's next frame starts, while its last frame, by
    // its own decode duration, can end earlier (variable frame rate,
    // reordered frames): ending the input there would place the next one
    // that much early and cut its audio. The edit is taken when it lies
    // within the media's declared duration, when it ends within the
    // spacing of the input's last two frames after the last one starts
    // (how long a player shows it), or less than a frame after the
    // samples end.
    let (window_start, window_end) = timeline.window()?;
    let mut end = scaled.ticks(last)?;
    if let Some(window_end) = window_end {
        let spacing = match latest {
            (Some(top), Some(second)) => top.saturating_sub(second),
            _ => 0,
        };
        let held = latest
            .0
            .map_or(last, |top| top.saturating_add(spacing.max(last_frame)));
        let held = scaled.ticks(last.max(held))?;
        let reach = scaled.ticks(last.checked_add(last_frame).ok_or_else(overflow)?)?;
        let declared = match i64::try_from(track.mdhd_duration) {
            Ok(duration) if duration > 0 => Some(scaled.ticks(scaled.present(duration)?)?),
            _ => None,
        };
        let within = match declared {
            Some(media_end) => !lt(media_end, window_end)?,
            None => false,
        };
        if within || !lt(held, window_end)? || lt(window_end, reach)? {
            end = window_end;
        }
    }
    // The stream's duration is what its one edit presents; with several,
    // the edits themselves bound it, and an end between two of them is
    // where the earlier one ends.
    if timeline.has_windows() {
        end = timeline.presented_until(end)?;
    } else if let Some(duration) = track.duration {
        end = min(end, add(window_start, duration)?)?;
    }
    let start = max(scaled.ticks(first)?, window_start)?;
    if !lt(start, end)? {
        return Err(unsupported("no video sample is presented"));
    }
    // The edit starts at the media time the video starts, which must not lie
    // before the first decode time (a negative media time is an empty edit).
    if sub(timeline.media(start)?, int(listed.first_dts))?.num < 0 {
        return Err(unsupported(
            "the first sync sample is presented before it is decoded",
        ));
    }
    Ok(VideoSpan {
        listed,
        starts_with_sync,
        leading,
        comp,
        last_cts,
        far,
        start,
        end,
        timeline,
    })
}

/// Listing 1 for an input's audio.
fn audio_span<'a, S: Source>(
    movie: &'a Movie,
    track: &'a Track,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<AudioSpan<'a>, Error> {
    let gapless = gapless(movie, track, &mut *source)?;
    let timeline = Timeline::with_several(track, movie, gapless)?;
    let scaled = Scaled::of(&timeline)?;
    let mut listed = Listed {
        samples: 0,
        duration: 0,
        first_dts: 0,
    };
    let mut presented: Option<(i64, i64)> = None;
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
        if listed.samples == 0 {
            listed.first_dts = packet.dts;
        }
        listed.samples = listed.samples.saturating_add(1);
        listed.duration = listed
            .duration
            .checked_add(u64::from(packet.duration))
            .ok_or_else(overflow)?;
        if scaled.presents(packet.dts, packet.duration)? {
            let q = scaled.present(packet.dts)?;
            let q_end = q
                .checked_add(scaled.span(packet.duration)?)
                .ok_or_else(overflow)?;
            presented = Some(presented.map_or((q, q_end), |(lo, hi)| (lo.min(q), hi.max(q_end))));
        }
    }
    let presented = match presented {
        Some((first, last)) => {
            let (window_start, window_end) = timeline.window()?;
            let start = max(scaled.ticks(first)?, window_start)?;
            let mut end = scaled.ticks(last)?;
            if let Some(window_end) = window_end {
                end = min(end, window_end)?;
            }
            let end = timeline.presented_until(end)?;
            lt(start, end)?.then_some((start, end))
        }
        None => None,
    };
    Ok(AudioSpan {
        listed,
        presented,
        timeline,
    })
}

/// One input's piece of an output track: presented in `[start, end)` of the
/// output (seconds), from media time `media` of the output (media ticks).
#[derive(Clone, Copy)]
struct Piece {
    start: Rational,
    end: Rational,
    media: Rational,
}

/// Where each input goes.
struct Placed {
    offset: Rational,
    extended: u64,
    /// Where its video is presented in the output, `[start, end)` in
    /// seconds.
    video: (Rational, Rational),
}

struct Layout {
    inputs: Vec<Placed>,
    video: Vec<Piece>,
    audio: Vec<Piece>,
    /// End of the last input's video in the output, in seconds.
    end: Rational,
}

/// Places the inputs one after another on the video's timeline
/// (`docs/design/join.md`): the first at its own time 0, each next one so
/// that its earliest presented sample starts where the previous input's
/// video ends. Decode times follow one another; a video sample presented
/// past the next input's first media time extends the input's last decode
/// duration.
fn place(spans: &[Spans<'_>], tv: u32, ta: Option<u32>) -> Result<Layout, Error> {
    let seconds = |ticks: Rational, timescale: u32| scale(ticks, 1, timescale);
    let mut inputs = Vec::new();
    let mut video = Vec::new();
    let mut audio = Vec::new();
    // Output time of each input's time 0.
    let mut offsets = Vec::new();
    let mut next_start = int(0);
    for (index, span) in spans.iter().enumerate() {
        let video_start = seconds(span.video.start, tv)?;
        // Where the input starts: its video, or its audio when that starts
        // earlier, moved back by whole video ticks so that the video stays
        // exact (the audio, rounded to its own ticks anyway, then starts
        // less than a video tick after it).
        let origin = if index == 0 {
            int(0)
        } else {
            match (span.audio.as_ref().and_then(|a| a.presented), ta) {
                (Some((start, _)), Some(ta)) if lt(seconds(start, ta)?, video_start)? => {
                    let lead = scale(sub(video_start, seconds(start, ta)?)?, tv, 1)?;
                    sub(video_start, seconds(Rational::from_int(ceil(lead)?), tv)?)?
                }
                _ => video_start,
            }
        };
        let offset = sub(next_start, origin)?;
        next_start = add(offset, seconds(span.video.end, tv)?)?;
        offsets.push(offset);
    }
    // What each input presents of its video, one piece per media edit.
    let mut shown_video = Vec::new();
    for span in spans {
        let pieces = shown(&span.video.timeline, span.video.start, span.video.end)?;
        if pieces.is_empty() {
            return Err(unsupported("no video sample is presented"));
        }
        shown_video.push(pieces);
    }
    // Where each input's video is presented in the output. An input whose
    // video starts after the previous input's ends (its own video starts
    // after its audio) does not leave a gap: the previous input's last frame
    // is held until then, its edit running on into the held time; unless a
    // frame of its own is composed after its last edit (a frame cut out
    // between two edits, or one a clip keeps only for decoding), which the
    // running edit would show: then the gap stays an empty edit.
    let mut video_spans = Vec::new();
    for (index, (span, pieces)) in spans.iter().zip(&shown_video).enumerate() {
        let offset = *offsets.get(index).ok_or_else(overflow)?;
        let start = add(offset, seconds(span.video.start, tv)?)?;
        let mut end = add(offset, seconds(span.video.end, tv)?)?;
        let &(s, e, media) = pieces.last().ok_or_else(overflow)?;
        let holds = lt(int(span.video.last_cts), add(media, sub(e, s)?)?)?;
        if let (true, Some(next), Some(next_offset)) = (
            holds,
            spans.get(index.saturating_add(1)),
            offsets.get(index.saturating_add(1)),
        ) {
            end = max(end, add(*next_offset, seconds(next.video.start, tv)?)?)?;
        }
        video_spans.push((start, end));
    }
    // Decode times of the video: each input after the previous one's
    // samples, and after every media time the previous one presents or
    // composes.
    let mut decode = int(0);
    let mut extended = Vec::new();
    let mut video_bases = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let v = &span.video;
        let base = decode;
        video_bases.push(base);
        let relative = |t: Rational| sub(t, int(v.listed.first_dts));
        let offset = *offsets.get(index).ok_or_else(overflow)?;
        let (_, out_end) = *video_spans.get(index).ok_or_else(overflow)?;
        // The media time where its last piece ends, held time included.
        let &(last_start, _, last_media) = shown_video
            .get(index)
            .and_then(|pieces| pieces.last())
            .ok_or_else(overflow)?;
        let edit_end = add(
            relative(last_media)?,
            scale(sub(out_end, add(offset, seconds(last_start, tv)?)?)?, tv, 1)?,
        )?;
        // Every composition time of this input must come before every one
        // of the next. When all of them lie before the end of its edit (a
        // segment cut at a frame of its source, say), the edit ends its
        // last frame: frames whose durations overlap the next frame by a
        // tick or so, as reordered frames of variable frame rate sources
        // do, need no room, and the segments of a split meet as in their
        // source. Otherwise (a clip that keeps frames past its end for
        // decoding) its frames' whole durations must end first, so that
        // none of them is shown in the next input's edit.
        let instants = relative(int(v.last_cts.checked_add(1).ok_or_else(overflow)?))?;
        let hi = if lt(edit_end, instants)? {
            max(relative(int(v.comp.1))?, edit_end)?
        } else {
            edit_end
        };
        let mut step = int(i64::try_from(v.listed.duration).map_err(|_| overflow())?);
        if let Some(next) = spans.get(index.saturating_add(1)) {
            let n = &next.video;
            let relative_n = |t: Rational| sub(t, int(n.listed.first_dts));
            let lo = min(
                relative_n(int(n.comp.0))?,
                relative_n(n.timeline.media(n.start)?)?,
            )?;
            let needed = Rational::from_int(ceil(sub(hi, lo)?)?);
            step = max(step, needed)?;
        }
        let extra = to_u64(round(sub(
            step,
            int(i64::try_from(v.listed.duration).map_err(|_| overflow())?),
        )?))?;
        extended.push(extra);
        decode = add(decode, step)?;
    }
    for (index, span) in spans.iter().enumerate() {
        let offset = *offsets.get(index).ok_or_else(overflow)?;
        let v = &span.video;
        let base = *video_bases.get(index).ok_or_else(overflow)?;
        let (start, end) = *video_spans.get(index).ok_or_else(overflow)?;
        let pieces = shown_video.get(index).ok_or_else(overflow)?;
        for (k, &(s, e, media)) in pieces.iter().enumerate() {
            // The last piece runs on to the input's end, held time included.
            let piece_end = if k.saturating_add(1) == pieces.len() {
                end
            } else {
                add(offset, seconds(e, tv)?)?
            };
            video.push(Piece {
                start: add(offset, seconds(s, tv)?)?,
                end: piece_end,
                media: add(base, sub(media, int(v.listed.first_dts))?)?,
            });
        }
        inputs.push(Placed {
            offset,
            extended: *extended.get(index).ok_or_else(overflow)?,
            video: (start, end),
        });
    }
    // Far samples are left out of the composition intervals above: no edit
    // of an input presents them, and none of the output may.
    let mut media_end = int(0);
    for piece in &video {
        media_end = max(
            media_end,
            add(piece.media, scale(sub(piece.end, piece.start)?, tv, 1)?)?,
        )?;
    }
    for (span, base) in spans.iter().zip(&video_bases) {
        if let Some(far) = span.video.far {
            let composed = add(*base, sub(int(far), int(span.video.listed.first_dts))?)?;
            if lt(composed, media_end)? {
                return Err(unsupported(
                    "a video sample composed 2^31 ticks or more after its decode time would be presented",
                ));
            }
        }
    }
    if let Some(ta) = ta {
        let mut base = int(0);
        for (index, span) in spans.iter().enumerate() {
            let offset = *offsets.get(index).ok_or_else(overflow)?;
            let Some(a) = span.audio.as_ref() else {
                continue;
            };
            if let Some((start, end)) = a.presented {
                // Up to where the next input's audio starts: an input's audio
                // never overlaps the next one's.
                let mut end = add(offset, seconds(end, ta)?)?;
                if let Some((next, next_offset)) = spans
                    .get(index.saturating_add(1))
                    .zip(offsets.get(index.saturating_add(1)))
                {
                    let next_start = match next.audio.as_ref().and_then(|n| n.presented) {
                        Some((n_start, _)) => add(*next_offset, seconds(n_start, ta)?)?,
                        None => add(*next_offset, seconds(next.video.start, tv)?)?,
                    };
                    end = min(end, next_start)?;
                }
                let samples = int(i64::try_from(a.listed.duration).map_err(|_| overflow())?);
                let local_end = scale(sub(end, offset)?, ta, 1)?;
                for (s, e, media) in shown(&a.timeline, start, local_end)? {
                    let media = sub(media, int(a.listed.first_dts))?;
                    if lt(samples, add(media, sub(e, s)?)?)? {
                        return Err(unsupported("an audio edit runs past the audio's samples"));
                    }
                    audio.push(Piece {
                        start: add(offset, seconds(s, ta)?)?,
                        end: add(offset, seconds(e, ta)?)?,
                        media: add(base, media)?,
                    });
                }
            }
            base = add(
                base,
                int(i64::try_from(a.listed.duration).map_err(|_| overflow())?),
            )?;
        }
    }
    Ok(Layout {
        inputs,
        video,
        audio,
        end: next_start,
    })
}

/// The edit list of an output track from its pieces: an empty edit before
/// any piece that starts after the previous one ends, then the piece; a
/// piece that starts where the previous one ends in both presentation and
/// media time extends it. Boundaries are rounded to the movie timescale
/// `tm`, where the previous edits end, so that rounding never accumulates.
fn edits(pieces: &[Piece], timescale: u32, tm: u32) -> Result<Vec<EditEntry>, Error> {
    let mut out: Vec<EditEntry> = Vec::new();
    let mut at = int(0);
    let ticks = |seconds: Rational| -> Result<i128, Error> { Ok(round(scale(seconds, tm, 1)?)) };
    for piece in pieces {
        let start = max(piece.start, at)?;
        if !lt(start, piece.end)? {
            continue;
        }
        let media = add(piece.media, scale(sub(start, piece.start)?, timescale, 1)?)?;
        let empty = to_u64(ticks(start)?.saturating_sub(ticks(at)?))?;
        if empty > 0 {
            out.push(EditEntry {
                duration: empty,
                media_time: -1,
                rate: UNIT_RATE,
            });
        }
        let duration = to_u64(ticks(piece.end)?.saturating_sub(ticks(start)?))?;
        let media_time = to_i64(round(media))?;
        at = piece.end;
        if duration == 0 {
            continue;
        }
        if let Some(last) = out.last_mut() {
            let last_end = add(
                int(last.media_time),
                scale(Rational::from_int(i128::from(last.duration)), timescale, tm)?,
            )?;
            if empty == 0 && last.media_time >= 0 && last_end == int(media_time) {
                last.duration = last.duration.checked_add(duration).ok_or_else(overflow)?;
                continue;
            }
        }
        out.push(EditEntry {
            duration,
            media_time,
            rate: UNIT_RATE,
        });
    }
    Ok(out)
}

/// The media time where the last media edit ends, in media ticks.
fn media_end(edits: &[EditEntry], timescale: u32, tm: u32) -> Result<u64, Error> {
    let mut end = 0u64;
    for edit in edits.iter().filter(|e| e.media_time >= 0) {
        let length = to_u64(ceil(scale(
            Rational::from_int(i128::from(edit.duration)),
            timescale,
            tm,
        )?)?)?;
        let media = u64::try_from(edit.media_time).map_err(|_| overflow())?;
        end = end.max(media.checked_add(length).ok_or_else(overflow)?);
    }
    Ok(end)
}

fn track_duration(edits: &[EditEntry]) -> u64 {
    edits
        .iter()
        .fold(0u64, |sum, e| sum.saturating_add(e.duration))
}

/// Listing 2 for one input's track: appends its samples to `tables`, the
/// last one's decode duration extended by `extended`.
#[allow(clippy::too_many_arguments)]
fn append<S: Source>(
    tables: &mut Tables,
    movie: &Movie,
    track: &Track,
    listed: &Listed,
    extended: u64,
    video: bool,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<(), ExtractError> {
    let mut count = 0u64;
    let mut duration = 0u64;
    for packet in movie.packets(track.index, &mut *source, limits, budget)? {
        let packet = packet?;
        count = count.saturating_add(1);
        duration = duration.saturating_add(u64::from(packet.duration));
        let mut delta = packet.duration;
        if count == listed.samples && extended > 0 {
            delta = u32::try_from(u64::from(delta).saturating_add(extended))
                .map_err(|_| unsupported("a decode duration above 32 bits"))?;
        }
        let offset = if video {
            packet.cts.checked_sub(packet.dts).ok_or_else(overflow)?
        } else {
            0
        };
        tables.push_sample(packet.size, delta, packet.keyframe, offset)?;
    }
    if count != listed.samples || duration != listed.duration {
        return Err(ExtractError::SourceChanged);
    }
    Ok(())
}

/// One output track's samples in the copy pass: each input's samples in
/// turn, checked against the output tables.
struct Chain<'a, S: Source, B: Iterator<Item = &'a mut Budget>> {
    movies: &'a [Movie],
    sources: &'a [RefCell<S>],
    limits: &'a [Limits],
    budgets: B,
    tracks: Vec<usize>,
    counts: Vec<u64>,
    tables: &'a Tables,
    /// The input being copied, its listing and its samples copied so far.
    current: Option<(usize, Packets<'a, Shared<'a, S>>, u64)>,
    next_input: usize,
    copied: u32,
}

impl<'a, S: Source, B: Iterator<Item = &'a mut Budget>> Chain<'a, S, B> {
    fn new(
        movies: &'a [Movie],
        sources: &'a [RefCell<S>],
        limits: &'a [Limits],
        budgets: B,
        tracks: Vec<usize>,
        counts: Vec<u64>,
        tables: &'a Tables,
    ) -> Self {
        Self {
            movies,
            sources,
            limits,
            budgets,
            tracks,
            counts,
            tables,
            current: None,
            next_input: 0,
            copied: 0,
        }
    }

    /// The input, offset and size of the next sample.
    fn next(&mut self) -> Result<(usize, u64, u32), JoinError> {
        loop {
            if let Some((input, packets, done)) = self.current.as_mut() {
                let count = self.counts.get(*input).copied().unwrap_or(0);
                if *done < count {
                    let input = *input;
                    let packet = packets
                        .next()
                        .ok_or(JoinError::Input {
                            input,
                            error: ExtractError::SourceChanged,
                        })?
                        .map_err(|e| at(input, e))?;
                    *done = done.saturating_add(1);
                    if self.tables.size(self.copied) != Some(packet.size) {
                        return Err(JoinError::Input {
                            input,
                            error: ExtractError::SourceChanged,
                        });
                    }
                    self.copied = self.copied.saturating_add(1);
                    return Ok((input, packet.offset, packet.size));
                }
            }
            // The next input: every input's budget is handed out once, in
            // order, as the chain reaches it.
            let input = self.next_input;
            let budget = self.budgets.next().ok_or(JoinError::Output(overflow()))?;
            self.next_input = input.saturating_add(1);
            let (Some(movie), Some(source), Some(limits), Some(track)) = (
                self.movies.get(input),
                self.sources.get(input),
                self.limits.get(input),
                self.tracks.get(input),
            ) else {
                return Err(JoinError::Output(overflow()));
            };
            let packets = movie
                .packets(*track, Shared(source), limits, budget)
                .map_err(|e| at(input, e))?;
            self.current = Some((input, packets, 0));
        }
    }

    /// Fails unless every sample of the tables was copied.
    fn finish(&self) -> Result<(), JoinError> {
        if self.copied != self.tables.samples() {
            return Err(JoinError::Input {
                input: self.current.as_ref().map_or(0, |c| c.0),
                error: ExtractError::SourceChanged,
            });
        }
        Ok(())
    }
}
