//! The arm64 kernels, written with NEON intrinsics.
//!
//! NEON is part of the arm64 baseline, so the kernels need no feature
//! detection; they are `unsafe fn`s because the intrinsics are. The loads
//! and stores go through the helpers at the top of the file, which check
//! the length of the slice they are given before they form a pointer;
//! nothing else in the file touches memory.
//!
//! The file mirrors `x86.rs`: the same algorithms, the same order, and the
//! same arithmetic widths, so that both give exactly what the scalar
//! kernels give.

use core::arch::aarch64::{
    int16x8_t, int32x4_t, uint8x8_t, uint8x16_t, vabdq_s16, vaddq_s16, vaddq_s32, vaddq_u16,
    vandq_s16, vandq_u16, vbslq_s16, vcgtq_s16, vcombine_s16, vcombine_u8, vdup_n_u8, vdupq_n_s16,
    vdupq_n_s32, vdupq_n_u16, vget_high_s16, vget_high_u8, vget_low_s16, vget_low_u8, vld1_u8,
    vld1q_s16, vld1q_s32, vld1q_u8, vmaxq_s16, vmaxq_s32, vminq_s16, vminq_s32, vmlal_u8,
    vmovl_s16, vmovl_u8, vmovn_s32, vmull_u8, vmulq_n_s16, vmulq_n_s32, vmulq_s16, vmvnq_u16,
    vqmovn_s32, vqmovun_s16, vreinterpret_u8_u16, vreinterpret_u8_u32, vreinterpret_u16_u8,
    vreinterpret_u32_u8, vrhadd_u8, vshlq_n_s16, vshlq_s16, vshlq_s32, vshrq_n_s16, vshrq_n_s32,
    vshrq_n_u16, vst1_u8, vst1q_s16, vst1q_u8, vsubq_s16, vsubq_s32, vtrn1_u8, vtrn1_u16,
    vtrn1_u32, vtrn2_u8, vtrn2_u16, vtrn2_u32,
};

use crate::scalar::{MAX_B1_ROWS, MAX_BLOCK, Tap, taps};

/// Whether an 8-wide luma block at a full-sample position goes to the
/// vector kernel: no, the reference copies it faster
/// (`docs/measurements/h264-kernels.md`).
pub(super) const LUMA_COPY8_VECTOR: bool = false;

/// Whether a chroma block goes to the vector kernel: no, as on x86-64.
/// The `kernels-arm64` job measured it at 1.01 to 1.05 against the
/// reference (`docs/measurements/h264-kernels.md`).
pub(super) const CHROMA_MC_VECTOR: bool = false;

/// Whether a weighted prediction goes to the vector kernel: no. The same
/// job measured 1.32, where x86-64 measures 0.93; the kernel splits each
/// row into 32-bit halves, and the performance step is where that is
/// worth changing.
pub(super) const COMBINE_VECTOR: bool = false;
use crate::views::{Block, BlockMut, EdgeMut, Reach};
use crate::{Combine, EdgeParams};

// ---------------------------------------------------------------------
// Loads and stores. Each one checks the slice it is handed; they are the
// only places in this file where a pointer is formed.
// ---------------------------------------------------------------------

/// Copies `from` into `to` without a call: the lengths here are one vector
/// at most, where a `copy_from_slice` would leave a `memcpy`.
#[inline(always)]
fn copy_small(to: &mut [u8], from: &[u8]) {
    for (d, s) in to.iter_mut().zip(from.iter()) {
        *d = *s;
    }
}

/// Sixteen bytes of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load16(src: &[u8]) -> uint8x16_t {
    if src.len() >= 16 {
        // SAFETY: the slice holds the 16 bytes the load reads.
        unsafe { vld1q_u8(src.as_ptr()) }
    } else {
        let mut padded = [0u8; 16];
        copy_small(&mut padded, src);
        // SAFETY: `padded` is 16 bytes long.
        unsafe { vld1q_u8(padded.as_ptr()) }
    }
}

/// Eight bytes of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load8(src: &[u8]) -> uint8x8_t {
    if src.len() >= 8 {
        // SAFETY: the slice holds the 8 bytes the load reads.
        unsafe { vld1_u8(src.as_ptr()) }
    } else {
        let mut padded = [0u8; 8];
        copy_small(&mut padded, src);
        // SAFETY: `padded` is 8 bytes long.
        unsafe { vld1_u8(padded.as_ptr()) }
    }
}

/// Writes the first `count` bytes of `v`, or as many as `dst` holds.
#[inline(always)]
fn store_bytes(dst: &mut [u8], v: uint8x16_t, count: usize) {
    let n = count.min(dst.len());
    if n >= 16 {
        // SAFETY: the slice holds the 16 bytes the store writes.
        unsafe { vst1q_u8(dst.as_mut_ptr(), v) }
        return;
    }
    let mut bytes = [0u8; 16];
    // SAFETY: `bytes` is 16 bytes long.
    unsafe { vst1q_u8(bytes.as_mut_ptr(), v) }
    copy_small(dst.get_mut(..n).unwrap_or_default(), &bytes);
}

/// Writes the first `count` of the eight bytes of `v`, or as many as
/// `dst` holds.
#[inline(always)]
fn store8_bytes(dst: &mut [u8], v: uint8x8_t, count: usize) {
    let n = count.min(dst.len());
    if n >= 8 {
        // SAFETY: the slice holds the 8 bytes the store writes.
        unsafe { vst1_u8(dst.as_mut_ptr(), v) }
        return;
    }
    let mut bytes = [0u8; 8];
    // SAFETY: `bytes` is 8 bytes long.
    unsafe { vst1_u8(bytes.as_mut_ptr(), v) }
    copy_small(dst.get_mut(..n).unwrap_or_default(), &bytes);
}

/// Eight 16-bit values of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load_i16x8(src: &[i16]) -> int16x8_t {
    if src.len() >= 8 {
        // SAFETY: the slice holds the eight values the load reads.
        unsafe { vld1q_s16(src.as_ptr()) }
    } else {
        let mut padded = [0i16; 8];
        for (d, s) in padded.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        // SAFETY: `padded` holds eight values.
        unsafe { vld1q_s16(padded.as_ptr()) }
    }
}

/// Stores eight 16-bit values, as many as `dst` holds.
#[inline(always)]
fn store_i16x8(dst: &mut [i16], v: int16x8_t) {
    if dst.len() >= 8 {
        // SAFETY: the slice holds the eight values the store writes.
        unsafe { vst1q_s16(dst.as_mut_ptr(), v) }
    } else {
        let mut padded = [0i16; 8];
        // SAFETY: `padded` holds eight values.
        unsafe { vst1q_s16(padded.as_mut_ptr(), v) }
        for (d, s) in dst.iter_mut().zip(padded.iter()) {
            *d = *s;
        }
    }
}

/// Four 32-bit values of `src`, zero-padded where `src` is shorter.
#[inline(always)]
fn load4_i32(src: &[i32]) -> int32x4_t {
    if src.len() >= 4 {
        // SAFETY: the slice holds the four values the load reads.
        unsafe { vld1q_s32(src.as_ptr()) }
    } else {
        let mut padded = [0i32; 4];
        for (d, s) in padded.iter_mut().zip(src.iter()) {
            *d = *s;
        }
        // SAFETY: `padded` holds four values.
        unsafe { vld1q_s32(padded.as_ptr()) }
    }
}

/// Widens eight unsigned bytes to 16-bit lanes.
#[inline(always)]
unsafe fn widen(v: uint8x8_t) -> int16x8_t {
    // SAFETY: a register operation.
    unsafe { core::mem::transmute::<core::arch::aarch64::uint16x8_t, int16x8_t>(vmovl_u8(v)) }
}

/// Narrows two sets of 16-bit lanes to sixteen bytes, clipping to 0..=255
/// (the Clip1 of the equations).
#[inline(always)]
unsafe fn narrow(low: int16x8_t, high: int16x8_t) -> uint8x16_t {
    // SAFETY: register operations.
    unsafe { vcombine_u8(vqmovun_s16(low), vqmovun_s16(high)) }
}

// ---------------------------------------------------------------------
// The inverse 8x8 transform (clause 8.5.13.2). The 4x4 transform stays in
// the reference, as on x86-64.
// ---------------------------------------------------------------------

/// Transposes four vectors of four 32-bit values.
#[inline(always)]
unsafe fn transpose4(rows: [int32x4_t; 4]) -> [int32x4_t; 4] {
    // SAFETY: register operations.
    unsafe {
        use core::arch::aarch64::{vtrn1q_s32, vtrn1q_s64, vtrn2q_s32, vtrn2q_s64};
        let [r0, r1, r2, r3] = rows;
        let a = vtrn1q_s32(r0, r1);
        let b = vtrn2q_s32(r0, r1);
        let c = vtrn1q_s32(r2, r3);
        let d = vtrn2q_s32(r2, r3);
        let as64 =
            |v: int32x4_t| core::mem::transmute::<int32x4_t, core::arch::aarch64::int64x2_t>(v);
        let as32 = |v: core::arch::aarch64::int64x2_t| {
            core::mem::transmute::<core::arch::aarch64::int64x2_t, int32x4_t>(v)
        };
        [
            as32(vtrn1q_s64(as64(a), as64(c))),
            as32(vtrn1q_s64(as64(b), as64(d))),
            as32(vtrn2q_s64(as64(a), as64(c))),
            as32(vtrn2q_s64(as64(b), as64(d))),
        ]
    }
}

/// One pass of the inverse 8x8 transform (Equations 8-351 to 8-366).
#[inline(always)]
unsafe fn idct8_pass(x: [int32x4_t; 8]) -> [int32x4_t; 8] {
    // SAFETY: register operations.
    unsafe {
        let [x0, x1, x2, x3, x4, x5, x6, x7] = x;
        let half = |v: int32x4_t| vshrq_n_s32::<1>(v);
        let quarter = |v: int32x4_t| vshrq_n_s32::<2>(v);
        let zero = vdupq_n_s32(0);
        let e0 = vaddq_s32(x0, x4);
        let e1 = vsubq_s32(vsubq_s32(vaddq_s32(vsubq_s32(zero, x3), x5), x7), half(x7));
        let e2 = vsubq_s32(x0, x4);
        let e3 = vsubq_s32(vsubq_s32(vaddq_s32(x1, x7), x3), half(x3));
        let e4 = vsubq_s32(half(x2), x6);
        let e5 = vaddq_s32(vaddq_s32(vaddq_s32(vsubq_s32(zero, x1), x7), x5), half(x5));
        let e6 = vaddq_s32(x2, half(x6));
        let e7 = vaddq_s32(vaddq_s32(vaddq_s32(x3, x5), x1), half(x1));
        let f0 = vaddq_s32(e0, e6);
        let f1 = vaddq_s32(e1, quarter(e7));
        let f2 = vaddq_s32(e2, e4);
        let f3 = vaddq_s32(e3, quarter(e5));
        let f4 = vsubq_s32(e2, e4);
        let f5 = vsubq_s32(quarter(e3), e5);
        let f6 = vsubq_s32(e0, e6);
        let f7 = vsubq_s32(e7, quarter(e1));
        [
            vaddq_s32(f0, f7),
            vaddq_s32(f2, f5),
            vaddq_s32(f4, f3),
            vaddq_s32(f6, f1),
            vsubq_s32(f6, f1),
            vsubq_s32(f4, f3),
            vsubq_s32(f2, f5),
            vsubq_s32(f0, f7),
        ]
    }
}

/// The horizontal pass of the inverse 8x8 transform: in and out, a row is
/// `left[i]` followed by `right[i]`.
#[inline(always)]
unsafe fn horizontal8(
    left: [int32x4_t; 8],
    right: [int32x4_t; 8],
) -> ([int32x4_t; 8], [int32x4_t; 8]) {
    // SAFETY: register operations.
    unsafe {
        let [l0, l1, l2, l3, l4, l5, l6, l7] = left;
        let [r0, r1, r2, r3, r4, r5, r6, r7] = right;
        let a = transpose4([l0, l1, l2, l3]);
        let b = transpose4([r0, r1, r2, r3]);
        let top = idct8_pass([a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]]);
        let c = transpose4([l4, l5, l6, l7]);
        let d = transpose4([r4, r5, r6, r7]);
        let bottom = idct8_pass([c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]);
        let top_left = transpose4([top[0], top[1], top[2], top[3]]);
        let top_right = transpose4([top[4], top[5], top[6], top[7]]);
        let bottom_left = transpose4([bottom[0], bottom[1], bottom[2], bottom[3]]);
        let bottom_right = transpose4([bottom[4], bottom[5], bottom[6], bottom[7]]);
        (
            [
                top_left[0],
                top_left[1],
                top_left[2],
                top_left[3],
                bottom_left[0],
                bottom_left[1],
                bottom_left[2],
                bottom_left[3],
            ],
            [
                top_right[0],
                top_right[1],
                top_right[2],
                top_right[3],
                bottom_right[0],
                bottom_right[1],
                bottom_right[2],
                bottom_right[3],
            ],
        )
    }
}

/// Adds one row of residual to eight prediction samples, with rounding and
/// clipping (Equation 8-369).
///
/// The sum is formed in 32 bits and clipped before it is narrowed, so that
/// the narrowing can never change a value, whatever the coefficients were.
#[inline(always)]
unsafe fn add_row8(dst: &mut [u8], left: int32x4_t, right: int32x4_t) {
    // SAFETY: the load and store check their slice; the rest are register
    // operations.
    unsafe {
        let round = vdupq_n_s32(32);
        let l = vshrq_n_s32::<6>(vaddq_s32(left, round));
        let r = vshrq_n_s32::<6>(vaddq_s32(right, round));
        let samples = widen(load8(dst));
        let low = vaddq_s32(vmovl_s16(vget_low_s16(samples)), l);
        let high = vaddq_s32(vmovl_s16(vget_high_s16(samples)), r);
        let clip = |v: int32x4_t| vmaxq_s32(vminq_s32(v, vdupq_n_s32(255)), vdupq_n_s32(0));
        let packed = vcombine_s16(vmovn_s32(clip(low)), vmovn_s32(clip(high)));
        store8_bytes(dst, vqmovun_s16(packed), 8);
    }
}

/// Inverse 8x8 transform of `coeffs`, added to `dst` (clause 8.5.13.2).
pub(super) unsafe fn idct8x8_add(mut dst: BlockMut<'_>, coeffs: &[i32; 64]) {
    // SAFETY: every access goes through the helpers.
    unsafe {
        let mut left = [vdupq_n_s32(0); 8];
        let mut right = [vdupq_n_s32(0); 8];
        for (i, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
            *l = load4_i32(coeffs.get(i * 8..i * 8 + 4).unwrap_or_default());
            *r = load4_i32(coeffs.get(i * 8 + 4..i * 8 + 8).unwrap_or_default());
        }
        let (fleft, fright) = horizontal8(left, right);
        let gleft = idct8_pass(fleft);
        let gright = idct8_pass(fright);
        for (y, row) in dst.rows_mut().enumerate() {
            let (Some(l), Some(r)) = (gleft.get(y), gright.get(y)) else {
                return;
            };
            add_row8(row, *l, *r);
        }
    }
}

// ---------------------------------------------------------------------
// Interpolation (clauses 8.4.2.2.1 and 8.4.2.2.2).
// ---------------------------------------------------------------------

/// The 6-tap filter of Equations 8-241 and 8-242 over eight 16-bit lanes.
#[inline(always)]
unsafe fn tap6_i16(v: [int16x8_t; 6]) -> int16x8_t {
    // SAFETY: register operations.
    unsafe {
        let [a, b, c, d, e, f] = v;
        let ends = vaddq_s16(a, f);
        let inner = vmulq_n_s16(vaddq_s16(b, e), 5);
        let middle = vmulq_n_s16(vaddq_s16(c, d), 20);
        vaddq_s16(vsubq_s16(ends, inner), middle)
    }
}

/// The same over four 32-bit lanes (Equation 8-246).
#[inline(always)]
unsafe fn tap6_i32(v: [int32x4_t; 6]) -> int32x4_t {
    // SAFETY: register operations.
    unsafe {
        let [a, b, c, d, e, f] = v;
        let ends = vaddq_s32(a, f);
        let inner = vmulq_n_s32(vaddq_s32(b, e), 5);
        let middle = vmulq_n_s32(vaddq_s32(c, d), 20);
        vaddq_s32(vsubq_s32(ends, inner), middle)
    }
}

/// The horizontal 6-tap intermediates b1 of one source row.
#[inline(always)]
unsafe fn horizontal_taps(row: &[u8], out: &mut [i16]) {
    // SAFETY: the loads and stores check their slices.
    unsafe {
        let mut at = 0;
        while at < out.len() {
            let mut v = [vdupq_n_s16(0); 6];
            for (k, slot) in v.iter_mut().enumerate() {
                *slot = widen(load8(row.get(at + k..).unwrap_or_default()));
            }
            store_i16x8(out.get_mut(at..).unwrap_or_default(), tap6_i16(v));
            at += 8;
        }
    }
}

/// Rounds eight half-sample intermediates: `(v + 16) >> 5`.
#[inline(always)]
unsafe fn half_round(v: int16x8_t) -> int16x8_t {
    // SAFETY: register operations.
    unsafe { vshrq_n_s16::<5>(vaddq_s16(v, vdupq_n_s16(16))) }
}

/// Eight vertical 6-tap intermediates of the rows `y..y + 6`, from column
/// `from`.
#[inline(always)]
unsafe fn vertical_tap(src: &Block<'_>, y: usize, from: usize) -> int16x8_t {
    // SAFETY: the loads check their slices.
    unsafe {
        let mut v = [vdupq_n_s16(0); 6];
        for (k, slot) in v.iter_mut().enumerate() {
            let row = src.row(y + k).unwrap_or_default();
            *slot = widen(load8(row.get(from..).unwrap_or_default()));
        }
        tap6_i16(v)
    }
}

/// Eight half-samples at the centre of four integer samples, as 16-bit
/// values (Equations 8-246 and 8-247).
#[inline(always)]
unsafe fn centre_tap(b1: &[i16], y: usize, at: usize) -> int16x8_t {
    // SAFETY: the loads check their slices.
    unsafe {
        let four = |at: usize, high: bool| {
            let mut v = [vdupq_n_s32(0); 6];
            for (k, slot) in v.iter_mut().enumerate() {
                let row = b1.get((y + k) * MAX_BLOCK..).unwrap_or_default();
                let wide = load_i16x8(row.get(at..).unwrap_or_default());
                *slot = if high {
                    vmovl_s16(vget_high_s16(wide))
                } else {
                    vmovl_s16(vget_low_s16(wide))
                };
            }
            vshrq_n_s32::<10>(vaddq_s32(tap6_i32(v), vdupq_n_s32(512)))
        };
        // The narrowing saturates, as the x86 kernel's pack does.
        vcombine_s16(vqmovn_s32(four(at, false)), vqmovn_s32(four(at, true)))
    }
}

/// The `w` samples of one tap of one output row, as bytes.
#[inline(always)]
unsafe fn tap_row(tap: Tap, src: &Block<'_>, b1: &[i16], y: usize, w: usize) -> uint8x16_t {
    // SAFETY: the loads check their slices.
    unsafe {
        match tap {
            Tap::Int { dx, dy } => load16(
                src.row(y + 2 + dy)
                    .and_then(|r| r.get(2 + dx..))
                    .unwrap_or_default(),
            ),
            Tap::B { dy } => {
                let row = b1
                    .chunks_exact(MAX_BLOCK)
                    .nth(y + 2 + dy)
                    .unwrap_or_default();
                let low = half_round(load_i16x8(row));
                let high = if w > 8 {
                    half_round(load_i16x8(row.get(8..).unwrap_or_default()))
                } else {
                    low
                };
                narrow(low, high)
            }
            Tap::H { dx } => {
                let low = half_round(vertical_tap(src, y, 2 + dx));
                let high = if w > 8 {
                    half_round(vertical_tap(src, y, 10 + dx))
                } else {
                    low
                };
                narrow(low, high)
            }
            Tap::J => {
                let low = centre_tap(b1, y, 0);
                let high = if w > 8 { centre_tap(b1, y, 8) } else { low };
                narrow(low, high)
            }
        }
    }
}

/// Luma quarter-sample interpolation (clause 8.4.2.2.1).
pub(super) unsafe fn luma_mc(mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    // SAFETY: every access goes through the helpers.
    unsafe {
        let (w, h) = (dst.w(), dst.h());
        if (xf, yf) == (0, 0) {
            for (out, row) in dst.rows_mut().zip(src.rows_from(2)) {
                store_bytes(out, load16(row.get(2..).unwrap_or_default()), w);
            }
            return;
        }
        let (first, second) = taps(xf, yf);
        let needs = |want: fn(&Tap) -> bool| want(&first) || second.as_ref().is_some_and(want);
        let needs_b = needs(|t| matches!(t, Tap::B { .. }));
        let needs_j = needs(|t| matches!(t, Tap::J));
        let mut b1 = [0i16; MAX_B1_ROWS * MAX_BLOCK];
        if needs_b || needs_j {
            let (skip, take) = if needs_j { (0, h + 5) } else { (2, h + 1) };
            let rows = b1.chunks_exact_mut(MAX_BLOCK).zip(src.rows());
            for (out, row) in rows.skip(skip).take(take) {
                horizontal_taps(row, out.get_mut(..w).unwrap_or_default());
            }
        }
        for (y, out) in dst.rows_mut().enumerate() {
            let a = tap_row(first, &src, &b1, y, w);
            let combined = match second {
                None => a,
                Some(tap) => {
                    let b = tap_row(tap, &src, &b1, y, w);
                    // The rounding halving add is `(a + b + 1) >> 1`.
                    vcombine_u8(
                        vrhadd_u8(vget_low_u8(a), vget_low_u8(b)),
                        vrhadd_u8(vget_high_u8(a), vget_high_u8(b)),
                    )
                }
            };
            store_bytes(out, combined, w);
        }
    }
}

/// Chroma eighth-sample interpolation (clause 8.4.2.2.2).
pub(super) unsafe fn chroma_mc(mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    // SAFETY: every access goes through the helpers.
    unsafe {
        let w = dst.w();
        let (x, y) = (u32::from(xf), u32::from(yf));
        let (wa, wb) = (((8 - x) * (8 - y)) as u8, (x * (8 - y)) as u8);
        let (wc, wd) = (((8 - x) * y) as u8, (x * y) as u8);
        let mut up = src.rows();
        let mut down = src.rows_from(1);
        for out in dst.rows_mut() {
            let (Some(above), Some(below)) = (up.next(), down.next()) else {
                return;
            };
            let mut at = 0;
            while at < w {
                // The four weights add up to 64, so the whole sum of
                // Equation 8-266 fits 16 bits.
                let mut sum = vmull_u8(load8(above.get(at..).unwrap_or_default()), vdup_n_u8(wa));
                sum = vmlal_u8(
                    sum,
                    load8(above.get(at + 1..).unwrap_or_default()),
                    vdup_n_u8(wb),
                );
                sum = vmlal_u8(
                    sum,
                    load8(below.get(at..).unwrap_or_default()),
                    vdup_n_u8(wc),
                );
                sum = vmlal_u8(
                    sum,
                    load8(below.get(at + 1..).unwrap_or_default()),
                    vdup_n_u8(wd),
                );
                let rounded = vaddq_u16(sum, vdupq_n_u16(32));
                let bytes = vqmovun_s16(core::mem::transmute::<
                    core::arch::aarch64::uint16x8_t,
                    int16x8_t,
                >(vshrq_n_u16::<6>(rounded)));
                store8_bytes(out.get_mut(at..).unwrap_or_default(), bytes, 8);
                at += 8;
            }
        }
    }
}

// ---------------------------------------------------------------------
// Prediction combining (clause 8.4.2.3).
// ---------------------------------------------------------------------

/// Writes the prediction of a block from the two lists' predictions.
pub(super) unsafe fn combine(mut dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine) {
    // SAFETY: every access goes through the helpers.
    unsafe {
        for ((out, a), b) in dst.rows_mut().zip(p0.rows()).zip(p1.rows()) {
            match mode {
                Combine::Uni {
                    list,
                    log_wd,
                    w: weight,
                    o,
                } => {
                    let from = if list == 0 { a } else { b };
                    weighted_one(out, from, log_wd, weight, o);
                }
                Combine::Bi { log_wd, w0, w1, o } => weighted_two(out, a, b, log_wd, w0, w1, o),
                // The entry point sends copying and averaging to the
                // reference, which is as fast there.
                Combine::Copy(_) | Combine::Average => return,
            }
        }
    }
}

/// Equations 8-286 and 8-287: one list, explicit weighting.
#[inline(always)]
unsafe fn weighted_one(out: &mut [u8], src: &[u8], log_wd: i32, weight: i32, offset: i32) {
    // SAFETY: the loads and stores check their slices.
    unsafe {
        let w = vdupq_n_s16(weight as i16);
        let o = vdupq_n_s16(offset as i16);
        let round = vdupq_n_s16(if log_wd >= 1 { 1 << (log_wd - 1) } else { 0 });
        let shift = vdupq_n_s16(-(log_wd as i16));
        let mut at = 0;
        while at < out.len() {
            let samples = widen(load8(src.get(at..).unwrap_or_default()));
            let scaled = vmulq_s16(samples, w);
            let shifted = if log_wd >= 1 {
                vshlq_s16(vaddq_s16(scaled, round), shift)
            } else {
                scaled
            };
            let sum = vaddq_s16(shifted, o);
            store8_bytes(out.get_mut(at..).unwrap_or_default(), vqmovun_s16(sum), 8);
            at += 8;
        }
    }
}

/// Equation 8-288: both lists. The sum of the two products can leave 16
/// bits, so it is computed in 32.
#[inline(always)]
unsafe fn weighted_two(
    out: &mut [u8],
    a: &[u8],
    b: &[u8],
    log_wd: i32,
    w0: i32,
    w1: i32,
    offset: i32,
) {
    // SAFETY: the loads and stores check their slices.
    unsafe {
        let round = vdupq_n_s32(1 << log_wd);
        let shift = vdupq_n_s32(-(log_wd + 1));
        let off = vdupq_n_s32(offset);
        let mut at = 0;
        while at < out.len() {
            let x = widen(load8(a.get(at..).unwrap_or_default()));
            let y = widen(load8(b.get(at..).unwrap_or_default()));
            let half = |v: int16x8_t, high: bool| {
                if high {
                    vmovl_s16(vget_high_s16(v))
                } else {
                    vmovl_s16(vget_low_s16(v))
                }
            };
            let one = |high: bool| {
                let sum = vaddq_s32(
                    vaddq_s32(
                        vmulq_n_s32(half(x, high), w0),
                        vmulq_n_s32(half(y, high), w1),
                    ),
                    round,
                );
                vaddq_s32(vshlq_s32(sum, shift), off)
            };
            // The narrowing saturates, as the x86 kernel's pack does: the
            // sum of two weighted samples can leave 16 bits at the ends of
            // the ranges clause 7.4.3.2 allows.
            let packed = vcombine_s16(vqmovn_s32(one(false)), vqmovn_s32(one(true)));
            store8_bytes(
                out.get_mut(at..).unwrap_or_default(),
                vqmovun_s16(packed),
                8,
            );
            at += 8;
        }
    }
}

// ---------------------------------------------------------------------
// The deblocking filter (clauses 8.7.2.3 and 8.7.2.4).
// ---------------------------------------------------------------------

/// Transposes eight vectors of eight bytes.
#[inline(always)]
unsafe fn transpose8x8_u8(lines: [uint8x8_t; 8]) -> [uint8x8_t; 8] {
    // SAFETY: register operations.
    unsafe {
        let [l0, l1, l2, l3, l4, l5, l6, l7] = lines;
        let a0 = vtrn1_u8(l0, l1);
        let a1 = vtrn2_u8(l0, l1);
        let a2 = vtrn1_u8(l2, l3);
        let a3 = vtrn2_u8(l2, l3);
        let a4 = vtrn1_u8(l4, l5);
        let a5 = vtrn2_u8(l4, l5);
        let a6 = vtrn1_u8(l6, l7);
        let a7 = vtrn2_u8(l6, l7);
        let u16_of = |v: uint8x8_t| vreinterpret_u16_u8(v);
        let u8_of16 = |v: core::arch::aarch64::uint16x4_t| vreinterpret_u8_u16(v);
        let b0 = u8_of16(vtrn1_u16(u16_of(a0), u16_of(a2)));
        let b1 = u8_of16(vtrn1_u16(u16_of(a1), u16_of(a3)));
        let b2 = u8_of16(vtrn2_u16(u16_of(a0), u16_of(a2)));
        let b3 = u8_of16(vtrn2_u16(u16_of(a1), u16_of(a3)));
        let b4 = u8_of16(vtrn1_u16(u16_of(a4), u16_of(a6)));
        let b5 = u8_of16(vtrn1_u16(u16_of(a5), u16_of(a7)));
        let b6 = u8_of16(vtrn2_u16(u16_of(a4), u16_of(a6)));
        let b7 = u8_of16(vtrn2_u16(u16_of(a5), u16_of(a7)));
        let u32_of = |v: uint8x8_t| vreinterpret_u32_u8(v);
        let u8_of32 = |v: core::arch::aarch64::uint32x2_t| vreinterpret_u8_u32(v);
        [
            u8_of32(vtrn1_u32(u32_of(b0), u32_of(b4))),
            u8_of32(vtrn1_u32(u32_of(b1), u32_of(b5))),
            u8_of32(vtrn1_u32(u32_of(b2), u32_of(b6))),
            u8_of32(vtrn1_u32(u32_of(b3), u32_of(b7))),
            u8_of32(vtrn2_u32(u32_of(b0), u32_of(b4))),
            u8_of32(vtrn2_u32(u32_of(b1), u32_of(b5))),
            u8_of32(vtrn2_u32(u32_of(b2), u32_of(b6))),
            u8_of32(vtrn2_u32(u32_of(b3), u32_of(b7))),
        ]
    }
}

/// `|a - b| < limit`, as a lane mask.
#[inline(always)]
unsafe fn near(a: int16x8_t, b: int16x8_t, limit: int16x8_t) -> core::arch::aarch64::uint16x8_t {
    // SAFETY: register operations.
    unsafe { vcgtq_s16(limit, vabdq_s16(a, b)) }
}

/// The luma filter of eight lines.
#[inline(always)]
unsafe fn filter_luma8(
    s: &mut [int16x8_t; 8],
    alpha: int16x8_t,
    beta: int16x8_t,
    bs: int16x8_t,
    tc0: int16x8_t,
) {
    // SAFETY: register operations.
    unsafe {
        let [p3, p2, p1, p0, q0, q1, q2, q3] = *s;
        let zero = vdupq_n_s16(0);
        let one = vdupq_n_s16(1);
        let filtered = vandq_u16(
            vandq_u16(near(p0, q0, alpha), near(p1, p0, beta)),
            vandq_u16(near(q1, q0, beta), vcgtq_s16(bs, zero)),
        );
        let ap = near(p2, p0, beta);
        let aq = near(q2, q0, beta);
        let strong_bs = vcgtq_s16(bs, vdupq_n_s16(3));

        let as_s16 = |m: core::arch::aarch64::uint16x8_t| {
            core::mem::transmute::<core::arch::aarch64::uint16x8_t, int16x8_t>(m)
        };
        let tc = vaddq_s16(
            vaddq_s16(tc0, vandq_s16(as_s16(ap), one)),
            vandq_s16(as_s16(aq), one),
        );
        let raw = vshrq_n_s16::<3>(vaddq_s16(
            vaddq_s16(vshlq_n_s16::<2>(vsubq_s16(q0, p0)), vsubq_s16(p1, q1)),
            vdupq_n_s16(4),
        ));
        let delta = vmaxq_s16(vminq_s16(raw, tc), vsubq_s16(zero, tc));
        let weak_p0 = vaddq_s16(p0, delta);
        let weak_q0 = vsubq_s16(q0, delta);
        let mean = vshrq_n_s16::<1>(vaddq_s16(vaddq_s16(p0, q0), one));
        let clamp0 = |v: int16x8_t| vmaxq_s16(vminq_s16(v, tc0), vsubq_s16(zero, tc0));
        let weak_p1 = vaddq_s16(
            p1,
            clamp0(vshrq_n_s16::<1>(vsubq_s16(
                vaddq_s16(p2, mean),
                vshlq_n_s16::<1>(p1),
            ))),
        );
        let weak_q1 = vaddq_s16(
            q1,
            clamp0(vshrq_n_s16::<1>(vsubq_s16(
                vaddq_s16(q2, mean),
                vshlq_n_s16::<1>(q1),
            ))),
        );

        let strong = vcgtq_s16(
            vaddq_s16(vshrq_n_s16::<2>(alpha), vdupq_n_s16(2)),
            vabdq_s16(p0, q0),
        );
        let two = vdupq_n_s16(2);
        let four = vdupq_n_s16(4);
        let sum = |v: &[int16x8_t]| v.iter().fold(zero, |a, b| vaddq_s16(a, *b));
        let p0_long = vshrq_n_s16::<3>(sum(&[
            p2,
            vshlq_n_s16::<1>(p1),
            vshlq_n_s16::<1>(p0),
            vshlq_n_s16::<1>(q0),
            q1,
            four,
        ]));
        let p1_long = vshrq_n_s16::<2>(sum(&[p2, p1, p0, q0, two]));
        let p2_long = vshrq_n_s16::<3>(sum(&[
            vshlq_n_s16::<1>(p3),
            vmulq_n_s16(p2, 3),
            p1,
            p0,
            q0,
            four,
        ]));
        let p0_short = vshrq_n_s16::<2>(sum(&[vshlq_n_s16::<1>(p1), p0, q1, two]));
        let q0_long = vshrq_n_s16::<3>(sum(&[
            p1,
            vshlq_n_s16::<1>(p0),
            vshlq_n_s16::<1>(q0),
            vshlq_n_s16::<1>(q1),
            q2,
            four,
        ]));
        let q1_long = vshrq_n_s16::<2>(sum(&[p0, q0, q1, q2, two]));
        let q2_long = vshrq_n_s16::<3>(sum(&[
            vshlq_n_s16::<1>(q3),
            vmulq_n_s16(q2, 3),
            q1,
            q0,
            p0,
            four,
        ]));
        let q0_short = vshrq_n_s16::<2>(sum(&[vshlq_n_s16::<1>(q1), q0, p1, two]));

        let strong_p = vandq_u16(strong_bs, vandq_u16(ap, strong));
        let strong_q = vandq_u16(strong_bs, vandq_u16(aq, strong));
        let not_strong_bs = vmvnq_u16(strong_bs);
        let new_p0 = vbslq_s16(strong_p, p0_long, vbslq_s16(strong_bs, p0_short, weak_p0));
        let new_q0 = vbslq_s16(strong_q, q0_long, vbslq_s16(strong_bs, q0_short, weak_q0));
        let new_p1 = vbslq_s16(
            strong_p,
            p1_long,
            vbslq_s16(vandq_u16(not_strong_bs, ap), weak_p1, p1),
        );
        let new_q1 = vbslq_s16(
            strong_q,
            q1_long,
            vbslq_s16(vandq_u16(not_strong_bs, aq), weak_q1, q1),
        );
        let new_p2 = vbslq_s16(strong_p, p2_long, p2);
        let new_q2 = vbslq_s16(strong_q, q2_long, q2);

        let clip = |v: int16x8_t| vmaxq_s16(vminq_s16(v, vdupq_n_s16(255)), zero);
        s[1] = vbslq_s16(filtered, clip(new_p2), p2);
        s[2] = vbslq_s16(filtered, clip(new_p1), p1);
        s[3] = vbslq_s16(filtered, clip(new_p0), p0);
        s[4] = vbslq_s16(filtered, clip(new_q0), q0);
        s[5] = vbslq_s16(filtered, clip(new_q1), q1);
        s[6] = vbslq_s16(filtered, clip(new_q2), q2);
    }
}

/// The chroma filter of eight lines (clauses 8.7.2.3 and 8.7.2.4 with
/// chromaStyleFilteringFlag).
///
/// `s` holds p1, p0, q0 and q1, one lane per line.
#[inline(always)]
unsafe fn filter_chroma8(
    s: &mut [int16x8_t; 8],
    alpha: int16x8_t,
    beta: int16x8_t,
    bs: int16x8_t,
    tc0: int16x8_t,
) {
    // SAFETY: register operations.
    unsafe {
        let [p1, p0, q0, q1, ..] = *s;
        let zero = vdupq_n_s16(0);
        let filtered = vandq_u16(
            vandq_u16(near(p0, q0, alpha), near(p1, p0, beta)),
            vandq_u16(near(q1, q0, beta), vcgtq_s16(bs, zero)),
        );
        let strong_bs = vcgtq_s16(bs, vdupq_n_s16(3));
        let tc = vaddq_s16(tc0, vdupq_n_s16(1));
        let raw = vshrq_n_s16::<3>(vaddq_s16(
            vaddq_s16(vshlq_n_s16::<2>(vsubq_s16(q0, p0)), vsubq_s16(p1, q1)),
            vdupq_n_s16(4),
        ));
        let delta = vmaxq_s16(vminq_s16(raw, tc), vsubq_s16(zero, tc));
        let two = vdupq_n_s16(2);
        let weak_p0 = vaddq_s16(p0, delta);
        let weak_q0 = vsubq_s16(q0, delta);
        let long_p0 = vshrq_n_s16::<2>(vaddq_s16(
            vaddq_s16(vshlq_n_s16::<1>(p1), p0),
            vaddq_s16(q1, two),
        ));
        let long_q0 = vshrq_n_s16::<2>(vaddq_s16(
            vaddq_s16(vshlq_n_s16::<1>(q1), q0),
            vaddq_s16(p1, two),
        ));
        let clip = |v: int16x8_t| vmaxq_s16(vminq_s16(v, vdupq_n_s16(255)), zero);
        let new_p0 = vbslq_s16(strong_bs, long_p0, clip(weak_p0));
        let new_q0 = vbslq_s16(strong_bs, long_q0, clip(weak_q0));
        s[1] = vbslq_s16(filtered, new_p0, p0);
        s[2] = vbslq_s16(filtered, new_q0, q0);
    }
}

/// The boundary strength and threshold of each of eight lines, as lanes.
#[inline(always)]
unsafe fn per_line(params: &EdgeParams, segment: usize, first: usize) -> (int16x8_t, int16x8_t) {
    // SAFETY: the loads check their slices.
    unsafe {
        let mut strengths = [0i16; 8];
        let mut thresholds = [0i16; 8];
        for (lane, (b, t)) in strengths.iter_mut().zip(thresholds.iter_mut()).enumerate() {
            let s = (first + lane) / segment.max(1);
            *b = i16::from(params.bs.get(s).copied().unwrap_or(0));
            *t = i16::from(params.tc0_at(s));
        }
        (load_i16x8(&strengths), load_i16x8(&thresholds))
    }
}

/// The deblocking filter of one edge.
pub(super) unsafe fn filter_edge(mut edge: EdgeMut<'_>, params: &EdgeParams) {
    // SAFETY: every access goes through the helpers.
    unsafe {
        let lines = edge.lines();
        let segment = edge.segment();
        let chroma = edge.reach() == Reach::Chroma;
        let reach = edge.reach().samples();
        let (across, along) = (edge.across(), edge.along());
        // EdgeMut::new keeps the two directions orthogonal, which the
        // addressing below relies on.
        debug_assert!(across == 1 || along == 1);
        let alpha = vdupq_n_s16(i16::from(params.alpha));
        let beta = vdupq_n_s16(i16::from(params.beta));
        let (data, q0) = edge.window();
        let mut first = 0;
        while first < lines {
            let count = (lines - first).min(8);
            let (bs, tc0) = per_line(params, segment, first);
            let mut lanes = [vdupq_n_s16(0); 8];
            if along == 1 {
                for (j, slot) in lanes.iter_mut().enumerate().take(2 * reach) {
                    let start = q0 + j * across - reach * across + first;
                    let row = data.get(start..start + count).unwrap_or_default();
                    *slot = widen(load8(row));
                }
            } else {
                let mut rows = [vdup_n_u8(0); 8];
                for (k, slot) in rows.iter_mut().enumerate().take(count) {
                    let start = q0 + (first + k) * along - reach;
                    let row = data.get(start..start + 2 * reach).unwrap_or_default();
                    *slot = load8(row);
                }
                let transposed = transpose8x8_u8(rows);
                for (j, slot) in lanes.iter_mut().enumerate().take(2 * reach) {
                    *slot = widen(transposed.get(j).copied().unwrap_or(vdup_n_u8(0)));
                }
            }
            if chroma {
                filter_chroma8(&mut lanes, alpha, beta, bs, tc0);
            } else {
                filter_luma8(&mut lanes, alpha, beta, bs, tc0);
            }
            if along == 1 {
                for (j, v) in lanes.iter().enumerate().take(2 * reach) {
                    let start = q0 + j * across - reach * across + first;
                    let row = data.get_mut(start..start + count).unwrap_or_default();
                    store8_bytes(row, vqmovun_s16(*v), count);
                }
            } else {
                let mut packed = [vdup_n_u8(0); 8];
                for (j, v) in packed.iter_mut().enumerate() {
                    *v = vqmovun_s16(lanes.get(j).copied().unwrap_or(vdupq_n_s16(0)));
                }
                let back = transpose8x8_u8(packed);
                for (k, v) in back.iter().enumerate().take(count) {
                    let start = q0 + (first + k) * along - reach;
                    let row = data.get_mut(start..start + 2 * reach).unwrap_or_default();
                    store8_bytes(row, *v, 2 * reach);
                }
            }
            first += 8;
        }
    }
}

/// [`crate::Kernels::idct4x4_add_quad`]: no vector kernel here yet, so the
/// reference's blocks one at a time.
pub(super) unsafe fn idct4x4_add_quad(dst: BlockMut<'_>, coeffs: &[[i32; 16]; 4], nz: u8) {
    crate::idct4x4_add_quad_by_blocks(&crate::Scalar, dst, coeffs, nz);
}
