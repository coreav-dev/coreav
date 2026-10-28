//! Building the index from a segment index, without reading the fragments
//! (design note `docs/design/hls.md`, "Sources with a segment index").
//!
//! A fragmented source whose `sidx` lists every subsegment already says
//! where each one starts, how long it lasts, how many bytes it holds and
//! that it starts at a stream access point. That is what the playlists
//! need, so the index records each subsegment's byte range instead of its
//! samples, and a request reads that one `moof`. It is the work
//! libavformat's open does on the same file; listing every fragment is
//! not.

use super::build::{Timing, bit_rates, header_of};
use super::index::{Built, SegmentEntry};
use super::playlist::target_duration;
use super::segment::Sample;
use super::{HlsOptions, Rendition};
use crate::ExtractError;
use crate::moov::overflow;
use coreav_isobmff::{
    Budget, EditList, Error, Limits, MAX_HEADER_LEN, Movie, Parsed, Rational, SegmentIndex, Source,
    Track, TrackDefaults, parse_box_header, subsegment_samples,
};

/// The `SAP_type` values whose subsegment decodes on its own and presents
/// nothing before its first decoded sample (ISO/IEC 14496-12 Annex I):
/// a rendition of these alone carries `EXT-X-INDEPENDENT-SEGMENTS`.
const CLOSED_SAP: [u8; 2] = [1, 2];
/// The largest `SAP_type` a segment can start at: 1 to 3 start with a
/// sample that decodes on its own, 0 says only that the subsegment starts
/// at an access point, and 4 and above are gradual refreshes, which a
/// player cannot start from. `SAP_delta_time` is not read: packagers
/// write it against the decode time as often as against the earliest
/// presentation time the specification names, so a segment's first sample
/// is checked against the fragment instead, here for the first subsegment
/// and per request for every other.
const MAX_SAP: u8 = 3;
/// Top-level boxes that may follow the media a segment index lists: a
/// movie fragment random access box, its size, and padding.
const AFTER_MEDIA: [[u8; 4]; 5] = [*b"mfra", *b"mfro", *b"free", *b"skip", *b"wide"];
/// Most of them, so a file of padding does not cost a read apiece.
const MAX_AFTER_MEDIA: u32 = 8;

/// One packaged track as its segment index describes it.
struct Planned<'a> {
    track: &'a Track,
    rendition: Rendition,
    index: &'a SegmentIndex,
    timing: Timing,
    /// Presentation start of each subsegment in media ticks, the first 0.
    starts: Vec<u64>,
    /// Presentation end in media ticks.
    end: u64,
    /// Shortest sample duration of the first fragment, for the maximum
    /// frame rate.
    min_sample_duration: u32,
}

/// Builds the packaged rendition from the source's segment index, or
/// returns `None` when it cannot be and the source is listed as usual.
/// Each condition is one thing a segment index does not say.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan<S: Source>(
    movie: &Movie,
    renditions: &[(&Track, Rendition, &[u8])],
    source: &mut S,
    source_len: u64,
    limits: &Limits,
    options: &HlsOptions,
    budget: &mut Budget,
) -> Result<Option<Vec<Built>>, ExtractError> {
    // One rendition only. Two renditions of one source would have to
    // share a timeline, which needs their subsegments to start at the
    // same presentation times, and their segment indexes would name the
    // same byte ranges, so neither could say how many of those bytes are
    // its own (the bit rates below). A fragmented source CoreAV packages
    // as two renditions is listed instead.
    let [(track, rendition, stsd)] = renditions else {
        return Ok(None);
    };
    let Some(index) = segment_index(movie, track, source, source_len)? else {
        return Ok(None);
    };
    let timing = Timing::of(movie, track, &mut *source)?;
    let Some((starts, end)) = timeline(index, &timing) else {
        return Ok(None);
    };
    // The one field the segment index cannot give, and the check that a
    // subsegment of this source lists on its own. A source that does not
    // list there is listed whole, where its fragments say where they lie:
    // the error, if there is one, belongs to that path.
    let listed = first_subsegment(
        track, *rendition, index, &timing, source, source_len, limits, budget,
    );
    let Ok(Some(min_sample_duration)) = listed else {
        return Ok(None);
    };
    let planned = Planned {
        track,
        rendition: *rendition,
        index,
        timing,
        starts,
        end,
        min_sample_duration,
    };
    Ok(Some(vec![finish(
        &planned,
        stsd,
        options.segment_duration,
    )?]))
}

/// The segment index that lists every one of `track`'s samples, when one
/// does: the track has no samples in the movie box, the file has
/// fragments, and one index describes the track's subsegments, each
/// starting a closed access point.
fn segment_index<'a, S: Source>(
    movie: &'a Movie,
    track: &Track,
    source: &mut S,
    source_len: u64,
) -> Result<Option<&'a SegmentIndex>, Error> {
    let (Some(first_fragment), 0, Some(_)) = (
        movie.first_fragment(),
        track.sample_count,
        track.fragment_defaults(),
    ) else {
        return Ok(None);
    };
    // A fragment that may hold another track makes the subsegment's bytes
    // more than this track's, and `referenced_size` is what the bit rates
    // are taken from.
    if movie.fragment_tracks() != 1 {
        return Ok(None);
    }
    let mut found = movie
        .segment_indexes
        .iter()
        .filter(|index| index.reference_id == track.track_id);
    let Some(index) = found.next() else {
        return Ok(None);
    };
    // A second index for the track would list its subsegments twice, and
    // durations in another timescale would not be media ticks.
    if found.next().is_some() || index.timescale != track.timescale {
        return Ok(None);
    }
    let listed = !index.references.is_empty()
        && index.references.iter().all(|reference| {
            !reference.is_index
                && reference.starts_with_sap
                && reference.sap_type <= MAX_SAP
                && reference.duration > 0
                && reference.size > 0
        });
    if !listed {
        return Ok(None);
    }
    // The references must name every fragment of the track, not just the
    // first subsegment: probe reads the segment indexes that come before
    // the first fragment, so a file with one index per subsegment (as
    // `ffmpeg -movflags +dash` writes) offers only its first. The
    // references are contiguous by construction, so it is enough that
    // they begin at or before the first fragment and end where the media
    // does.
    let (Some(first), Some(last)) = (index.references.first(), index.references.last()) else {
        return Ok(None);
    };
    if first.offset > first_fragment {
        return Ok(None);
    }
    let end = last
        .offset
        .checked_add(u64::from(last.size))
        .ok_or_else(overflow)?;
    Ok(ends_the_media(source, end, source_len)?.then_some(index))
}

/// Whether nothing but a random access box or padding follows the media
/// at `from`: the boxes from there to the end of the file are all
/// [`AFTER_MEDIA`].
fn ends_the_media<S: Source>(source: &mut S, from: u64, source_len: u64) -> Result<bool, Error> {
    let mut at = from;
    for _ in 0..MAX_AFTER_MEDIA {
        if at >= source_len {
            return Ok(at == source_len);
        }
        let left = source_len.checked_sub(at).ok_or_else(overflow)?;
        let want = usize::try_from(left.min(MAX_HEADER_LEN as u64)).map_err(|_| overflow())?;
        let mut head = [0u8; MAX_HEADER_LEN];
        let Some(buffer) = head.get_mut(..want) else {
            return Ok(false);
        };
        source.read_exact_at(at, buffer)?;
        // Bytes that are not a box end the walk: the references stop
        // inside the media, which is what this asks about.
        let Ok(Parsed::Done(header)) = parse_box_header(buffer, at, left) else {
            return Ok(false);
        };
        if !AFTER_MEDIA.contains(&header.kind.0) || header.end() <= at {
            return Ok(false);
        }
        at = header.end();
    }
    Ok(false)
}

/// Each subsegment's presentation start and the presentation end, in media
/// ticks, when the edit presents every sample. An edit that hides the start
/// or the end leaves out samples that only the fragments name, and an empty
/// edit shifts decode times that only the fragments carry, so there is no
/// plan then.
///
/// An edit whose start falls inside the first subsegment leaves nothing
/// out: the listing keeps every sample from the latest sync sample at or
/// before the edit's start that every earlier sample is presented before,
/// and in the first subsegment that is its own first sample (the access
/// point `CLOSED_SAP` promises). That the edit opens no later subsegment
/// is what the strict bound below says; that it opens no sample inside the
/// first one is checked against that fragment ([`first_fragment`]).
fn timeline(index: &SegmentIndex, timing: &Timing) -> Option<(Vec<u64>, u64)> {
    if timing.empty != 0 || timing.media_time < 0 {
        return None;
    }
    let first_duration = i128::from(index.references.first()?.duration);
    let earliest = i128::from(index.earliest_presentation_time);
    if i128::from(timing.media_time) >= earliest.checked_add(first_duration)? {
        return None;
    }
    let mut starts = Vec::with_capacity(index.references.len());
    let mut at = earliest;
    for reference in &index.references {
        let start = at.checked_sub(i128::from(timing.media_time))?;
        starts.push(if starts.is_empty() {
            0
        } else {
            u64::try_from(start).ok()?
        });
        at = at.checked_add(i128::from(reference.duration))?;
    }
    let end = u64::try_from(at.checked_sub(i128::from(timing.media_time))?).ok()?;
    // A media edit that ends before the media does hides the samples
    // after it.
    if let Some(window) = timing.window_end() {
        if i128::from(end) > i128::from(window) {
            return None;
        }
    }
    Some((starts, end))
}

/// The shortest sample duration of the first subsegment, for the maximum
/// frame rate (RFC 8216 4.3.4.2); `None` when this source's subsegments
/// do not list on their own, or when the edit opens the media inside the
/// first one.
///
/// A subsegment must hold at least one movie fragment that says where its
/// data lies, must keep that data inside itself, and must carry a decode
/// time, which a request takes from it. The edit check repeats the
/// listing's rule on this one subsegment: a sync sample after the first
/// that the edit's start is at or after, and that every earlier sample is
/// presented before, would make the listing drop everything before it,
/// which the segment index cannot say.
#[allow(clippy::too_many_arguments)]
fn first_subsegment<S: Source>(
    track: &Track,
    rendition: Rendition,
    index: &SegmentIndex,
    timing: &Timing,
    source: &mut S,
    source_len: u64,
    limits: &Limits,
    budget: &mut Budget,
) -> Result<Option<u32>, Error> {
    let (Some(defaults), Some(first)) = (track.fragment_defaults(), index.references.first())
    else {
        return Ok(None);
    };
    let listed = subsegment_samples(
        source,
        first.offset,
        u64::from(first.size),
        source_len,
        defaults,
        limits,
        budget,
    )?;
    if listed.base_decode_time.is_none()
        || !listed.runs.first().is_some_and(|run| run.keyframe)
        || !inside(&listed, first.offset, u64::from(first.size))
    {
        return Ok(None);
    }
    // Only a video rendition restarts at a later sync sample; an audio one
    // keeps every sample a sync flag marks (design note, "Timing").
    // An edit with a media time of 0 still hides samples presented before
    // it (negative composition offsets), which can end at a sync sample.
    if rendition == Rendition::Video && track.edit_list != EditList::None {
        let mut latest: Option<i64> = None;
        let mut sample = 0u64;
        for run in &listed.runs {
            for i in 0..run.count {
                let Some(dts) = run.sample_dts(i) else {
                    return Ok(None);
                };
                let Some(p) = dts
                    .checked_add(run.composition_offset)
                    .and_then(|cts| cts.checked_sub(timing.media_time))
                else {
                    return Ok(None);
                };
                if sample > 0 && run.keyframe && p <= 0 && latest.is_some_and(|l| p > l) {
                    return Ok(None);
                }
                latest = Some(latest.map_or(p, |l| l.max(p)));
                sample = sample.saturating_add(1);
            }
        }
    }
    // The track's own rule (`Rate`) needs every sample; one fragment's
    // shortest duration stands for the rate of a uniform packaging, which
    // is what a segment index describes.
    Ok(Some(
        listed
            .runs
            .iter()
            .map(|run| run.sample_duration)
            .filter(|duration| *duration > 0)
            .min()
            .unwrap_or(0),
    ))
}

/// Whether every sample of a listed fragment lies in the subsegment at
/// `offset` of `size` bytes: a request reads that range alone.
fn inside(listed: &coreav_isobmff::FragmentSamples, offset: u64, size: u64) -> bool {
    let end = offset.saturating_add(size);
    listed.runs.iter().all(|run| {
        run.offset >= offset && run.offset.saturating_add(run.byte_len()) <= end && run.count > 0
    })
}

/// The rendition's header and directory.
fn finish(plan: &Planned<'_>, stsd: &[u8], target: Rational) -> Result<Built, Error> {
    let mut track = header_of(plan.track, plan.rendition)?;
    let mut segments = Vec::with_capacity(plan.index.references.len());
    let mut data_bytes = 0u64;
    let sizes: Vec<u64> = plan
        .index
        .references
        .iter()
        .map(|reference| u64::from(reference.size))
        .collect();
    for (reference, start) in plan.index.references.iter().zip(&plan.starts) {
        let size = u64::from(reference.size);
        data_bytes = data_bytes.checked_add(size).ok_or_else(overflow)?;
        segments.push(SegmentEntry {
            start: *start,
            // The fragment's `tfdt` carries it; the directory does not.
            base_decode_time: 0,
            // Unknown until the fragment is read.
            samples: 0,
            // The subsegment's bytes, header included: what RFC 8216
            // 4.3.4.2 counts, and what a request reads.
            data_bytes: size,
            records: reference.offset,
            records_len: reference.size,
        });
    }
    track.deferred = plan.track.fragment_defaults();
    track.segments = segments;
    track.samples = 0;
    track.data_bytes = data_bytes;
    // Only where every subsegment starts a closed access point: an open
    // GOP's leading samples need the subsegment before it, and a type of
    // 0 says nothing.
    track.independent = plan
        .index
        .references
        .iter()
        .all(|reference| CLOSED_SAP.contains(&reference.sap_type));
    track.end = plan.end;
    track.min_sample_duration = plan.min_sample_duration;
    track.edit = plan.timing.edit()?;
    track.stsd = stsd.to_vec();
    let durations: Vec<u64> = (0..track.segments.len())
        .map(|n| {
            let start = track.segments.get(n).map_or(0, |s| s.start);
            track.segment_end(n).unwrap_or(start).saturating_sub(start)
        })
        .collect();
    let target = target_duration(&track).map_or(target, Rational::from_int);
    let (peak, average) = bit_rates(&durations, &sizes, track.timescale, target)?;
    track.peak_bit_rate = peak;
    track.average_bit_rate = average;
    Ok(Built {
        track,
        records: Vec::new(),
    })
}

/// The samples of a deferred segment: lists the subsegment the entry
/// names and checks that every sample lies in it.
pub(crate) fn segment_samples<S: Source>(
    source: &mut S,
    entry: &SegmentEntry,
    defaults: TrackDefaults,
    source_len: u64,
    limits: &Limits,
) -> Result<(u64, Vec<Sample>), ExtractError> {
    let mut budget = Budget::new(limits);
    let listed = subsegment_samples(
        source,
        entry.records,
        u64::from(entry.records_len),
        source_len,
        defaults,
        limits,
        &mut budget,
    )?;
    let base_decode_time = listed
        .base_decode_time
        .and_then(|time| u64::try_from(time).ok())
        .ok_or_else(|| invalid("a fragment without a decode time"))?;
    if !inside(&listed, entry.records, u64::from(entry.records_len)) {
        return Err(invalid("a fragment whose samples lie outside it"));
    }
    let mut samples = Vec::new();
    let mut charged = 0u64;
    for run in &listed.runs {
        charged = charged
            .checked_add(u64::from(run.count).checked_mul(32).ok_or_else(overflow)?)
            .ok_or_else(overflow)?;
        if charged > limits.max_table_bytes {
            return Err(ExtractError::Source(Error::LimitExceeded {
                limit: coreav_isobmff::LimitKind::TableBytes,
                offset: entry.records,
            }));
        }
        let cto = run.composition_offset;
        if cto < i64::from(i32::MIN) || cto > i64::from(u32::MAX) {
            return Err(invalid("a composition offset outside 32 bits"));
        }
        for i in 0..run.count {
            samples.push(Sample {
                offset: run.sample_offset(i).ok_or_else(overflow)?,
                size: run.sample_size,
                duration: run.sample_duration,
                cto,
                sync: run.keyframe,
            });
        }
    }
    // A segment starts with a sync sample: the segment index promised one
    // (`starts_with_SAP`), and the fragment is what says so.
    if !samples.first().is_some_and(|sample| sample.sync) {
        return Err(invalid(
            "a subsegment that does not start with a sync sample",
        ));
    }
    Ok((base_decode_time, samples))
}

fn invalid(reason: &'static str) -> ExtractError {
    ExtractError::InvalidIndex { reason }
}
