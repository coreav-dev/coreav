use crate::{bits::Bits, codec, syntax};

/// A bounded encoding failure. No access unit is returned on failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Dimensions, QP, signalling, or plane layout is unsupported.
    Settings,
    /// A caller or level bound would be exceeded.
    Limit,
    /// Memory reservation failed.
    Allocation,
    /// A coefficient or intermediate is outside the normative range.
    Coefficient,
    /// A syntax value cannot be represented.
    Syntax,
    /// An internal geometry invariant was violated.
    Internal,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "H.264 encoding failed: {self:?}")
    }
}
impl std::error::Error for Error {}

/// Result of a checked encoding operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Explicit preparation signalling, independent of source history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// Full sample range rather than limited range.
    pub full_range: bool,
    /// H.264 colour_primaries value.
    pub primaries: u8,
    /// H.264 transfer_characteristics value.
    pub transfer: u8,
    /// H.264 matrix_coefficients value.
    pub matrix: u8,
    /// H.264 chroma_sample_loc_type for both field positions, 0 through 5.
    pub chroma_location: u8,
}

/// Immutable sequence settings. Timing is always nominal 30/1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Even visible width, 16 through 1920.
    pub width: usize,
    /// Even visible height, 16 through 1080.
    pub height: usize,
    /// Fixed luma QP, 10 through 51.
    pub qp: u8,
    /// Preparation metadata written to VUI.
    pub colour: Colour,
}

/// Caller-owned resource limits, also capped by level 5.1.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Maximum coded luma samples including padding.
    pub coded_pixels: usize,
    /// Maximum successful pictures in this sequence.
    pub frames: u64,
    /// Maximum bytes in SPS, PPS and IDR together, including NAL headers.
    pub access_unit_bytes: usize,
    /// Maximum cumulative NAL bytes. Annex B delimiters are not included.
    pub total_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            coded_pixels: 1920 * 1088,
            frames: 10_000,
            access_unit_bytes: 1_000_000,
            total_bytes: 1_000_000_000,
        }
    }
}

/// A checked 8-bit plane view. Padding after its final visible row is optional.
#[derive(Debug, Clone, Copy)]
pub struct Plane<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) stride: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}
impl<'a> Plane<'a> {
    /// Validate the complete visible extent without overflowing arithmetic.
    pub fn new(bytes: &'a [u8], stride: usize, width: usize, height: usize) -> Result<Self> {
        let end = height
            .checked_sub(1)
            .and_then(|h| h.checked_mul(stride))
            .and_then(|n| n.checked_add(width))
            .ok_or(Error::Settings)?;
        if width == 0 || stride < width || end > bytes.len() {
            return Err(Error::Settings);
        }
        Ok(Self {
            bytes,
            stride,
            width,
            height,
        })
    }
    pub(crate) fn sample(self, x: usize, y: usize) -> Result<u8> {
        self.bytes
            .get(y.min(self.height - 1) * self.stride + x.min(self.width - 1))
            .copied()
            .ok_or(Error::Internal)
    }
}

/// The three visible, progressive 4:2:0 planes in Y, Cb, Cr order.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a> {
    /// Luma plane.
    pub y: Plane<'a>,
    /// Blue-difference plane.
    pub cb: Plane<'a>,
    /// Red-difference plane.
    pub cr: Plane<'a>,
}

/// One complete access unit and the encoder's visible reconstruction.
#[derive(Debug)]
pub struct AccessUnit {
    /// Complete NAL units, without Annex B delimiters, in SPS/PPS/IDR order.
    pub nals: [Vec<u8>; 3],
    /// Tightly packed visible Y, then Cb, then Cr samples.
    pub reconstruction: Vec<u8>,
}

/// Fixed-QP sequence accounting. A failed call changes no sequence state.
pub struct Encoder {
    pub(crate) settings: Settings,
    pub(crate) limits: Limits,
    frames: u64,
    bytes: u64,
    pub(crate) coded_width: usize,
    pub(crate) coded_height: usize,
}

impl Encoder {
    /// Validate the complete G1 envelope before any frame allocation.
    pub fn new(settings: Settings, mut limits: Limits) -> Result<Self> {
        let Settings {
            width,
            height,
            qp,
            colour: c,
        } = settings;
        if !(16..=1920).contains(&width)
            || !(16..=1080).contains(&height)
            || width % 2 != 0
            || height % 2 != 0
            || !(10..=51).contains(&qp)
            || c.chroma_location > 5
            || !matches!(c.primaries, 1 | 2 | 4..=12 | 22)
            || !matches!(c.transfer, 1 | 2 | 4..=15 | 17)
            || !matches!(c.matrix, 1 | 2 | 4..=14)
            || (c.primaries == 2 && matches!(c.matrix, 12 | 13))
        {
            return Err(Error::Settings);
        }
        let coded_width = width.div_ceil(16) * 16;
        let coded_height = height.div_ceil(16) * 16;
        let mbs = coded_width * coded_height / 256;
        if coded_width * coded_height > limits.coded_pixels
            || mbs > 36_864
            || mbs * 30 > 983_040
            || mbs > 184_320
            || (coded_width / 16).pow(2) > 36_864 * 8
            || (coded_height / 16).pow(2) > 36_864 * 8
            || limits.frames == 0
        {
            return Err(Error::Limit);
        }
        limits.access_unit_bytes = limits.access_unit_bytes.min(192 * mbs).min(1_000_000);
        Ok(Self {
            settings,
            limits,
            frames: 0,
            bytes: 0,
            coded_width,
            coded_height,
        })
    }

    /// Encode one independent IDR picture, committing counters only on success.
    pub fn encode(&mut self, picture: Picture<'_>) -> Result<AccessUnit> {
        for (p, scale) in [(picture.y, 1), (picture.cb, 2), (picture.cr, 2)] {
            if p.width != self.settings.width / scale || p.height != self.settings.height / scale {
                return Err(Error::Settings);
            }
        }
        if self.frames >= self.limits.frames {
            return Err(Error::Limit);
        }
        let mut sps = Bits::new(self.limits.access_unit_bytes);
        syntax::sps(&mut sps, self)?;
        let mut pps = Bits::new(self.limits.access_unit_bytes);
        syntax::pps(&mut pps)?;
        let (slice, reconstruction) = codec::picture(self, picture, (self.frames % 2) as u32)?;
        let nals = [sps.nal(0x67)?, pps.nal(0x68)?, slice];
        let size: usize = nals.iter().map(Vec::len).sum();
        let total = self.bytes.checked_add(size as u64).ok_or(Error::Limit)?;
        if size > self.limits.access_unit_bytes || total > self.limits.total_bytes {
            return Err(Error::Limit);
        }
        self.bytes = total;
        self.frames += 1;
        Ok(AccessUnit {
            nals,
            reconstruction,
        })
    }
}

pub(crate) fn zeros<T: Clone + Default>(n: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| Error::Allocation)?;
    v.resize(n, T::default());
    Ok(v)
}
