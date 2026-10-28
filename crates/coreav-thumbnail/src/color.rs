//! The colour description the coded data signals, which wins over the
//! container's, as FFmpeg's decoders apply it (`docs/design/thumbnails.md`,
//! "Colour conversion"): the parameter set of the configuration record for
//! H.264, HEVC and AV1; the uncompressed header of a key frame for VP9;
//! BT.601 for VP8, by its specification; the frame header for ProRes.

use coreav_decode::{Codec, CodecConfig, ConfigKind};
use coreav_frame::ColorInfo;
use coreav_isobmff::{FourCc, parameter_set_color};

/// Bytes of the first keyframe that [`applied`] reads.
pub(crate) const HEADER_BYTES: usize = 64;

/// The colour description a decoder applies: from the coded data where
/// FFmpeg's decoders take it, else the container's (`container`), per codec
/// as `docs/design/probe-color.md` measured for H.264, HEVC and AV1:
/// - H.264: the parameter set's colour description when its video
///   usability information has one ("unspecified" included), else the
///   container's; its range when it signals one, else the container's;
/// - HEVC: always the parameter set's, a missing range limited (ITU-T
///   H.265 E.3.1); AV1: always the sequence header's;
/// - VP9: the key frame header's matrix and range over the container's;
///   VP8: BT.601, limited (RFC 6386 9.2); ProRes: the frame header's
///   colour code points over the container's.
pub(crate) fn applied(
    codec: Codec,
    config: Option<&CodecConfig>,
    keyframe: &[u8],
    container: ColorInfo,
) -> ColorInfo {
    match codec {
        Codec::H264 => match record(config) {
            Some(set) => {
                let described = set.primaries.is_some();
                let pick = |own: Option<u8>, other: Option<u8>| if described { own } else { other };
                ColorInfo {
                    primaries: pick(set.primaries, container.primaries),
                    transfer: pick(set.transfer, container.transfer),
                    matrix: pick(set.matrix, container.matrix),
                    full_range: set.full_range.or(container.full_range),
                    chroma_location: container.chroma_location,
                }
            }
            None => container,
        },
        Codec::Hevc => record(config).map_or(container, |set| ColorInfo {
            full_range: Some(set.full_range.unwrap_or(false)),
            chroma_location: container.chroma_location,
            ..set
        }),
        Codec::Av1 => record(config).map_or(container, |set| ColorInfo {
            chroma_location: container.chroma_location,
            ..set
        }),
        Codec::Vp9 => vp9(keyframe).unwrap_or_default().or(container),
        // RFC 6386 9.2: `color_space` 0 is the only one defined, BT.601.
        Codec::Vp8 => ColorInfo {
            matrix: Some(6),
            full_range: Some(false),
            ..ColorInfo::default()
        }
        .or(container),
        Codec::ProRes => prores(keyframe).unwrap_or_default().or(container),
        _ => container,
    }
}

/// The raw colour description of the record's parameter set.
fn record(config: Option<&CodecConfig>) -> Option<ColorInfo> {
    let config = config?;
    let kind = match config.kind {
        ConfigKind::Avcc => FourCc(*b"avcC"),
        ConfigKind::Hvcc => FourCc(*b"hvcC"),
        ConfigKind::Av1c => FourCc(*b"av1C"),
        _ => return None,
    };
    let c = parameter_set_color(kind, &config.data)?;
    let code = |v: Option<u16>| v.and_then(|v| u8::try_from(v).ok());
    Some(ColorInfo {
        primaries: code(c.primaries),
        transfer: code(c.transfer),
        matrix: code(c.matrix),
        full_range: c.full_range,
        chroma_location: None,
    })
}

/// A big-endian bit reader.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.data.get(self.pos / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - self.pos % 8)) & 1);
            self.pos += 1;
        }
        Some(v)
    }
}

/// `color_config()` of a VP9 key frame's uncompressed header (VP9
/// bitstream specification 6.2 and 7.2): `color_space` as ITU-T H.273
/// matrix coefficients, and `color_range`.
fn vp9(frame: &[u8]) -> Option<ColorInfo> {
    let mut b = Bits {
        data: frame,
        pos: 0,
    };
    if b.read(2)? != 2 {
        return None;
    }
    let profile = b.read(1)? | (b.read(1)? << 1);
    if profile == 3 {
        b.read(1)?;
    }
    if b.read(1)? == 1 {
        return None; // show_existing_frame
    }
    let key = b.read(1)? == 0;
    b.read(2)?; // show_frame, error_resilient_mode
    if !key || b.read(24)? != 0x49_8342 {
        return None;
    }
    if profile >= 2 {
        b.read(1)?;
    }
    let space = b.read(3)?;
    let matrix = match space {
        1 | 3 => Some(6),
        2 => Some(1),
        4 => Some(7),
        5 => Some(9),
        7 => Some(0),
        _ => None,
    };
    let full_range = if space == 7 {
        Some(true)
    } else {
        Some(b.read(1)? == 1)
    };
    Some(ColorInfo {
        matrix,
        full_range,
        ..ColorInfo::default()
    })
}

/// The colour code points of a ProRes frame header (SMPTE RDD 36 5.1):
/// after the frame size and `icpf`, bytes 22 to 24 of the frame. 0 and 2
/// (unspecified) say nothing.
fn prores(frame: &[u8]) -> Option<ColorInfo> {
    if frame.get(4..8)? != b"icpf" {
        return None;
    }
    let code = |at: usize| frame.get(at).copied().filter(|v| *v != 0 && *v != 2);
    Some(ColorInfo {
        primaries: code(22),
        transfer: code(23),
        matrix: code(24),
        ..ColorInfo::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vp9_key_frame_header() {
        // frame_marker 2, profile 0, show_existing 0, key frame, show_frame
        // 1, error_resilient 0, sync code, color_space 2 (BT.709), range 0.
        let bits = "10 0 0 0 0 1 0 010010011000001101000010 010 0";
        let bits: String = bits.chars().filter(|c| !c.is_whitespace()).collect();
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, c) in bits.chars().enumerate() {
            if c == '1' {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let color = vp9(&bytes).expect("header");
        assert_eq!((color.matrix, color.full_range), (Some(1), Some(false)));
        assert_eq!(vp9(&[0xFF; 8]), None);
        assert_eq!(vp9(&[]), None);
    }

    #[test]
    fn prores_frame_header() {
        let mut frame = vec![0u8; 32];
        frame[4..8].copy_from_slice(b"icpf");
        frame[22] = 1;
        frame[23] = 2;
        frame[24] = 1;
        let color = prores(&frame).expect("header");
        assert_eq!(
            (color.primaries, color.transfer, color.matrix),
            (Some(1), None, Some(1))
        );
        assert_eq!(prores(b"short"), None);
    }
}
