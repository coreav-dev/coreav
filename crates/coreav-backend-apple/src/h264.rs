//! Bounded High-profile H.264 parameter sets and access-unit metadata.
//!
//! This validates a supported header subset, not compressed slice syntax,
//! successful decoding, timestamps, source identity, or final drain counts.

mod bits;
mod params;

use bits::{Bits, rbsp};

/// A configuration or access unit outside the checked output contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Truncated or invalid supported syntax.
    Malformed,
    /// A profile, coding tool, NAL type, or layout outside the initial subset.
    Unsupported,
    /// The encoded geometry or in-band configuration changed.
    FormatMismatch,
    /// A parameter-set reference is unknown.
    ParameterSet,
    /// Slices do not describe one picture in increasing macroblock order.
    Picture,
    /// Declared random access disagrees with IDR slice headers.
    Sync,
    /// A caller-supplied resource bound was exceeded.
    Limit,
    /// Bounded owned storage could not be reserved.
    Allocation,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed H.264 output metadata",
            Self::Unsupported => "unsupported H.264 output metadata",
            Self::FormatMismatch => "H.264 output format changed",
            Self::ParameterSet => "unknown H.264 parameter-set reference",
            Self::Picture => "H.264 slices do not form one ordered picture",
            Self::Sync => "H.264 sync declaration disagrees with IDR headers",
            Self::Limit => "H.264 output metadata exceeds its limit",
            Self::Allocation => "cannot reserve H.264 output storage",
        })
    }
}

impl std::error::Error for Error {}

/// Nonempty, even visible dimensions for progressive 4:2:0 output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dimensions {
    width: u32,
    height: u32,
}

impl Dimensions {
    /// Checks dimensions without allocating picture storage.
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(Error::Unsupported);
        }
        Ok(Self { width, height })
    }

    /// Visible luma width after SPS cropping.
    pub fn width(self) -> u32 {
        self.width
    }

    /// Visible luma height after SPS cropping.
    pub fn height(self) -> u32 {
        self.height
    }
}

/// Bounds applied before owned output is allocated.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum complete avcC size, including both parameter sets and its headers.
    pub max_configuration_bytes: usize,
    /// Maximum uncropped coded width, in luma samples.
    pub max_width: u32,
    /// Maximum uncropped coded height, in luma samples.
    pub max_height: u32,
    /// Maximum complete length-prefixed access-unit size.
    pub max_access_unit_bytes: usize,
    /// Maximum NAL count in one access unit.
    pub max_nals: usize,
}

/// Owned configuration for one High-profile, 8-bit 4:2:0 progressive sequence.
#[derive(Debug)]
pub struct Config {
    sps: Vec<u8>,
    pps: Vec<u8>,
    avcc: Vec<u8>,
    dimensions: Dimensions,
    sequence: params::Sps,
    picture: params::Pps,
}

impl Config {
    /// Checks one SPS and one PPS, including their NAL headers, against the source.
    ///
    /// Scaling matrices, field pictures, POC type 1, slice groups, redundant
    /// pictures, and declared reordering are unsupported. No decoder is linked.
    pub fn from_parameter_sets(
        sps: &[u8],
        pps: &[u8],
        expected: Dimensions,
        limits: Limits,
    ) -> Result<Self, Error> {
        let size = sps
            .len()
            .checked_add(pps.len())
            .and_then(|n| n.checked_add(15))
            .ok_or(Error::Limit)?;
        if size > limits.max_configuration_bytes {
            return Err(Error::Limit);
        }
        let sps_len = u16::try_from(sps.len()).map_err(|_| Error::Limit)?;
        let pps_len = u16::try_from(pps.len()).map_err(|_| Error::Limit)?;
        let sequence = params::sps(sps, expected, limits)?;
        let picture = params::pps(pps, sequence)?;
        let mut avcc = Vec::new();
        avcc.try_reserve_exact(size)
            .map_err(|_| Error::Allocation)?;
        avcc.extend([1, 100, sequence.constraints, sequence.level, 0xff, 0xe1]);
        avcc.extend(sps_len.to_be_bytes());
        avcc.extend(sps);
        avcc.push(1);
        avcc.extend(pps_len.to_be_bytes());
        avcc.extend(pps);
        avcc.extend([0xfd, 0xf8, 0xf8, 0]);
        Ok(Self {
            sps: copy(sps)?,
            pps: copy(pps)?,
            avcc,
            dimensions: expected,
            sequence,
            picture,
        })
    }

    /// Complete canonical avcC with four-byte NAL lengths and High extensions.
    pub fn as_avcc(&self) -> &[u8] {
        &self.avcc
    }

    /// Checked SPS bytes, including the NAL header.
    pub fn sps(&self) -> &[u8] {
        &self.sps
    }

    /// Checked PPS bytes, including the NAL header.
    pub fn pps(&self) -> &[u8] {
        &self.pps
    }

    /// Visible dimensions checked against the requested source.
    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }
}

/// Whether a complete picture declares IDR random access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    /// All coded slices belong to an IDR picture.
    Idr,
    /// A non-IDR I or P picture; no random access is inferred.
    NonIdr,
}

/// One owned four-byte-length-prefixed access unit with consistent picture headers.
#[derive(Debug)]
pub struct AccessUnit {
    bytes: Vec<u8>,
    sync: Sync,
    slices: usize,
    nals: usize,
}

impl AccessUnit {
    /// Validates framing and supported picture-header identity before copying.
    ///
    /// At least one I/P coded slice is required, starting at macroblock zero.
    /// Further slices must share picture identity and increase their start address.
    /// Slice data, reference availability and macroblock coverage require decoding.
    pub fn copy_from(
        data: &[u8],
        config: &Config,
        declared: Sync,
        limits: Limits,
    ) -> Result<Self, Error> {
        if data.len() > limits.max_access_unit_bytes {
            return Err(Error::Limit);
        }
        let mut rest = data;
        let mut picture = None;
        let mut last_mb = 0;
        let mut slices = 0;
        let mut nals = 0_usize;
        let mut actual = Sync::NonIdr;
        let mut delimiter = None;
        while !rest.is_empty() {
            nals = nals.checked_add(1).ok_or(Error::Limit)?;
            if nals > limits.max_nals {
                return Err(Error::Limit);
            }
            let [a, b, c, d] = take(&mut rest, 4)? else {
                return Err(Error::Malformed);
            };
            let length =
                usize::try_from(u32::from_be_bytes([*a, *b, *c, *d])).map_err(|_| Error::Limit)?;
            let nal = take(&mut rest, length)?;
            match header(nal)? & 31 {
                1 | 5 => {
                    let slice = params::slice(nal, config.sequence, config.picture)?;
                    let first_mb = slice.first_mb;
                    let identity = slice.picture;
                    if delimiter.is_some_and(|kind| {
                        if slice.intra {
                            !matches!(kind, 0..=2 | 5..=7)
                        } else {
                            !matches!(kind, 1 | 2 | 6 | 7)
                        }
                    }) {
                        return Err(Error::Picture);
                    }
                    if let Some(previous) = picture {
                        if identity != previous || first_mb <= last_mb {
                            return Err(Error::Picture);
                        }
                    } else {
                        if first_mb != 0 {
                            return Err(Error::Picture);
                        }
                        actual = if nal.first().is_some_and(|b| b & 31 == 5) {
                            Sync::Idr
                        } else {
                            Sync::NonIdr
                        };
                        picture = Some(identity);
                    }
                    last_mb = first_mb;
                    slices += 1;
                }
                7 | 8 => {
                    if slices != 0 {
                        return Err(Error::Unsupported);
                    }
                    let expected = if header(nal)? & 31 == 7 {
                        config.sps()
                    } else {
                        config.pps()
                    };
                    if nal != expected {
                        return Err(Error::FormatMismatch);
                    }
                }
                6 => sei(&rbsp(nal)?)?,
                9 if nals == 1 => {
                    let body = rbsp(nal)?;
                    let mut r = Bits::new(&body)?;
                    delimiter = Some(r.bits(3)?);
                    r.finish()?;
                }
                12 => {
                    let body = rbsp(nal)?;
                    let Some((&0x80, filler)) = body.split_last() else {
                        return Err(Error::Malformed);
                    };
                    if filler.iter().any(|&b| b != 0xff) {
                        return Err(Error::Malformed);
                    }
                }
                _ => return Err(Error::Unsupported),
            }
        }
        if picture.is_none() {
            return Err(Error::Picture);
        }
        if actual != declared {
            return Err(Error::Sync);
        }
        Ok(Self {
            bytes: copy(data)?,
            sync: actual,
            slices,
            nals,
        })
    }

    /// Original checked bytes, including four-byte length prefixes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Sync classification derived from the slice NAL headers.
    pub fn sync(&self) -> Sync {
        self.sync
    }

    /// Number of coded slices, not a decoded-frame count.
    pub fn slices(&self) -> usize {
        self.slices
    }

    /// Number of NAL units, including supported non-VCL data.
    pub fn nals(&self) -> usize {
        self.nals
    }
}

fn header(nal: &[u8]) -> Result<u8, Error> {
    let (&head, _) = nal.split_first().ok_or(Error::Malformed)?;
    if head & 0x80 != 0 {
        return Err(Error::Malformed);
    }
    match head & 31 {
        5 | 7 | 8 if head & 0x60 == 0 => Err(Error::Malformed),
        6 | 9 | 12 if head & 0x60 != 0 => Err(Error::Malformed),
        1 | 5..=9 | 12 => Ok(head),
        _ => Err(Error::Unsupported),
    }
}

fn copy(data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut owned = Vec::new();
    owned
        .try_reserve_exact(data.len())
        .map_err(|_| Error::Allocation)?;
    owned.extend(data);
    Ok(owned)
}

fn take<'a>(data: &mut &'a [u8], count: usize) -> Result<&'a [u8], Error> {
    let value = data.get(..count).ok_or(Error::Malformed)?;
    *data = data.get(count..).ok_or(Error::Malformed)?;
    Ok(value)
}

fn extended(data: &mut &[u8]) -> Result<usize, Error> {
    let mut value = 0_usize;
    loop {
        let (&byte, rest) = data.split_first().ok_or(Error::Malformed)?;
        *data = rest;
        value = value.checked_add(usize::from(byte)).ok_or(Error::Limit)?;
        if byte != 255 {
            return Ok(value);
        }
    }
}

fn sei(mut body: &[u8]) -> Result<(), Error> {
    while body != [0x80] {
        extended(&mut body)?;
        let length = extended(&mut body)?;
        take(&mut body, length)?;
    }
    Ok(())
}
