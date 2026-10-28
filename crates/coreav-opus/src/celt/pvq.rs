//! Pyramid vector quantization (RFC 6716 4.3.4.1 to 4.3.4.3): the index of
//! a vector of `n` integers whose magnitudes sum to `k`, its normalization
//! to the band's gain, and the spreading rotation.
//!
//! The index is decoded with the recurrence of 4.3.4.2 on one row of
//! `V(n, k)`, the number of such vectors, for the current dimension, which
//! steps down a dimension per coefficient; the bits the index costs are the
//! pulse cache's (4.3.4.1), so the decoder's reads do not depend on how the
//! row is computed. Normalization and the rotation are `f32`.

use super::{at, set};
use crate::range::RangeDecoder;
use crate::tables::celt as t;

/// The longest band, in bins: 22 at 20 ms.
pub(super) const MAX_BAND: usize = 176;

/// The most pulses in a band (4.3.4.1: the pulse cache's largest `k`).
const MAX_PULSES: usize = 128;

/// Decodes a vector of `y.len()` integers whose magnitudes sum to `k`
/// (4.3.4.2), returning the sum of their squares.
fn decode_pulses(rd: &mut RangeDecoder<'_>, k: usize, y: &mut [i32]) -> u32 {
    let k = k.min(MAX_PULSES);
    let n = y.len();
    // `row[j]` is V(m, j) for the current dimension m, from m = 0 up.
    let mut row = [0u32; MAX_PULSES + 1];
    set(&mut row, 0, 1);
    for _ in 0..n {
        step_up(row.get_mut(..=k).unwrap_or(&mut []));
    }
    // V(n, k), which the encoder sends the index modulo.
    let total = at(&row, k);
    let mut index = rd.uint(total);
    let mut k = k;
    let mut energy = 0u32;
    for out in y.iter_mut() {
        // `row` holds V(m, .) for this coefficient's m; step it to
        // V(m - 1, .) beside it.
        let upper = row;
        step_down(row.get_mut(..=k).unwrap_or(&mut []));
        // How many vectors have this coefficient >= 0, then which magnitude.
        // (V(m, k) + V(m - 1, k)) / 2, without overflowing: the difference
        // is even.
        let below = at(&row, k);
        let mut p = below.wrapping_add(at(&upper, k).wrapping_sub(below) >> 1);
        let negative = index >= p;
        if negative {
            index = index.wrapping_sub(p);
        }
        let before = k;
        p = p.wrapping_sub(at(&row, k));
        while p > index && k > 0 {
            k -= 1;
            p = p.wrapping_sub(at(&row, k));
        }
        index = index.wrapping_sub(p);
        let value = (before - k) as i32;
        *out = if negative { -value } else { value };
        energy = energy.wrapping_add((value * value) as u32);
    }
    energy
}

/// V(m, .) to V(m + 1, .): V(m + 1, j) = V(m, j) + V(m + 1, j - 1) +
/// V(m, j - 1).
fn step_up(row: &mut [u32]) {
    let mut previous_old = 0u32;
    let mut previous_new = 0u32;
    for v in row.iter_mut() {
        let old = *v;
        *v = old.wrapping_add(previous_new).wrapping_add(previous_old);
        previous_old = old;
        previous_new = *v;
    }
}

/// V(m, .) to V(m - 1, .), the same recurrence solved for V(m - 1, j);
/// V(m - 1, 0) = 1 for m >= 1.
fn step_down(row: &mut [u32]) {
    let mut previous_old = 0u32;
    let mut previous_new = 0u32;
    for (j, v) in row.iter_mut().enumerate() {
        let old = *v;
        *v = if j == 0 {
            1
        } else {
            old.wrapping_sub(previous_old).wrapping_sub(previous_new)
        };
        previous_old = old;
        previous_new = *v;
    }
}

/// Decodes a band's shape with `k` pulses into `x`, scaled to `gain`,
/// and returns its collapse mask: a bit per block of the `blocks` that has
/// a pulse (4.3.4.2, 4.3.4.3, 4.3.5).
pub(super) fn decode(
    rd: &mut RangeDecoder<'_>,
    x: &mut [f32],
    k: usize,
    spread: usize,
    blocks: usize,
    gain: f32,
) -> u32 {
    let n = x.len().min(MAX_BAND);
    let mut y = [0i32; MAX_BAND];
    let y = y.get_mut(..n).unwrap_or(&mut []);
    let energy = decode_pulses(rd, k, y);
    let g = gain / (energy as f32).sqrt();
    for (x, &y) in x.iter_mut().zip(y.iter()) {
        *x = g * y as f32;
    }
    rotate(x, -1, blocks, k, spread);
    if blocks <= 1 {
        return 1;
    }
    let size = (n / blocks).max(1);
    y.chunks(size)
        .enumerate()
        .filter(|(_, block)| block.iter().any(|&v| v != 0))
        .fold(0, |mask, (b, _)| mask | 1 << b)
}

/// The spreading rotation (4.3.4.3) of a band of `blocks` interleaved
/// blocks, undone (`direction` < 0) as the decoder needs it.
pub(super) fn rotate(x: &mut [f32], direction: i32, blocks: usize, k: usize, spread: usize) {
    let len = x.len();
    if spread == 0 || 2 * k >= len || blocks == 0 {
        return;
    }
    let factor = at(&t::SPREAD_FACTOR, spread - 1) as usize;
    let gain = len as f32 / (len + factor * k) as f32;
    let theta = 0.5 * gain * gain;
    let c = (core::f32::consts::FRAC_PI_2 * theta).cos();
    let s = (core::f32::consts::FRAC_PI_2 * (1.0 - theta)).cos();
    // A second rotation, at about sqrt(len / blocks), for long bands.
    let mut stride2 = 0;
    if len >= 8 * blocks {
        stride2 = 1;
        while (stride2 * stride2 + stride2) * blocks + (blocks >> 2) < len {
            stride2 += 1;
        }
    }
    let size = len / blocks;
    for block in x.chunks_exact_mut(size) {
        if direction < 0 {
            if stride2 > 0 {
                givens(block, stride2, s, c);
            }
            givens(block, 1, c, s);
        } else {
            givens(block, 1, c, -s);
            if stride2 > 0 {
                givens(block, stride2, s, -c);
            }
        }
    }
}

/// Rotates each pair `stride` apart by the angle of (`c`, `s`), forward
/// through the block, then back from its end.
fn givens(x: &mut [f32], stride: usize, c: f32, s: f32) {
    let len = x.len();
    if len <= stride {
        return;
    }
    let pair = |x: &mut [f32], i: usize| {
        if let (Some(&a), Some(&b)) = (x.get(i), x.get(i + stride)) {
            set(x, i + stride, c * b + s * a);
            set(x, i, c * a - s * b);
        }
    };
    for i in 0..len - stride {
        pair(x, i);
    }
    for i in (0..len.saturating_sub(2 * stride)).rev() {
        pair(x, i);
    }
}
