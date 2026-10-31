//! Audio extraction to M4A, written from ISO/IEC 14496-12
//! (boxes), ISO/IEC 14496-14 (MP4 file format) and ISO/IEC 14496-3 (AAC).

use crate::audio;
use crate::copy::{COPY_BUFFER, ReadPlan, Window};
use crate::moov::{
    EditEntry, Media, MovieBox, Ties, TrackBox, TrackFlags, UNIT_RATE, ftyp, language, overflow,
    rescale, write_head,
};
use crate::mux::{RawBox, copied};
use crate::remux::{Retained, copy_boxes, movie_tags};
use crate::scan::Path;
use crate::tables::Tables;
use crate::{ExtractError, HeaderMetadata, Summary, TrackChoice};
use coreav_isobmff::{
    Allowlist, Budget, Codec, EditList, Error, FourCc, ItunesGapless, Limits, Movie, Source, Track,
    TrackKind, metadata, probe,
};
use std::io::Write;

/// The output's only track.
const TRACK_ID: u32 = 1;

/// The most regions of pass 2's reads given to a source as advice: 65,536
/// of 16 bytes, so that the list stays at 1 MiB whatever the sample tables
/// claim. A track whose reads need more is advised by its extent instead.
const MAX_REGIONS: usize = 1 << 16;

/// Copies one AAC track of `source` into an M4A file written to `out`.
///
/// The source is probed with `limits` and `allowlist`; the track is chosen by
/// `choice`; `metadata` says which of the source's header times, language,
/// handler name and `udta` and `meta` boxes the output carries. Its samples are listed once to build the sample tables, then
/// listed again and copied, so `out` receives `ftyp`, `moov` and `mdat` in
/// that order without seeking. `out` gets many small writes; give it a
/// buffered writer.
///
/// Timing: the output keeps the source's media timescale, sample durations
/// and order, and its movie timescale, so its edit list is copied exactly,
/// entry by entry, whether it is the simple model's one media edit or
/// several (`docs/design/edit-list-writers.md`, "M4A extraction"). The media header duration is the source's when it declares one
/// for a movie without fragments, even when it disagrees with the sample
/// durations; otherwise it is their sum.
/// A source without an edit list whose iTunes gapless item (`iTunSMPB`)
/// agrees exactly with its sample durations gets an edit list that skips the
/// encoder delay and the padding. Output decode times
/// start at 0; an edit's media time moves with them. A source whose first
/// decode time is not 0 and that has no edit list (a fragmented file with a
/// base decode time, say) fails rather than losing that start: whether to
/// add an edit list for it is an open question.
///
/// Fails before writing anything when the file cannot be probed or the track
/// cannot be extracted: not AAC, more than one sample description, an edit
/// list outside the supported model (`docs/design/edit-lists.md`,
/// decision 1, which leaves several media edits in a movie with fragments
/// outside it), a non-zero first decode time without an
/// edit list, composition offsets, a gap or an overlap
/// between decode times (fragments set their own base decode time), external
/// data or invalid tables. Fails with a partial output when the source cannot be
/// read later on, changes between the passes, or the writer fails.
pub fn extract_audio<S: Source, W: Write>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    choice: TrackChoice,
    metadata: HeaderMetadata,
    mut out: W,
) -> Result<Summary, ExtractError> {
    let mut source = source;
    let movie = probe(&mut source, limits, allowlist)?;
    let track = select(&movie, choice)?;
    let entry = audio_sample_entry(track)?;
    let mut budget = Budget::new(limits);

    // Pass 1: the output tables, from sizes and times only.
    let mut tables = Tables::new(track.timescale, limits.max_table_bytes);
    let mut first_dts = None;
    let mut next_dts = None;
    // Where the samples start and end in the source, for the advice to the
    // source before pass 2: the samples of other tracks lie between them.
    let mut first_offset = None;
    let mut last_end = 0u64;
    // A remote source that can skip bytes is told exactly which bytes pass
    // 2 reads. Only without fragments: pass 2 then reads nothing but
    // the samples, while fragments would have it read their boxes too.
    let mut plan = (source.wants_regions() && !movie.has_fragments())
        .then(|| {
            source
                .len()
                .map(|total| ReadPlan::new(COPY_BUFFER, total, MAX_REGIONS))
        })
        .transpose()?;
    for packet in movie.packets(track.index, &mut source, limits, &mut budget)? {
        let packet = packet?;
        first_offset.get_or_insert(packet.offset);
        last_end = last_end.max(packet.offset.saturating_add(u64::from(packet.size)));
        if let Some(plan) = plan.as_mut() {
            plan.push(packet.offset, packet.size);
        }
        if packet.cts != packet.dts {
            return Err(unsupported("composition offsets in an audio track").into());
        }
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
        tables.push(packet.size, packet.duration, packet.keyframe)?;
    }
    tables.finish();
    let first_dts = first_dts.unwrap_or(0);
    let gapless = if matches!(track.edit_list, EditList::None) && first_dts == 0 {
        gapless_edit(&movie, &mut source, &tables)?
    } else {
        None
    };
    let header = Header::new(&movie, track, &tables, first_dts, gapless, metadata)?;
    let described = match metadata {
        HeaderMetadata::Copy => described(&movie, track, gapless.is_some(), &mut source, limits)?,
        HeaderMetadata::Clear | HeaderMetadata::MediaTime => Described::default(),
    };
    let (data_start, data_bytes) = write_head(
        &mut out,
        ftyp(b"M4A ", &[b"M4A ", b"mp42", b"isom"]),
        &header.movie(&described),
        vec![header.track(entry, &tables, &described)],
        Ties::TrackOrder,
    )?;

    // Pass 2: the same packets again, copied through one buffer that also
    // serves as a read window: a sample outside it refills it from the
    // sample's start, so the samples that follow within the next 64 KiB
    // (interleaved with other tracks' data) need no read of their own. The
    // source is told what follows: the regions the pass reads, or else the
    // extent of the samples.
    let regions = plan
        .and_then(ReadPlan::finish)
        .filter(|regions| !regions.is_empty());
    match (&regions, first_offset) {
        (Some(regions), _) => source.will_read_regions(regions),
        (None, Some(offset)) => source.will_read(offset, last_end.saturating_sub(offset)),
        (None, None) => {}
    }
    let copied = copy_samples(
        &movie,
        track,
        &mut source,
        limits,
        &mut budget,
        &tables,
        &mut out,
    );
    if regions.is_some() {
        // Whatever reads the source next goes anywhere.
        source.will_read_regions(&[]);
    }
    let copied = copied?;
    if copied != tables.samples() {
        return Err(ExtractError::SourceChanged);
    }
    out.flush()?;
    Ok(Summary {
        stream_index: track.stream_index,
        track_id: track.track_id,
        samples: copied,
        bytes_written: data_start.saturating_add(data_bytes),
        timescale: track.timescale,
        duration: tables.duration,
    })
}

/// Pass 2: copies the track's samples to `out` through one window, checking
/// each size against pass 1's; returns how many it copied.
fn copy_samples<S: Source, W: Write>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    limits: &Limits,
    budget: &mut Budget,
    tables: &Tables,
    out: &mut W,
) -> Result<u32, ExtractError> {
    let mut window = Window::new(COPY_BUFFER);
    let mut copied = 0u32;
    let mut packets = movie.packets(track.index, source, limits, budget)?;
    while let Some(packet) = packets.next() {
        let packet = packet?;
        if tables.size(copied) != Some(packet.size) {
            return Err(ExtractError::SourceChanged);
        }
        window.copy(packets.source_mut(), packet.offset, packet.size, out)?;
        copied = copied.checked_add(1).ok_or(ExtractError::SourceChanged)?;
    }
    Ok(copied)
}

/// The track `choice` names: the first audio track the allowlist keeps by
/// default.
fn select(movie: &Movie, choice: TrackChoice) -> Result<&Track, Error> {
    let track = match choice {
        TrackChoice::FirstAudio => match movie.tracks.iter().find(|t| t.kind == TrackKind::Audio) {
            Some(track) => track,
            None => return Err(crate::clip::no_allowed_track(movie, TrackKind::Audio)),
        },
        TrackChoice::Stream(index) => {
            if let Some(track) = movie.tracks.iter().find(|t| t.stream_index == index) {
                track
            } else if let Some(dropped) = movie.dropped.iter().find(|d| d.stream_index == index) {
                return Err(Error::CodecNotAllowed {
                    track_id: dropped.track_id,
                    fourcc: dropped.fourcc,
                });
            } else {
                return Err(Error::NoSuchTrack);
            }
        }
    };
    if track.kind != TrackKind::Audio {
        return Err(unsupported("the stream is not an audio track"));
    }
    if track.description.codec != Codec::Aac {
        return Err(unsupported("only AAC audio can be extracted"));
    }
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
    Ok(track)
}

/// The `stsd` payload of a copy that holds part of an audio track's samples
/// (an M4A file, a clip, a split) or begins a join: the track's sample entry
/// (14496-12 12.2.3; 14496-14 5.6 for `mp4a`) as an ISO version 0 sound
/// entry with the source's channel count (as probe reports it: for AAC,
/// from the `AudioSpecificConfig` where that names one) and sample rate,
/// followed by the source's configuration box with the fields that describe
/// the whole source stream unknown ([`audio::excerpt_config`]). QuickTime
/// version 1 and 2 layouts have no place in MP4 and are written as
/// version 0. The callers choose which codecs they copy.
pub(crate) fn audio_sample_entry(track: &Track) -> Result<Vec<u8>, Error> {
    let description = &track.description;
    let kind = audio::config_kind(description.fourcc)
        .filter(|_| description.codec != Codec::Other)
        .ok_or(unsupported("the audio has no MP4 sample entry"))?;
    let config = description
        .codec_config
        .as_ref()
        .filter(|c| c.kind == kind)
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
    let entry = crate::mux::audio_sample_entry(
        description.fourcc,
        channels,
        audio::sample_size(description)
            .ok_or(unsupported("the FLAC or ALAC configuration does not parse"))?,
        rate,
        &[RawBox {
            kind,
            payload: audio::excerpt_config(config),
        }],
    )?;
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&entry);
    Ok(stsd)
}

/// The source's `iTunSMPB` item when it can stand for an edit list: the
/// movie has one audio track, and the encoder delay, the content and the
/// padding add up exactly to the track's sample durations (the item counts
/// samples at the decoded rate, which then equals the media timescale). An
/// item that disagrees, such as one describing a longer original recording,
/// is ignored rather than shifting the audio.
fn gapless_edit<S: Source>(
    movie: &Movie,
    source: &mut S,
    tables: &Tables,
) -> Result<Option<ItunesGapless>, Error> {
    let audio = movie
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .count();
    if audio != 1 {
        return Ok(None);
    }
    let Some(item) = movie.itunes_gapless(source)? else {
        return Ok(None);
    };
    let total = u64::from(item.encoder_delay)
        .checked_add(u64::from(item.padding))
        .and_then(|t| t.checked_add(item.original_samples));
    Ok((item.original_samples > 0 && total == Some(tables.duration)).then_some(item))
}

/// What describes the source's movie and track beside their headers: their
/// user data (ISO/IEC 14496-12 8.10.1) and metadata boxes (8.11.1), and the
/// track's handler name (8.4.3).
#[derive(Default)]
struct Described<'m> {
    movie: Vec<RawBox>,
    track: Vec<RawBox>,
    handler_name: Option<&'m str>,
}

/// The source's `udta` and `meta` boxes of the movie and of `track`, as
/// stored, without the movie's cover pictures: the output carries one audio
/// stream, and a cover picture is a stream of its own
/// (`docs/design/compat-cli.md`, "Cover pictures"), which a remux leaves
/// out the same way. With `gapless`, the output's edit list stands for the
/// movie's `iTunSMPB` items, which are left out too, so that the delay and
/// padding are not given twice. A box larger than what
/// `max_retained_bytes` leaves is left out. A compressed movie's boxes are
/// not copied: its file's movie box holds only the compressed data
/// (`docs/design/compressed-movie.md`).
fn described<'m, S: Source>(
    movie: &Movie,
    track: &'m Track,
    gapless: bool,
    source: &mut S,
    limits: &Limits,
) -> Result<Described<'m>, Error> {
    let handler_name = track.handler_name.as_deref();
    if movie.compressed {
        return Ok(Described {
            handler_name,
            ..Described::default()
        });
    }
    let left_out: Vec<u64> = if movie.tag_boxes().is_empty() {
        Vec::new()
    } else {
        let read = metadata::read(&mut *source, limits)?;
        let smpb = read.items.iter().filter_map(|item| match &item.value {
            metadata::ItemValue::ListItem {
                key: metadata::ListKey::Free { mean, name },
                data: Some(data),
                ..
            } if gapless
                && item.track.is_none()
                && mean.as_deref() == Some(b"com.apple.iTunes".as_slice())
                && name.as_deref() == Some(b"iTunSMPB".as_slice()) =>
            {
                Some(data.offset)
            }
            _ => None,
        });
        let copied = |at: &u64| {
            movie
                .tag_boxes()
                .iter()
                .any(|b| b.offset < *at && *at < b.end())
        };
        read.covers()
            .map(|cover| cover.offset)
            .chain(smpb)
            .filter(copied)
            .collect()
    };
    let mut retained = Retained::new(limits);
    let movie_boxes = movie_tags(
        movie.tag_boxes(),
        &left_out,
        source,
        &mut retained,
        &mut |_| {},
    )?;
    let trak_path = Path::top().with(FourCc(*b"moov")).with(FourCc(*b"trak"));
    let track_boxes = copy_boxes(
        Some(track.stream_index),
        trak_path,
        track.tag_boxes(),
        source,
        &mut retained,
    )?;
    Ok(Described {
        movie: movie_boxes,
        track: track_boxes,
        handler_name,
    })
}

/// Header values of the output movie.
struct Header {
    movie_timescale: u32,
    media_timescale: u32,
    media_duration: u64,
    movie_duration: u64,
    /// The output's edit list, in movie ticks and media ticks, empty
    /// without one.
    edits: Vec<EditEntry>,
    movie_created: u64,
    track_created: u64,
    media_created: u64,
    language: [u8; 3],
}

impl Header {
    fn movie<'a>(&self, described: &'a Described<'_>) -> MovieBox<'a> {
        MovieBox {
            timescale: self.movie_timescale,
            created: self.movie_created,
            duration: self.movie_duration,
            extra: copied(&described.movie),
        }
    }

    fn track<'a>(
        &self,
        stsd: Vec<u8>,
        tables: &'a Tables,
        described: &'a Described<'_>,
    ) -> TrackBox<'a> {
        let edits = self.edits.clone();
        TrackBox {
            track_id: TRACK_ID,
            media: Media::Audio,
            handler_name: described.handler_name,
            header: TrackFlags::DEFAULT,
            created: self.track_created,
            media_created: self.media_created,
            language: self.language,
            media_timescale: self.media_timescale,
            media_duration: self.media_duration,
            duration: self.movie_duration,
            edits: edits.into(),
            stsd,
            tables,
            extra: copied(&described.track),
            stbl_extra: Vec::new(),
        }
    }

    /// The output keeps the source's movie timescale, so the edit's
    /// durations are copied exactly; a source movie timescale of 0 (no
    /// usable movie header times) is replaced by the media timescale.
    fn new(
        movie: &Movie,
        track: &Track,
        tables: &Tables,
        first_dts: i64,
        gapless: Option<ItunesGapless>,
        metadata: HeaderMetadata,
    ) -> Result<Self, Error> {
        let media_timescale = track.timescale;
        let movie_timescale = if movie.timescale == 0 {
            media_timescale
        } else {
            movie.timescale
        };
        // The output's decode times start at 0, so every media time moves
        // with them.
        let moved = |media_time: i64| -> Result<i64, Error> {
            media_time
                .checked_sub(first_dts)
                .filter(|t| *t >= 0)
                .ok_or(unsupported("edit starts before the first sample"))
        };
        let edits: Vec<EditEntry> = match track.edit_list {
            // Without an edit list the source presents its first sample at
            // its decode time; output decode times start at 0, so that start
            // would be lost; whether to add an edit list for it is open.
            EditList::None if first_dts != 0 => {
                return Err(unsupported(
                    "first decode time is not 0 and there is no edit list",
                ));
            }
            // The gapless item's delay and content length.
            EditList::None => gapless
                .map(|g| -> Result<EditEntry, Error> {
                    Ok(EditEntry {
                        duration: rescale(g.original_samples, media_timescale, movie_timescale)?,
                        media_time: i64::from(g.encoder_delay),
                        rate: UNIT_RATE,
                    })
                })
                .transpose()?
                .into_iter()
                .collect(),
            EditList::Unsupported => {
                return Err(unsupported("edit list outside the supported model"));
            }
            EditList::Simple { .. } | EditList::Multiple if movie.timescale == 0 => {
                return Err(unsupported("edit list with movie timescale 0"));
            }
            // Every sample is copied in order and the output keeps the
            // source's timescales, so a list of several media edits is the
            // source's entries with their media times moved
            // (`docs/design/edit-list-writers.md`, "M4A extraction").
            EditList::Multiple => track
                .edits
                .iter()
                .map(|edit| -> Result<EditEntry, Error> {
                    Ok(EditEntry {
                        duration: edit.segment_duration,
                        media_time: match edit.media_time {
                            -1 => -1,
                            media_time => moved(media_time)?,
                        },
                        rate: edit.media_rate,
                    })
                })
                .collect::<Result<_, _>>()?,
            EditList::Simple { segment: 0, .. } => {
                return Err(unsupported("an edit that lasts to the end of the media"));
            }
            EditList::Simple {
                empty,
                media_time,
                segment,
            } => {
                let media_time = moved(media_time)?;
                let mut entries = Vec::with_capacity(2);
                if empty > 0 {
                    entries.push(EditEntry {
                        duration: empty,
                        media_time: -1,
                        // An empty edit presents nothing at any rate, and
                        // the simple model reads it without one; the
                        // output states the rate it means.
                        rate: UNIT_RATE,
                    });
                }
                entries.push(EditEntry {
                    duration: segment,
                    media_time,
                    rate: UNIT_RATE,
                });
                entries
            }
        };
        // The declared media duration is copied, also when it disagrees with
        // the sample durations (probe then reports the same duration and
        // warning for the output as for the source).
        // With fragments the movie header durations cover only part of the
        // samples, so the sum is written.
        let media_duration = if !movie.has_fragments() && track.mdhd_duration != 0 {
            track.mdhd_duration
        } else {
            tables.duration
        };
        // The edits play one after another, so the movie lasts as long as
        // they add up to (14496-12 8.6.6); without them, as long as the
        // media.
        let movie_duration = if edits.is_empty() {
            rescale(media_duration, media_timescale, movie_timescale)?
        } else {
            edits
                .iter()
                .try_fold(0u64, |sum, e| sum.checked_add(e.duration))
                .ok_or_else(overflow)?
        };
        let (movie_created, track_created, media_created, language) = match metadata {
            HeaderMetadata::Copy => (
                movie.creation_time,
                track.creation_time,
                track.media_creation_time,
                language(track.language),
            ),
            HeaderMetadata::Clear => (0, 0, 0, *b"und"),
            HeaderMetadata::MediaTime => (
                track.media_creation_time,
                track.media_creation_time,
                track.media_creation_time,
                *b"und",
            ),
        };
        Ok(Self {
            movie_timescale,
            media_timescale,
            media_duration,
            movie_duration,
            edits,
            movie_created,
            track_created,
            media_created,
            language,
        })
    }
}

pub(crate) fn unsupported(reason: &'static str) -> Error {
    Error::Unsupported { reason }
}
