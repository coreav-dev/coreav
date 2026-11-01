//! The decoder's state between packets.

use crate::error::{Error, LimitKind, Result};
use crate::header::{self, Carried, Header, Slot};
use crate::limits::Limits;
use crate::probs::Contexts;
use crate::superframe;

/// A VP9 decoder: what one stream's frames leave for the next.
///
/// Today it reads headers only ([`Decoder::read_headers`]): it splits each
/// packet into its frames, reads each frame's uncompressed and compressed
/// headers, and keeps the reference slots' sizes and formats and the saved
/// probability contexts as decoding the frames would.
#[derive(Debug)]
pub struct Decoder {
    limits: Limits,
    slots: [Option<Slot>; 8],
    carried: Carried,
    /// The saved frame contexts. A frame that adapts its probabilities to
    /// what its tiles decoded (`refresh_probs()`, clause 6.1.2) saves them
    /// unadapted until tiles are decoded (`docs/design/vp9.md`, milestone
    /// 2).
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
            let parsed = header::parse(frame, &self.slots, &self.carried, &self.contexts)?;
            if let (Header::Frame { header: h, .. }, Some(probs)) = (&parsed.header, parsed.probs) {
                self.check_size(h.width, h.height)?;
                let (mut contexts, probs) = *probs;
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

    fn check_size(&self, width: u32, height: u32) -> Result<()> {
        if width > self.limits.max_width {
            return Err(Error::Limit(LimitKind::Width));
        }
        if height > self.limits.max_height {
            return Err(Error::Limit(LimitKind::Height));
        }
        if u64::from(width) * u64::from(height) > self.limits.max_pixels {
            return Err(Error::Limit(LimitKind::Pixels));
        }
        Ok(())
    }
}
