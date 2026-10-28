//! Packets that nothing decoded after them depends on, which `--exact`
//! need not send to the decoder when they come before its target
//! (`docs/design/thumbnails.md`, "Frames nothing depends on").
//!
//! A packet is disposable when its picture cannot be a reference and it
//! carries nothing else that later pictures read:
//! - ITU-T H.264 access units whose every slice has `nal_ref_idc` 0 (7.4.1);
//! - ITU-T H.265 access units whose every slice is a sub-layer non-reference
//!   picture (7.4.2.2) of the highest TemporalId among the packets decoded.
//!   Only a picture of a higher TemporalId may reference such a picture, so
//!   with none decoded, nothing decoded references it. The highest
//!   TemporalId comes from the packets themselves, not from `hvcC`, which
//!   is container metadata a file may understate.
//!
//! Besides slices, such a unit may hold only access unit delimiters, filler
//! data, and SEI whose every message is local to its picture (buffering
//! period, picture timing, filler, user data, and HEVC's decoded picture
//! hash); a message that persists past its picture (film grain, mastering
//! display, frame packing, ...), a parameter set, an end of sequence or
//! anything unknown keeps the unit. VP8 and VP9 are not skipped: a frame
//! that refreshes no reference may still update entropy contexts,
//! loop-filter deltas or segmentation that later frames read.

use coreav_decode::{Codec, CodecConfig, ConfigKind};

/// How packets of a stream are read for disposability, from its
/// configuration record; `None` when they are not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    /// `avcC`: NAL units with `length` length bytes.
    Avc { length: usize },
    /// `hvcC`: NAL units with `length` length bytes.
    Hevc { length: usize },
}

/// SEI payload types local to their picture: buffering period (0), picture
/// timing (1), filler (3), user data registered (4) and unregistered (5).
const LOCAL_SEI: [u32; 5] = [0, 1, 3, 4, 5];
/// HEVC's decoded picture hash (132), in a suffix SEI.
const HEVC_PICTURE_HASH: u32 = 132;

impl Rule {
    /// The rule for a stream, if its packets can be judged.
    pub(crate) fn of(codec: Codec, config: Option<&CodecConfig>) -> Option<Self> {
        let config = config?;
        match (codec, config.kind) {
            (Codec::H264, ConfigKind::Avcc) => {
                // avcC byte 4: six reserved bits, lengthSizeMinusOne.
                let length = usize::from(config.data.get(4)? & 3) + 1;
                (length != 3).then_some(Self::Avc { length })
            }
            (Codec::Hevc, ConfigKind::Hvcc) => {
                // hvcC byte 21: constantFrameRate (2), numTemporalLayers
                // (3), temporalIdNested (1), lengthSizeMinusOne (2).
                let length = usize::from(config.data.get(21)? & 3) + 1;
                (length != 3).then_some(Self::Hevc { length })
            }
            _ => None,
        }
    }

    /// Whether the decision needs the highest TemporalId of the packets
    /// decoded ([`Rule::temporal_id`]) first.
    pub(crate) fn needs_temporal_ids(self) -> bool {
        matches!(self, Self::Hevc { .. })
    }

    /// The highest TemporalId of the slices of `packet` (HEVC), or `None`
    /// when a unit cannot be read.
    pub(crate) fn temporal_id(self, packet: &[u8]) -> Option<u8> {
        let mut top = 0u8;
        for nal in units(self.length(), packet)? {
            let (Some(&first), Some(&second)) = (nal.first(), nal.get(1)) else {
                return None;
            };
            // VCL units are types 0 to 31.
            if (first >> 1) & 0x3F < 32 {
                top = top.max((second & 7).saturating_sub(1));
            }
        }
        Some(top)
    }

    /// Bytes of each unit's length prefix.
    pub(crate) fn length(self) -> usize {
        match self {
            Self::Avc { length } | Self::Hevc { length } => length,
        }
    }

    /// Whether `packet` (one access unit) is disposable; `top` is the
    /// highest TemporalId of the packets decoded (HEVC; ignored for H.264).
    /// Anything it cannot read, such as a truncated length, keeps the
    /// packet.
    pub(crate) fn disposable(self, packet: &[u8], top: u8) -> bool {
        let Some(nals) = units(self.length(), packet) else {
            return false;
        };
        let mut slices = 0usize;
        for nal in nals {
            match self.unit(nal, top) {
                Unit::DisposableSlice => slices += 1,
                Unit::Neutral => {}
                Unit::Keep => return false,
            }
        }
        slices > 0
    }

    fn unit(self, nal: &[u8], top: u8) -> Unit {
        match self {
            Self::Avc { .. } => {
                let Some(&head) = nal.first() else {
                    return Unit::Keep;
                };
                let (reference, kind) = ((head >> 5) & 3, head & 0x1F);
                match kind {
                    // A non-IDR slice that is not a reference.
                    1 if reference == 0 => Unit::DisposableSlice,
                    6 if sei_is_local(nal.get(1..).unwrap_or(&[]), &[]) => Unit::Neutral,
                    // Access unit delimiter, filler data.
                    9 | 12 => Unit::Neutral,
                    _ => Unit::Keep,
                }
            }
            Self::Hevc { .. } => {
                let (Some(&first), Some(&second)) = (nal.first(), nal.get(1)) else {
                    return Unit::Keep;
                };
                let kind = (first >> 1) & 0x3F;
                let temporal = (second & 7).saturating_sub(1);
                let payload = nal.get(2..).unwrap_or(&[]);
                match kind {
                    // TRAIL_N, TSA_N, STSA_N, RADL_N, RASL_N, RSV_VCL_N10,
                    // _N12, _N14: sub-layer non-reference pictures, which
                    // only pictures of a higher TemporalId may reference.
                    0 | 2 | 4 | 6 | 8 | 10 | 12 | 14 if temporal >= top => Unit::DisposableSlice,
                    39 if sei_is_local(payload, &[]) => Unit::Neutral,
                    40 if sei_is_local(payload, &[HEVC_PICTURE_HASH]) => Unit::Neutral,
                    // Access unit delimiter, filler data.
                    35 | 38 => Unit::Neutral,
                    _ => Unit::Keep,
                }
            }
        }
    }
}

enum Unit {
    DisposableSlice,
    Neutral,
    Keep,
}

/// The NAL units of a length-prefixed access unit, or `None` when a length
/// cannot be read.
fn units(length: usize, packet: &[u8]) -> Option<Vec<&[u8]>> {
    let mut rest = packet;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (size, tail) = rest.split_at_checked(length)?;
        let size = size.iter().fold(0usize, |v, b| {
            v.saturating_mul(256).saturating_add(usize::from(*b))
        });
        let (nal, tail) = tail.split_at_checked(size)?;
        out.push(nal);
        rest = tail;
    }
    Some(out)
}

/// Whether every message of an SEI unit's payload (after the NAL header)
/// is local to its picture: [`LOCAL_SEI`] or one of `also`. Anything it
/// cannot read is not.
fn sei_is_local(payload: &[u8], also: &[u32]) -> bool {
    // The RBSP: emulation prevention bytes (0x03 after two zero bytes)
    // removed (ITU-T H.264 7.4.1, H.265 7.4.2).
    let mut rbsp = Vec::with_capacity(payload.len());
    let mut zeros = 0usize;
    for &b in payload {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        rbsp.push(b);
    }
    let mut rest = rbsp.as_slice();
    let mut messages = 0usize;
    // sei_message() until rbsp_trailing_bits (0x80 and zero bytes).
    while let Some(&first) = rest.first() {
        if first == 0x80 && rest.iter().skip(1).all(|b| *b == 0) {
            break;
        }
        let Some((kind, tail)) = read_value(rest) else {
            return false;
        };
        let Some((size, tail)) = read_value(tail) else {
            return false;
        };
        if !LOCAL_SEI.contains(&kind) && !also.contains(&kind) {
            return false;
        }
        let Some(tail) = usize::try_from(size).ok().and_then(|s| tail.get(s..)) else {
            return false;
        };
        rest = tail;
        messages += 1;
    }
    messages > 0
}

/// An SEI payload type or size: 0xFF bytes summed, then the last byte.
fn read_value(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let mut value = 0u32;
    let mut rest = bytes;
    loop {
        let (&b, tail) = rest.split_first()?;
        value = value.checked_add(u32::from(b))?;
        rest = tail;
        if b != 0xFF {
            return Some((value, rest));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(length: usize, nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for nal in nals {
            let size = u32::try_from(nal.len()).expect("size").to_be_bytes();
            out.extend_from_slice(&size[4 - length..]);
            out.extend_from_slice(nal);
        }
        out
    }

    #[test]
    fn avc() {
        let config = CodecConfig {
            kind: ConfigKind::Avcc,
            data: vec![1, 100, 0, 40, 0xFF, 0xE1],
        };
        let rule = Rule::of(Codec::H264, Some(&config)).expect("rule");
        assert_eq!(rule, Rule::Avc { length: 4 });
        // A non-reference slice with an AUD and a user data SEI (type 5,
        // 2 bytes, trailing bits).
        let sei: &[u8] = &[0x06, 5, 2, 0xAA, 0xBB, 0x80];
        assert!(rule.disposable(&pack(4, &[&[0x09, 0xF0], sei, &[0x01, 0x9A]]), 0));
        // A film grain SEI (type 19) persists: kept.
        let grain: &[u8] = &[0x06, 19, 1, 0x00, 0x80];
        assert!(!rule.disposable(&pack(4, &[grain, &[0x01, 0x9A]]), 0));
        // A malformed SEI: kept.
        assert!(!rule.disposable(&pack(4, &[&[0x06, 5, 9, 1], &[0x01, 0x9A]]), 0));
        // A reference slice, an IDR, a parameter set.
        assert!(!rule.disposable(&pack(4, &[&[0x41, 0x9A]]), 0));
        assert!(!rule.disposable(&pack(4, &[&[0x65, 0x88]]), 0));
        assert!(!rule.disposable(&pack(4, &[&[0x68, 0xEE], &[0x01, 0x9A]]), 0));
        // No slice, a truncated length, a NAL past the end.
        assert!(!rule.disposable(&pack(4, &[&[0x09, 0xF0]]), 0));
        assert!(!rule.disposable(&[0, 0, 0], 0));
        assert!(!rule.disposable(&[0, 0, 0, 9, 0x01], 0));
        assert!(!rule.disposable(&[], 0));
    }

    #[test]
    fn sei_messages() {
        // Two local messages: type 0 with payload 00 00 01 (escaped as
        // 00 00 03 01), then type 5 with one byte.
        assert!(sei_is_local(&[0, 3, 0, 0, 3, 1, 5, 1, 0xAA, 0x80], &[]));
        // Without the escape removed, the sizes would not add up.
        assert!(!sei_is_local(&[0, 4, 0, 0, 3, 1, 5, 1, 0xAA, 0x80], &[]));
        // A type above 255 (0xFF + 0x2A = 297) is not local.
        assert!(!sei_is_local(&[0xFF, 0x2A, 0, 0x80], &[]));
        assert!(sei_is_local(&[0x84, 0, 0x80], &[HEVC_PICTURE_HASH]));
        assert!(!sei_is_local(&[], &[]));
        assert!(!sei_is_local(&[0x80], &[]));
    }

    #[test]
    fn hevc() {
        let mut data = vec![0u8; 23];
        // One temporal layer declared, 4-byte lengths.
        data[21] = (1 << 3) | 3;
        let config = CodecConfig {
            kind: ConfigKind::Hvcc,
            data,
        };
        let rule = Rule::of(Codec::Hevc, Some(&config)).expect("rule");
        assert_eq!(rule, Rule::Hevc { length: 4 });
        assert!(rule.needs_temporal_ids());
        // TRAIL_N at TemporalId 0 with a prefix user data SEI; the highest
        // TemporalId decoded is 0.
        let sei: &[u8] = &[39 << 1, 1, 5, 1, 0xAA, 0x80];
        assert!(rule.disposable(&pack(4, &[sei, &[0, 1, 0xAF]]), 0));
        // TRAIL_R, a PPS, an IDR.
        assert!(!rule.disposable(&pack(4, &[&[1 << 1, 1, 0xAF]]), 0));
        assert!(!rule.disposable(&pack(4, &[&[34 << 1, 1], &[0, 1, 0xAF]]), 0));
        assert!(!rule.disposable(&pack(4, &[&[19 << 1, 1, 0xAF]]), 0));
        // Pictures of TemporalId 1 decoded: TRAIL_N at 0 may be referenced
        // by them and is kept, whatever hvcC says; at 1 it is disposable.
        assert!(!rule.disposable(&pack(4, &[&[0, 1, 0xAF]]), 1));
        assert!(rule.disposable(&pack(4, &[&[0, 2, 0xAF]]), 1));
        // The highest TemporalId of a packet, from its slices.
        assert_eq!(rule.temporal_id(&pack(4, &[sei, &[0, 2, 0xAF]])), Some(1));
        assert_eq!(rule.temporal_id(&[0, 0, 0, 9, 0]), None);
        // A suffix decoded picture hash is local.
        let hash: &[u8] = &[40 << 1, 1, 132, 1, 0, 0x80];
        assert!(rule.disposable(&pack(4, &[&[0, 1, 0xAF], hash]), 0));
    }

    #[test]
    fn other_codecs_have_no_rule() {
        assert_eq!(Rule::of(Codec::Vp9, None), None);
        assert_eq!(Rule::of(Codec::H264, None), None);
    }
}
