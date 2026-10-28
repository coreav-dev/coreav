//! Packet iteration: one packet per frame, in storage order (RFC 9559 10
//! and 11).

use crate::audit::Audit;
use crate::block::{BlockHeader, parse_block};
use crate::ebml::{self, ElementHeader, Parsed};
use crate::error::{LimitKind, Warning, warn};
use crate::ids;
use crate::inflate::{FRAME_CHARGE, Failure, Inflater};
use crate::reader::{read_data, read_header};
use crate::segment::Segment;
use crate::track::{Encoding, Track};
use crate::{Error, Limits, Source};
use std::collections::VecDeque;

/// Block header bytes read before the lacing is known.
const FIRST_BLOCK_READ: u64 = 64;

/// One frame of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Index of the track in [`Segment::tracks`].
    pub track: usize,
    /// Absolute offset of the frame's stored bytes.
    pub offset: u64,
    /// Stored bytes of the frame.
    pub stored_size: u64,
    /// Bytes the track's header stripping removed from the front of the
    /// frame; the frame is these bytes followed by the stored ones.
    pub stripped: u32,
    /// The frame's size once inflated, for a track whose frames are
    /// zlib-compressed ([`Encoding::Zlib`]), when the listing was asked for
    /// it ([`Packets::with_inflated_sizes`]) and the frame inflates within
    /// the limits; the stored bytes are the compressed frame. `None` for a
    /// zlib frame means its size is unknown.
    pub inflated_size: Option<u64>,
    /// Absolute offset of the data of the SimpleBlock or Block element that
    /// holds the frame.
    pub block_offset: u64,
    /// Position of the frame in its block.
    pub lace: u16,
    /// Frames in the block (1 without lacing).
    pub frames_in_block: u16,
    /// The block's timestamp in Track Ticks, as stored: the Cluster
    /// timestamp plus the block's relative timestamp (RFC 9559 11.2).
    pub block_timestamp: i64,
    /// Presentation timestamp in Track Ticks: the block timestamp, plus,
    /// for a later frame of a lace, its position times `DefaultDuration`
    /// truncated to the tick in which the frame starts, minus `CodecDelay`
    /// rounded to the nearest tick (halves up). [`Packet::pts_ns`] gives
    /// the exact time. `None` when it cannot be known: a later frame of a
    /// lace without `DefaultDuration`, or a `TrackTimestampScale` other
    /// than 1.0.
    pub pts: Option<i64>,
    /// Decode timestamp in Track Ticks: the presentation timestamp for
    /// codecs that decode in presentation order
    /// ([`crate::Codec::decodes_in_presentation_order`]), else `None`,
    /// because Matroska stores no decode times.
    pub dts: Option<i64>,
    /// `BlockDuration` in Track Ticks, for a frame of a BlockGroup that has
    /// one.
    pub block_duration: Option<u64>,
    /// A random access point: the SimpleBlock keyframe flag, or a
    /// BlockGroup without `ReferenceBlock` (RFC 9559 10.4).
    pub keyframe: bool,
    /// The invisible flag.
    pub invisible: bool,
    /// The SimpleBlock discardable flag.
    pub discardable: bool,
    /// `DiscardPadding` in nanoseconds.
    pub discard_padding: Option<i64>,
    /// The `BlockAdditions` element of the frame's BlockGroup (for example
    /// a VP8 or VP9 alpha plane): its absolute offset and size, header
    /// included.
    pub block_additions: Option<(u64, u64)>,
}

impl Packet {
    /// Size of the frame in bytes: inflated for a zlib-compressed track
    /// when known ([`Packet::inflated_size`]), else stored, stripped header
    /// included.
    pub fn size(&self) -> u64 {
        self.inflated_size
            .unwrap_or_else(|| self.stored_size.saturating_add(u64::from(self.stripped)))
    }
}

impl Segment {
    /// Iterates the packets of every kept track in storage order, from the
    /// first Cluster. Blocks of dropped tracks and of tracks whose frames
    /// cannot be read ([`Track::packets_available`]) are skipped.
    pub fn packets<S: Source>(&self, source: S, limits: &Limits) -> Packets<'_, S> {
        Packets::new(self, source, limits, self.first_cluster)
    }

    /// Iterates packets from the Cluster at absolute offset `cluster`, for
    /// example one that [`crate::Index::cluster_for`] returned.
    pub fn packets_from<S: Source>(
        &self,
        source: S,
        limits: &Limits,
        cluster: u64,
    ) -> Packets<'_, S> {
        Packets::new(self, source, limits, Some(cluster))
    }
}

#[derive(Clone, Copy, Debug)]
struct Cluster {
    /// Known end, or `None` for an unknown size.
    end: Option<u64>,
    timestamp: Option<u64>,
}

/// Iterator over packets; see [`Segment::packets`].
///
/// After a file ends inside a Cluster or a block, the iterator returns
/// [`Error::Truncated`] once, after every frame of every complete block (a
/// block cut off by the end of the file yields none of its frames).
pub struct Packets<'a, S> {
    segment: &'a Segment,
    source: S,
    limits: Limits,
    len: Option<u64>,
    work: u64,
    /// The decompressor that sizes zlib-compressed frames, when asked for
    /// ([`Packets::with_inflated_sizes`]), and the budget it has used
    /// against `max_inflated_bytes`.
    inflater: Option<(Inflater, u64)>,
    /// The structure audit, walked along with the listing when the scan
    /// asks for it ([`Packets::with_audit`]).
    audit: Option<Audit>,
    at: u64,
    cluster: Option<Cluster>,
    pending: VecDeque<Packet>,
    done: bool,
    warnings: Vec<Warning>,
}

impl<'a, S: Source> Packets<'a, S> {
    fn new(segment: &'a Segment, source: S, limits: &Limits, start: Option<u64>) -> Self {
        Self {
            segment,
            source,
            limits: *limits,
            len: None,
            work: 0,
            inflater: None,
            audit: None,
            at: start.unwrap_or(segment.end),
            cluster: None,
            pending: VecDeque::new(),
            done: start.is_none(),
            warnings: Vec::new(),
        }
    }

    /// Sizes zlib-compressed frames by inflating them
    /// ([`Packet::inflated_size`]). Each frame is bounded by
    /// `max_packet_bytes`, and the listing by `max_inflated_bytes`, which
    /// every frame is charged its inflated bytes plus 4 KiB against; a frame
    /// that does not inflate within them, or is not a complete zlib stream,
    /// is listed with an unknown size. Without it, zlib frames are listed
    /// with their stored ranges only, and nothing is inflated.
    pub fn with_inflated_sizes(mut self) -> Self {
        self.inflater = Some((Inflater::new(), 0));
        self
    }

    /// Walks the structure audit (`crate::audit`) with the listing, from the
    /// start of the Segment to the end of the file: an audit error, found
    /// anywhere, is returned instead of the listing's error or its end.
    pub(crate) fn with_audit(mut self) -> Self {
        self.audit = Some(Audit::new(self.segment, &self.limits));
        self
    }

    /// Walks the audit up to and past the element at `at`.
    fn audit_through(&mut self, at: u64) -> Result<(), Error> {
        match self.audit.as_mut() {
            Some(audit) => audit.through(self.segment, &mut self.source, at),
            None => Ok(()),
        }
    }

    /// Walks the rest of the audit, once: the listing is over.
    fn finish_audit(&mut self) -> Result<(), Error> {
        match self.audit.take() {
            Some(mut audit) => audit.through(self.segment, &mut self.source, u64::MAX),
            None => Ok(()),
        }
    }

    /// The error to report for `error`, met by the listing or by its caller
    /// reading the packets: the audit's, if the rest of the file fails it.
    /// A source that failed to read is not read further: its error stays.
    pub(crate) fn audit_error(&mut self, error: Error) -> Error {
        if matches!(error, Error::Io(_)) {
            return error;
        }
        match self.finish_audit() {
            Err(audit) => audit,
            Ok(()) => error,
        }
    }

    /// Problems met so far, such as blocks of undefined tracks.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The source the packets are read from, so a caller can read each
    /// packet's bytes between calls. Reads through it do not move the
    /// iterator.
    pub fn source_mut(&mut self) -> &mut S {
        &mut self.source
    }

    /// Returns the source.
    pub fn into_source(self) -> S {
        self.source
    }

    fn count(&mut self, offset: u64) -> Result<(), Error> {
        if self.work >= self.limits.max_block_work {
            return Err(Error::LimitExceeded {
                limit: LimitKind::BlockWork,
                offset,
            });
        }
        self.work += 1;
        Ok(())
    }

    /// Reads elements until a block yields frames or the segment ends.
    fn advance(&mut self) -> Result<(), Error> {
        let len = match self.len {
            Some(len) => len,
            None => {
                let len = self.source.len()?;
                self.len = Some(len);
                len
            }
        };
        let segment_end = self.segment.end.min(len);
        while self.pending.is_empty() {
            let Some(cluster) = self.cluster else {
                if self.at >= segment_end {
                    self.done = true;
                    return Ok(());
                }
                self.top_level(segment_end)?;
                if self.done {
                    return Ok(());
                }
                continue;
            };
            let limit = cluster.end.unwrap_or(segment_end);
            if self.at >= limit {
                if let Some(end) = cluster.end {
                    if end > len {
                        // A sized Cluster cut off by the end of the file.
                        return Err(Error::Truncated { offset: len });
                    }
                }
                self.cluster = None;
                continue;
            }
            self.audit_through(self.at)?;
            self.count(self.at)?;
            let h = read_header(
                &mut self.source,
                self.at,
                limit.min(len),
                self.segment.max_size_len,
            )?;
            if cluster.end.is_none() && (h.id == ids::EBML || ids::TOP_LEVEL.contains(&h.id)) {
                // An element that cannot be a child ends an unknown-sized
                // Cluster (RFC 8794 6.2).
                self.cluster = None;
                continue;
            }
            let end = h.end().ok_or(Error::InvalidElement {
                offset: h.offset,
                id: h.id,
                reason: "unknown size not allowed for this element",
            })?;
            if cluster.end.is_some_and(|cluster_end| end > cluster_end) {
                return Err(Error::InvalidElement {
                    offset: h.offset,
                    id: h.id,
                    reason: "element runs past its Cluster",
                });
            }
            if end > len {
                return Err(Error::Truncated { offset: h.offset });
            }
            match h.id {
                ids::TIMESTAMP => {
                    let data = read_data(&mut self.source, &h, 8, LimitKind::ElementBytes)?;
                    let timestamp = ebml::uint(&data, &h)?;
                    if let Some(cluster) = self.cluster.as_mut() {
                        cluster.timestamp = Some(timestamp);
                    }
                }
                ids::SIMPLE_BLOCK => {
                    let header = self.block_header(&h)?;
                    self.block(&h, header, None)?;
                }
                ids::BLOCK_GROUP => self.block_group(&h)?,
                _ => {}
            }
            self.at = end;
        }
        Ok(())
    }

    /// Reads one element outside a Cluster.
    fn top_level(&mut self, segment_end: u64) -> Result<(), Error> {
        self.audit_through(self.at)?;
        self.count(self.at)?;
        let h = read_header(
            &mut self.source,
            self.at,
            segment_end,
            self.segment.max_size_len,
        )?;
        match (h.id, h.end()) {
            (ids::EBML, _) => self.done = true,
            (ids::CLUSTER, end) => {
                self.cluster = Some(Cluster {
                    end,
                    timestamp: None,
                });
                self.at = h.data_offset();
            }
            (_, Some(end)) => self.at = end,
            (id, None) => {
                return Err(Error::InvalidElement {
                    offset: h.offset,
                    id,
                    reason: "unknown size not allowed for this element",
                });
            }
        }
        Ok(())
    }

    fn block_group(&mut self, group: &ElementHeader) -> Result<(), Error> {
        let end = group.end().ok_or(Error::Overflow {
            offset: group.offset,
        })?;
        let mut at = group.data_offset();
        let mut extra = GroupFields::default();
        let mut block = None;
        while at < end {
            self.audit_through(at)?;
            self.count(at)?;
            let h = read_header(&mut self.source, at, end, self.segment.max_size_len)?;
            let child_end = h.end().filter(|e| *e <= end).ok_or(Error::InvalidElement {
                offset: at,
                id: h.id,
                reason: "element runs past its BlockGroup",
            })?;
            match h.id {
                // The Block's header is read where the Block is, before the
                // elements after it, so that the reads move forward; an
                // error in it counts after theirs, as when it was read after
                // them, but a source that failed to read stops the listing.
                ids::BLOCK if block.is_none() => {
                    let header = self.block_header(&h);
                    if let Err(error @ Error::Io(_)) = header {
                        return Err(error);
                    }
                    block = Some((h, header));
                }
                ids::BLOCK_DURATION => {
                    let data = read_data(&mut self.source, &h, 8, LimitKind::ElementBytes)?;
                    extra.duration = Some(ebml::uint(&data, &h)?);
                }
                ids::REFERENCE_BLOCK => extra.referenced = true,
                ids::BLOCK_ADDITIONS => extra.additions = Some((h.offset, child_end - h.offset)),
                ids::DISCARD_PADDING => {
                    let data = read_data(&mut self.source, &h, 8, LimitKind::ElementBytes)?;
                    extra.discard_padding = Some(ebml::int(&data, &h)?);
                }
                _ => {}
            }
            at = child_end;
        }
        match block {
            Some((block, header)) => self.block(&block, header?, Some(extra)),
            None => Err(Error::InvalidElement {
                offset: group.offset,
                id: group.id,
                reason: "BlockGroup without a Block",
            }),
        }
    }

    /// Reads the header of a SimpleBlock or a Block: the first read into a
    /// buffer on the stack, a longer one (laced frame sizes) into the spare
    /// capacity of a vector, so that no block allocates and zeroes a buffer.
    fn block_header(&mut self, h: &ElementHeader) -> Result<BlockHeader, Error> {
        let size = h.size.unwrap_or(0);
        let offset = h.data_offset();
        let mut want = size.min(FIRST_BLOCK_READ);
        let (mut stack, mut heap) = ([0u8; FIRST_BLOCK_READ as usize], Vec::new());
        loop {
            if want > self.limits.max_element_bytes {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::ElementBytes,
                    offset,
                });
            }
            let len = usize::try_from(want).unwrap_or(0);
            let buf: &[u8] = match stack.get_mut(..len) {
                Some(target) => {
                    self.source.read_exact_at(offset, target)?;
                    target
                }
                None => {
                    heap.clear();
                    self.source.read_append(offset, len, &mut heap)?;
                    &heap
                }
            };
            match parse_block(buf, size, offset)? {
                Parsed::Done(header) => return Ok(header),
                Parsed::NeedMore(n) => want = (n as u64).max(want.saturating_mul(2)).min(size),
            }
        }
    }

    /// Queues the frames of a SimpleBlock (`group` is `None`) or the Block
    /// of a BlockGroup, from its header.
    fn block(
        &mut self,
        h: &ElementHeader,
        header: BlockHeader,
        group: Option<GroupFields>,
    ) -> Result<(), Error> {
        let offset = h.data_offset();
        let segment = self.segment;
        let Some(track) = segment.tracks.iter().find(|t| t.number == header.track) else {
            if !segment.dropped.iter().any(|d| d.number == header.track) {
                warn(&mut self.warnings, Warning::UnknownTrackNumber);
            }
            return Ok(());
        };
        if !track.packets_available() {
            return Ok(());
        }
        let cluster_time = self
            .cluster
            .and_then(|c| c.timestamp)
            .ok_or(Error::InvalidElement {
                offset: h.offset,
                id: h.id,
                reason: "block before the Cluster Timestamp",
            })?;
        let block_timestamp = i64::try_from(cluster_time)
            .ok()
            .and_then(|t| t.checked_add(i64::from(header.timestamp)))
            .ok_or(Error::Overflow { offset: h.offset })?;
        let frames_in_block = u16::try_from(header.frames.len()).unwrap_or(u16::MAX);
        let stripped = match &track.encoding {
            Encoding::HeaderStripping(prefix) => u32::try_from(prefix.len()).unwrap_or(u32::MAX),
            _ => 0,
        };
        // Every frame is checked before any is queued, so a rejected block
        // yields none.
        let too_large = header.frames.iter().any(|(_, size)| {
            size.saturating_add(u64::from(stripped)) > self.limits.max_packet_bytes
        });
        if too_large {
            return Err(Error::LimitExceeded {
                limit: LimitKind::PacketBytes,
                offset,
            });
        }
        // A zlib-compressed frame is sized by inflating it, when asked; a
        // frame that does not inflate within the limits has an unknown size.
        let mut inflated_sizes = Vec::new();
        if let (Encoding::Zlib, Some((inflater, used))) = (&track.encoding, self.inflater.as_mut())
        {
            for (frame_offset, frame_size) in &header.frames {
                let budget = self
                    .limits
                    .max_inflated_bytes
                    .saturating_sub(*used)
                    .checked_sub(FRAME_CHARGE);
                let Some(budget) = budget else {
                    inflated_sizes.push(None);
                    continue;
                };
                let max = self.limits.max_packet_bytes.min(budget);
                let at = offset.saturating_add(*frame_offset);
                // Each frame is charged what it inflated to: all of `max`
                // for one that went past it.
                let (size, charge) = match inflater.size(&mut self.source, at, *frame_size, max) {
                    Ok(size) => (Some(size), size),
                    Err(Failure::Read(error)) => return Err(error),
                    Err(Failure::TooLarge) => (None, max),
                    Err(Failure::Invalid(inflated)) => (None, inflated),
                };
                *used = used.saturating_add(FRAME_CHARGE).saturating_add(charge);
                inflated_sizes.push(size);
            }
        }
        let timing = Timing::new(track, segment.timestamp_scale);
        for (lace, (frame_offset, frame_size)) in header.frames.iter().enumerate() {
            let lace = u16::try_from(lace).unwrap_or(u16::MAX);
            let pts = timing.pts(block_timestamp, lace);
            let unlaced = frames_in_block == 1;
            self.pending.push_back(Packet {
                track: track.index,
                offset: offset.saturating_add(*frame_offset),
                stored_size: *frame_size,
                stripped,
                inflated_size: inflated_sizes.get(usize::from(lace)).copied().flatten(),
                block_offset: offset,
                lace,
                frames_in_block,
                block_timestamp,
                pts,
                dts: pts.filter(|_| track.codec.decodes_in_presentation_order()),
                block_duration: group.and_then(|g| g.duration).filter(|_| unlaced),
                keyframe: match group {
                    None => header.keyframe(),
                    Some(group) => !group.referenced,
                },
                invisible: header.invisible(),
                discardable: group.is_none() && header.discardable(),
                discard_padding: group.and_then(|g| g.discard_padding),
                block_additions: group.and_then(|g| g.additions).filter(|_| lace == 0),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct GroupFields {
    duration: Option<u64>,
    referenced: bool,
    discard_padding: Option<i64>,
    additions: Option<(u64, u64)>,
}

/// How a track's block timestamps become presentation timestamps.
struct Timing {
    /// `CodecDelay` in Track Ticks, rounded to the nearest tick.
    delay: i64,
    default_duration: Option<u64>,
    scale: u64,
    applies: bool,
}

impl Timing {
    fn new(track: &Track, scale: u64) -> Self {
        Self {
            delay: ticks_rounded(u128::from(track.codec_delay), scale),
            default_duration: track.default_duration,
            scale,
            applies: track.timestamp_scale_one,
        }
    }

    fn pts(&self, block_timestamp: i64, lace: u16) -> Option<i64> {
        if !self.applies {
            return None;
        }
        let offset = if lace == 0 {
            0
        } else {
            // The tick in which the frame starts.
            let duration = u128::from(self.default_duration?);
            let ticks = duration.checked_mul(u128::from(lace))? / u128::from(self.scale.max(1));
            i64::try_from(ticks).ok()?
        };
        block_timestamp.checked_add(offset)?.checked_sub(self.delay)
    }
}

/// Nanoseconds to ticks of `scale` nanoseconds, rounded to the nearest
/// tick, halves up; saturates.
fn ticks_rounded(nanoseconds: u128, scale: u64) -> i64 {
    let scale = u128::from(scale.max(1));
    let ticks = nanoseconds.saturating_add(scale / 2) / scale;
    i64::try_from(ticks).unwrap_or(i64::MAX)
}

impl Packet {
    /// The exact presentation time in nanoseconds (RFC 9559 11.2): the
    /// block timestamp times `TimestampScale`, plus the frame's position in
    /// its lace times `DefaultDuration`, minus `CodecDelay`. `None` when
    /// [`Packet::pts`] is `None` for the same reasons.
    pub fn pts_ns(&self, segment: &Segment) -> Option<i128> {
        let track = segment.tracks.get(self.track)?;
        if !track.timestamp_scale_one {
            return None;
        }
        let lace = if self.lace == 0 {
            0
        } else {
            i128::from(track.default_duration?).checked_mul(i128::from(self.lace))?
        };
        i128::from(self.block_timestamp)
            .checked_mul(i128::from(segment.timestamp_scale))?
            .checked_add(lace)?
            .checked_sub(i128::from(track.codec_delay))
    }
}

impl<S: Source> Iterator for Packets<'_, S> {
    type Item = Result<Packet, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(packet) = self.pending.pop_front() {
            return Some(Ok(packet));
        }
        if !self.done {
            match self.advance() {
                Ok(()) => {
                    if let Some(packet) = self.pending.pop_front() {
                        return Some(Ok(packet));
                    }
                }
                Err(error) => {
                    self.done = true;
                    return Some(Err(self.audit_error(error)));
                }
            }
        }
        // The listing is over; the audit, if any, walks on to the end of
        // the file.
        self.finish_audit().err().map(Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_rounding() {
        // Opus pre-skip of 312 samples: 6.5 ms rounds up to 7 ticks.
        assert_eq!(ticks_rounded(6_500_000, 1_000_000), 7);
        // AAC priming of 1024 samples at 44.1 kHz: 23.22 ms.
        assert_eq!(ticks_rounded(23_219_955, 1_000_000), 23);
        assert_eq!(ticks_rounded(0, 0), 0);
    }
}
