//! Probing: the EBML header (RFC 8794 8.1), the Segment and its metadata
//! elements (RFC 9559 5.1), found by a linear walk up to the first Cluster
//! and through the `SeekHead`.

use crate::bitstream;
use crate::ebml::{self, ElementHeader};
use crate::error::{LimitKind, Warning, warn};
use crate::ids;
use crate::meta::{Edition, Retained, Tag, parse_edition, parse_tag};
use crate::packets::Packet;
use crate::reader::{Children, Counter, read_data, read_header};
use crate::track::{Allowlist, Codec, DroppedTrack, Encoding, Track, parse_track_entry};
use crate::{Error, Limits, Source};
use coreav_isobmff::{FourCc, Profile, Rational};

/// The `DocType` of the EBML header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocType {
    /// `matroska`.
    Matroska,
    /// `webm`, the web profile.
    Webm,
}

/// An attached file (RFC 9559 5.1.6), described without its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// `FileName`.
    pub name: String,
    /// `FileMediaType`.
    pub media_type: String,
    /// Size of `FileData` in bytes.
    pub size: u64,
    /// `FileUID`, which tags name the attachment by. Read only by
    /// [`probe_with_metadata`].
    pub uid: Option<u64>,
    /// `FileDescription`, when stored. Read only by
    /// [`probe_with_metadata`].
    pub description: Option<String>,
    /// Absolute offset of `FileData`'s data, so a caller can read the
    /// file's first bytes.
    pub data_offset: u64,
}

/// The probe result: one Matroska Segment.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    /// `DocType`.
    pub doc_type: DocType,
    /// `DocTypeVersion`.
    pub doc_type_version: u64,
    /// `DocTypeReadVersion`.
    pub doc_type_read_version: u64,
    /// Absolute offset of the Segment element.
    pub offset: u64,
    /// Absolute offset of the Segment's data: Segment Positions count from
    /// here (RFC 9559 16).
    pub data_offset: u64,
    /// The Segment's data size; `None` when unknown (a live recording).
    pub size: Option<u64>,
    /// `TimestampScale`: nanoseconds per Segment Tick.
    pub timestamp_scale: u64,
    /// `Duration` in Segment Ticks, as stored (a float).
    pub duration: Option<f64>,
    /// `DateUTC`: nanoseconds since 2001-01-01T00:00:00 UTC.
    pub date_utc: Option<i64>,
    /// `Title`.
    pub title: Option<String>,
    /// `MuxingApp`.
    pub muxing_app: Option<String>,
    /// `WritingApp`.
    pub writing_app: Option<String>,
    /// Tracks with an allowed codec, in `Tracks` order.
    pub tracks: Vec<Track>,
    /// Tracks left out because their codec is not allowed.
    pub dropped: Vec<DroppedTrack>,
    /// Absolute offset of the first Cluster, when one was found.
    pub first_cluster: Option<u64>,
    /// Absolute offset of the `Cues` element, when one was found.
    pub cues: Option<u64>,
    /// The segment has a `Chapters` element.
    pub has_chapters: bool,
    /// The segment has a `Tags` element.
    pub has_tags: bool,
    /// Attached files.
    pub attachments: Vec<Attachment>,
    /// The `Tag` elements of every `Tags` element read, in stored order. A
    /// `Tag` larger than `max_element_bytes` is left out. Read only by
    /// [`probe_with_metadata`]; empty otherwise.
    pub tags: Vec<Tag>,
    /// The `EditionEntry` elements of the `Chapters` element read, in
    /// stored order. An edition larger than `max_element_bytes` is left
    /// out. Read only by [`probe_with_metadata`]; empty otherwise.
    pub editions: Vec<Edition>,
    /// Recoverable problems in the segment.
    pub warnings: Vec<Warning>,
    /// Where the segment's elements end: the declared end, or the end of the
    /// file when the size is unknown or runs past it.
    pub(crate) end: u64,
    /// `EBMLMaxSizeLength`.
    pub(crate) max_size_len: u8,
}

impl Segment {
    /// `Duration` in nanoseconds, rounded to the nearest nanosecond (RFC
    /// 9559 11.3); `None` when absent, or not finite and positive.
    pub fn duration_ns(&self) -> Option<i128> {
        let duration = self.duration?;
        if !duration.is_finite() || duration <= 0.0 {
            return None;
        }
        mul_round(duration, self.timestamp_scale)
    }

    /// `Duration` in seconds, exactly as [`Segment::duration_ns`] gives it.
    pub fn duration_seconds(&self) -> Option<Rational> {
        Rational::new(self.duration_ns()?, 1_000_000_000)
    }

    /// Segment Ticks per second: 10^9 / `TimestampScale`.
    pub fn ticks_per_second(&self) -> Rational {
        Rational::new(1_000_000_000, self.timestamp_scale).unwrap_or(Rational::from_int(1_000))
    }

    /// The track with this `TrackNumber`, when it was kept.
    pub fn track_by_number(&self, number: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.number == number)
    }
}

/// `value` × `factor` rounded to the nearest integer, halves away from zero,
/// computed exactly from the binary representation of `value`.
fn mul_round(value: f64, factor: u64) -> Option<i128> {
    let bits = value.to_bits();
    let negative = bits >> 63 == 1;
    let exponent = i32::try_from((bits >> 52) & 0x7FF).ok()?;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, shift) = if exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), exponent - 1075)
    };
    let product = i128::from(mantissa).checked_mul(i128::from(factor))?;
    let magnitude = if shift >= 0 {
        product.checked_mul(1i128.checked_shl(u32::try_from(shift).ok()?)?)?
    } else {
        let down = u32::try_from(-shift).ok()?;
        if down >= 127 {
            0
        } else {
            let half = 1i128 << (down - 1).min(126);
            let rounded = product.checked_add(if down == 0 { 0 } else { half })?;
            rounded >> down
        }
    };
    Some(if negative { -magnitude } else { magnitude })
}

/// Probes a Matroska or WebM file: the EBML header, the Segment, `Info`,
/// `Tracks`, and the positions of the first Cluster and of `Cues`.
///
/// Reads the elements before the first Cluster, then follows the
/// `SeekHead` for metadata stored after the clusters; it never reads
/// clusters. Tracks whose codec is not allowed are dropped, or reject the
/// file in strict mode. `Tags` and `Chapters` are only noted
/// ([`Segment::has_tags`], [`Segment::has_chapters`]); see
/// [`probe_with_metadata`] for a probe that reads them.
pub fn probe<S: Source>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
) -> Result<Segment, Error> {
    probe_segment(source, limits, allowlist, false)
}

/// [`probe`], and also reads the metadata a caller describing the whole
/// file needs (`coreav ffprobe`): every `Tag` ([`Segment::tags`]), the
/// editions of `Chapters` ([`Segment::editions`]), and each attached
/// file's `FileUID` and `FileDescription`. Opt-in because it reads
/// elements, often stored after the clusters, that the probe, remux and
/// check paths do not use.
pub fn probe_with_metadata<S: Source>(
    source: S,
    limits: &Limits,
    allowlist: &Allowlist,
) -> Result<Segment, Error> {
    probe_segment(source, limits, allowlist, true)
}

fn probe_segment<S: Source>(
    mut source: S,
    limits: &Limits,
    allowlist: &Allowlist,
    metadata: bool,
) -> Result<Segment, Error> {
    let len = source.len()?;
    let mut magic = [0u8; 4];
    if len < 4 {
        return Err(Error::NotMatroska);
    }
    source.read_exact_at(0, &mut magic)?;
    if u32::from_be_bytes(magic) != ids::EBML {
        return Err(Error::NotMatroska);
    }
    let counter = Counter::new(limits.max_elements);
    counter.count(0)?;
    let header = read_header(&mut source, 0, len, 8)?;
    let data = read_data(
        &mut source,
        &header,
        limits.max_element_bytes,
        LimitKind::ElementBytes,
    )?;
    let ebml = parse_ebml_header(&data, &header, &counter)?;

    // The Segment follows, possibly after Void elements (RFC 8794 11.3.2).
    let mut at = header.end().ok_or(Error::Overflow { offset: 0 })?;
    let segment = loop {
        if at >= len {
            return Err(Error::Truncated { offset: at });
        }
        counter.count(at)?;
        let h = read_header(&mut source, at, len, ebml.max_size_len)?;
        match h.id {
            ids::SEGMENT => break h,
            ids::VOID | ids::CRC32 => {
                at = h.end().ok_or(Error::InvalidElement {
                    offset: at,
                    id: h.id,
                    reason: "unknown size not allowed for this element",
                })?;
            }
            _ => {
                return Err(Error::InvalidElement {
                    offset: at,
                    id: h.id,
                    reason: "expected a Segment after the EBML header",
                });
            }
        }
    };
    let mut warnings = Vec::new();
    let declared_end = segment.end();
    let end = match declared_end {
        Some(end) if end <= len => end,
        Some(_) => {
            warn(&mut warnings, Warning::TruncatedSegment);
            len
        }
        None => len,
    };
    let mut walk = Walk {
        source: &mut source,
        limits,
        counter,
        max_size_len: ebml.max_size_len,
        data_offset: segment.data_offset(),
        end,
        found: Found::default(),
        retained: 0,
        warnings,
        metadata,
    };
    walk.linear()?;
    walk.follow_seek_heads()?;
    let mut segment = walk.finish(segment, ebml, allowlist)?;
    read_frame_headers(&mut segment, &mut source, limits)?;
    Ok(segment)
}

/// Element visits allowed for finding the first keyframe of each track
/// whose header lacks what the frame header holds.
const FRAME_HEADER_WORK: u64 = 4_096;

/// Bytes of a frame read for its headers.
const FRAME_HEADER_BYTES: u64 = 1_024;

/// What the first keyframe of a track is read for, if anything: VP8 and
/// VP9 keep profile, bit depth and color in the frame header (the VP9
/// codec private data is optional and has no color range); some writers
/// leave the H.264 sequence parameter set or the AV1 sequence header only
/// in the first keyframe (no `CodecPrivate`, or an `av1C` without
/// configuration OBUs).
fn frame_header_wanted(track: &Track) -> bool {
    if !track.packets_available() || track.encoding.opaque() {
        return false;
    }
    match track.codec {
        Codec::Vp8 | Codec::Vp9 => true,
        Codec::H264 => track.codec_private.is_none(),
        Codec::Av1 => track.codec_private.as_ref().is_some_and(|p| p.len() == 4),
        _ => false,
    }
}

/// Reads the first keyframe of each track that [`frame_header_wanted`]
/// selects. Best effort: the frame must be among the first few thousand
/// elements of the clusters, and damaged or missing clusters leave the
/// values unknown, because the probe result does not depend on the
/// clusters. A source that fails to read fails the probe: an I/O error is
/// not the end of the data.
fn read_frame_headers<S: Source>(
    segment: &mut Segment,
    source: &mut S,
    limits: &Limits,
) -> Result<(), Error> {
    let mut pending = segment
        .tracks
        .iter()
        .filter(|t| frame_header_wanted(t))
        .count();
    if pending == 0 {
        return Ok(());
    }
    let peek = Limits {
        max_block_work: limits.max_block_work.min(FRAME_HEADER_WORK),
        ..*limits
    };
    // Per track: its first frame, and its first frame flagged a keyframe.
    let mut first: Vec<(Option<Packet>, Option<Packet>)> = vec![(None, None); segment.tracks.len()];
    for packet in segment.packets(&mut *source, &peek) {
        let packet = match packet {
            Ok(packet) => packet,
            Err(error @ Error::Io(_)) => return Err(error),
            Err(_) => break,
        };
        let Some((any, key)) = first.get_mut(packet.track) else {
            continue;
        };
        if key.is_some()
            || !segment
                .tracks
                .get(packet.track)
                .is_some_and(frame_header_wanted)
        {
            continue;
        }
        any.get_or_insert(packet);
        if packet.keyframe {
            *key = Some(packet);
            pending -= 1;
            if pending == 0 {
                break;
            }
        }
    }
    // A frame cut off by the end of the file is `None`.
    let read = |source: &mut S, track: &Track, packet: Packet| -> Result<Option<Vec<u8>>, Error> {
        let mut header = match &track.encoding {
            Encoding::HeaderStripping(prefix) => prefix.clone(),
            _ => Vec::new(),
        };
        let stored = packet.stored_size.min(FRAME_HEADER_BYTES);
        let Ok(stored) = usize::try_from(stored) else {
            return Ok(None);
        };
        let mut bytes = vec![0u8; stored];
        match source
            .read_exact_at(packet.offset, &mut bytes)
            .map_err(Error::from)
        {
            Ok(()) => {}
            Err(error @ Error::Io(_)) => return Err(error),
            Err(_) => return Ok(None),
        }
        header.extend_from_slice(&bytes);
        Ok(Some(header))
    };
    for (track, (any, key)) in segment.tracks.iter_mut().zip(first) {
        // The keyframe first; a stream's first frame is a key frame for
        // these codecs even where a writer left the flag unset.
        let fallback = any.filter(|a| key.is_none_or(|k| k.offset != a.offset));
        for packet in [key, fallback].into_iter().flatten() {
            if read(source, track, packet)?.is_some_and(|frame| apply_frame(track, &frame)) {
                break;
            }
        }
    }
    Ok(())
}

/// Applies what one frame header says; whether it said anything.
fn apply_frame(track: &mut Track, frame: &[u8]) -> bool {
    match track.codec {
        Codec::Vp8 | Codec::Vp9 => apply_vpx(track, frame),
        Codec::H264 => bitstream::h264_record(frame)
            .is_some_and(|record| apply_record(track, FourCc(*b"avcC"), &record)),
        Codec::Av1 => {
            let stored = track.codec_private.clone().unwrap_or_default();
            bitstream::av1_record(&stored, frame)
                .is_some_and(|record| apply_record(track, FourCc(*b"av1C"), &record))
        }
        _ => false,
    }
}

fn apply_vpx(track: &mut Track, frame: &[u8]) -> bool {
    let info = match track.codec {
        Codec::Vp9 => bitstream::vp9(frame),
        _ => bitstream::vp8(frame),
    };
    let Some(info) = info else { return false };
    track.frame_profile = Some(info.profile);
    track.chroma_subsampling = track.chroma_subsampling.or(info.subsampling);
    if track.codec == Codec::Vp9 && track.profile.is_none() {
        track.profile = Some(Profile::Vp9 {
            profile: info.profile,
            level: 0,
            bit_depth: info.bit_depth.unwrap_or(0),
        });
    }
    if track.color.matrix.is_none() {
        track.color.matrix = info.matrix;
    }
    if track.color.full_range.is_none() {
        track.color.full_range = info.full_range;
    }
    true
}

/// Applies a record built from the first keyframe: its parameter set gives
/// the profile (it is what a decoder uses) and fills the colors `Colour`
/// left unknown.
fn apply_record(track: &mut Track, kind: FourCc, record: &[u8]) -> bool {
    let Some(described) = coreav_isobmff::describe_config_record(kind, record) else {
        return false;
    };
    if !described.parameter_set {
        return false;
    }
    if described.profile.is_some() {
        track.profile = described.profile;
    }
    track.fill_from_record(&described);
    true
}

/// A parser of one metadata element held in memory: its data, the data's
/// offset, `EBMLMaxSizeLength`, the element counter, and the bytes of
/// strings it keeps.
type ParseFn<T> = fn(&[u8], u64, u8, &Counter, &mut Retained) -> Result<T, Error>;

struct EbmlHeader {
    doc_type: DocType,
    version: u64,
    read_version: u64,
    max_size_len: u8,
}

fn parse_ebml_header(
    data: &[u8],
    header: &ElementHeader,
    counter: &Counter,
) -> Result<EbmlHeader, Error> {
    let mut doc_type = None;
    let (mut version, mut read_version, mut max_size_len) = (1u64, 1u64, 8u64);
    for child in Children::new(data, header.data_offset(), 8, counter) {
        let (h, d) = child?;
        match h.id {
            ids::EBML_READ_VERSION if ebml::uint(d, &h)? > 1 => {
                return Err(Error::Unsupported {
                    reason: "EBMLReadVersion above 1",
                });
            }
            ids::EBML_MAX_SIZE_LENGTH => max_size_len = ebml::uint(d, &h)?,
            ids::EBML_MAX_ID_LENGTH if ebml::uint(d, &h)? < 4 => {
                return Err(Error::InvalidElement {
                    offset: h.offset,
                    id: h.id,
                    reason: "EBMLMaxIDLength below 4",
                });
            }
            ids::DOC_TYPE => doc_type = Some(ebml::string(d)),
            ids::DOC_TYPE_VERSION => version = ebml::uint(d, &h)?,
            ids::DOC_TYPE_READ_VERSION => read_version = ebml::uint(d, &h)?,
            ids::EBML_VERSION | ids::EBML_READ_VERSION | ids::EBML_MAX_ID_LENGTH => {}
            _ => {}
        }
    }
    let doc_type = match doc_type.as_deref() {
        Some("matroska") => DocType::Matroska,
        Some("webm") => DocType::Webm,
        _ => return Err(Error::NotMatroska),
    };
    // RFC 9559 7: versions 1 to 4 are defined.
    if read_version > 4 {
        return Err(Error::Unsupported {
            reason: "DocTypeReadVersion above 4",
        });
    }
    // RFC 9559 4.3: EBMLMaxSizeLength is 1 to 8.
    let max_size_len = u8::try_from(max_size_len)
        .ok()
        .filter(|n| (1..=8).contains(n))
        .ok_or(Error::InvalidElement {
            offset: header.offset,
            id: header.id,
            reason: "EBMLMaxSizeLength outside 1 to 8",
        })?;
    Ok(EbmlHeader {
        doc_type,
        version,
        read_version,
        max_size_len,
    })
}

#[derive(Default)]
struct Info {
    timestamp_scale: u64,
    duration: Option<f64>,
    date_utc: Option<i64>,
    title: Option<String>,
    muxing_app: Option<String>,
    writing_app: Option<String>,
}

#[derive(Default)]
struct Found {
    seek: Vec<(u32, u64)>,
    seek_heads: Vec<u64>,
    info: Option<Info>,
    tracks: Option<(u64, Vec<Track>)>,
    first_cluster: Option<u64>,
    cues: Option<u64>,
    chapters: bool,
    tags: bool,
    attachments: Option<Vec<Attachment>>,
    tag_list: Vec<Tag>,
    editions: Vec<Edition>,
}

struct Walk<'s, S> {
    source: &'s mut S,
    limits: &'s Limits,
    counter: Counter,
    max_size_len: u8,
    data_offset: u64,
    end: u64,
    found: Found,
    retained: u64,
    warnings: Vec<Warning>,
    /// Read `Tags`, `Chapters` and the attachments' UIDs and descriptions
    /// ([`probe_with_metadata`]).
    metadata: bool,
}

impl<S: Source> Walk<'_, S> {
    /// Reads the top-level elements in order up to the first Cluster.
    fn linear(&mut self) -> Result<(), Error> {
        let mut at = self.data_offset;
        while at < self.end {
            self.counter.count(at)?;
            let h = match read_header(self.source, at, self.end, self.max_size_len) {
                Ok(h) => h,
                Err(Error::Truncated { .. }) if self.found.tracks.is_some() => {
                    // A header cut off at the end of a truncated file.
                    warn(&mut self.warnings, Warning::TruncatedSegment);
                    break;
                }
                Err(error) => return Err(error),
            };
            if h.id == ids::EBML {
                // A second EBML document; this crate reads the first.
                break;
            }
            if h.id == ids::CLUSTER {
                self.found.first_cluster = Some(at);
                break;
            }
            let element_end = h.end().ok_or(Error::InvalidElement {
                offset: at,
                id: h.id,
                reason: "unknown size not allowed for this element",
            })?;
            if element_end > self.end {
                if self.found.tracks.is_some() && !matches!(h.id, ids::INFO | ids::TRACKS) {
                    // Metadata cut off at the end of a truncated file.
                    warn(&mut self.warnings, Warning::TruncatedSegment);
                    break;
                }
                return Err(Error::Truncated { offset: at });
            }
            self.top_level(&h)?;
            at = element_end;
        }
        Ok(())
    }

    /// Reads one top-level element other than a Cluster.
    fn top_level(&mut self, h: &ElementHeader) -> Result<(), Error> {
        match h.id {
            ids::SEEK_HEAD if !self.found.seek_heads.contains(&h.offset) => {
                self.found.seek_heads.push(h.offset);
                self.seek_head(h)?;
            }
            ids::INFO if self.found.info.is_none() => self.found.info = Some(self.info(h)?),
            ids::TRACKS if self.found.tracks.is_none() => {
                self.found.tracks = Some((h.offset, self.tracks(h)?));
            }
            ids::CUES => {
                self.found.cues.get_or_insert(h.offset);
            }
            ids::CHAPTERS if !self.found.chapters => {
                self.found.chapters = true;
                if self.metadata {
                    self.chapters(h)?;
                }
            }
            ids::TAGS => {
                self.found.tags = true;
                if self.metadata {
                    self.tags(h)?;
                }
            }
            ids::ATTACHMENTS if self.found.attachments.is_none() => {
                self.found.attachments = Some(self.attachments(h)?);
            }
            _ => {}
        }
        Ok(())
    }

    /// Reads what the `SeekHead` entries point to and the linear walk did not
    /// reach (RFC 9559 6.3): metadata stored after the clusters, and a
    /// second `SeekHead`.
    fn follow_seek_heads(&mut self) -> Result<(), Error> {
        let mut next = 0;
        while let Some(&(id, position)) = self.found.seek.get(next) {
            next += 1;
            let wanted = match id {
                ids::SEEK_HEAD => !self.found.seek_heads.contains(&position),
                ids::INFO => self.found.info.is_none(),
                ids::TRACKS => self.found.tracks.is_none(),
                ids::CUES => self.found.cues.is_none(),
                ids::CHAPTERS => !self.found.chapters,
                ids::TAGS => !self.found.tags,
                ids::ATTACHMENTS => self.found.attachments.is_none(),
                ids::CLUSTER => self.found.first_cluster.is_none(),
                _ => false,
            };
            if !wanted {
                continue;
            }
            self.counter.count(position)?;
            let h = match read_header(self.source, position, self.end, self.max_size_len) {
                Ok(h) if h.id == id => h,
                Ok(_) | Err(Error::Truncated { .. } | Error::InvalidElement { .. }) => {
                    warn(&mut self.warnings, Warning::InvalidSeekEntry);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if id == ids::CLUSTER {
                self.found.first_cluster = Some(position);
                continue;
            }
            match h.end() {
                Some(end) if end <= self.end => self.top_level(&h)?,
                _ if matches!(id, ids::INFO | ids::TRACKS) => {
                    return Err(Error::Truncated { offset: position });
                }
                _ => warn(&mut self.warnings, Warning::InvalidSeekEntry),
            }
        }
        Ok(())
    }

    fn seek_head(&mut self, h: &ElementHeader) -> Result<(), Error> {
        let data = read_data(
            self.source,
            h,
            self.limits.max_element_bytes,
            LimitKind::ElementBytes,
        )?;
        let mut entries = Vec::new();
        let mut invalid = false;
        for seek in Children::new(&data, h.data_offset(), self.max_size_len, &self.counter) {
            let (seek, d) = seek?;
            if seek.id != ids::SEEK {
                continue;
            }
            let (mut id, mut position) = (None, None);
            for child in Children::new(d, seek.data_offset(), self.max_size_len, &self.counter) {
                let (c, d) = child?;
                match c.id {
                    ids::SEEK_ID if d.len() <= 4 => {
                        id = Some(d.iter().fold(0u32, |v, &b| (v << 8) | u32::from(b)));
                    }
                    ids::SEEK_POSITION => position = Some(ebml::uint(d, &c)?),
                    _ => {}
                }
            }
            let target = position.and_then(|p| self.data_offset.checked_add(p));
            match (id, target) {
                (Some(id), Some(target)) if target < self.end => entries.push((id, target)),
                _ => invalid = true,
            }
        }
        if invalid {
            warn(&mut self.warnings, Warning::InvalidSeekEntry);
        }
        self.found.seek.extend(entries);
        Ok(())
    }

    fn info(&mut self, h: &ElementHeader) -> Result<Info, Error> {
        let data = read_data(
            self.source,
            h,
            self.limits.max_element_bytes,
            LimitKind::ElementBytes,
        )?;
        let mut info = Info {
            timestamp_scale: 1_000_000,
            ..Info::default()
        };
        for child in Children::new(&data, h.data_offset(), self.max_size_len, &self.counter) {
            let (c, d) = child?;
            match c.id {
                ids::TIMESTAMP_SCALE => {
                    // An empty element takes the default (RFC 8794 6.1).
                    if !d.is_empty() {
                        info.timestamp_scale = ebml::uint(d, &c)?;
                    }
                    if info.timestamp_scale == 0 {
                        return Err(Error::InvalidElement {
                            offset: c.offset,
                            id: c.id,
                            reason: "TimestampScale is 0",
                        });
                    }
                }
                ids::DURATION => {
                    info.duration = Some(ebml::float(d, &c)?).filter(|v| v.is_finite() && *v > 0.0);
                }
                ids::DATE_UTC if d.len() == 8 => info.date_utc = Some(ebml::int(d, &c)?),
                ids::TITLE => info.title = Some(ebml::string(d)),
                ids::MUXING_APP => info.muxing_app = Some(ebml::string(d)),
                ids::WRITING_APP => info.writing_app = Some(ebml::string(d)),
                _ => {}
            }
        }
        let strings = [&info.title, &info.muxing_app, &info.writing_app]
            .iter()
            .map(|s| s.as_ref().map_or(0, String::len))
            .sum();
        self.retain(strings, h.offset)?;
        Ok(info)
    }

    fn retain(&mut self, bytes: usize, offset: u64) -> Result<(), Error> {
        self.retained = self.retained.saturating_add(bytes as u64);
        if self.retained > self.limits.max_retained_bytes {
            return Err(Error::LimitExceeded {
                limit: LimitKind::RetainedBytes,
                offset,
            });
        }
        Ok(())
    }

    fn retain_string(&mut self, data: &[u8], h: &ElementHeader) -> Result<String, Error> {
        self.retain(data.len(), h.offset)?;
        Ok(ebml::string(data))
    }

    /// Visits the children of a known-sized master element through the
    /// source, without reading the master into memory.
    fn children_headers(
        &mut self,
        parent: &ElementHeader,
        mut visit: impl FnMut(&mut Self, ElementHeader) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let end = parent.end().ok_or(Error::Overflow {
            offset: parent.offset,
        })?;
        let mut at = parent.data_offset();
        while at < end {
            self.counter.count(at)?;
            let h = read_header(self.source, at, end, self.max_size_len)?;
            let child_end = h.end().ok_or(Error::InvalidElement {
                offset: at,
                id: h.id,
                reason: "unknown size not allowed for this element",
            })?;
            if child_end > end {
                return Err(Error::InvalidElement {
                    offset: at,
                    id: h.id,
                    reason: "element runs past its parent",
                });
            }
            visit(self, h)?;
            at = child_end;
        }
        Ok(())
    }

    fn tracks(&mut self, h: &ElementHeader) -> Result<Vec<Track>, Error> {
        let mut tracks = Vec::new();
        self.children_headers(h, |walk, entry| {
            if entry.id != ids::TRACK_ENTRY {
                return Ok(());
            }
            if tracks.len() >= walk.limits.max_tracks as usize {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::Tracks,
                    offset: entry.offset,
                });
            }
            let data = read_data(
                walk.source,
                &entry,
                walk.limits.max_element_bytes,
                LimitKind::ElementBytes,
            )?;
            let track = parse_track_entry(
                &data,
                entry.data_offset(),
                &entry,
                walk.max_size_len,
                &walk.counter,
            )?;
            let stripped = match &track.encoding {
                Encoding::HeaderStripping(prefix) => prefix.len(),
                _ => 0,
            };
            let kept = track.codec_private.as_ref().map_or(0, Vec::len)
                + stripped
                + track.codec_id.len()
                + track.language.len()
                + track.language_iso639.as_ref().map_or(0, String::len)
                + track.name.as_ref().map_or(0, String::len);
            walk.retain(kept, entry.offset)?;
            if tracks.iter().any(|t: &Track| t.number == track.number) {
                return Err(Error::InvalidElement {
                    offset: entry.offset,
                    id: entry.id,
                    reason: "duplicate TrackNumber",
                });
            }
            tracks.push(track);
            Ok(())
        })?;
        Ok(tracks)
    }

    /// Reads the children of `parent` whose ID is `id` into memory one at
    /// a time and calls `parse` on each; a child larger than
    /// `max_element_bytes` is skipped.
    fn small_children<T>(
        &mut self,
        parent: &ElementHeader,
        id: u32,
        parse: ParseFn<T>,
    ) -> Result<Vec<T>, Error> {
        let mut out = Vec::new();
        self.children_headers(parent, |walk, child| {
            if child.id != id || child.size.is_none_or(|s| s > walk.limits.max_element_bytes) {
                return Ok(());
            }
            let data = read_data(
                walk.source,
                &child,
                walk.limits.max_element_bytes,
                LimitKind::ElementBytes,
            )?;
            let mut retained = Retained(0);
            let value = parse(
                &data,
                child.data_offset(),
                walk.max_size_len,
                &walk.counter,
                &mut retained,
            )?;
            walk.retain(
                usize::try_from(retained.0).unwrap_or(usize::MAX),
                child.offset,
            )?;
            out.push(value);
            Ok(())
        })?;
        Ok(out)
    }

    fn tags(&mut self, h: &ElementHeader) -> Result<(), Error> {
        let tags = self.small_children(h, ids::TAG, parse_tag)?;
        self.found.tag_list.extend(tags);
        Ok(())
    }

    fn chapters(&mut self, h: &ElementHeader) -> Result<(), Error> {
        self.found.editions = self.small_children(h, ids::EDITION_ENTRY, parse_edition)?;
        Ok(())
    }

    fn attachments(&mut self, h: &ElementHeader) -> Result<Vec<Attachment>, Error> {
        let mut files = Vec::new();
        self.children_headers(h, |walk, file| {
            if file.id != ids::ATTACHED_FILE {
                return Ok(());
            }
            let mut attachment = Attachment {
                name: String::new(),
                media_type: String::new(),
                size: 0,
                uid: None,
                description: None,
                data_offset: 0,
            };
            walk.children_headers(&file, |walk, field| {
                match field.id {
                    ids::FILE_NAME | ids::FILE_MEDIA_TYPE => {
                        let data = read_data(
                            walk.source,
                            &field,
                            walk.limits.max_element_bytes,
                            LimitKind::ElementBytes,
                        )?;
                        let value = walk.retain_string(&data, &field)?;
                        if field.id == ids::FILE_NAME {
                            attachment.name = value;
                        } else {
                            attachment.media_type = value;
                        }
                    }
                    ids::FILE_DATA => {
                        attachment.size = field.size.unwrap_or(0);
                        attachment.data_offset = field.data_offset();
                    }
                    ids::FILE_DESCRIPTION if walk.metadata => {
                        let data = read_data(
                            walk.source,
                            &field,
                            walk.limits.max_element_bytes,
                            LimitKind::ElementBytes,
                        )?;
                        attachment.description = Some(walk.retain_string(&data, &field)?);
                    }
                    ids::FILE_UID if walk.metadata => {
                        let data = read_data(walk.source, &field, 8, LimitKind::ElementBytes)?;
                        attachment.uid = Some(ebml::uint(&data, &field)?);
                    }
                    _ => {}
                }
                Ok(())
            })?;
            files.push(attachment);
            Ok(())
        })?;
        Ok(files)
    }

    fn finish(
        self,
        segment: ElementHeader,
        ebml: EbmlHeader,
        allowlist: &Allowlist,
    ) -> Result<Segment, Error> {
        let found = self.found;
        let missing = |reason| Error::InvalidElement {
            offset: segment.offset,
            id: segment.id,
            reason,
        };
        let info = found.info.ok_or(missing("no Info element"))?;
        let (_, all_tracks) = found.tracks.ok_or(missing("no Tracks element"))?;
        let mut tracks = Vec::new();
        let mut dropped = Vec::new();
        for (stream_index, mut track) in all_tracks.into_iter().enumerate() {
            track.stream_index = stream_index;
            if allowlist.allows(track.codec) {
                track.index = tracks.len();
                tracks.push(track);
            } else if allowlist.is_strict() {
                return Err(Error::CodecNotAllowed {
                    track_number: track.number,
                    codec: track.codec,
                });
            } else {
                dropped.push(DroppedTrack {
                    stream_index,
                    number: track.number,
                    codec_id: track.codec_id,
                    codec: track.codec,
                    kind: track.kind,
                });
            }
        }
        Ok(Segment {
            doc_type: ebml.doc_type,
            doc_type_version: ebml.version,
            doc_type_read_version: ebml.read_version,
            offset: segment.offset,
            data_offset: segment.data_offset(),
            size: segment.size,
            timestamp_scale: info.timestamp_scale,
            duration: info.duration,
            date_utc: info.date_utc,
            title: info.title,
            muxing_app: info.muxing_app,
            writing_app: info.writing_app,
            tracks,
            dropped,
            first_cluster: found.first_cluster,
            cues: found.cues,
            has_chapters: found.chapters,
            has_tags: found.tags,
            attachments: found.attachments.unwrap_or_default(),
            tags: found.tag_list,
            editions: found.editions,
            warnings: self.warnings,
            end: self.end,
            max_size_len: self.max_size_len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_float_products() {
        assert_eq!(mul_round(2008.0, 1_000_000), Some(2_008_000_000));
        assert_eq!(mul_round(0.5, 3), Some(2));
        assert_eq!(mul_round(-0.5, 3), Some(-2));
        assert_eq!(mul_round(1.0 / 3.0, 3), Some(1));
        assert_eq!(mul_round(5e-324, 1), Some(0));
        assert_eq!(mul_round(1e300, 1_000_000), None);
        assert_eq!(
            mul_round(33.333_333_333_333_336, 1_000_000),
            Some(33_333_333)
        );
    }
}
