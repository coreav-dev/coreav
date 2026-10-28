//! The reference kernels: safe Rust, written from the equations of Rec.
//! ITU-T H.264 (08/2024).
//!
//! Every other implementation is checked against these, bit for bit
//! (`docs/design/h264.md`, bar C3). They are also what the WebAssembly build
//! runs, so they stay reasonably efficient: fixed-size buffers, no
//! allocation, and iteration over exact row slices rather than per-sample
//! bounds checks.

use crate::views::{Block, BlockMut, EdgeMut, Reach};
use crate::{Combine, EdgeParams, Kernels};

/// The reference implementation of every kernel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scalar;

/// Clip1Y / Clip1C for 8-bit samples (clause 5.7).
#[inline(always)]
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The 6-tap filter of Equations 8-241 and 8-242: (1, -5, 20, 20, -5, 1).
#[inline(always)]
fn tap6(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i32 {
    a - 5 * b + 20 * c + 20 * d - 5 * e + f
}

/// One line of the inverse 4x4 transform (Equations 8-338 to 8-341).
#[inline(always)]
fn idct4(x: [i32; 4]) -> [i32; 4] {
    let [x0, x1, x2, x3] = x;
    let e0 = x0 + x2;
    let e1 = x0 - x2;
    let e2 = (x1 >> 1) - x3;
    let e3 = x1 + (x3 >> 1);
    [e0 + e3, e1 + e2, e1 - e2, e0 - e3]
}

/// One line of the inverse 8x8 transform (Equations 8-351 to 8-366).
#[inline(always)]
fn idct8(x: [i32; 8]) -> [i32; 8] {
    let [x0, x1, x2, x3, x4, x5, x6, x7] = x;
    let e0 = x0 + x4;
    let e1 = -x3 + x5 - x7 - (x7 >> 1);
    let e2 = x0 - x4;
    let e3 = x1 + x7 - x3 - (x3 >> 1);
    let e4 = (x2 >> 1) - x6;
    let e5 = -x1 + x7 + x5 + (x5 >> 1);
    let e6 = x2 + (x6 >> 1);
    let e7 = x3 + x5 + x1 + (x1 >> 1);
    let f0 = e0 + e6;
    let f1 = e1 + (e7 >> 2);
    let f2 = e2 + e4;
    let f3 = e3 + (e5 >> 2);
    let f4 = e2 - e4;
    let f5 = (e3 >> 2) - e5;
    let f6 = e0 - e6;
    let f7 = e7 - (e1 >> 2);
    [
        f0 + f7,
        f2 + f5,
        f4 + f3,
        f6 + f1,
        f6 - f1,
        f4 - f3,
        f2 - f5,
        f0 - f7,
    ]
}

/// Adds the residual of one row to the prediction, with clipping
/// (Equation 8-369).
#[inline(always)]
fn add_residual(dst: &mut [u8], residual: impl Iterator<Item = i32>) {
    for (p, r) in dst.iter_mut().zip(residual) {
        *p = clip1(i32::from(*p) + ((r + 32) >> 6));
    }
}

/// Where one of the two samples an interpolated sample averages comes from
/// (Figure 8-4 and Equations 8-243 to 8-258).
///
/// Shared with the SIMD kernels, so that both pick the same samples for a
/// fractional position.
#[derive(Clone, Copy)]
pub(crate) enum Tap {
    /// The integer sample G, shifted by `dx` columns and `dy` rows.
    Int { dx: usize, dy: usize },
    /// The half-sample b, shifted by `dy` rows.
    B { dy: usize },
    /// The half-sample h, shifted by `dx` columns.
    H { dx: usize },
    /// The half-sample j.
    J,
}

/// The one or two samples the quarter-sample position `(xf, yf)` averages
/// (Table 8-12 read as Figure 8-4's sample names).
pub(crate) const fn taps(xf: u8, yf: u8) -> (Tap, Option<Tap>) {
    // A `const fn`, so that the x86 kernels take a position's taps as
    // constants.
    const fn int(dx: usize, dy: usize) -> Tap {
        Tap::Int { dx, dy }
    }
    match (xf, yf) {
        (0, 0) => (int(0, 0), None),
        (2, 0) => (Tap::B { dy: 0 }, None),
        (0, 2) => (Tap::H { dx: 0 }, None),
        (2, 2) => (Tap::J, None),
        // a and c.
        (1, 0) => (int(0, 0), Some(Tap::B { dy: 0 })),
        (3, 0) => (int(1, 0), Some(Tap::B { dy: 0 })),
        // d and n.
        (0, 1) => (int(0, 0), Some(Tap::H { dx: 0 })),
        (0, 3) => (int(0, 1), Some(Tap::H { dx: 0 })),
        // e, g, p and r.
        (1, 1) => (Tap::B { dy: 0 }, Some(Tap::H { dx: 0 })),
        (3, 1) => (Tap::B { dy: 0 }, Some(Tap::H { dx: 1 })),
        (1, 3) => (Tap::B { dy: 1 }, Some(Tap::H { dx: 0 })),
        (3, 3) => (Tap::B { dy: 1 }, Some(Tap::H { dx: 1 })),
        // f, i, k and q.
        (2, 1) => (Tap::J, Some(Tap::B { dy: 0 })),
        (2, 3) => (Tap::J, Some(Tap::B { dy: 1 })),
        (1, 2) => (Tap::J, Some(Tap::H { dx: 0 })),
        _ => (Tap::J, Some(Tap::H { dx: 1 })),
    }
}

/// Largest block a kernel works on, in samples.
pub(crate) const MAX_BLOCK: usize = 16;
/// Rows of horizontal half-sample intermediates: a 16-row block needs rows
/// -2 to +3 around it.
pub(crate) const MAX_B1_ROWS: usize = MAX_BLOCK + 5;

/// The horizontal 6-tap intermediates b1 (Equation 8-241) of one source row,
/// for columns 0 to `out.len()`.
fn horizontal_taps(row: &[u8], out: &mut [i32]) {
    for (o, w) in out.iter_mut().zip(row.windows(6)) {
        let [a, b, c, d, e, f] = <[u8; 6]>::try_from(w).unwrap_or_default();
        *o = tap6(
            i32::from(a),
            i32::from(b),
            i32::from(c),
            i32::from(d),
            i32::from(e),
            i32::from(f),
        );
    }
}

/// The vertical 6-tap intermediates (Equation 8-242) over six rows.
fn vertical_taps(rows: &[&[u8]; 6], out: &mut [i32]) {
    let [r0, r1, r2, r3, r4, r5] = *rows;
    let pairs = r0.iter().zip(r1).zip(r2.iter().zip(r3));
    for ((o, ((a, b), (c, d))), (e, f)) in out.iter_mut().zip(pairs).zip(r4.iter().zip(r5)) {
        *o = tap6(
            i32::from(*a),
            i32::from(*b),
            i32::from(*c),
            i32::from(*d),
            i32::from(*e),
            i32::from(*f),
        );
    }
}

/// The same, over six rows of intermediates: j1 (Equation 8-246).
fn vertical_taps_i32(rows: &[&[i32]; 6], out: &mut [i32]) {
    let [r0, r1, r2, r3, r4, r5] = *rows;
    let pairs = r0.iter().zip(r1).zip(r2.iter().zip(r3));
    for ((o, ((a, b), (c, d))), (e, f)) in out.iter_mut().zip(pairs).zip(r4.iter().zip(r5)) {
        *o = tap6(*a, *b, *c, *d, *e, *f);
    }
}

impl Kernels for Scalar {
    fn idct4x4_add(&self, mut dst: BlockMut<'_>, coeffs: &[i32; 16]) {
        if dst.w() != 4 || dst.h() != 4 {
            return;
        }
        let mut rows = [[0i32; 4]; 4];
        for (out, inp) in rows.iter_mut().zip(coeffs.chunks_exact(4)) {
            *out = idct4(<[i32; 4]>::try_from(inp).unwrap_or_default());
        }
        let [r0, r1, r2, r3] = rows;
        let mut cols = [[0i32; 4]; 4];
        for (j, col) in cols.iter_mut().enumerate() {
            let pick = |r: [i32; 4]| r.get(j).copied().unwrap_or(0);
            *col = idct4([pick(r0), pick(r1), pick(r2), pick(r3)]);
        }
        // `cols[j]` holds column j of the result, top to bottom.
        for (y, row) in dst.rows_mut().enumerate() {
            add_residual(row, cols.iter().map(|c| c.get(y).copied().unwrap_or(0)));
        }
    }

    fn idct4x4_dc_add(&self, mut dst: BlockMut<'_>, dc: i32) {
        if dst.w() != 4 || dst.h() != 4 {
            return;
        }
        let v = (dc + 32) >> 6;
        for row in dst.rows_mut() {
            for p in row.iter_mut() {
                *p = clip1(i32::from(*p) + v);
            }
        }
    }

    fn idct8x8_add(&self, mut dst: BlockMut<'_>, coeffs: &[i32; 64]) {
        if dst.w() != 8 || dst.h() != 8 {
            return;
        }
        let mut rows = [[0i32; 8]; 8];
        for (out, inp) in rows.iter_mut().zip(coeffs.chunks_exact(8)) {
            *out = idct8(<[i32; 8]>::try_from(inp).unwrap_or_default());
        }
        let mut cols = [[0i32; 8]; 8];
        for (j, col) in cols.iter_mut().enumerate() {
            let mut x = [0i32; 8];
            for (v, row) in x.iter_mut().zip(rows.iter()) {
                *v = row.get(j).copied().unwrap_or(0);
            }
            *col = idct8(x);
        }
        for (y, row) in dst.rows_mut().enumerate() {
            add_residual(row, cols.iter().map(|c| c.get(y).copied().unwrap_or(0)));
        }
    }

    fn luma_mc(&self, mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        let (xf, yf) = (xf & 3, yf & 3);
        if src.w() != w + 5 || src.h() != h + 5 || w > MAX_BLOCK || h > MAX_BLOCK {
            return;
        }
        if (xf, yf) == (0, 0) {
            for (out, row) in dst.rows_mut().zip(src.rows_from(2)) {
                let from = row.get(2..2 + w).unwrap_or_default();
                out.iter_mut().zip(from).for_each(|(o, s)| *o = *s);
            }
            return;
        }
        let (first, second) = taps(xf, yf);
        let needs = |want: fn(&Tap) -> bool| want(&first) || second.as_ref().is_some_and(want);
        let needs_b = needs(|t| matches!(t, Tap::B { .. }));
        let needs_h = needs(|t| matches!(t, Tap::H { .. }));
        let needs_j = needs(|t| matches!(t, Tap::J));

        // b1 row r holds the horizontal taps of source row r, columns 0..w.
        let mut b1 = [0i32; MAX_B1_ROWS * MAX_BLOCK];
        if needs_b || needs_j {
            // j needs every row; b alone needs the block's rows and one more.
            let (skip, take) = if needs_j { (0, h + 5) } else { (2, h + 1) };
            let rows = b1.chunks_exact_mut(MAX_BLOCK).zip(src.rows());
            for (out, row) in rows.skip(skip).take(take) {
                horizontal_taps(row, out.get_mut(..w).unwrap_or_default());
            }
        }
        // hh row y holds the vertical taps of output row y, columns 0..=w.
        let mut hh = [0i32; MAX_BLOCK * (MAX_BLOCK + 1)];
        if needs_h {
            for (y, out) in hh.chunks_exact_mut(MAX_BLOCK + 1).take(h).enumerate() {
                let mut rows: [&[u8]; 6] = [&[]; 6];
                for (r, slot) in rows.iter_mut().enumerate() {
                    *slot = src
                        .row(y + r)
                        .and_then(|row| row.get(2..3 + w))
                        .unwrap_or_default();
                }
                vertical_taps(&rows, out.get_mut(..=w).unwrap_or_default());
            }
        }

        let mut a = [0i32; MAX_BLOCK];
        let mut b = [0i32; MAX_BLOCK];
        for (y, out) in dst.rows_mut().enumerate() {
            let fill = |slot: &mut [i32; MAX_BLOCK], tap: Tap| match tap {
                Tap::Int { dx, dy } => {
                    let row = src
                        .row(y + 2 + dy)
                        .and_then(|r| r.get(2 + dx..2 + dx + w))
                        .unwrap_or_default();
                    for (o, s) in slot.iter_mut().zip(row) {
                        *o = i32::from(*s);
                    }
                }
                Tap::B { dy } => {
                    let row = b1
                        .chunks_exact(MAX_BLOCK)
                        .nth(y + 2 + dy)
                        .unwrap_or_default();
                    for (o, v) in slot.iter_mut().zip(row) {
                        *o = i32::from(clip1((*v + 16) >> 5));
                    }
                }
                Tap::H { dx } => {
                    let row = hh
                        .chunks_exact(MAX_BLOCK + 1)
                        .nth(y)
                        .and_then(|r| r.get(dx..dx + w))
                        .unwrap_or_default();
                    for (o, v) in slot.iter_mut().zip(row) {
                        *o = i32::from(clip1((*v + 16) >> 5));
                    }
                }
                Tap::J => {
                    let mut rows: [&[i32]; 6] = [&[]; 6];
                    for (r, s) in rows.iter_mut().enumerate() {
                        *s = b1
                            .chunks_exact(MAX_BLOCK)
                            .nth(y + r)
                            .and_then(|row| row.get(..w))
                            .unwrap_or_default();
                    }
                    let mut j1 = [0i32; MAX_BLOCK];
                    vertical_taps_i32(&rows, j1.get_mut(..w).unwrap_or_default());
                    for (o, v) in slot.iter_mut().zip(j1.iter()) {
                        *o = i32::from(clip1((*v + 512) >> 10));
                    }
                }
            };
            fill(&mut a, first);
            match second {
                None => {
                    for (o, v) in out.iter_mut().zip(a.iter()) {
                        *o = *v as u8;
                    }
                }
                Some(t) => {
                    fill(&mut b, t);
                    for ((o, p), q) in out.iter_mut().zip(a.iter()).zip(b.iter()) {
                        *o = ((p + q + 1) >> 1) as u8;
                    }
                }
            }
        }
    }

    fn chroma_mc(&self, mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        chroma_mc_with(&mut dst, &src, xf, yf, |o, v| *o = v);
    }

    fn combine(&self, mut dst: BlockMut<'_>, p0: Block<'_>, p1: Block<'_>, mode: Combine) {
        let (w, h) = (dst.w(), dst.h());
        if p0.w() < w || p0.h() < h || p1.w() < w || p1.h() < h {
            return;
        }
        // luma_log2_weight_denom and chroma_log2_weight_denom are 0 to 7
        // (clause 7.4.3.2); a shift outside that is not a weighting this
        // kernel computes.
        let denom = match mode {
            Combine::Uni { log_wd, .. } | Combine::Bi { log_wd, .. } => log_wd,
            _ => 0,
        };
        if !(0..=7).contains(&denom) {
            return;
        }
        for ((out, a), b) in dst.rows_mut().zip(p0.rows()).zip(p1.rows()) {
            match mode {
                Combine::Copy(list) => {
                    let from = if list == 0 { a } else { b };
                    out.iter_mut().zip(from).for_each(|(o, s)| *o = *s);
                }
                // Equation 8-284.
                Combine::Average => {
                    for ((o, x), y) in out.iter_mut().zip(a).zip(b) {
                        *o = ((u16::from(*x) + u16::from(*y) + 1) >> 1) as u8;
                    }
                }
                // Equations 8-286 and 8-287.
                Combine::Uni { list, log_wd, w, o } => {
                    let from = if list == 0 { a } else { b };
                    for (out, s) in out.iter_mut().zip(from) {
                        let p = i32::from(*s);
                        let v = if log_wd >= 1 {
                            ((p * w + (1 << (log_wd - 1))) >> log_wd) + o
                        } else {
                            p * w + o
                        };
                        *out = clip1(v);
                    }
                }
                // Equation 8-288.
                Combine::Bi {
                    log_wd,
                    w0,
                    w1,
                    o: off,
                } => {
                    for ((out, x), y) in out.iter_mut().zip(a).zip(b) {
                        let v = (i32::from(*x) * w0 + i32::from(*y) * w1 + (1 << log_wd))
                            >> (log_wd + 1);
                        *out = clip1(v + off);
                    }
                }
            }
        }
    }

    fn filter_edge(&self, mut edge: EdgeMut<'_>, params: &EdgeParams) {
        let segment = edge.segment();
        let chroma = edge.reach() == Reach::Chroma;
        let (alpha, beta) = (i32::from(params.alpha), i32::from(params.beta));
        for k in 0..edge.lines() {
            let seg = k / segment;
            let bs = params.bs.get(seg).copied().unwrap_or(0);
            if bs == 0 {
                continue;
            }
            let (p0, q0) = (i32::from(edge.get(k, -1)), i32::from(edge.get(k, 0)));
            let (p1, q1) = (i32::from(edge.get(k, -2)), i32::from(edge.get(k, 1)));
            // The filterSamplesFlag of Equation 8-468.
            if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
                continue;
            }
            let tc0 = i32::from(params.tc0_at(seg));
            if chroma {
                if bs < 4 {
                    // Equations 8-475 to 8-477 with chromaStyleFiltering.
                    let tc = tc0 + 1;
                    let delta = ((((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
                    edge.set(k, -1, clip1(p0 + delta));
                    edge.set(k, 0, clip1(q0 - delta));
                } else {
                    // Equations 8-484 and 8-488.
                    edge.set(k, -1, ((2 * p1 + p0 + q1 + 2) >> 2) as u8);
                    edge.set(k, 0, ((2 * q1 + q0 + p1 + 2) >> 2) as u8);
                }
                continue;
            }
            let (p2, q2) = (i32::from(edge.get(k, -3)), i32::from(edge.get(k, 2)));
            let ap = (p2 - p0).abs();
            let aq = (q2 - q0).abs();
            if bs < 4 {
                // Clause 8.7.2.3: Equations 8-473 to 8-480.
                let tc = tc0 + i32::from(ap < beta) + i32::from(aq < beta);
                let delta = ((((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
                edge.set(k, -1, clip1(p0 + delta));
                edge.set(k, 0, clip1(q0 - delta));
                if ap < beta {
                    let d = ((p2 + ((p0 + q0 + 1) >> 1) - (p1 << 1)) >> 1).clamp(-tc0, tc0);
                    edge.set(k, -2, (p1 + d) as u8);
                }
                if aq < beta {
                    let d = ((q2 + ((p0 + q0 + 1) >> 1) - (q1 << 1)) >> 1).clamp(-tc0, tc0);
                    edge.set(k, 1, (q1 + d) as u8);
                }
            } else {
                // Clause 8.7.2.4: Equations 8-481 to 8-490.
                let strong = (p0 - q0).abs() < ((alpha >> 2) + 2);
                if ap < beta && strong {
                    let p3 = i32::from(edge.get(k, -4));
                    edge.set(k, -1, ((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3) as u8);
                    edge.set(k, -2, ((p2 + p1 + p0 + q0 + 2) >> 2) as u8);
                    edge.set(k, -3, ((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3) as u8);
                } else {
                    edge.set(k, -1, ((2 * p1 + p0 + q1 + 2) >> 2) as u8);
                }
                if aq < beta && strong {
                    let q3 = i32::from(edge.get(k, 3));
                    edge.set(k, 0, ((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3) as u8);
                    edge.set(k, 1, ((p0 + q0 + q1 + q2 + 2) >> 2) as u8);
                    edge.set(k, 2, ((2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3) as u8);
                } else {
                    edge.set(k, 0, ((2 * q1 + q0 + p1 + 2) >> 2) as u8);
                }
            }
        }
    }
}

/// Chroma eighth-sample interpolation (Equation 8-266) of `dst` from
/// `src`, each predicted sample handed to `store` with the sample of `dst`
/// it is for: the reference's `chroma_mc` stores it, and the averaging
/// dispatch averages it in. A source of another shape than the
/// `(w + 1)` by `(h + 1)` window leaves `dst` unchanged.
#[inline(always)]
pub(crate) fn chroma_mc_with(
    dst: &mut BlockMut<'_>,
    src: &Block<'_>,
    xf: u8,
    yf: u8,
    store: impl Fn(&mut u8, u8),
) {
    let (w, h) = (dst.w(), dst.h());
    let (xf, yf) = (u32::from(xf & 7), u32::from(yf & 7));
    if src.w() != w + 1 || src.h() != h + 1 {
        return;
    }
    // Equation 8-266.
    let (wa, wb) = ((8 - xf) * (8 - yf), xf * (8 - yf));
    let (wc, wd) = ((8 - xf) * yf, xf * yf);
    let mut upper = src.rows();
    let mut lower = src.rows_from(1);
    for out in dst.rows_mut() {
        let (Some(up), Some(down)) = (upper.next(), lower.next()) else {
            return;
        };
        let a = up.iter().zip(up.iter().skip(1));
        let b = down.iter().zip(down.iter().skip(1));
        for ((o, (ul, ur)), (ll, lr)) in out.iter_mut().zip(a).zip(b) {
            let v = wa * u32::from(*ul)
                + wb * u32::from(*ur)
                + wc * u32::from(*ll)
                + wd * u32::from(*lr);
            store(o, ((v + 32) >> 6) as u8);
        }
    }
}
