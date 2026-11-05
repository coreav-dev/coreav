use super::{
    AUDIO_ID, Budget, COPY_BUFFER, ClipSummary, ClipTrack, Codec, EditEntry, EditList,
    ExtractError, Limits, Media, Movie, MovieBox, Rational, RefCell, Run, Shared, Source, Tables,
    Ties, Track, TrackBox, TrackFlags, UNIT_RATE, VIDEO_ID, Window, add, copy_cut, ftyp, int,
    language, movie_timescale, overflow, scale, to_i64, to_u64, unsupported, write_head,
};
use crate::moov::{CopiedBox, edited_decode_start};
use crate::replace::roll_boxes_all;
use crate::scan;
use coreav_isobmff::{Error, Packet, Profile};
use std::borrow::Cow;
use std::io::Write;

fn unqualified() -> Error {
    unsupported("an input-seek copy with an unqualified audio-bearing packet timeline")
}

fn ticks(time: Rational, clock: u32) -> Result<i64, Error> {
    let value = scale(time, clock, 1)?;
    if value.den != 1 {
        return Err(unqualified());
    }
    to_i64(value.num)
}

fn edit(track: &Track) -> Result<i64, Error> {
    match track.edit_list {
        EditList::Simple {
            empty: 0,
            media_time,
            segment,
        } if media_time >= 0
            && segment > 0
            && track.edits.len() == 1
            && track
                .edits
                .first()
                .is_some_and(|e| e.media_rate == UNIT_RATE) =>
        {
            Ok(media_time)
        }
        _ => Err(unqualified()),
    }
}

struct Composition {
    duration: Option<u32>,
    next: Option<i64>,
    times: Vec<i64>,
    samples: usize,
}

impl Composition {
    fn new(samples: u64, limit: u64) -> Result<Self, Error> {
        let bytes = samples.checked_mul(8).ok_or_else(overflow)?;
        if bytes > limit {
            return Err(table_limit());
        }
        let samples = usize::try_from(samples).map_err(|_| table_limit())?;
        let mut times = Vec::new();
        times
            .try_reserve_exact(samples)
            .map_err(|_| table_limit())?;
        Ok(Self {
            duration: None,
            next: None,
            times,
            samples,
        })
    }

    fn push(&mut self, packet: &Packet) -> Result<(), Error> {
        if packet.duration == 0
            || self.duration.is_some_and(|d| d != packet.duration)
            || self.next.is_some_and(|d| d != packet.dts)
            || packet.cts < packet.dts
            || self.times.len() == self.samples
        {
            return Err(unqualified());
        }
        self.duration = Some(packet.duration);
        self.next = Some(
            packet
                .dts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        self.times.push(packet.cts);
        Ok(())
    }

    fn finish(mut self) -> Result<(), Error> {
        self.times.sort_unstable();
        if self.times.len() != self.samples
            || self
                .times
                .windows(2)
                .any(|pair| pair.first() == pair.get(1))
        {
            return Err(unqualified());
        }
        Ok(())
    }
}

fn table_limit() -> Error {
    Error::LimitExceeded {
        limit: coreav_isobmff::LimitKind::TableBytes,
        offset: 0,
    }
}

fn full_span(movie: &Movie, track: &Track, media: i64, end: i64) -> Result<(), Error> {
    let EditList::Simple { segment, .. } = track.edit_list else {
        return Err(unqualified());
    };
    let span = end.checked_sub(media).ok_or_else(overflow)?;
    if i128::from(span) * i128::from(movie.timescale)
        != i128::from(segment) * i128::from(track.timescale)
    {
        return Err(unqualified());
    }
    Ok(())
}

fn video_start<S: Source>(
    movie: &Movie,
    video: &Track,
    media: i64,
    start: i64,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Packet, Error> {
    let format = movie.access_format(video.index).ok_or_else(unqualified)?;
    let mut packets = movie.packets(video.index, source, limits, budget)?;
    let mut chosen = None;
    let mut composition = Composition::new(u64::from(video.sample_count), limits.max_table_bytes)?;
    let mut end = 0;
    let mut last_sync = None;
    let target = media.checked_add(start).ok_or_else(overflow)?;
    while let Some(packet) = packets.next() {
        let packet = packet?;
        composition.push(&packet)?;
        if packet.sample == 0 && packet.cts != media {
            return Err(unqualified());
        }
        end = end.max(
            packet
                .cts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        if packet.keyframe {
            if last_sync.is_some_and(|last| last >= packet.cts) {
                return Err(unqualified());
            }
            last_sync = Some(packet.cts);
            if packet.cts <= target {
                if packets.starts_decoding_at(
                    format,
                    packet.offset,
                    packet.size,
                    movie.max_num_ref_frames(video.index),
                )? != Some(true)
                {
                    return Err(unqualified());
                }
                chosen = Some(packet);
            }
        }
    }
    composition.finish()?;
    full_span(movie, video, media, end)?;
    chosen.ok_or_else(unqualified)
}

fn audio_start<S: Source>(
    movie: &Movie,
    audio: &Track,
    media: i64,
    target: Rational,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<u64, Error> {
    let packets = movie.packets(audio.index, source, limits, budget)?;
    let denominator = i128::from(target.den);
    let target = target.num;
    let mut chosen = None;
    let mut first = None;
    let mut next = None;
    let mut end = 0;
    for packet in packets {
        let packet = packet?;
        if packet.duration == 0 || packet.cts != packet.dts || next.is_some_and(|d| d != packet.dts)
        {
            return Err(unqualified());
        }
        next = Some(
            packet
                .dts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        end = next.ok_or_else(overflow)?;
        first.get_or_insert(packet.sample);
        let time = i128::from(packet.dts.checked_sub(media).ok_or_else(overflow)?) * denominator;
        if target % denominator != 0 && time.abs_diff(target) <= denominator as u128 {
            return Err(unqualified());
        }
        if time <= target {
            chosen = Some(packet.sample);
        }
    }
    full_span(movie, audio, media, end)?;
    chosen.or(first).ok_or_else(unqualified)
}

struct Kept {
    first: u64,
    tables: Tables,
    edits: Vec<EditEntry>,
    presented: u64,
}

#[allow(clippy::too_many_arguments)]
fn build<S: Source>(
    movie: &Movie,
    track: &Track,
    first: u64,
    shift: i64,
    end: i64,
    media: i64,
    movie_clock: u32,
    video: Option<Packet>,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
    table_budget: u64,
) -> Result<Kept, ExtractError> {
    let until = media.checked_add(end).ok_or_else(overflow)?;
    let samples = video
        .map(|key| {
            let span = u64::try_from(until.checked_sub(key.dts).ok_or_else(overflow)?)
                .map_err(|_| unqualified())?;
            let count = span.div_ceil(u64::from(key.duration));
            Ok::<_, Error>(
                count.min(
                    u64::from(track.sample_count)
                        .checked_sub(first)
                        .ok_or_else(overflow)?,
                ),
            )
        })
        .transpose()?;
    let proof_bytes = samples.unwrap_or(0).checked_mul(8).ok_or_else(overflow)?;
    let table_bytes = table_budget
        .checked_sub(proof_bytes)
        .ok_or_else(table_limit)?;
    let mut composition = samples
        .map(|count| Composition::new(count, proof_bytes))
        .transpose()?;
    let mut tables = Tables::with_chunk_bytes(track.timescale, table_bytes, 0);
    let packets = movie.packets(track.index, source, limits, budget)?;
    let mut initial = None;
    let mut next = None;
    let mut max_end = i64::MIN;
    for packet in packets {
        let packet = packet?;
        if packet.sample < first {
            continue;
        }
        if packet.dts >= until {
            break;
        }
        if packet.duration == 0 || next.is_some_and(|d| d != packet.dts) {
            return Err(unqualified().into());
        }
        next = Some(
            packet
                .dts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        let (first_dts, first_cts) = *initial.get_or_insert((packet.dts, packet.cts));
        if let Some(proof) = &mut composition {
            proof.push(&packet)?;
            if packet.cts < first_cts {
                return Err(unqualified().into());
            }
        } else if packet.cts != packet.dts {
            return Err(unqualified().into());
        }
        max_end = max_end.max(
            packet
                .cts
                .checked_add(i64::from(packet.duration))
                .ok_or_else(overflow)?,
        );
        tables.push_sample(
            packet.size,
            packet.duration,
            packet.keyframe,
            packet.cts.checked_sub(packet.dts).ok_or_else(overflow)?,
        )?;
        if packet.dts < first_dts {
            return Err(unqualified().into());
        }
    }
    if let Some(proof) = composition {
        proof.finish()?;
    }
    tables.finish();
    let (first_dts, _) = initial.ok_or_else(unqualified)?;
    let origin = media.checked_add(shift).ok_or_else(overflow)?;
    let media_time = origin
        .checked_sub(first_dts)
        .filter(|v| *v >= 0)
        .ok_or_else(unqualified)?;
    let presented = max_end
        .checked_sub(origin)
        .filter(|v| *v > 0)
        .ok_or_else(unqualified)?;
    let scaled = scale(int(presented), movie_clock, track.timescale)?;
    if scaled.den != 1 {
        return Err(unqualified().into());
    }
    let duration = to_u64(scaled.num)?;
    let edits = vec![EditEntry {
        duration,
        media_time,
        rate: UNIT_RATE,
    }];
    tables.decode_start = edited_decode_start(&edits, track.timescale, movie_clock)?;
    Ok(Kept {
        first,
        tables,
        edits,
        presented: u64::try_from(presented).map_err(|_| overflow())?,
    })
}

fn groups<S: Source>(
    video: &Track,
    audio: &Track,
    source: &mut S,
    limits: &Limits,
) -> Result<(TrackFlags, TrackFlags), Error> {
    let scan = scan::scan(source, limits)?;
    if scan.skipped.more > 0 || !scan.skipped.listed.is_empty() {
        return Err(unqualified());
    }
    let v = scan.traks.get(video.stream_index).ok_or_else(unqualified)?;
    let a = scan.traks.get(audio.stream_index).ok_or_else(unqualified)?;
    for track in [v, a] {
        if track.skipped.more > 0 || !track.skipped.listed.is_empty() || !track.tags.is_empty() {
            return Err(unqualified());
        }
    }
    if !v.sample_info.is_empty() {
        return Err(unqualified());
    }
    let mut description = false;
    let mut membership = false;
    for header in &a.sample_info {
        match &header.kind.0 {
            b"sgpd" if !description && header.payload_len() == 18 => {
                let mut bytes = [0; 18];
                source.read_exact_at(header.payload_offset(), &mut bytes)?;
                if bytes
                    != [
                        1, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 1, 255, 255,
                    ]
                {
                    return Err(unqualified());
                }
                description = true;
            }
            b"sbgp" if !membership && header.payload_len() == 20 => {
                let mut bytes = [0; 20];
                source.read_exact_at(header.payload_offset(), &mut bytes)?;
                let mut expected = [
                    0, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1,
                ];
                expected
                    .get_mut(12..16)
                    .ok_or_else(overflow)?
                    .copy_from_slice(&audio.sample_count.to_be_bytes());
                if bytes != expected {
                    return Err(unqualified());
                }
                membership = true;
            }
            _ => return Err(unqualified()),
        }
    }
    if description != membership {
        return Err(unqualified());
    }
    Ok((
        v.flags.ok_or_else(unqualified)?,
        a.flags.ok_or_else(unqualified)?,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn copy_av<S: Source, W: Write>(
    movie: &Movie,
    video: &Track,
    audio: &Track,
    start: Rational,
    end: Option<Rational>,
    mut source: S,
    limits: &Limits,
    mut out: W,
    entries: (Vec<u8>, Vec<u8>),
) -> Result<ClipSummary, ExtractError> {
    if movie.has_fragments()
        || movie.tracks.len() != 2
        || !movie.dropped.is_empty()
        || video.description.fourcc.0 != *b"avc1"
        || video.timescale < 10_000
        || audio.description.fourcc.0 != *b"mp4a"
        || audio.description.codec != Codec::Aac
        || !matches!(
            audio.description.profile,
            Some(Profile::Aac {
                object_type: 2,
                sbr: false,
                ps: false
            })
        )
        || video.handler_name.as_deref() != Some("VideoHandler")
        || audio.handler_name.as_deref() != Some("SoundHandler")
    {
        return Err(unqualified().into());
    }
    let end = end.ok_or_else(unqualified)?;
    let video_edit = edit(video)?;
    let audio_edit = edit(audio)?;
    let sv = ticks(start, video.timescale)?;
    let sa = ticks(start, audio.timescale)?;
    let ev = ticks(end, video.timescale)?;
    let ea = ticks(end, audio.timescale)?;
    let clock = movie_timescale(video.timescale, audio.timescale);
    if clock % video.timescale != 0 || clock % audio.timescale != 0 {
        return Err(unqualified().into());
    }
    let flags = groups(video, audio, &mut source, limits)?;
    let mut budget = Budget::new(limits);
    let key = video_start(
        movie,
        video,
        video_edit,
        sv,
        &mut source,
        limits,
        &mut budget,
    )?;
    let target = i128::from(key.dts.checked_sub(video_edit).ok_or_else(overflow)?)
        * i128::from(audio.timescale);
    let first_audio = audio_start(
        movie,
        audio,
        audio_edit,
        Rational::new(target, u64::from(video.timescale)).ok_or_else(overflow)?,
        &mut source,
        limits,
        &mut budget,
    )?;
    let listing_work = budget.used();
    let video_kept = build(
        movie,
        video,
        key.sample,
        sv,
        ev,
        video_edit,
        clock,
        Some(key),
        &mut source,
        limits,
        &mut budget,
        limits.max_table_bytes,
    )?;
    let audio_kept = build(
        movie,
        audio,
        first_audio,
        sa,
        ea,
        audio_edit,
        clock,
        None,
        &mut source,
        limits,
        &mut budget,
        limits
            .max_table_bytes
            .saturating_sub(video_kept.tables.used()),
    )?;
    if budget
        .used()
        .checked_add(listing_work)
        .is_none_or(|work| work > limits.max_sample_work)
    {
        return Err(Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }
    let roll = roll_boxes_all(audio_kept.tables.samples(), 1)
        .into_iter()
        .map(|entry| CopiedBox {
            kind: entry.kind.0,
            payload: Cow::Owned(entry.payload),
        })
        .collect();
    let video_duration = video_kept.edits.first().ok_or_else(overflow)?.duration;
    let audio_duration = audio_kept.edits.first().ok_or_else(overflow)?.duration;
    let tracks = vec![
        TrackBox {
            track_id: VIDEO_ID,
            media: Media::Video {
                matrix: video.matrix,
                size: video.presentation_size,
            },
            handler_name: None,
            header: flags.0,
            created: video.creation_time,
            modified: None,
            media_created: video.media_creation_time,
            media_modified: None,
            language: language(video.language),
            media_timescale: video.timescale,
            media_duration: video_kept.tables.duration,
            duration: video_duration,
            edits: Cow::Borrowed(&video_kept.edits),
            stsd: entries.0,
            tables: &video_kept.tables,
            extra: Vec::new(),
            stbl_extra: Vec::new(),
        },
        TrackBox {
            track_id: AUDIO_ID,
            media: Media::Audio,
            handler_name: None,
            header: flags.1,
            created: audio.creation_time,
            modified: None,
            media_created: audio.media_creation_time,
            media_modified: None,
            language: language(audio.language),
            media_timescale: audio.timescale,
            media_duration: audio_kept.tables.duration,
            duration: audio_duration,
            edits: Cow::Borrowed(&audio_kept.edits),
            stsd: entries.1,
            tables: &audio_kept.tables,
            extra: Vec::new(),
            stbl_extra: roll,
        },
    ];
    let (data_start, data_bytes) = write_head(
        &mut out,
        ftyp(b"isom", &[b"isom", b"mp42", b"avc1"]),
        &MovieBox {
            timescale: clock,
            created: movie.creation_time,
            modified: None,
            duration: video_duration.max(audio_duration),
            extra: Vec::new(),
        },
        tracks,
        Ties::TrackOrder,
    )?;
    let remaining = limits.max_sample_work.saturating_sub(budget.used());
    let copy_limits = Limits {
        max_sample_work: remaining,
        ..*limits
    };
    let shared = RefCell::new(source);
    let mut vb = Budget::new(&copy_limits);
    let mut ab = Budget::new(&copy_limits);
    let mut vp = movie.packets(video.index, Shared(&shared), &copy_limits, &mut vb)?;
    let mut ap = movie.packets(audio.index, Shared(&shared), &copy_limits, &mut ab)?;
    let vr = Run::new(&mut vp, video_kept.first, &video_kept.tables)?;
    let ar = Run::new(&mut ap, audio_kept.first, &audio_kept.tables)?;
    let mut window = Window::new(COPY_BUFFER);
    copy_cut(&shared, vr, Some(ar), &mut window, &mut out)?;
    drop((vp, ap, window));
    if vb.used().saturating_add(ab.used()) > remaining {
        return Err(Error::LimitExceeded {
            limit: coreav_isobmff::LimitKind::SampleWork,
            offset: 0,
        }
        .into());
    }
    out.flush()?;
    let summary = |track: &Track, kept: &Kept| ClipTrack {
        stream_index: track.stream_index,
        track_id: track.track_id,
        codec: track.description.codec,
        first_sample: kept.first,
        samples: kept.tables.samples(),
        timescale: track.timescale,
        duration: kept.tables.duration,
    };
    Ok(ClipSummary {
        video: summary(video, &video_kept),
        audio: Some(summary(audio, &audio_kept)),
        start,
        snapped_start: scale(
            int(key.cts.checked_sub(video_edit).ok_or_else(overflow)?),
            1,
            video.timescale,
        )?,
        end: Some(end),
        written_end: add(
            start,
            scale(
                int(to_i64(i128::from(video_kept.presented))?),
                1,
                video.timescale,
            )?,
        )?,
        bytes_written: data_start.checked_add(data_bytes).ok_or_else(overflow)?,
        left_out: Vec::new(),
        boxes_left_out: video.description.other_boxes.clone(),
    })
}
