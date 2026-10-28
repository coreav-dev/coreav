//! Whether an H.264 or HEVC sample starts a coded video sequence, from its
//! NAL unit headers (ITU-T H.264 7.4.1.2, ITU-T H.265 7.4.2.2), for sources
//! whose sync sample table says nothing: without one, ISO/IEC 14496-12
//! makes every sample a sync sample.

use coreav_isobmff::{Codec, Error, Source, Track};

/// Most NAL units looked at before the first slice.
const MAX_UNITS: u32 = 64;

/// How to find a sample's NAL units: the length field size of its
/// configuration record, and the codec.
#[derive(Clone, Copy)]
pub(crate) struct Units {
    length_size: u8,
    hevc: bool,
}

impl Units {
    /// For H.264 `avc1` and HEVC `hvc1` tracks with a readable
    /// configuration record; `None` for every other track.
    pub(crate) fn of(track: &Track) -> Option<Self> {
        let config = track.description.codec_config.as_ref()?;
        let (hevc, at) = match track.description.codec {
            // avcC: lengthSizeMinusOne in the low two bits of byte 4.
            Codec::H264 => (false, 4),
            // hvcC: lengthSizeMinusOne in the low two bits of byte 21.
            Codec::Hevc => (true, 21),
            _ => return None,
        };
        let length_size = config.data.get(at).map(|b| (b & 0x03) + 1)?;
        (length_size != 3).then_some(Self { length_size, hevc })
    }

    /// Whether the sample at `offset` of `size` bytes starts with an IDR
    /// (H.264) or IRAP (HEVC) picture: its first slice NAL unit has such a
    /// type. A sample whose NAL units cannot be read is not.
    pub(crate) fn starts_sequence<S: Source>(
        self,
        source: &mut S,
        offset: u64,
        size: u32,
    ) -> Result<bool, Error> {
        let len_size = u64::from(self.length_size);
        let end = offset.saturating_add(u64::from(size));
        let mut at = offset;
        for _ in 0..MAX_UNITS {
            let header_end = at.saturating_add(len_size).saturating_add(1);
            if header_end > end {
                return Ok(false);
            }
            let mut head = [0u8; 5];
            let wanted = usize::from(self.length_size).saturating_add(1);
            let Some(buf) = head.get_mut(..wanted) else {
                return Ok(false);
            };
            source.read_exact_at(at, buf)?;
            let (length, rest) = buf.split_at(usize::from(self.length_size));
            let length = length.iter().fold(0u64, |n, b| (n << 8) | u64::from(*b));
            let Some(&first) = rest.first() else {
                return Ok(false);
            };
            if self.hevc {
                let kind = (first >> 1) & 0x3f;
                // VCL units are 0 to 31; IRAP pictures 16 to 23.
                if kind < 32 {
                    return Ok((16..=23).contains(&kind));
                }
            } else {
                let kind = first & 0x1f;
                // Slices are 1 to 5; 5 is an IDR picture.
                if (1..=5).contains(&kind) {
                    return Ok(kind == 5);
                }
            }
            at = at.saturating_add(len_size).saturating_add(length);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_isobmff::SliceSource;

    fn units(hevc: bool) -> Units {
        Units {
            length_size: 4,
            hevc,
        }
    }

    #[test]
    fn finds_the_first_slice() {
        // H.264: SEI (6), SPS (7), then an IDR slice (5) or a P slice (1).
        let idr = [0, 0, 0, 2, 6, 0, 0, 0, 0, 2, 7, 0, 0, 0, 0, 2, 0x65, 0];
        let mut src = SliceSource::new(&idr);
        assert!(units(false).starts_sequence(&mut src, 0, 18).unwrap());
        let p = [0, 0, 0, 2, 6, 0, 0, 0, 0, 2, 0x41, 0];
        let mut src = SliceSource::new(&p);
        assert!(!units(false).starts_sequence(&mut src, 0, 12).unwrap());
        // HEVC: VPS (32), then a CRA (21) or a TRAIL_R (1) picture.
        let cra = [0, 0, 0, 3, 0x40, 1, 0, 0, 0, 0, 3, 0x2a, 1, 0];
        let mut src = SliceSource::new(&cra);
        assert!(units(true).starts_sequence(&mut src, 0, 14).unwrap());
        let trail = [0, 0, 0, 3, 0x02, 1, 0];
        let mut src = SliceSource::new(&trail);
        assert!(!units(true).starts_sequence(&mut src, 0, 7).unwrap());
        // Lengths past the sample, and an empty sample.
        let long = [0, 0, 0, 200, 6, 0];
        let mut src = SliceSource::new(&long);
        assert!(!units(false).starts_sequence(&mut src, 0, 6).unwrap());
        assert!(!units(false).starts_sequence(&mut src, 0, 0).unwrap());
    }
}
