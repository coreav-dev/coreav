//! The index format: a header with one record per rendition, then per
//! rendition a directory of fixed-size segment entries and the segments'
//! sample records. All integers are big-endian. A request handler reads the
//! header and the directories once ([`HlsIndex::open`]) and then one
//! segment's records per media segment, so it never holds more than the
//! directories and one segment's samples.
//!
//! The index is read as untrusted input: it may come back from object
//! storage changed. Every count, length and offset is checked before it
//! drives an allocation or a read, and every sample range must lie inside
//! the source length the index records.

use super::segment::{MediaSegment, Sample};
use super::{MAX_SOURCE_TAG, Rendition};
use crate::ExtractError;
use crate::moov::overflow;
use coreav_isobmff::{Error, LimitKind, Limits, Rational, Source, TrackDefaults};

/// First bytes of every index.
const MAGIC: [u8; 8] = *b"CAVHLSIX";
/// Format version; a reader refuses any other.
const VERSION: u16 = 3;
/// Bytes of one directory entry.
const ENTRY_LEN: u64 = 40;
/// Bytes of the fixed part of the header (magic, version, header length).
const PREFIX_LEN: usize = 14;
/// Memory charged per decoded sample against `max_table_bytes`.
const SAMPLE_CHARGE: u64 = 32;

/// Record flags.
const SYNC: u8 = 0x01;
const HAS_OFFSET: u8 = 0x02;
const HAS_DURATION: u8 = 0x04;
const HAS_CTO: u8 = 0x08;

/// The edit list of a rendition, the source's in media ticks: an empty
/// edit, then one media edit from `media_time` lasting `duration`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) empty: u64,
    pub(crate) media_time: i64,
    pub(crate) duration: u64,
}

/// One segment of a rendition, as the directory lists it. A deferred
/// rendition ([`IndexTrack::deferred`]) knows its segments' times and
/// bytes but not their samples: its entries name the source's movie
/// fragment instead of records in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentEntry {
    /// Presentation start in media ticks (0 for the first segment).
    pub start: u64,
    /// Decode time of the first sample in media ticks (`tfdt`); 0 for a
    /// deferred segment, whose fragment carries it.
    pub base_decode_time: u64,
    /// Samples, at least 1; 0 for a deferred segment.
    pub samples: u32,
    /// Sample bytes (the `mdat` payload); for a deferred segment the
    /// subsegment's bytes, its `moof` included.
    pub data_bytes: u64,
    /// Offset of the records from the start of the rendition's record
    /// area; for a deferred segment the source offset of its `moof`.
    pub(crate) records: u64,
    /// Bytes of the records; for a deferred segment the subsegment's.
    pub(crate) records_len: u32,
}

/// One rendition of an index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexTrack {
    /// Which rendition.
    pub rendition: Rendition,
    /// Position of the track among all tracks of the source.
    pub stream_index: u32,
    /// `track_ID` of the track in the source.
    pub track_id: u32,
    /// The RFC 6381 codec string (for example `avc1.64001F`).
    pub codecs: String,
    /// Media timescale (that of the source track).
    pub timescale: u32,
    /// ISO 639-2/T language code.
    pub language: [u8; 3],
    /// Creation times of the source's track and media headers.
    pub created: u64,
    /// Media creation time.
    pub media_created: u64,
    /// The source track header's transformation matrix.
    pub matrix: [i32; 9],
    /// The source track header's width and height (16.16 fixed point).
    pub presentation_size: (u32, u32),
    /// Display width: the coded width widened by the sample entry's pixel
    /// aspect ratio, else the track header's (0 for audio).
    pub width: u16,
    /// Display height.
    pub height: u16,
    /// Channels (0 for video).
    pub channels: u16,
    /// Sample rate in Hz (0 for video).
    pub sample_rate: u32,
    /// Samples in all segments.
    pub samples: u64,
    /// Sample bytes in all segments.
    pub data_bytes: u64,
    /// Every segment decodes on its own: no segment starts at a sync sample
    /// with leading samples (an open GOP).
    pub independent: bool,
    /// The track's `trex` defaults when the rendition is deferred: its
    /// segments are the source's subsegments, and a request lists the
    /// fragment each entry names rather than records in the index
    /// (`docs/design/hls.md`, "Sources with a segment index").
    pub deferred: Option<TrackDefaults>,
    pub(crate) edit: Option<Edit>,
    /// Presentation end in media ticks: the end of the last segment.
    pub end: u64,
    /// Peak segment bit rate in bits per second (RFC 8216 4.3.4.2).
    pub peak_bit_rate: u64,
    /// Average segment bit rate in bits per second.
    pub average_bit_rate: u64,
    /// Shortest decode duration of a presented sample but the last, in
    /// media ticks, for the maximum frame rate; 0 when it is under half the
    /// mean (a stray short sample, not a frame rate).
    pub min_sample_duration: u32,
    /// The `stsd` payload after version and flags.
    pub(crate) stsd: Vec<u8>,
    pub(crate) segments: Vec<SegmentEntry>,
    pub(crate) records_offset: u64,
    pub(crate) records_len: u64,
}

impl IndexTrack {
    /// The segments.
    pub fn segments(&self) -> &[SegmentEntry] {
        &self.segments
    }

    /// Presentation end of segment `n` in media ticks.
    pub fn segment_end(&self, n: usize) -> Option<u64> {
        self.segments.get(n)?;
        Some(
            self.segments
                .get(n.checked_add(1)?)
                .map_or(self.end, |next| next.start),
        )
    }

    /// Duration of segment `n` in seconds (its `EXTINF`).
    pub fn segment_duration(&self, n: usize) -> Option<Rational> {
        let start = self.segments.get(n)?.start;
        let end = self.segment_end(n)?;
        Rational::new(
            i128::from(end.checked_sub(start)?),
            u64::from(self.timescale),
        )
    }

    /// Presentation duration in seconds.
    pub fn duration(&self) -> Option<Rational> {
        Rational::new(i128::from(self.end), u64::from(self.timescale))
    }
}

/// An opened index: the header and the segment directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlsIndex {
    pub(crate) source_len: u64,
    pub(crate) source_tag: Vec<u8>,
    pub(crate) segment_duration: Rational,
    pub(crate) tracks: Vec<IndexTrack>,
    pub(crate) index_len: u64,
}

impl HlsIndex {
    /// Reads the header and the segment directories of `index`. The
    /// header is at most `limits.max_box_bytes`, and the directories
    /// count against `limits.max_table_bytes`.
    pub fn open<S: Source>(mut index: S, limits: &Limits) -> Result<Self, ExtractError> {
        let index_len = index.len()?;
        let mut prefix = [0u8; PREFIX_LEN];
        if index_len < PREFIX_LEN as u64 {
            return Err(invalid("shorter than its header"));
        }
        index.read_exact_at(0, &mut prefix)?;
        let mut r = Reader::new(&prefix);
        if r.bytes(MAGIC.len())? != MAGIC {
            return Err(invalid("not a CoreAV HLS index"));
        }
        if r.u16()? != VERSION {
            return Err(invalid("unknown index version"));
        }
        let header_len = u64::from(r.u32()?);
        if header_len > limits.max_box_bytes {
            return Err(limit(LimitKind::BoxBytes));
        }
        if header_len < PREFIX_LEN as u64 || header_len > index_len {
            return Err(invalid("header length outside the index"));
        }
        let mut header = vec![0; usize::try_from(header_len).map_err(|_| overflow())?];
        index.read_exact_at(0, &mut header)?;
        let mut r = Reader::new(header.get(PREFIX_LEN..).unwrap_or_default());
        let source_len = r.u64()?;
        let num = i128::from(r.i64()?);
        let den = r.u64()?;
        let segment_duration = Rational::new(num, den)
            .filter(|d| d.num > 0)
            .ok_or(invalid("segment duration"))?;
        let tag_len = usize::from(r.u8()?);
        let source_tag = r.bytes(tag_len)?.to_vec();
        let count = r.u8()?;
        if !(1..=2).contains(&count) {
            return Err(invalid("track count"));
        }
        let mut tracks: Vec<IndexTrack> = Vec::with_capacity(2);
        let mut directory_bytes = 0u64;
        for _ in 0..count {
            let (mut track, directory, segments) = read_track(&mut r, limits)?;
            if tracks.iter().any(|t| t.rendition == track.rendition) {
                return Err(invalid("a rendition appears twice"));
            }
            let len = u64::from(segments)
                .checked_mul(ENTRY_LEN)
                .ok_or_else(overflow)?;
            directory_bytes = directory_bytes.checked_add(len).ok_or_else(overflow)?;
            if directory_bytes > limits.max_table_bytes {
                return Err(limit(LimitKind::TableBytes));
            }
            inside(directory, len, index_len)?;
            inside(track.records_offset, track.records_len, index_len)?;
            let mut bytes = vec![0; usize::try_from(len).map_err(|_| overflow())?];
            index.read_exact_at(directory, &mut bytes)?;
            track.segments = read_directory(&bytes, &track, source_len)?;
            tracks.push(track);
        }
        if !r.is_empty() {
            return Err(invalid("bytes after the last track header"));
        }
        Ok(Self {
            source_len,
            source_tag,
            segment_duration,
            tracks,
            index_len,
        })
    }

    /// Length of the source the index was built from.
    pub fn source_len(&self) -> u64 {
        self.source_len
    }

    /// The caller's tag from [`HlsOptions::source_tag`](super::HlsOptions).
    pub fn source_tag(&self) -> &[u8] {
        &self.source_tag
    }

    /// The target segment duration the index was built with, in seconds.
    pub fn target_segment_duration(&self) -> Rational {
        self.segment_duration
    }

    /// Bytes of the index.
    pub fn index_len(&self) -> u64 {
        self.index_len
    }

    /// The renditions: the video first, if any.
    pub fn tracks(&self) -> &[IndexTrack] {
        &self.tracks
    }

    /// The rendition `rendition`, if the index has it.
    pub fn track(&self, rendition: Rendition) -> Option<&IndexTrack> {
        self.tracks.iter().find(|t| t.rendition == rendition)
    }

    /// Media segment `number` (counted from 0) of `rendition`: reads its
    /// records from `index`, the bytes this index was opened from. The
    /// decoded samples count against `limits.max_table_bytes`, and each
    /// against `limits.max_packet_bytes`.
    ///
    /// For a deferred rendition ([`IndexTrack::deferred`]), whose index
    /// came from the source's segment index, the samples come from
    /// `source` instead: the one movie fragment the entry names, whose
    /// `moof` is read and listed here. `source` is not touched for a
    /// rendition the index lists itself.
    pub fn media_segment<I: Source, S: Source>(
        &self,
        rendition: Rendition,
        number: u32,
        mut index: I,
        mut source: S,
        limits: &Limits,
    ) -> Result<MediaSegment, ExtractError> {
        let track = self.track(rendition).ok_or(Error::NoSuchTrack)?;
        let entry = usize::try_from(number)
            .ok()
            .and_then(|n| track.segments.get(n))
            .ok_or(Error::NoSuchTrack)?;
        let sequence = number.checked_add(1).ok_or_else(overflow)?;
        if let Some(defaults) = track.deferred {
            if source.len()? != self.source_len {
                return Err(ExtractError::SourceChanged);
            }
            let (base_decode_time, samples) = super::deferred::segment_samples(
                &mut source,
                entry,
                defaults,
                self.source_len,
                limits,
            )?;
            return Ok(MediaSegment::new(
                sequence,
                base_decode_time,
                &samples,
                self.source_len,
            )?);
        }
        if index.len()? != self.index_len {
            return Err(invalid("the index changed since it was opened"));
        }
        let charge = u64::from(entry.samples)
            .checked_mul(SAMPLE_CHARGE)
            .ok_or_else(overflow)?;
        if charge > limits.max_table_bytes || u64::from(entry.records_len) > limits.max_table_bytes
        {
            return Err(limit(LimitKind::TableBytes));
        }
        let at = track
            .records_offset
            .checked_add(entry.records)
            .ok_or_else(overflow)?;
        let mut bytes = vec![0; usize::try_from(entry.records_len).map_err(|_| overflow())?];
        index.read_exact_at(at, &mut bytes)?;
        let samples = decode_records(&bytes, entry, self.source_len, limits)?;
        Ok(MediaSegment::new(
            sequence,
            entry.base_decode_time,
            &samples,
            self.source_len,
        )?)
    }
}

/// A finished rendition and its records, for [`serialize`].
pub(crate) struct Built {
    pub(crate) track: IndexTrack,
    pub(crate) records: Vec<u8>,
}

/// The index bytes. Offsets in the track headers are filled in here.
pub(crate) fn serialize(
    source_len: u64,
    source_tag: &[u8],
    segment_duration: Rational,
    tracks: &mut [Built],
) -> Result<Vec<u8>, Error> {
    let header = |tracks: &[Built]| -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&source_len.to_be_bytes());
        let num = i64::try_from(segment_duration.num).map_err(|_| overflow())?;
        out.extend_from_slice(&num.to_be_bytes());
        out.extend_from_slice(&segment_duration.den.to_be_bytes());
        let tag_len = u8::try_from(source_tag.len())
            .ok()
            .filter(|l| usize::from(*l) <= MAX_SOURCE_TAG)
            .ok_or(Error::Unsupported {
                reason: "a source tag longer than 255 bytes",
            })?;
        out.push(tag_len);
        out.extend_from_slice(source_tag);
        out.push(u8::try_from(tracks.len()).map_err(|_| overflow())?);
        for built in tracks {
            write_track(&mut out, &built.track)?;
        }
        let len = u32::try_from(out.len()).map_err(|_| overflow())?;
        if let Some(field) = out.get_mut(10..14) {
            field.copy_from_slice(&len.to_be_bytes());
        }
        Ok(out)
    };
    // The header has a fixed size whatever the offsets are.
    let mut at = u64::try_from(header(tracks)?.len()).map_err(|_| overflow())?;
    for built in tracks.iter_mut() {
        let segments = u64::try_from(built.track.segments.len()).map_err(|_| overflow())?;
        built.track.records_offset = at
            .checked_add(segments.checked_mul(ENTRY_LEN).ok_or_else(overflow)?)
            .ok_or_else(overflow)?;
        built.track.records_len = u64::try_from(built.records.len()).map_err(|_| overflow())?;
        at = built
            .track
            .records_offset
            .checked_add(built.track.records_len)
            .ok_or_else(overflow)?;
    }
    let mut out = header(tracks)?;
    for built in tracks.iter() {
        for entry in &built.track.segments {
            out.extend_from_slice(&entry.start.to_be_bytes());
            out.extend_from_slice(&entry.base_decode_time.to_be_bytes());
            out.extend_from_slice(&entry.samples.to_be_bytes());
            out.extend_from_slice(&entry.records_len.to_be_bytes());
            out.extend_from_slice(&entry.data_bytes.to_be_bytes());
            out.extend_from_slice(&entry.records.to_be_bytes());
        }
        out.extend_from_slice(&built.records);
    }
    Ok(out)
}

fn write_track(out: &mut Vec<u8>, t: &IndexTrack) -> Result<(), Error> {
    out.push(match t.rendition {
        Rendition::Video => 0,
        Rendition::Audio => 1,
    });
    out.extend_from_slice(&t.stream_index.to_be_bytes());
    out.extend_from_slice(&t.track_id.to_be_bytes());
    out.push(u8::try_from(t.codecs.len()).map_err(|_| overflow())?);
    out.extend_from_slice(t.codecs.as_bytes());
    out.extend_from_slice(&t.timescale.to_be_bytes());
    out.extend_from_slice(&t.language);
    out.extend_from_slice(&t.created.to_be_bytes());
    out.extend_from_slice(&t.media_created.to_be_bytes());
    for value in t.matrix {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&t.presentation_size.0.to_be_bytes());
    out.extend_from_slice(&t.presentation_size.1.to_be_bytes());
    out.extend_from_slice(&t.width.to_be_bytes());
    out.extend_from_slice(&t.height.to_be_bytes());
    out.extend_from_slice(&t.channels.to_be_bytes());
    out.extend_from_slice(&t.sample_rate.to_be_bytes());
    out.extend_from_slice(&t.samples.to_be_bytes());
    out.extend_from_slice(&t.data_bytes.to_be_bytes());
    out.push(u8::from(t.independent));
    let defaults = t.deferred.unwrap_or_default();
    out.push(u8::from(t.deferred.is_some()));
    out.extend_from_slice(&defaults.duration.to_be_bytes());
    out.extend_from_slice(&defaults.size.to_be_bytes());
    out.extend_from_slice(&defaults.flags.to_be_bytes());
    let edit = t.edit.unwrap_or(Edit {
        empty: 0,
        media_time: 0,
        duration: 0,
    });
    out.push(u8::from(t.edit.is_some()));
    out.extend_from_slice(&edit.empty.to_be_bytes());
    out.extend_from_slice(&edit.media_time.to_be_bytes());
    out.extend_from_slice(&edit.duration.to_be_bytes());
    out.extend_from_slice(&t.end.to_be_bytes());
    out.extend_from_slice(&t.peak_bit_rate.to_be_bytes());
    out.extend_from_slice(&t.average_bit_rate.to_be_bytes());
    out.extend_from_slice(&t.min_sample_duration.to_be_bytes());
    out.extend_from_slice(
        &u32::try_from(t.stsd.len())
            .map_err(|_| overflow())?
            .to_be_bytes(),
    );
    out.extend_from_slice(&t.stsd);
    out.extend_from_slice(
        &u32::try_from(t.segments.len())
            .map_err(|_| overflow())?
            .to_be_bytes(),
    );
    let directory_len = u64::try_from(t.segments.len())
        .ok()
        .and_then(|n| n.checked_mul(ENTRY_LEN))
        .ok_or_else(overflow)?;
    // 0 while the header is only being sized.
    let directory = t.records_offset.saturating_sub(directory_len);
    out.extend_from_slice(&directory.to_be_bytes());
    out.extend_from_slice(&t.records_offset.to_be_bytes());
    out.extend_from_slice(&t.records_len.to_be_bytes());
    Ok(())
}

/// A track header; returns the track (without segments), the directory
/// offset and the segment count.
fn read_track(r: &mut Reader<'_>, limits: &Limits) -> Result<(IndexTrack, u64, u32), ExtractError> {
    let rendition = match r.u8()? {
        0 => Rendition::Video,
        1 => Rendition::Audio,
        _ => return Err(invalid("unknown rendition")),
    };
    let stream_index = r.u32()?;
    let track_id = r.u32()?;
    let codecs_len = usize::from(r.u8()?);
    let codecs = r.bytes(codecs_len)?;
    // Printed into playlists inside a quoted attribute: letters, digits and
    // dots only.
    if codecs.is_empty()
        || !codecs
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'.')
    {
        return Err(invalid("codec string"));
    }
    let codecs = String::from_utf8(codecs.to_vec()).map_err(|_| invalid("codec string"))?;
    let timescale = r.u32()?;
    if timescale == 0 {
        return Err(invalid("timescale 0"));
    }
    let mut language = [0u8; 3];
    language.copy_from_slice(r.bytes(3)?);
    if !language.iter().all(u8::is_ascii_lowercase) {
        return Err(invalid("language code"));
    }
    let created = r.u64()?;
    let media_created = r.u64()?;
    let mut matrix = [0i32; 9];
    for value in &mut matrix {
        *value = r.i32()?;
    }
    let presentation_size = (r.u32()?, r.u32()?);
    let width = r.u16()?;
    let height = r.u16()?;
    let channels = r.u16()?;
    let sample_rate = r.u32()?;
    let samples = r.u64()?;
    let data_bytes = r.u64()?;
    let independent = match r.u8()? {
        0 => false,
        1 => true,
        _ => return Err(invalid("independent flag")),
    };
    let deferred = match r.u8()? {
        0 => None,
        1 => Some(TrackDefaults {
            track_id,
            duration: 0,
            size: 0,
            flags: 0,
        }),
        _ => return Err(invalid("deferred flag")),
    };
    let (duration, size, flags) = (r.u32()?, r.u32()?, r.u32()?);
    let deferred = deferred.map(|d| TrackDefaults {
        duration,
        size,
        flags,
        ..d
    });
    let has_edit = r.u8()?;
    let edit = Edit {
        empty: r.u64()?,
        media_time: r.i64()?,
        duration: r.u64()?,
    };
    let edit = match has_edit {
        0 => None,
        1 if edit.media_time >= 0 => Some(edit),
        _ => return Err(invalid("edit")),
    };
    let end = r.u64()?;
    let peak_bit_rate = r.u64()?;
    let average_bit_rate = r.u64()?;
    let min_sample_duration = r.u32()?;
    let stsd_len = u64::from(r.u32()?);
    if stsd_len > limits.max_box_bytes {
        return Err(limit(LimitKind::BoxBytes));
    }
    let stsd = r
        .bytes(usize::try_from(stsd_len).map_err(|_| overflow())?)?
        .to_vec();
    let segments = r.u32()?;
    if segments == 0 {
        return Err(invalid("a rendition without segments"));
    }
    let directory = r.u64()?;
    let records_offset = r.u64()?;
    let records_len = r.u64()?;
    let track = IndexTrack {
        rendition,
        stream_index,
        track_id,
        codecs,
        timescale,
        language,
        created,
        media_created,
        matrix,
        presentation_size,
        width,
        height,
        channels,
        sample_rate,
        samples,
        data_bytes,
        independent,
        deferred,
        edit,
        end,
        peak_bit_rate,
        average_bit_rate,
        min_sample_duration,
        stsd,
        segments: Vec::new(),
        records_offset,
        records_len,
    };
    Ok((track, directory, segments))
}

fn read_directory(
    bytes: &[u8],
    track: &IndexTrack,
    source_len: u64,
) -> Result<Vec<SegmentEntry>, ExtractError> {
    let mut r = Reader::new(bytes);
    let count = bytes.len() / 40;
    let mut entries: Vec<SegmentEntry> = Vec::with_capacity(count);
    let mut samples = 0u64;
    let mut data = 0u64;
    for _ in 0..count {
        let entry = SegmentEntry {
            start: r.u64()?,
            base_decode_time: r.u64()?,
            samples: r.u32()?,
            records_len: r.u32()?,
            data_bytes: r.u64()?,
            records: r.u64()?,
        };
        let starts_right = match entries.last() {
            None => entry.start == 0,
            Some(previous) => entry.start >= previous.start,
        };
        if !starts_right {
            return Err(invalid("segment entry"));
        }
        if track.deferred.is_some() {
            // A deferred entry names a subsegment of the source: its
            // `moof` is read per request and holds its samples and decode
            // time, so the entry carries neither.
            if entry.samples != 0
                || entry.base_decode_time != 0
                || u64::from(entry.records_len) != entry.data_bytes
                || entry.records_len < 8
            {
                return Err(invalid("deferred segment entry"));
            }
            inside(entry.records, entry.data_bytes, source_len)?;
        } else {
            // Samples may share bytes, so a segment's bytes are not
            // bounded by the source length; each sample range is checked
            // when decoded.
            if entry.samples == 0 {
                return Err(invalid("segment entry"));
            }
            // Every record holds at least a flags byte and a size.
            if u64::from(entry.records_len) < u64::from(entry.samples).saturating_mul(2) {
                return Err(invalid("segment records"));
            }
            inside(
                entry.records,
                u64::from(entry.records_len),
                track.records_len,
            )?;
        }
        samples = samples
            .checked_add(u64::from(entry.samples))
            .ok_or_else(overflow)?;
        data = data.checked_add(entry.data_bytes).ok_or_else(overflow)?;
        entries.push(entry);
    }
    let last = entries.last().map_or(0, |e| e.start);
    if samples != track.samples || data != track.data_bytes || track.end < last {
        return Err(invalid("segment directory"));
    }
    Ok(entries)
}

/// Appends the records of one segment.
pub(crate) fn encode_records(samples: &[Sample], out: &mut Vec<u8>) {
    let mut previous: Option<&Sample> = None;
    for sample in samples {
        let mut flags = if sample.sync { SYNC } else { 0 };
        let offset =
            previous.is_none_or(|p| p.offset.checked_add(u64::from(p.size)) != Some(sample.offset));
        let duration = previous.is_none_or(|p| p.duration != sample.duration);
        let cto = previous.is_none_or(|p| p.cto != sample.cto);
        if offset {
            flags |= HAS_OFFSET;
        }
        if duration {
            flags |= HAS_DURATION;
        }
        if cto {
            flags |= HAS_CTO;
        }
        out.push(flags);
        put_varint(out, u64::from(sample.size));
        if offset {
            put_varint(out, sample.offset);
        }
        if duration {
            put_varint(out, u64::from(sample.duration));
        }
        if cto {
            let zigzag = (sample.cto << 1) ^ (sample.cto >> 63);
            put_varint(out, zigzag as u64);
        }
        previous = Some(sample);
    }
}

/// The samples of one segment, checked against its entry and the source.
fn decode_records(
    bytes: &[u8],
    entry: &SegmentEntry,
    source_len: u64,
    limits: &Limits,
) -> Result<Vec<Sample>, ExtractError> {
    let mut r = Reader::new(bytes);
    let count = usize::try_from(entry.samples).map_err(|_| overflow())?;
    let mut samples: Vec<Sample> = Vec::with_capacity(count);
    let mut data = 0u64;
    for _ in 0..count {
        let flags = r.u8()?;
        let explicit = HAS_OFFSET | HAS_DURATION | HAS_CTO;
        if flags & !(SYNC | explicit) != 0 {
            return Err(invalid("sample record flags"));
        }
        let previous = samples.last().copied();
        if previous.is_none() && flags & explicit != explicit {
            return Err(invalid("first sample record"));
        }
        let size = u32::try_from(r.varint()?).map_err(|_| invalid("sample size"))?;
        if u64::from(size) > limits.max_packet_bytes {
            return Err(limit(LimitKind::PacketBytes));
        }
        let offset = match previous {
            Some(p) if flags & HAS_OFFSET == 0 => p
                .offset
                .checked_add(u64::from(p.size))
                .ok_or_else(overflow)?,
            _ => r.varint()?,
        };
        let duration = match previous {
            Some(p) if flags & HAS_DURATION == 0 => p.duration,
            _ => u32::try_from(r.varint()?).map_err(|_| invalid("sample duration"))?,
        };
        let cto = match previous {
            Some(p) if flags & HAS_CTO == 0 => p.cto,
            _ => {
                let zigzag = r.varint()?;
                let value = ((zigzag >> 1) as i64) ^ -((zigzag & 1) as i64);
                if !(i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&value) {
                    return Err(invalid("composition offset"));
                }
                value
            }
        };
        if offset
            .checked_add(u64::from(size))
            .is_none_or(|end| end > source_len)
        {
            return Err(invalid("a sample outside the source"));
        }
        data = data.checked_add(u64::from(size)).ok_or_else(overflow)?;
        samples.push(Sample {
            offset,
            size,
            duration,
            cto,
            sync: flags & SYNC != 0,
        });
    }
    if !r.is_empty() || data != entry.data_bytes {
        return Err(invalid("segment records"));
    }
    if samples.first().is_some_and(|s| !s.sync) {
        return Err(invalid("a segment that does not start with a sync sample"));
    }
    Ok(samples)
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}

fn inside(offset: u64, len: u64, total: u64) -> Result<(), ExtractError> {
    match offset.checked_add(len) {
        Some(end) if end <= total => Ok(()),
        _ => Err(invalid("a range outside the index")),
    }
}

fn invalid(reason: &'static str) -> ExtractError {
    ExtractError::InvalidIndex { reason }
}

fn limit(limit: LimitKind) -> ExtractError {
    Error::LimitExceeded { limit, offset: 0 }.into()
}

/// A bounds-checked big-endian reader over a byte slice.
struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ExtractError> {
        let (head, tail) = self.bytes.split_at_checked(n).ok_or(invalid("truncated"))?;
        self.bytes = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ExtractError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, ExtractError> {
        Ok(u8::from_be_bytes(self.array()?))
    }

    fn u16(&mut self) -> Result<u16, ExtractError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, ExtractError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, ExtractError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ExtractError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, ExtractError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    /// An unsigned LEB128 value of at most 10 bytes.
    fn varint(&mut self) -> Result<u64, ExtractError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.u8()?;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 {
                return Err(invalid("varint"));
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(invalid("varint"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip() {
        let samples = [
            Sample {
                offset: 1_000,
                size: 300,
                duration: 3_000,
                cto: 6_000,
                sync: true,
            },
            Sample {
                offset: 1_300,
                size: 20,
                duration: 3_000,
                cto: -3_000,
                sync: false,
            },
            Sample {
                offset: 9_000,
                size: 0,
                duration: 2_999,
                cto: -3_000,
                sync: false,
            },
            Sample {
                offset: u64::MAX - 7,
                size: 7,
                duration: u32::MAX,
                cto: i64::from(u32::MAX),
                sync: false,
            },
        ];
        let mut bytes = Vec::new();
        encode_records(&samples, &mut bytes);
        let entry = SegmentEntry {
            start: 0,
            base_decode_time: 0,
            samples: 4,
            data_bytes: 327,
            records: 0,
            records_len: bytes.len() as u32,
        };
        let decoded = decode_records(&bytes, &entry, u64::MAX, &Limits::default()).unwrap();
        assert_eq!(decoded, samples);
        // A sample past the source's end.
        assert!(decode_records(&bytes, &entry, u64::MAX - 1, &Limits::default()).is_err());
        // Trailing bytes and a wrong byte count.
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_records(&longer, &entry, u64::MAX, &Limits::default()).is_err());
        let wrong = SegmentEntry {
            data_bytes: 326,
            ..entry
        };
        assert!(decode_records(&bytes, &wrong, u64::MAX, &Limits::default()).is_err());
    }

    /// Found by the `hls` fuzz target: samples may share bytes, so a
    /// segment can hold more sample bytes than the source has.
    #[test]
    fn segments_may_share_source_bytes() {
        let samples = [Sample {
            offset: 10,
            size: 80,
            duration: 1,
            cto: 0,
            sync: true,
        }; 3];
        let mut records = Vec::new();
        encode_records(&samples, &mut records);
        let track = IndexTrack {
            rendition: Rendition::Audio,
            stream_index: 0,
            track_id: 1,
            codecs: "mp4a.40.2".to_string(),
            timescale: 1,
            language: *b"und",
            created: 0,
            media_created: 0,
            matrix: [0; 9],
            presentation_size: (0, 0),
            width: 0,
            height: 0,
            channels: 2,
            sample_rate: 48_000,
            samples: 3,
            data_bytes: 240,
            independent: true,
            deferred: None,
            edit: None,
            end: 3,
            peak_bit_rate: 0,
            average_bit_rate: 0,
            min_sample_duration: 1,
            stsd: vec![0, 0, 0, 0],
            segments: vec![SegmentEntry {
                start: 0,
                base_decode_time: 0,
                samples: 3,
                data_bytes: 240,
                records: 0,
                records_len: records.len() as u32,
            }],
            records_offset: 0,
            records_len: 0,
        };
        let mut built = [Built { track, records }];
        let bytes = serialize(100, b"", Rational::from_int(6), &mut built).unwrap();
        let index =
            HlsIndex::open(coreav_isobmff::SliceSource::new(&bytes), &Limits::default()).unwrap();
        let segment = index
            .media_segment(
                Rendition::Audio,
                0,
                coreav_isobmff::SliceSource::new(&bytes),
                coreav_isobmff::SliceSource::new(&[0u8; 100]),
                &Limits::default(),
            )
            .unwrap();
        assert_eq!(segment.ranges().len(), 3);
    }

    #[test]
    fn varints_are_bounded() {
        let mut r = Reader::new(&[0xff; 11]);
        assert!(r.varint().is_err());
        let mut out = Vec::new();
        put_varint(&mut out, u64::MAX);
        assert_eq!(out.len(), 10);
        assert_eq!(Reader::new(&out).varint().unwrap(), u64::MAX);
        let mut over = out.clone();
        over[9] = 0x02;
        assert!(Reader::new(&over).varint().is_err());
    }
}
