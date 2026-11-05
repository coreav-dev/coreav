//! Bounded, completely prepared H.264 copy into MPEG transport streams.

mod avc;
mod tstd;
mod wire;

#[cfg(test)]
mod tests;

use crate::ExtractError;
use coreav_isobmff::{
    Allowlist, AvcCopyFacts, Budget, Codec, EditList, FourCc, IDENTITY_MATRIX, LimitKind,
    PlanningBudget, SliceSource, Source, TrackKind,
};
use std::io::Write;

/// Resource ceilings for one immutable ISO-to-TS preparation.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum owned input snapshot. Default 32 MiB.
    pub max_input_bytes: usize,
    /// Maximum complete transport output. Default 32 MiB.
    pub max_output_bytes: usize,
    /// Maximum monotonic backing-capacity charges. Default 128 MiB.
    pub max_planning_bytes: usize,
    /// Maximum retained ISO sample tables. Default 8 MiB.
    pub max_table_bytes: u64,
    /// Maximum retained ISO metadata. Default 1 MiB.
    pub max_metadata_bytes: u64,
    /// Maximum source or converted access unit. Default 1 MiB.
    pub max_access_unit_bytes: usize,
    /// Maximum copied samples. Default 65,536.
    pub max_samples: usize,
    /// Maximum framed NAL units visited in one sample. Default 64.
    pub max_nals_per_sample: usize,
    /// Maximum framed NAL units visited overall. Default 262,144.
    pub max_nals: u64,
    /// Combined bounded parsing, listing, conversion and transport work.
    /// Default 1,000,000 visits, conservatively divided across phases.
    pub max_work: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 32 << 20,
            max_output_bytes: 32 << 20,
            max_planning_bytes: 128 << 20,
            max_table_bytes: 8 << 20,
            max_metadata_bytes: 1 << 20,
            max_access_unit_bytes: 1 << 20,
            max_samples: 65_536,
            max_nals_per_sample: 64,
            max_nals: 262_144,
            max_work: 1_000_000,
        }
    }
}

/// Complete validated output, independent of the source after preparation.
pub struct Prepared {
    bytes: Vec<u8>,
}

impl Prepared {
    /// Writes already validated bytes without reading or admitting more media.
    ///
    /// # Errors
    /// Returns an output I/O error, possibly after writing part of the sink.
    pub fn write_to<W: Write>(&self, mut out: W) -> std::io::Result<()> {
        out.write_all(&self.bytes)
    }
}

struct Au {
    pts90: u64,
    dts90: u64,
    idr: bool,
    data: Vec<u8>,
}

struct Work {
    used: u64,
    max: u64,
}

impl Work {
    fn charge(&mut self, units: u64) -> Result<(), ExtractError> {
        self.used = self
            .used
            .checked_add(units)
            .filter(|used| *used <= self.max)
            .ok_or_else(|| limit(LimitKind::SampleWork))?;
        Ok(())
    }
}

/// Captures, admits, converts and verifies one ISO H.264 stream before output.
/// No output sink is used until this function returns successfully.
///
/// # Errors
/// Returns source, resource or unsupported-profile errors without output.
pub fn prepare<S: Source>(
    mut source: S,
    stream_index: usize,
    limits: &Limits,
) -> Result<Prepared, ExtractError> {
    let planning = PlanningBudget::new(limits.max_planning_bytes);
    let len = usize::try_from(source.len()?).map_err(|_| limit(LimitKind::ReadBytes))?;
    if len > limits.max_input_bytes {
        return Err(limit(LimitKind::ReadBytes));
    }
    let mut snapshot = Vec::new();
    reserve(&planning, &mut snapshot, len)?;
    snapshot.resize(len, 0);
    for (index, chunk) in snapshot.chunks_mut(coreav_isobmff::MAX_READ).enumerate() {
        let offset = index
            .checked_mul(coreav_isobmff::MAX_READ)
            .and_then(|offset| u64::try_from(offset).ok())
            .ok_or_else(|| limit(LimitKind::ReadBytes))?;
        source.read_exact_at(offset, chunk)?;
    }
    if source.len()? != len as u64 {
        return Err(ExtractError::SourceChanged);
    }
    let quarter = limits.max_work / 4;
    let iso_limits = coreav_isobmff::Limits {
        max_tracks: 1,
        max_fragments: 0,
        max_movie_bytes: 0,
        max_retained_bytes: limits.max_metadata_bytes,
        max_table_bytes: limits.max_table_bytes,
        max_packet_bytes: limits.max_access_unit_bytes as u64,
        max_sample_work: quarter,
        max_total_boxes: u32::try_from(quarter).unwrap_or(u32::MAX),
        max_top_level_boxes: u32::try_from(quarter).unwrap_or(u32::MAX),
        ..coreav_isobmff::Limits::default()
    };
    let movie = coreav_isobmff::probe_with_planning(
        SliceSource::new(&snapshot),
        &iso_limits,
        &Allowlist::default(),
        planning.clone(),
    )?;
    if movie.compressed
        || movie.fragmented
        || movie.tracks.len() != 1
        || !movie.dropped.is_empty()
        || !movie.chapter_references.is_empty()
        || movie.matrix != IDENTITY_MATRIX
    {
        return Err(unsupported(
            "MPEG-TS copy requires one unfragmented ISO video",
        ));
    }
    let track = movie
        .tracks
        .first()
        .ok_or_else(|| unsupported("missing video"))?;
    let description = &track.description;
    if stream_index != track.stream_index
        || track.kind != TrackKind::Video
        || !track.tables_valid
        || track.external_data
        || track.descriptions != 1
        || track.edit_list != EditList::None
        || !track.edits.is_empty()
        || track.matrix != IDENTITY_MATRIX
        || track.sample_count < 3
        || track.sample_count as usize > limits.max_samples
        || track
            .sample_bytes
            .is_none_or(|bytes| bytes > limits.max_output_bytes as u64)
        || description.fourcc != FourCc(*b"avc1")
        || description.codec != Codec::H264
        || !description.visual_children_complete
        || !description.other_boxes.is_empty()
        || description.dolby_vision.is_some()
        || description.ambient.is_some()
        || description
            .entry_boxes
            .iter()
            .any(|entry| !matches!(&entry.kind.0, b"pasp" | b"btrt"))
        || track.language.is_some_and(|language| language != *b"und")
        || track
            .handler_name
            .as_deref()
            .is_some_and(|name| name != "VideoHandler")
        || !track.tag_boxes().is_empty()
        || track.creation_time != 0
        || track.modification_time != 0
        || track.media_creation_time != 0
        || track.media_modification_time != 0
    {
        return Err(unsupported(
            "unqualified ISO video properties for MPEG-TS copy",
        ));
    }
    let mut work = Work {
        used: 0,
        max: quarter,
    };
    for header in movie.tag_boxes() {
        work.charge(1)?;
        let payload = span(&snapshot, header.payload_offset(), header.payload_len())?;
        match &header.kind.0 {
            b"udta" => children(payload, &mut work, |kind, data, work| {
                if kind != FourCc(*b"meta") {
                    return Err(unsupported("unqualified movie user data"));
                }
                encoder_metadata(data, work)
            })?,
            b"meta" => encoder_metadata(payload, &mut work)?,
            _ => return Err(unsupported("unqualified movie metadata")),
        }
    }
    let config = description
        .codec_config
        .as_ref()
        .filter(|config| config.kind == FourCc(*b"avcC"))
        .ok_or_else(|| unsupported("missing AVC configuration"))?;
    let config = avc::Configuration::read(&config.data, &planning)?;
    let bounds = tstd::Bounds::from_sps(config.facts)?;
    let mut budget = Budget::new(&iso_limits);
    let mut packets = movie.packets(
        track.index,
        SliceSource::new(&snapshot),
        &iso_limits,
        &mut budget,
    )?;
    let mut units = Vec::new();
    let mut conversion = avc::Conversion {
        limits,
        planning: &planning,
        work: &mut work,
        nals: 0,
    };
    let mut duration = None;
    let mut decode_end = 0;
    for packet in &mut packets {
        let packet = packet?;
        if units.len() >= limits.max_samples || packet.dts != decode_end {
            return Err(unsupported("noncontiguous MPEG-TS copy timeline"));
        }
        let ticks = exact90(i64::from(packet.duration), track.timescale)?;
        if ticks == 0
            || duration.is_some_and(|previous| previous != ticks)
            || u128::from(ticks) * u128::from(config.facts.time_scale)
                != 180_000u128 * u128::from(config.facts.num_units_in_tick)
        {
            return Err(unsupported(
                "MPEG-TS copy requires qualified constant cadence",
            ));
        }
        duration = Some(ticks);
        decode_end = decode_end
            .checked_add(i64::from(packet.duration))
            .ok_or_else(|| unsupported("MPEG-TS copy timeline overflow"))?;
        let pts90 = exact90(packet.cts, track.timescale)?
            .checked_add(126_000)
            .filter(|time| *time < 1 << 33)
            .ok_or_else(|| unsupported("MPEG-TS copy timestamp range"))?;
        let dts90 = exact90(packet.dts, track.timescale)?
            .checked_add(126_000)
            .filter(|time| *time < 1 << 33)
            .ok_or_else(|| unsupported("MPEG-TS copy timestamp range"))?;
        let data = span(&snapshot, packet.offset, u64::from(packet.size))?;
        let data = config.convert(data, packet.keyframe, units.is_empty(), &mut conversion)?;
        planning
            .reserve(&mut units, 1)
            .map_err(|_| limit(LimitKind::PlanningBytes))?;
        units.push(Au {
            pts90,
            dts90,
            idr: packet.keyframe,
            data,
        });
    }
    if units.len() != track.sample_count as usize || units.len() < 3 {
        return Err(unsupported("incomplete MPEG-TS copy sample listing"));
    }
    presentation(
        &units,
        duration.unwrap_or(0),
        config.facts,
        &planning,
        &mut work,
    )?;
    let bytes = wire::serialize(&units, bounds, limits, &planning, &mut work)?;
    tstd::verify(&bytes, &units, bounds, &planning, &mut work)?;
    Ok(Prepared { bytes })
}

fn presentation(
    units: &[Au],
    duration: u64,
    facts: AvcCopyFacts,
    planning: &PlanningBudget,
    work: &mut Work,
) -> Result<(), ExtractError> {
    let first = units
        .first()
        .ok_or_else(|| unsupported("empty MPEG-TS copy"))?
        .pts90;
    let reorder = facts
        .max_num_reorder_frames
        .ok_or_else(|| unsupported("missing AVC reordering bound"))? as usize;
    let dpb = facts
        .max_dec_frame_buffering
        .ok_or_else(|| unsupported("missing AVC picture-buffer bound"))? as usize;
    let mut seen = Vec::new();
    reserve(planning, &mut seen, units.len())?;
    seen.resize(units.len(), false);
    let mut retired = 0;
    for (index, unit) in units.iter().enumerate() {
        work.charge(1)?;
        let relative = unit
            .pts90
            .checked_sub(first)
            .filter(|time| duration > 0 && time % duration == 0)
            .ok_or_else(|| unsupported("unqualified AVC presentation slots"))?;
        let slot = usize::try_from(relative / duration)
            .ok()
            .filter(|slot| *slot < units.len())
            .ok_or_else(|| unsupported("AVC presentation outside complete copy window"))?;
        if slot.saturating_sub(index) > reorder {
            return Err(unsupported("AVC presentation exceeds its reordering bound"));
        }
        let present = seen
            .get_mut(slot)
            .ok_or_else(|| unsupported("AVC presentation slot"))?;
        if *present {
            return Err(unsupported("duplicate AVC presentation slot"));
        }
        *present = true;
        if index + 1 - retired > dpb {
            return Err(unsupported(
                "AVC presentation exceeds its picture-buffer bound",
            ));
        }
        while seen.get(retired) == Some(&true)
            && first
                .checked_add(
                    (retired as u64)
                        .checked_mul(duration)
                        .ok_or_else(|| unsupported("AVC presentation overflow"))?,
                )
                .is_some_and(|pts| pts <= unit.dts90)
        {
            retired += 1;
        }
    }
    Ok(())
}

fn encoder_metadata(data: &[u8], work: &mut Work) -> Result<(), ExtractError> {
    if data.get(..4) != Some(&[0, 0, 0, 0]) {
        return Err(unsupported("unqualified ISO metadata layout"));
    }
    let mut handler = false;
    let mut list = false;
    children(
        data.get(4..).unwrap_or_default(),
        work,
        |kind, data, work| {
            match &kind.0 {
                b"hdlr" if !handler && data.get(8..12) == Some(b"mdir") => handler = true,
                b"ilst" if !list => {
                    list = true;
                    children(data, work, |kind, data, work| {
                        if kind != FourCc([0xa9, b't', b'o', b'o']) {
                            return Err(unsupported("unqualified ISO metadata item"));
                        }
                        children(data, work, |kind, data, _| {
                            if kind != FourCc(*b"data")
                                || data.get(..8) != Some(&[0, 0, 0, 1, 0, 0, 0, 0])
                                || std::str::from_utf8(data.get(8..).unwrap_or_default()).is_err()
                            {
                                return Err(unsupported("unqualified ISO encoder metadata"));
                            }
                            Ok(())
                        })?;
                        Ok(())
                    })?;
                }
                _ => return Err(unsupported("unqualified ISO metadata child")),
            }
            Ok(())
        },
    )?;
    if !handler || !list {
        return Err(unsupported("incomplete ISO metadata layout"));
    }
    Ok(())
}

fn children(
    mut bytes: &[u8],
    work: &mut Work,
    mut visit: impl FnMut(FourCc, &[u8], &mut Work) -> Result<(), ExtractError>,
) -> Result<(), ExtractError> {
    while !bytes.is_empty() {
        work.charge(1)?;
        let header = match coreav_isobmff::parse_box_header(bytes, 0, bytes.len() as u64)? {
            coreav_isobmff::Parsed::Done(header) => header,
            coreav_isobmff::Parsed::NeedMore(_) => {
                return Err(unsupported("short ISO metadata box"));
            }
        };
        let size = usize::try_from(header.size).map_err(|_| limit(LimitKind::BoxBytes))?;
        let payload = bytes
            .get(usize::from(header.header_len)..size)
            .ok_or_else(|| unsupported("invalid ISO metadata span"))?;
        visit(header.kind, payload, work)?;
        bytes = bytes
            .get(size..)
            .ok_or_else(|| unsupported("invalid ISO metadata end"))?;
    }
    Ok(())
}

fn exact90(value: i64, timescale: u32) -> Result<u64, ExtractError> {
    let value = u64::try_from(value)
        .ok()
        .and_then(|value| value.checked_mul(90_000))
        .filter(|value| timescale > 0 && value % u64::from(timescale) == 0)
        .ok_or_else(|| unsupported("MPEG-TS copy requires exact nonnegative clocks"))?;
    Ok(value / u64::from(timescale))
}

fn span(bytes: &[u8], offset: u64, len: u64) -> Result<&[u8], ExtractError> {
    let start = usize::try_from(offset).map_err(|_| unsupported("ISO byte offset overflow"))?;
    let end = offset
        .checked_add(len)
        .and_then(|end| usize::try_from(end).ok())
        .ok_or_else(|| unsupported("ISO byte extent overflow"))?;
    bytes
        .get(start..end)
        .ok_or_else(|| unsupported("ISO byte extent outside snapshot"))
}

fn reserve<T>(
    planning: &PlanningBudget,
    bytes: &mut Vec<T>,
    count: usize,
) -> Result<(), ExtractError> {
    planning
        .reserve_exact(bytes, count)
        .map_err(|_| limit(LimitKind::PlanningBytes))
}

fn unsupported(reason: &'static str) -> ExtractError {
    coreav_isobmff::Error::Unsupported { reason }.into()
}

fn limit(kind: LimitKind) -> ExtractError {
    coreav_isobmff::Error::LimitExceeded {
        limit: kind,
        offset: 0,
    }
    .into()
}
