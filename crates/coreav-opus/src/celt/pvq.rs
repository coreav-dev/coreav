//! Pyramid vector quantization (RFC 6716 4.3.4.1 to 4.3.4.3): the index of
//! a vector of `n` integers whose magnitudes sum to `k`, its normalization
//! to the band's gain, and the spreading rotation.
//!
//! The index is decoded with the recurrence of 4.3.4.2 on one row of
//! `V(n, k)`, the number of such vectors, for the current dimension, which
//! steps down a dimension per coefficient; the bits the index costs are the
//! pulse cache's (4.3.4.1), so the decoder's reads do not depend on how the
//! row is computed. Normalization and the rotation are `f32`.

use super::at;
use crate::range::RangeDecoder;
use crate::tables::celt as t;

/// The longest band, in bins: 22 at 20 ms.
pub(super) const MAX_BAND: usize = 176;

/// The most pulses in a band (4.3.4.1: the pulse cache's largest `k`).
const MAX_PULSES: usize = 128;

/// V(n, k) for every dimension up to [`MAX_BAND`] and every pulse count up
/// to [`MAX_PULSES`], by 4.3.4.2's recurrence V(n, k) = V(n - 1, k) +
/// V(n, k - 1) + V(n - 1, k - 1); the values past 32 bits wrap, and no band
/// codes them (4.3.4.1: the pulse cache keeps the index within 32 bits).
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
static V: [[u32; MAX_PULSES + 1]; MAX_BAND + 1] = {
    let mut v = [[0u32; MAX_PULSES + 1]; MAX_BAND + 1];
    v[0][0] = 1;
    let mut n = 1;
    while n <= MAX_BAND {
        v[n][0] = 1;
        let mut k = 1;
        while k <= MAX_PULSES {
            v[n][k] = v[n - 1][k]
                .wrapping_add(v[n][k - 1])
                .wrapping_add(v[n - 1][k - 1]);
            k += 1;
        }
        n += 1;
    }
    v
};

/// Decodes a vector of `y.len()` integers whose magnitudes sum to `k`
/// (4.3.4.2), as `f32`, returning the sum of their squares.
fn decode_pulses(rd: &mut RangeDecoder<'_>, k: usize, y: &mut [f32]) -> u32 {
    let n = y.len().min(MAX_BAND);
    let k = k.min(MAX_PULSES);
    // V(n, k), which the encoder sends the index modulo.
    let index = rd.uint(at(V.get(n).unwrap_or(&[0; MAX_PULSES + 1]), k));
    pulses(index, k, y.get_mut(..n).unwrap_or(&mut []))
}

/// The vector of `y.len()` integers whose magnitudes sum to `k` with
/// the given index (4.3.4.2), as `f32`, returning the sum of their squares;
/// every value of `y` is written. The last
/// two coefficients take their magnitudes in closed form: V(1, j) is 2 for
/// every j above 0, so the search over the next to last is a division, and
/// the last takes the pulses left.
fn pulses(mut index: u32, mut k: usize, y: &mut [f32]) -> u32 {
    let n = y.len();
    let row = |m: usize| V.get(m).unwrap_or(&[0; MAX_PULSES + 1]);
    let mut energy = 0u32;
    let mut put = |out: &mut f32, value: usize, negative: bool| {
        let value = value as i32;
        let sign = -i32::from(negative);
        *out = ((value ^ sign) - sign) as f32;
        energy = energy.wrapping_add((value * value) as u32);
    };
    let (head, tail) = y.split_at_mut(n.saturating_sub(2));
    for (i, (m, out)) in (3..=n).rev().zip(head.iter_mut()).enumerate() {
        // No pulse left: the rest are zeros.
        if k == 0 {
            head.get_mut(i..).unwrap_or(&mut []).fill(0.0);
            tail.fill(0.0);
            return energy;
        }
        let (upper, lower) = (row(m), row(m - 1));
        // How many vectors have this coefficient >= 0, then which magnitude.
        // (V(m, k) + V(m - 1, k)) / 2, without overflowing: the difference
        // is even.
        let below = at(lower, k);
        let mut p = below.wrapping_add(at(upper, k).wrapping_sub(below) >> 1);
        let negative = index >= p;
        index = index.wrapping_sub(p & 0u32.wrapping_sub(u32::from(negative)));
        let before = k;
        p = p.wrapping_sub(below);
        while p > index && k > 0 {
            k -= 1;
            p = p.wrapping_sub(at(lower, k));
        }
        index = index.wrapping_sub(p);
        put(out, before - k, negative);
    }
    let (next, last) = match tail {
        [next, last] => (Some(next), last),
        [last] => (None, last),
        _ => return energy,
    };
    if let Some(out) = next {
        if k == 0 {
            *out = 0.0;
            *last = 0.0;
            return energy;
        }
        // V(2, k) = 4k and V(1, k) = 2: 2k + 1 vectors have this
        // coefficient >= 0, 2k - 1 of them above 0, and each magnitude
        // below k takes two of those (the last's two signs).
        let twice = 2 * k as u32;
        let negative = index > twice;
        if negative {
            index -= twice + 1;
        }
        let above = twice - 1;
        let value = if index >= above {
            0
        } else {
            ((twice - index) / 2).min(k as u32)
        };
        index = index.saturating_sub(above.saturating_sub(2 * value));
        put(out, value as usize, negative);
        k -= value as usize;
    }
    put(last, k, k > 0 && index > 0);
    energy
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
    let x = x.get_mut(..n).unwrap_or(&mut []);
    let energy = decode_pulses(rd, k, x);
    let mask = if blocks <= 1 {
        1
    } else {
        x.chunks((n / blocks).max(1))
            .enumerate()
            .filter(|(_, block)| block.iter().any(|&v| v != 0.0))
            .fold(0, |mask, (b, _)| mask | 1 << b)
    };
    let g = gain / (energy as f32).sqrt();
    x.iter_mut().for_each(|v| *v *= g);
    rotate(x, -1, blocks, k, spread);
    mask
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
    let c = crate::math::cos_half_pi(theta);
    let s = crate::math::cos_half_pi(1.0 - theta);
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
/// through the block, then back from its end. A pair's first value is the
/// second of the pair `stride` before it, so the pass goes a run of
/// `stride` at a time, each run's pairs apart from one another; with a
/// stride of 1 the pass is a first-order recurrence ([`chain`]).
fn givens(x: &mut [f32], stride: usize, c: f32, s: f32) {
    let len = x.len();
    if len <= stride || stride == 0 {
        return;
    }
    if stride == 1 {
        chain(x, c, s);
        // Back over the first len - 1 values, from the end: the same
        // recurrence on them reversed, with the angle's sine negated.
        let back = x.get_mut(..len - 1).unwrap_or(&mut []);
        back.reverse();
        chain(back, c, -s);
        back.reverse();
        return;
    }
    let pair = |a: &mut f32, b: &mut f32| {
        (*a, *b) = (c * *a - s * *b, c * *b + s * *a);
    };
    let mut runs = x.chunks_mut(stride);
    if let Some(mut before) = runs.next() {
        for after in runs {
            before
                .iter_mut()
                .zip(after.iter_mut())
                .for_each(|(a, b)| pair(a, b));
            before = after;
        }
    }
    // Back: the pairs within the first len - stride values, from the end.
    let mut runs = x
        .get_mut(..len - stride)
        .unwrap_or(&mut [])
        .rchunks_mut(stride);
    if let Some(mut after) = runs.next() {
        for before in runs {
            before
                .iter_mut()
                .rev()
                .zip(after.iter_mut().rev())
                .for_each(|(a, b)| pair(a, b));
            after = before;
        }
    }
}

/// The rotation of each neighbouring pair of `x` in turn, forward: with
/// `t` the first value of the pair (already rotated as the second of the
/// pair before), the pair becomes `c t - s x[k + 1]` and
/// `t' = c x[k + 1] + s t`, which the next pair starts from. `t` follows a recurrence of
/// pole `s`, taken [`CHAIN`] values at a time: within a run, `t` is the
/// run's own response plus a power of `s` times the `t` entering it, so
/// that the chain of dependent steps is one per run. Up to [`SHORT_CHAIN`]
/// values go a pair at a time, where setting the runs up costs more.
fn chain(x: &mut [f32], c: f32, s: f32) {
    if x.len() <= SHORT_CHAIN {
        let Some((first, rest)) = x.split_first_mut() else {
            return;
        };
        let mut t = *first;
        let mut previous = first;
        for current in rest {
            let v = *current;
            *previous = c * t - s * v;
            t = c * v + s * t;
            previous = current;
        }
        *previous = t;
        return;
    }
    let Some(&first) = x.first() else {
        return;
    };
    // s^0 to s^CHAIN.
    let mut powers = [1f32; CHAIN + 1];
    for i in 1..=CHAIN {
        let previous = powers.get(i - 1).copied().unwrap_or(0.0);
        if let Some(p) = powers.get_mut(i) {
            *p = previous * s;
        }
    }
    let mut t = first;
    let len = x.len();
    let mut k = 0;
    while k + 1 < len {
        let n = (len - 1 - k).min(CHAIN);
        // The run's inputs x[k + 1 ..= k + n], before they are written.
        let mut input = [0f32; CHAIN];
        if let (Some(dst), Some(src)) = (input.get_mut(..n), x.get(k + 1..k + 1 + n)) {
            dst.copy_from_slice(src);
        }
        // q[j]: the run's response before step j, from a zero t.
        let mut q = [0f32; CHAIN + 1];
        let mut last = 0.0;
        for (q, &v) in q.iter_mut().skip(1).zip(&input) {
            last = c * v + s * last;
            *q = last;
        }
        if let Some(out) = x.get_mut(k..k + n) {
            for (((o, &q), &power), &v) in out.iter_mut().zip(&q).zip(&powers).zip(&input) {
                *o = c * (q + power * t) - s * v;
            }
        }
        t = q.get(n).copied().unwrap_or(0.0) + powers.get(n).copied().unwrap_or(0.0) * t;
        k += n;
    }
    if let Some(last) = x.last_mut() {
        *last = t;
    }
}

/// The values [`chain`] takes at a time.
const CHAIN: usize = 8;

/// The most values [`chain`] takes a pair at a time.
const SHORT_CHAIN: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass a pair at a time, as 4.3.4.3 describes it.
    fn givens_pairs(x: &mut [f32], stride: usize, c: f32, s: f32) {
        let len = x.len();
        for i in 0..len - stride {
            let (a, b) = (x[i], x[i + stride]);
            x[i] = c * a - s * b;
            x[i + stride] = c * b + s * a;
        }
        for i in (0..len.saturating_sub(2 * stride)).rev() {
            let (a, b) = (x[i], x[i + stride]);
            x[i] = c * a - s * b;
            x[i + stride] = c * b + s * a;
        }
    }

    /// 4.3.4.2's search a coefficient at a time, to the last.
    fn pulses_by_steps(mut index: u32, mut k: usize, y: &mut [i32]) {
        let n = y.len();
        for (m, out) in (1..=n).rev().zip(y.iter_mut()) {
            let (upper, lower) = (V[m], V[m - 1]);
            let mut p = ((u64::from(upper[k]) + u64::from(lower[k])) / 2) as u32;
            let negative = index >= p;
            if negative {
                index -= p;
            }
            let before = k;
            p -= lower[k];
            while p > index {
                k -= 1;
                p -= lower[k];
            }
            index -= p;
            let value = (before - k) as i32;
            *out = if negative { -value } else { value };
        }
    }

    #[test]
    fn pulses_match_the_search() {
        for (n, row) in V.iter().enumerate().take(5).skip(1) {
            for (k, &count) in row.iter().enumerate() {
                let step = (count / 4096).max(1);
                for index in (0..count).step_by(step as usize) {
                    let mut want = vec![0; n];
                    pulses_by_steps(index, k, &mut want);
                    let mut got = vec![f32::NAN; n];
                    let energy = pulses(index, k, &mut got);
                    let got: Vec<i32> = got.iter().map(|&v| v as i32).collect();
                    assert_eq!(got, want, "n {n} k {k} index {index}");
                    let sum: i32 = got.iter().map(|v| v * v).sum();
                    assert_eq!(energy, sum as u32);
                }
            }
        }
        for (n, k) in [(8, 20), (16, 12), (36, 4), (176, 3)] {
            for index in (0..V[n][k]).step_by(997) {
                let (mut want, mut got) = (vec![0; n], vec![f32::NAN; n]);
                pulses_by_steps(index, k, &mut want);
                pulses(index, k, &mut got);
                let got: Vec<i32> = got.iter().map(|&v| v as i32).collect();
                assert_eq!(got, want, "n {n} k {k} index {index}");
            }
        }
    }

    #[test]
    fn givens_matches_the_pairs() {
        for len in [2, 3, 5, 8, 9, 17, 31, 176] {
            for stride in [1, 2, 3] {
                if len <= stride {
                    continue;
                }
                let input: Vec<f32> = (0..len)
                    .map(|i| ((i * 37 % 23) as f32 - 11.0) / 7.0)
                    .collect();
                let (c, s) = (0.8f32, 0.6f32);
                let mut want = input.clone();
                givens_pairs(&mut want, stride, c, s);
                let mut got = input.clone();
                givens(&mut got, stride, c, s);
                for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                    assert!(
                        (g - w).abs() < 1e-4,
                        "len {len} stride {stride} at {i}: {g} {w}"
                    );
                }
            }
        }
    }
}
