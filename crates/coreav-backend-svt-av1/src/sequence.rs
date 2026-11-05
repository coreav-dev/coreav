//! Checked sequence configuration for the backend's 8-bit, non-monochrome 4:2:0 scope.
//!
//! Reads AV1 sections 4.10.3, 5.5 and 6.4, including inferred fields and trailing
//! bits. This does not validate frames, level resource constraints, OBU ordering,
//! decoder-model schedules, submitted work, or timestamps.

use std::fmt;

use crate::obu::{Obu, ObuKind};

/// Caller-selected bounds on sequence payload bytes and maximum frame dimensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Maximum sequence payload size, excluding OBU framing.
    pub max_payload_bytes: usize,
    /// Maximum allowed sequence maximum width, in luma samples.
    pub max_width: u32,
    /// Maximum allowed sequence maximum height, in luma samples.
    pub max_height: u32,
}

/// A malformed sequence syntax or field relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Malformed {
    /// A profile value reserved by AV1.
    ReservedProfile,
    /// A level index in the reserved range 24 through 30.
    ReservedLevel,
    /// A reduced header without the still-picture flag.
    ReducedWithoutStill,
    /// A display or decoding clock has a zero numerator or scale.
    ZeroClock,
    /// The equal-picture-interval UVLC exceeds its allowed range.
    PictureInterval,
    /// Two operating points have the same layer-selection mask.
    DuplicateOperatingPoint,
    /// The combined frame-ID bit length exceeds sixteen.
    FrameIdLength,
    /// A color description uses an unassigned or reserved value.
    ReservedColor,
    /// Identity matrix coefficients are combined with chroma subsampling.
    IdentitySubsampling,
    /// Chroma sample position is reserved.
    ReservedChromaPosition,
    /// The required trailing one bit or subsequent zero bits are invalid.
    TrailingBits,
}

/// A sequence feature outside the initial backend-output scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unsupported {
    /// A profile other than profile zero.
    Profile,
    /// Ten-bit or twelve-bit samples.
    BitDepth,
    /// Monochrome samples.
    Monochrome,
    /// A chroma format other than 4:2:0.
    ChromaFormat,
    /// PQ or HLG transfer characteristics.
    HdrTransfer,
    /// Film-grain parameters may be present in frames.
    FilmGrain,
}

/// A caller-selected resource bound that the sequence exceeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    /// Payload byte count.
    PayloadBytes,
    /// Maximum frame width.
    Width,
    /// Maximum frame height.
    Height,
}

/// A sequence type, syntax, supported-feature, or resource-bound failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The supplied OBU is not a sequence header.
    NotSequenceHeader,
    /// The payload ends before a required syntax field or trailing bit.
    Truncated,
    /// The syntax contains an invalid value or relationship.
    Malformed(Malformed),
    /// The complete syntax describes an unsupported configuration.
    Unsupported(Unsupported),
    /// The sequence exceeds a caller-selected limit.
    LimitExceeded(Limit),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotSequenceHeader => "AV1 OBU is not a sequence header",
            Self::Truncated => "AV1 sequence header is truncated",
            Self::Malformed(Malformed::ReservedProfile) => "AV1 sequence profile is reserved",
            Self::Malformed(Malformed::ReservedLevel) => "AV1 sequence level is reserved",
            Self::Malformed(Malformed::ReducedWithoutStill) => {
                "AV1 reduced header requires a still picture"
            }
            Self::Malformed(Malformed::ZeroClock) => "AV1 sequence clock is zero",
            Self::Malformed(Malformed::PictureInterval) => {
                "AV1 picture interval exceeds its integer bound"
            }
            Self::Malformed(Malformed::DuplicateOperatingPoint) => {
                "AV1 operating point is duplicated"
            }
            Self::Malformed(Malformed::FrameIdLength) => "AV1 frame ID length exceeds sixteen bits",
            Self::Malformed(Malformed::ReservedColor) => "AV1 color description is reserved",
            Self::Malformed(Malformed::IdentitySubsampling) => {
                "AV1 identity matrix forbids chroma subsampling"
            }
            Self::Malformed(Malformed::ReservedChromaPosition) => {
                "AV1 chroma sample position is reserved"
            }
            Self::Malformed(Malformed::TrailingBits) => "AV1 sequence trailing bits are invalid",
            Self::Unsupported(Unsupported::Profile) => "AV1 sequence profile is unsupported",
            Self::Unsupported(Unsupported::BitDepth) => "AV1 sequence bit depth is unsupported",
            Self::Unsupported(Unsupported::Monochrome) => "AV1 monochrome sequence is unsupported",
            Self::Unsupported(Unsupported::ChromaFormat) => "AV1 chroma format is unsupported",
            Self::Unsupported(Unsupported::HdrTransfer) => "AV1 HDR transfer is unsupported",
            Self::Unsupported(Unsupported::FilmGrain) => "AV1 film grain is unsupported",
            Self::LimitExceeded(Limit::PayloadBytes) => {
                "AV1 sequence exceeds the payload byte limit"
            }
            Self::LimitExceeded(Limit::Width) => "AV1 sequence exceeds the maximum width limit",
            Self::LimitExceeded(Limit::Height) => "AV1 sequence exceeds the maximum height limit",
        })
    }
}

impl std::error::Error for Error {}

/// The signaled sample range, not a guarantee about encoded sample values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorRange {
    /// Studio swing.
    Studio,
    /// Full swing.
    Full,
}

/// The AV1 position of 4:2:0 chroma samples relative to luma samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ChromaPosition {
    /// Unknown position; external signaling may be needed.
    Unknown = 0,
    /// Horizontally co-located, vertically centered.
    Vertical = 1,
    /// Co-located horizontally and vertically.
    Colocated = 2,
}

/// Checked color metadata, including inferred unspecified values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Color {
    primaries: u8,
    transfer: u8,
    matrix: u8,
    range: ColorRange,
    chroma_position: ChromaPosition,
}

impl Color {
    /// Returns the AV1 color-primaries code, or 2 when unspecified.
    pub fn primaries(&self) -> u8 {
        self.primaries
    }
    /// Returns the AV1 transfer-characteristics code, or 2 when unspecified.
    pub fn transfer(&self) -> u8 {
        self.transfer
    }
    /// Returns the AV1 matrix-coefficients code, or 2 when unspecified.
    pub fn matrix(&self) -> u8 {
        self.matrix
    }
    /// Returns the signaled sample range.
    pub fn range(&self) -> ColorRange {
        self.range
    }
    /// Returns the checked chroma sample position.
    pub fn chroma_position(&self) -> ChromaPosition {
        self.chroma_position
    }
}

/// Optional display timing. It does not establish frame timestamps or a schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timing {
    display_tick: u32,
    time_scale: u32,
    ticks_per_picture: Option<u32>,
}

impl Timing {
    /// Returns the nonzero numerator of the display tick duration.
    pub fn display_tick(&self) -> u32 {
        self.display_tick
    }
    /// Returns the nonzero clock frequency in Hz.
    pub fn time_scale(&self) -> u32 {
        self.time_scale
    }
    /// Returns equal-interval ticks per picture, or no specified picture interval.
    pub fn ticks_per_picture(&self) -> Option<u32> {
        self.ticks_per_picture
    }
}

/// A checked sequence configuration, not checked compressed frames or work counts.
///
/// Construction is restricted to [`Sequence::parse`]. The dimensions are maxima,
/// not actual coded or rendered frame sizes, and need not be even.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sequence {
    max_width: u32,
    max_height: u32,
    color: Color,
    first_level: u8,
    first_high_tier: bool,
    first_display_delay: Option<u8>,
    still_picture: bool,
    timing: Option<Timing>,
    frame_syntax: FrameSyntax,
}

/// Checked sequence fields that control uncompressed frame-header parsing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FrameSyntax {
    /// Whether the reduced still-picture header is used.
    pub reduced: bool,
    /// Whether a decoder model is present.
    pub decoder_model: bool,
    /// Whether frame IDs are present.
    pub frame_ids: bool,
    /// Whether the sequence has exactly one operating point with mask zero.
    pub single_operating_point: bool,
    /// Width syntax field length, from one through sixteen.
    pub width_bits: u8,
    /// Height syntax field length, from one through sixteen.
    pub height_bits: u8,
    /// Whether superblocks have 128 luma samples per side.
    pub superblock_128: bool,
    /// Order-hint field length, or zero when disabled.
    pub order_hint_bits: u8,
    /// Screen-content tool selection, with two meaning frame-selected.
    pub screen_tools: u8,
    /// Integer-motion selection, with two meaning frame-selected.
    pub integer_mv: u8,
    /// Whether reference-frame motion vectors are enabled.
    pub reference_mvs: bool,
    /// Whether warped motion is enabled.
    pub warped_motion: bool,
    /// Whether super-resolution may be used.
    pub superres: bool,
    /// Whether CDEF may be used.
    pub cdef: bool,
    /// Whether loop restoration may be used.
    pub restoration: bool,
    /// Whether U and V quantization deltas are separate.
    pub separate_uv_delta_q: bool,
}

impl Sequence {
    /// Reads one framed sequence header within the supplied limits.
    ///
    /// Every syntax field and the required trailing one and zero bits are read
    /// before accepting the supported configuration. Additional trailing zero
    /// bytes are allowed by AV1. Reserved values and invalid relationships are
    /// malformed; other profiles, bit depths, monochrome, non-4:2:0, PQ, HLG and
    /// film grain are unsupported. Unspecified color metadata remains accepted
    /// and does not prove that the source is SDR.
    pub fn parse(obu: Obu<'_>, limits: Limits) -> Result<Self, Error> {
        if obu.kind() != ObuKind::SequenceHeader {
            return Err(Error::NotSequenceHeader);
        }
        if obu.payload().len() > limits.max_payload_bytes {
            return Err(Error::LimitExceeded(Limit::PayloadBytes));
        }
        let mut bits = Bits {
            bytes: obu.payload(),
            current: 0,
            left: 0,
        };
        let profile = bits.read(3)? as u8;
        if profile > 2 {
            return Err(Error::Malformed(Malformed::ReservedProfile));
        }
        let still_picture = bits.flag()?;
        let reduced = bits.flag()?;
        let mut frame_syntax = FrameSyntax {
            reduced,
            single_operating_point: reduced,
            screen_tools: 2,
            integer_mv: 2,
            ..FrameSyntax::default()
        };
        if reduced && !still_picture {
            return Err(Error::Malformed(Malformed::ReducedWithoutStill));
        }
        let mut timing = None;
        let mut first_level = 0;
        let mut first_high_tier = false;
        let mut first_display_delay = None;
        if reduced {
            first_level = level(&mut bits)?;
        } else {
            let mut delay_bits = None;
            if bits.flag()? {
                timing = Some(read_timing(&mut bits)?);
                if bits.flag()? {
                    frame_syntax.decoder_model = true;
                    delay_bits = Some(bits.read(5)? as u8 + 1);
                    if bits.read(32)? == 0 {
                        return Err(Error::Malformed(Malformed::ZeroClock));
                    }
                    bits.read(5)?;
                    bits.read(5)?;
                }
            }
            let display_delays = bits.flag()?;
            let operating_points = bits.read(5)? + 1;
            let mut ids = [u16::MAX; 32];
            for op in 0..operating_points {
                let id = bits.read(12)? as u16;
                if op == 0 {
                    frame_syntax.single_operating_point = operating_points == 1 && id == 0;
                }
                if ids.contains(&id) {
                    return Err(Error::Malformed(Malformed::DuplicateOperatingPoint));
                }
                if let Some(slot) = ids.iter_mut().find(|slot| **slot == u16::MAX) {
                    *slot = id;
                }
                let level = level(&mut bits)?;
                let high_tier = level > 7 && bits.flag()?;
                if let Some(n) = delay_bits
                    && bits.flag()?
                {
                    bits.read(n)?;
                    bits.read(n)?;
                    bits.flag()?;
                }
                let display_delay = if display_delays && bits.flag()? {
                    Some(bits.read(4)? as u8 + 1)
                } else {
                    None
                };
                if op == 0 {
                    first_level = level;
                    first_high_tier = high_tier;
                    first_display_delay = display_delay;
                }
            }
        }
        let width_bits = bits.read(4)? as u8 + 1;
        let height_bits = bits.read(4)? as u8 + 1;
        frame_syntax.width_bits = width_bits;
        frame_syntax.height_bits = height_bits;
        let max_width = bits.read(width_bits)? + 1;
        let max_height = bits.read(height_bits)? + 1;
        if max_width > limits.max_width {
            return Err(Error::LimitExceeded(Limit::Width));
        }
        if max_height > limits.max_height {
            return Err(Error::LimitExceeded(Limit::Height));
        }
        if !reduced && bits.flag()? {
            frame_syntax.frame_ids = true;
            let delta = bits.read(4)?;
            let additional = bits.read(3)?;
            if delta + additional + 3 > 16 {
                return Err(Error::Malformed(Malformed::FrameIdLength));
            }
        }
        frame_syntax.superblock_128 = bits.flag()?;
        bits.read(2)?;
        if !reduced {
            bits.read(2)?;
            frame_syntax.warped_motion = bits.flag()?;
            bits.flag()?;
            let order_hint = bits.flag()?;
            if order_hint {
                bits.flag()?;
                frame_syntax.reference_mvs = bits.flag()?;
            }
            frame_syntax.screen_tools = if bits.flag()? {
                2
            } else {
                u8::from(bits.flag()?)
            };
            frame_syntax.integer_mv = if frame_syntax.screen_tools == 0 || bits.flag()? {
                2
            } else {
                u8::from(bits.flag()?)
            };
            if order_hint {
                frame_syntax.order_hint_bits = bits.read(3)? as u8 + 1;
            }
        }
        frame_syntax.superres = bits.flag()?;
        frame_syntax.cdef = bits.flag()?;
        frame_syntax.restoration = bits.flag()?;
        let color = read_color(&mut bits, profile)?;
        frame_syntax.separate_uv_delta_q = color.separate_uv_delta_q;
        let film_grain = bits.flag()?;
        bits.finish()?;
        if profile != 0 {
            return Err(Error::Unsupported(Unsupported::Profile));
        }
        if color.bit_depth != 8 {
            return Err(Error::Unsupported(Unsupported::BitDepth));
        }
        if color.monochrome {
            return Err(Error::Unsupported(Unsupported::Monochrome));
        }
        if !color.subsampling_x || !color.subsampling_y {
            return Err(Error::Unsupported(Unsupported::ChromaFormat));
        }
        if matches!(color.color.transfer, 16 | 18) {
            return Err(Error::Unsupported(Unsupported::HdrTransfer));
        }
        if film_grain {
            return Err(Error::Unsupported(Unsupported::FilmGrain));
        }
        Ok(Self {
            max_width,
            max_height,
            color: color.color,
            first_level,
            first_high_tier,
            first_display_delay,
            still_picture,
            timing,
            frame_syntax,
        })
    }

    /// Returns the checked fields that determine frame-header syntax.
    pub fn frame_syntax(&self) -> FrameSyntax {
        self.frame_syntax
    }

    /// Returns the sequence maximum width in luma samples, not actual frame width.
    pub fn max_width(&self) -> u32 {
        self.max_width
    }
    /// Returns the sequence maximum height in luma samples, not actual frame height.
    pub fn max_height(&self) -> u32 {
        self.max_height
    }
    /// Returns the checked color description, range and chroma position.
    pub fn color(&self) -> Color {
        self.color
    }
    /// Returns the first operating point's level index, including 31 for maximum parameters.
    pub fn first_level(&self) -> u8 {
        self.first_level
    }
    /// Returns the first operating point's tier, false when inferred to be main tier.
    pub fn first_high_tier(&self) -> bool {
        self.first_high_tier
    }
    /// Returns the first signaled initial display delay in frames, from 1 through 16.
    ///
    /// None means not signaled, not zero delay. This is not the ISOBMFF initial
    /// presentation delay, whose unit is samples rather than decoded frames.
    pub fn first_display_delay(&self) -> Option<u8> {
        self.first_display_delay
    }
    /// Returns the still-picture flag without checking the number of coded frames.
    pub fn still_picture(&self) -> bool {
        self.still_picture
    }
    /// Returns the optional display-clock information.
    pub fn timing(&self) -> Option<Timing> {
        self.timing
    }
    /// Derives the four-byte av1C record prefix, not an entire box or configOBUs.
    ///
    /// Follows the AV1 ISOBMFF binding v1.3.0 section 2.3. Presentation delay is
    /// absent because a sequence alone cannot establish its sample-count value.
    pub fn av1c_header(&self) -> Av1CHeader {
        Av1CHeader([
            0x81,
            self.first_level,
            (u8::from(self.first_high_tier) << 7) | 0x0c | self.color.chroma_position as u8,
            0,
        ])
    }
}

/// The fixed four-byte AV1CodecConfigurationRecord prefix, derived from a sequence.
///
/// This is not a complete av1C box and does not establish sample conformance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Av1CHeader([u8; 4]);

impl Av1CHeader {
    /// Returns the fixed record bytes, with no signaled initial presentation delay.
    pub fn bytes(&self) -> [u8; 4] {
        self.0
    }
}

fn level(bits: &mut Bits<'_>) -> Result<u8, Error> {
    let level = bits.read(5)? as u8;
    if (24..=30).contains(&level) {
        return Err(Error::Malformed(Malformed::ReservedLevel));
    }
    Ok(level)
}

fn read_timing(bits: &mut Bits<'_>) -> Result<Timing, Error> {
    let display_tick = bits.read(32)?;
    let time_scale = bits.read(32)?;
    if display_tick == 0 || time_scale == 0 {
        return Err(Error::Malformed(Malformed::ZeroClock));
    }
    let ticks_per_picture = if bits.flag()? {
        Some(
            bits.uvlc()?
                .checked_add(1)
                .ok_or(Error::Malformed(Malformed::PictureInterval))?,
        )
    } else {
        None
    };
    Ok(Timing {
        display_tick,
        time_scale,
        ticks_per_picture,
    })
}

struct ParsedColor {
    color: Color,
    bit_depth: u8,
    monochrome: bool,
    subsampling_x: bool,
    subsampling_y: bool,
    separate_uv_delta_q: bool,
}

fn read_color(bits: &mut Bits<'_>, profile: u8) -> Result<ParsedColor, Error> {
    let high = bits.flag()?;
    let bit_depth = if profile == 2 && high && bits.flag()? {
        12
    } else if high {
        10
    } else {
        8
    };
    let monochrome = profile != 1 && bits.flag()?;
    let (primaries, transfer, matrix) = if bits.flag()? {
        let primaries = bits.read(8)? as u8;
        let transfer = bits.read(8)? as u8;
        let matrix = bits.read(8)? as u8;
        if !matches!(primaries, 1 | 2 | 4..=12 | 22)
            || !matches!(transfer, 1 | 2 | 4..=18)
            || !matches!(matrix, 0..=2 | 4..=14)
        {
            return Err(Error::Malformed(Malformed::ReservedColor));
        }
        (primaries, transfer, matrix)
    } else {
        (2, 2, 2)
    };
    let mut chroma_position = ChromaPosition::Unknown;
    let (full_range, subsampling_x, subsampling_y) = if monochrome {
        (bits.flag()?, true, true)
    } else if (primaries, transfer, matrix) == (1, 13, 0) {
        (true, false, false)
    } else {
        let full_range = bits.flag()?;
        let (x, y) = match profile {
            0 => (true, true),
            1 => (false, false),
            _ if bit_depth == 12 => {
                let x = bits.flag()?;
                (x, x && bits.flag()?)
            }
            _ => (true, false),
        };
        if x && y {
            chroma_position = match bits.read(2)? {
                0 => ChromaPosition::Unknown,
                1 => ChromaPosition::Vertical,
                2 => ChromaPosition::Colocated,
                _ => return Err(Error::Malformed(Malformed::ReservedChromaPosition)),
            };
        }
        (full_range, x, y)
    };
    let separate_uv_delta_q = !monochrome && bits.flag()?;
    if matrix == 0 && (subsampling_x || subsampling_y) {
        return Err(Error::Malformed(Malformed::IdentitySubsampling));
    }
    Ok(ParsedColor {
        color: Color {
            primaries,
            transfer,
            matrix,
            range: if full_range {
                ColorRange::Full
            } else {
                ColorRange::Studio
            },
            chroma_position,
        },
        bit_depth,
        monochrome,
        subsampling_x,
        subsampling_y,
        separate_uv_delta_q,
    })
}

struct Bits<'a> {
    bytes: &'a [u8],
    current: u8,
    left: u8,
}

impl Bits<'_> {
    fn flag(&mut self) -> Result<bool, Error> {
        Ok(self.read(1)? != 0)
    }

    fn read(&mut self, count: u8) -> Result<u32, Error> {
        let mut value = 0;
        for _ in 0..count {
            if self.left == 0 {
                let (&byte, rest) = self.bytes.split_first().ok_or(Error::Truncated)?;
                self.bytes = rest;
                self.current = byte;
                self.left = 8;
            }
            self.left -= 1;
            value = (value << 1) | u32::from((self.current >> self.left) & 1);
        }
        Ok(value)
    }

    fn uvlc(&mut self) -> Result<u32, Error> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros >= 32 {
                return Err(Error::Malformed(Malformed::PictureInterval));
            }
        }
        Ok(self.read(zeros)? + ((1_u32 << zeros) - 1))
    }

    fn finish(mut self) -> Result<(), Error> {
        if !self.flag()? {
            return Err(Error::Malformed(Malformed::TrailingBits));
        }
        while self.left > 0 {
            if self.flag()? {
                return Err(Error::Malformed(Malformed::TrailingBits));
            }
        }
        if self.bytes.iter().any(|byte| *byte != 0) {
            return Err(Error::Malformed(Malformed::TrailingBits));
        }
        Ok(())
    }
}
