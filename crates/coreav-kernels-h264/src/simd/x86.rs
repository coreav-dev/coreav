//! The x86-64 kernels, written with AVX2 intrinsics.
//!
//! Every function here but the chroma interpolation needs AVX2, which
//! only [`crate::simd::Simd::detect`] establishes, so each one is an
//! `unsafe fn` with `#[target_feature]`, or a helper inlined only into
//! one. The loads and stores go through the
//! helpers at the top of the file, which check the length of the slice
//! they are given before they form a pointer; through `gather` and
//! `scatter`, which check the extent of a whole edge once before they form
//! a pointer per row; through the luma interpolation (`luma_mc_into`),
//! which checks the extent of the whole window and the whole block once
//! (`window_fits`) before it forms a pointer per row; through the
//! three-plane prediction (`predict`), which forms its pointers after
//! `crate::predict::place`'s one check of a block's regions; and through
//! `rows_at` and `rows_at_mut`, the same check for the deblocking filter's
//! standard edges and the inverse transforms' 8x8 blocks.
//! Nothing else in the file touches memory through a pointer.
//!
//! The chroma interpolation (`chroma_mc`) uses SSE2 alone, which every
//! x86-64 CPU has, so it is an ordinary function that inlines into the
//! dispatch. It reads and writes samples as arrays and moves them into
//! and out of registers as integers; its `unsafe` is register operations.

use core::arch::x86_64::{
    __m128i, __m256i, _mm_abs_epi16, _mm_add_epi16, _mm_add_epi32, _mm_adds_epi16, _mm_and_si128,
    _mm_avg_epu8, _mm_blendv_epi8, _mm_castpd_si128, _mm_castsi128_pd, _mm_cmpgt_epi16,
    _mm_cvtepu8_epi16, _mm_cvtsi32_si128, _mm_cvtsi64_si128, _mm_cvtsi128_si32, _mm_cvtsi128_si64,
    _mm_extract_epi32, _mm_extract_epi64, _mm_insert_epi8, _mm_insert_epi64, _mm_loadl_epi64,
    _mm_loadu_si128, _mm_madd_epi16, _mm_maddubs_epi16, _mm_max_epi16, _mm_min_epi16,
    _mm_mulhrs_epi16, _mm_mullo_epi16, _mm_or_si128, _mm_packs_epi32, _mm_packus_epi16,
    _mm_sad_epu8, _mm_set_epi64x, _mm_set1_epi8, _mm_set1_epi16, _mm_set1_epi32, _mm_setr_epi8,
    _mm_setr_epi16, _mm_setr_epi32, _mm_setzero_si128, _mm_shuffle_epi8, _mm_shuffle_pd,
    _mm_slli_epi16, _mm_sra_epi16, _mm_sra_epi32, _mm_srai_epi16, _mm_srli_epi16, _mm_srli_si128,
    _mm_storel_epi64, _mm_storeu_si128, _mm_sub_epi8, _mm_sub_epi16, _mm_testz_si128,
    _mm_unpackhi_epi8, _mm_unpackhi_epi16, _mm_unpackhi_epi32, _mm_unpackhi_epi64,
    _mm_unpacklo_epi8, _mm_unpacklo_epi16, _mm_unpacklo_epi32, _mm_unpacklo_epi64, _mm_xor_si128,
    _mm256_abs_epi16, _mm256_add_epi16, _mm256_add_epi32, _mm256_adds_epi16, _mm256_alignr_epi8,
    _mm256_and_si256, _mm256_avg_epu8, _mm256_blend_epi32, _mm256_blendv_epi8,
    _mm256_broadcastsi128_si256, _mm256_castsi256_si128, _mm256_cmpeq_epi8, _mm256_cmpeq_epi32,
    _mm256_cmpgt_epi16, _mm256_cvtepu8_epi16, _mm256_cvtepu8_epi32, _mm256_extract_epi32,
    _mm256_extracti128_si256, _mm256_loadu_si256, _mm256_madd_epi16, _mm256_maddubs_epi16,
    _mm256_max_epi16, _mm256_min_epi16, _mm256_mulhrs_epi16, _mm256_mullo_epi16,
    _mm256_mullo_epi32, _mm256_or_si256, _mm256_packs_epi32, _mm256_packus_epi16,
    _mm256_packus_epi32, _mm256_permute2x128_si256, _mm256_permute4x64_epi64,
    _mm256_permutevar8x32_epi32, _mm256_sad_epu8, _mm256_set_m128i, _mm256_set1_epi8,
    _mm256_set1_epi16, _mm256_set1_epi32, _mm256_setr_epi8, _mm256_setr_epi16, _mm256_setr_epi32,
    _mm256_setzero_si256, _mm256_shuffle_epi8, _mm256_slli_epi16, _mm256_sra_epi32,
    _mm256_srai_epi16, _mm256_srai_epi32, _mm256_srli_epi32, _mm256_sub_epi8, _mm256_sub_epi16,
    _mm256_sub_epi32, _mm256_testc_si256, _mm256_testz_si256, _mm256_unpackhi_epi32,
    _mm256_unpackhi_epi64, _mm256_unpacklo_epi32, _mm256_unpacklo_epi64, _mm256_xor_si256,
};

use crate::scalar::{Tap, taps};

pub(super) mod chroma_mb;
pub(super) mod predict;

pub(super) use chroma_mb::filter_chroma_mb;

/// Whether an 8-wide luma block at a full-sample position goes to the
/// vector kernel: yes, since the kernels by position copy a row with one
/// load and one store (`docs/design/h264.md`, "Kernel round: luma
/// interpolation by position").
pub(super) const LUMA_COPY8_VECTOR: bool = true;

/// Whether a 4-wide luma block at a fractional position goes to the
/// vector kernel: yes, through the 8-wide one on copies (`luma4_padded`).
pub(super) const LUMA4_VECTOR: bool = true;

/// Whether a chroma block at a fractional position goes to the vector
/// kernel, `chroma_mc`: SSE2, inlined into the dispatch
/// (`docs/design/h264.md`, "Kernel round: chroma interpolation
/// inlined"). An AVX2 kernel behind a call lost to the inlined reference
/// (`docs/measurements/h264-kernels.md`).
pub(super) const CHROMA_MC_VECTOR: bool = true;

/// Whether a chroma edge down a column goes to the vector kernel
/// (`filter_chroma_edge`). Measured on the training set,
/// `docs/measurements/h264-kernels-training.md`.
pub(super) const CHROMA_COLUMN_VECTOR: bool = true;

/// Whether a weighted prediction goes to the vector kernel: yes, measured
/// at 0.93 (`docs/measurements/h264-kernels.md`).
pub(super) const COMBINE_VECTOR: bool = true;
use crate::views::{Block, BlockMut, ChromaIntraMut, EdgeMut, IntraAvail, IntraMut, Reach};
use crate::{Combine, EdgeParams, Kernels, Scalar, Weighting};

// ---------------------------------------------------------------------
// Loads and stores. Each one checks the slice it is handed; they,
// `gather` and `scatter`, the luma interpolation, and `rows_at` and
// `rows_at_mut` (the deblocking filter's standard edges and the inverse
// transforms), which check a whole edge's, window's or block's extent
// once, are the only places in this file where a pointer is formed.
// ---------------------------------------------------------------------

/// Copies `from` into `to` without a call: the lengths here are one
/// vector at most, where a `copy_from_slice` would leave a `memcpy`.
#[inline(always)]
fn copy_small(to: &mut [u8], from: &[u8]) {
    for (d, s) in to.iter_mut().zip(from.iter()) {
        *d = *s;
    }
}

/// Sixteen bytes of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load16(src: &[u8]) -> __m128i {
    if src.len() >= 16 {
        // SAFETY: the slice holds the 16 bytes the load reads.
        unsafe { _mm_loadu_si128(src.as_ptr().cast()) }
    } else {
        let mut padded = [0u8; 16];
        copy_small(&mut padded, src);
        // SAFETY: `padded` is 16 bytes long.
        unsafe { _mm_loadu_si128(padded.as_ptr().cast()) }
    }
}

/// Eight bytes of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load8(src: &[u8]) -> __m128i {
    if src.len() >= 8 {
        // SAFETY: the slice holds the 8 bytes the load reads.
        unsafe { _mm_loadl_epi64(src.as_ptr().cast()) }
    } else {
        let mut padded = [0u8; 8];
        copy_small(&mut padded, src);
        // SAFETY: `padded` is 8 bytes long.
        unsafe { _mm_loadl_epi64(padded.as_ptr().cast()) }
    }
}

/// Writes the first `count` bytes of `v`, or as many as `dst` holds.
#[inline(always)]
fn store_bytes(dst: &mut [u8], v: __m128i, count: usize) {
    let n = count.min(dst.len());
    if n >= 16 {
        // SAFETY: the slice holds the 16 bytes the store writes.
        unsafe { _mm_storeu_si128(dst.as_mut_ptr().cast(), v) }
        return;
    }
    if n >= 8 {
        // SAFETY: the slice holds the 8 bytes the store writes.
        unsafe { _mm_storel_epi64(dst.as_mut_ptr().cast(), v) }
        if n == 8 {
            return;
        }
    }
    let mut bytes = [0u8; 16];
    // SAFETY: `bytes` is 16 bytes long.
    unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), v) }
    copy_small(dst.get_mut(..n).unwrap_or_default(), &bytes);
}

/// Four 32-bit values of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load4_i32(src: &[i32]) -> __m128i {
    if src.len() >= 4 {
        // SAFETY: the slice holds the four values the load reads.
        unsafe { _mm_loadu_si128(src.as_ptr().cast()) }
    } else {
        let mut padded = [0i32; 4];
        for (d, s) in padded.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        // SAFETY: `padded` is four values long.
        unsafe { _mm_loadu_si128(padded.as_ptr().cast()) }
    }
}

// ---------------------------------------------------------------------
// The inverse transforms (clauses 8.5.12 and 8.5.13). A single 4x4 block
// is faster in the reference, inlined into the decoder, than called
// (`examples/bench.rs`); four of them in one call are not
// (`idct4x4_add_quad`).
//
// Both kernels keep one row of the 8x8 block they add to in one 256-bit
// vector of eight 32-bit values: an 8x8 block's row, or the rows of a
// horizontal pair of 4x4 blocks side by side, the left block's in the low
// 128-bit lane. The unpacks of a transpose work within a lane, so a 4x4
// transpose transposes both blocks of a pair at once. The rows are added
// to the block four at a time (`add_four_rows`), read and written through
// a pointer after one check of the block's extent, as the deblocking
// filter's standard edges are.
// ---------------------------------------------------------------------

/// Eight 32-bit values of `src`, zero-padded where `src` is shorter.
///
/// # Safety
///
/// AVX must be available: called only from `idct8x8`, inlined into
/// functions that enable AVX2.
#[inline(always)]
unsafe fn load8_i32(src: &[i32]) -> __m256i {
    if src.len() >= 8 {
        // SAFETY: AVX is available (the caller's contract), and the slice
        // holds the eight values the load reads.
        unsafe { _mm256_loadu_si256(src.as_ptr().cast()) }
    } else {
        let mut padded = [0i32; 8];
        for (d, s) in padded.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        // SAFETY: AVX is available (the caller's contract), and `padded`
        // is eight values long.
        unsafe { _mm256_loadu_si256(padded.as_ptr().cast()) }
    }
}

/// Transposes, in each 128-bit lane, the four vectors' four 32-bit values.
///
/// # Safety
///
/// AVX2 must be available; called only from the transforms below, which
/// enable it.
#[inline(always)]
unsafe fn transpose4x2(rows: [__m256i; 4]) -> [__m256i; 4] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let [r0, r1, r2, r3] = rows;
        let a = _mm256_unpacklo_epi32(r0, r1);
        let b = _mm256_unpackhi_epi32(r0, r1);
        let c = _mm256_unpacklo_epi32(r2, r3);
        let d = _mm256_unpackhi_epi32(r2, r3);
        [
            _mm256_unpacklo_epi64(a, c),
            _mm256_unpackhi_epi64(a, c),
            _mm256_unpacklo_epi64(b, d),
            _mm256_unpackhi_epi64(b, d),
        ]
    }
}

/// Transposes eight vectors of eight 32-bit values: in each lane four by
/// four, then the lanes' halves exchanged.
///
/// # Safety
///
/// As [`transpose4x2`].
#[inline(always)]
unsafe fn transpose8(rows: [__m256i; 8]) -> [__m256i; 8] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let [r0, r1, r2, r3, r4, r5, r6, r7] = rows;
        // Vector j of the first four holds columns j and j + 4 of rows 0
        // to 3, one a lane; of the last four, the same of rows 4 to 7.
        let [t0, t1, t2, t3] = transpose4x2([r0, r1, r2, r3]);
        let [b0, b1, b2, b3] = transpose4x2([r4, r5, r6, r7]);
        [
            _mm256_permute2x128_si256::<0x20>(t0, b0),
            _mm256_permute2x128_si256::<0x20>(t1, b1),
            _mm256_permute2x128_si256::<0x20>(t2, b2),
            _mm256_permute2x128_si256::<0x20>(t3, b3),
            _mm256_permute2x128_si256::<0x31>(t0, b0),
            _mm256_permute2x128_si256::<0x31>(t1, b1),
            _mm256_permute2x128_si256::<0x31>(t2, b2),
            _mm256_permute2x128_si256::<0x31>(t3, b3),
        ]
    }
}

/// One pass of the inverse 4x4 transform (Equations 8-338 to 8-345) over
/// four vectors.
///
/// # Safety
///
/// As [`transpose4x2`].
#[inline(always)]
unsafe fn idct4_pass(x: [__m256i; 4]) -> [__m256i; 4] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let [x0, x1, x2, x3] = x;
        let e0 = _mm256_add_epi32(x0, x2);
        let e1 = _mm256_sub_epi32(x0, x2);
        let e2 = _mm256_sub_epi32(_mm256_srai_epi32::<1>(x1), x3);
        let e3 = _mm256_add_epi32(x1, _mm256_srai_epi32::<1>(x3));
        [
            _mm256_add_epi32(e0, e3),
            _mm256_add_epi32(e1, e2),
            _mm256_sub_epi32(e1, e2),
            _mm256_sub_epi32(e0, e3),
        ]
    }
}

/// One pass of the inverse 8x8 transform (Equations 8-351 to 8-366) over
/// eight vectors.
///
/// # Safety
///
/// As [`transpose4x2`].
#[inline(always)]
unsafe fn idct8_pass(x: [__m256i; 8]) -> [__m256i; 8] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let [x0, x1, x2, x3, x4, x5, x6, x7] = x;
        let e0 = _mm256_add_epi32(x0, x4);
        let e1 = _mm256_sub_epi32(
            _mm256_sub_epi32(_mm256_sub_epi32(x5, x3), x7),
            _mm256_srai_epi32::<1>(x7),
        );
        let e2 = _mm256_sub_epi32(x0, x4);
        let e3 = _mm256_sub_epi32(
            _mm256_sub_epi32(_mm256_add_epi32(x1, x7), x3),
            _mm256_srai_epi32::<1>(x3),
        );
        let e4 = _mm256_sub_epi32(_mm256_srai_epi32::<1>(x2), x6);
        let e5 = _mm256_add_epi32(
            _mm256_add_epi32(_mm256_sub_epi32(x7, x1), x5),
            _mm256_srai_epi32::<1>(x5),
        );
        let e6 = _mm256_add_epi32(x2, _mm256_srai_epi32::<1>(x6));
        let e7 = _mm256_add_epi32(
            _mm256_add_epi32(_mm256_add_epi32(x3, x5), x1),
            _mm256_srai_epi32::<1>(x1),
        );
        let f0 = _mm256_add_epi32(e0, e6);
        let f1 = _mm256_add_epi32(e1, _mm256_srai_epi32::<2>(e7));
        let f2 = _mm256_add_epi32(e2, e4);
        let f3 = _mm256_add_epi32(e3, _mm256_srai_epi32::<2>(e5));
        let f4 = _mm256_sub_epi32(e2, e4);
        let f5 = _mm256_sub_epi32(_mm256_srai_epi32::<2>(e3), e5);
        let f6 = _mm256_sub_epi32(e0, e6);
        let f7 = _mm256_sub_epi32(e7, _mm256_srai_epi32::<2>(e1));
        [
            _mm256_add_epi32(f0, f7),
            _mm256_add_epi32(f2, f5),
            _mm256_add_epi32(f4, f3),
            _mm256_add_epi32(f6, f1),
            _mm256_sub_epi32(f6, f1),
            _mm256_sub_epi32(f4, f3),
            _mm256_sub_epi32(f2, f5),
            _mm256_sub_epi32(f0, f7),
        ]
    }
}

/// Adds four rows of residual, before rounding, to the four rows of eight
/// samples from `at`, `stride` apart, with rounding and clipping
/// ([`add_residual8`]).
///
/// # Safety
///
/// AVX2 must be available, and the four rows of eight bytes from `at`,
/// `stride` apart, must lie inside one slice the caller may write
/// ([`rows_at_mut`]).
#[inline(always)]
unsafe fn add_four_rows(at: *mut u8, stride: usize, rows: [__m256i; 4]) {
    // SAFETY: AVX2 is available and every row lies inside the checked
    // extent (the caller's contract); the accesses are unaligned loads and
    // stores of eight bytes from the start of a row.
    unsafe {
        let [rows01, rows23] = add_residual8(load_four_rows(at, stride), rows);
        store_four_rows(at, stride, rows01, rows23);
    }
}

/// The four rows of eight samples from `at`, `stride` apart: rows 0 and 1,
/// then rows 2 and 3.
///
/// # Safety
///
/// As [`add_four_rows`], for reading.
#[inline(always)]
unsafe fn load_four_rows(at: *const u8, stride: usize) -> [__m128i; 2] {
    // SAFETY: AVX2 is available and every row lies inside the checked
    // extent (the caller's contract); the accesses are unaligned loads of
    // eight bytes from the start of a row.
    unsafe {
        [
            _mm_insert_epi64::<1>(
                _mm_loadl_epi64(at.cast()),
                at.add(stride).cast::<i64>().read_unaligned(),
            ),
            _mm_insert_epi64::<1>(
                _mm_loadl_epi64(at.add(2 * stride).cast()),
                at.add(3 * stride).cast::<i64>().read_unaligned(),
            ),
        ]
    }
}

/// Four rows of eight samples, `pred` (rows 0 and 1, then rows 2 and 3,
/// each row's samples in order), with four rows of residual before
/// rounding added, rounded and clipped (Equation 8-369 and Clip1), in the
/// same order.
///
/// The residual is rounded in 32 bits and packed to 16 with signed
/// saturation, the prediction added with signed saturation, and the sums
/// packed to bytes with unsigned saturation. A rounded residual the 16-bit
/// pack changes is above 32,767 or below -32,768, and with any prediction
/// from 0 to 255 its sum stays above 255 or below 0, as the clamped
/// value's does; the other steps clamp to ranges that hold 0 to 255. So
/// the result is `Clip1(p + r)` for every rounded 32-bit residual `r`.
///
/// # Safety
///
/// AVX2 must be available; inlined only into functions that enable it.
#[inline(always)]
unsafe fn add_residual8(pred: [__m128i; 2], rows: [__m256i; 4]) -> [__m128i; 2] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let round = _mm256_set1_epi32(32);
        let [r0, r1, r2, r3] = rows;
        let r0 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r0, round));
        let r1 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r1, round));
        let r2 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r2, round));
        let r3 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r3, round));
        // In each pair, lane 0 holds the two rows' first four samples and
        // lane 1 their last four.
        let res01 = _mm256_packs_epi32(r0, r1);
        let res23 = _mm256_packs_epi32(r2, r3);
        // Two rows of samples, `[a0..a7, b0..b7]`, put in the residual's
        // order, `[a0..a3, b0..b3, a4..a7, b4..b7]`, and widened.
        let order = _mm_setr_epi8(0, 1, 2, 3, 8, 9, 10, 11, 4, 5, 6, 7, 12, 13, 14, 15);
        let [pred01, pred23] = pred;
        let sum01 = _mm256_adds_epi16(res01, _mm256_cvtepu8_epi16(_mm_shuffle_epi8(pred01, order)));
        let sum23 = _mm256_adds_epi16(res23, _mm256_cvtepu8_epi16(_mm_shuffle_epi8(pred23, order)));
        // Lane 0: the first four samples of rows 0 to 3; lane 1: the last
        // four.
        let bytes = _mm256_packus_epi16(sum01, sum23);
        let first = _mm256_castsi256_si128(bytes);
        let last = _mm256_extracti128_si256::<1>(bytes);
        [
            _mm_unpacklo_epi32(first, last),
            _mm_unpackhi_epi32(first, last),
        ]
    }
}

/// Writes four rows of eight samples from `at`, `stride` apart: rows 0
/// and 1 from `rows01`, rows 2 and 3 from `rows23`.
///
/// # Safety
///
/// AVX2 must be available, and the four rows of eight bytes from `at`,
/// `stride` apart, must lie inside one slice the caller may write: the
/// extent [`rows_at_mut`] checked, or a block's rows inside the extent
/// `IntraRows::of` checked.
#[inline(always)]
unsafe fn store_four_rows(at: *mut u8, stride: usize, rows01: __m128i, rows23: __m128i) {
    // SAFETY: AVX2 is available and every row lies inside the checked
    // extent (the caller's contract); the accesses are unaligned stores of
    // eight bytes from the start of a row.
    unsafe {
        _mm_storel_epi64(at.cast(), rows01);
        at.add(stride)
            .cast::<i64>()
            .write_unaligned(_mm_extract_epi64::<1>(rows01));
        _mm_storel_epi64(at.add(2 * stride).cast(), rows23);
        at.add(3 * stride)
            .cast::<i64>()
            .write_unaligned(_mm_extract_epi64::<1>(rows23));
    }
}

/// The inverse 4x4 transforms of a horizontal pair of blocks, before
/// rounding: vector `y` holds row `y` of `left` in its low lane and of
/// `right` in its high lane.
///
/// The horizontal pass needs a row's values spread over the vectors, so
/// the pair is transposed, passed, transposed back and passed again
/// vertically, rows first as the clause has it.
///
/// # Safety
///
/// As [`transpose4x2`].
#[inline(always)]
unsafe fn idct4_pair(left: &[i32; 16], right: &[i32; 16], keep: Option<__m256i>) -> [__m256i; 4] {
    // SAFETY: AVX2 is available (the caller's contract).
    unsafe { idct4_rows(pair_rows(left, right), keep) }
}

/// The coefficients of a horizontal pair of blocks, as [`idct4_pair`]
/// transforms them: vector `y` holds row `y` of `left` in its low lane and
/// of `right` in its high lane.
///
/// # Safety
///
/// AVX2 must be available; inlined only into functions that enable it.
#[inline(always)]
unsafe fn pair_rows(left: &[i32; 16], right: &[i32; 16]) -> [__m256i; 4] {
    // SAFETY: AVX2 is available (the caller's contract); the loads check
    // their slices.
    unsafe {
        let row = |y: usize| {
            [
                load4_i32(left.get(y * 4..y * 4 + 4).unwrap_or_default()),
                load4_i32(right.get(y * 4..y * 4 + 4).unwrap_or_default()),
            ]
        };
        let ([l0, r0], [l1, r1], [l2, r2], [l3, r3]) = (row(0), row(1), row(2), row(3));
        [
            _mm256_set_m128i(r0, l0),
            _mm256_set_m128i(r1, l1),
            _mm256_set_m128i(r2, l2),
            _mm256_set_m128i(r3, l3),
        ]
    }
}

/// [`idct4_pair`] of rows already loaded, each lane masked by `keep`
/// first where it is given.
///
/// # Safety
///
/// As [`transpose4x2`].
#[inline(always)]
unsafe fn idct4_rows(rows: [__m256i; 4], keep: Option<__m256i>) -> [__m256i; 4] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let rows = match keep {
            Some(mask) => {
                let [r0, r1, r2, r3] = rows;
                [
                    _mm256_and_si256(r0, mask),
                    _mm256_and_si256(r1, mask),
                    _mm256_and_si256(r2, mask),
                    _mm256_and_si256(r3, mask),
                ]
            }
            None => rows,
        };
        let rows = transpose4x2(idct4_pass(transpose4x2(rows)));
        idct4_pass(rows)
    }
}

/// The lanes of a horizontal pair to keep for `bits` (1 for the left
/// block, 2 for the right): `None` where both are kept.
///
/// # Safety
///
/// AVX2 must be available; inlined only into functions that enable it.
#[inline(always)]
unsafe fn pair_keep(bits: u8) -> Option<__m256i> {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        if bits == 3 {
            return None;
        }
        let lanes = _mm256_setr_epi32(1, 1, 1, 1, 2, 2, 2, 2);
        let set = _mm256_and_si256(_mm256_set1_epi32(i32::from(bits)), lanes);
        Some(_mm256_cmpeq_epi32(set, lanes))
    }
}

/// [`crate::Kernels::idct4x4_add_quad`]: the four 4x4 blocks of an 8x8
/// block, a horizontal pair at a time, each pair's four rows added as rows
/// of eight. A pair whose bits in `nz` are both clear is skipped; a block
/// whose bit is clear in a pair that is not has its coefficients masked to
/// zero, which transform to a residual of zero.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn idct4x4_add_quad(mut dst: BlockMut<'_>, coeffs: &[[i32; 16]; 4], nz: u8) {
    let stride = dst.stride();
    let Some(base) = rows_at_mut(dst.samples_mut(), 0, stride, 8, 8) else {
        crate::idct4x4_add_quad_by_blocks(&Scalar, dst, coeffs, nz);
        return;
    };
    let [b0, b1, b2, b3] = coeffs;
    // SAFETY: AVX2 is available (this function's contract); rows 0 to 3
    // and 4 to 7 lie inside the extent `rows_at_mut` checked, eight rows of
    // eight bytes.
    unsafe {
        add_pair(base, stride, b0, b1, nz & 3);
        add_pair(base.add(4 * stride), stride, b2, b3, (nz >> 2) & 3);
    }
}

/// Adds the horizontal pair of 4x4 blocks `left` and `right` to the four
/// rows of eight samples from `at`, each block where its bit of `bits`
/// (1 for `left`, 2 for `right`) is set.
///
/// # Safety
///
/// As [`add_four_rows`].
#[inline(always)]
unsafe fn add_pair(at: *mut u8, stride: usize, left: &[i32; 16], right: &[i32; 16], bits: u8) {
    if bits == 0 {
        return;
    }
    // SAFETY: AVX2 is available and the rows lie inside the checked extent
    // (the caller's contract).
    unsafe {
        add_four_rows(at, stride, idct4_pair(left, right, pair_keep(bits)));
    }
}

/// Inverse 8x8 transform of `coeffs`, added to `dst` (clause 8.5.13.2).
///
/// A row is one vector. The horizontal pass needs the values of one row
/// spread over the vectors rather than over the lanes, so the block is
/// transposed, passed and transposed back; the vertical pass then works
/// on the rows as they are.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn idct8x8_add(mut dst: BlockMut<'_>, coeffs: &[i32; 64]) {
    let stride = dst.stride();
    let Some(base) = rows_at_mut(dst.samples_mut(), 0, stride, 8, 8) else {
        Scalar.idct8x8_add(dst, coeffs);
        return;
    };
    // SAFETY: AVX2 is available (this function's contract); the loads check
    // their slices, and the eight rows lie inside the extent `rows_at_mut`
    // checked.
    unsafe {
        let [g0, g1, g2, g3, g4, g5, g6, g7] = idct8x8(coeffs);
        add_four_rows(base, stride, [g0, g1, g2, g3]);
        add_four_rows(base.add(4 * stride), stride, [g4, g5, g6, g7]);
    }
}

/// The inverse 8x8 transform of `coeffs` before rounding, row `y` in
/// vector `y`.
///
/// # Safety
///
/// As [`transpose4x2`].
#[inline(always)]
unsafe fn idct8x8(coeffs: &[i32; 64]) -> [__m256i; 8] {
    // SAFETY: AVX2 is available (the caller's contract); the loads check
    // their slices.
    unsafe {
        let mut rows = [_mm256_setzero_si256(); 8];
        for (y, row) in rows.iter_mut().enumerate() {
            *row = load8_i32(coeffs.get(y * 8..y * 8 + 8).unwrap_or_default());
        }
        let rows = transpose8(idct8_pass(transpose8(rows)));
        idct8_pass(rows)
    }
}

// ---------------------------------------------------------------------
// Intra prediction (clauses 8.3.1.2, 8.3.2.2, 8.3.3 and 8.3.4), and the
// Intra_4x4 and Intra_8x8 macroblocks with each block's residual added in
// registers.
//
// A kernel checks the view's block and the neighbours its flags name once
// (`IntraRows::of`), then reads and writes every sample through a pointer
// into the view: the rows of the block, the row above it (up to 2n
// samples), the column left of it and the sample above and left, each
// only where the view's flags say it exists. The Intra_4x4 and Intra_8x8
// blocks of a macroblock read their neighbours inside the macroblock from
// what the blocks before them wrote.
// ---------------------------------------------------------------------

/// An intra view's block and neighbours, behind one check of their extent.
#[derive(Clone, Copy)]
struct IntraRows {
    /// The view's first sample.
    base: *mut u8,
    /// Index of the block's first sample.
    origin: usize,
    stride: usize,
    avail: IntraAvail,
}

impl IntraRows {
    /// The `n` by `n` view `dst`, where it is that size and where, as its
    /// constructor established, the block's rows lie inside the view and
    /// the view starts at or before every neighbour its flags name.
    /// Checked again here, in a few comparisons, so that what the pointer
    /// accesses below rely on is established in this file.
    #[inline(always)]
    fn of(dst: &mut IntraMut<'_>, n: usize) -> Option<IntraRows> {
        let (stride, avail) = (dst.stride(), dst.avail());
        if dst.n() != n || stride < n {
            return None;
        }
        let (data, origin) = dst.window();
        let fits = Self::fits(data.len(), origin, stride, n, avail);
        fits.then_some(IntraRows {
            base: data.as_mut_ptr(),
            origin,
            stride,
            avail,
        })
    }

    /// The two 8x8 views of `dst`, Cb then Cr, with one check of the
    /// geometry they share, as [`IntraRows::of`] checks each: their
    /// samples are the same range of each plane, so the shorter slice
    /// bounds both.
    #[inline(always)]
    fn pair(dst: &mut ChromaIntraMut<'_>) -> Option<[IntraRows; 2]> {
        let stride = dst.stride();
        let ([cb, cr], origin, bits) = dst.windows();
        let avail = IntraAvail::from_bits(bits);
        let len = cb.len().min(cr.len());
        let fits = stride >= 8 && Self::fits(len, origin, stride, 8, avail);
        let rows = |data: &mut [u8]| IntraRows {
            base: data.as_mut_ptr(),
            origin,
            stride,
            avail,
        };
        fits.then(|| [rows(cb), rows(cr)])
    }

    /// Whether an `n` by `n` block whose first sample is `data[origin]`,
    /// with rows `stride` apart, lies inside `len` samples, at or after
    /// every neighbour `avail` names.
    #[inline(always)]
    fn fits(len: usize, origin: usize, stride: usize, n: usize, avail: IntraAvail) -> bool {
        let before = if avail.top_left {
            stride.checked_add(1)
        } else if avail.top || avail.top_right {
            Some(stride)
        } else if avail.left {
            Some(1)
        } else {
            Some(0)
        };
        before.is_some_and(|before| before <= origin) && rows_fit(len, origin, stride, n, n)
    }

    /// A pointer to sample (x - 1, y - 1) of the block: `x` and `y` are
    /// one more than the sample's coordinates, so that the row above and
    /// the column left are 0.
    ///
    /// Forming it is safe; it points inside the view where the sample is
    /// in the block's rows (`x` from 1 to n, `y` from 1 to n), or in the
    /// row above (`y` = 0, `x` from 1 to 2n) with `top` set, or in the
    /// column left (`x` = 0, `y` from 1 to n) with `left` set, or is
    /// p[-1, -1] with `top_left` set ([`IntraRows::of`]; the row above ends
    /// before the block's last row does, as `stride >= n`).
    #[inline(always)]
    fn at(&self, x: usize, y: usize) -> *mut u8 {
        // Wrapping: the true index is not negative wherever the pointer is
        // used, and then the wrapped sum is that index.
        let i = self
            .origin
            .wrapping_sub(self.stride + 1)
            .wrapping_add(y.wrapping_mul(self.stride))
            .wrapping_add(x);
        self.base.wrapping_add(i)
    }
}

/// [`crate::Kernels::intra16x16`].
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn intra16x16(mut dst: IntraMut<'_>, mode: u8) {
    let Some(w) = IntraRows::of(&mut dst, 16) else {
        return Scalar.intra16x16(dst, mode);
    };
    // SAFETY: AVX2 is available (this function's contract); every access
    // is one `IntraRows::at` points inside the view: the row above with
    // `top`, the column left with `left`, p[-1, -1] with `top_left`, and
    // the block's rows.
    unsafe {
        let top: [u8; 16] = if w.avail.top {
            w.at(1, 0).cast::<[u8; 16]>().read_unaligned()
        } else {
            [0; 16]
        };
        let left: [u8; 16] = if w.avail.left {
            core::array::from_fn(|y| *w.at(0, y + 1))
        } else {
            [0; 16]
        };
        match mode {
            0 => {
                let v = vector16(top);
                for y in 0..16 {
                    _mm_storeu_si128(w.at(1, y + 1).cast(), v);
                }
            }
            1 => {
                for (y, l) in left.iter().enumerate() {
                    _mm_storeu_si128(w.at(1, y + 1).cast(), _mm_set1_epi8(*l as i8));
                }
            }
            3 => {
                let corner = if w.avail.top_left { *w.at(0, 0) } else { 0 };
                let [a, b, c] = plane_parameters::<16>(&top, &left, corner);
                // Each lane of a row holds a + b * (x - 7) + c * (y - 7) +
                // 16; with samples of 0 to 255 its magnitude stays below
                // 2^15 (the design note), so 16-bit lanes hold it exactly.
                let xs = _mm256_setr_epi16(-7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8);
                let step = _mm256_set1_epi16(c as i16);
                let mut v = _mm256_add_epi16(
                    _mm256_set1_epi16((a + 16 - 7 * c) as i16),
                    _mm256_mullo_epi16(_mm256_set1_epi16(b as i16), xs),
                );
                for y in (0..16).step_by(2) {
                    let next = _mm256_add_epi16(v, step);
                    // The pack interleaves the two rows by lane; the
                    // permute puts row y in the low lane and y + 1 in the
                    // high one.
                    let both = _mm256_packus_epi16(
                        _mm256_srai_epi16::<5>(v),
                        _mm256_srai_epi16::<5>(next),
                    );
                    let both = _mm256_permute4x64_epi64::<0b11_01_10_00>(both);
                    _mm_storeu_si128(w.at(1, y + 1).cast(), _mm256_castsi256_si128(both));
                    _mm_storeu_si128(w.at(1, y + 2).cast(), _mm256_extracti128_si256::<1>(both));
                    v = _mm256_add_epi16(next, step);
                }
            }
            _ => {
                let (t, l) = (byte_sum16(top), byte_sum16(left));
                let dc = match (w.avail.top, w.avail.left) {
                    (true, true) => (t + l + 16) >> 5,
                    (true, false) => (t + 8) >> 4,
                    (false, true) => (l + 8) >> 4,
                    (false, false) => 128,
                };
                let v = _mm_set1_epi8(dc as u8 as i8);
                for y in 0..16 {
                    _mm_storeu_si128(w.at(1, y + 1).cast(), v);
                }
            }
        }
    }
}

/// The sum of sixteen bytes.
#[inline(always)]
fn byte_sum16(bytes: [u8; 16]) -> i32 {
    // SAFETY: SSE2 is part of the x86-64 baseline; register operations.
    unsafe {
        let s = _mm_sad_epu8(vector16(bytes), _mm_setzero_si128());
        _mm_cvtsi128_si32(_mm_add_epi32(s, _mm_srli_si128::<8>(s)))
    }
}

/// a, b and c of the plane prediction of an `N` by `N` block from the
/// `N` samples above it, the `N` left of it and the one above and left
/// (Equations 8-105 to 8-109 for 16x16, 8-141 to 8-145 for 4:2:0 chroma):
/// a without its rounding, so that a sample is
/// `Clip1((a + b * (x - N/2 + 1) + c * (y - N/2 + 1) + 16) >> 5)`.
#[inline(always)]
fn plane_parameters<const N: usize>(top: &[u8], left: &[u8], corner: u8) -> [i32; 3] {
    let half = N / 2;
    let at = |side: &[u8], i: usize| -> i32 {
        // Index i of the side, where i = N/2 - 2 - k may reach -1 (wrapped):
        // the sample above and left.
        side.get(i).map_or(i32::from(corner), |&s| i32::from(s))
    };
    let (mut h, mut v) = (0, 0);
    for k in 0..half {
        let weight = k as i32 + 1;
        let near = (half - 2).wrapping_sub(k);
        h += weight * (at(top, half + k) - at(top, near));
        v += weight * (at(left, half + k) - at(left, near));
    }
    let a = 16 * (at(left, N - 1) + at(top, N - 1));
    let scale = if N == 16 { 5 } else { 34 };
    [a, (scale * h + 32) >> 6, (scale * v + 32) >> 6]
}

/// [`crate::Kernels::intra_chroma`]: one 8x8 block of a 4:2:0 picture.
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn intra_chroma(mut dst: IntraMut<'_>, mode: u8) {
    let Some(w) = IntraRows::of(&mut dst, 8) else {
        return Scalar.intra_chroma(dst, mode);
    };
    // SAFETY: as in `intra16x16`, for an 8x8 block.
    unsafe {
        let top: [u8; 8] = if w.avail.top {
            w.at(1, 0).cast::<[u8; 8]>().read_unaligned()
        } else {
            [0; 8]
        };
        let left: [u8; 8] = if w.avail.left {
            core::array::from_fn(|y| *w.at(0, y + 1))
        } else {
            [0; 8]
        };
        let row = |y: usize, v: u64| w.at(1, y + 1).cast::<u64>().write_unaligned(v);
        let spread = |s: u8| u64::from(s) * 0x0101_0101_0101_0101;
        match mode {
            1 => {
                for (y, l) in left.iter().enumerate() {
                    row(y, spread(*l));
                }
            }
            2 => {
                let v = u64::from_le_bytes(top);
                for y in 0..8 {
                    row(y, v);
                }
            }
            3 => {
                let corner = if w.avail.top_left { *w.at(0, 0) } else { 0 };
                let [a, b, c] = plane_parameters::<8>(&top, &left, corner);
                // As for 16x16: a sample before its shift stays inside 16
                // bits.
                let xs = _mm_setr_epi16(-3, -2, -1, 0, 1, 2, 3, 4);
                let step = _mm_set1_epi16(c as i16);
                let mut v = _mm_add_epi16(
                    _mm_set1_epi16((a + 16 - 3 * c) as i16),
                    _mm_mullo_epi16(_mm_set1_epi16(b as i16), xs),
                );
                for y in (0..8).step_by(2) {
                    let next = _mm_add_epi16(v, step);
                    let both = _mm_packus_epi16(_mm_srai_epi16::<5>(v), _mm_srai_epi16::<5>(next));
                    row(y, _mm_cvtsi128_si64(both) as u64);
                    row(y + 1, _mm_extract_epi64::<1>(both) as u64);
                    v = _mm_add_epi16(next, step);
                }
            }
            // DC, each 4x4 quarter by its own rule (clause 8.3.4.1-3).
            _ => {
                let sum = |s: &[u8]| s.iter().map(|&v| i32::from(v)).sum::<i32>();
                let (t0, t1) = (sum(&top[..4]), sum(&top[4..]));
                let (l0, l1) = (sum(&left[..4]), sum(&left[4..]));
                let quarter = |t: i32, l: i32, diagonal: bool, prefer_top: bool| -> u64 {
                    let v = match (w.avail.top, w.avail.left) {
                        (true, true) if diagonal => (t + l + 4) >> 3,
                        (true, true) if prefer_top => (t + 2) >> 2,
                        (true, true) => (l + 2) >> 2,
                        (true, false) => (t + 2) >> 2,
                        (false, true) => (l + 2) >> 2,
                        (false, false) => 128,
                    };
                    u64::from(v as u8) * 0x0101_0101
                };
                let upper = quarter(t0, l0, true, false) | quarter(t1, l0, false, true) << 32;
                let lower = quarter(t0, l1, false, false) | quarter(t1, l1, true, false) << 32;
                for y in 0..4 {
                    row(y, upper);
                    row(y + 4, lower);
                }
            }
        }
    }
}

/// For the chroma DC prediction of [`chroma_mb`], by availability (bit 0
/// `left`, bit 1 `top`; 0, neither, is not used): the two sums each 4x4
/// quarter averages, as indices into the eight sums of both components
/// (T0, T1, L0, L1 of Cb, then of Cr: the sums of p[0..3, -1], p[4..7,
/// -1], p[-1, 0..3] and p[-1, 4..7]). A quarter is `(A + B + 4) >> 3`,
/// which is `(X + 2) >> 2` where A and B are both X (clause 8.3.4.1 to
/// 8.3.4.3): with both sides the quarters on the diagonal average both and
/// the others take the side they touch; with one side every quarter takes
/// that side's sum nearest it.
static CHROMA_DC_PICKS: [[[i32; 8]; 2]; 4] = {
    const BOTH: [[i32; 8]; 2] = [[0, 1, 3, 1, 4, 5, 7, 5], [2, 1, 3, 3, 6, 5, 7, 7]];
    const LEFT: [i32; 8] = [2, 2, 3, 3, 6, 6, 7, 7];
    const TOP: [i32; 8] = [0, 1, 0, 1, 4, 5, 4, 5];
    [BOTH, [LEFT, LEFT], [TOP, TOP], BOTH]
};

/// The rows of the 8x8 chroma prediction of both components, from their
/// neighbours: `edges` holds, per lane (Cb low, Cr high), p[0..7, -1] in
/// its first eight bytes and p[-1, 0..7] in its last eight, each zero where
/// the component has no such neighbour, as in the reference; `plane` the
/// parameters a, b and c of each component's plane prediction, used only
/// in mode 3. Vector `i` holds rows `2i` and `2i + 1` of each component in
/// its lane.
///
/// # Safety
///
/// AVX2 must be available; inlined only into `chroma_mb`, which enables
/// it.
#[inline(always)]
unsafe fn predict_chroma(
    mode: u8,
    avail: IntraAvail,
    edges: __m256i,
    plane: [[i32; 3]; 2],
) -> [__m256i; 4] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations, and the loads read entries of static arrays.
    unsafe {
        match mode {
            1 => {
                let [r01, r23, r45, r67] = &CHROMA_HORIZONTAL;
                [
                    spread(edges, r01),
                    spread(edges, r23),
                    spread(edges, r45),
                    spread(edges, r67),
                ]
            }
            2 => [spread(edges, &CHROMA_VERTICAL); 4],
            3 => {
                let [[ab, bb, cb], [ar, br, cr]] = plane;
                // Each lane of a row holds a + b * (x - 3) + c * (y - 3) +
                // 16 of its component, inside 16 bits as for 16x16.
                let xs = _mm256_setr_epi16(-3, -2, -1, 0, 1, 2, 3, 4, -3, -2, -1, 0, 1, 2, 3, 4);
                let step = lanes16(cb, cr);
                let mut v = _mm256_add_epi16(
                    lanes16(ab + 16 - 3 * cb, ar + 16 - 3 * cr),
                    _mm256_mullo_epi16(lanes16(bb, br), xs),
                );
                let mut rows = [_mm256_setzero_si256(); 4];
                for row in rows.iter_mut() {
                    let next = _mm256_add_epi16(v, step);
                    // In each lane, row y then row y + 1.
                    *row = _mm256_packus_epi16(
                        _mm256_srai_epi16::<5>(v),
                        _mm256_srai_epi16::<5>(next),
                    );
                    v = _mm256_add_epi16(next, step);
                }
                rows
            }
            _ => {
                let bits = usize::from(avail.left) | usize::from(avail.top) << 1;
                let Some([a, b]) = CHROMA_DC_PICKS.get(bits).filter(|_| bits != 0) else {
                    return [_mm256_set1_epi8(-128); 4];
                };
                // The sums of four samples, as 32-bit lanes T0, T1, L0, L1
                // of each component.
                let sums = _mm256_madd_epi16(
                    _mm256_maddubs_epi16(edges, _mm256_set1_epi8(1)),
                    _mm256_set1_epi16(1),
                );
                let a = _mm256_permutevar8x32_epi32(sums, _mm256_loadu_si256(a.as_ptr().cast()));
                let b = _mm256_permutevar8x32_epi32(sums, _mm256_loadu_si256(b.as_ptr().cast()));
                let quarters = _mm256_srli_epi32::<3>(_mm256_add_epi32(
                    _mm256_add_epi32(a, b),
                    _mm256_set1_epi32(4),
                ));
                // Each quarter's value is below 256, in the low byte of
                // its lane: rows 0 to 3 are quarters 0 and 1, rows 4 to 7
                // quarters 2 and 3.
                let [upper, lower] = &CHROMA_DC_ROWS;
                let (upper, lower) = (spread(quarters, upper), spread(quarters, lower));
                [upper, upper, lower, lower]
            }
        }
    }
}

/// Byte shuffles that spread [`predict_chroma`]'s edges into rows: for
/// horizontal, rows `2i` and `2i + 1` from p[-1, 2i] and p[-1, 2i + 1]
/// (bytes 8 + 2i and 9 + 2i of each lane).
static CHROMA_HORIZONTAL: [[u8; 16]; 4] = [
    [8, 8, 8, 8, 8, 8, 8, 8, 9, 9, 9, 9, 9, 9, 9, 9],
    [
        10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 11, 11, 11, 11,
    ],
    [
        12, 12, 12, 12, 12, 12, 12, 12, 13, 13, 13, 13, 13, 13, 13, 13,
    ],
    [
        14, 14, 14, 14, 14, 14, 14, 14, 15, 15, 15, 15, 15, 15, 15, 15,
    ],
];

/// For vertical, two rows of p[0..7, -1].
static CHROMA_VERTICAL: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 0, 1, 2, 3, 4, 5, 6, 7];

/// For DC, two rows of the upper quarters' values (the low bytes of 32-bit
/// lanes 0 and 1) and two of the lower quarters' (lanes 2 and 3).
static CHROMA_DC_ROWS: [[u8; 16]; 2] = [
    [0, 0, 0, 0, 4, 4, 4, 4, 0, 0, 0, 0, 4, 4, 4, 4],
    [8, 8, 8, 8, 12, 12, 12, 12, 8, 8, 8, 8, 12, 12, 12, 12],
];

/// `v` shuffled by `pattern` in each lane.
///
/// # Safety
///
/// AVX2 must be available; inlined only into functions that enable it.
#[inline(always)]
unsafe fn spread(v: __m256i, pattern: &[u8; 16]) -> __m256i {
    // SAFETY: AVX2 is available (the caller's contract); the load reads the
    // sixteen bytes of `pattern`.
    unsafe {
        let p = _mm_loadu_si128(pattern.as_ptr().cast());
        _mm256_shuffle_epi8(v, _mm256_broadcastsi128_si256(p))
    }
}

/// `low` in every 16-bit lane of the low half, `high` in every one of the
/// high half.
///
/// # Safety
///
/// AVX2 must be available; inlined only into functions that enable it.
#[inline(always)]
unsafe fn lanes16(low: i32, high: i32) -> __m256i {
    // SAFETY: AVX2 is available (the caller's contract); register
    // operations.
    unsafe { _mm256_set_m128i(_mm_set1_epi16(high as i16), _mm_set1_epi16(low as i16)) }
}

/// [`crate::Kernels::intra_chroma_mb`]: both components' prediction in one
/// set of vectors, Cb in the low lane and Cr in the high one, then each
/// component's residual added in registers, a horizontal pair of blocks at
/// a time with the DC coefficients put in place in registers, and its rows
/// written once. Each mode reads only the neighbours it predicts from.
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn intra_chroma_mb(
    mut dst: ChromaIntraMut<'_>,
    mode: u8,
    dc: &[[i32; 4]; 2],
    ac: &[[[i32; 16]; 4]; 2],
    ac_nz: u8,
) {
    let Some([b, r]) = IntraRows::pair(&mut dst) else {
        return crate::intra_chroma_mb_by_parts(&Scalar, dst, mode, dc, ac, ac_nz);
    };
    let ([dc_b, dc_r], [ac_b, ac_r]) = (dc, ac);
    let (bits_b, bits_r) = (ac_nz & 0xf, ac_nz >> 4);
    // SAFETY: AVX2 is available (this function's contract). Each
    // component's accesses are those of `intra_chroma` through its own
    // `IntraRows`, which `IntraRows::pair` checked together: the row above
    // where the flags have `top`, the column left where they have `left`,
    // p[-1, -1] where they have `top_left`, and the block's eight rows.
    unsafe {
        let avail = b.avail;
        // p[0..7, -1] of a component in the low eight bytes, zero where
        // absent or where the mode does not use it; p[-1, 0..7] in the high
        // eight bytes likewise.
        let edge = |w: &IntraRows| {
            let top = if avail.top && mode != 1 {
                _mm_loadl_epi64(w.at(1, 0).cast())
            } else {
                _mm_setzero_si128()
            };
            if avail.left && mode != 2 {
                column8(w, top)
            } else {
                top
            }
        };
        let (eb, er) = (edge(&b), edge(&r));
        let plane = if mode == 3 {
            let parameters = |w: &IntraRows, e: __m128i| {
                let corner = if avail.top_left { *w.at(0, 0) } else { 0 };
                let top = (_mm_cvtsi128_si64(e) as u64).to_le_bytes();
                let left = (_mm_extract_epi64::<1>(e) as u64).to_le_bytes();
                plane_parameters::<8>(&top, &left, corner)
            };
            [parameters(&b, eb), parameters(&r, er)]
        } else {
            [[0; 3]; 2]
        };
        let [p0, p1, p2, p3] = predict_chroma(mode, avail, _mm256_set_m128i(er, eb), plane);
        let cb = [
            _mm256_castsi256_si128(p0),
            _mm256_castsi256_si128(p1),
            _mm256_castsi256_si128(p2),
            _mm256_castsi256_si128(p3),
        ];
        let cr = [
            _mm256_extracti128_si256::<1>(p0),
            _mm256_extracti128_si256::<1>(p1),
            _mm256_extracti128_si256::<1>(p2),
            _mm256_extracti128_si256::<1>(p3),
        ];
        chroma_residual((b.at(1, 1), b.stride), Some(cb), dc_b, ac_b, bits_b);
        chroma_residual((r.at(1, 1), r.stride), Some(cr), dc_r, ac_r, bits_r);
    }
}

/// `v` with p[-1, 0..7] of an 8x8 block in bytes 8 to 15: one `pinsrb`
/// a sample.
///
/// # Safety
///
/// SSE4.1 must be available, and the view's flags must have `left`
/// ([`IntraRows::at`]); inlined only into functions that enable AVX2.
#[inline(always)]
unsafe fn column8(w: &IntraRows, v: __m128i) -> __m128i {
    // SAFETY: SSE4.1 is available, and p[-1, y] lies inside the view for y
    // below 8 (the caller's contract).
    unsafe {
        let at = |y: usize| i32::from(*w.at(0, y + 1));
        let v = _mm_insert_epi8::<8>(v, at(0));
        let v = _mm_insert_epi8::<9>(v, at(1));
        let v = _mm_insert_epi8::<10>(v, at(2));
        let v = _mm_insert_epi8::<11>(v, at(3));
        let v = _mm_insert_epi8::<12>(v, at(4));
        let v = _mm_insert_epi8::<13>(v, at(5));
        let v = _mm_insert_epi8::<14>(v, at(6));
        _mm_insert_epi8::<15>(v, at(7))
    }
}

/// [`crate::Kernels::chroma_residual_mb`]: each component's residual added
/// a horizontal pair of blocks at a time, as [`intra_chroma_mb`] adds it to
/// its prediction, here to the rows the block holds.
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn chroma_residual_mb(
    mut views: [BlockMut<'_>; 2],
    dc: &[[i32; 4]; 2],
    ac: &[[[i32; 16]; 4]; 2],
    ac_nz: u8,
) {
    let [cb, cr] = &mut views;
    let at = |v: &mut BlockMut<'_>| {
        let stride = v.stride();
        let fits = v.w() == 8 && v.h() == 8;
        fits.then(|| rows_at_mut(v.samples_mut(), 0, stride, 8, 8).map(|at| (at, stride)))
            .flatten()
    };
    let (Some(b), Some(r)) = (at(cb), at(cr)) else {
        return crate::chroma_residual_mb_by_parts(&Scalar, views, dc, ac, ac_nz);
    };
    let ([dc_b, dc_r], [ac_b, ac_r]) = (dc, ac);
    // SAFETY: AVX2 is available (this function's contract), and each
    // component's eight rows of eight bytes lie inside the extent
    // `rows_at_mut` checked.
    unsafe {
        chroma_residual(b, None, dc_b, ac_b, ac_nz & 0xf);
        chroma_residual(r, None, dc_r, ac_r, ac_nz >> 4);
    }
}

/// One component's residual: its four blocks (`ac_bits` saying which have
/// coefficients other than the DC) added to `pred`, the rows of its
/// prediction (two to a vector), or to the rows the block holds where there
/// is no `pred`, and the rows written, a horizontal pair of blocks at a
/// time. `at` is the block's first sample and `stride` the distance between
/// rows.
///
/// # Safety
///
/// AVX2 must be available, and the eight rows of eight bytes from `at`,
/// `stride` apart, must lie inside one slice the caller may write;
/// inlined only into functions that enable AVX2.
#[inline(always)]
unsafe fn chroma_residual(
    (at, stride): (*mut u8, usize),
    pred: Option<[__m128i; 4]>,
    dc: &[i32; 4],
    ac: &[[i32; 16]; 4],
    ac_bits: u8,
) {
    // SAFETY: AVX2 is available and the block's rows lie inside the view
    // (the caller's contract).
    unsafe {
        let [b0, b1, b2, b3] = ac;
        let dcs = _mm256_broadcastsi128_si256(load4_i32(dc));
        // Each pair's DCs, the left block's in the low lane and the right
        // one's in the high lane.
        let upper = _mm256_permutevar8x32_epi32(dcs, _mm256_setr_epi32(0, 0, 0, 0, 1, 1, 1, 1));
        let lower = _mm256_permutevar8x32_epi32(dcs, _mm256_setr_epi32(2, 2, 2, 2, 3, 3, 3, 3));
        let bottom = at.add(4 * stride);
        match pred {
            Some([p0, p1, p2, p3]) => {
                pair_into(at, stride, Some([p0, p1]), b0, b1, upper, ac_bits & 3);
                pair_into(bottom, stride, Some([p2, p3]), b2, b3, lower, ac_bits >> 2);
            }
            None => {
                pair_into(at, stride, None, b0, b1, upper, ac_bits & 3);
                pair_into(bottom, stride, None, b2, b3, lower, ac_bits >> 2);
            }
        }
    }
}

/// Four rows of eight samples written from `at`: `pred` (rows 0 and 1,
/// then 2 and 3), or the rows `at` holds where there is no `pred`, with the
/// residual of the horizontal pair `left` and `right` added. `dc` holds the
/// left block's DC in its low lane and the right one's in its high lane;
/// `bits` says which blocks (1 for `left`, 2 for `right`) have other
/// coefficients, which are taken as 0 in the other.
///
/// A pair without other coefficients transforms to its DC in every
/// sample before rounding (in each pass a row or column with only its
/// first value `d` gives `d` four times), which is `dc` itself in each row;
/// with neither, the rows are left as they are.
///
/// # Safety
///
/// As [`add_four_rows`].
#[inline(always)]
unsafe fn pair_into(
    at: *mut u8,
    stride: usize,
    pred: Option<[__m128i; 2]>,
    left: &[i32; 16],
    right: &[i32; 16],
    dc: __m256i,
    bits: u8,
) {
    // SAFETY: AVX2 is available and the four rows lie inside the checked
    // extent (the caller's contract).
    unsafe {
        let residual = if bits != 0 {
            let [r0, r1, r2, r3] = pair_rows(left, right);
            let [r0, r1, r2, r3] = match pair_keep(bits) {
                Some(mask) => [
                    _mm256_and_si256(r0, mask),
                    _mm256_and_si256(r1, mask),
                    _mm256_and_si256(r2, mask),
                    _mm256_and_si256(r3, mask),
                ],
                None => [r0, r1, r2, r3],
            };
            // Each block's first coefficient is its DC.
            let r0 = _mm256_blend_epi32::<0b0001_0001>(r0, dc);
            idct4_rows([r0, r1, r2, r3], None)
        } else if _mm256_testz_si256(dc, dc) == 0 {
            [dc; 4]
        } else {
            if let Some([r01, r23]) = pred {
                store_four_rows(at, stride, r01, r23);
            }
            return;
        };
        let pred = match pred {
            Some(p) => p,
            None => load_four_rows(at, stride),
        };
        let [r01, r23] = add_residual8(pred, residual);
        store_four_rows(at, stride, r01, r23);
    }
}

/// For each Intra4x4PredMode, the three shuffles that take a 4x4 block's
/// prediction from its edge line, the line's 2-tap averages and its 3-tap
/// filter: byte `i` of shuffle `s` is where sample `i` (row by row) comes
/// from in vector `s`, or 0x80 (zero) where it comes from another one. Modes
/// 3 to 8 are [`crate::intra::sources`]; vertical and horizontal copy
/// samples of the line; DC (2) is not a shuffle and is left at zero.
static SHUFFLES_4X4: [[[u8; 16]; 3]; 9] = shuffles_4x4();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn shuffles_4x4() -> [[[u8; 16]; 3]; 9] {
    const LEN: usize = crate::intra::line_len(4);
    let sources = crate::intra::sources::<4, 16>();
    let mut out = [[[0x80u8; 16]; 3]; 9];
    let mut mode = 0;
    while mode < 9 {
        let mut i = 0;
        while i < 16 {
            let (x, y) = (i % 4, i / 4);
            // The line is p[-1, 3] .. p[-1, 0], p[-1, -1], p[0, -1] ..
            // p[7, -1].
            let s = match mode {
                0 => 5 + x,
                1 => 3 - y,
                2 => usize::MAX,
                _ => sources[mode][i] as usize,
            };
            if s < LEN {
                out[mode][0][i] = s as u8;
            } else if s < 2 * LEN - 1 {
                out[mode][1][i] = (s - LEN) as u8;
            } else if s < 3 * LEN - 1 {
                out[mode][2][i] = (s - (2 * LEN - 1)) as u8;
            }
            i += 1;
        }
        mode += 1;
    }
    out
}

/// The Intra_4x4 prediction of one block from its edge line: `top` holds
/// p[0, -1] to p[7, -1] (the last four already repeated where there is
/// no top right), `left` p[-1, 0] to p[-1, 3] in its low four bytes, and
/// `corner` p[-1, -1]; each zero where the block has no such neighbour,
/// as in the reference.
///
/// # Safety
///
/// AVX2 must be available; inlined only into `intra4x4_mb`, which enables
/// it.
#[inline(always)]
unsafe fn predict4x4(mode: u8, avail: IntraAvail, top: u64, left: u32, corner: u8) -> __m128i {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let Some(shuffles) = SHUFFLES_4X4.get(usize::from(mode)).filter(|_| mode != 2) else {
            let sum = |v: u64| (v.to_le_bytes().iter().map(|&b| u32::from(b)).sum::<u32>()) as i32;
            let (t, l) = (sum(top & 0xffff_ffff), sum(u64::from(left)));
            let dc = match (avail.top, avail.left) {
                (true, true) => (t + l + 4) >> 3,
                (true, false) => (t + 2) >> 2,
                (false, true) => (l + 2) >> 2,
                (false, false) => 128,
            };
            return _mm_set1_epi8(dc as u8 as i8);
        };
        // The line with its ends repeated, e[0] to e[14]: p[-1, 3] twice,
        // p[-1, 2], p[-1, 1], p[-1, 0], p[-1, -1], p[0, -1] to p[7, -1], and
        // p[7, -1] again.
        let l = left.swap_bytes();
        let low = u64::from(l & 0xff) | u64::from(l) << 8 | u64::from(corner) << 40 | top << 48;
        let high = top >> 16 | (top >> 56) << 48;
        let e = _mm_set_epi64x(high as i64, low as i64);
        let b = _mm_srli_si128::<1>(e);
        let c = _mm_srli_si128::<2>(e);
        // The line: b[k] = e[k + 1]. Its 2-tap averages: (b + c + 1) >> 1.
        // Its 3-tap filter, (e + 2b + c + 2) >> 2, is the rounded average of
        // b and the truncated average of e and c, which is exact in bytes.
        let pairs = _mm_avg_epu8(b, c);
        let one = _mm_set1_epi8(1);
        let floor = _mm_sub_epi8(_mm_avg_epu8(e, c), _mm_and_si128(_mm_xor_si128(e, c), one));
        let taps = _mm_avg_epu8(floor, b);
        let [s0, s1, s2] = shuffles;
        _mm_or_si128(
            _mm_or_si128(
                _mm_shuffle_epi8(b, vector16(*s0)),
                _mm_shuffle_epi8(pairs, vector16(*s1)),
            ),
            _mm_shuffle_epi8(taps, vector16(*s2)),
        )
    }
}

/// The prediction `pred` of a 4x4 block (row by row) with the inverse
/// transform of `coeffs` added and clipped, as `add_four_rows` does it.
///
/// # Safety
///
/// As [`predict4x4`].
#[inline(always)]
unsafe fn add_residual4(pred: __m128i, coeffs: &[i32; 16]) -> __m128i {
    // SAFETY: AVX2 is available (the caller's contract); the loads check
    // their slices.
    unsafe {
        let zero = _mm_setzero_si128();
        let mut rows = [_mm256_setzero_si256(); 4];
        for (y, row) in rows.iter_mut().enumerate() {
            let values = load4_i32(coeffs.get(y * 4..y * 4 + 4).unwrap_or_default());
            *row = _mm256_set_m128i(zero, values);
        }
        let rows = transpose4x2(idct4_pass(transpose4x2(rows)));
        let round = _mm256_set1_epi32(32);
        let [r0, r1, r2, r3] = idct4_pass(rows);
        let r0 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r0, round));
        let r1 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r1, round));
        let r2 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r2, round));
        let r3 = _mm256_srai_epi32::<6>(_mm256_add_epi32(r3, round));
        // Rows 0 and 1, and 2 and 3, as 16-bit values, saturated; added to
        // the prediction with saturation and packed with it, which clips
        // as Clip1 does (the transforms' design, point 3).
        let res01 = _mm256_castsi256_si128(_mm256_packs_epi32(r0, r1));
        let res23 = _mm256_castsi256_si128(_mm256_packs_epi32(r2, r3));
        let sum01 = _mm_adds_epi16(res01, _mm_cvtepu8_epi16(pred));
        let sum23 = _mm_adds_epi16(res23, _mm_cvtepu8_epi16(_mm_srli_si128::<8>(pred)));
        _mm_packus_epi16(sum01, sum23)
    }
}

/// [`crate::Kernels::intra4x4_mb`]: each block predicted from the
/// samples the blocks before it wrote, its residual added in registers,
/// and its four rows written once.
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn intra4x4_mb(
    mut dst: IntraMut<'_>,
    modes: &[u8; 16],
    coeffs: &[[i32; 16]; 16],
    nz: u16,
) {
    let Some(w) = IntraRows::of(&mut dst, 16) else {
        return Scalar.intra4x4_mb(dst, modes, coeffs, nz);
    };
    for (blk, (mode, coeffs)) in modes.iter().zip(coeffs).enumerate() {
        let (x, y) = crate::intra::blk_xy(blk);
        let a = crate::intra::block_avail(w.avail, x, y, 4, blk);
        // SAFETY: AVX2 is available (this function's contract). The row
        // above is read where `a` has `top`, the column left where it has
        // `left` and the corner where it has `top_left`, and `a` names a
        // neighbour outside the macroblock only where the view's flags do
        // (`block_avail`). The row above is read whole, p[x .. x + 7,
        // y - 1], also where `a` has no top right, whose four samples are
        // then replaced: inside the macroblock or in the row above it, it
        // reaches at most 20 samples along, which ends before the
        // macroblock's last row does (the stride is at least 16). The
        // column left is inside the macroblock or the view's left column;
        // the corner is one of those or p[-1, -1]. The four rows written
        // are the block's, inside the macroblock.
        unsafe {
            let mut top = if a.top {
                w.at(x + 1, y).cast::<u64>().read_unaligned()
            } else {
                0
            };
            if !a.top_right {
                top = (top & 0xffff_ffff) | (((top >> 24) & 0xff) * 0x0101_0101_0000_0000);
            }
            let left = if a.left {
                u32::from_le_bytes(core::array::from_fn(|k| *w.at(x, y + 1 + k)))
            } else {
                0
            };
            let corner = if a.top_left { *w.at(x, y) } else { 0 };
            let mut pred = predict4x4(*mode, a, top, left, corner);
            if nz & (1 << blk) != 0 {
                pred = add_residual4(pred, coeffs);
            }
            w.at(x + 1, y + 1)
                .cast::<i32>()
                .write_unaligned(_mm_cvtsi128_si32(pred));
            w.at(x + 1, y + 2)
                .cast::<i32>()
                .write_unaligned(_mm_extract_epi32::<1>(pred));
            w.at(x + 1, y + 3)
                .cast::<i32>()
                .write_unaligned(_mm_extract_epi32::<2>(pred));
            w.at(x + 1, y + 4)
                .cast::<i32>()
                .write_unaligned(_mm_extract_epi32::<3>(pred));
        }
    }
}

/// For each Intra8x8PredMode, the shuffles that take an 8x8 block's
/// prediction from two lines of 32 bytes, `a` and `b` ([`predict8x8`]):
/// for the vertical and horizontal modes the filtered edge line (`a`),
/// for modes 3 to 8 its 2-tap averages (`a`) and its 3-tap filter (`b`).
///
/// `[mode][half][source]`: half 0 makes rows 0 to 3 of the block, rows 0
/// and 1 in the low 128-bit lane and 2 and 3 in the high one, and half 1
/// rows 4 to 7 alike. Sources 0 to 3 are bytes 0 to 15 of `a`, 16 to 31 of
/// `a`, 0 to 15 of `b` and 16 to 31 of `b`, each in both lanes; byte `i` of
/// a shuffle is where byte `i` of the half comes from in that source, or
/// 0x80 (zero) where it comes from another. Modes 3 to 8 are
/// [`crate::intra::sources`]: its 2-tap average between positions `k` and
/// `k + 1` is byte `k + 1` of `a`, its 3-tap filter centred on `k` byte
/// `k` of `b`, and the one sample of the line it takes whole
/// (horizontal-up's copies of p'[-1, 7], position 0) byte 0 of `a`, which
/// holds that sample. DC (2) is not a shuffle and is left at zero.
static SHUFFLES_8X8: [[[[u8; 32]; 4]; 2]; 9] = shuffles_8x8();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn shuffles_8x8() -> [[[[u8; 32]; 4]; 2]; 9] {
    const LEN: usize = crate::intra::line_len(8);
    let sources = crate::intra::sources::<8, 64>();
    let mut out = [[[[0x80u8; 32]; 4]; 2]; 9];
    let mut mode = 0;
    while mode < 9 {
        let mut i = 0;
        while i < 64 {
            let (x, y) = (i % 8, i / 8);
            // The line is p'[-1, 7] .. p'[-1, 0], p'[-1, -1], p'[0, -1] ..
            // p'[15, -1]; `at` is a byte of line `a` (0) or `b` (1).
            let (line, at) = match mode {
                0 => (0, 9 + x),
                1 => (0, 7 - y),
                2 => (2, 0),
                _ => {
                    let s = sources[mode][i] as usize;
                    if s < LEN {
                        // Only position 0 of the line is taken whole; any
                        // other names a source past the four and fails the
                        // build.
                        (0, s * 64)
                    } else if s < 2 * LEN - 1 {
                        (0, s - LEN + 1)
                    } else {
                        (1, s - (2 * LEN - 1))
                    }
                }
            };
            if line < 2 {
                let byte = (y % 4 / 2) * 16 + (y % 2) * 8 + x;
                out[mode][y / 4][line * 2 + at / 16][byte] = (at % 16) as u8;
            }
            i += 1;
        }
        mode += 1;
    }
    out
}

/// Byte `k` of `v` moved to byte `k + 1` (`UP`) or to byte `k - 1`, across
/// the whole 256-bit vector; the byte moved in is zero.
///
/// # Safety
///
/// AVX2 must be available; inlined only into functions that enable it.
#[inline(always)]
unsafe fn shift_byte<const UP: bool>(v: __m256i) -> __m256i {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        if UP {
            // [0 | low lane]; each lane then takes its bytes after the
            // last of the 16 below them.
            let below = _mm256_permute2x128_si256::<0x08>(v, v);
            _mm256_alignr_epi8::<15>(v, below)
        } else {
            // [high lane | 0]; each lane then takes its bytes from the
            // second on, followed by the first of the 16 above.
            let above = _mm256_permute2x128_si256::<0x81>(v, v);
            _mm256_alignr_epi8::<1>(above, v)
        }
    }
}

/// The 3-tap filter (1, 2, 1) with rounding, `(a + 2b + c + 2) >> 2`, in
/// bytes: the rounded average of `b` and the truncated average of `a` and
/// `c`, which is exact.
///
/// # Safety
///
/// As [`shift_byte`].
#[inline(always)]
unsafe fn tap121(a: __m256i, b: __m256i, c: __m256i) -> __m256i {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations.
    unsafe {
        let odd = _mm256_and_si256(_mm256_xor_si256(a, c), _mm256_set1_epi8(1));
        _mm256_avg_epu8(_mm256_sub_epi8(_mm256_avg_epu8(a, c), odd), b)
    }
}

/// Where each byte of a line is kept for a block whose neighbours are
/// `bits` (`left`, `top`, `top_right` and `top_left` in bits 0 to 3, in
/// every byte): byte `k` of the result is 0xff where all the bits of
/// `select[k]` are in `bits` (so always where it is 0, and never where it
/// is 0x80), and 0 elsewhere.
///
/// # Safety
///
/// As [`shift_byte`].
#[inline(always)]
unsafe fn keep(bits: __m256i, select: &[u8; 32]) -> __m256i {
    // SAFETY: AVX2 is available (the caller's contract); `select` holds
    // the 32 bytes the load reads.
    unsafe {
        let select = _mm256_loadu_si256(select.as_ptr().cast());
        _mm256_cmpeq_epi8(_mm256_and_si256(bits, select), select)
    }
}

/// [`keep`]'s selections for the filter of clause 8.3.2.2.1 over the line
/// p[-1, 7] .. p[-1, 0], p[-1, -1], p[0, -1] .. p[15, -1] (bytes 0 to
/// 24). A sample's neighbour before it on the line is the sample itself at
/// the line's start, at p[-1, -1] without `left` and at p[0, -1] without
/// `top_left` (`LINE_BEFORE`); the one after it is the sample itself at
/// p[-1, 0] without `top_left`, at p[-1, -1] without `top` and at the
/// line's end (`LINE_AFTER`). The filtered line is kept where the
/// reference has it (`LINE_FILTERED`): p'[-1, y] with `left`, p'[-1, -1]
/// with `top_left` and p'[x, -1] with `top`, and is zero elsewhere, as the
/// reference's neighbours are.
const LINE_BEFORE: [u8; 32] = line_select([0x80, 0, 1, 8, 0]);
const LINE_AFTER: [u8; 32] = line_select([0, 8, 2, 0, 0x80]);
const LINE_FILTERED: [u8; 32] = line_filtered();

/// [`LINE_FILTERED`]: `left` for bytes 0 to 7, `top_left` for byte 8,
/// `top` for bytes 9 to 24, and nothing past the line.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn line_filtered() -> [u8; 32] {
    let mut s = [0x80u8; 32];
    let mut k = 0;
    while k < 25 {
        s[k] = match k {
            0..8 => 1,
            8 => 8,
            _ => 2,
        };
        k += 1;
    }
    s
}

/// A selection that keeps every byte (0) but the line's first, p[-1, 0],
/// p[-1, -1], p[0, -1] and the line's last (bytes 0, 7, 8, 9 and 24),
/// which take `at` in that order.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn line_select(at: [u8; 5]) -> [u8; 32] {
    let mut s = [0u8; 32];
    s[0] = at[0];
    s[7] = at[1];
    s[8] = at[2];
    s[9] = at[3];
    s[24] = at[4];
    s
}

/// The Intra_8x8 prediction of one block (clause 8.3.2.2), rows 0 to 3
/// and 4 to 7 in the layout of [`SHUFFLES_8X8`]'s halves, from `top`,
/// p[0, -1] to p[15, -1] (the last eight already repeated where there is
/// no top right), `left`, p[-1, 0] to p[-1, 7] from its low byte up, and
/// `corner`, p[-1, -1]; each zero where the block has no such neighbour,
/// as in the reference.
///
/// # Safety
///
/// AVX2 must be available; inlined only into `intra8x8_mb`, which enables
/// it.
#[inline(always)]
unsafe fn predict8x8(
    mode: u8,
    avail: IntraAvail,
    top: __m128i,
    left: u64,
    corner: u8,
) -> [__m256i; 2] {
    // SAFETY: AVX2 is available (the caller's contract); these are register
    // operations, and loads of constant arrays of the length they read.
    unsafe {
        // The line, bytes 0 to 24: p[-1, 7] .. p[-1, 0], p[-1, -1], then
        // p[0, -1] .. p[15, -1].
        let low = _mm_set_epi64x(
            (u64::from(corner) | (_mm_cvtsi128_si64(top) as u64) << 8) as i64,
            left.swap_bytes() as i64,
        );
        let line = _mm256_set_m128i(_mm_srli_si128::<7>(top), low);
        let bits = u8::from(avail.left)
            | u8::from(avail.top) << 1
            | u8::from(avail.top_right) << 2
            | u8::from(avail.top_left) << 3;
        let bits = _mm256_set1_epi8(bits as i8);
        // The filter of clause 8.3.2.2.1: each sample with its neighbours
        // on the line, or with itself where the clause has no neighbour.
        let before = _mm256_blendv_epi8(line, shift_byte::<true>(line), keep(bits, &LINE_BEFORE));
        let after = _mm256_blendv_epi8(line, shift_byte::<false>(line), keep(bits, &LINE_AFTER));
        let filtered = _mm256_and_si256(tap121(before, line, after), keep(bits, &LINE_FILTERED));
        let (a, b) = match mode {
            0 | 1 => (filtered, filtered),
            3..=8 => {
                // The filtered line's 2-tap averages and 3-tap filter
                // (`crate::intra::source`), with its ends repeated: bytes 0
                // and 24 are their own neighbours beyond the line.
                let not_first = _mm256_setr_epi8(
                    0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, //
                    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
                );
                let not_last = _mm256_setr_epi8(
                    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, //
                    -1, -1, -1, -1, -1, -1, -1, -1, 0, -1, -1, -1, -1, -1, -1, -1,
                );
                let before = _mm256_blendv_epi8(filtered, shift_byte::<true>(filtered), not_first);
                let after = _mm256_blendv_epi8(filtered, shift_byte::<false>(filtered), not_last);
                (
                    _mm256_avg_epu8(before, filtered),
                    tap121(before, filtered, after),
                )
            }
            _ => {
                // DC (clause 8.3.2.2.4), from the sums of p'[-1, 0..7]
                // (bytes 0 to 7) and of p'[0..7, -1] (bytes 9 to 16),
                // summed by groups of eight bytes with the others cleared.
                let sides = _mm256_setr_epi8(
                    -1, -1, -1, -1, -1, -1, -1, -1, 0, -1, -1, -1, -1, -1, -1, -1, //
                    -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                );
                let sums =
                    _mm256_sad_epu8(_mm256_and_si256(filtered, sides), _mm256_setzero_si256());
                let l = _mm256_extract_epi32::<0>(sums);
                let t = _mm256_extract_epi32::<2>(sums) + _mm256_extract_epi32::<4>(sums);
                let dc = match (avail.top, avail.left) {
                    (true, true) => (t + l + 8) >> 4,
                    (true, false) => (t + 4) >> 3,
                    (false, true) => (l + 4) >> 3,
                    (false, false) => 128,
                };
                let v = _mm256_set1_epi8(dc as u8 as i8);
                return [v, v];
            }
        };
        let Some(shuffles) = SHUFFLES_8X8.get(usize::from(mode)) else {
            return [_mm256_setzero_si256(); 2];
        };
        let sources = [
            _mm256_permute2x128_si256::<0x00>(a, a),
            _mm256_permute2x128_si256::<0x11>(a, a),
            _mm256_permute2x128_si256::<0x00>(b, b),
            _mm256_permute2x128_si256::<0x11>(b, b),
        ];
        shuffles.map(|half| {
            let [s0, s1, s2, s3] = sources;
            let [m0, m1, m2, m3] = half.map(|m| _mm256_loadu_si256(m.as_ptr().cast()));
            _mm256_or_si256(
                _mm256_or_si256(_mm256_shuffle_epi8(s0, m0), _mm256_shuffle_epi8(s1, m1)),
                _mm256_or_si256(_mm256_shuffle_epi8(s2, m2), _mm256_shuffle_epi8(s3, m3)),
            )
        })
    }
}

/// [`crate::Kernels::intra8x8_mb`]: each block predicted from the samples
/// the blocks before it wrote, its residual added in registers, and its
/// eight rows written once.
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn intra8x8_mb(
    mut dst: IntraMut<'_>,
    modes: &[u8; 4],
    coeffs: &[[i32; 64]; 4],
    nz: u8,
) {
    let Some(w) = IntraRows::of(&mut dst, 16) else {
        return Scalar.intra8x8_mb(dst, modes, coeffs, nz);
    };
    for (b8, (mode, coeffs)) in modes.iter().zip(coeffs).enumerate() {
        let (x, y) = ((b8 % 2) * 8, (b8 / 2) * 8);
        let a = crate::intra::block_avail(w.avail, x, y, 8, b8);
        // SAFETY: AVX2 is available (this function's contract). The row
        // above is read where `a` has `top`, the column left where it has
        // `left` and the corner where it has `top_left`, and `a` names a
        // neighbour outside the macroblock only where the view's flags do
        // (`block_avail`). The row above is read whole, p[x .. x + 15,
        // y - 1], also where `a` has no top right, whose eight samples are
        // then replaced: it is the row above the macroblock or its eighth
        // row, and reaches at most 24 samples along, which ends before the
        // macroblock's last row does (the stride is at least 16). The
        // column left is inside the macroblock or the view's left column;
        // the corner is one of those or p[-1, -1]. The eight rows written
        // are the block's, inside the macroblock.
        unsafe {
            let mut top = if a.top {
                _mm_loadu_si128(w.at(x + 1, y).cast())
            } else {
                _mm_setzero_si128()
            };
            if !a.top_right {
                let repeat = _mm_setr_epi8(0, 1, 2, 3, 4, 5, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7);
                top = _mm_shuffle_epi8(top, repeat);
            }
            let left = if a.left {
                u64::from_le_bytes(core::array::from_fn(|k| *w.at(x, y + 1 + k)))
            } else {
                0
            };
            let corner = if a.top_left { *w.at(x, y) } else { 0 };
            let [first, second] = predict8x8(*mode, a, top, left, corner);
            let mut rows = [
                _mm256_castsi256_si128(first),
                _mm256_extracti128_si256::<1>(first),
                _mm256_castsi256_si128(second),
                _mm256_extracti128_si256::<1>(second),
            ];
            if nz & (1 << b8) != 0 {
                let [g0, g1, g2, g3, g4, g5, g6, g7] = idct8x8(coeffs);
                let [r0, r1, r2, r3] = rows;
                let [r0, r1] = add_residual8([r0, r1], [g0, g1, g2, g3]);
                let [r2, r3] = add_residual8([r2, r3], [g4, g5, g6, g7]);
                rows = [r0, r1, r2, r3];
            }
            let [r0, r1, r2, r3] = rows;
            store_four_rows(w.at(x + 1, y + 1), w.stride, r0, r1);
            store_four_rows(w.at(x + 1, y + 5), w.stride, r2, r3);
        }
    }
}

// ---------------------------------------------------------------------
// Prediction combining (clause 8.4.2.3).
// ---------------------------------------------------------------------

/// Writes the prediction of a block from the two lists' predictions.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn combine(mut dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine) {
    // SAFETY: AVX2 is available; every access goes through the helpers.
    unsafe {
        for ((out, a), b) in dst.rows_mut().zip(p0.rows()).zip(p1.rows()) {
            match mode {
                // The entry point sends copying and averaging to the
                // reference, which is as fast there.
                Combine::Copy(_) | Combine::Average => return,
                Combine::Uni {
                    list,
                    log_wd,
                    w: weight,
                    o,
                } => {
                    let from = if list == 0 { a } else { b };
                    weighted_one(out, from, log_wd, weight, o);
                }
                Combine::Bi { log_wd, w0, w1, o } => {
                    weighted_two(out, a, b, log_wd, w0, w1, o);
                }
            }
        }
    }
}

/// Equations 8-286 and 8-287: one list, explicit weighting.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn weighted_one(out: &mut [u8], src: &[u8], log_wd: i32, weight: i32, offset: i32) {
    // SAFETY: AVX2 is available; the loads and stores check their slices.
    unsafe {
        // The products fit 16 bits: a sample is at most 255 and a weight is
        // -128 to 127 (clause 7.4.3.2).
        let w = _mm_set1_epi16(weight as i16);
        let o = _mm_set1_epi16(offset as i16);
        let round = _mm_set1_epi16(if log_wd >= 1 { 1 << (log_wd - 1) } else { 0 });
        let shift = _mm_cvtsi32_si128(log_wd);
        let zero = _mm_setzero_si128();
        let mut at = 0;
        while at < out.len() {
            let samples = load8(src.get(at..).unwrap_or_default());
            let wide = _mm_cvtepu8_epi16(samples);
            let scaled = _mm_mullo_epi16(wide, w);
            let shifted = if log_wd >= 1 {
                _mm_sra_epi16(_mm_add_epi16(scaled, round), shift)
            } else {
                scaled
            };
            let sum = _mm_add_epi16(shifted, o);
            let clipped = _mm_max_epi16(_mm_min_epi16(sum, _mm_set1_epi16(255)), zero);
            store_bytes(
                out.get_mut(at..).unwrap_or_default(),
                _mm_packus_epi16(clipped, clipped),
                8,
            );
            at += 8;
        }
    }
}

/// Equation 8-288: both lists, explicit or implicit weighting. The sum of
/// the two products can leave 16 bits, so it is computed in 32.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn weighted_two(
    out: &mut [u8],
    a: &[u8],
    b: &[u8],
    log_wd: i32,
    w0: i32,
    w1: i32,
    offset: i32,
) {
    // SAFETY: AVX2 is available; the loads and stores check their slices.
    unsafe {
        let v0 = _mm256_set1_epi32(w0);
        let v1 = _mm256_set1_epi32(w1);
        let round = _mm256_set1_epi32(1 << log_wd);
        let shift = _mm_cvtsi32_si128(log_wd + 1);
        let mut at = 0;
        while at < out.len() {
            let x = _mm256_cvtepu8_epi32(load8(a.get(at..).unwrap_or_default()));
            let y = _mm256_cvtepu8_epi32(load8(b.get(at..).unwrap_or_default()));
            let sum = _mm256_add_epi32(
                _mm256_add_epi32(_mm256_mullo_epi32(x, v0), _mm256_mullo_epi32(y, v1)),
                round,
            );
            let shifted = _mm256_sra_epi32(sum, shift);
            let with_offset = _mm256_add_epi32(shifted, _mm256_set1_epi32(offset));
            // Packing saturates to 0..65535 and then to 0..255, which is
            // the clipping of Equation 8-288.
            let packed = _mm256_packus_epi32(with_offset, with_offset);
            let low = _mm256_castsi256_si128(packed);
            let high = _mm256_extracti128_si256(packed, 1);
            let joined = _mm_unpacklo_epi64(low, high);
            store_bytes(
                out.get_mut(at..).unwrap_or_default(),
                _mm_packus_epi16(joined, joined),
                8,
            );
            at += 8;
        }
    }
}

/// Stores eight 16-bit values, as many as `dst` holds.
#[inline(always)]
fn store_i16x8(dst: &mut [i16], v: __m128i) {
    if dst.len() >= 8 {
        // SAFETY: the slice holds the eight values the store writes.
        unsafe { _mm_storeu_si128(dst.as_mut_ptr().cast(), v) }
    } else {
        let mut padded = [0i16; 8];
        // SAFETY: `padded` holds eight values.
        unsafe { _mm_storeu_si128(padded.as_mut_ptr().cast(), v) }
        for (d, s) in dst.iter_mut().zip(padded.iter()) {
            *d = *s;
        }
    }
}

/// Eight 16-bit values of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load_i16x8(src: &[i16]) -> __m128i {
    if src.len() >= 8 {
        // SAFETY: the slice holds the eight values the load reads.
        unsafe { _mm_loadu_si128(src.as_ptr().cast()) }
    } else {
        let mut padded = [0i16; 8];
        for (d, s) in padded.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        // SAFETY: `padded` holds eight values.
        unsafe { _mm_loadu_si128(padded.as_ptr().cast()) }
    }
}

/// Whether `n` rows of `w` bytes, the first at `start` and each `stride`
/// further, all lie inside a slice of `len` bytes: the last row's end,
/// `(n - 1) * stride + start + w`, computed with checked arithmetic, is at
/// most `len`. An overflow is a failed check, never a saturated value.
#[inline(always)]
fn rows_fit(len: usize, start: usize, stride: usize, n: usize, w: usize) -> bool {
    n.checked_sub(1)
        .and_then(|last| last.checked_mul(stride))
        .and_then(|v| v.checked_add(start))
        .and_then(|v| v.checked_add(w))
        .is_some_and(|end| end <= len)
}

/// `W` bytes of each of `N` rows of `data`, row `k` from
/// `start + k * stride`: the lines of a deblocking edge.
///
/// This and [`scatter`] form a pointer after the check that justified it,
/// as the luma interpolation does (`luma_mc_into`), and the argument is
/// the same:
///
/// - The check, made once and before any pointer exists, is [`rows_fit`]:
///   the last row's end is inside the slice, in checked arithmetic.
/// - Every row lies inside: row `k < N` starts at most where the last row
///   starts and ends at most where it ends, and as each term of its offset
///   is at most its counterpart in the checked sum, no row's offset
///   overflows either.
/// - The read is of `W` bytes, unaligned, into an array; nothing else is
///   done through the pointer.
/// - Where the check fails, no pointer is formed: each row is read through
///   the slice instead, a row past the end reading as zeros. The views make
///   that impossible for an edge of the decoder, whose window holds every
///   line the edge has.
///
/// One check instead of one per row: the rows of a vertical edge are
/// sixteen loads a pass, and their checks were as many instructions as the
/// filter.
#[inline(always)]
fn gather<const N: usize, const W: usize>(
    data: &[u8],
    start: usize,
    stride: usize,
) -> [[u8; W]; N] {
    if !rows_fit(data.len(), start, stride, N, W) {
        // The check may have failed by overflowing, so the offsets wrap; a
        // wrapped one lies past the slice and reads as zeros.
        return core::array::from_fn(|k| {
            bytes::<W>(data, start.wrapping_add(k.wrapping_mul(stride)))
        });
    }
    let base = data.as_ptr();
    core::array::from_fn(|k| {
        // SAFETY: row `k < N` lies inside the extent `rows_fit` checked.
        unsafe {
            base.add(start + k * stride)
                .cast::<[u8; W]>()
                .read_unaligned()
        }
    })
}

/// Writes `rows[k]` to `data` from `start + k * stride`: the inverse of
/// [`gather`], with the same check and the same argument. Where the check
/// fails, each row that fits is written through the slice.
#[inline(always)]
fn scatter<const N: usize, const W: usize>(
    data: &mut [u8],
    start: usize,
    stride: usize,
    rows: &[[u8; W]; N],
) {
    if !rows_fit(data.len(), start, stride, N, W) {
        for (k, row) in rows.iter().enumerate() {
            // Wrapping, as in `gather`.
            if let Some(to) = row_mut::<W>(data, start.wrapping_add(k.wrapping_mul(stride))) {
                *to = *row;
            }
        }
        return;
    }
    let base = data.as_mut_ptr();
    for (k, row) in rows.iter().enumerate() {
        // SAFETY: row `k < N` lies inside the extent `rows_fit` checked.
        unsafe {
            base.add(start + k * stride)
                .cast::<[u8; W]>()
                .write_unaligned(*row)
        }
    }
}

/// A pointer to `data[start]`, where `n` rows of `w` bytes from `start`,
/// `stride` apart, lie inside `data` ([`rows_fit`]); otherwise `None`.
///
/// The deblocking filter's standard edges read and write their rows
/// through it straight into and out of vector registers, and so do the
/// inverse transforms (`add_four_rows`), with the argument of [`gather`]:
/// one check before any row is touched, every row inside the checked
/// extent. Forming the pointer is safe; each access
/// through it is an `unsafe` load or store of at most `w` bytes from the
/// start of a row `k < n`.
#[inline(always)]
fn rows_at(data: &[u8], start: usize, stride: usize, n: usize, w: usize) -> Option<*const u8> {
    rows_fit(data.len(), start, stride, n, w).then(|| data.as_ptr().wrapping_add(start))
}

/// [`rows_at`] for writing.
#[inline(always)]
pub(super) fn rows_at_mut(
    data: &mut [u8],
    start: usize,
    stride: usize,
    n: usize,
    w: usize,
) -> Option<*mut u8> {
    rows_fit(data.len(), start, stride, n, w).then(|| data.as_mut_ptr().wrapping_add(start))
}

/// Sixteen bytes as a vector.
#[inline(always)]
fn vector16(bytes: [u8; 16]) -> __m128i {
    // SAFETY: `bytes` is 16 bytes long; SSE2 is part of the x86-64
    // baseline.
    unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
}

/// The sixteen bytes of `v`, as an array.
#[inline(always)]
fn array16(v: __m128i) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    // SAFETY: `bytes` is 16 bytes long; SSE2 is part of the x86-64
    // baseline.
    unsafe { _mm_storeu_si128(bytes.as_mut_ptr().cast(), v) }
    bytes
}

// ---------------------------------------------------------------------
// Luma interpolation (clause 8.4.2.2.1): one function per block width and
// quarter-sample position (`docs/design/h264.md`, "Kernel round: luma
// interpolation by position").
//
// A position is one or two of the samples G, b, h and j, averaged
// (`scalar::taps`). The width and the position are constants of each
// function, so the choice of samples, the sliding windows and the second
// sample's average fold away, and a row is the arithmetic and one store.
// Rows are reached through pointers formed after one check of the whole
// window and the whole block (`window_fits`), as `gather` does for an
// edge.
//
// The helpers below are `#[inline(always)]` functions without
// `#[target_feature]`: they use AVX2 intrinsics, and are only ever
// inlined into `interpolate`, which has the feature, so that the
// intrinsics inline there too. An `#[inline]` function with the feature is
// left out of line by the compiler at these sizes, and a call per row is
// what the earlier kernel paid.
// ---------------------------------------------------------------------

/// One row of a block in 16-bit lanes: a 256-bit vector for a 16-wide
/// block, a 128-bit one for an 8-wide block.
///
/// # Safety
///
/// Every function takes AVX2 as given: callers are `#[target_feature(enable
/// = "avx2")]` functions. Those that take a pointer read or write `W`
/// bytes from it (`b1`, `W + 5`), which the caller has checked lie inside
/// its slice.
trait Row: Copy {
    /// Whether a row is sixteen samples, rather than eight.
    const WIDE: bool;
    /// The `W` samples from `p`, widened.
    unsafe fn widen(p: *const u8) -> Self;
    /// The horizontal intermediates b1 of the `W + 5` samples from `p`
    /// (Equation 8-241), one lane per output column.
    unsafe fn b1(p: *const u8) -> Self;
    /// Lane-wise `a + b`.
    unsafe fn add(a: Self, b: Self) -> Self;
    /// Lane-wise `a - b`.
    unsafe fn sub(a: Self, b: Self) -> Self;
    /// Lane-wise `a << 2`.
    unsafe fn times4(a: Self) -> Self;
    /// `(v + 16) >> 5` (Equations 8-243 and 8-244), before the clip: one
    /// `pmulhrsw` by 1024, `(1024 v + 2^14) >> 15`, the same value.
    unsafe fn round5(v: Self) -> Self;
    /// The vertical 6-tap of six rows of intermediates b1, `(j1 + 512) >>
    /// 10` (Equations 8-246 and 8-247), clipped as [`Row::bytes`] will
    /// clip it: in 16-bit lanes, as [`centre_in_words`] shows.
    unsafe fn centre(rows: [Self; 6]) -> Self;
    /// The lanes as bytes, clipped to 0 to 255 (Clip1), in the low `W`
    /// bytes.
    unsafe fn bytes(v: Self) -> __m128i;
    /// [`Words`]'s weighting of the low `W` bytes of `x` (list 0) and `y`
    /// (list 1), clipped to 0 to 255, in the low `W` bytes.
    unsafe fn weigh_words(x: __m128i, y: __m128i, words: Words) -> __m128i;
    /// The `W` bytes from `p`, in the low `W` bytes.
    unsafe fn load(p: *const u8) -> __m128i;
    /// Writes the low `W` bytes of `v` to `p`.
    unsafe fn store(p: *mut u8, v: __m128i);
}

/// Sixteen lanes: a 16-wide block.
#[derive(Clone, Copy)]
struct Row16(__m256i);

/// Eight lanes: an 8-wide block.
#[derive(Clone, Copy)]
struct Row8(__m128i);

/// The 6-tap filter of Equation 8-241 as three `pmaddubsw` of byte pairs:
/// output column `i`'s samples `i` and `i + 1` against (1, -5), `i + 2` and
/// `i + 3` against (20, 20), and `i + 5` and `i + 4` against (1, -5) again.
/// Each pair's sum is -1,275 to 10,200, and the three's -2,550 to 10,710
/// (b1's range), so neither the `pmaddubsw` nor the adds saturate.
const PAIRS: [[usize; 2]; 3] = [[0, 1], [2, 3], [5, 4]];

/// The taps (1, -5) and (20, 20) of [`PAIRS`], as `pmaddubsw` takes them.
const OUTER: i16 = i16::from_le_bytes([1, (-5i8) as u8]);
const INNER: i16 = i16::from_le_bytes([20, 20]);

/// The shuffles of [`PAIRS`] for eight output columns: byte `2 i + k` of
/// pair `p`'s shuffle is where sample `i + PAIRS[p][k]` of the row sits in
/// the vector loaded, sample `s` at byte `s + shift`, or `s + shift + skip`
/// from sample 8 on.
#[expect(
    clippy::indexing_slicing,
    reason = "constant evaluation: an index out of range fails the build"
)]
const fn pair_shuffles(shift: usize, skip: usize) -> [[u8; 16]; 3] {
    let mut out = [[0u8; 16]; 3];
    let mut p = 0;
    while p < 3 {
        let mut i = 0;
        while i < 8 {
            let mut k = 0;
            while k < 2 {
                let s = i + PAIRS[p][k];
                out[p][2 * i + k] = (s + shift + if s < 8 { 0 } else { skip }) as u8;
                k += 1;
            }
            i += 1;
        }
        p += 1;
    }
    out
}

/// [`pair_shuffles`] of a 16-wide row loaded as samples 0 to 15 (the low
/// 128 bits, columns 0 to 7) and 5 to 20 (the high, columns 8 to 15).
const PAIRS16: [[[u8; 16]; 3]; 2] = [pair_shuffles(0, 0), pair_shuffles(3, 0)];

/// [`pair_shuffles`] of an 8-wide row loaded as samples 0 to 7 and 5 to
/// 12, one after the other.
const PAIRS8: [[u8; 16]; 3] = pair_shuffles(0, 3);

// The centre j in 16 bits ([`Row::centre`]). With `x = r0 + r5`, `y =
// r1 + r4` and `z = r2 + r3` of six rows of b1 (each -2,550 to 10,710, so
// `x`, `y` and `z` are -5,100 to 21,420), j is `Clip1((x - 5 y + 20 z +
// 512) >> 10)`:
//
// - `(x - 5 y + 20 z + 512) >> 10` is `(((x - y) >> 2) - y + z + 4 z +
//   128) >> 8` (the bits `>> 2` drops are below the units of the rest,
//   an integer), which is `((c >> 2) + z + 32) >> 6` with `c = ((x - y) >>
//   2) - y + z`, for the same reason; the last step is a `pmulhrsw` by 512.
// - `x - y` is within 26,520 and `((x - y) >> 2) - y` within -28,050 to
//   11,730; only `c` can leave 16 bits, and its add saturates. Where `c`
//   would be above 32,767, `z` is above 21,037, so the result from the
//   saturated `c`, `(8,191 + z + 32) >> 6`, is above 255, as the true one
//   is, and both clip to 255. Where `c` would be below -32,768, `z` is
//   below -4,718, and both are below 0 and clip to 0. `(c >> 2) + z` is
//   then within -13,292 to 29,611.
//
// `tests::the_centre_in_words_is_the_equation` checks the steps against
// the equation over that range, and the kernels' tests against the
// reference.

impl Row for Row16 {
    const WIDE: bool = true;
    #[inline(always)]
    unsafe fn widen(p: *const u8) -> Self {
        // SAFETY: AVX2 is available; `p` holds 16 bytes (the trait).
        unsafe { Row16(_mm256_cvtepu8_epi16(_mm_loadu_si128(p.cast()))) }
    }
    #[inline(always)]
    unsafe fn b1(p: *const u8) -> Self {
        // SAFETY: AVX2 is available; `p` holds 21 bytes (the trait), and
        // the loads read bytes 0 to 15 and 5 to 20.
        unsafe {
            let v = _mm256_set_m128i(_mm_loadu_si128(p.add(5).cast()), _mm_loadu_si128(p.cast()));
            let [[lo0, lo1, lo2], [hi0, hi1, hi2]] = PAIRS16;
            let first = _mm256_shuffle_epi8(v, _mm256_set_m128i(vector16(hi0), vector16(lo0)));
            let middle = _mm256_shuffle_epi8(v, _mm256_set_m128i(vector16(hi1), vector16(lo1)));
            let last = _mm256_shuffle_epi8(v, _mm256_set_m128i(vector16(hi2), vector16(lo2)));
            let (outer, inner) = (_mm256_set1_epi16(OUTER), _mm256_set1_epi16(INNER));
            Row16(_mm256_add_epi16(
                _mm256_add_epi16(
                    _mm256_maddubs_epi16(first, outer),
                    _mm256_maddubs_epi16(last, outer),
                ),
                _mm256_maddubs_epi16(middle, inner),
            ))
        }
    }
    #[inline(always)]
    unsafe fn add(a: Self, b: Self) -> Self {
        // SAFETY: AVX2 is available; a register operation.
        unsafe { Row16(_mm256_add_epi16(a.0, b.0)) }
    }
    #[inline(always)]
    unsafe fn sub(a: Self, b: Self) -> Self {
        // SAFETY: as above.
        unsafe { Row16(_mm256_sub_epi16(a.0, b.0)) }
    }
    #[inline(always)]
    unsafe fn times4(a: Self) -> Self {
        // SAFETY: as above.
        unsafe { Row16(_mm256_slli_epi16(a.0, 2)) }
    }
    #[inline(always)]
    unsafe fn round5(v: Self) -> Self {
        // SAFETY: as above.
        unsafe { Row16(_mm256_mulhrs_epi16(v.0, _mm256_set1_epi16(1024))) }
    }
    #[inline(always)]
    unsafe fn centre(rows: [Self; 6]) -> Self {
        // SAFETY: AVX2 is available; register operations. The steps are
        // those of "The centre j in 16 bits" above.
        unsafe {
            let [r0, r1, r2, r3, r4, r5] = rows.map(|r| r.0);
            let (x, y, z) = (
                _mm256_add_epi16(r0, r5),
                _mm256_add_epi16(r1, r4),
                _mm256_add_epi16(r2, r3),
            );
            let quarter = _mm256_srai_epi16(_mm256_sub_epi16(x, y), 2);
            let c = _mm256_adds_epi16(_mm256_sub_epi16(quarter, y), z);
            let d = _mm256_add_epi16(_mm256_srai_epi16(c, 2), z);
            Row16(_mm256_mulhrs_epi16(d, _mm256_set1_epi16(512)))
        }
    }
    #[inline(always)]
    unsafe fn bytes(v: Self) -> __m128i {
        // SAFETY: AVX2 is available; register operations.
        unsafe {
            _mm_packus_epi16(
                _mm256_castsi256_si128(v.0),
                _mm256_extracti128_si256(v.0, 1),
            )
        }
    }
    #[inline(always)]
    unsafe fn weigh_words(x: __m128i, y: __m128i, words: Words) -> __m128i {
        // SAFETY: AVX2 is available; register operations.
        unsafe {
            let pairs = _mm256_set_m128i(_mm_unpackhi_epi8(x, y), _mm_unpacklo_epi8(x, y));
            let sum = _mm256_maddubs_epi16(pairs, _mm256_set1_epi16(words.pair));
            let v = _mm256_add_epi16(
                _mm256_mulhrs_epi16(sum, _mm256_set1_epi16(words.scale)),
                _mm256_set1_epi16(words.offset),
            );
            Self::bytes(Row16(v))
        }
    }
    #[inline(always)]
    unsafe fn load(p: *const u8) -> __m128i {
        // SAFETY: `p` holds 16 bytes (the trait).
        unsafe { _mm_loadu_si128(p.cast()) }
    }
    #[inline(always)]
    unsafe fn store(p: *mut u8, v: __m128i) {
        // SAFETY: `p` holds 16 bytes (the trait).
        unsafe { _mm_storeu_si128(p.cast(), v) }
    }
}

impl Row for Row8 {
    const WIDE: bool = false;
    #[inline(always)]
    unsafe fn widen(p: *const u8) -> Self {
        // SAFETY: AVX2 is available; `p` holds 8 bytes (the trait).
        unsafe { Row8(_mm_cvtepu8_epi16(_mm_loadl_epi64(p.cast()))) }
    }
    #[inline(always)]
    unsafe fn b1(p: *const u8) -> Self {
        // SAFETY: AVX2 is available; `p` holds 13 bytes (the trait), and
        // the loads read bytes 0 to 7 and 5 to 12.
        unsafe {
            let v = _mm_unpacklo_epi64(_mm_loadl_epi64(p.cast()), _mm_loadl_epi64(p.add(5).cast()));
            let [first, middle, last] = PAIRS8;
            let (first, middle, last) = (
                _mm_shuffle_epi8(v, vector16(first)),
                _mm_shuffle_epi8(v, vector16(middle)),
                _mm_shuffle_epi8(v, vector16(last)),
            );
            let (outer, inner) = (_mm_set1_epi16(OUTER), _mm_set1_epi16(INNER));
            Row8(_mm_add_epi16(
                _mm_add_epi16(
                    _mm_maddubs_epi16(first, outer),
                    _mm_maddubs_epi16(last, outer),
                ),
                _mm_maddubs_epi16(middle, inner),
            ))
        }
    }
    #[inline(always)]
    unsafe fn add(a: Self, b: Self) -> Self {
        // SAFETY: AVX2 is available; a register operation.
        unsafe { Row8(_mm_add_epi16(a.0, b.0)) }
    }
    #[inline(always)]
    unsafe fn sub(a: Self, b: Self) -> Self {
        // SAFETY: as above.
        unsafe { Row8(_mm_sub_epi16(a.0, b.0)) }
    }
    #[inline(always)]
    unsafe fn times4(a: Self) -> Self {
        // SAFETY: as above.
        unsafe { Row8(_mm_slli_epi16(a.0, 2)) }
    }
    #[inline(always)]
    unsafe fn round5(v: Self) -> Self {
        // SAFETY: as above.
        unsafe { Row8(_mm_mulhrs_epi16(v.0, _mm_set1_epi16(1024))) }
    }
    #[inline(always)]
    unsafe fn centre(rows: [Self; 6]) -> Self {
        // SAFETY: AVX2 is available; register operations, as Row16's.
        unsafe {
            let [r0, r1, r2, r3, r4, r5] = rows.map(|r| r.0);
            let (x, y, z) = (
                _mm_add_epi16(r0, r5),
                _mm_add_epi16(r1, r4),
                _mm_add_epi16(r2, r3),
            );
            let quarter = _mm_srai_epi16(_mm_sub_epi16(x, y), 2);
            let c = _mm_adds_epi16(_mm_sub_epi16(quarter, y), z);
            let d = _mm_add_epi16(_mm_srai_epi16(c, 2), z);
            Row8(_mm_mulhrs_epi16(d, _mm_set1_epi16(512)))
        }
    }
    #[inline(always)]
    unsafe fn bytes(v: Self) -> __m128i {
        // SAFETY: a register operation.
        unsafe { _mm_packus_epi16(v.0, v.0) }
    }
    #[inline(always)]
    unsafe fn weigh_words(x: __m128i, y: __m128i, words: Words) -> __m128i {
        // SAFETY: AVX2 is available; register operations.
        unsafe {
            let sum = _mm_maddubs_epi16(_mm_unpacklo_epi8(x, y), _mm_set1_epi16(words.pair));
            let v = _mm_add_epi16(
                _mm_mulhrs_epi16(sum, _mm_set1_epi16(words.scale)),
                _mm_set1_epi16(words.offset),
            );
            _mm_packus_epi16(v, v)
        }
    }
    #[inline(always)]
    unsafe fn load(p: *const u8) -> __m128i {
        // SAFETY: `p` holds 8 bytes (the trait).
        unsafe { _mm_loadl_epi64(p.cast()) }
    }
    #[inline(always)]
    unsafe fn store(p: *mut u8, v: __m128i) {
        // SAFETY: `p` holds 8 bytes (the trait).
        unsafe { _mm_storel_epi64(p.cast(), v) }
    }
}

/// The 6-tap filter of Equations 8-241 and 8-242, `(a + f) - 5 (b + e) +
/// 20 (c + d)`, as `(a + f) + 5 t` with `t = 4 (c + d) - (b + e)`: shifts
/// and adds, no multiply. With samples of at most 255 every value stays
/// inside 16 bits (`t` is -510 to 2,040).
#[inline(always)]
unsafe fn tap6<R: Row>(v: [R; 6]) -> R {
    // SAFETY: AVX2 is available (the trait); register operations.
    unsafe {
        let [a, b, c, d, e, f] = v;
        let t = R::sub(R::times4(R::add(c, d)), R::add(b, e));
        R::add(R::add(a, f), R::add(t, R::times4(t)))
    }
}

/// The horizontal intermediates b1 of the source row at `p` (Equation
/// 8-241), one lane per output column ([`Row::b1`]).
///
/// # Safety
///
/// AVX2 is available, and `p` holds `W + 5` bytes.
#[inline(always)]
unsafe fn horizontal<R: Row>(p: *const u8) -> R {
    // SAFETY: the contract.
    unsafe { R::b1(p) }
}

/// `taps` as constants of `interpolate`: `T::FIRST` and `T::SECOND`.
struct Position<const XF: u8, const YF: u8>;

impl<const XF: u8, const YF: u8> Position<XF, YF> {
    const TAPS: (Tap, Option<Tap>) = taps(XF, YF);
    /// Whether a tap is j, which needs the b1 of every row.
    const J: bool = matches!(Self::TAPS.0, Tap::J);
    /// The column of h, where a tap is h.
    const H: Option<usize> = match Self::TAPS {
        (Tap::H { dx }, _) | (_, Some(Tap::H { dx })) => Some(dx),
        _ => None,
    };
    /// What `interpolate` takes from `taps` without testing it: j, where a
    /// position has it, is its first tap, and no position has two h taps.
    /// A table that broke either fails to compile.
    const CHECKED: () = assert!(
        !matches!(Self::TAPS.1, Some(Tap::J))
            && !matches!(Self::TAPS, (Tap::H { .. }, Some(Tap::H { .. })))
    );
}

/// The five rows from `p`, `stride` apart, each read by `read`, as the
/// last five of a window of six: the first is a placeholder, which the
/// first row's step shifts out.
///
/// # Safety
///
/// AVX2 is available, and `read` may read each of the five rows.
#[inline(always)]
unsafe fn five<R: Row>(p: *const u8, stride: usize, read: unsafe fn(*const u8) -> R) -> [R; 6] {
    // SAFETY: the contract.
    unsafe {
        let first = read(p);
        [
            first,
            first,
            read(p.add(stride)),
            read(p.add(2 * stride)),
            read(p.add(3 * stride)),
            read(p.add(4 * stride)),
        ]
    }
}

/// The samples of one tap for output row `y`, as bytes: `row` is the
/// source row `y + 2`, `b1` the intermediates of rows `y` to `y + 5`
/// where `J` is set, and `column` the samples of those rows at h's
/// column.
///
/// # Safety
///
/// AVX2 is available, and the source rows `y + 2` and `y + 3` are inside
/// the window.
#[inline(always)]
unsafe fn sample<R: Row, const J: bool>(
    tap: Tap,
    row: *const u8,
    stride: usize,
    b1: &[R; 6],
    column: &[R; 6],
) -> __m128i {
    // SAFETY: the contract; `dy` and `dx` are 0 or 1.
    unsafe {
        match tap {
            Tap::Int { dx, dy } => R::load(row.add(dy * stride + 2 + dx)),
            Tap::B { dy } => {
                let [_, _, b2, b3, _, _] = *b1;
                let b = match (J, dy) {
                    (true, 0) => b2,
                    (true, _) => b3,
                    (false, _) => horizontal::<R>(row.add(dy * stride)),
                };
                R::bytes(R::round5(b))
            }
            Tap::H { .. } => R::bytes(R::round5(tap6(*column))),
            Tap::J => R::bytes(R::centre(*b1)),
        }
    }
}

/// The prediction of an `R` wide (8 or 16) by `h` block at the
/// quarter-sample position (`XF`, `YF`), written to `dst` or averaged into
/// it.
///
/// A vertical filter keeps the six rows it needs in registers and moves
/// down one row per output row, so each source row is widened, or filtered
/// horizontally, once; b next to j is the j window's middle row rounded.
/// The window holds rows `y - 1` to `y + 4` when row `y` starts, and its
/// first step reads row `y + 5`, which every row of the block has, so the
/// loop has no test for its last row.
///
/// # Safety
///
/// AVX2 is available. `src` holds `h + 5` rows of `R`'s width plus 5
/// samples, `stride` apart, and `dst` holds `h` rows of `R`'s width,
/// `dst_stride` apart (`window_fits`); `h` is 1 to 16.
#[inline(always)]
unsafe fn interpolate<R: Row, F: LumaFinish, const XF: u8, const YF: u8>(
    dst: *mut u8,
    dst_stride: usize,
    src: *const u8,
    stride: usize,
    h: usize,
    finish: F,
) {
    // SAFETY: the source rows read are rows 0 to `h + 4`, the block's
    // rows written 0 to `h - 1`, and each read or write stays inside its
    // row, so every pointer that is read or written through lies inside
    // the extent the caller checked. The pointers step on with
    // `wrapping_add`, so the steps past the last row form no pointer that
    // is used.
    unsafe {
        let () = Position::<XF, YF>::CHECKED;
        let (first, second) = Position::<XF, YF>::TAPS;
        let j = Position::<XF, YF>::J;
        let hx = Position::<XF, YF>::H;
        // b1 of rows y to y + 5, where j is needed.
        let mut b1 = [R::widen(src); 6];
        if j {
            b1 = five(src, stride, horizontal::<R>);
        }
        // The samples of rows y to y + 5 at column 2 + dx, where h is.
        let mut column = b1;
        if let Some(dx) = hx {
            column = five(src.add(2 + dx), stride, R::widen);
        }
        // Source rows y + 2 and y + 5, and the block's row y.
        let (mut row, mut last, mut out) = (src.add(2 * stride), src.add(5 * stride), dst);
        for _ in 0..h {
            if j {
                let [_, a, b, c, d, e] = b1;
                b1 = [a, b, c, d, e, horizontal::<R>(last)];
            }
            if let Some(dx) = hx {
                let [_, a, b, c, d, e] = column;
                column = [a, b, c, d, e, R::widen(last.add(2 + dx))];
            }
            let mut v = if j {
                sample::<R, true>(first, row, stride, &b1, &column)
            } else {
                sample::<R, false>(first, row, stride, &b1, &column)
            };
            if let Some(tap) = second {
                let w = if j {
                    sample::<R, true>(tap, row, stride, &b1, &column)
                } else {
                    sample::<R, false>(tap, row, stride, &b1, &column)
                };
                v = _mm_avg_epu8(v, w);
            }
            v = finish.finish::<R>(v, out);
            R::store(out, v);
            row = row.wrapping_add(stride);
            last = last.wrapping_add(stride);
            out = out.wrapping_add(dst_stride);
        }
    }
}

/// Whether `n` rows of `w` bytes, `stride` apart from the start of a slice
/// of `len` bytes, lie inside it: [`rows_fit`] from offset 0.
#[inline(always)]
fn window_fits(len: usize, stride: usize, n: usize, w: usize) -> bool {
    rows_fit(len, 0, stride, n, w)
}

/// Luma quarter-sample interpolation (clause 8.4.2.2.1) of a 4-, 8- or
/// 16-wide block, 1 to 16 rows, finished into `dst` by `finish`: written,
/// averaged into it (`(dst + pred + 1) >> 1`, which `pavgb` computes) or
/// weighted.
///
/// A 4-wide block goes through the 8-wide kernel on copies
/// ([`luma4_padded`]). For the others, the views hold exactly the window and
/// the block, and one check of each (`window_fits`) comes before any
/// pointer is formed. Any other shape, or a check that fails, which no view
/// allows, goes to the reference.
#[target_feature(enable = "avx2")]
unsafe fn luma_mc_into<F: LumaFinish>(
    mut dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    finish: F,
) {
    let (w, h) = (dst.w(), dst.h());
    if w == 4 && (1..=16).contains(&h) && src.w() == 9 && src.h() == h + 5 {
        // SAFETY: AVX2 is available (this function's feature).
        return unsafe { luma4_padded(dst, src, xf, yf, finish) };
    }
    let (stride, dst_stride) = (src.stride(), dst.stride());
    let fits = matches!(w, 8 | 16)
        && (1..=16).contains(&h)
        && src.w() == w + 5
        && src.h() == h + 5
        && window_fits(src.samples().len(), stride, h + 5, w + 5)
        && window_fits(dst.samples_mut().len(), dst_stride, h, w);
    if !fits {
        return finish.reference(dst, src, xf, yf);
    }
    let (from, to) = (src.samples().as_ptr(), dst.samples_mut().as_mut_ptr());
    // SAFETY: AVX2 is available (this function's feature); `fits` checked
    // the window and the block.
    unsafe { luma_mc_ptrs(to, dst_stride, from, stride, w, h, xf, yf, finish) }
}

/// A 4-wide block of [`luma_mc_into`], `h` rows of 1 to 16, by the 8-wide
/// kernel on copies: the window's rows go into rows of 16 bytes, the
/// block's into rows of 8, the kernel predicts and finishes 8 columns, and
/// the block's 4 go back. An output column's taps read only the window's
/// columns from its own to five past it (clause 8.4.2.2.1), so the first 4
/// columns are the 4-wide block's whatever the copy holds past the
/// window's 9; the finishes that read the block (averaging, and weighting
/// both lists) find its rows in the copy. A row is copied as 16 bytes where
/// the window's samples hold that many from its start, which every row but
/// the last few does, and as its 9 otherwise.
///
/// # Safety
///
/// AVX2 is available; `dst` is 4 wide and 1 to 16 rows, and `src` 9 wide
/// and 5 rows more.
#[target_feature(enable = "avx2")]
unsafe fn luma4_padded<F: LumaFinish>(
    mut dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    finish: F,
) {
    let h = dst.h();
    let (samples, stride) = (src.samples(), src.stride());
    let mut window = [[0u8; 16]; 21];
    for (r, row) in window.iter_mut().enumerate().take(h + 5) {
        let from = samples.get(r.saturating_mul(stride)..).unwrap_or_default();
        if let Some(whole) = from.first_chunk::<16>() {
            *row = *whole;
        } else if let (Some(to), Some(from)) = (row.first_chunk_mut::<9>(), from.first_chunk::<9>())
        {
            *to = *from;
        }
    }
    let mut block = [[0u8; 8]; 16];
    for (to, from) in block.iter_mut().zip(dst.rows_mut()) {
        if let (Some(to), Some(from)) = (to.first_chunk_mut::<4>(), from.first_chunk::<4>()) {
            *to = *from;
        }
    }
    // SAFETY: AVX2 is available; `window` holds 21 rows of 16 samples, 16
    // apart, so the `h + 5` rows of 13 the kernel reads (`h` at most 16),
    // and `block` 16 rows of 8, 8 apart, so the `h` it writes.
    unsafe {
        luma_mc_ptrs(
            block.as_mut_ptr().cast::<u8>(),
            8,
            window.as_ptr().cast::<u8>(),
            16,
            8,
            h,
            xf,
            yf,
            finish,
        );
    }
    for (to, from) in dst.rows_mut().zip(&block) {
        if let (Some(to), Some(from)) = (to.first_chunk_mut::<4>(), from.first_chunk::<4>()) {
            *to = *from;
        }
    }
}

/// [`luma_mc_into`] after its check: the prediction of a `w` by `h` block
/// at `to`, rows `dst_stride` apart, from the window at `from`, rows
/// `stride` apart, finished by `finish`. A width other than 8 or 16 writes
/// nothing.
///
/// # Safety
///
/// AVX2 is available, and the extents `luma_mc_into` checks hold: `from`
/// holds `h + 5` rows of `w + 5` samples, `stride` apart, and `to` holds
/// `h` rows of `w`, `dst_stride` apart (`window_fits` of each); `h` is 1
/// to 16.
#[inline(always)]
#[allow(
    clippy::too_many_arguments,
    reason = "the parts of two views, unpacked"
)]
unsafe fn luma_mc_ptrs<F: LumaFinish>(
    to: *mut u8,
    dst_stride: usize,
    from: *const u8,
    stride: usize,
    w: usize,
    h: usize,
    xf: u8,
    yf: u8,
    finish: F,
) {
    if !matches!(w, 8 | 16) {
        return;
    }
    macro_rules! by_position {
        ($($index:literal => $row:ty, $xf:literal, $yf:literal;)*) => {
            match usize::from(xf & 3) | usize::from(yf & 3) << 2 | usize::from(w == 16) << 4 {
                $($index => interpolate::<$row, F, $xf, $yf>(to, dst_stride, from, stride, h, finish),)*
                _ => {}
            }
        };
    }
    // SAFETY: AVX2 is available, and the window and the block
    // `interpolate` reads and writes lie inside their extents (the
    // contract).
    unsafe {
        // One table of every width and position: the index is xFrac,
        // yFrac and whether the block is 16 wide.
        by_position! {
            0 => Row8, 0, 0; 1 => Row8, 1, 0; 2 => Row8, 2, 0; 3 => Row8, 3, 0;
            4 => Row8, 0, 1; 5 => Row8, 1, 1; 6 => Row8, 2, 1; 7 => Row8, 3, 1;
            8 => Row8, 0, 2; 9 => Row8, 1, 2; 10 => Row8, 2, 2; 11 => Row8, 3, 2;
            12 => Row8, 0, 3; 13 => Row8, 1, 3; 14 => Row8, 2, 3; 15 => Row8, 3, 3;
            16 => Row16, 0, 0; 17 => Row16, 1, 0; 18 => Row16, 2, 0; 19 => Row16, 3, 0;
            20 => Row16, 0, 1; 21 => Row16, 1, 1; 22 => Row16, 2, 1; 23 => Row16, 3, 1;
            24 => Row16, 0, 2; 25 => Row16, 1, 2; 26 => Row16, 2, 2; 27 => Row16, 3, 2;
            28 => Row16, 0, 3; 29 => Row16, 1, 3; 30 => Row16, 2, 3; 31 => Row16, 3, 3;
        }
    }
}

/// Luma quarter-sample interpolation (clause 8.4.2.2.1).
#[target_feature(enable = "avx2")]
pub(super) unsafe fn luma_mc(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    // SAFETY: AVX2 is available.
    unsafe { luma_mc_into(dst, src, xf, yf, Average(false)) }
}

/// [`luma_mc`], with each predicted sample averaged into `dst`:
/// `(dst + pred + 1) >> 1`, which is what `pavgb` computes.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn luma_mc_avg(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    // SAFETY: AVX2 is available.
    unsafe { luma_mc_into(dst, src, xf, yf, Average(true)) }
}

/// [`luma_mc`], with the prediction weighted into `dst` as `weighting`
/// says ([`Kernels::luma_mc_weighted`]). A weighting outside
/// [`Weighting::in_range`] goes to the reference, which leaves `dst`
/// alone.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn luma_mc_weighted(
    dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
    weighting: Weighting,
) {
    if !weighting.in_range() {
        return Scalar.luma_mc_weighted(dst, src, xf, yf, weighting);
    }
    // SAFETY: AVX2 is available.
    unsafe { luma_mc_into(dst, src, xf, yf, Weigh(Weights::new(weighting), weighting)) }
}

/// The last step of a row of [`interpolate`]: what becomes of the
/// prediction `v` before it is stored at `out`.
///
/// # Safety
///
/// AVX2 is available, and `out` holds a row of `R`'s width ([`Row`]).
trait LumaFinish: Copy {
    /// The row to store, from the prediction `v` and the block's row at
    /// `out`.
    unsafe fn finish<R: Row>(self, v: __m128i, out: *const u8) -> __m128i;
    /// The reference's form of the whole kernel, for the shapes the vector
    /// kernel does not take.
    fn reference(self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8);
}

/// Written as predicted, or averaged into the block: one test a row, which
/// the plain and the averaging kernels share, as one function.
#[derive(Clone, Copy)]
struct Average(bool);

impl LumaFinish for Average {
    #[inline(always)]
    unsafe fn finish<R: Row>(self, v: __m128i, out: *const u8) -> __m128i {
        if self.0 {
            // SAFETY: the trait's.
            unsafe { _mm_avg_epu8(v, R::load(out)) }
        } else {
            v
        }
    }

    fn reference(self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        if self.0 {
            Scalar.luma_mc_avg(dst, src, xf, yf);
        } else {
            Scalar.luma_mc(dst, src, xf, yf);
        }
    }
}

/// Weighted into the block, in kernels of their own, so that the plain
/// and averaging kernels stay as they are: the vectors of the weighting,
/// made once a block, and the weighting.
#[derive(Clone, Copy)]
struct Weigh(Weights, Weighting);

impl LumaFinish for Weigh {
    #[inline(always)]
    unsafe fn finish<R: Row>(self, v: __m128i, out: *const u8) -> __m128i {
        let weights = self.0;
        // List 0 is the block's samples where both lists are weighted;
        // one list's weighting takes `v` twice and a weight of 0 for the
        // second.
        // SAFETY: the trait's.
        let x = if weights.both {
            unsafe { R::load(out) }
        } else {
            v
        };
        if R::WIDE {
            // SAFETY: AVX2 is available (the trait).
            unsafe { weigh16(x, v, &weights) }
        } else {
            weigh8(x, v, &weights)
        }
    }

    fn reference(self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        Scalar.luma_mc_weighted(dst, src, xf, yf, self.1);
    }
}

/// Equations 8-286 to 8-288 of 16 samples: [`weigh8`] for each half,
/// in 256-bit lanes.
///
/// # Safety
///
/// AVX2 is available.
#[inline(always)]
unsafe fn weigh16(x: __m128i, y: __m128i, weights: &Weights) -> __m128i {
    // SAFETY: AVX2 is available; register operations.
    unsafe {
        let pair = _mm256_broadcastsi128_si256(weights.pair);
        let bias = _mm256_broadcastsi128_si256(weights.bias);
        let low = _mm256_cvtepu8_epi16(_mm_unpacklo_epi8(x, y));
        let high = _mm256_cvtepu8_epi16(_mm_unpackhi_epi8(x, y));
        let first = _mm256_sra_epi32(
            _mm256_add_epi32(_mm256_madd_epi16(low, pair), bias),
            weights.shift,
        );
        let second = _mm256_sra_epi32(
            _mm256_add_epi32(_mm256_madd_epi16(high, pair), bias),
            weights.shift,
        );
        // Packing works within each 128-bit half: samples 0 to 3, 8 to
        // 11, 4 to 7 and 12 to 15, in 64-bit quarters put back in order.
        let words = _mm256_permute4x64_epi64::<0b11_01_10_00>(_mm256_packs_epi32(first, second));
        _mm_packus_epi16(
            _mm256_castsi256_si128(words),
            _mm256_extracti128_si256::<1>(words),
        )
    }
}

/// A weighting (clause 8.4.2.3.2) in 16-bit lanes, where it can be one,
/// for [`weigh_full`]: one `pmaddubsw` of each pair of samples, list 0's
/// and list 1's bytes, against the two weights as bytes, `s = x w0 + y
/// w1`; the rounding and the shift as one `pmulhrsw`; and the offset.
///
/// - `s` is exact where the weights are -128 to 127, the positive ones add
///   to at most 128 and the negative ones to at least -128: then `s` is
///   within 255 * 128 = 32,640 and `pmaddubsw` does not saturate.
/// - Equation 8-288's `(s + 2^logWD) >> (logWD + 1)` is `pmulhrsw` by
///   `2^(14 - logWD)`, `(s 2^(14 - logWD) + 2^14) >> 15`, the same value.
///   Equations 8-286 and 8-287 (one list, logWD 1 or more) take the
///   prediction as both samples and 0 as the second weight, and `(s +
///   2^(logWD - 1)) >> logWD` is `pmulhrsw` by `2^(15 - logWD)`. One list
///   with logWD 0 has no rounding to fold and keeps the 32-bit form.
/// - The product is within 2^14 and the offset -128 to 127, so their sum
///   fits; the pack to bytes is the clip.
#[derive(Clone, Copy)]
pub(super) struct Words {
    /// `w0` and `w1` as the low and high byte of a 16-bit lane.
    pair: i16,
    /// The `pmulhrsw` multiplier.
    scale: i16,
    /// The offset.
    offset: i16,
}

impl Words {
    /// `weighting` in 16-bit lanes, or `None` where it cannot be one (see
    /// [`Words`]).
    pub(super) fn new(weighting: Weighting) -> Option<Words> {
        let (w0, w1, scale, o) = match weighting {
            Weighting::Uni { log_wd, w, o } if (1..=7).contains(&log_wd) => (w, 0, 15 - log_wd, o),
            Weighting::Bi { log_wd, w0, w1, o } if (0..=7).contains(&log_wd) => {
                (w0, w1, 14 - log_wd, o)
            }
            _ => return None,
        };
        let (b0, b1) = (i8::try_from(w0).ok()?, i8::try_from(w1).ok()?);
        let fits = w0.max(0) + w1.max(0) <= 128
            && w0.min(0) + w1.min(0) >= -128
            && (-128..=127).contains(&o);
        fits.then(|| Words {
            pair: i16::from_le_bytes([b0.to_le_bytes()[0], b1.to_le_bytes()[0]]),
            scale: 1 << scale,
            offset: o as i16,
        })
    }
}

/// The full-sample prediction of an 8- or 16-wide block, `h` rows,
/// weighted into the block at `to` as `words` says: list 0 the block's
/// samples where `both` (Equation 8-288), the prediction otherwise. The
/// window at `from` is [`luma_mc_ptrs`]'s; its full samples start at row
/// 2, column 2. A width other than 8 or 16 writes nothing.
///
/// # Safety
///
/// AVX2 is available, and the extents of [`luma_mc_ptrs`] hold.
#[target_feature(enable = "avx2")]
#[allow(
    clippy::too_many_arguments,
    reason = "the parts of two views, unpacked"
)]
pub(super) unsafe fn weigh_full(
    to: *mut u8,
    dst_stride: usize,
    from: *const u8,
    stride: usize,
    (w, h): (usize, usize),
    words: Words,
    both: bool,
) {
    let from = from.wrapping_add(2 * stride + 2);
    // SAFETY: AVX2 is available; the rows read are the window's rows 2
    // to `h + 1` from column 2, and the rows written the block's, `w`
    // samples each (the contract).
    unsafe {
        match (w, both) {
            (16, true) => weigh_rows::<Row16, true>(to, dst_stride, from, stride, h, words),
            (16, false) => weigh_rows::<Row16, false>(to, dst_stride, from, stride, h, words),
            (8, true) => weigh_rows::<Row8, true>(to, dst_stride, from, stride, h, words),
            (8, false) => weigh_rows::<Row8, false>(to, dst_stride, from, stride, h, words),
            _ => {}
        }
    }
}

/// [`weigh_full`] of `R`'s width, list 0 the block's samples where `BI`.
///
/// # Safety
///
/// AVX2 is available; `from` holds `h` rows of `R`'s width, `stride`
/// apart, and `to` as many, `dst_stride` apart.
#[inline(always)]
unsafe fn weigh_rows<R: Row, const BI: bool>(
    to: *mut u8,
    dst_stride: usize,
    from: *const u8,
    stride: usize,
    h: usize,
    words: Words,
) {
    let (mut row, mut out) = (from, to);
    // SAFETY: the contract; the pointers step on with `wrapping_add`, so
    // the steps past the last row form no pointer that is used.
    unsafe {
        for _ in 0..h {
            let y = R::load(row);
            let x = if BI { R::load(out) } else { y };
            R::store(out, R::weigh_words(x, y, words));
            row = row.wrapping_add(stride);
            out = out.wrapping_add(dst_stride);
        }
    }
}

// ---------------------------------------------------------------------
// Chroma interpolation (clause 8.4.2.2.2), in SSE2 alone. SSE2 is part of
// every x86-64 CPU, so these functions need no `#[target_feature]` and no
// detection: they are ordinary functions, inlined into the dispatch and
// from there into the decoder's prediction loop, which a call into an AVX2
// function cannot be. A chroma block is 2 to 8 samples wide and the work
// per block is a few dozen instructions, so the call is what decides
// (`docs/design/h264.md`, "Kernel round: chroma interpolation
// inlined").
//
// Equation 8-266 weights the four neighbours of a sample by
// (8 - xFrac)(8 - yFrac), xFrac(8 - yFrac), (8 - xFrac)yFrac and
// xFrac yFrac. That is a horizontal step and a vertical one:
//
//   H(row)[i] = 8 row[i] + xFrac (row[i + 1] - row[i])
//   pred[i]   = (8 H(up)[i] + yFrac (H(down)[i] - H(up)[i]) + 32) >> 6
//
// with the same integer values, since both sides expand to the same sum.
// H is 0 to 2040 and the sum before the shift 0 to 16,352, so every value
// fits a 16-bit lane, and a difference that goes negative on the way
// wraps and comes back exactly. Each source row is filtered horizontally
// once and serves two output rows: one multiply per row each way, where
// the four-weight form takes four per row.
// ---------------------------------------------------------------------

/// `N` bytes of `data` from `at`, or zeros where `data` is shorter. Read
/// as one array: one bounds check, and no pointer formed.
#[inline(always)]
fn bytes<const N: usize>(data: &[u8], at: usize) -> [u8; N] {
    data.get(at..)
        .and_then(|rest| rest.first_chunk::<N>())
        .copied()
        .unwrap_or([0; N])
}

/// Eight bytes as the low half of a vector.
#[inline(always)]
fn vector8(bytes: [u8; 8]) -> __m128i {
    // SAFETY: SSE2 is part of the x86-64 baseline; a register operation.
    unsafe { _mm_cvtsi64_si128(i64::from_le_bytes(bytes)) }
}

/// The first `W` of `bytes` (`W` 2 or 4), zero-extended to 32 bits.
#[inline(always)]
fn word<const W: usize>(bytes: &[u8]) -> u64 {
    let mut word = [0u8; 4];
    if let (Some(to), Some(from)) = (word.first_chunk_mut::<W>(), bytes.first_chunk::<W>()) {
        *to = *from;
    }
    u64::from(u32::from_le_bytes(word))
}

/// The low eight bytes of `v`, as an array.
#[inline(always)]
fn low8(v: __m128i) -> [u8; 8] {
    // SAFETY: SSE2 is part of the x86-64 baseline; a register operation.
    unsafe { _mm_cvtsi128_si64(v) }.to_le_bytes()
}

/// The `N` bytes of `data` from `at`, where `data` holds them.
#[inline(always)]
fn row_mut<const N: usize>(data: &mut [u8], at: usize) -> Option<&mut [u8; N]> {
    data.get_mut(at..)
        .and_then(|rest| rest.first_chunk_mut::<N>())
}

/// `H` of a row, given its samples and their right neighbours as 16-bit
/// lanes.
#[inline(always)]
fn chroma_horizontal(left: __m128i, right: __m128i, x: __m128i) -> __m128i {
    // SAFETY: SSE2 is part of the x86-64 baseline; register operations.
    unsafe {
        _mm_add_epi16(
            _mm_slli_epi16(left, 3),
            _mm_mullo_epi16(_mm_sub_epi16(right, left), x),
        )
    }
}

/// The predicted samples from `H` of the rows above and below, packed to
/// bytes in the low half and repeated in the high one.
#[inline(always)]
fn chroma_vertical(up: __m128i, down: __m128i, y: __m128i) -> __m128i {
    // SAFETY: SSE2 is part of the x86-64 baseline; register operations.
    unsafe {
        let sum = _mm_add_epi16(
            _mm_slli_epi16(up, 3),
            _mm_mullo_epi16(_mm_sub_epi16(down, up), y),
        );
        let v = _mm_srli_epi16(_mm_add_epi16(sum, _mm_set1_epi16(32)), 6);
        _mm_packus_epi16(v, v)
    }
}

/// Chroma eighth-sample interpolation (Equation 8-266) of a `w` by `h`
/// block from its `(w + 1)` by `(h + 1)` window, `w` 2, 4 or 8. With
/// `average`, each predicted sample is averaged into `dst` as
/// `(dst + pred + 1) >> 1` instead of stored. Another width, a source of
/// another shape, or with `average` a block taller than 16 rows, leaves
/// `dst` unchanged.
///
/// Rows are reached by offset, `y * stride`, each read or written as one
/// fixed-size array: one bounds check a row, where the views' row
/// iterators take several. The views' constructors checked that every row
/// lies inside its slice, so no check fails; one that did would read zeros
/// or leave its row unwritten.
#[inline(always)]
pub(super) fn chroma_mc(mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8, finish: Finish) {
    let (w, h) = (dst.w(), dst.h());
    // The averaging and weighting forms leave a block taller than 16 rows
    // alone, as the trait's reference does (`chroma_mc_avg_by_parts`).
    if src.w() != w + 1 || src.h() != h + 1 || (!matches!(finish, Finish::Write) && h > 16) {
        return;
    }
    // SAFETY: SSE2 is part of the x86-64 baseline; register operations.
    let (x, y) = unsafe {
        (
            _mm_set1_epi16(i16::from(xf & 7)),
            _mm_set1_epi16(i16::from(yf & 7)),
        )
    };
    let rows = Rows {
        from: src.samples(),
        from_stride: src.stride(),
        to_stride: dst.stride(),
        h,
        x,
        y,
        finish,
    };
    let to = dst.samples_mut();
    match w {
        8 => rows.eight(to),
        4 => rows.pairs::<4, 5>(to),
        2 => rows.pairs::<2, 3>(to),
        _ => {}
    }
}

/// What [`chroma_mc`] works from: the source window by offset, the
/// destination's stride and the weights.
struct Rows<'a> {
    from: &'a [u8],
    from_stride: usize,
    to_stride: usize,
    h: usize,
    x: __m128i,
    y: __m128i,
    finish: Finish,
}

/// What [`chroma_mc`] does with its prediction. The decoder's calls name
/// the form, and with the function inlined into each, the test of it
/// folds away.
#[derive(Clone, Copy)]
pub(super) enum Finish {
    /// Written to the block.
    Write,
    /// Averaged into the block, `(dst + pred + 1) >> 1`.
    Average,
    /// Weighted into the block ([`crate::Kernels::chroma_mc_weighted`]).
    Weigh(Weights),
}

impl Finish {
    /// [`Finish::Weigh`] of `weighting`, which lies in
    /// [`Weighting::in_range`].
    #[inline(always)]
    pub(super) fn weigh(weighting: Weighting) -> Finish {
        Finish::Weigh(Weights::new(weighting))
    }

    /// The row to store, of the prediction `pred` and of the block's
    /// samples, which `old` reads, in the low eight bytes.
    #[inline(always)]
    fn apply(self, pred: __m128i, old: impl FnOnce() -> __m128i) -> __m128i {
        match self {
            Finish::Write => pred,
            // SAFETY: SSE2 is part of the x86-64 baseline; register
            // operations.
            Finish::Average => unsafe { _mm_avg_epu8(pred, old()) },
            Finish::Weigh(weights) => {
                let x = if weights.both { old() } else { pred };
                weigh8(x, pred, &weights)
            }
        }
    }
}

/// A weighting (clause 8.4.2.3.2) as the vector kernels apply it. Each
/// equation is one multiply-add of 16-bit pairs into 32 bits: list 0's
/// sample and list 1's against their weights, `x w0 + y w1`, then the
/// rounding and the offset as one addition, `bias`, and the shift, since
///
///   ((x w0 + y w1 + 2^logWD) >> (logWD + 1)) + o
///     = (x w0 + y w1 + 2^logWD + o 2^(logWD + 1)) >> (logWD + 1)
///
/// (Equation 8-288) and likewise `(x w + 2^(logWD - 1) + o 2^logWD) >>
/// logWD` for Equations 8-286 and 8-287, where one list takes its sample
/// with a weight of 0 as the second. Within the ranges of
/// [`Weighting::in_range`] every value fits 32 bits with room to spare
/// (at most 255 * 256 + 2^7 + 128 * 2^8 before the shift), and the sum
/// of two products would not fit the 16 bits of `pmullw`.
#[derive(Clone, Copy)]
pub(super) struct Weights {
    /// `w0` in the low 16 bits of each 32-bit lane, `w1` in the high.
    pair: __m128i,
    /// The rounding and the offset, in each 32-bit lane.
    bias: __m128i,
    /// The shift, as `psrad` takes it.
    shift: __m128i,
    /// Whether list 0 is the block's samples ([`Weighting::Bi`]).
    both: bool,
}

impl Weights {
    /// The weights of `weighting`, which lies in [`Weighting::in_range`].
    #[inline(always)]
    fn new(weighting: Weighting) -> Weights {
        let (w0, w1, bias, shift, both) = match weighting {
            Weighting::Uni { log_wd, w, o } => {
                let round = if log_wd >= 1 { 1 << (log_wd - 1) } else { 0 };
                (w, 0, round + (o << log_wd), log_wd, false)
            }
            Weighting::Bi { log_wd, w0, w1, o } => (
                w0,
                w1,
                (1 << log_wd) + (o << (log_wd + 1)),
                log_wd + 1,
                true,
            ),
        };
        let pair = (w0 & 0xffff) | (w1 << 16);
        // SAFETY: SSE2 is part of the x86-64 baseline; register
        // operations.
        unsafe {
            Weights {
                pair: _mm_set1_epi32(pair),
                bias: _mm_set1_epi32(bias),
                shift: _mm_cvtsi32_si128(shift),
                both,
            }
        }
    }
}

/// Equations 8-286 to 8-288 of the low eight samples of `x` (list 0) and
/// `y` (list 1), clipped to 0 to 255, in the low eight bytes.
#[inline(always)]
fn weigh8(x: __m128i, y: __m128i, weights: &Weights) -> __m128i {
    // SAFETY: SSE2 is part of the x86-64 baseline; register operations.
    unsafe {
        let zero = _mm_setzero_si128();
        let pairs = _mm_unpacklo_epi8(x, y);
        let first = _mm_sra_epi32(
            _mm_add_epi32(
                _mm_madd_epi16(_mm_unpacklo_epi8(pairs, zero), weights.pair),
                weights.bias,
            ),
            weights.shift,
        );
        let second = _mm_sra_epi32(
            _mm_add_epi32(
                _mm_madd_epi16(_mm_unpackhi_epi8(pairs, zero), weights.pair),
                weights.bias,
            ),
            weights.shift,
        );
        // Saturating to 16 bits and then to 0 to 255 is the clip.
        let words = _mm_packs_epi32(first, second);
        _mm_packus_epi16(words, words)
    }
}

/// Bytes widened to 16-bit lanes, from the low half of `v`.
#[inline(always)]
fn bytes_to_lanes(v: __m128i) -> __m128i {
    // SAFETY: SSE2 is part of the x86-64 baseline; register operations.
    unsafe { _mm_unpacklo_epi8(v, _mm_setzero_si128()) }
}

impl Rows<'_> {
    /// `H` of the nine-sample source row at offset `at`.
    #[inline(always)]
    fn horizontal8(&self, at: usize) -> __m128i {
        let row = bytes::<9>(self.from, at);
        let [.., ninth] = row;
        let left = u64::from_le_bytes(row.first_chunk::<8>().copied().unwrap_or_default());
        // Samples 1 to 8, shifted into place rather than read again.
        let right = (left >> 8) | (u64::from(ninth) << 56);
        chroma_horizontal(
            bytes_to_lanes(vector8(left.to_le_bytes())),
            bytes_to_lanes(vector8(right.to_le_bytes())),
            self.x,
        )
    }

    /// Eight samples wide: one row a vector.
    #[inline(always)]
    fn eight(&self, to: &mut [u8]) {
        let mut up = self.horizontal8(0);
        let (mut from, mut at) = (self.from_stride, 0usize);
        for _ in 0..self.h {
            let down = self.horizontal8(from);
            let mut pred = chroma_vertical(up, down, self.y);
            if let Some(out) = row_mut::<8>(to, at) {
                pred = self.finish.apply(pred, || vector8(*out));
                *out = low8(pred);
            }
            up = down;
            // Wrapping: past the last row the offsets are never used, and a
            // view of one row may have any stride.
            from = from.wrapping_add(self.from_stride);
            at = at.wrapping_add(self.to_stride);
        }
    }

    /// `H` of the `R`-sample source rows (`R` = `W` + 1) at offsets `at`
    /// and `at + stride`, one in each half of the vector. A row past the
    /// window reads as zeros.
    #[inline(always)]
    fn horizontal_pair<const W: usize, const R: usize>(&self, at: usize) -> __m128i {
        let first = bytes::<R>(self.from, at);
        let second = bytes::<R>(self.from, at + self.from_stride);
        let two = |a: &[u8], b: &[u8]| {
            bytes_to_lanes(vector8((word::<W>(a) | (word::<W>(b) << 32)).to_le_bytes()))
        };
        chroma_horizontal(
            two(&first, &second),
            two(
                first.get(1..).unwrap_or_default(),
                second.get(1..).unwrap_or_default(),
            ),
            self.x,
        )
    }

    /// `W` samples wide (`W` 2 or 4, `R` = `W` + 1): two rows a vector,
    /// each in four 16-bit lanes, of which a 2-wide block uses two. Source
    /// row `h + 1`, which an odd `h` pairs with the last, is past the
    /// window and reads as zeros; it only reaches an output row that does
    /// not exist.
    #[inline(always)]
    fn pairs<const W: usize, const R: usize>(&self, to: &mut [u8]) {
        let mut up = self.horizontal_pair::<W, R>(0);
        let (mut from, mut at) = (self.from_stride.wrapping_mul(2), 0usize);
        let mut row = 0;
        while row < self.h {
            let next = self.horizontal_pair::<W, R>(from);
            // `H` of rows 2k + 1 and 2k + 2: the high half of this pair
            // and the low half of the next.
            // SAFETY: SSE2 is part of the x86-64 baseline; register
            // operations.
            let down = unsafe {
                _mm_castpd_si128(_mm_shuffle_pd::<0b01>(
                    _mm_castsi128_pd(up),
                    _mm_castsi128_pd(next),
                ))
            };
            let pred = chroma_vertical(up, down, self.y);
            let below = at.wrapping_add(self.to_stride);
            let last = row + 1 == self.h;
            let pred = self.finish.apply(pred, || {
                let first = word::<W>(&bytes::<W>(to, at));
                let second = if last {
                    0
                } else {
                    word::<W>(&bytes::<W>(to, below))
                };
                vector8((first | (second << 32)).to_le_bytes())
            });
            let pred = low8(pred);
            if let (Some(out), Some(one)) = (row_mut::<W>(to, at), pred.first_chunk::<W>()) {
                *out = *one;
            }
            if !last {
                let other = pred.get(4..).and_then(|rest| rest.first_chunk::<W>());
                if let (Some(out), Some(other)) = (row_mut::<W>(to, below), other) {
                    *out = *other;
                }
            }
            up = next;
            row += 2;
            // Wrapping, as in `eight`: a view of one or two rows may have
            // any stride, and these offsets are past its last row.
            from = from.wrapping_add(self.from_stride.wrapping_mul(2));
            at = at.wrapping_add(self.to_stride.wrapping_mul(2));
        }
    }
}

/// `|a - b| < limit`, as a lane mask.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn near(a: __m128i, b: __m128i, limit: __m128i) -> __m128i {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe { _mm_cmpgt_epi16(limit, _mm_abs_epi16(_mm_sub_epi16(a, b))) }
}

/// `|a - b| < limit`, as a lane mask, over sixteen lanes.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn near16(a: __m256i, b: __m256i, limit: __m256i) -> __m256i {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe { _mm256_cmpgt_epi16(limit, _mm256_abs_epi16(_mm256_sub_epi16(a, b))) }
}

/// `new` in the lanes `mask` sets, `old` in the others.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn keep16(old: __m256i, new: __m256i, mask: __m256i) -> __m256i {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe { _mm256_blendv_epi8(old, new, mask) }
}

/// The sum of `v`, lane by lane.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn sum16(v: &[__m256i]) -> __m256i {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        v.iter()
            .fold(_mm256_setzero_si256(), |a, b| _mm256_add_epi16(a, *b))
    }
}

/// The luma filter of sixteen lines (clauses 8.7.2.3 and 8.7.2.4).
///
/// `s` holds p3, p2, p1, p0, q0, q1, q2 and q3, one lane per line, and is
/// written back with the filtered samples. Returns whether any line was
/// filtered: where none was, `s` is unchanged and nothing needs storing.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn filter_luma16(
    s: &mut [__m256i; 8],
    alpha: __m256i,
    beta: __m256i,
    bs: __m256i,
    tc0: __m256i,
) -> bool {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let [p3, p2, p1, p0, q0, q1, q2, q3] = *s;
        let zero = _mm256_setzero_si256();
        let one = _mm256_set1_epi16(1);
        let two = _mm256_set1_epi16(2);
        let four = _mm256_set1_epi16(4);
        // filterSamplesFlag (Equation 8-468), and bS = 0 filters nothing.
        let filtered = _mm256_and_si256(
            _mm256_and_si256(near16(p0, q0, alpha), near16(p1, p0, beta)),
            _mm256_and_si256(near16(q1, q0, beta), _mm256_cmpgt_epi16(bs, zero)),
        );
        if _mm256_testz_si256(filtered, filtered) == 1 {
            return false;
        }
        let ap = near16(p2, p0, beta);
        let aq = near16(q2, q0, beta);
        let strong_bs = _mm256_cmpgt_epi16(bs, _mm256_set1_epi16(3));
        let any_strong = _mm256_testz_si256(strong_bs, filtered) == 0;
        let any_weak = _mm256_testc_si256(strong_bs, filtered) == 0;
        let clip = |v: __m256i| _mm256_max_epi16(_mm256_min_epi16(v, _mm256_set1_epi16(255)), zero);

        // bS below 4 (clause 8.7.2.3). Lanes of bS 4 compute it too and
        // throw it away; an edge with none skips it.
        let (mut weak_p1, mut weak_p0, mut weak_q0, mut weak_q1) = (p1, p0, q0, q1);
        if any_weak {
            let tc = _mm256_add_epi16(
                _mm256_add_epi16(tc0, _mm256_and_si256(ap, one)),
                _mm256_and_si256(aq, one),
            );
            let raw = _mm256_srai_epi16(
                _mm256_add_epi16(
                    _mm256_add_epi16(
                        _mm256_slli_epi16(_mm256_sub_epi16(q0, p0), 2),
                        _mm256_sub_epi16(p1, q1),
                    ),
                    four,
                ),
                3,
            );
            let delta = _mm256_max_epi16(_mm256_min_epi16(raw, tc), _mm256_sub_epi16(zero, tc));
            weak_p0 = clip(_mm256_add_epi16(p0, delta));
            weak_q0 = clip(_mm256_sub_epi16(q0, delta));
            let mean = _mm256_srai_epi16(_mm256_add_epi16(_mm256_add_epi16(p0, q0), one), 1);
            let clamp0 = |v: __m256i| {
                _mm256_max_epi16(_mm256_min_epi16(v, tc0), _mm256_sub_epi16(zero, tc0))
            };
            weak_p1 = keep16(
                p1,
                _mm256_add_epi16(
                    p1,
                    clamp0(_mm256_srai_epi16(
                        _mm256_sub_epi16(_mm256_add_epi16(p2, mean), _mm256_slli_epi16(p1, 1)),
                        1,
                    )),
                ),
                ap,
            );
            weak_q1 = keep16(
                q1,
                _mm256_add_epi16(
                    q1,
                    clamp0(_mm256_srai_epi16(
                        _mm256_sub_epi16(_mm256_add_epi16(q2, mean), _mm256_slli_epi16(q1, 1)),
                        1,
                    )),
                ),
                aq,
            );
        }
        if !any_strong {
            // Every filtered line takes the filter of clause 8.7.2.3.
            s[2] = keep16(p1, weak_p1, filtered);
            s[3] = keep16(p0, weak_p0, filtered);
            s[4] = keep16(q0, weak_q0, filtered);
            s[5] = keep16(q1, weak_q1, filtered);
            return true;
        }

        // bS of 4 (clause 8.7.2.4). Every sum stays within 0 to 255 once
        // shifted, so no clip is needed.
        let strong = _mm256_cmpgt_epi16(
            _mm256_add_epi16(_mm256_srai_epi16(alpha, 2), two),
            _mm256_abs_epi16(_mm256_sub_epi16(p0, q0)),
        );
        let p0_long = _mm256_srai_epi16(
            sum16(&[
                p2,
                _mm256_slli_epi16(p1, 1),
                _mm256_slli_epi16(p0, 1),
                _mm256_slli_epi16(q0, 1),
                q1,
                four,
            ]),
            3,
        );
        let p1_long = _mm256_srai_epi16(sum16(&[p2, p1, p0, q0, two]), 2);
        let p2_long = _mm256_srai_epi16(
            sum16(&[
                _mm256_slli_epi16(p3, 1),
                _mm256_mullo_epi16(p2, _mm256_set1_epi16(3)),
                p1,
                p0,
                q0,
                four,
            ]),
            3,
        );
        let p0_short = _mm256_srai_epi16(sum16(&[_mm256_slli_epi16(p1, 1), p0, q1, two]), 2);
        let q0_long = _mm256_srai_epi16(
            sum16(&[
                p1,
                _mm256_slli_epi16(p0, 1),
                _mm256_slli_epi16(q0, 1),
                _mm256_slli_epi16(q1, 1),
                q2,
                four,
            ]),
            3,
        );
        let q1_long = _mm256_srai_epi16(sum16(&[p0, q0, q1, q2, two]), 2);
        let q2_long = _mm256_srai_epi16(
            sum16(&[
                _mm256_slli_epi16(q3, 1),
                _mm256_mullo_epi16(q2, _mm256_set1_epi16(3)),
                q1,
                q0,
                p0,
                four,
            ]),
            3,
        );
        let q0_short = _mm256_srai_epi16(sum16(&[_mm256_slli_epi16(q1, 1), q0, p1, two]), 2);

        // Which of the three outcomes each lane takes.
        let strong_p = _mm256_and_si256(strong_bs, _mm256_and_si256(ap, strong));
        let strong_q = _mm256_and_si256(strong_bs, _mm256_and_si256(aq, strong));
        let new_p0 = keep16(keep16(weak_p0, p0_short, strong_bs), p0_long, strong_p);
        let new_q0 = keep16(keep16(weak_q0, q0_short, strong_bs), q0_long, strong_q);
        let new_p1 = keep16(keep16(weak_p1, p1, strong_bs), p1_long, strong_p);
        let new_q1 = keep16(keep16(weak_q1, q1, strong_bs), q1_long, strong_q);
        let new_p2 = keep16(p2, p2_long, strong_p);
        let new_q2 = keep16(q2, q2_long, strong_q);

        // Only the lines the filter applies to change.
        s[1] = keep16(p2, new_p2, filtered);
        s[2] = keep16(p1, new_p1, filtered);
        s[3] = keep16(p0, new_p0, filtered);
        s[4] = keep16(q0, new_q0, filtered);
        s[5] = keep16(q1, new_q1, filtered);
        s[6] = keep16(q2, new_q2, filtered);
        true
    }
}

/// The chroma filter of eight lines (clauses 8.7.2.3 and 8.7.2.4 with
/// chromaStyleFilteringFlag).
///
/// `s` holds p1, p0, q0 and q1, one lane per line. Returns whether any
/// line was filtered: where none was, `s` is unchanged.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn filter_chroma8(
    s: &mut [__m128i; 4],
    alpha: __m128i,
    beta: __m128i,
    bs: __m128i,
    tc0: __m128i,
) -> bool {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let [p1, p0, q0, q1] = *s;
        let zero = _mm_setzero_si128();
        let filtered = _mm_and_si128(
            _mm_and_si128(near(p0, q0, alpha), near(p1, p0, beta)),
            _mm_and_si128(near(q1, q0, beta), _mm_cmpgt_epi16(bs, zero)),
        );
        if _mm_testz_si128(filtered, filtered) == 1 {
            return false;
        }
        let strong_bs = _mm_cmpgt_epi16(bs, _mm_set1_epi16(3));
        let tc = _mm_add_epi16(tc0, _mm_set1_epi16(1));
        let raw = _mm_srai_epi16(
            _mm_add_epi16(
                _mm_add_epi16(
                    _mm_slli_epi16(_mm_sub_epi16(q0, p0), 2),
                    _mm_sub_epi16(p1, q1),
                ),
                _mm_set1_epi16(4),
            ),
            3,
        );
        let delta = _mm_max_epi16(_mm_min_epi16(raw, tc), _mm_sub_epi16(zero, tc));
        let two = _mm_set1_epi16(2);
        let weak_p0 = _mm_add_epi16(p0, delta);
        let weak_q0 = _mm_sub_epi16(q0, delta);
        let long_p0 = _mm_srai_epi16(
            _mm_add_epi16(
                _mm_add_epi16(_mm_slli_epi16(p1, 1), p0),
                _mm_add_epi16(q1, two),
            ),
            2,
        );
        let long_q0 = _mm_srai_epi16(
            _mm_add_epi16(
                _mm_add_epi16(_mm_slli_epi16(q1, 1), q0),
                _mm_add_epi16(p1, two),
            ),
            2,
        );
        let clip = |v: __m128i| _mm_max_epi16(_mm_min_epi16(v, _mm_set1_epi16(255)), zero);
        let new_p0 = _mm_blendv_epi8(clip(weak_p0), long_p0, strong_bs);
        let new_q0 = _mm_blendv_epi8(clip(weak_q0), long_q0, strong_bs);
        s[1] = _mm_blendv_epi8(p0, new_p0, filtered);
        s[2] = _mm_blendv_epi8(q0, new_q0, filtered);
        true
    }
}

/// Transposes sixteen vectors whose low eight bytes hold one line each
/// into eight vectors, one per sample position, each holding that sample
/// of all sixteen lines.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn transpose16x8_u8(lines: [__m128i; 16]) -> [__m128i; 8] {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let [
            l0,
            l1,
            l2,
            l3,
            l4,
            l5,
            l6,
            l7,
            l8,
            l9,
            l10,
            l11,
            l12,
            l13,
            l14,
            l15,
        ] = lines;
        // Two lines a sample, then four, then eight, then sixteen.
        let a0 = _mm_unpacklo_epi8(l0, l1);
        let a1 = _mm_unpacklo_epi8(l2, l3);
        let a2 = _mm_unpacklo_epi8(l4, l5);
        let a3 = _mm_unpacklo_epi8(l6, l7);
        let a4 = _mm_unpacklo_epi8(l8, l9);
        let a5 = _mm_unpacklo_epi8(l10, l11);
        let a6 = _mm_unpacklo_epi8(l12, l13);
        let a7 = _mm_unpacklo_epi8(l14, l15);
        let b0 = _mm_unpacklo_epi16(a0, a1);
        let b1 = _mm_unpackhi_epi16(a0, a1);
        let b2 = _mm_unpacklo_epi16(a2, a3);
        let b3 = _mm_unpackhi_epi16(a2, a3);
        let b4 = _mm_unpacklo_epi16(a4, a5);
        let b5 = _mm_unpackhi_epi16(a4, a5);
        let b6 = _mm_unpacklo_epi16(a6, a7);
        let b7 = _mm_unpackhi_epi16(a6, a7);
        let c0 = _mm_unpacklo_epi32(b0, b2);
        let c1 = _mm_unpackhi_epi32(b0, b2);
        let c2 = _mm_unpacklo_epi32(b1, b3);
        let c3 = _mm_unpackhi_epi32(b1, b3);
        let c4 = _mm_unpacklo_epi32(b4, b6);
        let c5 = _mm_unpackhi_epi32(b4, b6);
        let c6 = _mm_unpacklo_epi32(b5, b7);
        let c7 = _mm_unpackhi_epi32(b5, b7);
        [
            _mm_unpacklo_epi64(c0, c4),
            _mm_unpackhi_epi64(c0, c4),
            _mm_unpacklo_epi64(c1, c5),
            _mm_unpackhi_epi64(c1, c5),
            _mm_unpacklo_epi64(c2, c6),
            _mm_unpackhi_epi64(c2, c6),
            _mm_unpacklo_epi64(c3, c7),
            _mm_unpackhi_epi64(c3, c7),
        ]
    }
}

/// The inverse of [`transpose16x8_u8`]: eight vectors of one sample
/// position each become eight vectors of two lines each, line `2i` in the
/// low eight bytes of vector `i` and line `2i + 1` in the high eight.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn transpose8x16_u8(samples: [__m128i; 8]) -> [__m128i; 8] {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let [s0, s1, s2, s3, s4, s5, s6, s7] = samples;
        // Two samples a line, then four, then eight.
        let a0 = _mm_unpacklo_epi8(s0, s1);
        let a1 = _mm_unpackhi_epi8(s0, s1);
        let a2 = _mm_unpacklo_epi8(s2, s3);
        let a3 = _mm_unpackhi_epi8(s2, s3);
        let a4 = _mm_unpacklo_epi8(s4, s5);
        let a5 = _mm_unpackhi_epi8(s4, s5);
        let a6 = _mm_unpacklo_epi8(s6, s7);
        let a7 = _mm_unpackhi_epi8(s6, s7);
        let b0 = _mm_unpacklo_epi16(a0, a2);
        let b1 = _mm_unpackhi_epi16(a0, a2);
        let b2 = _mm_unpacklo_epi16(a1, a3);
        let b3 = _mm_unpackhi_epi16(a1, a3);
        let b4 = _mm_unpacklo_epi16(a4, a6);
        let b5 = _mm_unpackhi_epi16(a4, a6);
        let b6 = _mm_unpacklo_epi16(a5, a7);
        let b7 = _mm_unpackhi_epi16(a5, a7);
        [
            _mm_unpacklo_epi32(b0, b4),
            _mm_unpackhi_epi32(b0, b4),
            _mm_unpacklo_epi32(b1, b5),
            _mm_unpackhi_epi32(b1, b5),
            _mm_unpacklo_epi32(b2, b6),
            _mm_unpackhi_epi32(b2, b6),
            _mm_unpacklo_epi32(b3, b7),
            _mm_unpackhi_epi32(b3, b7),
        ]
    }
}

/// The four bytes of `params.bs` and each segment's t'C0, in the low four
/// bytes of each vector: one byte shuffle picks each segment's entry of
/// the row by its bS, modulo 8 as [`EdgeParams::tc0_at`] does.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn segment_bytes(params: &EdgeParams) -> (__m128i, __m128i) {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let bs = _mm_cvtsi32_si128(i32::from_le_bytes(params.bs));
        let row = _mm_cvtsi64_si128(i64::from_le_bytes(params.tc0));
        (
            bs,
            _mm_shuffle_epi8(row, _mm_and_si128(bs, _mm_set1_epi8(7))),
        )
    }
}

/// The boundary strength and threshold of each of eight chroma lines, as
/// lanes.
///
/// A chroma edge of eight lines has two lines per segment, so one pass
/// covers all four segments; any other shape is spelled out lane by lane.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn per_line8(params: &EdgeParams, segment: usize, first: usize) -> (__m128i, __m128i) {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        if segment == 2 && first == 0 {
            return spread8(params);
        }
        let mut strengths = [0i16; 8];
        let mut thresholds = [0i16; 8];
        lane_values(params, segment, first, &mut strengths, &mut thresholds);
        (load_i16x8(&strengths), load_i16x8(&thresholds))
    }
}

/// The boundary strength and threshold of each of sixteen luma lines, as
/// lanes.
///
/// A luma edge of sixteen lines has four lines per segment, so one pass
/// covers all four segments; any other shape is spelled out lane by lane.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn per_line16(params: &EdgeParams, segment: usize, first: usize) -> (__m256i, __m256i) {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        if segment == 4 && first == 0 {
            return spread16(params);
        }
        let mut strengths = [0i16; 16];
        let mut thresholds = [0i16; 16];
        lane_values(params, segment, first, &mut strengths, &mut thresholds);
        let (bs_low, bs_high) = strengths.split_at(8);
        let (tc_low, tc_high) = thresholds.split_at(8);
        (
            _mm256_set_m128i(load_i16x8(bs_high), load_i16x8(bs_low)),
            _mm256_set_m128i(load_i16x8(tc_high), load_i16x8(tc_low)),
        )
    }
}

/// The boundary strength and threshold of each of the eight lines of a
/// chroma edge of two lines a segment, as lanes: lane k takes segment
/// k / 2, widened to 16 bits.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn spread8(params: &EdgeParams) -> (__m128i, __m128i) {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let (bs, tc) = segment_bytes(params);
        let spread = _mm_setr_epi8(0, -1, 0, -1, 1, -1, 1, -1, 2, -1, 2, -1, 3, -1, 3, -1);
        (_mm_shuffle_epi8(bs, spread), _mm_shuffle_epi8(tc, spread))
    }
}

/// The boundary strength and threshold of each of the sixteen lines of a
/// luma edge of four lines a segment, as lanes: lane k takes segment
/// k / 4, widened to 16 bits.
#[inline]
#[target_feature(enable = "avx2")]
pub(super) unsafe fn spread16(params: &EdgeParams) -> (__m256i, __m256i) {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let (bs, tc) = segment_bytes(params);
        // The shuffle works within each half of the vector, so both halves
        // get the four bytes first.
        let spread = _mm256_setr_epi8(
            0, -1, 0, -1, 0, -1, 0, -1, 1, -1, 1, -1, 1, -1, 1, -1, 2, -1, 2, -1, 2, -1, 2, -1, 3,
            -1, 3, -1, 3, -1, 3, -1,
        );
        (
            _mm256_shuffle_epi8(_mm256_broadcastsi128_si256(bs), spread),
            _mm256_shuffle_epi8(_mm256_broadcastsi128_si256(tc), spread),
        )
    }
}

/// The boundary strength and threshold of lines `first` onwards, one per
/// entry, for the shapes [`per_line8`] and [`per_line16`] do not shuffle.
/// A line past the edge falls in segment 4 or later, whose strength is 0.
fn lane_values(
    params: &EdgeParams,
    segment: usize,
    first: usize,
    strengths: &mut [i16],
    thresholds: &mut [i16],
) {
    for (lane, (b, t)) in strengths.iter_mut().zip(thresholds.iter_mut()).enumerate() {
        let s = (first + lane) / segment.max(1);
        *b = i16::from(params.bs.get(s).copied().unwrap_or(0));
        *t = i16::from(params.tc0_at(s));
    }
}

// The bar of the sixteen-lane luma path, fixed before it was measured.
// The stream is `0404af1890da` of the tuning set (828x1792, High profile,
// no B frames, 300 frames), decoded by `examples/decode` of `coreav-h264`
// built with the `dist` profile, and counted with callgrind:
//
// 1. The path is kept for luma edges only if the decoder's whole
//    instruction count is below what it is with the reference filtering
//    every luma edge, everything else unchanged. Callgrind is exact, so any
//    decrease counts.
// 2. The wall time of the same decode, seven runs of each alternated, is
//    reported beside it; a median ratio against the reference above 1.02
//    rejects the path even where the count is lower.
// 3. Where the path is rejected, the dispatch sends luma edges to the
//    reference.
//
// Measured: the decoder ran 18.875 G instructions with this path, against
// 19.859 G with the reference on luma edges (-5.0%) and 19.785 G with the
// earlier eight-lane path (-4.6%); this function went from 1.646 G to
// 0.722 G. Wall time, median of the per-pair ratios: 0.989 against the
// reference on luma edges (five pairs of seven faster) and 0.964 against
// the eight-lane path. Both bars hold, so luma edges keep the vector path.

/// The deblocking filter of one edge (clauses 8.7.2.3 and 8.7.2.4).
///
/// A luma edge is filtered sixteen lines at a time, one 16-bit lane per
/// line, so the sixteen lines of a macroblock edge take one pass; a chroma
/// edge eight at a time. A horizontal edge has its lines along a row, so
/// each sample position is a run of bytes and no transpose is needed. A
/// vertical edge has each line in a row of its own: a luma edge's lines are
/// transposed, filtered and transposed back; a chroma edge's four-byte
/// lines are gathered by a shuffle, and only p0 and q0 are written back.
///
/// Each plane, direction and standard length (sixteen luma lines, eight
/// chroma lines) has a function of its own, and this one, which is inlined
/// into the caller, picks it. Where the caller builds the edge, its plane
/// and length are constants and the choice folds away; inside the chosen
/// function the shape is a constant too, so no branch in it depends on
/// the shape. One function for every edge mispredicted its first branch,
/// luma or chroma, on 41% of edges, since the decoder interleaves them.
///
/// # Safety
///
/// The CPU must support AVX2.
#[inline(always)]
pub(super) unsafe fn filter_edge(edge: EdgeMut<'_>, params: &EdgeParams) {
    // The lines run along a row exactly where `across` is not 1:
    // `EdgeMut::new` makes one of `across` and `along` 1 and the other at
    // least 4. Tested on `across` because a caller that builds a vertical
    // edge passes it as the literal 1, so the test folds there, and for a
    // horizontal edge it always goes the same way.
    let rows = edge.across() != 1;
    // SAFETY: the caller guarantees AVX2.
    unsafe {
        match (edge.reach(), rows, edge.lines()) {
            (Reach::Luma, true, 16) => filter_luma_edge::<true, true>(edge, params),
            (Reach::Luma, false, 16) => filter_luma_edge::<false, true>(edge, params),
            (Reach::Luma, true, _) => filter_luma_edge::<true, false>(edge, params),
            (Reach::Luma, false, _) => filter_luma_edge::<false, false>(edge, params),
            (Reach::Chroma, true, 8) => filter_chroma_edge::<true, true>(edge, params),
            (Reach::Chroma, false, 8) => filter_chroma_edge::<false, true>(edge, params),
            (Reach::Chroma, true, _) => filter_chroma_edge::<true, false>(edge, params),
            (Reach::Chroma, false, _) => filter_chroma_edge::<false, false>(edge, params),
        }
    }
}

/// [`filter_edge`] for a luma edge, sixteen lines a pass. `ROWS` is
/// whether the lines run along a row (`along == 1`, a horizontal edge);
/// `STANDARD` whether the edge has sixteen lines, in which case the shape
/// is a constant here and every branch on it folds away.
#[target_feature(enable = "avx2")]
unsafe fn filter_luma_edge<const ROWS: bool, const STANDARD: bool>(
    mut edge: EdgeMut<'_>,
    params: &EdgeParams,
) {
    // SAFETY: AVX2 is available. Memory is reached through the helpers, or,
    // for a standard edge, through the pointer `rows_at` or `rows_at_mut`
    // returned after checking that every row the loads and stores below
    // touch lies inside the slice: each is at most the checked width from
    // the start of a checked row, and none needs alignment.
    unsafe {
        let (lines, segment) = if STANDARD {
            (16, 4)
        } else {
            (edge.lines(), edge.segment())
        };
        let (across, along) = if ROWS {
            (edge.across(), 1)
        } else {
            (1, edge.along())
        };
        // EdgeMut::new keeps the two directions orthogonal, which the
        // addressing below relies on.
        debug_assert!(edge.across() == 1 || edge.along() == 1);
        debug_assert_eq!(ROWS, edge.along() == 1 && edge.across() != 1);
        let alpha = _mm256_set1_epi16(i16::from(params.alpha));
        let beta = _mm256_set1_epi16(i16::from(params.beta));
        let (data, q0) = edge.window();
        let mut first = 0;
        while first < lines {
            let count = (lines - first).min(16);
            let (bs, tc0) = if STANDARD {
                spread16(params)
            } else {
                per_line16(params, segment, first)
            };
            // A segment whose boundary strength is zero is not filtered
            // (clause 8.7.2.1), and most interior edges are such.
            if _mm256_testz_si256(bs, bs) == 1 {
                first += 16;
                continue;
            }
            // Gather p3..q3 of these lines into one vector per sample
            // position.
            let mut lanes = [_mm256_setzero_si256(); 8];
            let mut rows = [_mm_setzero_si128(); 16];
            if ROWS && STANDARD {
                // The lines run along a row: sample j of all of them is one
                // run of sixteen bytes.
                let runs = gather::<8, 16>(data, q0 - 4 * across, across);
                for (slot, run) in lanes.iter_mut().zip(runs) {
                    *slot = _mm256_cvtepu8_epi16(vector16(run));
                }
            } else if ROWS {
                for (j, slot) in lanes.iter_mut().enumerate() {
                    let start = q0 + j * across - 4 * across + first;
                    let row = data.get(start..start + count).unwrap_or_default();
                    *slot = _mm256_cvtepu8_epi16(load16(row));
                }
            } else {
                // One line per row: load each line and transpose.
                if let (true, Some(base)) =
                    (STANDARD, rows_at(data, q0.wrapping_sub(4), along, 16, 8))
                {
                    for (k, slot) in rows.iter_mut().enumerate() {
                        *slot = _mm_loadl_epi64(base.add(k * along).cast());
                    }
                } else if STANDARD {
                    let lines = gather::<16, 8>(data, q0 - 4, along);
                    for (slot, line) in rows.iter_mut().zip(lines) {
                        *slot = vector8(line);
                    }
                } else {
                    for (k, slot) in rows.iter_mut().enumerate().take(count) {
                        let start = q0 + (first + k) * along - 4;
                        let row = data.get(start..start + 8).unwrap_or_default();
                        *slot = load8(row);
                    }
                }
                for (slot, samples) in lanes.iter_mut().zip(transpose16x8_u8(rows)) {
                    *slot = _mm256_cvtepu8_epi16(samples);
                }
            }
            if !filter_luma16(&mut lanes, alpha, beta, bs, tc0) {
                first += 16;
                continue;
            }
            // Back to bytes. p3 and q3 never change; the filter writes p2
            // to q2.
            let to_bytes = |a: __m256i, b: __m256i| {
                // The pack works within each half of the vector; the
                // permute puts `a`'s sixteen bytes in the low half.
                let packed = _mm256_permute4x64_epi64(_mm256_packus_epi16(a, b), 0b11_01_10_00);
                (
                    _mm256_castsi256_si128(packed),
                    _mm256_extracti128_si256(packed, 1),
                )
            };
            let (b2, b1) = to_bytes(lanes[1], lanes[2]);
            let (b0, c0) = to_bytes(lanes[3], lanes[4]);
            let (c1, c2) = to_bytes(lanes[5], lanes[6]);
            if ROWS && STANDARD {
                let runs = [b2, b1, b0, c0, c1, c2].map(array16);
                scatter::<6, 16>(data, q0 - 3 * across, across, &runs);
            } else if ROWS {
                for (j, v) in [b2, b1, b0, c0, c1, c2].into_iter().enumerate() {
                    let start = q0 + (j + 1) * across - 4 * across + first;
                    let row = data.get_mut(start..start + count).unwrap_or_default();
                    store_bytes(row, v, count);
                }
            } else if STANDARD {
                let (b3, c3) = to_bytes(lanes[0], lanes[7]);
                let pairs = transpose8x16_u8([b3, b2, b1, b0, c0, c1, c2, c3]);
                // Line 2k in the low half of pair k, line 2k + 1 in the
                // high half.
                if let Some(base) = rows_at_mut(data, q0.wrapping_sub(4), along, 16, 8) {
                    for (k, pair) in pairs.into_iter().enumerate() {
                        _mm_storel_epi64(base.add(2 * k * along).cast(), pair);
                        // The high half as an unaligned 64-bit write: rows
                        // are byte addresses, which a typed store such as
                        // `_mm_storeh_pd` would require to be aligned.
                        base.add((2 * k + 1) * along)
                            .cast::<i64>()
                            .write_unaligned(_mm_extract_epi64::<1>(pair));
                    }
                    first += 16;
                    continue;
                }
                let mut lines = [[0u8; 8]; 16];
                for (two, pair) in lines.chunks_exact_mut(2).zip(pairs) {
                    if let [low, high] = two {
                        *low = low8(pair);
                        *high = low8(_mm_unpackhi_epi64(pair, pair));
                    }
                }
                scatter::<16, 8>(data, q0 - 4, along, &lines);
            } else {
                let (b3, c3) = to_bytes(lanes[0], lanes[7]);
                let pairs = transpose8x16_u8([b3, b2, b1, b0, c0, c1, c2, c3]);
                for (k, pair) in pairs.iter().enumerate() {
                    for (half, v) in [*pair, _mm_unpackhi_epi64(*pair, *pair)]
                        .into_iter()
                        .enumerate()
                    {
                        let line = 2 * k + half;
                        if line >= count {
                            break;
                        }
                        let start = q0 + (first + line) * along - 4;
                        let row = data.get_mut(start..start + 8).unwrap_or_default();
                        store_bytes(row, v, 8);
                    }
                }
            }
            first += 16;
        }
    }
}

/// The first four bytes of `src` as one 32-bit value, or 0 where `src` is
/// shorter: one load, with no pointer formed.
#[inline(always)]
fn le4(src: &[u8]) -> i32 {
    let bytes: [u8; 4] = src
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .unwrap_or_default();
    i32::from_le_bytes(bytes)
}

/// The four samples p1, p0, q0 and q1 of eight lines of a chroma edge
/// down a column, each line a 32-bit value, as four vectors of eight 16-bit
/// lanes, one vector per sample position.
///
/// Four lines fill one vector; a byte shuffle groups each sample position
/// of those four lines into one 32-bit lane, and joining the two vectors'
/// lanes gives each sample position for all eight lines.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn samples_by_position(lines: [i32; 8]) -> [__m128i; 4] {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let [l0, l1, l2, l3, l4, l5, l6, l7] = lines;
        let group = _mm_setr_epi8(0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15);
        let upper = _mm_shuffle_epi8(_mm_setr_epi32(l0, l1, l2, l3), group);
        let lower = _mm_shuffle_epi8(_mm_setr_epi32(l4, l5, l6, l7), group);
        // p1 of all eight lines, then p0; q0, then q1.
        let p = _mm_unpacklo_epi32(upper, lower);
        let q = _mm_unpackhi_epi32(upper, lower);
        [
            _mm_cvtepu8_epi16(p),
            _mm_cvtepu8_epi16(_mm_srli_si128(p, 8)),
            _mm_cvtepu8_epi16(q),
            _mm_cvtepu8_epi16(_mm_srli_si128(q, 8)),
        ]
    }
}

/// The chroma filter of sixteen lines: [`filter_chroma8`] with the lines
/// of two edges side by side, one in each 128-bit half, each half with its
/// own thresholds.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn filter_chroma16(
    s: &mut [__m256i; 4],
    alpha: __m256i,
    beta: __m256i,
    bs: __m256i,
    tc0: __m256i,
) -> bool {
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let [p1, p0, q0, q1] = *s;
        let zero = _mm256_setzero_si256();
        let filtered = _mm256_and_si256(
            _mm256_and_si256(near16(p0, q0, alpha), near16(p1, p0, beta)),
            _mm256_and_si256(near16(q1, q0, beta), _mm256_cmpgt_epi16(bs, zero)),
        );
        if _mm256_testz_si256(filtered, filtered) == 1 {
            return false;
        }
        let strong_bs = _mm256_cmpgt_epi16(bs, _mm256_set1_epi16(3));
        let tc = _mm256_add_epi16(tc0, _mm256_set1_epi16(1));
        let raw = _mm256_srai_epi16::<3>(_mm256_add_epi16(
            _mm256_add_epi16(
                _mm256_slli_epi16::<2>(_mm256_sub_epi16(q0, p0)),
                _mm256_sub_epi16(p1, q1),
            ),
            _mm256_set1_epi16(4),
        ));
        let delta = _mm256_max_epi16(_mm256_min_epi16(raw, tc), _mm256_sub_epi16(zero, tc));
        let two = _mm256_set1_epi16(2);
        let max = _mm256_set1_epi16(255);
        let weak_p0 = _mm256_max_epi16(_mm256_min_epi16(_mm256_add_epi16(p0, delta), max), zero);
        let weak_q0 = _mm256_max_epi16(_mm256_min_epi16(_mm256_sub_epi16(q0, delta), max), zero);
        let long_p0 = _mm256_srai_epi16::<2>(_mm256_add_epi16(
            _mm256_add_epi16(_mm256_slli_epi16::<1>(p1), p0),
            _mm256_add_epi16(q1, two),
        ));
        let long_q0 = _mm256_srai_epi16::<2>(_mm256_add_epi16(
            _mm256_add_epi16(_mm256_slli_epi16::<1>(q1), q0),
            _mm256_add_epi16(p1, two),
        ));
        let new_p0 = keep16(weak_p0, long_p0, strong_bs);
        let new_q0 = keep16(weak_q0, long_q0, strong_bs);
        s[1] = keep16(p0, new_p0, filtered);
        s[2] = keep16(q0, new_q0, filtered);
        true
    }
}

/// [`crate::Kernels::filter_chroma_pair`]: the same standard edge (eight
/// lines) of both chroma planes in one pass, Cb's lines in the low half of
/// each vector and Cr's in the high half. Edges of any other shape, or
/// whose two planes differ in shape, go to [`filter_edge`] one at a time.
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn filter_chroma_pair(
    cb: EdgeMut<'_>,
    cr: EdgeMut<'_>,
    params: &[EdgeParams; 2],
) {
    let [for_cb, for_cr] = params;
    let pair = cb.reach() == Reach::Chroma
        && cr.reach() == Reach::Chroma
        && cb.lines() == 8
        && cr.lines() == 8
        && cb.across() == cr.across()
        && cb.along() == cr.along();
    // SAFETY: AVX2 is available (this function's contract).
    unsafe {
        if !pair {
            filter_edge(cb, for_cb);
            filter_edge(cr, for_cr);
        } else if cb.across() != 1 {
            chroma_pair::<true>(cb, cr, params);
        } else {
            chroma_pair::<false>(cb, cr, params);
        }
    }
}

/// [`filter_chroma_pair`] for two edges of eight lines each that run along
/// a row (`ROWS`, a horizontal edge) or down a column.
///
/// Each plane's edge is checked once, as a standard edge of
/// `filter_chroma_edge` is: the four rows of eight samples p1 to q1 of a
/// horizontal edge, or the eight rows of four samples p1 to q1 of a
/// vertical one (`rows_at_mut`). Every read and write is then one of those
/// rows, or part of one, through the pointer; where a check fails, both
/// edges go to [`filter_edge`] before anything is read.
///
/// # Safety
///
/// AVX2 must be available, and the two edges must be chroma edges of eight
/// lines with the same `across` and `along`, `across != 1` exactly where
/// `ROWS`.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn chroma_pair<const ROWS: bool>(
    mut cb: EdgeMut<'_>,
    mut cr: EdgeMut<'_>,
    params: &[EdgeParams; 2],
) {
    let [for_cb, for_cr] = params;
    // SAFETY: AVX2 is available (this function's contract). Every access
    // through `at_cb` and `at_cr` is inside the extent `rows_at_mut`
    // checked for its plane: rows 0 to 3, eight bytes each, `across`
    // apart, of a horizontal edge; rows 0 to 7, four bytes each, `along`
    // apart, of a vertical one; unaligned, none a typed access that needs
    // alignment.
    unsafe {
        let (bs_cb, tc_cb) = spread8(for_cb);
        let (bs_cr, tc_cr) = spread8(for_cr);
        let bs = _mm256_set_m128i(bs_cr, bs_cb);
        if _mm256_testz_si256(bs, bs) == 1 {
            return;
        }
        let tc0 = _mm256_set_m128i(tc_cr, tc_cb);
        let alpha = _mm256_set_m128i(
            _mm_set1_epi16(i16::from(for_cr.alpha)),
            _mm_set1_epi16(i16::from(for_cb.alpha)),
        );
        let beta = _mm256_set_m128i(
            _mm_set1_epi16(i16::from(for_cr.beta)),
            _mm_set1_epi16(i16::from(for_cb.beta)),
        );
        let (across, along) = (cb.across(), cb.along());
        let first_row = |edge: &mut EdgeMut<'_>| -> Option<*mut u8> {
            let (data, q0) = edge.window();
            if ROWS {
                rows_at_mut(data, q0.wrapping_sub(2 * across), across, 4, 8)
            } else {
                rows_at_mut(data, q0.wrapping_sub(2), along, 8, 4)
            }
        };
        let (Some(at_cb), Some(at_cr)) = (first_row(&mut cb), first_row(&mut cr)) else {
            filter_edge(cb, for_cb);
            filter_edge(cr, for_cr);
            return;
        };
        let mut lanes = [_mm256_setzero_si256(); 4];
        if ROWS {
            for (j, slot) in lanes.iter_mut().enumerate() {
                let both = _mm_unpacklo_epi64(
                    _mm_loadl_epi64(at_cb.add(j * across).cast()),
                    _mm_loadl_epi64(at_cr.add(j * across).cast()),
                );
                *slot = _mm256_cvtepu8_epi16(both);
            }
        } else {
            let mut rows_cb = [0i32; 8];
            let mut rows_cr = [0i32; 8];
            for (k, (b, r)) in rows_cb.iter_mut().zip(rows_cr.iter_mut()).enumerate() {
                *b = at_cb.add(k * along).cast::<i32>().read_unaligned();
                *r = at_cr.add(k * along).cast::<i32>().read_unaligned();
            }
            let low = samples_by_position(rows_cb);
            let high = samples_by_position(rows_cr);
            for (slot, (l, h)) in lanes.iter_mut().zip(low.into_iter().zip(high)) {
                *slot = _mm256_set_m128i(h, l);
            }
        }
        if !filter_chroma16(&mut lanes, alpha, beta, bs, tc0) {
            return;
        }
        let [_, p0, q0, _] = lanes;
        if ROWS {
            // Row 1 holds p0 and row 2 q0; each half of the pack holds its
            // plane's eight samples in its low eight bytes.
            for (j, v) in [(1, p0), (2, q0)] {
                let packed = _mm256_packus_epi16(v, v);
                _mm_storel_epi64(at_cb.add(j * across).cast(), _mm256_castsi256_si128(packed));
                _mm_storel_epi64(
                    at_cr.add(j * across).cast(),
                    _mm256_extracti128_si256::<1>(packed),
                );
            }
        } else {
            // Line k's p0 and q0, side by side in 16-bit lane k, written at
            // byte 1 of its row of four.
            let planes = [
                (
                    at_cb,
                    _mm256_castsi256_si128(p0),
                    _mm256_castsi256_si128(q0),
                ),
                (
                    at_cr,
                    _mm256_extracti128_si256::<1>(p0),
                    _mm256_extracti128_si256::<1>(q0),
                ),
            ];
            for (at, p0, q0) in planes {
                let packed = _mm_packus_epi16(p0, q0);
                let pairs = _mm_unpacklo_epi8(packed, _mm_srli_si128::<8>(packed));
                let halves = [
                    _mm_cvtsi128_si64(pairs) as u64,
                    _mm_cvtsi128_si64(_mm_unpackhi_epi64(pairs, pairs)) as u64,
                ];
                for (h, half) in halves.into_iter().enumerate() {
                    for k in 0..4 {
                        at.add((4 * h + k) * along + 1)
                            .cast::<u16>()
                            .write_unaligned((half >> (16 * k)) as u16);
                    }
                }
            }
        }
    }
}

/// [`filter_edge`] for a chroma edge, eight lines a pass. Only p0 and q0
/// change (clause 8.7.2.3 with chromaStyleFilteringFlag), so only they are
/// written back, and only where a line was filtered. `ROWS` and
/// `STANDARD` (eight lines) are as in [`filter_luma_edge`].
#[target_feature(enable = "avx2")]
unsafe fn filter_chroma_edge<const ROWS: bool, const STANDARD: bool>(
    mut edge: EdgeMut<'_>,
    params: &EdgeParams,
) {
    // SAFETY: as in `filter_luma_edge`: the helpers, or for a standard
    // edge the checked rows of `rows_at` and `rows_at_mut`, each access at
    // most the checked width from a row's start and unaligned.
    unsafe {
        let (lines, segment) = if STANDARD {
            (8, 2)
        } else {
            (edge.lines(), edge.segment())
        };
        let (across, along) = if ROWS {
            (edge.across(), 1)
        } else {
            (1, edge.along())
        };
        debug_assert_eq!(ROWS, edge.along() == 1 && edge.across() != 1);
        let alpha = _mm_set1_epi16(i16::from(params.alpha));
        let beta = _mm_set1_epi16(i16::from(params.beta));
        let (data, q0) = edge.window();
        let mut first = 0;
        while first < lines {
            let count = (lines - first).min(8);
            let (bs, tc0) = if STANDARD {
                spread8(params)
            } else {
                per_line8(params, segment, first)
            };
            if _mm_testz_si128(bs, bs) == 1 {
                first += 8;
                continue;
            }
            // Gather p1..q1 of these lines into one vector per sample
            // position.
            let mut lanes = [_mm_setzero_si128(); 4];
            if let (true, true, Some(base)) = (
                ROWS,
                STANDARD,
                rows_at(data, q0.wrapping_sub(2 * across), across, 4, 8),
            ) {
                for (j, slot) in lanes.iter_mut().enumerate() {
                    *slot = _mm_cvtepu8_epi16(_mm_loadl_epi64(base.add(j * across).cast()));
                }
            } else if ROWS && STANDARD {
                let runs = gather::<4, 8>(data, q0 - 2 * across, across);
                for (slot, run) in lanes.iter_mut().zip(runs) {
                    *slot = _mm_cvtepu8_epi16(vector8(run));
                }
            } else if ROWS {
                for (j, slot) in lanes.iter_mut().enumerate() {
                    let start = q0 + j * across - 2 * across + first;
                    let row = data.get(start..start + count).unwrap_or_default();
                    *slot = _mm_cvtepu8_epi16(load8(row));
                }
            } else if let (true, Some(base)) =
                (STANDARD, rows_at(data, q0.wrapping_sub(2), along, 8, 4))
            {
                // One line per row, four samples each.
                lanes = samples_by_position(core::array::from_fn(|k| {
                    base.add(k * along).cast::<i32>().read_unaligned()
                }));
            } else if STANDARD {
                lanes = samples_by_position(
                    gather::<8, 4>(data, q0 - 2, along).map(i32::from_le_bytes),
                );
            } else {
                let mut rows = [0i32; 8];
                for (k, slot) in rows.iter_mut().enumerate().take(count) {
                    let start = q0 + (first + k) * along - 2;
                    *slot = le4(data.get(start..).unwrap_or_default());
                }
                lanes = samples_by_position(rows);
            }
            if !filter_chroma8(&mut lanes, alpha, beta, bs, tc0) {
                first += 8;
                continue;
            }
            let [_, p0, q0_, _] = lanes;
            if let (true, true, Some(base)) = (
                ROWS,
                STANDARD,
                rows_at_mut(data, q0.wrapping_sub(across), across, 2, 8),
            ) {
                for (j, v) in [p0, q0_].into_iter().enumerate() {
                    _mm_storel_epi64(base.add(j * across).cast(), _mm_packus_epi16(v, v));
                }
            } else if ROWS && STANDARD {
                let runs = [p0, q0_].map(|v| low8(_mm_packus_epi16(v, v)));
                scatter::<2, 8>(data, q0 - across, across, &runs);
            } else if ROWS {
                for (j, v) in [(1, p0), (2, q0_)] {
                    let start = q0 + j * across - 2 * across + first;
                    let row = data.get_mut(start..start + count).unwrap_or_default();
                    store_bytes(row, _mm_packus_epi16(v, v), count);
                }
            } else if let (true, Some(base)) =
                (STANDARD, rows_at_mut(data, q0.wrapping_sub(1), along, 8, 2))
            {
                // Line k's p0 and q0, side by side in 16-bit lane k: four
                // lines from each half, each written as one 16-bit store.
                let packed = _mm_packus_epi16(p0, q0_);
                let pairs = _mm_unpacklo_epi8(packed, _mm_srli_si128(packed, 8));
                let halves = [
                    _mm_cvtsi128_si64(pairs) as u64,
                    _mm_cvtsi128_si64(_mm_unpackhi_epi64(pairs, pairs)) as u64,
                ];
                for (h, half) in halves.into_iter().enumerate() {
                    for k in 0..4 {
                        base.add((4 * h + k) * along)
                            .cast::<u16>()
                            .write_unaligned((half >> (16 * k)) as u16);
                    }
                }
            } else if STANDARD {
                let packed = _mm_packus_epi16(p0, q0_);
                let side_by_side = u128::from_le_bytes(array16(_mm_unpacklo_epi8(
                    packed,
                    _mm_srli_si128(packed, 8),
                )));
                let pairs: [[u8; 2]; 8] =
                    core::array::from_fn(|k| ((side_by_side >> (16 * k)) as u16).to_le_bytes());
                scatter::<8, 2>(data, q0 - 1, along, &pairs);
            } else {
                let packed = _mm_packus_epi16(p0, q0_);
                let mut pairs = [0i16; 8];
                store_i16x8(
                    &mut pairs,
                    _mm_unpacklo_epi8(packed, _mm_srli_si128(packed, 8)),
                );
                for (k, pair) in pairs.iter().enumerate().take(count) {
                    let start = q0 + (first + k) * along - 1;
                    if let Some(out) = data.get_mut(start..start + 2) {
                        out.copy_from_slice(&pair.to_le_bytes());
                    }
                }
            }
            first += 8;
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    //! Bar C3 for the deblocking filter of this file, over every threshold
    //! Tables 8-16 and 8-17 give, every boundary strength, every number of
    //! lines from 4 to 40 (so passes that start past the first line, and
    //! last passes shorter than a vector) and both directions, plus
    //! thresholds and strengths outside the tables.

    use super::{
        filter_edge, gather, idct4x4_add_quad, idct8x8_add, luma_mc, luma_mc_avg, scatter,
    };
    use crate::scalar::Scalar;
    use crate::views::{Block, BlockMut, EdgeMut, Reach};
    use crate::{EdgeParams, Kernels};

    /// A reproducible pseudo-random source (SplitMix64).
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> i32 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((z ^ (z >> 31)) % n.max(1)) as i32
        }
    }

    /// A deblocking kernel of this file or of the AVX-512 tier.
    pub(in crate::simd) type EdgeKernel = unsafe fn(EdgeMut<'_>, &EdgeParams);

    /// Runs one edge through `kernel` and the reference and compares the
    /// planes.
    fn check(case: usize, params: &EdgeParams, rng: &mut Rng, kernel: EdgeKernel) {
        let chroma = case % 2 == 0;
        let vertical = (case / 2) % 2 == 0;
        let lines = 4 * (1 + (case / 4) % 10);
        let reach = if chroma { Reach::Chroma } else { Reach::Luma };
        let stride = 48;
        // Samples near a step across the edge, the step and the noise
        // around it scaled to the thresholds, so that every outcome of the
        // filter's comparisons turns up; every so often, any sample at all.
        let step = rng.below(u64::from(params.alpha) + 3);
        let noise = u64::from(params.beta) + 2;
        let wild = rng.below(8) == 0;
        let base = rng.below(256);
        let plane: Vec<u8> = (0..stride * (lines + 12) + 8)
            .map(|i| {
                if wild {
                    return rng.below(256) as u8;
                }
                let (row, col) = (i / stride, i % stride);
                let far = if vertical { col >= 8 } else { row >= 8 };
                let v = base + if far { step } else { 0 } + rng.below(noise) - (noise / 2) as i32;
                v.clamp(0, 255) as u8
            })
            .collect();
        let (q0, across, along) = if vertical {
            (4 * stride + 8, 1, stride)
        } else {
            (8 * stride + 4, stride, 1)
        };
        let mut want = plane.clone();
        let mut got = plane;
        Scalar.filter_edge(
            EdgeMut::new(&mut want, q0, across, along, lines, reach).expect("edge"),
            params,
        );
        // SAFETY: the callers return before this where the CPU lacks the
        // kernel's features.
        unsafe {
            kernel(
                EdgeMut::new(&mut got, q0, across, along, lines, reach).expect("edge"),
                params,
            );
        }
        assert_eq!(
            got, want,
            "case {case}: {params:?}, chroma {chroma}, vertical {vertical}, {lines} lines"
        );
    }

    #[test]
    fn every_threshold_and_strength_matches_the_reference() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        every_threshold_and_strength(filter_edge);
    }

    /// Every threshold of Tables 8-16 and 8-17 and every combination of
    /// boundary strengths, then thresholds and strengths outside the
    /// tables, through `kernel`.
    pub(in crate::simd) fn every_threshold_and_strength(kernel: EdgeKernel) {
        let mut rng = Rng(0x0e1);
        let mut case = 0;
        // Every alpha and beta of Table 8-16, each with tC0 of Table 8-17
        // and boundary strengths running through all 5^4 combinations; the
        // row's entries for bS 0 and 4 to 7, which no in-range strength
        // reads, hold values a kernel that read them would show.
        for alpha in 0..=255 {
            for beta in 0..=18 {
                for _ in 0..2 {
                    let mut params = EdgeParams {
                        alpha,
                        beta,
                        tc0: [200, 0, 0, 0, 255, 99, 1, 77],
                        bs: [0; 4],
                    };
                    let mut strengths = case;
                    for bs in &mut params.bs {
                        *bs = (strengths % 5) as u8;
                        strengths /= 5;
                    }
                    for (i, tc0) in params.tc0.iter_mut().enumerate().skip(1).take(3) {
                        *tc0 = ((case * 7 + i * 11) % 26) as u8;
                    }
                    check(case, &params, &mut rng, kernel);
                    case += 1;
                }
            }
        }
        // Thresholds outside the tables: beta and t'C0 above them, and
        // strengths above 4, which take their row entry modulo 8.
        for alpha in [0, 1, 128, 255] {
            for beta in [0, 19, 128, 255] {
                for tc0 in [0, 26, 128, 255] {
                    for bs in [0, 1, 2, 3, 4, 5, 7, 8, 12, 255] {
                        let params = EdgeParams {
                            alpha,
                            beta,
                            tc0: [tc0, 25, tc0, 0, 7, tc0, 255, 1],
                            bs: [bs, 4, bs / 2, 2],
                        };
                        check(case, &params, &mut rng, kernel);
                        case += 1;
                    }
                }
            }
        }
    }

    /// Bar C3 for luma interpolation over every block size, every
    /// quarter-sample position, and strides from the tightest the views
    /// allow to wider ones, with samples at the ends of the range.
    /// Whatever the kernel writes outside the block, past a row's end or
    /// below the last row, shows up as a difference in the plane.
    #[test]
    fn every_luma_position_and_size_matches_the_reference() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let mut rng = Rng(0x14c);
        let mut case = 0;
        for w in [4, 8, 16] {
            for h in 1..=16 {
                for (xf, yf) in (0..4u8).flat_map(|x| (0..4u8).map(move |y| (x, y))) {
                    for (src_pad, dst_pad) in [(0, 0), (0, 3), (11, 0), (27, 16)] {
                        let src_stride = w + 5 + src_pad;
                        let source: Vec<u8> = (0..src_stride * (h + 5))
                            .map(|_| match rng.below(6) {
                                0 => 0,
                                1 => 255,
                                _ => rng.below(256) as u8,
                            })
                            .collect();
                        let dst_stride = w + dst_pad;
                        // A margin of one row and one column round the
                        // block, which neither kernel may touch.
                        let plane: Vec<u8> = (0..dst_stride * (h + 2) + 1)
                            .map(|_| rng.below(256) as u8)
                            .collect();
                        let at = dst_stride + usize::from(dst_pad > 0);
                        let src = Block::new(&source, 0, src_stride, w + 5, h + 5).expect("src");
                        let mut want = plane.clone();
                        let mut got = plane;
                        Scalar.luma_mc(
                            BlockMut::new(&mut want, at, dst_stride, w, h).expect("dst"),
                            src,
                            xf,
                            yf,
                        );
                        // SAFETY: the test returns before this where the
                        // CPU lacks AVX2.
                        unsafe {
                            luma_mc(
                                BlockMut::new(&mut got, at, dst_stride, w, h).expect("dst"),
                                src,
                                xf,
                                yf,
                            );
                        }
                        assert_eq!(
                            got, want,
                            "case {case}: {w}x{h} at ({xf}, {yf}), strides {src_stride} and {dst_stride}"
                        );
                        // The averaging form, into the block as predicted.
                        Scalar.luma_mc_avg(
                            BlockMut::new(&mut want, at, dst_stride, w, h).expect("dst"),
                            src,
                            xf,
                            yf,
                        );
                        // SAFETY: as above.
                        unsafe {
                            luma_mc_avg(
                                BlockMut::new(&mut got, at, dst_stride, w, h).expect("dst"),
                                src,
                                xf,
                                yf,
                            );
                        }
                        assert_eq!(got, want, "case {case}: averaged");
                        case += 1;
                    }
                }
            }
        }
    }

    /// The steps of "The centre j in 16 bits", on integers: `None` where a
    /// step other than `c`'s saturating add leaves 16 bits.
    fn centre_in_words(x: i32, y: i32, z: i32) -> Option<i32> {
        let word = |v: i32| i16::try_from(v).ok().map(i32::from);
        let quarter = word(word(x - y)? >> 2)?;
        let c = (word(quarter - y)? + z).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        let d = word((c >> 2) + z)?;
        Some(((d * 512 + (1 << 14)) >> 15).clamp(0, 255))
    }

    #[test]
    fn the_centre_in_words_is_the_equation() {
        // `x`, `y` and `z` over -5,100 to 21,420: every value of each at
        // both ends, the rest on a grid, so that both saturating cases and
        // the clip around them are reached.
        let (lo, hi) = (-5100, 21420);
        let mut values: Vec<i32> = (lo..lo + 20).chain(hi - 20..=hi).collect();
        values.extend((lo..=hi).step_by(499));
        let mut saturated = [0usize; 2];
        for &x in &values {
            for &y in &values {
                for z in (lo..=hi).step_by(13).chain(hi - 12..=hi) {
                    let want = ((x - 5 * y + 20 * z + 512) >> 10).clamp(0, 255);
                    assert_eq!(centre_in_words(x, y, z), Some(want), "{x} {y} {z}");
                    let c = ((x - y) >> 2) - y + z;
                    if c > i32::from(i16::MAX) {
                        saturated[0] += 1;
                    } else if c < i32::from(i16::MIN) {
                        saturated[1] += 1;
                    }
                }
            }
        }
        assert!(saturated.iter().all(|&n| n > 0), "{saturated:?}");
    }

    /// The positions with j, on windows whose rows give b1 its extremes:
    /// each row is 255, 0, 255, 255, 0, 255 (b1 10,710 at a column) or 0,
    /// 255, 0, 0, 255, 0 (-2,550) repeated, all in phase, or random, so
    /// that the centre's saturating add is reached, which the test checks.
    #[test]
    fn the_centre_of_extreme_rows_matches_the_reference() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let mut rng = Rng(0x7e17);
        let b1 = |row: &[u8], i: usize| -> i32 {
            let s = |k: usize| i32::from(row.get(i + k).copied().unwrap_or(0));
            s(0) + s(5) - 5 * (s(1) + s(4)) + 20 * (s(2) + s(3))
        };
        let mut saturated = [false; 2];
        for case in 0..3000 {
            let (w, h) = ([8, 16][case % 2], 16);
            let stride = w + 5;
            let phase = rng.below(6) as usize;
            let high = [255u8, 0, 255, 255, 0, 255];
            let mut source = Vec::with_capacity(stride * (h + 5));
            for _ in 0..h + 5 {
                let kind = rng.below(5);
                for i in 0..stride {
                    let s = high[(i + phase) % 6];
                    source.push(match kind {
                        0 | 1 => s,
                        2 | 3 => 255 - s,
                        _ => rng.below(256) as u8,
                    });
                }
            }
            for y in 0..h {
                for i in 0..w {
                    let r: Vec<i32> = (0..6)
                        .map(|k| b1(&source[(y + k) * stride..(y + k + 1) * stride], i))
                        .collect();
                    let (x, yy, z) = (r[0] + r[5], r[1] + r[4], r[2] + r[3]);
                    let c = ((x - yy) >> 2) - yy + z;
                    saturated[0] |= c > i32::from(i16::MAX);
                    saturated[1] |= c < i32::from(i16::MIN);
                }
            }
            let src = Block::new(&source, 0, stride, w + 5, h + 5).expect("src");
            for (xf, yf) in [(2, 2), (2, 1), (2, 3), (1, 2), (3, 2)] {
                let mut want = vec![0u8; w * h];
                let mut got = want.clone();
                Scalar.luma_mc(
                    BlockMut::new(&mut want, 0, w, w, h).expect("dst"),
                    src,
                    xf,
                    yf,
                );
                // SAFETY: the test returns before this where the CPU lacks
                // AVX2.
                unsafe {
                    luma_mc(
                        BlockMut::new(&mut got, 0, w, w, h).expect("dst"),
                        src,
                        xf,
                        yf,
                    )
                };
                assert_eq!(got, want, "case {case}: {w}x{h} at ({xf}, {yf})");
            }
        }
        assert_eq!(saturated, [true, true]);
    }

    #[test]
    fn gathered_rows_are_the_slice_rows_and_scattered_ones_land_there() {
        // Every start and stride around the slice's end, including the
        // ones whose extent check fails (by length or by overflow), where
        // a row past the end reads as zeros and is not written.
        let data: Vec<u8> = (0..200u32).map(|v| (v * 7 + 3) as u8).collect();
        let row = |start: usize, stride: usize, k: usize| -> [u8; 8] {
            let at = start.wrapping_add(k.wrapping_mul(stride));
            data.get(at..at.saturating_add(8))
                .and_then(|r| <[u8; 8]>::try_from(r).ok())
                .unwrap_or([0; 8])
        };
        for stride in [8usize, 9, 24, 25, 100, usize::MAX / 2, usize::MAX] {
            for start in [0usize, 1, 7, 40, 57, 60, 192, 193, 200, usize::MAX] {
                let got = gather::<4, 8>(&data, start, stride);
                for (k, r) in got.iter().enumerate() {
                    assert_eq!(*r, row(start, stride, k), "gather {start} {stride} row {k}");
                }
                let mut out = data.clone();
                let rows = [[0xa5u8; 8], [0x5a; 8], [0x11; 8], [0xee; 8]];
                scatter::<4, 8>(&mut out, start, stride, &rows);
                let mut want = data.clone();
                for (k, r) in rows.iter().enumerate() {
                    let at = start.wrapping_add(k.wrapping_mul(stride));
                    if let Some(to) = want.get_mut(at..at.saturating_add(8)) {
                        if to.len() == 8 {
                            to.copy_from_slice(r);
                        }
                    }
                }
                assert_eq!(out, want, "scatter {start} {stride}");
            }
        }
    }

    #[test]
    fn the_tightest_edges_match_the_reference() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        tightest_edges(filter_edge);
    }

    /// The narrowest steps EdgeMut::new accepts (along = 2 * reach for a
    /// vertical edge, across = lines for a horizontal one), in a plane that
    /// ends exactly where the edge's window does: `gather`, `scatter` and
    /// the rows reached through `rows_at` then read and write up to the
    /// slice's last byte. Through `kernel`, against the reference.
    pub(in crate::simd) fn tightest_edges(kernel: EdgeKernel) {
        let mut rng = Rng(77);
        for chroma in [false, true] {
            let (reach, r) = if chroma {
                (Reach::Chroma, 2)
            } else {
                (Reach::Luma, 4)
            };
            for vertical in [false, true] {
                for lines in [4usize, 8, 12, 16, 20, 32] {
                    for _ in 0..200 {
                        let (across, along) = if vertical { (1, 2 * r) } else { (lines, 1) };
                        let q0 = r * across;
                        let len = (lines - 1) * along + q0 + (r - 1) * across + 1;
                        let plane: Vec<u8> = (0..len).map(|_| rng.below(256) as u8).collect();
                        let params = EdgeParams {
                            alpha: rng.below(256) as u8,
                            beta: rng.below(19) as u8,
                            tc0: [0; 8].map(|_| rng.below(26) as u8),
                            bs: [0, 1, 2, 4].map(|_| rng.below(5) as u8),
                        };
                        let mut want = plane.clone();
                        let mut got = plane;
                        Scalar.filter_edge(
                            EdgeMut::new(&mut want, q0, across, along, lines, reach).expect("edge"),
                            &params,
                        );
                        // SAFETY: the callers return before this where the
                        // CPU lacks the kernel's features.
                        unsafe {
                            kernel(
                                EdgeMut::new(&mut got, q0, across, along, lines, reach)
                                    .expect("edge"),
                                &params,
                            );
                        }
                        assert_eq!(
                            got, want,
                            "chroma {chroma}, vertical {vertical}, {lines} lines"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_tightest_transforms_match_the_reference() {
        // Both transforms at strides from the narrowest a view takes (8),
        // in a plane that ends exactly at the block's last sample, at
        // offsets from 0 to 7: the rows reached through `rows_at_mut` then
        // read and write up to the slice's last byte. Coefficients at the
        // ends of their range and every mask.
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let mut rng = Rng(78);
        let coefficient = |rng: &mut Rng| match rng.below(6) {
            0 => 32767,
            1 => -32768,
            2 => 0,
            _ => rng.below(65536) - 32768,
        };
        for case in 0..2000usize {
            let stride = 8 + case % 4;
            let offset = (case / 4) % 8;
            let len = offset + 7 * stride + 8;
            let plane: Vec<u8> = (0..len).map(|_| rng.below(256) as u8).collect();
            let mut quad = [[0i32; 16]; 4];
            for c in quad.as_flattened_mut() {
                *c = coefficient(&mut rng);
            }
            let nz = ((case / 32) % 16) as u8;
            let mut want = plane.clone();
            let mut got = plane.clone();
            for (k, block) in quad.iter().enumerate() {
                if nz & (1 << k) != 0 {
                    let at = offset + (k / 2) * 4 * stride + (k % 2) * 4;
                    Scalar.idct4x4_add(
                        BlockMut::new(&mut want, at, stride, 4, 4).expect("block"),
                        block,
                    );
                }
            }
            // SAFETY: the test returns before this where the CPU lacks
            // AVX2.
            unsafe {
                idct4x4_add_quad(
                    BlockMut::new(&mut got, offset, stride, 8, 8).expect("block"),
                    &quad,
                    nz,
                );
            }
            assert_eq!(
                got, want,
                "quad: stride {stride}, offset {offset}, nz {nz:#x}"
            );
            let mut coeffs = [0i32; 64];
            for c in coeffs.iter_mut() {
                *c = coefficient(&mut rng);
            }
            let mut want = plane.clone();
            let mut got = plane;
            Scalar.idct8x8_add(
                BlockMut::new(&mut want, offset, stride, 8, 8).expect("block"),
                &coeffs,
            );
            // SAFETY: as above.
            unsafe {
                idct8x8_add(
                    BlockMut::new(&mut got, offset, stride, 8, 8).expect("block"),
                    &coeffs,
                );
            }
            assert_eq!(got, want, "8x8: stride {stride}, offset {offset}");
        }
    }
}
