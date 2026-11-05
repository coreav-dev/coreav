/// A rejected kernel input. No error reads or retains external state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Only 8-bit and 10-bit samples are supported.
    BitDepth,
    /// Transform blocks must be square with side 4, 8, 16 or 32.
    BlockSize,
    /// An intra prediction mode must be in 0..=34.
    IntraMode,
    /// A stride, slice length or checked address is invalid.
    Buffer,
    /// A reference or prediction sample exceeds the declared bit depth.
    Sample,
    /// QP is outside 0..=51 + 6 * (bit depth - 8).
    Qp,
    /// A supplied effective scaling factor is zero.
    Scaling,
    /// The transform type is not valid for this block size.
    Transform,
}

/// Maps an effective 4:2:0 chroma QP index using H.265 Table 8-10.
pub fn chroma_qp_index(index: i32) -> Result<i32, Error> {
    if !(-24..=63).contains(&index) {
        return Err(Error::Qp);
    }
    Ok(match index {
        30..=43 => *[29, 30, 31, 32, 33, 33, 34, 34, 35, 35, 36, 36, 37, 37]
            .get((index - 30) as usize)
            .ok_or(Error::Qp)?,
        44.. => index - 6,
        _ => index,
    })
}

/// Sample depth admitted by Main and Main10.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitDepth(u8);

impl BitDepth {
    /// Checks a depth of 8 or 10 bits.
    pub fn new(bits: u8) -> Result<Self, Error> {
        match bits {
            8 | 10 => Ok(Self(bits)),
            _ => Err(Error::BitDepth),
        }
    }

    /// Returns the number of bits per sample.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns the largest sample value.
    pub const fn max_sample(self) -> u16 {
        (1 << self.0) - 1
    }

    pub(crate) fn clip(self, sample: i64) -> u16 {
        sample.clamp(0, i64::from(self.max_sample())) as u16
    }
}

/// A supported square transform or prediction block size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockSize(u8);

impl BlockSize {
    /// Checks a side length of 4, 8, 16 or 32 samples.
    pub fn new(side: usize) -> Result<Self, Error> {
        match side {
            4 | 8 | 16 | 32 => Ok(Self(side as u8)),
            _ => Err(Error::BlockSize),
        }
    }

    /// Returns the side length in samples.
    pub const fn side(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn log2(self) -> u32 {
        self.0.ilog2()
    }
}

/// A resolved prediction mode, including the caller's chroma-mode derivation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IntraMode(u8);

impl IntraMode {
    /// Checks a mode: planar is 0, DC is 1, angular modes are 2..=34.
    pub fn new(mode: u8) -> Result<Self, Error> {
        if mode <= 34 {
            Ok(Self(mode))
        } else {
            Err(Error::IntraMode)
        }
    }

    /// Returns the normative mode number.
    pub const fn number(self) -> u8 {
        self.0
    }
}

/// Selects luma boundary filtering or 4:2:0 chroma prediction semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Component {
    /// Luma samples.
    Luma,
    /// Either chroma component; both use identical kernel rules.
    Chroma,
}

/// A checked borrowed square block; padding is never read by a kernel.
#[derive(Clone, Copy, Debug)]
pub struct Block<'a, T> {
    pub(crate) data: &'a [T],
    pub(crate) stride: usize,
    pub(crate) size: BlockSize,
}

fn check_buffer(len: usize, stride: usize, size: BlockSize) -> Result<(), Error> {
    let n = size.side();
    let required = (n - 1)
        .checked_mul(stride)
        .and_then(|v| v.checked_add(n))
        .ok_or(Error::Buffer)?;
    if stride < n || len < required {
        return Err(Error::Buffer);
    }
    Ok(())
}

impl<'a, T> Block<'a, T> {
    /// Checks stride and the last active sample, including address overflow.
    pub fn new(data: &'a [T], stride: usize, size: BlockSize) -> Result<Self, Error> {
        check_buffer(data.len(), stride, size)?;
        Ok(Self { data, stride, size })
    }

    /// Returns the checked square size.
    pub const fn size(&self) -> BlockSize {
        self.size
    }
}

impl<T: Copy> Block<'_, T> {
    pub(crate) fn at(&self, x: usize, y: usize) -> Result<T, Error> {
        self.data
            .get(y * self.stride + x)
            .copied()
            .ok_or(Error::Buffer)
    }
}

/// A checked caller-provided output block; padding is left unchanged.
#[derive(Debug)]
pub struct BlockMut<'a, T> {
    pub(crate) data: &'a mut [T],
    pub(crate) stride: usize,
    pub(crate) size: BlockSize,
}

impl<'a, T> BlockMut<'a, T> {
    /// Checks stride and the last active sample, including address overflow.
    pub fn new(data: &'a mut [T], stride: usize, size: BlockSize) -> Result<Self, Error> {
        check_buffer(data.len(), stride, size)?;
        Ok(Self { data, stride, size })
    }

    /// Returns the checked square size.
    pub const fn size(&self) -> BlockSize {
        self.size
    }

    pub(crate) fn put(&mut self, x: usize, y: usize, value: T) -> Result<(), Error> {
        *self
            .data
            .get_mut(y * self.stride + x)
            .ok_or(Error::Buffer)? = value;
        Ok(())
    }
}
