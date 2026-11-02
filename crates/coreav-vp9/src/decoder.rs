//! The decoder's state between packets.

use crate::compressed::TxMode;
use crate::error::{Error, LimitKind, Result};
use crate::header::{self, Carried, FrameHeader, Header, Slot};
use crate::limits::Limits;
use crate::loop_filter;
use crate::picture::{Picture, Planes, Sample};
use crate::probs::{Contexts, Probs};
use crate::superframe;
use crate::tile;

/// A VP9 decoder: what one stream's frames leave for the next.
///
/// Today it reads headers only ([`Decoder::read_headers`]): it splits each
/// packet into its frames, reads each frame's uncompressed and compressed
/// headers, and keeps the reference slots' sizes and formats and the saved
/// header-updated probability contexts. It reconstructs intra frames but
/// skips inter tiles, so it does not perform backward adaptation. Use
/// [`crate::FrameDecoder`] for pixel decoding with its own complete state.
#[derive(Debug)]
pub struct Decoder {
    limits: Limits,
    slots: [Option<Slot>; 8],
    carried: Carried,
    /// Inspection's saved contexts, without backward adaptation because
    /// inspection does not read inter tiles.
    contexts: Contexts,
}

impl Decoder {
    /// A decoder for one stream, within `limits`.
    pub fn new(limits: Limits) -> Self {
        Decoder {
            limits,
            slots: [None; 8],
            carried: Carried::default(),
            contexts: Contexts::default(),
        }
    }

    /// Reads the headers of every frame of `packet`, a frame or a
    /// superframe, in order, saves the probability contexts each frame
    /// refreshes, and refreshes the reference slots each frame names
    /// (clause 8.10).
    ///
    /// A frame that fails leaves the state as the frames before it left it.
    pub fn read_headers(&mut self, packet: &[u8]) -> Result<Vec<Header>> {
        if packet.len() > self.limits.max_packet_bytes {
            return Err(Error::Limit(LimitKind::PacketBytes));
        }
        let frames = superframe::split(packet)?;
        let mut headers = Vec::with_capacity(frames.len());
        for frame in frames {
            let mut parsed = header::parse(frame, &self.slots, &self.carried, &self.contexts)?;
            if let (
                Header::Frame {
                    header: h,
                    compressed,
                    tiles_end_cleanly,
                    picture,
                },
                Some(probs),
            ) = (&mut parsed.header, parsed.probs)
            {
                self.limits.check_size(h.width, h.height)?;
                let (mut contexts, probs) = *probs;
                if h.is_intra() {
                    let tiles = frame
                        .get(h.uncompressed_size + usize::from(h.compressed_header_size)..)
                        .unwrap_or_default();
                    let (clean, decoded) = if h.color.bit_depth == 8 {
                        decode_intra::<u8>(tiles, h, &probs, compressed.tx_mode)?
                    } else {
                        decode_intra::<u16>(tiles, h, &probs, compressed.tx_mode)?
                    };
                    *tiles_end_cleanly = Some(clean);
                    *picture = Some(Box::new(decoded));
                }
                if h.refresh_frame_context {
                    contexts.save(h.frame_context_idx, probs);
                }
                self.contexts = contexts;
                for (i, slot) in self.slots.iter_mut().enumerate() {
                    if h.refresh_frame_flags >> i & 1 == 1 {
                        *slot = Some(h.slot());
                    }
                }
                self.carried = h.carried();
            }
            headers.push(parsed.header);
        }
        Ok(headers)
    }

    #[cfg(test)]
    pub(crate) fn contexts(&self) -> &Contexts {
        &self.contexts
    }
}

/// Decodes the tiles of intra frame `h` and filters the result (clauses
/// 8.6 to 8.8). Returns whether every tile ended cleanly, and the frame.
fn decode_intra<P: Sample>(
    tiles: &[u8],
    h: &FrameHeader,
    probs: &Probs,
    tx_mode: TxMode,
) -> Result<(bool, Picture)> {
    let c = h.color;
    let mut frame = Planes::<P>::new(h.mi_cols(), h.mi_rows(), c.subsampling_x, c.subsampling_y);
    let decoded = tile::read(
        tiles,
        h,
        probs,
        tx_mode,
        &mut frame,
        tile::TileContext {
            references: &[],
            previous: None,
            previous_segments: &[],
            reference_mode: crate::compressed::ReferenceMode::Single,
        },
    )?;
    loop_filter::apply(h, &decoded.modes, &mut frame)?;
    Ok((
        decoded.clean,
        frame.picture(
            h.width,
            h.height,
            c.bit_depth,
            c.subsampling_x,
            c.subsampling_y,
        ),
    ))
}
