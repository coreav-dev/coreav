//! Bounded HEVC output storage and stateful header validation.
//!
//! Header syntax and picture bookkeeping belong to `coreav_hevc`. These checks
//! do not decode entropy data or prove that compressed pictures reconstruct.

use crate::h264::{Dimensions, Limits};
use coreav_hevc::{HeaderParser, LengthPrefixed, NalHeader, NalInfo};

/// A refusal of the checked HEVC output contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// An internal decoder or validator failure, not invalid output metadata.
    Internal,
    /// Truncated or invalid syntax or framing.
    Malformed,
    /// A profile, coding tool, or NAL type outside the output contract.
    Unsupported,
    /// The supplied configuration or activated picture format changed.
    FormatMismatch,
    /// A sample does not describe exactly one complete picture header sequence.
    Picture,
    /// The sample's sync declaration disagrees with its IRAP headers.
    Sync,
    /// A supplied resource bound was exceeded.
    Limit,
    /// Bounded output storage could not be reserved.
    Allocation,
    /// Validation was attempted after failure or final drain.
    State,
    /// Final coded, output-flag, or drained picture counts disagree.
    Count,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Internal => "internal HEVC failure",
            Self::Malformed => "malformed HEVC output metadata",
            Self::Unsupported => "unsupported HEVC output metadata",
            Self::FormatMismatch => "HEVC output format changed",
            Self::Picture => "HEVC sample does not contain one picture",
            Self::Sync => "HEVC sync declaration disagrees with IRAP headers",
            Self::Limit => "HEVC output metadata exceeds its limit",
            Self::Allocation => "cannot reserve HEVC output storage",
            Self::State => "HEVC output validator is no longer active",
            Self::Count => "HEVC output picture counts disagree",
        })
    }
}

impl std::error::Error for Error {}

impl From<coreav_hevc::Error> for Error {
    fn from(error: coreav_hevc::Error) -> Self {
        match error {
            coreav_hevc::Error::Internal(_) => Self::Internal,
            coreav_hevc::Error::InvalidData(_) => Self::Malformed,
            coreav_hevc::Error::Unsupported(_) => Self::Unsupported,
            coreav_hevc::Error::Limit(_) => Self::Limit,
        }
    }
}

/// Owned, syntax-checked parameter sets and their exact CoreMedia hvcC record.
///
/// Profile, colour and visible geometry are checked when a slice activates the
/// sets through `Stream`, rather than by a second parameter-set parser.
#[derive(Debug)]
pub struct Config {
    sets: [Vec<u8>; 3],
    hvcc: Vec<u8>,
    dimensions: Dimensions,
    limits: Limits,
}

impl Config {
    /// Checks all three sets and a bounded, exact three-array hvcC record.
    pub fn from_parameter_sets(
        vps: &[u8],
        sps: &[u8],
        pps: &[u8],
        hvcc: &[u8],
        expected: Dimensions,
        limits: Limits,
    ) -> Result<Self, Error> {
        if limits.max_configuration_bytes == 0
            || limits.max_width == 0
            || limits.max_height == 0
            || expected.width() > limits.max_width
            || expected.height() > limits.max_height
        {
            return Err(Error::Limit);
        }
        let size = [vps, sps, pps, hvcc]
            .iter()
            .try_fold(0usize, |size, data| {
                size.checked_add(data.len()).ok_or(Error::Limit)
            })?;
        if size > limits.max_configuration_bytes {
            return Err(Error::Limit);
        }
        check_hvcc(hvcc, [vps, sps, pps])?;
        let mut parser = new_parser(coreav_hevc::Limits {
            max_nal_bytes: limits.max_configuration_bytes,
            max_width: limits.max_width,
            max_height: limits.max_height,
            max_ctus: u32::MAX,
            max_pictures: 0,
            ..coreav_hevc::Limits::default()
        })?;
        read_sets(parser.first_mut().ok_or(Error::State)?, [vps, sps, pps])?;
        Ok(Self {
            sets: [copy(vps)?, copy(sps)?, copy(pps)?],
            hvcc: copy(hvcc)?,
            dimensions: expected,
            limits,
        })
    }

    /// The validated VPS including its NAL header.
    pub fn vps(&self) -> &[u8] {
        let [vps, _, _] = &self.sets;
        vps
    }

    /// The validated SPS including its NAL header.
    pub fn sps(&self) -> &[u8] {
        let [_, sps, _] = &self.sets;
        sps
    }

    /// The validated PPS including its NAL header.
    pub fn pps(&self) -> &[u8] {
        let [_, _, pps] = &self.sets;
        pps
    }

    /// The copied hvcC record, with four-byte NAL lengths.
    pub fn as_hvcc(&self) -> &[u8] {
        &self.hvcc
    }

    /// Requested visible dimensions, checked on picture activation.
    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }
}

/// Owned, four-byte-length-prefixed sample awaiting stateful validation.
#[derive(Debug)]
pub struct AccessUnit {
    bytes: Vec<u8>,
    sync: bool,
}

impl AccessUnit {
    /// Bounds and checks complete NAL framing before copying native bytes.
    ///
    /// This does not validate picture headers. Call `Stream::validate` in source
    /// order before making these bytes available as encoded output.
    pub fn copy_from(bytes: &[u8], sync: bool, limits: Limits) -> Result<Self, Error> {
        if bytes.len() > limits.max_access_unit_bytes {
            return Err(Error::Limit);
        }
        let mut count = 0usize;
        for nal in LengthPrefixed::new(bytes, 4)? {
            let nal = nal?;
            count = count.checked_add(1).ok_or(Error::Limit)?;
            if count > limits.max_nals {
                return Err(Error::Limit);
            }
            NalHeader::parse(nal)?;
        }
        if count == 0 {
            return Err(Error::Picture);
        }
        Ok(Self {
            bytes: copy(bytes)?,
            sync,
        })
    }

    /// The copied sample; delivery still requires successful stream validation.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The native sample's declared random-access status.
    pub fn is_sync(&self) -> bool {
        self.sync
    }
}

/// A persistent, terminal-on-error HEVC output validator.
pub struct Stream {
    parser: Vec<HeaderParser>,
    sets: [Vec<u8>; 3],
    dimensions: Dimensions,
    pictures: u64,
    active: bool,
}

impl Stream {
    /// Loads the checked configuration into one bounded header parser.
    pub fn new(config: &Config, mut limits: coreav_hevc::Limits) -> Result<Self, Error> {
        limits.max_width = limits.max_width.min(config.limits.max_width);
        limits.max_height = limits.max_height.min(config.limits.max_height);
        let mut parser = new_parser(limits)?;
        read_sets(
            parser.first_mut().ok_or(Error::State)?,
            [config.vps(), config.sps(), config.pps()],
        )?;
        Ok(Self {
            parser,
            sets: [
                copy(config.vps())?,
                copy(config.sps())?,
                copy(config.pps())?,
            ],
            dimensions: config.dimensions,
            pictures: 0,
            active: true,
        })
    }

    /// Checks exactly one I/P picture in source order before sample delivery.
    ///
    /// Exact repeated parameter sets are allowed; replacements, B slices,
    /// suppressed output, end markers and format changes are refused.
    pub fn validate(&mut self, unit: &AccessUnit, config: &Config) -> Result<(), Error> {
        if !self.active {
            return Err(Error::State);
        }
        let result = self.validate_inner(unit, config);
        if result.is_err() {
            self.active = false;
        }
        result
    }

    fn validate_inner(&mut self, unit: &AccessUnit, config: &Config) -> Result<(), Error> {
        if self.sets != config.sets || self.dimensions != config.dimensions {
            return Err(Error::FormatMismatch);
        }
        let mut first = false;
        let mut suffix = false;
        let mut delimiter = false;
        for nal in LengthPrefixed::new(unit.as_bytes(), 4)? {
            let nal = nal?;
            let kind = NalHeader::parse(nal)?.unit_type;
            match kind {
                0..=9 | 16..=21 => {
                    if suffix {
                        return Err(Error::Picture);
                    }
                    if unit.sync != (16..=21).contains(&kind) {
                        return Err(Error::Sync);
                    }
                }
                32..=34 => {
                    let expected = match kind {
                        32 => config.vps(),
                        33 => config.sps(),
                        _ => config.pps(),
                    };
                    if nal != expected {
                        return Err(Error::FormatMismatch);
                    }
                    if suffix {
                        return Err(Error::Picture);
                    }
                }
                35 => {
                    if first || delimiter {
                        return Err(Error::Picture);
                    }
                    delimiter = true;
                }
                39 => {
                    if first {
                        return Err(Error::Picture);
                    }
                }
                38 | 40 => {
                    if !first {
                        return Err(Error::Picture);
                    }
                    suffix = true;
                }
                _ => return Err(Error::Unsupported),
            }
            if let NalInfo::Slice {
                header,
                picture,
                order,
            } = self
                .parser
                .first_mut()
                .ok_or(Error::State)?
                .read_nal(nal, |_| Ok(()))?
            {
                if header.first == first {
                    return Err(Error::Picture);
                }
                first = true;
                if header.slice_type == Some(0) || header.output == Some(false) {
                    return Err(Error::Unsupported);
                }
                if order.is_some_and(|order| !order.output) {
                    return Err(Error::Picture);
                }
                if picture.profile_idc != 1 || picture.bit_depth != 8 {
                    return Err(Error::Unsupported);
                }
                if picture.visible_width != config.dimensions.width()
                    || picture.visible_height != config.dimensions.height()
                    || picture.vui.full_range != Some(false)
                    || picture.vui.colour_primaries != Some(1)
                    || picture.vui.transfer_characteristics != Some(1)
                    || picture.vui.matrix_coeffs != Some(1)
                    || picture
                        .vui
                        .default_display_window
                        .is_some_and(|window| window != [0; 4])
                {
                    return Err(Error::FormatMismatch);
                }
            }
        }
        if !first {
            return Err(Error::Picture);
        }
        self.pictures = self.pictures.checked_add(1).ok_or(Error::Limit)?;
        Ok(())
    }

    /// Drains header-only picture bookkeeping and checks all output counts.
    ///
    /// Successful or failed finalization makes this validator terminal.
    pub fn finish(&mut self, expected_frames: u64) -> Result<(), Error> {
        if !self.active {
            return Err(Error::State);
        }
        self.active = false;
        let summary = self
            .parser
            .first_mut()
            .ok_or(Error::State)?
            .finish(|_| Ok(()))?;
        if self.pictures != expected_frames
            || summary.pictures != expected_frames
            || summary.output_flag_pictures != expected_frames
            || summary.output_pictures != expected_frames
        {
            return Err(Error::Count);
        }
        Ok(())
    }
}

fn new_parser(limits: coreav_hevc::Limits) -> Result<Vec<HeaderParser>, Error> {
    let mut parser = Vec::new();
    parser.try_reserve_exact(1).map_err(|_| Error::Allocation)?;
    parser.push(HeaderParser::new(limits));
    Ok(parser)
}

fn copy(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::Allocation)?;
    result.extend_from_slice(bytes);
    Ok(result)
}

fn read_sets(parser: &mut HeaderParser, sets: [&[u8]; 3]) -> Result<(), Error> {
    for (expected, bytes) in (32..=34).zip(sets) {
        if NalHeader::parse(bytes)?.unit_type != expected {
            return Err(Error::Malformed);
        }
        match parser.read_nal(bytes, |_| Ok(()))? {
            NalInfo::Vps(_) | NalInfo::Sps(_) | NalInfo::Pps(_) => {}
            _ => return Err(Error::Malformed),
        }
    }
    Ok(())
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8], Error> {
    let (head, tail) = bytes.split_at_checked(length).ok_or(Error::Malformed)?;
    *bytes = tail;
    Ok(head)
}

fn byte(bytes: &mut &[u8]) -> Result<u8, Error> {
    take(bytes, 1)?.first().copied().ok_or(Error::Malformed)
}

fn short(bytes: &mut &[u8]) -> Result<u16, Error> {
    let [a, b] = take(bytes, 2)? else {
        return Err(Error::Malformed);
    };
    Ok(u16::from_be_bytes([*a, *b]))
}

fn check_hvcc(mut bytes: &[u8], sets: [&[u8]; 3]) -> Result<(), Error> {
    if byte(&mut bytes)? != 1 {
        return Err(Error::Malformed);
    }
    let profile = byte(&mut bytes)?;
    if profile & 0xc0 != 0 || profile & 0x1f != 1 {
        return Err(Error::Unsupported);
    }
    take(&mut bytes, 11)?;
    if short(&mut bytes)? & 0xf000 != 0xf000
        || byte(&mut bytes)? & 0xfc != 0xfc
        || byte(&mut bytes)? != 0xfd
        || byte(&mut bytes)? != 0xf8
        || byte(&mut bytes)? != 0xf8
    {
        return Err(Error::Unsupported);
    }
    take(&mut bytes, 2)?;
    let temporal = byte(&mut bytes)?;
    if temporal & 3 != 3 || (temporal >> 3) & 7 == 0 {
        return Err(Error::Unsupported);
    }
    if byte(&mut bytes)? != 3 {
        return Err(Error::Malformed);
    }
    let mut seen = 0u8;
    for _ in 0..3 {
        let header = byte(&mut bytes)?;
        let (mask, expected) = match header & 0x3f {
            32 => (1, sets.first().copied()),
            33 => (2, sets.get(1).copied()),
            34 => (4, sets.get(2).copied()),
            _ => return Err(Error::Unsupported),
        };
        if header & 0x40 != 0 || seen & mask != 0 || short(&mut bytes)? != 1 {
            return Err(Error::Malformed);
        }
        seen |= mask;
        let length = usize::from(short(&mut bytes)?);
        if Some(take(&mut bytes, length)?) != expected {
            return Err(Error::FormatMismatch);
        }
    }
    if !bytes.is_empty() || seen != 7 {
        return Err(Error::Malformed);
    }
    Ok(())
}
