//! SIMD kernels: AVX2 on x86-64, with an AVX-512 tier where the CPU has
//! AVX-512 F, BW, VL and VBMI, and NEON on arm64, behind the same trait
//! as the scalar reference.
//!
//! Design (`docs/design/h264.md`, "Kernels"):
//!
//! - Every kernel gives the same samples as [`Scalar`], bit for bit, for
//!   every input the trait allows (bar C3: the property tests in
//!   `tests/equivalence.rs` and the fuzz targets in `fuzz/`). Arithmetic
//!   keeps the widths the scalar kernels use wherever a narrower lane could
//!   differ.
//! - A shape or a parameter range without a vector path falls back to the
//!   scalar kernel, which keeps the output identical.
//! - The algorithms come from the specification's equations and from the
//!   scalar kernels in `scalar.rs`, never from another decoder's assembly.
//!
//! `unsafe` lives in six places only, and all of them are listed in
//! `docs/unsafe-sites.md`:
//!
//! 1. the loads and stores of `x86::load16` and its neighbours, each of
//!    which checks the length of the slice it reads or writes immediately
//!    before the pointer is formed;
//! 2. the reads and writes of a whole region after one check of its
//!    extent. The deblocking filter's `x86::gather` and `x86::scatter`
//!    check that `n` rows of `w` bytes, the first at `start` and each
//!    `stride` further, end inside the slice (`(n - 1) * stride + start +
//!    w`, checked arithmetic), then make one unaligned read or write of `w`
//!    bytes a row, between an array and the slice; where the check fails,
//!    every row goes through the slice. The luma interpolation
//!    (`x86::luma_mc_into`, which runs `x86::interpolate`) checks the same
//!    of the whole window, `h + 5` rows of `w + 5` samples, and of the
//!    whole block, `h` rows of `w`, before any pointer exists, then reads
//!    and writes each row through a pointer inside those extents; where a
//!    check fails, no pointer is formed and the block goes to the
//!    reference. The three-plane prediction (`x86::predict`) checks the
//!    six regions of a block once (`crate::predict::place`: each window,
//!    from its first sample to its last, lies inside its reference slice,
//!    whether the planes are a whole reference's (`Planes::with_origin`)
//!    or one band's (`Planes::band`), and each block's last row ends
//!    inside its plane), then forms pointers
//!    from those offsets and reads and writes rows through them; where
//!    the check fails, it declines the block before any pointer exists.
//!    The intra kernels (`x86::intra16x16`,
//!    `x86::intra_chroma`, `x86::intra4x4_mb`, `x86::intra8x8_mb`,
//!    `x86::intra_chroma_mb`) check the same of the
//!    block's rows, and that the view reaches back to each neighbour its
//!    flags name (`x86::IntraRows::of`), then read the neighbours and write
//!    the rows through pointers inside those extents; the chroma residual
//!    of a macroblock (`x86::chroma_residual_mb`) checks each plane's block
//!    as the inverse transforms do (`x86::rows_at_mut`). These are the places
//!    where a pointer is formed after the check that justified it rather
//!    than immediately before;
//! 3. the calls into `#[target_feature]` functions, which [`Simd::detect`]
//!    has established the CPU supports;
//! 4. the SSE2 register operations of the x86 chroma interpolation
//!    (`x86::chroma_mc` and its helpers), which are ordinary functions:
//!    SSE2 is part of the x86-64 baseline, so they need no detection, and
//!    they read and write through arrays (`from_le_bytes`, `first_chunk`),
//!    so they form no pointer;
//! 5. the AVX2 register operations of the luma interpolation's helpers
//!    (`x86::Row` and its implementations, `x86::tap6`, `x86::horizontal`,
//!    `x86::five`, `x86::sample`, `x86::interpolate`), `#[inline(always)]`
//!    functions without `#[target_feature]` that are called only from
//!    `x86::luma_mc_into`, which has it, so that they and the intrinsics in
//!    them inline there; each is an `unsafe fn` whose contract is that AVX2
//!    is available;
//! 6. the AVX-512 tier (`avx512`): calls into its `#[target_feature]`
//!    functions, which a `Simd` with its `avx512` flag set proves the CPU
//!    supports, and the luma deblocking filter's standard edges, whose rows
//!    are read and written through the pointer `x86::rows_at_mut` returns
//!    after one check of the whole edge, as in item 2; and the boundary
//!    strengths' reads of a macroblock's motion (`avx512::strengths`), a
//!    `&[Motion; 16]` whose type fixes its 192 bytes, each read ending
//!    inside them.
//!
//! No kernel forms a pointer outside the slices the views (`views.rs`)
//! hand out, or outside the arrays the boundary strengths are handed: the
//! decoder hands a kernel views, whose constructors check every length,
//! stride and range, and the kernel works on their slices.

#![allow(unsafe_code)]
// The register intrinsics are safe to call inside a `#[target_feature]`
// function on the workspace's Rust version (1.89, which the AVX-512 tier's
// target features need) and `unsafe fn` elsewhere, as in the
// `#[inline(always)]` helpers without the attribute. Every call keeps its
// block, so one rule covers both, and the blocks inside the attribute's
// functions look unnecessary to the compiler.
#![allow(unused_unsafe)]

// The tier's target features and intrinsics are stable from Rust 1.89, the
// workspace's version; clippy.toml keeps the version-aware lints of the
// other modules at 1.85.
#[cfg(target_arch = "x86_64")]
#[clippy::msrv = "1.89"]
mod avx512;
#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod portable;
#[cfg(test)]
mod tests;
#[cfg(target_arch = "x86_64")]
mod x86;

#[cfg(target_arch = "aarch64")]
use neon as arch;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
use portable as arch;
#[cfg(target_arch = "x86_64")]
use x86 as arch;

use crate::views::{Block, BlockMut, ChromaIntraMut, ChromaMbMut, EdgeMut, IntraMut, Reach};
use crate::{
    Combine, EdgeParams, Kernels, PlaneOp, Planes, PlanesMut, Scalar, StrengthMb, Weighting,
};

/// A read hint for the cache line that holds `at` (`crate::prefetch`).
/// The reference proves the address lies inside a slice the caller holds.
#[inline(always)]
pub(crate) fn hint_read(at: &u8) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: a prefetch makes no architectural access, cannot fault and
    // changes no value; SSE, which has it, is part of the x86-64 baseline;
    // and the address is that of a live reference.
    unsafe {
        core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(
            core::ptr::from_ref(at).cast(),
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = at;
}

/// A write hint for the cache line that holds `at`. The same instruction
/// as [`hint_read`] for now: `prefetchw` needs the `prfchw` target
/// feature, which the release build does not enable, and the decoder's
/// measurement was of `prefetcht0` for both.
#[inline(always)]
pub(crate) fn hint_write(at: &u8) {
    hint_read(at);
}

/// Whether this CPU has `prefetchw` (CPUID leaf 0x8000_0001, ECX bit 8,
/// PRFCHW), which [`hint_own`] issues. Asked once and kept: CPUID is slow,
/// and in a virtual machine it may leave to the hypervisor.
pub(crate) fn has_prefetchw() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *HAS.get_or_init(|| {
            use core::arch::x86_64::__cpuid;
            // SAFETY: CPUID is part of the x86-64 baseline; leaf 0x8000_0000
            // gives the highest extended leaf, and 0x8000_0001 is read only
            // where it is at most that. (`__cpuid` is safe to call on newer
            // toolchains; the block is for the minimum supported one.)
            unsafe {
                __cpuid(0x8000_0000).eax >= 0x8000_0001 && __cpuid(0x8000_0001).ecx & (1 << 8) != 0
            }
        })
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}

/// An ownership hint for the cache line that holds `at`: `prefetchw`,
/// which fetches the line for writing, so that a line other cores hold
/// shared is taken from them now rather than by the store that follows
/// (`crate::prefetch::Ownership`). Holding an `Ownership`, which only
/// [`has_prefetchw`] grants, is the proof that the CPU has the
/// instruction; it is written out because its target feature, `prfchw`,
/// is not stable to enable. The reference proves the address lies inside
/// a slice the caller holds.
#[inline(always)]
pub(crate) fn hint_own(_: crate::prefetch::Ownership, at: &u8) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: the `Ownership` held proves `has_prefetchw`, so the CPU has
    // the instruction; like any prefetch it makes no architectural access,
    // cannot fault and changes no value, no register but the address
    // operand is named, and no flag or stack slot is touched; the address
    // is that of a live reference.
    unsafe {
        core::arch::asm!(
            "prefetchw [{at}]",
            at = in(reg) core::ptr::from_ref(at),
            options(nostack, preserves_flags, readonly),
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = at;
}

/// Whether a chroma edge down a column goes to the vector kernel.
#[cfg(target_arch = "x86_64")]
const CHROMA_COLUMN_VECTOR: bool = x86::CHROMA_COLUMN_VECTOR;
#[cfg(not(target_arch = "x86_64"))]
const CHROMA_COLUMN_VECTOR: bool = false;

/// The SIMD kernels of this CPU. Build it with [`Simd::detect`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Simd {
    /// Whether the CPU also has AVX-512 F, BW, VL and VBMI, and the
    /// kernels of that tier (`avx512`) are used where they exist. Only
    /// [`Simd::detect`] sets it, so holding a `Simd` with it set is the
    /// proof that the CPU has them.
    avx512: bool,
}

impl Simd {
    /// The SIMD kernels, where this CPU supports them: AVX2 on x86-64,
    /// with the AVX-512 tier on top where the CPU has AVX-512 F, BW, VL
    /// and VBMI, and NEON on arm64, where it is part of the baseline.
    pub fn detect() -> Option<Simd> {
        #[cfg(target_arch = "x86_64")]
        {
            if std::is_x86_feature_detected!("avx2") {
                let avx512 = std::is_x86_feature_detected!("avx512f")
                    && std::is_x86_feature_detected!("avx512bw")
                    && std::is_x86_feature_detected!("avx512vl")
                    && std::is_x86_feature_detected!("avx512vbmi");
                return Some(Simd { avx512 });
            }
            None
        }
        #[cfg(target_arch = "aarch64")]
        {
            Some(Simd { avx512: false })
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            None
        }
    }

    /// The same kernels without the AVX-512 tier: what a CPU with AVX2
    /// alone runs. Tests, fuzz targets and benchmarks use it to reach both
    /// tiers on one machine.
    pub const fn without_avx512(self) -> Simd {
        Simd { avx512: false }
    }

    /// Whether the AVX-512 tier is in use.
    pub const fn has_avx512(&self) -> bool {
        self.avx512
    }
}

impl Simd {
    /// Whether the vector kernel beats the reference for this luma block.
    ///
    /// This and its neighbours hold the measurement of
    /// `docs/measurements/h264-kernels.md`, which [`crate::Auto`] follows;
    /// `Simd` itself always takes the vector path where it has one, so
    /// that a test or a benchmark can reach every kernel.
    pub(crate) fn prefers_luma_mc(w: usize, xf: u8, yf: u8) -> bool {
        // A 4-wide block is faster in the reference, and so is an 8-wide
        // one that is only copied, where the architecture says so.
        matches!(w, 8 | 16) && ((xf & 3, yf & 3) != (0, 0) || w == 16 || arch::LUMA_COPY8_VECTOR)
    }

    /// Whether the vector kernel beats the reference for this chroma
    /// block.
    pub(crate) fn prefers_chroma_mc(w: usize) -> bool {
        arch::CHROMA_MC_VECTOR && matches!(w, 2 | 4 | 8)
    }

    /// Whether the vector kernel beats the reference for this
    /// combination. Copying and averaging are a few instructions a row
    /// either way, and the reference is inlined into the decoder.
    pub(crate) fn prefers_combine(mode: Combine) -> bool {
        arch::COMBINE_VECTOR && matches!(mode, Combine::Uni { .. } | Combine::Bi { .. })
    }

    /// Whether the vector kernel beats the reference for this edge. A
    /// chroma edge down a column, eight lines of four samples, goes to the
    /// vector kernel only where its architecture measured it faster
    /// (`x86::CHROMA_COLUMN_VECTOR`).
    pub(crate) fn prefers_filter_edge(reach: Reach, across: usize) -> bool {
        reach == Reach::Luma || across != 1 || CHROMA_COLUMN_VECTOR
    }
}

impl Kernels for Simd {
    fn idct4x4_add(&self, dst: BlockMut<'_>, coeffs: &[i32; 16]) {
        // There is no vector kernel for a 4x4 block: one measured about
        // twice the reference's time, which a call that cannot be inlined
        // cannot make back (`examples/bench.rs`).
        Scalar.idct4x4_add(dst, coeffs);
    }

    fn idct4x4_dc_add(&self, dst: BlockMut<'_>, dc: i32) {
        Scalar.idct4x4_dc_add(dst, dc);
    }

    fn idct8x8_add(&self, dst: BlockMut<'_>, coeffs: &[i32; 64]) {
        if dst.w() != 8 || dst.h() != 8 {
            return;
        }
        // SAFETY: a `Simd` exists only where the CPU has the features the
        // kernels need (`Simd::detect`).
        unsafe { arch::idct8x8_add(dst, coeffs) }
    }

    fn idct4x4_add_quad(&self, dst: BlockMut<'_>, coeffs: &[[i32; 16]; 4], nz: u8) {
        if dst.w() != 8 || dst.h() != 8 {
            return;
        }
        // SAFETY: a `Simd` exists only where the CPU has the features the
        // kernels need (`Simd::detect`).
        unsafe { arch::idct4x4_add_quad(dst, coeffs, nz) }
    }

    fn intra4x4_mb(&self, dst: IntraMut<'_>, modes: &[u8; 16], coeffs: &[[i32; 16]; 16], nz: u16) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::intra4x4_mb(dst, modes, coeffs, nz)
        }
        #[cfg(not(target_arch = "x86_64"))]
        Scalar.intra4x4_mb(dst, modes, coeffs, nz);
    }

    fn intra8x8_mb(&self, dst: IntraMut<'_>, modes: &[u8; 4], coeffs: &[[i32; 64]; 4], nz: u8) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::intra8x8_mb(dst, modes, coeffs, nz)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::intra::intra8x8_mb(self, dst, modes, coeffs, nz);
    }

    fn intra16x16(&self, dst: IntraMut<'_>, mode: u8) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::intra16x16(dst, mode)
        }
        #[cfg(not(target_arch = "x86_64"))]
        Scalar.intra16x16(dst, mode);
    }

    fn intra_chroma(&self, dst: IntraMut<'_>, mode: u8) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::intra_chroma(dst, mode)
        }
        #[cfg(not(target_arch = "x86_64"))]
        Scalar.intra_chroma(dst, mode);
    }

    fn chroma_residual_mb(
        &self,
        cb: BlockMut<'_>,
        cr: BlockMut<'_>,
        dc: &[[i32; 4]; 2],
        ac: &[[[i32; 16]; 4]; 2],
        ac_nz: u8,
    ) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::chroma_residual_mb([cb, cr], dc, ac, ac_nz)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::chroma_residual_mb_by_parts(self, [cb, cr], dc, ac, ac_nz);
    }

    fn intra_chroma_mb(
        &self,
        dst: ChromaIntraMut<'_>,
        mode: u8,
        dc: &[[i32; 4]; 2],
        ac: &[[[i32; 16]; 4]; 2],
        ac_nz: u8,
    ) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::intra_chroma_mb(dst, mode, dc, ac, ac_nz)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::intra_chroma_mb_by_parts(self, dst, mode, dc, ac, ac_nz);
    }

    fn luma_mc(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 5 || src.h() != h + 5 || !matches!(w, 4 | 8 | 16) || h > 16 {
            return Scalar.luma_mc(dst, src, xf, yf);
        }
        // SAFETY: as above.
        unsafe { arch::luma_mc(dst, src, xf & 3, yf & 3) }
    }

    fn luma_mc_avg(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 5 || src.h() != h + 5 || !matches!(w, 4 | 8 | 16) || h > 16 {
            return Scalar.luma_mc_avg(dst, src, xf, yf);
        }
        // Only the x86 kernels predict and average in one pass; elsewhere
        // the vector prediction is averaged in afterwards.
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::luma_mc_avg(dst, src, xf & 3, yf & 3)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::luma_mc_avg_by_parts(self, dst, src, xf, yf);
    }

    #[inline]
    fn chroma_mc(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 1 || src.h() != h + 1 || !matches!(w, 2 | 4 | 8) || h > 8 {
            return Scalar.chroma_mc(dst, src, xf, yf);
        }
        // The x86 kernel is SSE2, the baseline, and an ordinary function.
        #[cfg(target_arch = "x86_64")]
        arch::chroma_mc(dst, src, xf, yf, arch::Finish::Write);
        #[cfg(not(target_arch = "x86_64"))]
        // SAFETY: as above.
        unsafe {
            arch::chroma_mc(dst, src, xf & 7, yf & 7)
        }
    }

    #[inline]
    fn chroma_mc_avg(&self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 1 || src.h() != h + 1 || !matches!(w, 2 | 4 | 8) || h > 8 {
            return Scalar.chroma_mc_avg(dst, src, xf, yf);
        }
        // Only the x86 kernel predicts and averages in one pass; elsewhere
        // the vector prediction is averaged in afterwards.
        #[cfg(target_arch = "x86_64")]
        arch::chroma_mc(dst, src, xf, yf, arch::Finish::Average);
        #[cfg(not(target_arch = "x86_64"))]
        crate::chroma_mc_avg_by_parts(self, dst, src, xf, yf);
    }

    fn luma_mc_weighted(
        &self,
        dst: BlockMut<'_>,
        src: Block<'_>,
        xf: u8,
        yf: u8,
        weighting: Weighting,
    ) {
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 5
            || src.h() != h + 5
            || !matches!(w, 8 | 16)
            || h > 16
            || !weighting.in_range()
        {
            return crate::luma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting);
        }
        // Only the x86 kernels predict and weight in one pass; elsewhere
        // the vector prediction is weighted afterwards.
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::luma_mc_weighted(dst, src, xf & 3, yf & 3, weighting)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::luma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting);
    }

    #[inline]
    fn chroma_mc_weighted(
        &self,
        dst: BlockMut<'_>,
        src: Block<'_>,
        xf: u8,
        yf: u8,
        weighting: Weighting,
    ) {
        let (w, h) = (dst.w(), dst.h());
        if src.w() != w + 1
            || src.h() != h + 1
            || !matches!(w, 2 | 4 | 8)
            || h > 8
            || !weighting.in_range()
        {
            return crate::chroma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting);
        }
        #[cfg(target_arch = "x86_64")]
        arch::chroma_mc(dst, src, xf, yf, arch::Finish::weigh(weighting));
        #[cfg(not(target_arch = "x86_64"))]
        crate::chroma_mc_weighted_by_parts(self, dst, src, xf, yf, weighting);
    }

    fn predict(
        &self,
        dst: PlanesMut<'_>,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) -> bool {
        // A 4-wide luma block goes where the dispatching kernels send it.
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            let four = crate::Auto { simd: Some(*self) };
            arch::predict::predict(&four, dst, src, px, py, w, h, mv, ops)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::predict::predict_by_parts(self, dst, src, px, py, w, h, mv, ops)
    }

    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    fn predict_inlined(
        &self,
        dst: PlanesMut<'_>,
        src: &Planes<'_>,
        px: usize,
        py: usize,
        w: usize,
        h: usize,
        mv: [i16; 2],
        ops: [PlaneOp; 3],
    ) -> bool {
        let four = crate::Auto { simd: Some(*self) };
        // SAFETY: a `Simd` exists only where the CPU has AVX2 (`detect`),
        // the contract of `predict_body`. Inlined into a caller compiled
        // with AVX2, the body's AVX2 code is inlined with it; into any
        // other, it is called, which the same detection makes sound.
        unsafe { arch::predict::predict_body(&four, dst, src, px, py, w, h, mv, ops) }
    }

    fn combine(&self, dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine) {
        let (w, h) = (dst.w(), dst.h());
        let denominator = match mode {
            Combine::Uni { log_wd, .. } | Combine::Bi { log_wd, .. } => log_wd,
            // There is no vector kernel for these two.
            Combine::Copy(_) | Combine::Average => return Scalar.combine(dst, p0, p1, mode),
        };
        let weights = match mode {
            Combine::Uni { w, o, .. } => [w, o, 0],
            Combine::Bi { w0, w1, o, .. } => [w0, w1, o],
            _ => [0; 3],
        };
        // Outside the ranges of clause 7.4.3.2 the 16-bit lanes of the
        // vector kernel could differ from the reference's arithmetic.
        let in_range = weights.iter().all(|v| (-128..=127).contains(v));
        if p0.w() < w
            || p0.h() < h
            || p1.w() < w
            || p1.h() < h
            || !(0..=7).contains(&denominator)
            || !in_range
        {
            return Scalar.combine(dst, p0, p1, mode);
        }
        // SAFETY: as above.
        unsafe { arch::combine(dst, p0, p1, mode) }
    }

    #[inline(always)]
    fn filter_edge(&self, edge: EdgeMut<'_>, params: &EdgeParams) {
        #[cfg(target_arch = "x86_64")]
        if self.avx512 {
            // SAFETY: the flag is set only where the CPU has the tier's
            // features (`Simd::detect`).
            return unsafe { avx512::filter_edge(edge, params) };
        }
        // SAFETY: as above.
        unsafe { arch::filter_edge(edge, params) }
    }

    fn filter_chroma_pair(&self, cb: EdgeMut<'_>, cr: EdgeMut<'_>, params: &[EdgeParams; 2]) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::filter_chroma_pair(cb, cr, params)
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let [for_cb, for_cr] = params;
            self.filter_edge(cb, for_cb);
            self.filter_edge(cr, for_cr);
        }
    }

    fn filter_chroma_mb(
        &self,
        cb: ChromaMbMut<'_>,
        cr: ChromaMbMut<'_>,
        params: &[[EdgeParams; 2]; 4],
    ) {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: as above.
        unsafe {
            arch::filter_chroma_mb(self, cb, cr, params)
        }
        #[cfg(not(target_arch = "x86_64"))]
        crate::chroma_mb_by_edges(self, cb, cr, params);
    }

    fn strengths(&self, mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
        #[cfg(target_arch = "x86_64")]
        if self.avx512 {
            // SAFETY: the flag is set only where the CPU has the tier's
            // features (`Simd::detect`).
            return unsafe { avx512::strengths(mb) };
        }
        crate::strengths::strengths(mb)
    }
}
