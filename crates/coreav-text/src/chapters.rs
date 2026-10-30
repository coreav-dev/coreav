//! QuickTime chapter lists (Apple QuickTime File Format, "Chapter lists").

use std::ops::ControlFlow;

use crate::sample::parse_sample;
use crate::timing::Edit;
use crate::{Dropped, TextError, TextLimitKind, TextLimits};
use coreav_isobmff::{
    Allowlist, Budget, Codec, Error, Limits, Movie, Rational, Source, Track, probe,
};

/// One chapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chapter {
    /// Start in media ticks of the chapter track on the presentation
    /// timeline: the presentation time of its sample, cut to the track's
    /// edit.
    pub start: Rational,
    /// End in media ticks: the end of its sample, cut to the track's edit.
    pub end: Rational,
    /// The sample's text, decoded (UTF-8, or UTF-16 after a byte order
    /// mark) up to its first NUL character. May be empty.
    pub title: String,
}

/// The chapter list of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chapters {
    /// Position of the chapter track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the chapter track.
    pub track_id: u32,
    /// The chapter track's codec: [`Codec::QuickTimeText`] or
    /// [`Codec::TimedText`].
    pub codec: Codec,
    /// Media timescale of the chapter track.
    pub timescale: u32,
    /// The chapters, in sample order.
    pub chapters: Vec<Chapter>,
    /// Samples read.
    pub samples: u64,
    /// What the list does not carry; `empty` and `too_short` stay 0, as
    /// empty titles and zero-length chapters are kept.
    pub dropped: Dropped,
}

/// Reads the QuickTime chapter list of `source`: the samples of the first
/// track that an enabled track references with a `chap` track reference
/// ([`coreav_isobmff::Movie::chapter_tracks`]), when it is a text track
/// (QuickTime `text` or 3GPP `tx3g`). `Ok(None)` when the file has no such
/// track. Nero `chpl` boxes are not read: no cited specification defines
/// them.
///
/// Only text tracks are parsed beyond their headers. Each presented sample
/// is one chapter, titled by its text; styling and other modifier boxes are
/// dropped and counted. Fails when the file cannot be probed, the chapter
/// track's edit list is outside the supported model, a sample has a
/// composition offset, its data is in another
/// file or its tables are invalid, or the list exceeds `text_limits`.
pub fn read_chapters<S: Source>(
    source: S,
    limits: &Limits,
    text_limits: &TextLimits,
) -> Result<Option<Chapters>, TextError> {
    let mut source = source;
    let movie = probe_text(&mut source, limits)?;
    let Some(track) = movie
        .chapter_tracks
        .iter()
        .find_map(|id| movie.tracks.iter().find(|t| t.track_id == *id))
    else {
        return Ok(None);
    };
    track_chapters(&movie, track, &mut source, limits, text_limits).map(Some)
}

/// [`read_chapters`], from each text track `track_ids` names, in order,
/// whether or not a `chap` reference names it, the file probed once:
/// `each` gets the result of each ID of a text track as it is read, so
/// only one track's chapters are held at a time, and none for other IDs.
/// Stops at the first `ControlFlow::Break` and returns it.
pub fn read_chapter_tracks<S: Source, B>(
    source: S,
    track_ids: &[u32],
    limits: &Limits,
    text_limits: &TextLimits,
    mut each: impl FnMut(Result<Chapters, TextError>) -> ControlFlow<B>,
) -> Result<ControlFlow<B>, TextError> {
    let mut source = source;
    let movie = probe_text(&mut source, limits)?;
    for track in track_ids
        .iter()
        .filter_map(|id| movie.tracks.iter().find(|t| t.track_id == *id))
    {
        if let ControlFlow::Break(b) = each(track_chapters(
            &movie,
            track,
            &mut source,
            limits,
            text_limits,
        )) {
            return Ok(ControlFlow::Break(b));
        }
    }
    Ok(ControlFlow::Continue(()))
}

fn probe_text<S: Source>(source: &mut S, limits: &Limits) -> Result<Movie, Error> {
    probe(
        source,
        limits,
        &Allowlist::new(&[Codec::QuickTimeText, Codec::TimedText]),
    )
}

/// The chapters of one text track of `movie`.
fn track_chapters<S: Source>(
    movie: &Movie,
    track: &Track,
    source: &mut S,
    limits: &Limits,
    text_limits: &TextLimits,
) -> Result<Chapters, TextError> {
    check(track)?;
    let mut edit = Edit::of(track, movie.timescale)?;
    let mut list = Chapters {
        stream_index: track.stream_index,
        track_id: track.track_id,
        codec: track.description.codec,
        timescale: track.timescale,
        chapters: Vec::new(),
        samples: 0,
        dropped: Dropped::default(),
    };
    let mut budget = Budget::new(limits);
    let mut packets = movie.packets(track.index, source, limits, &mut budget)?;
    let mut buffer = Vec::new();
    // The parts of one sample the edits present, reused sample by sample.
    let mut shown = Vec::new();
    let mut title_bytes = 0u64;
    while let Some(packet) = packets.next() {
        let packet = packet?;
        if packet.cts != packet.dts {
            return Err(TextError::Source(Error::Unsupported {
                reason: "composition offsets in a chapter track",
            }));
        }
        list.samples = list.samples.saturating_add(1);
        if packet.size > text_limits.max_sample_bytes {
            return Err(TextError::LimitExceeded {
                limit: TextLimitKind::SampleBytes,
                offset: packet.offset,
            });
        }
        // Every part of the sample the edits present; a chapter each.
        if edit.intervals(&packet, &mut shown).is_none() || shown.is_empty() {
            list.dropped.not_presented = list.dropped.not_presented.saturating_add(1);
            continue;
        }
        buffer.clear();
        buffer.resize(packet.size as usize, 0);
        packets
            .source_mut()
            .read_exact_at(packet.offset, &mut buffer)?;
        let Ok(sample) = parse_sample(&buffer) else {
            list.dropped.malformed = list.dropped.malformed.saturating_add(1);
            continue;
        };
        list.dropped.add_sample(&sample);
        let title = sample.text;
        for (start, end) in &shown {
            title_bytes = title_bytes.saturating_add(title.len() as u64);
            let exceeded = if list.chapters.len() >= text_limits.max_chapters as usize {
                Some(TextLimitKind::Chapters)
            } else if title_bytes > text_limits.max_title_bytes {
                Some(TextLimitKind::TitleBytes)
            } else {
                None
            };
            if let Some(limit) = exceeded {
                return Err(TextError::LimitExceeded {
                    limit,
                    offset: packet.offset,
                });
            }
            list.chapters.push(Chapter {
                start: *start,
                end: *end,
                title: title.clone(),
            });
        }
    }
    Ok(list)
}

fn check(track: &Track) -> Result<(), Error> {
    if track.external_data {
        return Err(Error::Unsupported {
            reason: "the chapter track's data is in another file",
        });
    }
    if !track.tables_valid {
        return Err(Error::Unsupported {
            reason: "the chapter track's sample tables are invalid",
        });
    }
    Ok(())
}
