//! Intra prediction (clauses 8.3.1.2, 8.3.2.2, 8.3.3 and 8.3.4): the
//! reference, which [`crate::Scalar`] runs and every vector kernel is
//! compared with.
//!
//! Each prediction reads the neighbouring samples of its block out of the
//! view ([`IntraMut`]) first, then writes the block, so that no prediction
//! reads a sample it has just written. The macroblock-level calls predict
//! the blocks of an Intra_4x4 or Intra_8x8 macroblock in decoding order and
//! add each block's residual before the next block reads it.

use crate::Kernels;
use crate::views::{IntraAvail, IntraMut};

/// Clip1Y / Clip1C for 8-bit samples.
#[inline]
fn clip1(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The neighbouring samples of a block: p[x, -1] for x up to 2n, p[-1, y]
/// for y up to n, and p[-1, -1].
struct Neighbours {
    top: [i32; 16],
    left: [i32; 16],
    top_left: i32,
    avail: IntraAvail,
}

impl Neighbours {
    /// p(x, y) of the prediction equations, with x and y at least -1.
    #[inline]
    fn at(&self, x: i32, y: i32) -> i32 {
        if y < 0 {
            if x < 0 {
                self.top_left
            } else {
                self.top.get(x as usize).copied().unwrap_or(0)
            }
        } else {
            self.left.get(y as usize).copied().unwrap_or(0)
        }
    }

    /// The sum of the `n` samples above the block.
    fn top_sum(&self, n: usize) -> i32 {
        self.top.iter().take(n).sum()
    }

    /// The sum of the `n` samples left of the block.
    fn left_sum(&self, n: usize) -> i32 {
        self.left.iter().take(n).sum()
    }
}

/// Reads the neighbouring samples of the view's block: the first `top`
/// samples above it (`n` or `2n`, as the modes of the block size read
/// them), the `n` left of it, and the one above and left.
///
/// The `n` samples above the block are followed by `n` more where the view
/// has them (`top_right`); where it does not, the last sample above the
/// block is repeated, as clause 8.3.1.2.3 and its neighbours do.
///
/// Inlined: returned from a call, the record is copied by `memcpy`.
#[inline(always)]
fn neighbours(v: &IntraMut<'_>, top: usize) -> Neighbours {
    let n = v.n();
    let mut nb = Neighbours {
        top: [0; 16],
        left: [0; 16],
        top_left: i32::from(v.top_left()),
        avail: v.avail(),
    };
    let row = v.top();
    if let Some(&last) = row.get(n.wrapping_sub(1)) {
        let top = top.min(nb.top.len());
        for (x, slot) in nb.top.iter_mut().enumerate().take(top) {
            *slot = i32::from(row.get(x).copied().unwrap_or(last));
        }
    }
    if nb.avail.left {
        for (y, slot) in nb.left.iter_mut().enumerate().take(n) {
            *slot = i32::from(v.left(y));
        }
    }
    nb
}

/// Writes an `n` by `n` prediction, taken row by row from `block`, into the
/// view's block.
fn write_block(v: &mut IntraMut<'_>, block: &[u8]) {
    let n = v.n();
    if let Some(mut dst) = v.block_mut() {
        for (out, row) in dst.rows_mut().zip(block.chunks_exact(n)) {
            out.copy_from_slice(row);
        }
    }
}

/// Fills each row of the `n`-wide `block` with the copy of `top`: the
/// vertical modes.
fn copy_rows(block: &mut [u8], top: &[i32; 16], n: usize) {
    for row in block.chunks_exact_mut(n) {
        for (out, t) in row.iter_mut().zip(top) {
            *out = *t as u8;
        }
    }
}

/// Fills row `y` of the `n`-wide `block` with `left[y]`: the horizontal
/// modes.
fn fill_rows(block: &mut [u8], left: &[i32; 16], n: usize) {
    for (row, l) in block.chunks_exact_mut(n).zip(left) {
        row.fill(*l as u8);
    }
}

/// The DC prediction of an `n` by `n` block (Equations 8-47 to 8-50 and
/// their 8x8 and 16x16 counterparts).
fn dc(nb: &Neighbours, n: usize) -> i32 {
    let shift = n.trailing_zeros();
    match (nb.avail.top, nb.avail.left) {
        (true, true) => (nb.top_sum(n) + nb.left_sum(n) + n as i32) >> (shift + 1),
        (true, false) => (nb.top_sum(n) + (n as i32 >> 1)) >> shift,
        (false, true) => (nb.left_sum(n) + (n as i32 >> 1)) >> shift,
        (false, false) => 128,
    }
}

/// Intra_4x4 prediction of a 4x4 view (clause 8.3.1.2). `mode` is
/// Intra4x4PredMode; above 8 it predicts DC. A view of another size is
/// left unchanged.
pub(crate) fn pred4x4(v: &mut IntraMut<'_>, mode: u8) {
    if v.n() != 4 {
        return;
    }
    let nb = neighbours(v, 8);
    let mut block = [0u8; 16];
    match mode {
        0 => copy_rows(&mut block, &nb.top, 4),
        1 => fill_rows(&mut block, &nb.left, 4),
        3..=8 => {
            let table = SOURCES_4X4.get(usize::from(mode)).unwrap_or(&[0; 16]);
            directional::<4>(&nb, table, &mut block);
        }
        _ => block.fill(dc(&nb, 4) as u8),
    }
    write_block(v, &block);
}

/// The length of the edge line of an `n` by `n` block: `n` samples left of
/// it, the one above and left, and `2n` above.
pub(crate) const fn line_len(n: usize) -> usize {
    3 * n + 1
}

/// Where sample (x, y) of an `n` by `n` block predicted in directional
/// mode `mode` (3 to 8) comes from: an index into the buffer
/// [`directional`] builds.
///
/// Every directional sample of clauses 8.3.1.2.4 to 8.3.1.2.9 and 8.3.2.2.5
/// to 8.3.2.2.10 is one of three things about the block's edge line, the
/// neighbouring samples in order from p[-1, n-1] up the left column to
/// p[-1, -1] and along the row above to p[2n-1, -1]: a sample of the line,
/// the rounded average of two neighbours on it, or the 3-tap filter
/// (1, 2, 1) centred on one of its samples. The two equations that weight
/// an end sample by 3 (the last sample of diagonal down-left and the one
/// before horizontal-up's copies) are the 3-tap filter with the line's end
/// repeated. So each sample is an index fixed by its mode and position,
/// worked out here when the crate is compiled.
const fn source(n: i32, mode: u8, x: i32, y: i32) -> u8 {
    // The position of p[px, py] on the line.
    const fn pos(n: i32, px: i32, py: i32) -> i32 {
        if py >= 0 {
            n - 1 - py
        } else if px < 0 {
            n
        } else {
            n + 1 + px
        }
    }
    let len = 3 * n + 1;
    // The buffer: the line, then its 2-tap averages (between k and k + 1),
    // then its 3-tap filter (centred on k).
    let pair = len;
    let tap3 = 2 * len - 1;
    let i = match mode {
        // Diagonal down-left (Equations 8-51, 8-52, 8-80 and 8-81).
        3 => tap3 + pos(n, x + y + 1, -1),
        // Diagonal down-right (Equations 8-53 to 8-55 and 8-82 to 8-84).
        4 => tap3 + n + x - y,
        // Vertical-right (Equations 8-56 to 8-58 and 8-85 to 8-87).
        5 => {
            let z = 2 * x - y;
            if z >= 0 && z % 2 == 0 {
                pair + pos(n, x - (y >> 1) - 1, -1)
            } else if z >= 0 {
                tap3 + pos(n, x - (y >> 1) - 1, -1)
            } else if z == -1 {
                tap3 + n
            } else {
                tap3 + pos(n, -1, y - 2 * x - 2)
            }
        }
        // Horizontal-down (Equations 8-59 to 8-61 and 8-88 to 8-90).
        6 => {
            let z = 2 * y - x;
            if z >= 0 && z % 2 == 0 {
                pair + pos(n, -1, y - (x >> 1))
            } else if z >= 0 {
                tap3 + pos(n, -1, y - (x >> 1) - 1)
            } else if z == -1 {
                tap3 + n
            } else {
                tap3 + pos(n, x - 2 * y - 2, -1)
            }
        }
        // Vertical-left (Equations 8-62, 8-63, 8-91 and 8-92).
        7 => {
            if y % 2 == 0 {
                pair + pos(n, x + (y >> 1), -1)
            } else {
                tap3 + pos(n, x + (y >> 1) + 1, -1)
            }
        }
        // Horizontal-up (Equations 8-64 to 8-66 and 8-93 to 8-95).
        _ => {
            let z = x + 2 * y;
            let last = 2 * n - 4;
            if z <= last && z % 2 == 0 {
                pair + pos(n, -1, y + (x >> 1) + 1)
            } else if z < last + 1 {
                tap3 + pos(n, -1, y + (x >> 1) + 1)
            } else if z == last + 1 {
                tap3 + pos(n, -1, n - 1)
            } else {
                pos(n, -1, n - 1)
            }
        }
    };
    i as u8
}

/// [`source`] for every sample of an `N` by `N` block, `S` = `N * N`, and
/// every mode below 9; modes 0 to 2 are not directional and are left at 0.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
pub(crate) const fn sources<const N: usize, const S: usize>() -> [[u8; S]; 9] {
    let mut out = [[0u8; S]; 9];
    let mut mode = 3;
    while mode < 9 {
        let mut i = 0;
        while i < S {
            out[mode][i] = source(N as i32, mode as u8, (i % N) as i32, (i / N) as i32);
            i += 1;
        }
        mode += 1;
    }
    out
}

static SOURCES_4X4: [[u8; 16]; 9] = sources::<4, 16>();
static SOURCES_8X8: [[u8; 64]; 9] = sources::<8, 64>();

/// A directional prediction of an `N` by `N` block from its neighbours
/// `nb`, each sample taken from the buffer of the edge line, its 2-tap
/// averages and its 3-tap filter at the index `table` gives it
/// ([`source`]).
fn directional<const N: usize>(nb: &Neighbours, table: &[u8], block: &mut [u8]) {
    let len = line_len(N);
    // The line with its ends repeated once more at each end, for the
    // 3-tap filter of the end samples. At most 8 + 1 + 16 samples.
    let mut line = [0i32; line_len(8) + 2];
    let left = nb.left.iter().take(N).rev();
    let top = nb.top.iter().take(2 * N);
    let edge = left.chain(core::iter::once(&nb.top_left)).chain(top);
    for (slot, v) in line.iter_mut().skip(1).zip(edge) {
        *slot = *v;
    }
    let first = line.get(1).copied().unwrap_or(0);
    let last = line.get(len).copied().unwrap_or(0);
    if let Some(slot) = line.first_mut() {
        *slot = first;
    }
    if let Some(slot) = line.get_mut(len + 1) {
        *slot = last;
    }
    let line = line.get(..len + 2).unwrap_or(&[]);
    // Indices are u8, so a buffer of 256 needs no check to read.
    let mut buf = [0u8; 256];
    let (samples, rest) = buf.split_at_mut(len);
    let (pairs, taps) = rest.split_at_mut(len - 1);
    for (out, v) in samples.iter_mut().zip(line.iter().skip(1)) {
        *out = *v as u8;
    }
    for (out, w) in pairs.iter_mut().zip(line.windows(2).skip(1)) {
        if let [a, b] = w {
            *out = ((a + b + 1) >> 1) as u8;
        }
    }
    for (out, w) in taps.iter_mut().zip(line.windows(3)).take(len) {
        if let [a, b, c] = w {
            *out = ((a + 2 * b + c + 2) >> 2) as u8;
        }
    }
    for (out, &i) in block.iter_mut().zip(table) {
        *out = buf.get(usize::from(i)).copied().unwrap_or(0);
    }
}

/// Filters the neighbouring samples of an Intra_8x8 block
/// (clause 8.3.2.2.1).
fn filter8x8(nb: &Neighbours) -> Neighbours {
    let mut out = Neighbours {
        top: [0; 16],
        left: [0; 16],
        top_left: nb.top_left,
        avail: nb.avail,
    };
    let t = |x: i32| nb.at(x, -1);
    let l = |y: i32| nb.at(-1, y);
    if nb.avail.top {
        for (x, slot) in out.top.iter_mut().enumerate() {
            let x = x as i32;
            *slot = if x == 0 {
                if nb.avail.top_left {
                    (nb.top_left + 2 * t(0) + t(1) + 2) >> 2
                } else {
                    (3 * t(0) + t(1) + 2) >> 2
                }
            } else if x == 15 {
                (t(14) + 3 * t(15) + 2) >> 2
            } else {
                (t(x - 1) + 2 * t(x) + t(x + 1) + 2) >> 2
            };
        }
    }
    if nb.avail.top_left {
        out.top_left = match (nb.avail.top, nb.avail.left) {
            (true, true) => (t(0) + 2 * nb.top_left + l(0) + 2) >> 2,
            (true, false) => (3 * nb.top_left + t(0) + 2) >> 2,
            (false, true) => (3 * nb.top_left + l(0) + 2) >> 2,
            (false, false) => nb.top_left,
        };
    }
    if nb.avail.left {
        for (y, slot) in out.left.iter_mut().enumerate().take(8) {
            let y = y as i32;
            *slot = if y == 0 {
                if nb.avail.top_left {
                    (nb.top_left + 2 * l(0) + l(1) + 2) >> 2
                } else {
                    (3 * l(0) + l(1) + 2) >> 2
                }
            } else if y == 7 {
                (l(6) + 3 * l(7) + 2) >> 2
            } else {
                (l(y - 1) + 2 * l(y) + l(y + 1) + 2) >> 2
            };
        }
    }
    out
}

/// Intra_8x8 prediction of an 8x8 view (clause 8.3.2.2), from the filtered
/// neighbours. `mode` is Intra8x8PredMode; above 8 it predicts DC. A view
/// of another size is left unchanged.
pub(crate) fn pred8x8(v: &mut IntraMut<'_>, mode: u8) {
    if v.n() != 8 {
        return;
    }
    let nb = filter8x8(&neighbours(v, 16));
    let mut block = [0u8; 64];
    match mode {
        0 => copy_rows(&mut block, &nb.top, 8),
        1 => fill_rows(&mut block, &nb.left, 8),
        3..=8 => {
            let table = SOURCES_8X8.get(usize::from(mode)).unwrap_or(&[0; 64]);
            directional::<8>(&nb, table, &mut block);
        }
        _ => block.fill(dc(&nb, 8) as u8),
    }
    write_block(v, &block);
}

/// The plane prediction mode of Intra_16x16 and of chroma (Equations 8-105
/// to 8-109 and 8-141 to 8-145), over an `n` by `n` block.
fn plane_prediction(nb: &Neighbours, n: usize, block: &mut [u8]) {
    let half = n as i32 / 2;
    let taps = n as i32 / 2;
    let (mut h, mut v) = (0i32, 0i32);
    for i in 0..taps {
        h += (i + 1) * (nb.at(half + i, -1) - nb.at(half - 2 - i, -1));
        v += (i + 1) * (nb.at(-1, half + i) - nb.at(-1, half - 2 - i));
    }
    let a = 16 * (nb.at(-1, n as i32 - 1) + nb.at(n as i32 - 1, -1));
    // The multiplier is (5 * H + 32) >> 6 for 16x16 and (34 * H + 32) >> 6
    // for an 8x8 chroma block.
    let scale = if n == 16 { 5 } else { 34 };
    let b = (scale * h + 32) >> 6;
    let c = (scale * v + 32) >> 6;
    for (i, out) in block.iter_mut().enumerate().take(n * n) {
        let (x, y) = ((i % n) as i32, (i / n) as i32);
        *out = clip1((a + b * (x - half + 1) + c * (y - half + 1) + 16) >> 5);
    }
}

/// Intra_16x16 prediction of a 16x16 view (clause 8.3.3). `mode` is
/// Intra16x16PredMode: 0 vertical, 1 horizontal, 2 DC, 3 plane; above 3 it
/// predicts DC. A view of another size is left unchanged.
pub(crate) fn pred16x16(v: &mut IntraMut<'_>, mode: u8) {
    if v.n() != 16 {
        return;
    }
    let nb = neighbours(v, 16);
    let mut block = [0u8; 256];
    match mode {
        0 => copy_rows(&mut block, &nb.top, 16),
        1 => fill_rows(&mut block, &nb.left, 16),
        3 => plane_prediction(&nb, 16, &mut block),
        _ => block.fill(dc(&nb, 16) as u8),
    }
    write_block(v, &block);
}

/// Intra chroma prediction of one 8x8 block of a 4:2:0 picture
/// (clause 8.3.4). `mode` is intra_chroma_pred_mode: 0 DC, 1 horizontal,
/// 2 vertical, 3 plane; above 3 it predicts DC. A view of another size is
/// left unchanged.
pub(crate) fn pred_chroma(v: &mut IntraMut<'_>, mode: u8) {
    if v.n() != 8 {
        return;
    }
    let nb = neighbours(v, 8);
    let mut block = [0u8; 64];
    match mode {
        1 => fill_rows(&mut block, &nb.left, 8),
        2 => copy_rows(&mut block, &nb.top, 8),
        3 => plane_prediction(&nb, 8, &mut block),
        // DC, over the four 4x4 blocks with their own rules
        // (clause 8.3.4.1): the four sums once, then each block filled.
        _ => {
            let sum =
                |side: &[i32; 16], from: usize| -> i32 { side.iter().skip(from).take(4).sum() };
            let top = [sum(&nb.top, 0), sum(&nb.top, 4)];
            let left = [sum(&nb.left, 0), sum(&nb.left, 4)];
            for (y, row) in block.chunks_exact_mut(8).enumerate() {
                let by = y / 4;
                for (bx, quarter) in row.chunks_exact_mut(4).enumerate() {
                    let (t, l) = (
                        top.get(bx).copied().unwrap_or(0),
                        left.get(by).copied().unwrap_or(0),
                    );
                    // Blocks on the diagonal average both sides; the others
                    // prefer the side they touch.
                    let v = match (nb.avail.top, nb.avail.left) {
                        (true, true) if bx == by => (t + l + 4) >> 3,
                        (true, true) if bx > by => (t + 2) >> 2,
                        (true, true) => (l + 2) >> 2,
                        (true, false) => (t + 2) >> 2,
                        (false, true) => (l + 2) >> 2,
                        (false, false) => 128,
                    };
                    quarter.fill(v as u8);
                }
            }
        }
    }
    write_block(v, &block);
}

/// The position (x, y), in samples, of luma4x4BlkIdx `blk` inside its
/// macroblock (clause 6.4.3).
pub(crate) const fn blk_xy(blk: usize) -> (usize, usize) {
    let (b8, b4) = (blk / 4 % 4, blk % 4);
    ((b8 % 2) * 8 + (b4 % 2) * 4, (b8 / 2) * 8 + (b4 / 2) * 4)
}

/// luma4x4BlkIdx of the 4x4 block at raster position `r` (row by row) of a
/// macroblock.
const BLK_OF_RASTER: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

/// Which neighbours the `n` by `n` block at (x, y) of a macroblock may
/// predict from, given those of the macroblock, `mb` (A left, B top,
/// C top right, D top left).
///
/// `order` is the block's index in decoding order (luma4x4BlkIdx, or
/// luma8x8BlkIdx for an 8x8 block). Inside the macroblock, a block left of,
/// above or above and left of the current one is always decoded before it;
/// the one above and right only sometimes, which its order decides. Right
/// of the macroblock nothing is decoded yet.
pub(crate) fn block_avail(
    mb: IntraAvail,
    x: usize,
    y: usize,
    n: usize,
    order: usize,
) -> IntraAvail {
    let top_right = if y == 0 {
        if x + n < 16 { mb.top } else { mb.top_right }
    } else if x + n < 16 {
        let r = ((y - 1) / 4) * 4 + (x + n) / 4;
        let other = BLK_OF_RASTER.get(r).copied().unwrap_or(usize::MAX);
        let other = if n == 8 { other / 4 } else { other };
        other < order
    } else {
        false
    };
    IntraAvail {
        left: x > 0 || mb.left,
        top: y > 0 || mb.top,
        top_right,
        top_left: match (x > 0, y > 0) {
            (true, true) => true,
            (false, true) => mb.left,
            (true, false) => mb.top,
            (false, false) => mb.top_left,
        },
    }
}

/// [`Kernels::intra4x4_mb`]: the trait's reference semantics, each block
/// predicted by [`pred4x4`] and its residual added by `k.idct4x4_add`.
pub(crate) fn intra4x4_mb<K: Kernels + ?Sized>(
    k: &K,
    mut dst: IntraMut<'_>,
    modes: &[u8; 16],
    coeffs: &[[i32; 16]; 16],
    nz: u16,
) {
    if dst.n() != 16 {
        return;
    }
    let mb = dst.avail();
    for (blk, (mode, coeffs)) in modes.iter().zip(coeffs).enumerate() {
        let (x, y) = blk_xy(blk);
        let Some(mut v) = dst.sub(x, y, 4, block_avail(mb, x, y, 4, blk)) else {
            continue;
        };
        pred4x4(&mut v, *mode);
        if nz & (1 << blk) != 0 {
            if let Some(b) = v.block_mut() {
                k.idct4x4_add(b, coeffs);
            }
        }
    }
}

/// [`Kernels::intra8x8_mb`]: the trait's reference semantics, each block
/// predicted by [`pred8x8`] and its residual added by `k.idct8x8_add`.
pub(crate) fn intra8x8_mb<K: Kernels + ?Sized>(
    k: &K,
    mut dst: IntraMut<'_>,
    modes: &[u8; 4],
    coeffs: &[[i32; 64]; 4],
    nz: u8,
) {
    if dst.n() != 16 {
        return;
    }
    let mb = dst.avail();
    for (b8, (mode, coeffs)) in modes.iter().zip(coeffs).enumerate() {
        let (x, y) = ((b8 % 2) * 8, (b8 / 2) * 8);
        let Some(mut v) = dst.sub(x, y, 8, block_avail(mb, x, y, 8, b8)) else {
            continue;
        };
        pred8x8(&mut v, *mode);
        if nz & (1 << b8) != 0 {
            if let Some(b) = v.block_mut() {
                k.idct8x8_add(b, coeffs);
            }
        }
    }
}

#[cfg(test)]
#[path = "intra_reference.rs"]
mod reference;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockMut, Scalar};

    const ALL: IntraAvail = IntraAvail {
        left: true,
        top: true,
        top_right: true,
        top_left: true,
    };

    const NONE: IntraAvail = IntraAvail {
        left: false,
        top: false,
        top_right: false,
        top_left: false,
    };

    fn avail_of(bits: u8) -> IntraAvail {
        IntraAvail {
            left: bits & 1 != 0,
            top: bits & 2 != 0,
            top_right: bits & 4 != 0,
            top_left: bits & 8 != 0,
        }
    }

    fn rng(seed: u32) -> impl FnMut() -> u8 {
        let mut seed = seed;
        move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        }
    }

    /// A 16-wide plane whose row 0 is 10, 11, ... and whose column 0 is
    /// 20, 21, ... so that vertical and horizontal predictions differ.
    fn plane() -> Vec<u8> {
        let mut p = vec![0u8; 16 * 16];
        for (i, s) in p.iter_mut().enumerate() {
            *s = match (i % 16, i / 16) {
                (x, 0) => 10 + x as u8,
                (0, y) => 20 + y as u8,
                _ => 0,
            };
        }
        p
    }

    fn view(p: &mut [u8], stride: usize, off: usize, n: usize, avail: IntraAvail) -> IntraMut<'_> {
        IntraMut::new(p, off, stride, n, avail).expect("view")
    }

    #[test]
    fn vertical_and_horizontal_copy_the_edges() {
        let mut p = plane();
        // A 4x4 block at (1, 1) of the plane, so row 0 is above it and
        // column 0 is left of it.
        pred4x4(&mut view(&mut p, 16, 17, 4, ALL), 0);
        assert_eq!(p.get(2 * 16 + 1), Some(&11));
        assert_eq!(p.get(16 * 4 + 1), Some(&11));
        let mut p = plane();
        pred4x4(&mut view(&mut p, 16, 17, 4, ALL), 1);
        assert_eq!(p.get(17), Some(&21));
        assert_eq!(p.get(16 * 4 + 1), Some(&24));
    }

    #[test]
    fn dc_without_neighbours_is_the_mid_grey() {
        let mut p = vec![7u8; 16 * 16];
        pred4x4(&mut view(&mut p, 16, 0, 4, NONE), 2);
        assert!(p.iter().take(4).all(|&s| s == 128));
        let mut p = vec![7u8; 16 * 16];
        Scalar.intra16x16(view(&mut p, 16, 0, 16, NONE), 2);
        assert!(p.iter().all(|&s| s == 128));
        let mut p = vec![7u8; 8 * 8];
        Scalar.intra_chroma(view(&mut p, 8, 0, 8, NONE), 0);
        assert!(p.iter().all(|&s| s == 128));
    }

    #[test]
    fn a_mode_out_of_range_predicts_dc() {
        for (n, mode, dc) in [(4, 9, 2), (8, 9, 2), (16, 4, 2), (8, 4, 0)] {
            let run = |p: &mut Vec<u8>, mode: u8| {
                let v = view(p, 16, 16 * 16 + 16, n, ALL);
                let mut v = v;
                match (n, dc) {
                    (4, _) => pred4x4(&mut v, mode),
                    (8, 2) => pred8x8(&mut v, mode),
                    (16, _) => pred16x16(&mut v, mode),
                    _ => pred_chroma(&mut v, mode),
                }
            };
            let mut next = rng(n as u32);
            let p: Vec<u8> = (0..48 * 16).map(|_| next()).collect();
            let (mut a, mut b) = (p.clone(), p);
            run(&mut a, mode);
            run(&mut b, dc);
            assert_eq!(a, b, "{n} mode {mode}");
        }
    }

    #[test]
    fn a_view_of_another_size_is_left_alone() {
        let mut p = vec![9u8; 32 * 32];
        let before = p.clone();
        pred4x4(&mut view(&mut p, 32, 33, 8, ALL), 2);
        pred8x8(&mut view(&mut p, 32, 33, 4, ALL), 2);
        Scalar.intra16x16(view(&mut p, 32, 33, 8, ALL), 2);
        Scalar.intra_chroma(view(&mut p, 32, 33, 16, ALL), 0);
        let coeffs = [[1; 16]; 16];
        Scalar.intra4x4_mb(view(&mut p, 32, 33, 8, ALL), &[2; 16], &coeffs, 0xffff);
        Scalar.intra8x8_mb(view(&mut p, 32, 33, 8, ALL), &[2; 4], &[[1; 64]; 4], 0xf);
        assert_eq!(p, before);
    }

    type Old = fn(&mut [u8], usize, usize, u8, IntraAvail) -> reference::Result<()>;
    type New = fn(&mut IntraMut<'_>, u8);

    /// Every prediction against the code the decoder ran before it moved
    /// here, byte for byte: every mode, every combination of available
    /// neighbours, blocks at every position of a plane (its edges and
    /// corners included), and samples from the whole range. Where the view
    /// refuses a block, because a neighbour it names or the block itself
    /// lies outside the plane, the old code read the missing neighbours as
    /// 0 or refused the block too; the decoder names only neighbours that
    /// exist, so those cases are counted and skipped.
    #[test]
    fn every_prediction_matches_the_code_it_replaced() {
        let cases: [(&str, New, Old, usize, u8); 4] = [
            ("4x4", pred4x4, reference::pred4x4, 4, 9),
            ("8x8", pred8x8, reference::pred8x8, 8, 9),
            ("16x16", pred16x16, reference::pred16x16, 16, 4),
            ("chroma", pred_chroma, reference::pred_chroma, 8, 4),
        ];
        let mut next = rng(0x1a7);
        let (mut checked, mut refused) = (0, 0);
        for (name, new, old, n, modes) in cases {
            for bits in 0..16u8 {
                let avail = avail_of(bits);
                // Planes as wide as the block (a picture one macroblock wide),
                // one sample wider, and wide, each also cut short in the
                // middle of its last row.
                let shapes = [(n, 0), (n + 1, n / 2 + 1), (40, 0), (40, 17)];
                for (mode, (stride, cut)) in (0..modes).flat_map(|m| shapes.map(|s| (m, s))) {
                    let rows = 40;
                    let len = stride * rows - cut;
                    let last = [rows - n, rows - n + 1, rows - 1];
                    for y in (0..=rows - n + 1).step_by(3).chain(last) {
                        for x in (0..=stride - n + 1).step_by(if stride > 8 { 5 } else { 1 }) {
                            // Smooth planes as well as noisy ones, so that
                            // the filtered and averaged modes see both.
                            let smooth = (x + y) % 2 == 0;
                            let base = next();
                            let plane: Vec<u8> = (0..len)
                                .map(|i| {
                                    if smooth {
                                        base.wrapping_add((i % 7) as u8)
                                    } else {
                                        next()
                                    }
                                })
                                .collect();
                            let off = y * stride + x;
                            let mut want = plane.clone();
                            let mut got = plane;
                            let Ok(mut v) = IntraMut::new(&mut got, off, stride, n, avail) else {
                                refused += 1;
                                continue;
                            };
                            new(&mut v, mode);
                            let a = old(&mut want, stride, off, mode, avail);
                            assert!(a.is_ok(), "{name} mode {mode} at ({x}, {y}), {avail:?}");
                            assert_eq!(got, want, "{name} mode {mode} at ({x}, {y}), {avail:?}");
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 10_000, "{checked} cases");
        assert!(refused > 0);
    }

    /// `block_avail` as `coreav-h264`'s `recon.rs` had it before the rule
    /// moved here, frozen.
    fn block_avail_before(nb: [bool; 4], x: usize, y: usize, n: usize, order: usize) -> IntraAvail {
        let [a, b, c, d] = nb;
        let top_right = if y == 0 {
            if x + n < 16 { b } else { c }
        } else if x + n < 16 {
            let r = ((y - 1) / 4) * 4 + (x + n) / 4;
            let other = BLK_OF_RASTER.get(r).copied().unwrap_or(usize::MAX);
            let other = if n == 8 { other / 4 } else { other };
            other < order
        } else {
            false
        };
        IntraAvail {
            left: x > 0 || a,
            top: y > 0 || b,
            top_right,
            top_left: match (x > 0, y > 0) {
                (true, true) => true,
                (false, true) => a,
                (true, false) => b,
                (false, false) => d,
            },
        }
    }

    /// Whether the luma sample at (sx, sy) of the macroblock's coordinates
    /// (-1 to 31 across, -1 to 15 down) is available to the block decoded
    /// `order`th (in units of `n`), from first principles (clauses 6.4.11.4
    /// and 6.4.12): inside the macroblock if its block is decoded earlier;
    /// above, above-left and above-right in macroblocks B, D and C; left in
    /// A; right of the macroblock never.
    fn sample_available(mb: IntraAvail, sx: i32, sy: i32, n: usize, order: usize) -> bool {
        match (sx, sy) {
            (x, y) if (0..16).contains(&x) && (0..16).contains(&y) => {
                let r = (y / 4 * 4 + x / 4) as usize;
                let blk = BLK_OF_RASTER[r];
                let unit = if n == 8 { blk / 4 } else { blk };
                unit < order
            }
            (-1, -1) => mb.top_left,
            (-1, y) if y >= 0 => mb.left,
            (x, -1) if (0..16).contains(&x) => mb.top,
            (x, -1) if x >= 16 => mb.top_right,
            _ => false,
        }
    }

    #[test]
    fn block_availability_is_the_decoders_and_the_specifications() {
        for bits in 0..16u8 {
            let mb = avail_of(bits);
            let nb = [mb.left, mb.top, mb.top_right, mb.top_left];
            for (n, count) in [(4, 16), (8, 4)] {
                for order in 0..count {
                    let (x, y) = if n == 4 {
                        blk_xy(order)
                    } else {
                        ((order % 2) * 8, (order / 2) * 8)
                    };
                    let got = block_avail(mb, x, y, n, order);
                    assert_eq!(
                        got,
                        block_avail_before(nb, x, y, n, order),
                        "{n} {order} {mb:?}"
                    );
                    let (x, y, n4) = (x as i32, y as i32, n as i32);
                    let spec = IntraAvail {
                        left: sample_available(mb, x - 1, y, n, order),
                        top: sample_available(mb, x, y - 1, n, order),
                        top_right: sample_available(mb, x + n4, y - 1, n, order),
                        top_left: sample_available(mb, x - 1, y - 1, n, order),
                    };
                    assert_eq!(got, spec, "{n} {order} {mb:?}");
                }
            }
        }
        // Raster block 3 (luma4x4BlkIdx 5) takes its top right from C,
        // and luma4x4BlkIdx 3, 7, 11, 13 and 15 never have one.
        let only_c = avail_of(4);
        let (x, y) = blk_xy(5);
        assert!(block_avail(only_c, x, y, 4, 5).top_right);
        for blk in [3, 7, 11, 13, 15] {
            let (x, y) = blk_xy(blk);
            assert!(!block_avail(ALL, x, y, 4, blk).top_right, "{blk}");
        }
    }

    /// The macroblock-level calls against the loop the decoder ran before
    /// them: the frozen prediction, the frozen availability and the
    /// reference transform, block by block, for every combination of
    /// macroblock neighbours, random modes, coefficients and masks.
    #[test]
    fn a_macroblock_call_is_the_decoders_loop() {
        let mut next = rng(0x4a1);
        let stride = 48;
        let off = 16 * stride + 16;
        for round in 0..64 {
            for bits in 0..16u8 {
                let mb = avail_of(bits);
                let nb = [mb.left, mb.top, mb.top_right, mb.top_left];
                let plane: Vec<u8> = (0..stride * 33).map(|_| next()).collect();
                let modes4: [u8; 16] = core::array::from_fn(|_| next() % 9);
                let modes8: [u8; 4] = core::array::from_fn(|_| next() % 9);
                let small = |next: &mut dyn FnMut() -> u8| i32::from(next()) * 8 - 1024;
                let coeffs4: [[i32; 16]; 16] =
                    core::array::from_fn(|_| core::array::from_fn(|_| small(&mut next)));
                let coeffs8: [[i32; 64]; 4] =
                    core::array::from_fn(|_| core::array::from_fn(|_| small(&mut next)));
                let nz4 = u16::from(next()) << 8 | u16::from(next());
                let nz8 = next() & 15;
                let nz4 = if round == 0 { 0xffff } else { nz4 };

                let mut want = plane.clone();
                for blk in 0..16 {
                    let (x, y) = blk_xy(blk);
                    let a = block_avail_before(nb, x, y, 4, blk);
                    let o = off + y * stride + x;
                    reference::pred4x4(&mut want, stride, o, modes4[blk], a).expect("old");
                    if nz4 & (1 << blk) != 0 {
                        let b = BlockMut::new(&mut want, o, stride, 4, 4).expect("block");
                        Scalar.idct4x4_add(b, &coeffs4[blk]);
                    }
                }
                let mut got = plane.clone();
                Scalar.intra4x4_mb(view(&mut got, stride, off, 16, mb), &modes4, &coeffs4, nz4);
                assert_eq!(got, want, "4x4 {mb:?}");

                let mut want = plane.clone();
                for b8 in 0..4 {
                    let (x, y) = ((b8 % 2) * 8, (b8 / 2) * 8);
                    let a = block_avail_before(nb, x, y, 8, b8);
                    let o = off + y * stride + x;
                    reference::pred8x8(&mut want, stride, o, modes8[b8], a).expect("old");
                    if nz8 & (1 << b8) != 0 {
                        let b = BlockMut::new(&mut want, o, stride, 8, 8).expect("block");
                        Scalar.idct8x8_add(b, &coeffs8[b8]);
                    }
                }
                let mut got = plane;
                Scalar.intra8x8_mb(view(&mut got, stride, off, 16, mb), &modes8, &coeffs8, nz8);
                assert_eq!(got, want, "8x8 {mb:?}");
            }
        }
    }
}
