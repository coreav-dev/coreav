//! Pixel decoding state, separate from header inspection.

use crate::adapt;
use crate::error::{Error, LimitKind, Result};
use crate::header::{self, Carried, FrameHeader, Header, Slot};
use crate::memory::Memory;
use crate::picture::{Picture, Planes, Sample};
use crate::probs::{Contexts, Probs};
use crate::tile::{self, DecodedTiles, ModeInfo, TileContext};
use crate::{CompressedHeader, Limits, loop_filter, superframe};

#[derive(Debug)]
struct Previous {
    width: u32,
    height: u32,
    shown: bool,
    key: bool,
    modes: Vec<ModeInfo>,
}

/// An experimental pixel decoder. Unlike [`crate::Decoder::read_headers`],
/// this entry point refuses frames it cannot reconstruct and never advances
/// its stream state using undecoded pictures or unadapted probabilities.
/// Today it supports profile 0 with single-reference and compound inter
/// prediction and segmentation.
#[derive(Debug)]
pub struct FrameDecoder {
    limits: Limits,
    slots: [Option<Slot>; 8],
    carried: Carried,
    contexts: Contexts,
    references: [Option<Picture>; 8],
    reference_slots: [Option<usize>; 8],
    previous: Option<Previous>,
    segment_ids: Vec<u8>,
}

impl FrameDecoder {
    /// Starts a stream within `limits`.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            slots: [None; 8],
            carried: Carried::default(),
            contexts: Contexts::default(),
            references: core::array::from_fn(|_| None),
            reference_slots: [None; 8],
            previous: None,
            segment_ids: Vec::new(),
        }
    }

    /// Decodes the shown pictures of a packet, in order. Hidden coded
    /// frames update references and probabilities without producing output.
    /// A failed frame changes no state; earlier successful members of a
    /// superframe remain committed.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<Picture>> {
        if packet.len() > self.limits.max_packet_bytes {
            return Err(Error::Limit(LimitKind::PacketBytes));
        }
        let mut memory = Memory::new(self.limits.max_memory_bytes);
        for reference in self.references.iter().flatten() {
            reference.charge(&mut memory)?;
        }
        if let Some(previous) = &self.previous {
            memory.charge::<ModeInfo>(previous.modes.capacity())?;
        }
        memory.charge::<u8>(self.segment_ids.capacity())?;
        let frames = superframe::split_with_memory(packet, &mut memory)?;
        let mut output = memory.empty(frames.len())?;
        for bytes in frames {
            let parsed = header::parse(bytes, &self.slots, &self.carried, &self.contexts)?;
            let (h, compressed) = match parsed.header {
                Header::ShowExisting { slot, .. } => {
                    let index = self
                        .reference_slots
                        .get(usize::from(slot))
                        .copied()
                        .flatten()
                        .ok_or(Error::Invalid("an unavailable shown reference"))?;
                    let reference = self
                        .references
                        .get(index)
                        .and_then(Option::as_ref)
                        .ok_or(Error::Invalid("an unavailable shown reference"))?;
                    output.push(reference.copy(&mut memory)?);
                    continue;
                }
                Header::Frame {
                    header, compressed, ..
                } => (header, compressed),
            };
            self.limits.check_size(h.width, h.height)?;
            if h.profile != 0 {
                return Err(Error::Unsupported("pixel decoding outside profile 0"));
            }
            let (mut contexts, probs) = parsed
                .probs
                .ok_or(Error::Invalid("missing frame probabilities"))?;
            let previous = self.previous.as_ref().filter(|p| {
                p.width == h.width
                    && p.height == h.height
                    && p.shown
                    && !h.error_resilient
                    && !h.is_intra()
            });
            let reset_segments = h.is_intra()
                || h.error_resilient
                || self
                    .previous
                    .as_ref()
                    .is_none_or(|p| p.width != h.width || p.height != h.height);
            let references: [Option<&Picture>; 8] = core::array::from_fn(|i| {
                self.reference_slots
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|index| self.references.get(index))
                    .and_then(Option::as_ref)
            });
            let context = TileContext {
                references: &references,
                previous: previous.map(|p| p.modes.as_slice()),
                previous_segments: if reset_segments {
                    &[]
                } else {
                    &self.segment_ids
                },
                reference_mode: compressed.reference_mode,
            };
            let data = bytes
                .get(h.uncompressed_size + usize::from(h.compressed_header_size)..)
                .unwrap_or_default();
            let (picture, tiles) =
                decode::<u8>(data, &h, &probs, compressed, context, &mut memory)?;
            let refreshed = adapt::refresh(
                &contexts.load(h.frame_context_idx),
                &probs,
                &tiles.counts,
                &h,
                compressed.tx_mode,
                self.previous.as_ref().is_some_and(|p| p.key),
            );
            if h.refresh_frame_context {
                contexts.save(h.frame_context_idx, refreshed);
            }
            let shown = if h.show_frame {
                Some(picture.copy(&mut memory)?)
            } else {
                None
            };
            let segments = if h.segmentation.enabled && h.segmentation.update_map {
                let mut segments = memory.empty(tiles.modes.len())?;
                segments.extend(tiles.modes.iter().map(|m| m.segment_id));
                Some(segments)
            } else {
                None
            };
            let mut reference_slots = self.reference_slots;
            for (i, slot) in reference_slots.iter_mut().enumerate() {
                if h.refresh_frame_flags >> i & 1 == 1 {
                    *slot = None;
                }
            }
            let retained_index = if h.refresh_frame_flags != 0 {
                Some(
                    (0..8)
                        .find(|i| !reference_slots.contains(&Some(*i)))
                        .ok_or(Error::Invalid("no free reference picture"))?,
                )
            } else {
                None
            };
            if let Some(index) = retained_index {
                let entry = self
                    .references
                    .get_mut(index)
                    .ok_or(Error::Invalid("reference picture index"))?;
                *entry = Some(picture);
                for (i, slot) in reference_slots.iter_mut().enumerate() {
                    if h.refresh_frame_flags >> i & 1 == 1 {
                        *slot = Some(index);
                    }
                }
            }
            for (i, reference) in self.references.iter_mut().enumerate() {
                if !reference_slots.contains(&Some(i)) {
                    *reference = None;
                }
            }
            self.reference_slots = reference_slots;
            for (i, slot) in self.slots.iter_mut().enumerate() {
                if h.refresh_frame_flags >> i & 1 == 1 {
                    *slot = Some(h.slot());
                }
            }
            self.contexts = contexts;
            self.carried = h.carried();
            if let Some(segments) = segments {
                self.segment_ids = segments;
            } else if reset_segments {
                self.segment_ids.clear();
            }
            self.previous = Some(Previous {
                width: h.width,
                height: h.height,
                shown: h.show_frame,
                key: h.key_frame,
                modes: tiles.modes,
            });
            if let Some(picture) = shown {
                output.push(picture);
            }
        }
        Ok(output)
    }
}

fn decode<P: Sample>(
    data: &[u8],
    h: &FrameHeader,
    probs: &Probs,
    compressed: CompressedHeader,
    context: TileContext<'_>,
    memory: &mut Memory,
) -> Result<(Picture, DecodedTiles)> {
    let c = h.color;
    let mut frame = Planes::<P>::new(
        h.mi_cols(),
        h.mi_rows(),
        c.subsampling_x,
        c.subsampling_y,
        memory,
    )?;
    let tiles = tile::read(
        data,
        h,
        probs,
        compressed.tx_mode,
        &mut frame,
        context,
        memory,
    )?;
    loop_filter::apply(h, &tiles.modes, &mut frame)?;
    Ok((
        frame.picture(
            h.width,
            h.height,
            c.bit_depth,
            c.subsampling_x,
            c.subsampling_y,
            memory,
        )?,
        tiles,
    ))
}

#[cfg(test)]
mod tests;
