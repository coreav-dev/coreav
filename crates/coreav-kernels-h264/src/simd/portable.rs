//! The stand-in for an architecture without vector kernels, WebAssembly
//! among them.
//!
//! [`crate::Simd::detect`] returns `None` there, so nothing here is ever
//! called; it exists so that the trait is whole on every target the
//! decoder builds for, and every function hands the work to the reference.

use crate::scalar::Scalar;
use crate::views::{Block, BlockMut, EdgeMut};
use crate::{Combine, EdgeParams, Kernels};

/// Whether an 8-wide luma block at a full-sample position goes to the
/// vector kernel; there is none.
pub(super) const LUMA_COPY8_VECTOR: bool = false;

/// Never: there is no vector kernel here.
pub(super) const CHROMA_MC_VECTOR: bool = false;
/// The same.
pub(super) const COMBINE_VECTOR: bool = false;

pub(super) unsafe fn idct8x8_add(dst: BlockMut<'_>, coeffs: &[i32; 64]) {
    Scalar.idct8x8_add(dst, coeffs);
}

pub(super) unsafe fn luma_mc(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    Scalar.luma_mc(dst, src, xf, yf);
}

pub(super) unsafe fn chroma_mc(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    Scalar.chroma_mc(dst, src, xf, yf);
}

pub(super) unsafe fn combine(dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine) {
    Scalar.combine(dst, p0, p1, mode);
}

pub(super) unsafe fn filter_edge(edge: EdgeMut<'_>, params: &EdgeParams) {
    Scalar.filter_edge(edge, params);
}

/// [`crate::Kernels::idct4x4_add_quad`]: no vector kernel here yet, so the
/// reference's blocks one at a time.
pub(super) unsafe fn idct4x4_add_quad(dst: BlockMut<'_>, coeffs: &[[i32; 16]; 4], nz: u8) {
    crate::idct4x4_add_quad_by_blocks(&crate::Scalar, dst, coeffs, nz);
}
