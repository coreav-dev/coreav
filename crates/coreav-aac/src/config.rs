//! The decoder configuration: `AudioSpecificConfig` (ISO/IEC 14496-3
//! 1.6.2.1), `GASpecificConfig` (4.4.1) and `program_config_element`
//! (4.4.1.1), and the channel layout they give.

use crate::bits::Bits;
use crate::tables::SAMPLE_RATES;
use crate::{Error, Limits};

/// The MPEG-4 audio object type of AAC LC.
const AOT_LC: u32 = 2;

/// WAV speaker positions (`dwChannelMask` bits).
const FL: u32 = 0x1;
const FR: u32 = 0x2;
const FC: u32 = 0x4;
const LFE: u32 = 0x8;
const BL: u32 = 0x10;
const BR: u32 = 0x20;
const FLC: u32 = 0x40;
const FRC: u32 = 0x80;
const BC: u32 = 0x100;

/// A decoder configuration: AAC LC, its rate, frame length and channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    sample_rate: u32,
    /// Index of the band tables (Table 4.82): the index in `SAMPLE_RATES`
    /// of the rate whose tables this rate uses, 0 to 11.
    pub(crate) rate_index: usize,
    frame_length: usize,
    channel_configuration: u8,
    pub(crate) layout: Layout,
}

/// Where the elements of a frame go: for each SCE, CPE and LFE of the
/// configuration, in the configuration's order, the output channels it
/// writes and the `element_instance_tag` a program configuration names
/// for it (channel configurations name none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) channels: usize,
    pub(crate) mask: u32,
    pub(crate) sce: Vec<usize>,
    pub(crate) cpe: Vec<[usize; 2]>,
    pub(crate) lfe: Vec<usize>,
    pub(crate) sce_tags: Vec<Option<u8>>,
    pub(crate) cpe_tags: Vec<Option<u8>>,
    pub(crate) lfe_tags: Vec<Option<u8>>,
}

/// An element of a channel layout: a single channel, a pair or an LFE,
/// with its WAV positions (0 where it has none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Single(u32),
    Pair(u32, u32),
    Lfe(u32),
}

/// The elements of channel configurations 1 to 7 (Table 1.19), in their
/// order in a frame. Configuration 7's second pair is the outside front
/// pair, written as WAV's front left and right of centre.
fn configuration_slots(configuration: u8) -> Option<&'static [Slot]> {
    use Slot::{Lfe, Pair, Single};
    Some(match configuration {
        1 => &[Single(FC)],
        2 => &[Pair(FL, FR)],
        3 => &[Single(FC), Pair(FL, FR)],
        4 => &[Single(FC), Pair(FL, FR), Single(BC)],
        5 => &[Single(FC), Pair(FL, FR), Pair(BL, BR)],
        6 => &[Single(FC), Pair(FL, FR), Pair(BL, BR), Lfe(LFE)],
        7 => &[
            Single(FC),
            Pair(FL, FR),
            Pair(FLC, FRC),
            Pair(BL, BR),
            Lfe(LFE),
        ],
        _ => return None,
    })
}

impl Layout {
    /// The layout of `slots`. With `wav` the slots carry WAV positions,
    /// all distinct, and the channels go in WAV's order (by position bit)
    /// with the mask of their positions; without, they go in the slots'
    /// order and the mask is 0. `tags`, when not empty, gives each slot's
    /// tag.
    fn new(slots: &[Slot], tags: &[u8], wav: bool) -> Self {
        let mut positions = Vec::new();
        for slot in slots {
            match *slot {
                Slot::Single(p) | Slot::Lfe(p) => positions.push(p),
                Slot::Pair(l, r) => {
                    positions.push(l);
                    positions.push(r);
                }
            }
        }
        let channels = positions.len();
        // Output channel of the n-th coded channel.
        let mut order: Vec<usize> = (0..channels).collect();
        if wav {
            let mut by_position: Vec<usize> = (0..channels).collect();
            by_position.sort_by_key(|&i| positions.get(i).copied().unwrap_or(0));
            for (out, &coded) in by_position.iter().enumerate() {
                if let Some(slot) = order.get_mut(coded) {
                    *slot = out;
                }
            }
        }
        let mut layout = Self {
            channels,
            mask: if wav {
                positions.iter().fold(0, |m, p| m | p)
            } else {
                0
            },
            ..Self::default()
        };
        let mut coded = 0;
        let tag = |i: usize| tags.get(i).copied();
        let mut next = || {
            let out = order.get(coded).copied().unwrap_or(0);
            coded += 1;
            out
        };
        for (i, slot) in slots.iter().enumerate() {
            match slot {
                Slot::Single(_) => {
                    layout.sce.push(next());
                    layout.sce_tags.push(tag(i));
                }
                Slot::Pair(..) => {
                    let l = next();
                    let r = next();
                    layout.cpe.push([l, r]);
                    layout.cpe_tags.push(tag(i));
                }
                Slot::Lfe(_) => {
                    layout.lfe.push(next());
                    layout.lfe_tags.push(tag(i));
                }
            }
        }
        layout
    }
}

/// A `program_config_element`'s channel elements, by position: whether
/// each is a pair, and its tag.
#[derive(Default)]
struct Program {
    front: Vec<(bool, u8)>,
    side: Vec<(bool, u8)>,
    back: Vec<(bool, u8)>,
    lfe: Vec<u8>,
}

impl Program {
    /// Reads a `program_config_element` (Table 4.2); `origin` is the bit
    /// its byte alignment counts from.
    fn read(bits: &mut Bits<'_>, origin: usize) -> Self {
        bits.skip(4 + 2 + 4); // element_instance_tag, object_type, sampling_frequency_index
        let front = bits.read(4) as usize;
        let side = bits.read(4) as usize;
        let back = bits.read(4) as usize;
        let lfe = bits.read(2) as usize;
        let assoc = bits.read(3) as usize;
        let cc = bits.read(4) as usize;
        for _ in 0..2 {
            // mono_mixdown_present, stereo_mixdown_present
            if bits.bit() {
                bits.skip(4);
            }
        }
        if bits.bit() {
            bits.skip(3); // matrix_mixdown_idx, pseudo_surround_enable
        }
        let mut list = |n: usize| {
            (0..n)
                .map(|_| {
                    let is_cpe = bits.bit();
                    (is_cpe, bits.read(4) as u8)
                })
                .collect::<Vec<(bool, u8)>>()
        };
        let front = list(front);
        let side = list(side);
        let back = list(back);
        let lfe = (0..lfe).map(|_| bits.read(4) as u8).collect();
        bits.skip(4 * assoc + 5 * cc);
        bits.align(origin);
        let comment = bits.read(8) as usize;
        bits.skip(8 * comment);
        Self {
            front,
            side,
            back,
            lfe,
        }
    }

    /// The layout: that of a channel configuration when the elements are
    /// that configuration's, else front, side, back and LFE elements in
    /// that order, with no WAV positions.
    fn layout(&self) -> Layout {
        let mut slots = Vec::new();
        let mut shape = Vec::new();
        let mut tags = Vec::new();
        for (group, list) in [&self.front, &self.side, &self.back]
            .into_iter()
            .enumerate()
        {
            for &(is_cpe, tag) in list {
                slots.push(if is_cpe {
                    Slot::Pair(0, 0)
                } else {
                    Slot::Single(0)
                });
                shape.push((group, is_cpe));
                tags.push(tag);
            }
        }
        for &tag in &self.lfe {
            slots.push(Slot::Lfe(0));
            shape.push((3, false));
            tags.push(tag);
        }
        for configuration in 1..=7 {
            let Some(known) = configuration_slots(configuration) else {
                continue;
            };
            if known_shape(configuration) == shape {
                return Layout::new(known, &tags, true);
            }
        }
        Layout::new(&slots, &tags, false)
    }
}

/// Reads past a `program_config_element` in a frame (its byte alignment
/// counts from the frame's start). Its counts are not used: the stream's
/// configuration stays the one it started with.
pub(crate) fn skip_program(bits: &mut Bits<'_>) {
    let _ = Program::read(bits, 0);
}

/// The program shape of configuration `c`: (group, is a pair) per element,
/// groups 0 front, 2 back, 3 LFE.
fn known_shape(c: u8) -> Vec<(usize, bool)> {
    let front: &[bool] = match c {
        1 => &[false],
        2 => &[true],
        7 => &[false, true, true],
        _ => &[false, true],
    };
    let back: &[bool] = match c {
        4 => &[false],
        5..=7 => &[true],
        _ => &[],
    };
    let mut shape: Vec<(usize, bool)> = front.iter().map(|&p| (0, p)).collect();
    shape.extend(back.iter().map(|&p| (2, p)));
    if c >= 6 {
        shape.push((3, false));
    }
    shape
}

/// Reads an audio object type (Table 1.16).
fn object_type(bits: &mut Bits<'_>) -> u32 {
    match bits.read(5) {
        31 => 32 + bits.read(6),
        aot => aot,
    }
}

/// Reads a sampling frequency index and, for the escape, the frequency.
fn sample_rate(bits: &mut Bits<'_>) -> Result<u32, Error> {
    let index = bits.read(4) as usize;
    if index == 0xf {
        return Ok(bits.read(24));
    }
    SAMPLE_RATES
        .get(index)
        .copied()
        .ok_or(Error::Invalid("reserved sampling frequency index"))
}

/// The band table index of a rate (Table 4.82).
fn rate_index(rate: u32) -> usize {
    const BOUNDS: [u32; 11] = [
        92017, 75132, 55426, 46009, 37566, 27713, 23004, 18783, 13856, 11502, 9391,
    ];
    BOUNDS.iter().take_while(|&&b| rate < b).count()
}

/// The name of an object type CoreAV refuses.
fn refused(aot: u32) -> &'static str {
    match aot {
        1 => "AAC Main",
        3 => "AAC SSR",
        4 => "AAC LTP",
        5 | 29 => "HE-AAC (SBR)",
        6 => "AAC scalable",
        17 => "ER AAC LC",
        19 => "ER AAC LTP",
        20 => "ER AAC scalable",
        23 => "ER AAC LD",
        39 => "ER AAC ELD",
        42 => "USAC",
        _ => "a non-AAC MPEG-4 audio object type",
    }
}

impl Config {
    /// Parses an `AudioSpecificConfig`, as MP4's `DecoderSpecificInfo`
    /// and Matroska's `CodecPrivate` carry it. AAC LC only: other object
    /// types, and SBR or PS signalled explicitly (object types 5 and 29,
    /// or the backward-compatible extension with `sbrPresentFlag` set),
    /// are [`Error::Unsupported`] with their name.
    pub fn parse(data: &[u8], limits: &Limits) -> Result<Self, Error> {
        let mut bits = Bits::new(data);
        let mut aot = object_type(&mut bits);
        let rate = sample_rate(&mut bits)?;
        let channel_configuration = bits.read(4) as u8;
        if aot == 5 || aot == 29 {
            return Err(Error::Unsupported(refused(aot)));
        }
        if aot != AOT_LC {
            aot = aot.min(255);
            return Err(Error::Unsupported(refused(aot)));
        }
        // GASpecificConfig (Table 4.1).
        let frame_length = if bits.bit() { 960 } else { 1024 };
        if bits.bit() {
            return Err(Error::Unsupported("AAC with a core coder"));
        }
        let extension = bits.bit();
        let program = if channel_configuration == 0 {
            Some(Program::read(&mut bits, 0))
        } else {
            None
        };
        if extension {
            bits.skip(1); // extensionFlag3
        }
        if bits.overrun() {
            return Err(Error::Invalid("truncated AudioSpecificConfig"));
        }
        // The backward-compatible extension signalling.
        if bits.left() >= 16 && bits.read(11) == 0x2b7 {
            let ext = object_type(&mut bits);
            if ext == 5 && bits.bit() {
                return Err(Error::Unsupported(refused(5)));
            }
        }
        let layout = match (&program, configuration_slots(channel_configuration)) {
            (Some(program), _) => program.layout(),
            (None, Some(slots)) => Layout::new(slots, &[], true),
            (None, None) => return Err(Error::Invalid("reserved channel configuration")),
        };
        Self::finish(rate, frame_length, channel_configuration, layout, limits)
    }

    /// The configuration of an AAC LC stream known by its rate and channel
    /// count alone (Matroska's legacy codec IDs without `CodecPrivate`):
    /// channel configurations 1 to 6 for 1 to 6 channels, 7 for 8.
    pub fn from_channels(
        sample_rate: u32,
        channels: usize,
        frame_length: usize,
        limits: &Limits,
    ) -> Result<Self, Error> {
        let configuration = match channels {
            1..=6 => channels as u8,
            8 => 7,
            _ => {
                return Err(Error::Unsupported(
                    "an AAC channel count without a configuration",
                ));
            }
        };
        if frame_length != 1024 && frame_length != 960 {
            return Err(Error::Invalid("frame length"));
        }
        let slots = configuration_slots(configuration).unwrap_or_default();
        Self::finish(
            sample_rate,
            frame_length,
            configuration,
            Layout::new(slots, &[], true),
            limits,
        )
    }

    fn finish(
        sample_rate: u32,
        frame_length: usize,
        channel_configuration: u8,
        layout: Layout,
        limits: &Limits,
    ) -> Result<Self, Error> {
        if sample_rate == 0 {
            return Err(Error::Invalid("sample rate 0"));
        }
        if layout.channels == 0 {
            return Err(Error::Invalid("no channels"));
        }
        if layout.channels > limits.max_channels {
            return Err(Error::TooManyChannels {
                channels: layout.channels,
                max: limits.max_channels,
            });
        }
        Ok(Self {
            sample_rate,
            rate_index: rate_index(sample_rate),
            frame_length,
            channel_configuration,
            layout,
        })
    }

    /// Output sample rate, in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Samples per channel of every frame: 1,024 or 960.
    pub fn frame_length(&self) -> usize {
        self.frame_length
    }

    /// Output channels.
    pub fn channels(&self) -> usize {
        self.layout.channels
    }

    /// The WAV speaker mask of the output channels, which are in WAV's
    /// order; 0 for a program configuration that matches no channel
    /// configuration (its channels are then front, side, back and LFE).
    pub fn channel_mask(&self) -> u32 {
        self.layout.mask
    }

    /// `channelConfiguration`: 1 to 7, or 0 for a program configuration.
    pub fn channel_configuration(&self) -> u8 {
        self.channel_configuration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Result<Config, Error> {
        Config::parse(bytes, &Limits::default())
    }

    #[test]
    fn stereo_lc_at_44100() {
        // 00010 0100 0010 000: LC, 44.1 kHz, two channels.
        let config = parse(&[0x12, 0x10]).unwrap();
        assert_eq!(config.sample_rate(), 44100);
        assert_eq!(config.channels(), 2);
        assert_eq!(config.frame_length(), 1024);
        assert_eq!(config.channel_mask(), 0x3);
        assert_eq!(config.rate_index, 4);
    }

    #[test]
    fn frame_length_flag_gives_960() {
        let config = parse(&[0x12, 0x14]).unwrap();
        assert_eq!(config.frame_length(), 960);
    }

    #[test]
    fn he_aac_is_refused_by_name() {
        // Object type 5.
        assert_eq!(
            parse(&[0x2b, 0x92, 0x08, 0x00]),
            Err(Error::Unsupported("HE-AAC (SBR)"))
        );
        // LC with the 0x2b7 extension and sbrPresentFlag set.
        assert_eq!(
            parse(&[0x13, 0x10, 0x56, 0xe5, 0x98]),
            Err(Error::Unsupported("HE-AAC (SBR)"))
        );
        // AAC Main.
        assert_eq!(parse(&[0x0a, 0x10]), Err(Error::Unsupported("AAC Main")));
    }

    #[test]
    fn five_one_goes_in_wav_order() {
        // LC, 48 kHz, configuration 6.
        let config = parse(&[0x11, 0xb0]).unwrap();
        assert_eq!(config.channels(), 6);
        assert_eq!(config.channel_mask(), 0x3f);
        // C, then L/R, then the back pair, then LFE.
        assert_eq!(config.layout.sce, vec![2]);
        assert_eq!(config.layout.cpe, vec![[0, 1], [4, 5]]);
        assert_eq!(config.layout.lfe, vec![3]);
    }

    #[test]
    fn rates_map_to_band_tables() {
        assert_eq!(rate_index(96000), 0);
        assert_eq!(rate_index(44100), 4);
        assert_eq!(rate_index(46008), 4);
        assert_eq!(rate_index(46009), 3);
        assert_eq!(rate_index(7350), 11);
        assert_eq!(rate_index(8000), 11);
    }

    #[test]
    fn channel_limit_is_enforced() {
        let limits = Limits { max_channels: 2 };
        assert_eq!(
            Config::parse(&[0x11, 0xb0], &limits),
            Err(Error::TooManyChannels {
                channels: 6,
                max: 2
            })
        );
    }
}
