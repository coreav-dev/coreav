//! The Opus identification header (RFC 7845 5.1) and its channel mapping.

use crate::Error;

/// The identification header, from `OpusHead` (Matroska, Ogg) or `dOps`
/// (MP4); read only, its fields checked against each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    version: u8,
    channels: u8,
    pre_skip: u16,
    input_sample_rate: u32,
    output_gain: i16,
    family: u8,
    streams: u8,
    coupled: u8,
    mapping: [u8; 255],
}

/// Where an output channel's samples come from (RFC 7845 5.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Silence (mapping index 255).
    Silence,
    /// One channel of a stream decoded as stereo: the left (`right` false)
    /// or the right one.
    Coupled {
        /// The stream, below `coupled`.
        stream: u8,
        /// Whether the right channel.
        right: bool,
    },
    /// A stream decoded as mono.
    Mono {
        /// The stream, from `coupled` to `streams - 1`.
        stream: u8,
    },
}

/// The output channels in WAV's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Speaker positions, as WAV's `dwChannelMask`; 0 when the family
    /// gives the channels no positions.
    pub mask: u32,
    /// For WAV channel `i`, the Opus output channel at index `i`.
    order: Option<[u8; 8]>,
}

impl Layout {
    /// The Opus output channel (RFC 7845 order) that is WAV channel
    /// `channel`.
    pub fn opus_channel(&self, channel: usize) -> usize {
        self.order
            .and_then(|order| order.get(channel).copied())
            .map_or(channel, usize::from)
    }
}

impl Head {
    /// Reads the configuration as a container carries it: an `OpusHead`
    /// packet (Matroska's `CodecPrivate`, RFC 7845's ID header, fields
    /// little-endian) or the body of an MP4 `dOps` box (the same fields
    /// big-endian, with no magic).
    pub fn from_config(config: &[u8]) -> Result<Self, Error> {
        match config.strip_prefix(b"OpusHead") {
            Some(rest) => {
                let version = *rest.first().ok_or(Error::Invalid("OpusHead too short"))?;
                // RFC 7845 5.1: a major version other than 0 is
                // incompatible; later minor versions may add fields.
                if version > 15 {
                    return Err(Error::Unsupported("OpusHead version 16 or later"));
                }
                Self::read(rest, u16::from_le_bytes, u32::from_le_bytes)
            }
            None => {
                // The Opus-in-ISOBMFF mapping 4.3.2: without support of
                // another version, nothing after it is read.
                match config.first() {
                    Some(0) => Self::read(config, u16::from_be_bytes, u32::from_be_bytes),
                    Some(_) => Err(Error::Unsupported("dOps box version")),
                    None => Err(Error::Invalid("dOps box too short")),
                }
            }
        }
    }

    fn read(
        data: &[u8],
        u16_of: fn([u8; 2]) -> u16,
        u32_of: fn([u8; 4]) -> u32,
    ) -> Result<Self, Error> {
        // Version, channels, pre-skip, input rate, gain, family.
        let [
            version,
            channels,
            s0,
            s1,
            r0,
            r1,
            r2,
            r3,
            g0,
            g1,
            family,
            ref table @ ..,
        ] = *data
        else {
            return Err(Error::Invalid("Opus header too short"));
        };
        if channels == 0 {
            return Err(Error::Invalid("Opus header with no output channels"));
        }
        let mut head = Self {
            version,
            channels,
            pre_skip: u16_of([s0, s1]),
            input_sample_rate: u32_of([r0, r1, r2, r3]),
            output_gain: u16_of([g0, g1]) as i16,
            family,
            streams: 1,
            coupled: channels - 1,
            mapping: [255; 255],
        };
        let c = usize::from(channels);
        match family {
            0 => {
                // RFC 7845 5.1.1.1: one stream, no table.
                if channels > 2 {
                    return Err(Error::Invalid(
                        "Opus mapping family 0 with more than 2 channels",
                    ));
                }
                for (i, slot) in head.mapping.iter_mut().take(c).enumerate() {
                    *slot = i as u8;
                }
                return Ok(head);
            }
            1 if channels > 8 => {
                return Err(Error::Invalid(
                    "Opus mapping family 1 with more than 8 channels",
                ));
            }
            2 if !is_ambisonic(channels) => {
                return Err(Error::Invalid(
                    "Opus mapping family 2 with a channel count that is not ambisonic",
                ));
            }
            3 => {
                // RFC 8486 3.2: the table holds a demixing matrix.
                return Err(Error::Unsupported(
                    "Opus mapping family 3 (ambisonics with a demixing matrix)",
                ));
            }
            _ => {}
        }
        let Some((&streams, rest)) = table.split_first() else {
            return Err(Error::Invalid(
                "Opus header without its channel mapping table",
            ));
        };
        let Some((&coupled, rest)) = rest.split_first() else {
            return Err(Error::Invalid(
                "Opus header without its channel mapping table",
            ));
        };
        let mapping = rest
            .get(..c)
            .ok_or(Error::Invalid("Opus channel mapping table too short"))?;
        let decoded = usize::from(streams) + usize::from(coupled);
        if streams == 0 || coupled > streams || decoded > 255 {
            return Err(Error::Invalid("Opus stream counts out of range"));
        }
        if mapping
            .iter()
            .any(|&index| index != 255 && usize::from(index) >= decoded)
        {
            return Err(Error::Invalid(
                "Opus channel mapping past the decoded channels",
            ));
        }
        head.streams = streams;
        head.coupled = coupled;
        for (slot, &index) in head.mapping.iter_mut().zip(mapping) {
            *slot = index;
        }
        Ok(head)
    }

    /// `OpusHead`'s version, 0 to 15 (RFC 7845 5.1: a major version of 0);
    /// `dOps`'s, always 0.
    pub fn version(&self) -> u8 {
        self.version
    }

    /// Output channels `C`, 1 to 255.
    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Samples at 48 kHz to drop from the start of the decoded stream.
    /// Informative in MP4, where the edit list trims.
    pub fn pre_skip(&self) -> u16 {
        self.pre_skip
    }

    /// The encoder's input rate in Hz, 0 when not stated; metadata only.
    pub fn input_sample_rate(&self) -> u32 {
        self.input_sample_rate
    }

    /// Gain to apply on output, in dB, Q7.8.
    pub fn output_gain(&self) -> i16 {
        self.output_gain
    }

    /// Channel mapping family as coded: 0, 1, 2 (RFC 8486) and 255 are
    /// defined; the table of 4 to 254 is read as 255's, and their output
    /// has no speaker positions (RFC 7845 5.1.1.4).
    pub fn family(&self) -> u8 {
        self.family
    }

    /// Opus streams per packet `N`, at least 1.
    pub fn streams(&self) -> u8 {
        self.streams
    }

    /// Streams of them decoded as stereo `M`, the first `M`.
    pub fn coupled(&self) -> u8 {
        self.coupled
    }

    /// The channel mapping index of each output channel, `channels` of
    /// them.
    pub fn mapping(&self) -> &[u8] {
        self.mapping
            .get(..usize::from(self.channels))
            .unwrap_or_default()
    }

    /// Where output channel `channel` (RFC 7845 order) comes from; `None`
    /// past the last channel.
    pub fn source(&self, channel: usize) -> Option<Source> {
        let index = *self.mapping().get(channel)?;
        let coupled = self.coupled.saturating_mul(2);
        Some(if index == 255 {
            Source::Silence
        } else if index < coupled {
            Source::Coupled {
                stream: index / 2,
                right: index % 2 == 1,
            }
        } else {
            Source::Mono {
                stream: index - self.coupled,
            }
        })
    }

    /// The output in WAV's order: families 0 and 1 name speaker positions,
    /// family 1 in Vorbis's order (RFC 7845 5.1.1.2), which is moved to
    /// WAV's; other families keep the coded order with no positions.
    pub fn layout(&self) -> Layout {
        let positional = self.family == 0 || self.family == 1;
        if !positional {
            return Layout {
                mask: 0,
                order: None,
            };
        }
        // For each WAV channel, its Vorbis index: front left, front
        // right, front centre, LFE, back left, back right, back centre,
        // side left, side right, in WAV's order of positions.
        let order = match self.channels {
            3 => Some([0, 2, 1, 0, 0, 0, 0, 0]),
            5 => Some([0, 2, 1, 3, 4, 0, 0, 0]),
            6 => Some([0, 2, 1, 5, 3, 4, 0, 0]),
            7 => Some([0, 2, 1, 6, 5, 3, 4, 0]),
            8 => Some([0, 2, 1, 7, 5, 6, 3, 4]),
            _ => None,
        };
        // WAV's masks: FC; FL FR; FL FR FC; FL FR BL BR; FL FR FC BL BR;
        // the same with LFE; FL FR FC LFE BC SL SR; FL FR FC LFE BL BR SL SR.
        Layout {
            mask: match self.channels {
                1 => 0x4,
                2 => 0x3,
                3 => 0x7,
                4 => 0x33,
                5 => 0x37,
                6 => 0x3F,
                7 => 0x70F,
                _ => 0x63F,
            },
            order,
        }
    }
}

/// Whether `channels` is (1 + n)^2 or (1 + n)^2 + 2 for an order n of 0 to
/// 14 (RFC 8486 3.1).
fn is_ambisonic(channels: u8) -> bool {
    (1u16..=15).any(|n| {
        let square = n * n;
        u16::from(channels) == square || u16::from(channels) == square + 2
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opus_head(channels: u8, family: u8, table: &[u8]) -> Vec<u8> {
        let mut data = b"OpusHead".to_vec();
        data.extend([1, channels]);
        data.extend(312u16.to_le_bytes());
        data.extend(44_100u32.to_le_bytes());
        data.extend((-256i16).to_le_bytes());
        data.push(family);
        data.extend(table);
        data
    }

    #[test]
    fn opus_head_family_0() {
        let head = Head::from_config(&opus_head(2, 0, &[])).unwrap();
        assert_eq!(
            (head.channels(), head.pre_skip(), head.input_sample_rate()),
            (2, 312, 44_100)
        );
        assert_eq!(head.output_gain(), -256);
        assert_eq!((head.streams(), head.coupled()), (1, 1));
        assert_eq!(head.mapping(), [0, 1]);
        assert_eq!(
            head.source(1),
            Some(Source::Coupled {
                stream: 0,
                right: true
            })
        );
        assert_eq!(head.layout().mask, 0x3);
        let mono = Head::from_config(&opus_head(1, 0, &[])).unwrap();
        assert_eq!((mono.streams(), mono.coupled()), (1, 0));
        assert_eq!(mono.source(0), Some(Source::Mono { stream: 0 }));
        assert!(Head::from_config(&opus_head(3, 0, &[])).is_err());
        assert!(Head::from_config(&opus_head(0, 0, &[])).is_err());
    }

    #[test]
    fn opus_head_versions_and_lengths() {
        let mut data = opus_head(1, 0, &[]);
        data[8] = 15;
        assert!(Head::from_config(&data).is_ok());
        data[8] = 16;
        assert!(matches!(
            Head::from_config(&data),
            Err(Error::Unsupported(_))
        ));
        // A later minor version may add fields.
        let mut data = opus_head(1, 0, &[]);
        data.extend([1, 2, 3]);
        assert!(Head::from_config(&data).is_ok());
        let data = opus_head(1, 0, &[]);
        assert!(Head::from_config(&data[..data.len() - 1]).is_err());
    }

    #[test]
    fn dops_is_big_endian() {
        let mut data = vec![0, 2];
        data.extend(3840u16.to_be_bytes());
        data.extend(48_000u32.to_be_bytes());
        data.extend(0i16.to_be_bytes());
        data.push(0);
        let head = Head::from_config(&data).unwrap();
        assert_eq!((head.pre_skip(), head.input_sample_rate()), (3840, 48_000));
        data[0] = 1;
        assert!(matches!(
            Head::from_config(&data),
            Err(Error::Unsupported(_))
        ));
        assert!(Head::from_config(&[]).is_err());
    }

    #[test]
    fn family_1_surround() {
        // 5.1: two coupled streams (FL/FR, RL/RR), then C and LFE in mono;
        // the table puts them in Vorbis order.
        let head = Head::from_config(&opus_head(6, 1, &[4, 2, 0, 4, 1, 2, 3, 5])).unwrap();
        assert_eq!((head.streams(), head.coupled()), (4, 2));
        assert_eq!(head.source(1), Some(Source::Mono { stream: 2 }));
        assert_eq!(head.source(5), Some(Source::Mono { stream: 3 }));
        let layout = head.layout();
        assert_eq!(layout.mask, 0x3F);
        // WAV: FL FR FC LFE BL BR from Vorbis FL FC FR BL BR LFE.
        let order: Vec<usize> = (0..6).map(|i| layout.opus_channel(i)).collect();
        assert_eq!(order, [0, 2, 1, 5, 3, 4]);
        assert!(Head::from_config(&opus_head(9, 1, &[9, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8])).is_err());
    }

    #[test]
    fn mapping_table_checks() {
        // Silence is allowed; an index past M + N is not.
        let head = Head::from_config(&opus_head(2, 255, &[1, 0, 0, 255])).unwrap();
        assert_eq!(head.source(1), Some(Source::Silence));
        assert_eq!(
            head.layout(),
            Layout {
                mask: 0,
                order: None
            }
        );
        assert!(Head::from_config(&opus_head(2, 255, &[1, 0, 0, 1])).is_err());
        assert!(Head::from_config(&opus_head(1, 255, &[0, 0, 0])).is_err());
        assert!(Head::from_config(&opus_head(1, 255, &[1, 2, 0])).is_err());
        assert!(Head::from_config(&opus_head(2, 255, &[1, 0, 0])).is_err());
        assert!(Head::from_config(&opus_head(2, 1, &[])).is_err());
        // Reserved families read as 255.
        assert!(Head::from_config(&opus_head(1, 7, &[1, 0, 0])).is_ok());
        assert!(matches!(
            Head::from_config(&opus_head(4, 3, &[2, 2])),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn family_2_counts() {
        assert!(is_ambisonic(1) && is_ambisonic(4) && is_ambisonic(6) && is_ambisonic(227));
        assert!(!is_ambisonic(2) && !is_ambisonic(5) && !is_ambisonic(228));
        let head = Head::from_config(&opus_head(4, 2, &[4, 0, 0, 1, 2, 3])).unwrap();
        assert_eq!(head.layout().mask, 0);
        assert!(Head::from_config(&opus_head(5, 2, &[5, 0, 0, 1, 2, 3, 4])).is_err());
    }
}
