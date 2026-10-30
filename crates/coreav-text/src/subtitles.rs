//! 3GPP timed text to WebVTT.

use crate::sample::parse_sample;
use crate::timing::{Edit, milliseconds, unsupported};
use crate::webvtt::{HEADER, cue_text, write_cue};
use crate::{Dropped, TextError, TextLimitKind, TextLimits, TrackChoice};
use coreav_isobmff::{Allowlist, Budget, Codec, Error, Limits, Movie, Source, Track, probe};
use std::io::Write;

/// What [`extract_subtitles`] wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubtitleSummary {
    /// Position of the track among all tracks of the source.
    pub stream_index: usize,
    /// `track_ID` of the track in the source.
    pub track_id: u32,
    /// The track's codec: [`Codec::TimedText`] or [`Codec::QuickTimeText`].
    pub codec: Codec,
    /// ISO 639-2/T language code of the track, when it has one.
    pub language: Option<[u8; 3]>,
    /// Media timescale of the track.
    pub timescale: u32,
    /// Samples read.
    pub samples: u64,
    /// Cues written: one per sample with text that is presented.
    pub cues: u64,
    /// What the output does not carry.
    pub dropped: Dropped,
    /// Bytes written to the output.
    pub bytes_written: u64,
}

/// Writes the text of one text track of `source` as a WebVTT file to `out`:
/// 3GPP timed text (`tx3g`), or QuickTime text (`text`), whose samples have
/// the same layout (QuickTime File Format, "Text sample data").
///
/// Only text tracks are parsed beyond their headers (the probe's allowlist
/// holds only the two text codecs), so the sample tables of the other
/// tracks are never decoded. The track is chosen by `choice`; by default it
/// is the first one that is not a QuickTime chapter track.
///
/// Each sample with text becomes one cue, shown from the sample's
/// presentation time for its duration, cut to the track's edit, in whole
/// milliseconds (rounded to the nearest). The text is decoded (UTF-8, or
/// UTF-16 after a byte order mark) and escaped ([`cue_text`]); styling and
/// the other modifier boxes are dropped and counted in the summary, as are
/// empty, unpresented, malformed and sub-millisecond samples. The output is
/// written front to back, one sample at a time: memory is the sample tables
/// and one sample, whatever the file size.
///
/// Fails before writing anything when the file cannot be probed or the track
/// cannot be extracted: not text, external data, invalid tables, or an edit
/// list outside the supported model. Fails with a partial output when the
/// source cannot be read later on, a sample has a composition offset (a
/// `ctts` or fragment offset that is not zero), a sample exceeds
/// [`TextLimits::max_sample_bytes`], or the writer fails.
pub fn extract_subtitles<S: Source, W: Write>(
    source: S,
    limits: &Limits,
    text_limits: &TextLimits,
    choice: TrackChoice,
    mut out: W,
) -> Result<SubtitleSummary, TextError> {
    let mut source = source;
    let allowlist = Allowlist::new(&[Codec::TimedText, Codec::QuickTimeText]);
    let movie = probe(&mut source, limits, &allowlist)?;
    let track = select(&movie, choice)?;
    let mut edit = Edit::of(track, movie.timescale)?;
    let mut budget = Budget::new(limits);
    let mut summary = SubtitleSummary {
        stream_index: track.stream_index,
        track_id: track.track_id,
        codec: track.description.codec,
        language: track.language,
        timescale: track.timescale,
        samples: 0,
        cues: 0,
        dropped: Dropped::default(),
        bytes_written: 0,
    };
    let mut packets = movie.packets(track.index, &mut source, limits, &mut budget)?;
    out.write_all(HEADER)?;
    summary.bytes_written = HEADER.len() as u64;
    let mut buffer = Vec::new();
    // The parts of one sample the edits present, reused sample by sample.
    let mut shown = Vec::new();
    let mut previous_start = 0u64;
    while let Some(packet) = packets.next() {
        let packet = packet?;
        if packet.cts != packet.dts {
            return Err(unsupported("composition offsets in a text track").into());
        }
        summary.samples = summary.samples.saturating_add(1);
        if packet.size > text_limits.max_sample_bytes {
            return Err(TextError::LimitExceeded {
                limit: TextLimitKind::SampleBytes,
                offset: packet.offset,
            });
        }
        // Every part of the sample the edits present; a cue each.
        if edit.intervals(&packet, &mut shown).is_none() || shown.is_empty() {
            summary.dropped.not_presented = summary.dropped.not_presented.saturating_add(1);
            continue;
        }
        buffer.clear();
        buffer.resize(packet.size as usize, 0);
        packets
            .source_mut()
            .read_exact_at(packet.offset, &mut buffer)?;
        let Ok(sample) = parse_sample(&buffer) else {
            summary.dropped.malformed = summary.dropped.malformed.saturating_add(1);
            continue;
        };
        summary.dropped.add_sample(&sample);
        let Some(text) = cue_text(&sample.text) else {
            summary.dropped.empty = summary.dropped.empty.saturating_add(1);
            continue;
        };
        for (from, to) in &shown {
            let (Some(start), Some(end)) = (
                milliseconds(*from, track.timescale),
                milliseconds(*to, track.timescale),
            ) else {
                return Err(Error::Overflow {
                    offset: packet.offset,
                }
                .into());
            };
            // WebVTT 4.1: a cue ends after it starts, and starts no earlier
            // than the cues before it. Without composition offsets,
            // presentation times follow decode order, so only rounding and
            // the edits themselves can break either.
            if end <= start || start < previous_start {
                summary.dropped.too_short = summary.dropped.too_short.saturating_add(1);
                continue;
            }
            previous_start = start;
            let written = write_cue(&mut out, start, end, &text)?;
            summary.bytes_written = summary.bytes_written.saturating_add(written);
            summary.cues = summary.cues.saturating_add(1);
        }
    }
    out.flush()?;
    Ok(summary)
}

/// The track `choice` names.
fn select(movie: &Movie, choice: TrackChoice) -> Result<&Track, Error> {
    let track = match choice {
        TrackChoice::First => movie
            .tracks
            .iter()
            .find(|t| !movie.chapter_tracks.contains(&t.track_id))
            .ok_or(Error::NoSuchTrack)?,
        TrackChoice::Stream(index) => {
            if let Some(track) = movie.tracks.iter().find(|t| t.stream_index == index) {
                track
            } else if movie.dropped.iter().any(|d| d.stream_index == index) {
                return Err(unsupported("the stream is not a text track"));
            } else {
                return Err(Error::NoSuchTrack);
            }
        }
    };
    if track.external_data {
        return Err(unsupported("the track's data is in another file"));
    }
    if !track.tables_valid {
        return Err(unsupported("the track's sample tables are invalid"));
    }
    Ok(track)
}
