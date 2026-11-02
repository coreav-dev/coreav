//! Pixel kernels of CoreAV's VP9 decoder: the inverse transforms and intra
//! prediction.
//!
//! The kernels are pure arithmetic over samples. They hold no decoder state
//! and read no bitstream: the decoder hands each one a [`BlockMut`] that
//! covers exactly the samples the kernel writes, and building it checked
//! its length and stride. Their other inputs are typed to the domains the
//! specification gives them ([`TxSize`], [`BitDepth`], [`Coefficients`]),
//! so no value outside them reaches the arithmetic. These are the scalar reference kernels, written
//! from clauses 8.5.1 and 8.7 of the VP9 Bitstream & Decoding Process
//! Specification (version 0.7); faster implementations come later and are
//! compared with them bit for bit (`docs/design/vp9.md`, milestone 6).
//!
//! ```
//! use coreav_kernels_vp9::{
//!     BitDepth, BlockMut, Coefficients, TxSize, TxType, inverse_transform_add,
//! };
//!
//! let mut plane = vec![100u8; 4 * 4];
//! let block = BlockMut::new(&mut plane, 0, 4, TxSize::Tx4x4).expect("in range");
//! let mut values = [0i64; 16];
//! values[0] = 32;
//! let coefs = Coefficients::new(&mut values, BitDepth::Eight).expect("in the domain");
//! // A 4x4 DCT whose only coefficient is a DC of 32 raises every sample by 1.
//! inverse_transform_add(block, coefs, TxType::DctDct, false);
//! assert_eq!(plane, [101u8; 16]);
//! ```

#![forbid(unsafe_code)]

mod intra;
mod tables;
mod transform;

pub use intra::{Edges, IntraMode, predict_intra};
pub use transform::{TxType, inverse_transform_add};

/// `BitDepth`: 8, 10 or 12 bits per sample, the only depths VP9 has
/// (clause 7.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
    /// 8 bits.
    Eight,
    /// 10 bits.
    Ten,
    /// 12 bits.
    Twelve,
}

impl BitDepth {
    /// The depth of `bits` bits, or `None` unless it is 8, 10 or 12.
    pub fn from_bits(bits: u8) -> Option<Self> {
        Some(match bits {
            8 => BitDepth::Eight,
            10 => BitDepth::Ten,
            12 => BitDepth::Twelve,
            _ => return None,
        })
    }

    /// Its number of bits.
    pub fn bits(self) -> u32 {
        match self {
            BitDepth::Eight => 8,
            BitDepth::Ten => 10,
            BitDepth::Twelve => 12,
        }
    }
}

/// A sample: `u8` for 8-bit video, `u16` for 10 and 12 bits.
pub trait Pixel: Copy + Default + core::fmt::Debug + PartialEq + 'static {
    /// The sample's value.
    fn get(self) -> i64;
    /// `Clip1(v)` at `bit_depth`: `v` clamped to 0 and
    /// `(1 << BitDepth) - 1`, and to the type's range.
    fn clip(v: i64, bit_depth: BitDepth) -> Self;
}

impl Pixel for u8 {
    fn get(self) -> i64 {
        i64::from(self)
    }

    fn clip(v: i64, _bit_depth: BitDepth) -> Self {
        v.clamp(0, 255) as u8
    }
}

impl Pixel for u16 {
    fn get(self) -> i64 {
        i64::from(self)
    }

    fn clip(v: i64, bit_depth: BitDepth) -> Self {
        v.clamp(0, (1 << bit_depth.bits()) - 1) as u16
    }
}

/// The dequantized coefficients of a transform block (`Dequant`, clause
/// 8.6.2), in raster order. Each is representable by a signed integer of
/// `8 + BitDepth` bits, as the clause requires of a conformant stream; in
/// that domain no step of the inverse transforms overflows (the test
/// `no_coefficient_in_the_domain_can_overflow` bounds every intermediate).
#[derive(Debug)]
pub struct Coefficients<'a> {
    values: &'a mut [i64],
    bit_depth: BitDepth,
}

impl<'a> Coefficients<'a> {
    /// `values` at `bit_depth`, or `None` when one is outside the domain.
    pub fn new(values: &'a mut [i64], bit_depth: BitDepth) -> Option<Self> {
        let limit = 1i64 << (7 + bit_depth.bits());
        values
            .iter()
            .all(|v| (-limit..limit).contains(v))
            .then_some(Coefficients { values, bit_depth })
    }
}

/// The size of a transform block, `tx_size` (clause 7.4.8, `TX_4X4` to
/// `TX_32X32`): the only sizes the kernels take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxSize {
    /// 4 by 4 samples.
    Tx4x4,
    /// 8 by 8 samples.
    Tx8x8,
    /// 16 by 16 samples.
    Tx16x16,
    /// 32 by 32 samples.
    Tx32x32,
}

impl TxSize {
    /// The size a `tx_size` of 0 to 3 names, or `None`.
    pub fn from_value(value: u8) -> Option<Self> {
        Some(match value {
            0 => TxSize::Tx4x4,
            1 => TxSize::Tx8x8,
            2 => TxSize::Tx16x16,
            3 => TxSize::Tx32x32,
            _ => return None,
        })
    }

    /// The base 2 logarithm of its width, `log2Size` (2 to 5).
    pub fn log2(self) -> u32 {
        2 + self as u32
    }

    /// Its width and height in samples.
    pub fn samples(self) -> usize {
        1 << self.log2()
    }
}

/// A square transform block in a plane whose rows are `stride` samples
/// apart.
#[derive(Debug)]
pub struct BlockMut<'a, P> {
    data: &'a mut [P],
    stride: usize,
    tx_size: TxSize,
}

impl<'a, P: Pixel> BlockMut<'a, P> {
    /// The block of `tx_size` whose top left sample is `plane[offset]`, or
    /// `None` when its rows would not fit in `plane` or would overlap.
    pub fn new(plane: &'a mut [P], offset: usize, stride: usize, tx_size: TxSize) -> Option<Self> {
        let size = tx_size.samples();
        if stride < size {
            return None;
        }
        let end = offset.checked_add((size - 1).checked_mul(stride)?.checked_add(size)?)?;
        let data = plane.get_mut(offset..end)?;
        Some(BlockMut {
            data,
            stride,
            tx_size,
        })
    }

    /// Its size.
    pub fn tx_size(&self) -> TxSize {
        self.tx_size
    }

    /// Its width and height in samples.
    pub fn size(&self) -> usize {
        self.tx_size.samples()
    }

    fn row_mut(&mut self, i: usize) -> &mut [P] {
        let start = i * self.stride;
        let size = self.size();
        self.data.get_mut(start..start + size).unwrap_or_default()
    }
}

/// `Round2(x, n)` of clause 2.
fn round2(x: i64, n: u32) -> i64 {
    (x + (1 << (n - 1))) >> n
}
