//! Stream descriptions and packets, and what selection reads from a codec
//! configuration record without decoding.

use coreav_frame::{Chroma, ColorInfo};

/// A video codec the boundary knows. Names follow `ffprobe`'s
/// `codec_name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// ITU-T H.264.
    H264,
    /// ITU-T H.265.
    Hevc,
    /// AV1.
    Av1,
    /// VP9.
    Vp9,
    /// VP8.
    Vp8,
    /// Apple ProRes (SMPTE RDD 36).
    ProRes,
    /// MPEG-4 Part 2 visual.
    Mpeg4,
    /// MPEG-1 video.
    Mpeg1Video,
    /// MPEG-2 video.
    Mpeg2Video,
    /// Motion JPEG.
    Mjpeg,
    /// Theora.
    Theora,
    /// FFV1.
    Ffv1,
}

impl Codec {
    /// Every codec the boundary knows.
    pub const ALL: [Codec; 12] = [
        Self::H264,
        Self::Hevc,
        Self::Av1,
        Self::Vp9,
        Self::Vp8,
        Self::ProRes,
        Self::Mpeg4,
        Self::Mpeg1Video,
        Self::Mpeg2Video,
        Self::Mjpeg,
        Self::Theora,
        Self::Ffv1,
    ];

    /// The codec's name, as `ffprobe` reports it.
    pub fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
            Self::Vp9 => "vp9",
            Self::Vp8 => "vp8",
            Self::ProRes => "prores",
            Self::Mpeg4 => "mpeg4",
            Self::Mpeg1Video => "mpeg1video",
            Self::Mpeg2Video => "mpeg2video",
            Self::Mjpeg => "mjpeg",
            Self::Theora => "theora",
            Self::Ffv1 => "ffv1",
        }
    }

    /// The codec named `name` ([`Codec::name`]).
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// How the bytes of a [`CodecConfig`] are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConfigKind {
    /// `AVCDecoderConfigurationRecord` (ISO/IEC 14496-15 5.3.3.1): the
    /// `avcC` payload, and Matroska's `CodecPrivate` for H.264.
    Avcc,
    /// `HEVCDecoderConfigurationRecord` (ISO/IEC 14496-15 8.3.3.1).
    Hvcc,
    /// `AV1CodecConfigurationRecord` (AV1 ISOBMFF binding 2.3.3).
    Av1c,
    /// The `vpcC` payload, version and flags included (VP codec ISOBMFF
    /// binding).
    Vpcc,
    /// The sample entry type of a ProRes stream (`apch`, `ap4h`, ...), as
    /// Matroska stores it in `CodecPrivate`.
    ProResFourcc,
    /// MPEG-4 Part 2 decoder specific information (the visual object
    /// sequence and object layer headers).
    Mpeg4Dsi,
    /// A Matroska `CodecPrivate` with no ISOBMFF counterpart (VP9 codec
    /// features, Theora headers).
    MatroskaPrivate,
}

/// A codec configuration as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecConfig {
    /// Layout of `data`.
    pub kind: ConfigKind,
    /// The bytes.
    pub data: Vec<u8>,
}

/// What a decoder needs to know about a stream before its first packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamConfig {
    /// The codec.
    pub codec: Codec,
    /// Its configuration, when the container stores one.
    pub config: Option<CodecConfig>,
    /// Coded width the container declares.
    pub width: Option<u32>,
    /// Coded height the container declares.
    pub height: Option<u32>,
    /// Colour description from the container or the parameter sets.
    pub color: ColorInfo,
}

impl StreamConfig {
    /// A stream of `codec` with nothing else known.
    pub fn new(codec: Codec) -> Self {
        Self {
            codec,
            config: None,
            width: None,
            height: None,
            color: ColorInfo::default(),
        }
    }
}

/// One compressed packet, in the container's own format: length-prefixed
/// NAL units for H.264 and HEVC (the length size from the configuration
/// record), OBUs with sizes for AV1, frames for the other codecs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet<'a> {
    /// The bytes.
    pub data: &'a [u8],
    /// Presentation timestamp, in the stream's time scale.
    pub pts: Option<i64>,
    /// Decode timestamp, in the stream's time scale.
    pub dts: Option<i64>,
    /// The container marks it as a random access point.
    pub keyframe: bool,
    /// The caller does not want this packet's frame: the decoder decodes
    /// the packet (later frames may reference it) but need not return its
    /// frame, which saves its transfer and copy.
    pub discard: bool,
}

/// What a configuration record says without decoding: what selection
/// reads (`docs/design/decode.md`, "Streams and packets"). `None` where the
/// record does not say or cannot be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamTraits {
    /// Bits per luma sample.
    pub bit_depth: Option<u8>,
    /// Chroma format.
    pub chroma: Option<Chroma>,
    /// The codec's profile code point (`profile_idc`, `seq_profile`,
    /// VP9's profile).
    pub profile: Option<u8>,
}

impl StreamConfig {
    /// Reads [`StreamTraits`] from the configuration record.
    pub fn traits(&self) -> StreamTraits {
        let Some(config) = &self.config else {
            return StreamTraits::default();
        };
        let d = config.data.as_slice();
        match config.kind {
            ConfigKind::Avcc => avcc_traits(d),
            ConfigKind::Hvcc => hvcc_traits(d),
            ConfigKind::Av1c => av1c_traits(d),
            ConfigKind::Vpcc => vpcc_traits(d),
            ConfigKind::ProResFourcc => prores_traits(d),
            ConfigKind::MatroskaPrivate if self.codec == Codec::Vp9 => vp9_private_traits(d),
            ConfigKind::Mpeg4Dsi | ConfigKind::MatroskaPrivate => StreamTraits::default(),
        }
    }
}

fn chroma_from_idc(idc: u32) -> Option<Chroma> {
    Some(match idc {
        0 => Chroma::Mono,
        1 => Chroma::Cs420,
        2 => Chroma::Cs422,
        3 => Chroma::Cs444,
        _ => return None,
    })
}

/// `hvcC`: `chromaFormat` in byte 16, `bitDepthLumaMinus8` in byte 17.
fn hvcc_traits(d: &[u8]) -> StreamTraits {
    let profile = d.get(1).map(|b| b & 0x1F);
    let chroma = d.get(16).and_then(|b| chroma_from_idc(u32::from(b & 3)));
    let bit_depth = d.get(17).map(|b| (b & 7) + 8);
    StreamTraits {
        bit_depth,
        chroma,
        profile,
    }
}

/// `av1C`: `seq_profile`, then `high_bitdepth`, `twelve_bit`,
/// `mono_chrome` and the subsampling flags in byte 2.
fn av1c_traits(d: &[u8]) -> StreamTraits {
    let (Some(b1), Some(b2)) = (d.get(1), d.get(2)) else {
        return StreamTraits::default();
    };
    let high = b2 & 0x40 != 0;
    let twelve = b2 & 0x20 != 0;
    let mono = b2 & 0x10 != 0;
    let (sx, sy) = (b2 & 0x08 != 0, b2 & 0x04 != 0);
    StreamTraits {
        bit_depth: Some(match (high, twelve) {
            (true, true) => 12,
            (true, false) => 10,
            _ => 8,
        }),
        chroma: Some(match (mono, sx, sy) {
            (true, _, _) => Chroma::Mono,
            (false, true, true) => Chroma::Cs420,
            (false, true, false) => Chroma::Cs422,
            (false, false, _) => Chroma::Cs444,
        }),
        profile: Some(b1 >> 5),
    }
}

/// `vpcC` version 1: version and flags, `profile`, `level`, then
/// `bitDepth` (4 bits) and `chromaSubsampling` (3 bits).
fn vpcc_traits(d: &[u8]) -> StreamTraits {
    let profile = d.get(4).copied();
    let packed = d.get(6).copied();
    StreamTraits {
        bit_depth: packed.map(|b| b >> 4).filter(|b| (8..=12).contains(b)),
        chroma: packed.and_then(|b| match (b >> 1) & 7 {
            0 | 1 => Some(Chroma::Cs420),
            2 => Some(Chroma::Cs422),
            3 => Some(Chroma::Cs444),
            _ => None,
        }),
        profile,
    }
}

/// Matroska's VP9 `CodecPrivate`: (ID, length, value) features; 1 is the
/// profile, 3 the bit depth, 4 the chroma subsampling.
fn vp9_private_traits(d: &[u8]) -> StreamTraits {
    let mut traits = StreamTraits::default();
    let mut rest = d;
    while let [id, len, tail @ ..] = rest {
        let n = usize::from(*len);
        let (Some(value), Some(next)) = (tail.get(..n), tail.get(n..)) else {
            break;
        };
        let v = value.first().copied();
        match id {
            1 => traits.profile = v,
            3 => traits.bit_depth = v.filter(|b| (8..=12).contains(b)),
            4 => {
                traits.chroma = v.and_then(|c| match c {
                    0 | 1 => Some(Chroma::Cs420),
                    2 => Some(Chroma::Cs422),
                    3 => Some(Chroma::Cs444),
                    _ => None,
                });
            }
            _ => {}
        }
        rest = next;
    }
    traits
}

/// ProRes: 4:2:2 at 10 bits, 4:4:4 at 12 bits for `ap4h` and `ap4x`
/// (SMPTE RDD 36).
fn prores_traits(d: &[u8]) -> StreamTraits {
    let four = matches!(d, [b'a', b'p', b'4', _]);
    StreamTraits {
        bit_depth: Some(if four { 12 } else { 10 }),
        chroma: Some(if four { Chroma::Cs444 } else { Chroma::Cs422 }),
        profile: None,
    }
}

/// `avcC`: the profile from byte 1; below the High profiles 8-bit 4:2:0,
/// else the first sequence parameter set's `chroma_format_idc` and
/// `bit_depth_luma_minus8` (ITU-T H.264 7.3.2.1.1).
fn avcc_traits(d: &[u8]) -> StreamTraits {
    let Some(&profile) = d.get(1) else {
        return StreamTraits::default();
    };
    let high = matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    );
    if !high {
        return StreamTraits {
            bit_depth: Some(8),
            chroma: Some(Chroma::Cs420),
            profile: Some(profile),
        };
    }
    let sps = d.get(5).filter(|n| *n & 0x1F > 0).and_then(|_| {
        let len = usize::from(u16::from_be_bytes([*d.get(6)?, *d.get(7)?]));
        d.get(8..8usize.checked_add(len)?)
    });
    let parsed = sps.and_then(sps_chroma_depth);
    StreamTraits {
        bit_depth: parsed.map(|(_, depth)| depth),
        chroma: parsed.and_then(|(idc, _)| chroma_from_idc(idc)),
        profile: Some(profile),
    }
}

/// `chroma_format_idc` and the luma bit depth of a High-profile sequence
/// parameter set NAL unit.
fn sps_chroma_depth(nal: &[u8]) -> Option<(u32, u8)> {
    // NAL header, then profile_idc, constraint flags, level_idc.
    let rbsp = unescape(nal.get(4..)?, 16);
    let mut bits = Bits::new(&rbsp);
    let _sps_id = bits.ue()?;
    let chroma_format_idc = bits.ue()?;
    if chroma_format_idc == 3 {
        bits.bit()?;
    }
    let depth = bits.ue()?;
    let depth = u8::try_from(depth.checked_add(8)?).ok()?;
    (depth <= 14).then_some((chroma_format_idc, depth))
}

/// The first `max` bytes of an RBSP: `emulation_prevention_three_byte`s
/// removed (ITU-T H.264 7.4.1).
fn unescape(bytes: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max);
    let mut zeros = 0;
    for &b in bytes {
        if out.len() >= max {
            break;
        }
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// A big-endian bit reader with Exp-Golomb codes.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Some(u32::from(bit))
    }

    /// `ue(v)`, at most 31 leading zeros.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let mut value = 0u64;
        for _ in 0..zeros {
            value = (value << 1) | u64::from(self.bit()?);
        }
        u32::try_from((1u64 << zeros) - 1 + value).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(kind: ConfigKind, data: &[u8], codec: Codec) -> StreamConfig {
        StreamConfig {
            config: Some(CodecConfig {
                kind,
                data: data.to_vec(),
            }),
            ..StreamConfig::new(codec)
        }
    }

    #[test]
    fn avcc_high10() {
        // avcC with one SPS: High 10 (110), chroma_format_idc 1, depth 10:
        // sps_id ue 0 ('1'), chroma_format_idc ue 1 ('010'),
        // bit_depth_luma_minus8 ue 2 ('011') -> bits 1010 011x.
        let sps = [0x67, 110, 0, 40, 0b1010_0110, 0x80];
        let mut record = vec![1, 110, 0, 40, 0xFF, 0xE1, 0, sps.len() as u8];
        record.extend_from_slice(&sps);
        record.push(0);
        let t = config(ConfigKind::Avcc, &record, Codec::H264).traits();
        assert_eq!(t.bit_depth, Some(10));
        assert_eq!(t.chroma, Some(Chroma::Cs420));
        assert_eq!(t.profile, Some(110));
        let main = config(ConfigKind::Avcc, &[1, 77, 0, 40], Codec::H264).traits();
        assert_eq!(main.bit_depth, Some(8));
    }

    #[test]
    fn hvcc_av1c_vpcc() {
        let mut hvcc = vec![0u8; 23];
        hvcc[1] = 2; // Main 10
        hvcc[16] = 0xFC | 1;
        hvcc[17] = 0xF8 | 2;
        let t = config(ConfigKind::Hvcc, &hvcc, Codec::Hevc).traits();
        assert_eq!(
            (t.bit_depth, t.chroma, t.profile),
            (Some(10), Some(Chroma::Cs420), Some(2))
        );
        let av1 = config(ConfigKind::Av1c, &[0x81, 0x08, 0x4C, 0], Codec::Av1).traits();
        assert_eq!((av1.bit_depth, av1.chroma), (Some(10), Some(Chroma::Cs420)));
        let vp9 = config(
            ConfigKind::Vpcc,
            &[1, 0, 0, 0, 2, 10, 0xA2, 1, 1, 1],
            Codec::Vp9,
        )
        .traits();
        assert_eq!((vp9.bit_depth, vp9.chroma), (Some(10), Some(Chroma::Cs420)));
        let mkv = config(
            ConfigKind::MatroskaPrivate,
            &[1, 1, 2, 3, 1, 10, 4, 1, 3],
            Codec::Vp9,
        )
        .traits();
        assert_eq!(
            (mkv.profile, mkv.bit_depth, mkv.chroma),
            (Some(2), Some(10), Some(Chroma::Cs444))
        );
    }

    #[test]
    fn truncated_records_say_nothing() {
        for kind in [
            ConfigKind::Avcc,
            ConfigKind::Hvcc,
            ConfigKind::Av1c,
            ConfigKind::Vpcc,
        ] {
            for len in 0..8 {
                let data = vec![0xFFu8; len];
                let _ = config(kind, &data, Codec::H264).traits();
            }
        }
        let t = config(
            ConfigKind::Avcc,
            &[1, 100, 0, 40, 0xFF, 0xE1, 0, 200],
            Codec::H264,
        )
        .traits();
        assert_eq!(t.bit_depth, None);
    }
}
