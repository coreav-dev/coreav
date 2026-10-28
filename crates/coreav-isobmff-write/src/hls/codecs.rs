//! RFC 6381 codec strings for the `CODECS` attribute (RFC 8216 4.3.4.2),
//! from the profile and level the parser reads from the stored codec
//! configuration.

use crate::m4a::unsupported;
use core::fmt::Write;
use coreav_isobmff::{Error, Profile, Track};

/// The codec string of a track the writer accepted: `avc1`, `hvc1`,
/// `av01`, `vp09` or `mp4a`.
pub(crate) fn codec_string(track: &Track) -> Result<String, Error> {
    let fourcc = &track.description.fourcc.0;
    let profile = track.description.profile.ok_or(unsupported(
        "the codec configuration has no readable profile",
    ))?;
    let mut out = String::new();
    let written = match (fourcc, profile) {
        // ISO/IEC 14496-15 Annex E.2: profile_idc, the constraint flags
        // byte and level_idc in hexadecimal.
        (
            b"avc1",
            Profile::H264 {
                profile_idc,
                constraint_flags,
                level_idc,
            },
        ) => write!(
            out,
            "avc1.{profile_idc:02X}{constraint_flags:02X}{level_idc:02X}"
        ),
        // Annex E.3: profile space and profile, the compatibility flags in
        // reverse bit order, tier and level, then the constraint bytes
        // with trailing zero bytes left out.
        (
            b"hvc1",
            Profile::Hevc {
                profile_space,
                high_tier,
                profile_idc,
                compatibility_flags,
                constraint_flags,
                level_idc,
            },
        ) => hevc(
            &mut out,
            profile_space,
            high_tier,
            profile_idc,
            compatibility_flags,
            constraint_flags,
            level_idc,
        ),
        // AV1 binding, "Codecs Parameter String": profile, two-digit level
        // and tier, two-digit bit depth.
        (
            b"av01",
            Profile::Av1 {
                profile,
                level_idx,
                high_tier,
                high_bitdepth,
                twelve_bit,
                ..
            },
        ) => {
            let depth = match (high_bitdepth, twelve_bit) {
                (false, _) => 8,
                (true, false) => 10,
                (true, true) => 12,
            };
            let tier = if high_tier { 'H' } else { 'M' };
            write!(out, "av01.{profile}.{level_idx:02}{tier}.{depth:02}")
        }
        // VP codec binding: profile, level and bit depth, two digits each.
        (
            b"vp09",
            Profile::Vp9 {
                profile,
                level,
                bit_depth,
            },
        ) => write!(out, "vp09.{profile:02}.{level:02}.{bit_depth:02}"),
        // RFC 6381 3.3: the object type indication (0x40, MPEG-4 audio) and
        // the audio object type; HE-AAC (5) or HE-AAC v2 (29) when the
        // configuration signals SBR or parametric stereo.
        (
            b"mp4a",
            Profile::Aac {
                object_type,
                sbr,
                ps,
            },
        ) => {
            let object_type = if ps {
                29
            } else if sbr {
                5
            } else {
                object_type
            };
            write!(out, "mp4a.40.{object_type}")
        }
        _ => return Err(unsupported("no codec string for this sample entry")),
    };
    written.map_err(|_| unsupported("no codec string for this sample entry"))?;
    Ok(out)
}

/// The channel count an AAC track's `AudioSpecificConfig` gives (ISO/IEC
/// 14496-3 1.6.3.5, table 1.19): 1 to 6 for channel configurations 1 to 6,
/// 8 for 7, and 2 when the configuration signals parametric stereo; `None`
/// when it defers to a program config element (0) or cannot be read.
pub(crate) fn aac_channels(track: &Track) -> Option<u16> {
    let esds = track.description.codec_config.as_ref()?;
    let config = decoder_specific_info(&esds.data)?;
    let mut bits = Bits {
        data: config,
        at: 0,
    };
    let object_type = bits.object_type()?;
    let frequency = bits.take(4)?;
    if frequency == 0xf {
        bits.take(24)?;
    }
    let channel_configuration = bits.take(4)?;
    // Explicit parametric stereo (14496-3 1.6.2.1), hierarchical or in the
    // backward-compatible extension the parser reads: a mono core decodes
    // to two channels, as CODECS says (`mp4a.40.29`).
    let ps = object_type == 29
        || matches!(
            track.description.profile,
            Some(Profile::Aac { ps: true, .. })
        );
    match (channel_configuration, ps) {
        (1, true) => Some(2),
        (1..=6, _) => u16::try_from(channel_configuration).ok(),
        (7, _) => Some(8),
        _ => None,
    }
}

/// The `DecoderSpecificInfo` payload of an `esds` box's payload (after
/// version and flags): ES_Descriptor (tag 3), DecoderConfigDescriptor (tag
/// 4), DecoderSpecificInfo (tag 5), each with its expandable size
/// (ISO/IEC 14496-1 7.2.2.3, 8.3.3).
fn decoder_specific_info(esds: &[u8]) -> Option<&[u8]> {
    let mut data = esds.get(4..)?;
    for tag in [3u8, 4, 5] {
        let (&found, rest) = data.split_first()?;
        if found != tag {
            return None;
        }
        let mut size = 0usize;
        let mut rest = rest;
        for _ in 0..4 {
            let (&byte, tail) = rest.split_first()?;
            size = (size << 7) | usize::from(byte & 0x7f);
            rest = tail;
            if byte & 0x80 == 0 {
                break;
            }
        }
        let body = rest.get(..size)?;
        data = match tag {
            // ES_ID, flags, then the optional fields the flags name.
            3 => {
                let flags = *body.get(2)?;
                let mut at = 3usize;
                if flags & 0x80 != 0 {
                    at += 2;
                }
                if flags & 0x40 != 0 {
                    at += 1 + usize::from(*body.get(at)?);
                }
                if flags & 0x20 != 0 {
                    at += 2;
                }
                body.get(at..)?
            }
            // objectTypeIndication, streamType and flags, buffer size, two
            // bit rates: 13 bytes.
            4 => body.get(13..)?,
            _ => body,
        };
    }
    Some(data)
}

/// A big-endian bit reader.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn take(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.at / 8)?;
            let bit = (byte >> (7 - self.at % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.at += 1;
        }
        Some(value)
    }

    /// audioObjectType, with the escape of 31 (14496-3 1.6.2.1).
    fn object_type(&mut self) -> Option<u32> {
        match self.take(5)? {
            31 => Some(32 + self.take(6)?),
            t => Some(t),
        }
    }
}

fn hevc(
    out: &mut String,
    profile_space: u8,
    high_tier: bool,
    profile_idc: u8,
    compatibility_flags: u32,
    constraint_flags: u64,
    level_idc: u8,
) -> core::fmt::Result {
    let space = match profile_space {
        1 => "A",
        2 => "B",
        3 => "C",
        _ => "",
    };
    let tier = if high_tier { 'H' } else { 'L' };
    write!(
        out,
        "hvc1.{space}{profile_idc}.{:X}.{tier}{level_idc}",
        compatibility_flags.reverse_bits()
    )?;
    let bytes = constraint_flags.to_be_bytes();
    let constraint = bytes.get(2..).unwrap_or_default();
    let kept = constraint
        .iter()
        .rposition(|b| *b != 0)
        .map_or(0, |last| last.saturating_add(1));
    for byte in constraint.iter().take(kept) {
        write!(out, ".{byte:02X}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_configurations() {
        // esds payload: version and flags, ES_Descriptor, DecoderConfig,
        // DecoderSpecificInfo `11 90` (AAC LC, 48 kHz, mono).
        let esds = |asc: &[u8]| {
            let mut dsi = vec![5, asc.len() as u8];
            dsi.extend_from_slice(asc);
            let mut dc = vec![4, (13 + dsi.len()) as u8, 0x40, 0x15];
            dc.extend_from_slice(&[0; 11]);
            dc.extend_from_slice(&dsi);
            let mut es = vec![3, (3 + dc.len()) as u8, 0, 1, 0];
            es.extend_from_slice(&dc);
            let mut out = vec![0; 4];
            out.extend_from_slice(&es);
            out
        };
        assert_eq!(
            decoder_specific_info(&esds(&[0x11, 0x90])),
            Some(&[0x11, 0x90][..])
        );
        let channels = |asc: &[u8]| {
            let config = esds(asc);
            let data = decoder_specific_info(&config).unwrap();
            let mut bits = Bits { data, at: 0 };
            bits.object_type().unwrap();
            bits.take(4).unwrap();
            bits.take(4).unwrap()
        };
        assert_eq!(channels(&[0x11, 0x90]), 2);
        assert_eq!(channels(&[0x11, 0x88]), 1);
        assert_eq!(channels(&[0x11, 0xb0]), 6);
    }

    #[test]
    fn hevc_strings() {
        // Main profile, level 3.1, compatible with profiles 1 and 2,
        // progressive and frame-only constraint flags.
        let mut out = String::new();
        hevc(&mut out, 0, false, 1, 0x6000_0000, 0x9000_0000_0000, 93).unwrap();
        assert_eq!(out, "hvc1.1.6.L93.90");
        let mut out = String::new();
        hevc(&mut out, 0, true, 2, 0x2000_0000, 0, 153).unwrap();
        assert_eq!(out, "hvc1.2.4.H153");
        let mut out = String::new();
        hevc(&mut out, 1, false, 1, 0x4000_0000, 0xb000_0000_0001, 120).unwrap();
        assert_eq!(out, "hvc1.A1.2.L120.B0.00.00.00.00.01");
    }
}
