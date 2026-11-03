use super::codec::{self, invalid};
use super::ebml::{element, header, id_bytes, sint, uint, write};
use super::{Media, Options, Packet, Track, WriteLimits};
use crate::{DocType, Error, LimitKind, ids};
use std::io::{self, Write};
use std::ops::Range;

const CLUSTER_NS: i128 = 5_000_000_000;
const CLUSTER_BYTES: u64 = 5 << 20;
const PACKET_CHARGE: u64 = 512;

struct Cluster {
    packets: Range<usize>,
    timestamp: u64,
    bytes: u64,
    position: u64,
}

struct Block {
    timestamp: i64,
}

/// A validated, immutable output layout. It borrows only packet metadata;
/// media bytes are supplied to [`Self::write`] one packet at a time.
pub struct Plan<'a> {
    tracks: &'a [Track],
    packets: &'a [Packet],
    options: Options,
    blocks: Vec<Block>,
    clusters: Vec<Cluster>,
    head: Vec<u8>,
    bytes: u64,
}

/// Checks all metadata and computes exact offsets before writing anything.
/// `packets` must already be interleaved in storage order. In particular,
/// H.264 packets must remain in decode order, not sorted by presentation time.
pub fn plan<'a>(
    tracks: &'a [Track],
    packets: &'a [Packet],
    options: Options,
    limits: &WriteLimits,
) -> Result<Plan<'a>, Error> {
    if options.timestamp_scale_ns == 0 || tracks.is_empty() || packets.is_empty() {
        return Err(invalid(
            "a writer needs a positive TimestampScale, tracks and packets",
        ));
    }
    if tracks.len() as u64 > u64::from(limits.max_tracks) {
        return Err(limit(LimitKind::Tracks));
    }
    if packets.len() as u64 > limits.max_block_work {
        return Err(limit(LimitKind::BlockWork));
    }
    let layout_bytes = (packets.len() as u64)
        .checked_mul(PACKET_CHARGE)
        .ok_or(overflow())?;
    if layout_bytes > limits.max_index_bytes {
        return Err(limit(LimitKind::IndexBytes));
    }
    let mut retained = 0u64;
    for track in tracks {
        retained = add(retained, track.private.len() as u64)?;
        retained = add(retained, track.language.len() as u64)?;
        retained = add(retained, track.name.as_ref().map_or(0, |s| s.len() as u64))?;
        retained = add(retained, 256)?;
        if retained > limits.max_retained_bytes {
            return Err(limit(LimitKind::RetainedBytes));
        }
        if track.private.len() as u64 > limits.max_element_bytes {
            return Err(limit(LimitKind::ElementBytes));
        }
        codec::validate(track, options.doc_type)?;
        if !track.language.is_ascii()
            || !track.language.bytes().all(|b| b.is_ascii_lowercase())
            || track.language.len() != 3
            || track.name.as_ref().is_some_and(|n| n.contains('\0'))
        {
            return Err(invalid(
                "track language must be three lowercase letters and names cannot contain NUL",
            ));
        }
    }
    let mut blocks = Vec::with_capacity(packets.len());
    let mut clusters: Vec<Cluster> = Vec::new();
    let mut seen = vec![false; tracks.len()];
    let mut last_duration = vec![None; tracks.len()];
    let mut end_ns = 0i128;
    let mut last_webm = None;
    for (index, packet) in packets.iter().enumerate() {
        let track = tracks
            .get(packet.track)
            .ok_or_else(|| invalid("packet track is out of range"))?;
        if let Some(seen) = seen.get_mut(packet.track) {
            *seen = true;
        }
        if packet.size > limits.max_packet_bytes {
            return Err(limit(LimitKind::PacketBytes));
        }
        if packet.duration_ns == Some(0) {
            return Err(invalid("packet duration must be positive"));
        }
        let duration = packet.duration_ns.or(track.default_duration_ns);
        if let Some(last) = last_duration.get_mut(packet.track) {
            if last.is_none_or(|(pts, _)| packet.pts_ns >= pts) {
                *last = Some((packet.pts_ns, duration));
            }
        }
        let padding = i128::from(packet.discard_padding_ns.unwrap_or(0));
        if padding != 0 && duration.is_none_or(|d| padding.abs() > i128::from(d)) {
            return Err(invalid(
                "DiscardPadding needs a duration at least as large as the trim",
            ));
        }
        let raw_ns = i128::from(packet.pts_ns) + i128::from(track.codec_delay_ns);
        let timestamp = rounded(raw_ns, options.timestamp_scale_ns)?;
        if options.doc_type == DocType::Webm {
            if last_webm.is_some_and(|last| timestamp < last) {
                return Err(invalid(
                    "WebM block timestamps must be monotonically increasing",
                ));
            }
            last_webm = Some(timestamp);
        }
        let prefix = block_head(packet, track, 0, options.timestamp_scale_ns)?;
        let bytes = add(prefix.len() as u64, packet.size)?;
        let new_cluster = clusters.last().is_none_or(|c| {
            let relative = i128::from(timestamp) - i128::from(c.timestamp);
            !(i128::from(i16::MIN)..=i128::from(i16::MAX)).contains(&relative)
                || relative * i128::from(options.timestamp_scale_ns) >= CLUSTER_NS
                || c.bytes.saturating_add(bytes) > CLUSTER_BYTES
                || (packet.keyframe && matches!(track.media, Media::Video { .. }))
        });
        if new_cluster {
            let base = u64::try_from(timestamp.max(0)).map_err(|_| overflow())?;
            if i16::try_from(i128::from(timestamp) - i128::from(base)).is_err() {
                return Err(invalid(
                    "a negative timestamp does not fit a block timestamp",
                ));
            }
            clusters.push(Cluster {
                packets: index..index,
                timestamp: base,
                bytes: uint(ids::TIMESTAMP, base)?.len() as u64,
                position: 0,
            });
        }
        let cluster = clusters.last_mut().ok_or_else(overflow)?;
        cluster.packets.end = index + 1;
        cluster.bytes = add(cluster.bytes, bytes)?;
        blocks.push(Block { timestamp });
        let end = i128::from(packet.pts_ns) + i128::from(duration.unwrap_or(0)) - padding.max(0);
        end_ns = end_ns.max(end);
    }
    if seen.contains(&false) {
        return Err(invalid("every output track must contain a packet"));
    }
    let duration = last_duration
        .iter()
        .all(|last| matches!(last, Some((_, Some(_)))))
        .then_some(end_ns);
    let info = info(options.timestamp_scale_ns, duration)?;
    let tracks_bytes = track_entries(tracks, limits.max_element_bytes)?;
    if info.len() as u64 > limits.max_element_bytes
        || tracks_bytes.len() as u64 > limits.max_retained_bytes
    {
        return Err(limit(LimitKind::RetainedBytes));
    }
    let mut result = Plan {
        tracks,
        packets,
        options,
        blocks,
        clusters,
        head: Vec::new(),
        bytes: 0,
    };
    let cues_len = result.cues()?.len() as u64;
    let has_cues = cues_len != 0;
    let seek_len = seek_head(0, 0, has_cues.then_some(0))?.len() as u64;
    if seek_len > limits.max_element_bytes {
        return Err(limit(LimitKind::ElementBytes));
    }
    let info_pos = seek_len;
    let tracks_pos = add(info_pos, info.len() as u64)?;
    let cues_pos = add(tracks_pos, tracks_bytes.len() as u64)?;
    if cues_len > limits.max_index_bytes {
        return Err(limit(LimitKind::IndexBytes));
    }
    let mut position = add(cues_pos, cues_len)?;
    for cluster in &mut result.clusters {
        cluster.position = position;
        position = add(position, header(ids::CLUSTER, cluster.bytes)?.len() as u64)?;
        position = add(position, cluster.bytes)?;
    }
    let mut ebml = Vec::new();
    for (id, value) in [
        (ids::EBML_VERSION, 1),
        (ids::EBML_READ_VERSION, 1),
        (ids::EBML_MAX_ID_LENGTH, 4),
        (ids::EBML_MAX_SIZE_LENGTH, 8),
    ] {
        ebml.extend(uint(id, value)?);
    }
    ebml.extend(element(
        ids::DOC_TYPE,
        match options.doc_type {
            DocType::Matroska => b"matroska",
            DocType::Webm => b"webm",
        },
    )?);
    ebml.extend(uint(ids::DOC_TYPE_VERSION, 4)?);
    ebml.extend(uint(ids::DOC_TYPE_READ_VERSION, 2)?);
    if ebml.len() as u64 > limits.max_element_bytes {
        return Err(limit(LimitKind::ElementBytes));
    }
    result.head = element(ids::EBML, &ebml)?;
    result.head.extend(header(ids::SEGMENT, position)?);
    result.bytes = add(result.head.len() as u64, position)?;
    result.head.extend(seek_head(
        info_pos,
        tracks_pos,
        has_cues.then_some(cues_pos),
    )?);
    result.head.extend(info);
    result.head.extend(tracks_bytes);
    result.head.extend(result.cues()?);
    Ok(result)
}

impl Plan<'_> {
    /// Exact final file size.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Writes the file without seeking. The callback receives the original
    /// packet index and a writer restricted to that packet's declared size.
    /// Too few or too many payload bytes fail the operation. Output can be
    /// partial after an I/O or callback error and must then be discarded.
    pub fn write<W: Write>(
        &self,
        mut out: W,
        mut copy_payload: impl FnMut(usize, &mut dyn Write) -> Result<(), Error>,
    ) -> Result<u64, Error> {
        write(&mut out, &self.head)?;
        for cluster in &self.clusters {
            write(&mut out, &header(ids::CLUSTER, cluster.bytes)?)?;
            write(&mut out, &uint(ids::TIMESTAMP, cluster.timestamp)?)?;
            for index in cluster.packets.clone() {
                let packet = self.packets.get(index).ok_or_else(overflow)?;
                let block = self.blocks.get(index).ok_or_else(overflow)?;
                let track = self.tracks.get(packet.track).ok_or_else(overflow)?;
                let relative =
                    i16::try_from(i128::from(block.timestamp) - i128::from(cluster.timestamp))
                        .map_err(|_| overflow())?;
                write(
                    &mut out,
                    &block_head(packet, track, relative, self.options.timestamp_scale_ns)?,
                )?;
                let mut payload = Payload {
                    out: &mut out,
                    remaining: packet.size,
                };
                copy_payload(index, &mut payload)?;
                if payload.remaining != 0 {
                    return Err(invalid("the payload callback wrote too few bytes"));
                }
            }
        }
        Ok(self.bytes)
    }

    fn cues(&self) -> Result<Vec<u8>, Error> {
        let video = self
            .tracks
            .iter()
            .any(|t| matches!(t.media, Media::Video { .. }));
        let mut entries = Vec::new();
        for cluster in &self.clusters {
            for index in cluster.packets.clone() {
                let packet = self.packets.get(index).ok_or_else(overflow)?;
                let block = self.blocks.get(index).ok_or_else(overflow)?;
                let track = self.tracks.get(packet.track).ok_or_else(overflow)?;
                let wanted = if video {
                    matches!(track.media, Media::Video { .. })
                } else {
                    index == cluster.packets.start
                };
                if wanted && packet.keyframe && !packet.invisible && block.timestamp >= 0 {
                    entries.push((block.timestamp, packet.track as u64 + 1, cluster.position));
                }
            }
        }
        entries.sort_by_key(|(time, _, _)| *time);
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut data = Vec::new();
        for (time, track, position) in entries {
            let mut positions = uint(ids::CUE_TRACK, track)?;
            positions.extend(element(ids::CUE_CLUSTER_POSITION, &position.to_be_bytes())?);
            let mut point = uint(ids::CUE_TIME, time as u64)?;
            point.extend(element(ids::CUE_TRACK_POSITIONS, &positions)?);
            data.extend(element(ids::CUE_POINT, &point)?);
        }
        element(ids::CUES, &data)
    }
}

fn track_entries(tracks: &[Track], max_element_bytes: u64) -> Result<Vec<u8>, Error> {
    let mut entries = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        let number = index as u64 + 1;
        let mut data = uint(ids::TRACK_NUMBER, number)?;
        data.extend(uint(ids::TRACK_UID, number)?);
        data.extend(uint(
            ids::TRACK_TYPE,
            if matches!(track.media, Media::Video { .. }) {
                1
            } else {
                2
            },
        )?);
        data.extend(uint(ids::FLAG_LACING, 0)?);
        data.extend(uint(ids::FLAG_DEFAULT, u64::from(track.default))?);
        data.extend(element(ids::CODEC_ID, track.codec.as_str().as_bytes())?);
        data.extend(element(ids::LANGUAGE, track.language.as_bytes())?);
        if let Some(name) = &track.name {
            data.extend(element(ids::NAME, name.as_bytes())?);
        }
        if !track.private.is_empty() {
            data.extend(element(ids::CODEC_PRIVATE, &track.private)?);
        }
        if let Some(ns) = track.default_duration_ns {
            data.extend(uint(ids::DEFAULT_DURATION, ns)?);
        }
        if track.codec_delay_ns != 0 {
            data.extend(uint(ids::CODEC_DELAY, track.codec_delay_ns)?);
        }
        if track.seek_pre_roll_ns != 0 {
            data.extend(uint(ids::SEEK_PRE_ROLL, track.seek_pre_roll_ns)?);
        }
        match track.media {
            Media::Video {
                width,
                height,
                display,
                colour,
                interlaced,
                field_order,
            } => {
                let mut media = uint(ids::PIXEL_WIDTH, u64::from(width))?;
                media.extend(uint(ids::PIXEL_HEIGHT, u64::from(height))?);
                if interlaced != 0 {
                    media.extend(uint(ids::FLAG_INTERLACED, u64::from(interlaced))?);
                }
                if field_order != 2 {
                    media.extend(uint(ids::FIELD_ORDER, u64::from(field_order))?);
                }
                if let Some((w, h)) = display {
                    media.extend(uint(ids::DISPLAY_WIDTH, u64::from(w))?);
                    media.extend(uint(ids::DISPLAY_HEIGHT, u64::from(h))?);
                }
                let mut colors = Vec::new();
                for (id, value) in [
                    (ids::MATRIX_COEFFICIENTS, colour.matrix.map(u64::from)),
                    (
                        ids::RANGE,
                        colour.full_range.map(|full| if full { 2 } else { 1 }),
                    ),
                    (
                        ids::TRANSFER_CHARACTERISTICS,
                        colour.transfer.map(u64::from),
                    ),
                    (ids::PRIMARIES, colour.primaries.map(u64::from)),
                ] {
                    if let Some(value) = value {
                        colors.extend(uint(id, value)?);
                    }
                }
                if !colors.is_empty() {
                    media.extend(element(ids::COLOUR, &colors)?);
                }
                data.extend(element(ids::VIDEO, &media)?);
            }
            Media::Audio {
                sample_rate,
                channels,
                bit_depth,
            } => {
                let mut media = element(
                    ids::SAMPLING_FREQUENCY,
                    &f64::from(sample_rate).to_be_bytes(),
                )?;
                media.extend(uint(ids::CHANNELS, u64::from(channels))?);
                if let Some(bits) = bit_depth {
                    media.extend(uint(ids::BIT_DEPTH, u64::from(bits))?);
                }
                data.extend(element(ids::AUDIO, &media)?);
            }
        }
        if data.len() as u64 > max_element_bytes {
            return Err(limit(LimitKind::ElementBytes));
        }
        entries.extend(element(ids::TRACK_ENTRY, &data)?);
    }
    element(ids::TRACKS, &entries)
}

fn info(scale: u64, end_ns: Option<i128>) -> Result<Vec<u8>, Error> {
    let mut data = uint(ids::TIMESTAMP_SCALE, scale)?;
    if let Some(end) = end_ns.filter(|end| *end > 0) {
        #[allow(clippy::cast_precision_loss)]
        let duration = end as f64 / scale as f64;
        data.extend(element(ids::DURATION, &duration.to_be_bytes())?);
    }
    data.extend(element(ids::MUXING_APP, b"CoreAV")?);
    data.extend(element(ids::WRITING_APP, b"CoreAV")?);
    element(ids::INFO, &data)
}

fn seek_head(info: u64, tracks: u64, cues: Option<u64>) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    for (id, position) in [
        (ids::INFO, Some(info)),
        (ids::TRACKS, Some(tracks)),
        (ids::CUES, cues),
    ] {
        let Some(position) = position else {
            continue;
        };
        let mut seek = element(ids::SEEK_ID, &id_bytes(id))?;
        seek.extend(element(ids::SEEK_POSITION, &position.to_be_bytes())?);
        data.extend(element(ids::SEEK, &seek)?);
    }
    element(ids::SEEK_HEAD, &data)
}

fn block_head(packet: &Packet, track: &Track, relative: i16, scale: u64) -> Result<Vec<u8>, Error> {
    let number = header(0, packet.track as u64 + 1)?;
    let size = add(packet.size, number.len() as u64 + 3)?;
    let group = packet.discard_padding_ns.is_some_and(|n| n != 0)
        || packet
            .duration_ns
            .is_some_and(|d| Some(d) != track.default_duration_ns);
    let mut out = Vec::new();
    if group {
        let mut attributes = Vec::new();
        if let Some(duration) = packet.duration_ns.or(track.default_duration_ns) {
            let tail = packet.discard_padding_ns.unwrap_or(0).max(0) as u64;
            let ticks = rounded(i128::from(duration.saturating_sub(tail)), scale)?;
            if ticks == 0 && tail != duration {
                return Err(invalid("packet duration rounds to zero ticks"));
            }
            attributes.extend(uint(ids::BLOCK_DURATION, ticks as u64)?);
        }
        if !packet.keyframe {
            attributes.extend(sint(ids::REFERENCE_BLOCK, 0)?);
        }
        if let Some(padding) = packet.discard_padding_ns.filter(|n| *n != 0) {
            attributes.extend(sint(ids::DISCARD_PADDING, padding)?);
        }
        let block = header(ids::BLOCK, size)?;
        out.extend(header(
            ids::BLOCK_GROUP,
            add(add(block.len() as u64, size)?, attributes.len() as u64)?,
        )?);
        out.extend(attributes);
        out.extend(block);
    } else {
        out.extend(header(ids::SIMPLE_BLOCK, size)?);
    }
    out.extend(number);
    out.extend(relative.to_be_bytes());
    out.push(
        (if !group && packet.keyframe { 0x80 } else { 0 }) | if packet.invisible { 8 } else { 0 },
    );
    Ok(out)
}

fn rounded(ns: i128, scale: u64) -> Result<i64, Error> {
    i64::try_from((ns + i128::from(scale / 2)).div_euclid(i128::from(scale)))
        .map_err(|_| overflow())
}

fn add(a: u64, b: u64) -> Result<u64, Error> {
    a.checked_add(b).ok_or_else(overflow)
}
fn overflow() -> Error {
    Error::Overflow { offset: 0 }
}
fn limit(kind: LimitKind) -> Error {
    Error::LimitExceeded {
        limit: kind,
        offset: 0,
    }
}

struct Payload<'a, W> {
    out: &'a mut W,
    remaining: u64,
}

impl<W: Write> Write for Payload<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let wrote = self.out.write(bytes)?;
        self.remaining -= wrote as u64;
        Ok(wrote)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}
