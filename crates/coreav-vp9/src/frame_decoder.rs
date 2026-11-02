//! Pixel decoding state, separate from header inspection.

use std::sync::Arc;

use crate::adapt;
use crate::error::{Error, LimitKind, Result};
use crate::header::{self, Carried, FrameHeader, Header, Slot};
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
/// Today it supports profile 0 with single-reference inter prediction;
/// compound prediction and segmentation in inter frames are unsupported.
#[derive(Debug)]
pub struct FrameDecoder {
    limits: Limits,
    slots: [Option<Slot>; 8],
    carried: Carried,
    contexts: Contexts,
    references: [Option<Arc<Picture>>; 8],
    previous: Option<Previous>,
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
            previous: None,
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
        let mut output = Vec::new();
        for bytes in superframe::split(packet)? {
            let parsed = header::parse(bytes, &self.slots, &self.carried, &self.contexts)?;
            let (h, compressed) = match parsed.header {
                Header::ShowExisting { slot, .. } => {
                    let reference = self
                        .references
                        .get(usize::from(slot))
                        .and_then(Option::as_deref)
                        .ok_or(Error::Invalid("an unavailable shown reference"))?;
                    output.push(reference.clone());
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
            let (mut contexts, probs) = *parsed
                .probs
                .ok_or(Error::Invalid("missing frame probabilities"))?;
            let previous = self.previous.as_ref().filter(|p| {
                p.width == h.width
                    && p.height == h.height
                    && p.shown
                    && !h.error_resilient
                    && !h.is_intra()
            });
            let context = TileContext {
                references: &self.references,
                previous: previous.map(|p| p.modes.as_slice()),
                reference_mode: compressed.reference_mode,
            };
            let data = bytes
                .get(h.uncompressed_size + usize::from(h.compressed_header_size)..)
                .unwrap_or_default();
            let (picture, tiles) = decode::<u8>(data, &h, &probs, compressed, context)?;
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
            let picture = Arc::new(picture);
            for (i, (slot, reference)) in
                self.slots.iter_mut().zip(&mut self.references).enumerate()
            {
                if h.refresh_frame_flags >> i & 1 == 1 {
                    *slot = Some(h.slot());
                    *reference = Some(Arc::clone(&picture));
                }
            }
            self.contexts = contexts;
            self.carried = h.carried();
            self.previous = Some(Previous {
                width: h.width,
                height: h.height,
                shown: h.show_frame,
                key: h.key_frame,
                modes: tiles.modes,
            });
            if h.show_frame {
                output.push(picture.as_ref().clone());
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
) -> Result<(Picture, DecodedTiles)> {
    let c = h.color;
    let mut frame = Planes::<P>::new(h.mi_cols(), h.mi_rows(), c.subsampling_x, c.subsampling_y);
    let tiles = tile::read(data, h, probs, compressed.tx_mode, &mut frame, context)?;
    loop_filter::apply(h, &tiles.modes, &mut frame)?;
    Ok((
        frame.picture(
            h.width,
            h.height,
            c.bit_depth,
            c.subsampling_x,
            c.subsampling_y,
        ),
        tiles,
    ))
}

#[cfg(test)]
mod tests;
