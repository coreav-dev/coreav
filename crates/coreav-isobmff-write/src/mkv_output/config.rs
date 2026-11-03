use super::{Error, Media, Track, ns_u, overflow, unsupported};
use coreav_mkv::write::CodecId;

pub(super) fn iso_codec(codec: coreav_isobmff::Codec) -> Result<CodecId, Error> {
    use coreav_isobmff::Codec;
    Ok(match codec {
        Codec::H264 => CodecId::H264,
        Codec::Hevc => CodecId::Hevc,
        Codec::Av1 => CodecId::Av1,
        Codec::Vp9 => CodecId::Vp9,
        Codec::Aac => CodecId::Aac,
        Codec::Opus => CodecId::Opus,
        Codec::Mp3 => CodecId::Mp3,
        Codec::Flac => CodecId::Flac,
        Codec::Ac3 => CodecId::Ac3,
        Codec::Eac3 => CodecId::Eac3,
        Codec::Alac => CodecId::Alac,
        _ => return Err(unsupported("an ISO codec without a Matroska mapping")),
    })
}

pub(super) fn codec_id(id: &str) -> Result<CodecId, Error> {
    Ok(match id {
        "V_MPEG4/ISO/AVC" => CodecId::H264,
        "V_MPEGH/ISO/HEVC" => CodecId::Hevc,
        "V_AV1" => CodecId::Av1,
        "V_VP8" => CodecId::Vp8,
        "V_VP9" => CodecId::Vp9,
        "A_AAC" => CodecId::Aac,
        "A_OPUS" => CodecId::Opus,
        "A_MPEG/L3" => CodecId::Mp3,
        "A_VORBIS" => CodecId::Vorbis,
        "A_FLAC" => CodecId::Flac,
        "A_AC3" => CodecId::Ac3,
        "A_EAC3" => CodecId::Eac3,
        "A_ALAC" => CodecId::Alac,
        "A_PCM/INT/LIT" => CodecId::PcmS16le,
        _ => return Err(unsupported("a Matroska codec without an output mapping")),
    })
}

pub(super) fn description(
    codec: CodecId,
    size: Option<(u32, u32)>,
    aspect: Option<(u32, u32)>,
    rate: Option<u32>,
    channels: Option<u32>,
    bits: Option<u32>,
    config: &[u8],
) -> Result<Track, Error> {
    let media = if let Some((width, height)) = size {
        Media::Video {
            width,
            height,
            colour: coreav_isobmff::Color::default(),
            interlaced: 0,
            field_order: 2,
            display: aspect
                .filter(|(a, b)| a != b)
                .map(|(a, b)| {
                    Ok::<_, Error>((
                        width.checked_mul(a).ok_or_else(overflow)?,
                        height.checked_mul(b).ok_or_else(overflow)?,
                    ))
                })
                .transpose()?,
        }
    } else {
        Media::Audio {
            sample_rate: rate.unwrap_or(0),
            channels: channels.unwrap_or(0),
            bit_depth: bits,
        }
    };
    let private = match codec {
        CodecId::Aac => coreav_isobmff::esds_decoder_specific_info(config)
            .and_then(|(_, data)| data)
            .ok_or_else(|| unsupported("AAC without AudioSpecificConfig"))?
            .to_vec(),
        CodecId::Opus => opus_head(config)?,
        CodecId::Flac => {
            let mut p = b"fLaC".to_vec();
            p.extend_from_slice(
                config
                    .get(4..)
                    .ok_or_else(|| unsupported("invalid FLAC configuration"))?,
            );
            p
        }
        CodecId::Alac => config
            .get(4..)
            .ok_or_else(|| unsupported("invalid ALAC configuration"))?
            .to_vec(),
        CodecId::Mp3 | CodecId::Ac3 | CodecId::Eac3 | CodecId::Vp9 | CodecId::PcmS16le => {
            Vec::new()
        }
        _ => config.to_vec(),
    };
    Ok(Track {
        codec,
        private,
        media,
        default_duration_ns: None,
        codec_delay_ns: 0,
        seek_pre_roll_ns: if codec == CodecId::Opus {
            80_000_000
        } else {
            0
        },
        language: "und".into(),
        name: None,
        default: true,
    })
}

pub(super) fn set_delay(header: &mut Track, trim: u64, scale: u32) -> Result<(), Error> {
    if scale == 0 {
        return Err(unsupported("zero audio timescale"));
    }
    header.codec_delay_ns = if header.codec == CodecId::Opus {
        let samples = trim.checked_mul(48_000).ok_or_else(overflow)?;
        if samples % u64::from(scale) != 0 {
            return Err(unsupported("Opus trim is not a whole 48 kHz sample"));
        }
        let pre_skip = u16::try_from(samples / u64::from(scale))
            .map_err(|_| unsupported("Opus trim exceeds its pre-skip field"))?;
        header
            .private
            .get_mut(10..12)
            .ok_or_else(|| unsupported("invalid OpusHead"))?
            .copy_from_slice(&pre_skip.to_le_bytes());
        ns_u(u64::from(pre_skip), 48_000)?
    } else {
        ns_u(trim, u64::from(scale))?
    };
    Ok(())
}

pub(super) fn opus_duration(header: &[u8]) -> Result<u64, Error> {
    let toc = *header
        .first()
        .ok_or_else(|| unsupported("empty Opus packet"))?;
    let config = toc >> 3;
    let per_frame: u32 = match config {
        0..=11 => match config & 3 {
            0 => 480,
            1 => 960,
            2 => 1920,
            _ => 2880,
        },
        12..=15 => 480 << (config & 1),
        _ => 120 << (config & 3),
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u32::from(
            *header
                .get(1)
                .ok_or_else(|| unsupported("truncated Opus packet"))?
                & 63,
        ),
    };
    let samples = per_frame
        .checked_mul(frames)
        .filter(|n| (1..=5760).contains(n))
        .ok_or_else(|| unsupported("invalid Opus packet duration"))?;
    ns_u(u64::from(samples), 48_000)
}

pub(super) fn entry(entry: &[u8], audio: bool) -> Result<Track, Error> {
    let codec = match entry.get(4..8) {
        Some(b"avc1" | b"avc3") => CodecId::H264,
        Some(b"hvc1" | b"hev1") => CodecId::Hevc,
        Some(b"av01") => CodecId::Av1,
        Some(b"vp09") => CodecId::Vp9,
        Some(b"mp4a") => CodecId::Aac,
        Some(b"Opus") => CodecId::Opus,
        _ => return Err(unsupported("unmapped replacement entry")),
    };
    let mut at = if audio { 36usize } else { 86usize };
    let mut config = &[][..];
    let mut aspect = None;
    let mut colour = None;
    while at < entry.len() {
        let size = be32(entry, at)? as usize;
        if size < 8 {
            return Err(unsupported("invalid replacement entry child"));
        }
        let end = at.checked_add(size).ok_or_else(overflow)?;
        let payload = entry
            .get(at + 8..end)
            .ok_or_else(|| unsupported("truncated replacement configuration"))?;
        match entry.get(at + 4..at + 8) {
            Some(b"avcC" | b"hvcC" | b"av1C" | b"vpcC" | b"esds" | b"dOps") => config = payload,
            Some(b"pasp") => aspect = Some((be32(payload, 0)?, be32(payload, 4)?)),
            Some(b"colr") => {
                if audio {
                    return Err(unsupported("colour on an audio replacement"));
                }
                let nclx = match payload.get(..4) {
                    Some(b"nclx") => true,
                    Some(b"nclc") => false,
                    _ => return Err(unsupported("unmapped replacement colour description")),
                };
                colour = Some(coreav_isobmff::Color {
                    primaries: Some(be16(payload, 4)?),
                    transfer: Some(be16(payload, 6)?),
                    matrix: Some(be16(payload, 8)?),
                    full_range: if nclx {
                        Some(*payload.get(10).ok_or_else(overflow)? & 0x80 != 0)
                    } else {
                        None
                    },
                });
            }
            _ => {}
        }
        at = end;
    }
    let mut header = description(
        codec,
        if audio {
            None
        } else {
            Some((u32::from(be16(entry, 32)?), u32::from(be16(entry, 34)?)))
        },
        aspect,
        if audio {
            Some(be32(entry, 32)? >> 16)
        } else {
            None
        },
        if audio {
            Some(u32::from(be16(entry, 24)?))
        } else {
            None
        },
        None,
        config,
    )?;
    if let Media::Video { colour: c, .. } = &mut header.media {
        let signalled = match codec {
            CodecId::H264 => {
                coreav_isobmff::parameter_set_color(coreav_isobmff::FourCc(*b"avcC"), config).map(
                    |mut color| {
                        color.full_range = Some(color.full_range.unwrap_or(false));
                        color
                    },
                )
            }
            _ => None,
        };
        *c = colour.or(signalled).unwrap_or_default();
        c.full_range = c.full_range.or(signalled.and_then(|v| v.full_range));
    }
    Ok(header)
}

fn opus_head(config: &[u8]) -> Result<Vec<u8>, Error> {
    if config.first() != Some(&0) || config.len() < 11 {
        return Err(unsupported("invalid dOps configuration"));
    }
    let mut p = b"OpusHead".to_vec();
    p.extend([1, *config.get(1).ok_or_else(overflow)?]);
    p.extend(be16(config, 2)?.to_le_bytes());
    p.extend(be32(config, 4)?.to_le_bytes());
    p.extend(be16(config, 8)?.to_le_bytes());
    p.extend_from_slice(config.get(10..).ok_or_else(overflow)?);
    Ok(p)
}
fn be16(b: &[u8], at: usize) -> Result<u16, Error> {
    Ok(u16::from_be_bytes(
        b.get(at..at + 2)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(overflow)?,
    ))
}
fn be32(b: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_be_bytes(
        b.get(at..at + 4)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(overflow)?,
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use coreav_isobmff::Color;

    fn colour(config: &[u8], declared: Option<crate::mux::Nclx>) -> Color {
        let bytes = crate::mux::avc1_sample_entry(320, 240, config, None, declared).unwrap();
        match entry(&bytes, false).unwrap().media {
            Media::Video { colour, .. } => colour,
            _ => panic!("video replacement"),
        }
    }

    #[test]
    fn replacement_colour_uses_its_own_readable_configuration_and_container_override() {
        let absent = [
            1, 66, 0, 30, 255, 225, 0, 8, 103, 66, 0, 30, 244, 10, 15, 200,
        ];
        let limited = [
            1, 66, 0, 30, 255, 225, 0, 13, 103, 66, 0, 30, 244, 10, 15, 211, 80, 16, 16, 16, 32,
        ];
        let full = [
            1, 66, 0, 30, 255, 225, 0, 13, 103, 66, 0, 30, 244, 10, 15, 211, 112, 16, 16, 16, 32,
        ];
        assert_eq!(
            colour(&absent, None),
            Color {
                full_range: Some(false),
                ..Color::default()
            }
        );
        for (config, full_range) in [(&limited, false), (&full, true)] {
            assert_eq!(
                colour(config, None),
                Color {
                    primaries: Some(1),
                    transfer: Some(1),
                    matrix: Some(1),
                    full_range: Some(full_range),
                }
            );
        }
        for config in [&absent[..12], &[1, 66, 0, 30, 255, 224, 0][..]] {
            assert_eq!(colour(config, None), Color::default());
        }
        assert_eq!(
            colour(
                &limited,
                Some(crate::mux::Nclx {
                    primaries: 9,
                    transfer: 16,
                    matrix: 1,
                    full_range: true,
                })
            ),
            Color {
                primaries: Some(9),
                transfer: Some(16),
                matrix: Some(1),
                full_range: Some(true)
            }
        );
        let mut nclc = crate::mux::avc1_sample_entry(
            320,
            240,
            &full,
            None,
            Some(crate::mux::Nclx {
                primaries: 9,
                transfer: 16,
                matrix: 1,
                full_range: false,
            }),
        )
        .unwrap();
        let at = nclc.windows(4).position(|v| v == b"nclx").unwrap();
        nclc[at..at + 4].copy_from_slice(b"nclc");
        nclc.pop();
        nclc[at - 8..at - 4].copy_from_slice(&18u32.to_be_bytes());
        let size = nclc.len() as u32;
        nclc[..4].copy_from_slice(&size.to_be_bytes());
        assert!(matches!(
            entry(&nclc, false).unwrap().media,
            Media::Video {
                colour: Color {
                    primaries: Some(9),
                    transfer: Some(16),
                    matrix: Some(1),
                    full_range: Some(true),
                },
                ..
            }
        ));
    }
}
