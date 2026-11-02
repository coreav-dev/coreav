//! Pixel kernels of CoreAV's VP9 decoder: the inverse transforms and intra
//! prediction.
//!
//! The kernels are pure arithmetic over samples. They hold no decoder state
//! and read no bitstream: the decoder hands each one a [`BlockMut`] that
//! covers exactly the samples the kernel writes, and building it checked
//! its length and stride. These are the scalar reference kernels, written
//! from clauses 8.5.1 and 8.7 of the VP9 Bitstream & Decoding Process
//! Specification (version 0.7); faster implementations come later and are
//! compared with them bit for bit (`docs/design/vp9.md`, milestone 6).
//!
//! ```
//! use coreav_kernels_vp9::{BlockMut, TxType, inverse_transform_add};
//!
//! let mut plane = vec![100u8; 4 * 4];
//! let block = BlockMut::new(&mut plane, 0, 4, 4).expect("in range");
//! let mut coefs = [0i64; 16];
//! coefs[0] = 32;
//! // A 4x4 DCT whose only coefficient is a DC of 32 raises every sample by 1.
//! inverse_transform_add(block, &mut coefs, TxType::DctDct, false, 8);
//! assert_eq!(plane, [101u8; 16]);
//! ```

#![forbid(unsafe_code)]

mod intra;
mod tables;
mod transform;

pub use intra::{Edges, IntraMode, predict_intra};
pub use transform::{TxType, inverse_transform_add};

/// A sample: `u8` for 8-bit video, `u16` for 10 and 12 bits.
pub trait Pixel: Copy + Default + core::fmt::Debug + PartialEq + 'static {
    /// The sample's value.
    fn get(self) -> i64;
    /// `Clip1(v)` at `bit_depth`: `v` clamped to 0 and `(1 << bit_depth) - 1`.
    fn clip(v: i64, bit_depth: u8) -> Self;
}

impl Pixel for u8 {
    fn get(self) -> i64 {
        i64::from(self)
    }

    fn clip(v: i64, _bit_depth: u8) -> Self {
        v.clamp(0, 255) as u8
    }
}

impl Pixel for u16 {
    fn get(self) -> i64 {
        i64::from(self)
    }

    fn clip(v: i64, bit_depth: u8) -> Self {
        v.clamp(0, (1i64 << bit_depth.min(16)) - 1) as u16
    }
}

/// A square block of `size` by `size` samples in a plane whose rows are
/// `stride` samples apart.
#[derive(Debug)]
pub struct BlockMut<'a, P> {
    data: &'a mut [P],
    stride: usize,
    size: usize,
}

impl<'a, P: Pixel> BlockMut<'a, P> {
    /// The block whose top left sample is `plane[offset]`, or `None` when
    /// its rows would not fit in `plane` or would overlap.
    pub fn new(plane: &'a mut [P], offset: usize, stride: usize, size: usize) -> Option<Self> {
        if size == 0 || stride < size {
            return None;
        }
        let end = offset.checked_add((size - 1).checked_mul(stride)?.checked_add(size)?)?;
        let data = plane.get_mut(offset..end)?;
        Some(BlockMut { data, stride, size })
    }

    /// Its width and height in samples.
    pub fn size(&self) -> usize {
        self.size
    }

    fn row_mut(&mut self, i: usize) -> &mut [P] {
        let start = i * self.stride;
        self.data
            .get_mut(start..start + self.size)
            .unwrap_or_default()
    }
}

/// `Round2(x, n)` of clause 2.
fn round2(x: i64, n: u32) -> i64 {
    (x + (1 << (n - 1))) >> n
}
