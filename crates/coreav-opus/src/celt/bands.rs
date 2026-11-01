//! The shape of each band (RFC 6716 4.3.4, 4.3.5): split recursively into
//! halves while it has bits enough, each half coded with pyramid vector
//! quantization ([`super::pvq`]), stereo bands coded as mid and side with
//! the angle between them, bands without pulses filled from a lower band
//! or with noise, and anti-collapse for transients.
//!
//! What decides the reads is integer and the reference's: each band's
//! budget from the running balance, the angle's resolution and
//! probability model, the bits given to each half (with the reference's
//! fixed-point cosine and log-tangent), and the pulse counts from the
//! pulse cache. The coefficients are `f32`.
//!
//! The arrangement is CoreAV's: a [`Shape`] carries the frame's decoding
//! state through the recursion; a band's top level (the time-frequency
//! change, [`Shape::mono`]) is apart from its splitting
//! ([`Shape::partition`]) and from stereo ([`Shape::stereo`]); and the
//! folding source is copied out before a band is decoded, so the band can
//! write the folding buffer it was read from.

use super::alloc::{Allocation, EIGHTHS};
use super::pvq::{self, MAX_BAND};
use super::{BANDS, Bands, at, edge, set, width};
use crate::range::{RangeDecoder, ilog};
use crate::tables::celt as t;

/// The coefficients of a 20 ms frame up to the top of the last band.
const NORM: usize = 800;

/// The folding buffers of [`decode`], kept between frames so that they are
/// not cleared for each: a frame reads only the values it wrote.
#[derive(Clone, Debug)]
pub(super) struct Folding {
    /// The decoded bands, scaled for folding, per channel.
    norm: [[f32; NORM]; 2],
    /// A band's folding source, copied out of `norm`.
    lowband: [f32; MAX_BAND],
}

impl Default for Folding {
    fn default() -> Self {
        Self {
            norm: [[0.0; NORM]; 2],
            lowband: [0.0; MAX_BAND],
        }
    }
}

/// Aggressive spreading, which disables folding's collapse estimate.
const SPREAD_AGGRESSIVE: usize = 3;

/// The pseudo-random generator of folding and anti-collapse (4.3.4.4).
fn lcg(seed: u32) -> u32 {
    seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
}

/// The reference's Q15 product of two 16-bit values, rounded.
fn frac_mul16(a: i32, b: i32) -> i32 {
    (16384 + i32::from(a as i16) * i32::from(b as i16)) >> 15
}

/// cos(pi/2 * x / 16384) in Q15, in the reference's integer arithmetic,
/// which the split of a band's bits depends on.
fn cos_q15(x: i32) -> i32 {
    let x2 = ((4096 + x * x) >> 13) as i16;
    let x2 = i32::from(x2);
    let y = (32767 - x2) + frac_mul16(x2, -7651 + frac_mul16(x2, 8277 + frac_mul16(-626, x2)));
    1 + i32::from(y as i16)
}

/// log2(sine / cosine) in Q11, in the reference's integer arithmetic.
fn log2_tan(sine: i32, cosine: i32) -> i32 {
    let lc = ilog(cosine as u32) as i32;
    let ls = ilog(sine as u32) as i32;
    let cosine = cosine << (15 - lc).max(0);
    let sine = sine << (15 - ls).max(0);
    (ls - lc) * (1 << 11) + frac_mul16(sine, frac_mul16(sine, -2597) + 7932)
        - frac_mul16(cosine, frac_mul16(cosine, -2597) + 7932)
}

/// floor(sqrt(x)).
fn isqrt(x: u32) -> u32 {
    let mut r = f64::from(x).sqrt() as u32;
    while u64::from(r) * u64::from(r) > u64::from(x) {
        r -= 1;
    }
    while u64::from(r + 1) * u64::from(r + 1) <= u64::from(x) {
        r += 1;
    }
    r
}

/// The row of the pulse cache for a band of `band`'s width at `lm`, -1
/// to 3: its first entry is the largest pulse count, then the cost of
/// each count in eighth bits, minus one (4.3.4.1).
fn cache(band: usize, lm: i32) -> &'static [u8] {
    let row = (lm + 1).max(0) as usize * BANDS + band;
    usize::try_from(at(&t::PULSE_CACHE_INDEX, row))
        .ok()
        .and_then(|i| t::PULSE_CACHE_BITS.get(i..))
        .unwrap_or(&[])
}

/// The pulse count, as a cache index, whose cost is closest to `bits`.
/// A row's costs do not fall with the index, so the index below `bits` is the
/// count of those below it, which takes no branch (the reference searches
/// the row by halves, to the same index).
fn bits_to_pulses(cache: &[u8], bits: i32) -> usize {
    let cost = |q: usize| i32::from(at(cache, q));
    let bits = bits - 1;
    let max = usize::from(at(cache, 0));
    let row = cache.get(1..=max).unwrap_or(&[]);
    let lo = row.iter().filter(|&&c| i32::from(c) < bits).count();
    let hi = (lo + 1).min(max);
    let below = if lo == 0 { -1 } else { cost(lo) };
    if bits - below <= cost(hi) - bits {
        lo
    } else {
        hi
    }
}

/// What a cache index of pulses costs, in eighth bits.
fn pulses_to_bits(cache: &[u8], q: usize) -> i32 {
    if q == 0 {
        0
    } else {
        i32::from(at(cache, q)) + 1
    }
}

/// The number of pulses of a cache index: exact up to 8, then 8 to 15
/// times a power of two.
fn pulses(q: usize) -> usize {
    if q < 8 {
        q
    } else {
        (8 + (q & 7)) << ((q >> 3) - 1)
    }
}

/// The dot product of `a` and `b`, in eight running sums so that the
/// additions do not wait on one another.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    const LANES: usize = 8;
    let mut sums = [0f32; LANES];
    let (a8, b8) = (a.chunks_exact(LANES), b.chunks_exact(LANES));
    let tail: f32 = a8
        .remainder()
        .iter()
        .zip(b8.remainder())
        .map(|(a, b)| a * b)
        .sum();
    for (a, b) in a8.zip(b8) {
        for ((s, a), b) in sums.iter_mut().zip(a).zip(b) {
            *s += a * b;
        }
    }
    sums.iter().sum::<f32>() + tail
}

/// Scales `x` to the norm `gain`.
fn renormalize(x: &mut [f32], gain: f32) {
    let energy = 1e-15 + dot(x, x);
    let g = gain / energy.sqrt();
    for v in x {
        *v *= g;
    }
}

/// One level of the Haar transform on `n` coefficients of `stride`
/// interleaved blocks: sums and differences of neighbouring pairs.
fn haar(x: &mut [f32], n: usize, stride: usize) {
    const S: f32 = core::f32::consts::FRAC_1_SQRT_2;
    for i in 0..stride {
        for j in 0..n / 2 {
            let (a, b) = (stride * 2 * j + i, stride * (2 * j + 1) + i);
            if let (Some(&u), Some(&v)) = (x.get(a), x.get(b)) {
                set(x, a, S * u + S * v);
                set(x, b, S * u - S * v);
            }
        }
    }
}

/// The blocks' order in time for `stride` blocks: the order of the
/// Hadamard transform's rows, or plain.
fn block_order(stride: usize, hadamard: bool) -> impl Fn(usize) -> usize {
    let order = t::HADAMARD_ORDER
        .get(stride.saturating_sub(2)..(2 * stride).saturating_sub(2))
        .unwrap_or(&[]);
    move |i| {
        if hadamard {
            usize::from(at(order, i))
        } else {
            i
        }
    }
}

/// Gathers `stride` interleaved blocks of `n` coefficients each into one
/// block after another (4.3.4.5).
fn deinterleave(x: &mut [f32], n: usize, stride: usize, hadamard: bool) {
    let mut tmp = [0f32; MAX_BAND];
    let order = block_order(stride, hadamard);
    for i in 0..stride {
        for j in 0..n {
            set(&mut tmp, order(i) * n + j, at(x, j * stride + i));
        }
    }
    let len = (n * stride).min(x.len());
    if let (Some(x), Some(tmp)) = (x.get_mut(..len), tmp.get(..len)) {
        x.copy_from_slice(tmp);
    }
}

/// The inverse of [`deinterleave`].
fn interleave(x: &mut [f32], n: usize, stride: usize, hadamard: bool) {
    let mut tmp = [0f32; MAX_BAND];
    let order = block_order(stride, hadamard);
    for i in 0..stride {
        for j in 0..n {
            set(&mut tmp, j * stride + i, at(x, order(i) * n + j));
        }
    }
    let len = (n * stride).min(x.len());
    if let (Some(x), Some(tmp)) = (x.get_mut(..len), tmp.get(..len)) {
        x.copy_from_slice(tmp);
    }
}

/// Mid and side, decoded with unit norm and the side scaled by its gain,
/// to left and right (4.3.4.4).
fn mid_side_to_left_right(x: &mut [f32], y: &mut [f32], mid: f32) {
    let cross = mid * dot(x, y);
    let side = dot(y, y);
    let left = mid * mid + side - 2.0 * cross;
    let right = mid * mid + side + 2.0 * cross;
    if right < 6e-4 || left < 6e-4 {
        y.copy_from_slice(x);
        return;
    }
    let (lg, rg) = (1.0 / left.sqrt(), 1.0 / right.sqrt());
    for (a, b) in x.iter_mut().zip(y.iter_mut()) {
        let l = mid * *a;
        let r = *b;
        *a = lg * (l - r);
        *b = rg * (l + r);
    }
}

/// The resolution of a split's angle: the number of steps over a quarter
/// turn, from the bits the band has (4.3.4.4).
fn angle_steps(n: usize, b: i32, offset: i32, pulse_cap: i32, stereo: bool) -> i32 {
    let mut n2 = 2 * n as i32 - 1;
    if stereo && n == 2 {
        n2 -= 1;
    }
    let qb = (b - pulse_cap - (4 << 3))
        .min((b + n2 * offset) / n2)
        .min(8 << 3);
    if qb < (1 << 3 >> 1) {
        1
    } else {
        let q = i32::from(at(&t::THETA_EXP2, (qb & 7) as usize)) >> (14 - (qb >> 3));
        (q + 1) >> 1 << 1
    }
}

/// A split's angle and what follows from it: the two halves' gains in Q15,
/// the difference of their bit shares, and the fill mask.
struct Split {
    mid: i32,
    side: i32,
    delta: i32,
    fill: u32,
}

impl Split {
    fn new(angle: i32, n: usize, blocks: usize, fill: u32) -> Self {
        let mask = ((1u64 << blocks.min(32)) - 1) as u32;
        match angle {
            0 => Self {
                mid: 32767,
                side: 0,
                delta: -16384,
                fill: fill & mask,
            },
            16384 => Self {
                mid: 0,
                side: 32767,
                delta: 16384,
                fill: fill & (mask << blocks.min(31)),
            },
            _ => {
                let mid = cos_q15(angle);
                let side = cos_q15(16384 - angle);
                let delta = frac_mul16(((n as i32) - 1) << 7, log2_tan(side, mid));
                Self {
                    mid,
                    side,
                    delta,
                    fill,
                }
            }
        }
    }
}

/// The state of one frame's shape decoding.
struct Shape<'a, 'b> {
    rd: &'a mut RangeDecoder<'b>,
    /// The bits left to the frame, in eighth bits.
    remaining: i32,
    seed: u32,
    spread: usize,
    intensity: usize,
    /// The band being decoded and its time-frequency change.
    band: usize,
    tf_change: i32,
}

impl Shape<'_, '_> {
    /// Decodes a split's angle (4.3.4.4) and, for a stereo band coded with
    /// intensity stereo, whether the side is inverted. Returns the angle in
    /// 1/16384 of a quarter turn, the flag and the eighth bits used.
    fn angle(
        &mut self,
        n: usize,
        b: i32,
        blocks: usize,
        lm: i32,
        stereo: bool,
    ) -> (i32, bool, i32) {
        let pulse_cap = i32::from(at(&t::LOG_N, self.band)) + lm * 8;
        let offset = (pulse_cap >> 1) - if stereo && n == 2 { 16 } else { 4 };
        let mut steps = angle_steps(n, b, offset, pulse_cap, stereo);
        if stereo && self.band >= self.intensity {
            steps = 1;
        }
        let before = self.rd.tell_frac() as i32;
        let mut angle = 0;
        let mut inverted = false;
        if steps != 1 {
            let qn = steps as u32;
            let x = if stereo && n > 2 {
                // A step: three times as likely up to a half turn.
                let half = qn / 2;
                let ft = 3 * (half + 1) + half;
                let fs = self.rd.decode(ft);
                let x = if fs < 3 * (half + 1) {
                    fs / 3
                } else {
                    half + 1 + (fs - 3 * (half + 1))
                };
                let (fl, fh) = if x <= half {
                    (3 * x, 3 * (x + 1))
                } else {
                    (x - 1 - half + 3 * (half + 1), x - half + 3 * (half + 1))
                };
                self.rd.update(fl, fh, ft);
                x
            } else if blocks > 1 || stereo {
                self.rd.uint(qn + 1)
            } else {
                // Triangular, peaking at a half turn.
                let half = qn >> 1;
                let ft = (half + 1) * (half + 1);
                let fm = self.rd.decode(ft);
                let (x, fl, fs) = if fm < (half * (half + 1)) >> 1 {
                    let x = (isqrt(8 * fm + 1) - 1) >> 1;
                    (x, (x * (x + 1)) >> 1, x + 1)
                } else {
                    let x = (2 * (qn + 1)).saturating_sub(isqrt(8 * (ft - fm - 1) + 1)) >> 1;
                    let rest = (qn + 1).saturating_sub(x);
                    (
                        x,
                        ft.saturating_sub((rest * (qn + 2).saturating_sub(x)) >> 1),
                        rest,
                    )
                };
                self.rd.update(fl, fl + fs, ft);
                x
            };
            angle = x as i32 * 16384 / steps;
        } else if stereo {
            inverted = b > 2 << 3 && self.remaining > 2 << 3 && self.rd.bit_logp(2);
        }
        (angle, inverted, self.rd.tell_frac() as i32 - before)
    }

    /// A band of one coefficient per channel: a sign each.
    fn single(&mut self, x: &mut [f32], y: Option<&mut [f32]>, out: Option<&mut [f32]>) -> u32 {
        for channel in core::iter::once(&mut *x).chain(y) {
            let mut negative = false;
            if self.remaining >= EIGHTHS {
                negative = self.rd.bits(1) == 1;
                self.remaining -= EIGHTHS;
            }
            set(channel, 0, if negative { -1.0 } else { 1.0 });
        }
        if let Some(out) = out {
            set(out, 0, at(x, 0));
        }
        1
    }

    /// A band of one channel at the top of its recursion: the time-frequency
    /// change around the split decoding, and the band's copy into the
    /// folding buffer (`out`). Returns the collapse mask.
    #[allow(clippy::too_many_arguments)]
    fn mono(
        &mut self,
        x: &mut [f32],
        b: i32,
        blocks: usize,
        mut lowband: Option<&mut [f32]>,
        lm: i32,
        out: Option<&mut [f32]>,
        gain: f32,
        mut fill: u32,
    ) -> u32 {
        let n = x.len();
        if n == 1 {
            return self.single(x, None, out);
        }
        let long = blocks == 1;
        let mut blocks = blocks;
        let mut per_block = n / blocks.max(1);
        let mut tf = self.tf_change;
        // Join blocks for frequency resolution, or split them for time
        // resolution (4.3.4.5).
        let joins = tf.max(0) as usize;
        for k in 0..joins {
            if let Some(l) = lowband.as_deref_mut() {
                haar(l, n >> k, 1 << k);
            }
            fill = u32::from(at(&t::FILL_INTERLEAVE, (fill & 0xF) as usize))
                | u32::from(at(&t::FILL_INTERLEAVE, (fill >> 4) as usize)) << 2;
        }
        blocks >>= joins;
        per_block <<= joins;
        let mut splits = 0;
        while per_block & 1 == 0 && tf < 0 {
            if let Some(l) = lowband.as_deref_mut() {
                haar(l, per_block, blocks);
            }
            fill |= fill << blocks;
            blocks <<= 1;
            per_block >>= 1;
            splits += 1;
            tf += 1;
        }
        let (blocks0, per_block0) = (blocks, per_block);
        if blocks0 > 1 {
            if let Some(l) = lowband.as_deref_mut() {
                deinterleave(l, per_block >> joins, blocks0 << joins, long);
            }
        }

        let mut mask = self.partition(x, b, blocks, lowband, lm, gain, fill);

        if blocks0 > 1 {
            interleave(x, per_block0 >> joins, blocks0 << joins, long);
        }
        let (mut blocks, mut per_block) = (blocks0, per_block0);
        for _ in 0..splits {
            blocks >>= 1;
            per_block <<= 1;
            mask |= mask >> blocks;
            haar(x, per_block, blocks);
        }
        for k in 0..joins {
            mask = u32::from(at(&t::FILL_DEINTERLEAVE, mask as usize));
            haar(x, n >> k, 1 << k);
        }
        blocks <<= joins;
        if let Some(out) = out {
            let scale = (n as f32).sqrt();
            for (o, &v) in out.iter_mut().zip(x.iter()) {
                *o = scale * v;
            }
        }
        mask & ((1u64 << blocks.min(32)) - 1) as u32
    }

    /// A band of one channel below its time-frequency change: split in two
    /// halves while it has bits enough for that (4.3.4.4), else its pulses
    /// or, without any, folding or noise.
    #[allow(clippy::too_many_arguments)]
    fn partition(
        &mut self,
        x: &mut [f32],
        b: i32,
        blocks: usize,
        lowband: Option<&mut [f32]>,
        lm: i32,
        gain: f32,
        fill: u32,
    ) -> u32 {
        let n = x.len();
        let row = cache(self.band, lm);
        let largest = i32::from(at(row, usize::from(at(row, 0))));
        if lm != -1 && n > 2 && b > largest + 12 {
            let half = n >> 1;
            let lm = lm - 1;
            let blocks0 = blocks;
            let fill = if blocks == 1 {
                (fill & 1) | (fill << 1)
            } else {
                fill
            };
            let blocks = (blocks + 1) >> 1;
            let (angle, _, used) = self.angle(half, b, blocks0, lm, false);
            let b = b - used;
            let split = Split::new(angle, half, blocks, fill);
            let mut delta = split.delta;
            if blocks0 > 1 && angle & 0x3fff != 0 {
                if angle > 8192 {
                    // Pre-echo masking.
                    delta -= delta >> (4 - lm);
                } else {
                    // Forward masking.
                    delta = (delta + ((half as i32) << 3 >> (5 - lm))).min(0);
                }
            }
            let mut mid_bits = ((b - delta) / 2).min(b).max(0);
            let mut side_bits = b - mid_bits;
            self.remaining -= used;
            let (mid_gain, side_gain) = (
                gain * (split.mid as f32 / 32768.0),
                gain * (split.side as f32 / 32768.0),
            );
            let (xm, xs) = x.split_at_mut(half.min(n));
            let (lm_low, ls_low) = match lowband {
                Some(l) => {
                    let at = half.min(l.len());
                    let (a, b) = l.split_at_mut(at);
                    (Some(a), Some(b))
                }
                None => (None, None),
            };
            let before = self.remaining;
            let shift = blocks0 >> 1;
            let fill = split.fill;
            return if mid_bits >= side_bits {
                let mut mask = self.partition(xm, mid_bits, blocks, lm_low, lm, mid_gain, fill);
                let rebalance = mid_bits - (before - self.remaining);
                if rebalance > 3 << 3 && angle != 0 {
                    side_bits += rebalance - (3 << 3);
                }
                mask |=
                    self.partition(xs, side_bits, blocks, ls_low, lm, side_gain, fill >> blocks)
                        << shift;
                mask
            } else {
                let mut mask =
                    self.partition(xs, side_bits, blocks, ls_low, lm, side_gain, fill >> blocks)
                        << shift;
                let rebalance = side_bits - (before - self.remaining);
                if rebalance > 3 << 3 && angle != 16384 {
                    mid_bits += rebalance - (3 << 3);
                }
                mask |= self.partition(xm, mid_bits, blocks, lm_low, lm, mid_gain, fill);
                mask
            };
        }

        // No split: the most pulses the bits pay for, within the budget.
        let mut q = bits_to_pulses(row, b);
        let mut cost = pulses_to_bits(row, q);
        self.remaining -= cost;
        while self.remaining < 0 && q > 0 {
            self.remaining += cost;
            q -= 1;
            cost = pulses_to_bits(row, q);
            self.remaining -= cost;
        }
        if q != 0 {
            return pvq::decode(self.rd, x, pulses(q), self.spread, blocks, gain);
        }
        let all = ((1u64 << blocks.min(32)) - 1) as u32;
        let fill = fill & all;
        if fill == 0 {
            x.fill(0.0);
            return 0;
        }
        let mask = match lowband {
            None => {
                for v in x.iter_mut() {
                    self.seed = lcg(self.seed);
                    *v = ((self.seed as i32) >> 20) as f32;
                }
                all
            }
            Some(l) => {
                // The folded band, with noise about 48 dB below it.
                for (v, &l) in x.iter_mut().zip(l.iter()) {
                    self.seed = lcg(self.seed);
                    let noise = if self.seed & 0x8000 != 0 {
                        1.0 / 256.0
                    } else {
                        -1.0 / 256.0
                    };
                    *v = l + noise;
                }
                fill
            }
        };
        renormalize(x, gain);
        mask
    }

    /// A stereo band coded as mid and side (4.3.4.4), not dual stereo.
    #[allow(clippy::too_many_arguments)]
    fn stereo(
        &mut self,
        x: &mut [f32],
        y: &mut [f32],
        b: i32,
        blocks: usize,
        lowband: Option<&mut [f32]>,
        lm: i32,
        out: Option<&mut [f32]>,
        fill: u32,
    ) -> u32 {
        let n = x.len();
        if n == 1 {
            return self.single(x, Some(y), out);
        }
        let (angle, inverted, used) = self.angle(n, b, blocks, lm, true);
        let b = b - used;
        let split = Split::new(angle, n, blocks, fill);
        let (mid, side) = (split.mid as f32 / 32768.0, split.side as f32 / 32768.0);
        let mask;
        if n == 2 {
            // The side is orthogonal to the mid: one bit for its sign.
            let side_bits = if angle != 0 && angle != 16384 { 8 } else { 0 };
            let mid_bits = b - side_bits;
            self.remaining -= used + side_bits;
            let negative = side_bits > 0 && self.rd.bits(1) == 1;
            let sign = if negative { -1.0 } else { 1.0 };
            let (first, second) = if angle > 8192 {
                (&mut *y, &mut *x)
            } else {
                (&mut *x, &mut *y)
            };
            mask = self.mono(first, mid_bits, blocks, lowband, lm, out, 1.0, fill);
            set(second, 0, -sign * at(first, 1));
            set(second, 1, sign * at(first, 0));
            for (a, b) in x.iter_mut().zip(y.iter_mut()) {
                let (m, s) = (mid * *a, side * *b);
                *a = m - s;
                *b = m + s;
            }
        } else {
            let mut mid_bits = ((b - split.delta) / 2).min(b).max(0);
            let mut side_bits = b - mid_bits;
            self.remaining -= used;
            let before = self.remaining;
            let side_fill = split.fill >> blocks;
            if mid_bits >= side_bits {
                let m = self.mono(x, mid_bits, blocks, lowband, lm, out, 1.0, split.fill);
                let rebalance = mid_bits - (before - self.remaining);
                if rebalance > 3 << 3 && angle != 0 {
                    side_bits += rebalance - (3 << 3);
                }
                mask = m | self.mono(y, side_bits, blocks, None, lm, None, side, side_fill);
            } else {
                let s = self.mono(y, side_bits, blocks, None, lm, None, side, side_fill);
                let rebalance = side_bits - (before - self.remaining);
                if rebalance > 3 << 3 && angle != 16384 {
                    mid_bits += rebalance - (3 << 3);
                }
                mask = s | self.mono(x, mid_bits, blocks, lowband, lm, out, 1.0, split.fill);
            }
            mid_side_to_left_right(x, y, mid);
        }
        if inverted {
            for v in y.iter_mut() {
                *v = -*v;
            }
        }
        mask
    }
}

/// The shape decoding's inputs besides the bitstream.
pub(super) struct Frame<'a> {
    pub(super) bands: Bands,
    pub(super) lm: usize,
    pub(super) transient: bool,
    pub(super) spread: usize,
    pub(super) allocation: &'a Allocation,
    pub(super) tf: &'a [i32; BANDS],
    /// The frame's bits for the bands, in eighth bits.
    pub(super) total: i32,
}

/// Decodes the shapes of all bands into `x`, one row per coded channel,
/// with `seed` the folding noise's state, and returns the collapse masks
/// by band and channel (4.3.4).
pub(super) fn decode(
    rd: &mut RangeDecoder<'_>,
    frame: &Frame<'_>,
    seed: &mut u32,
    x: &mut [&mut [f32]],
    buffers: &mut Folding,
) -> [[u8; 2]; BANDS] {
    let channels = x.len().clamp(1, 2);
    let lm = frame.lm;
    let m = 1usize << lm;
    let blocks = if frame.transient { m } else { 1 };
    let alloc = frame.allocation;
    let bands = frame.bands;
    let Folding { norm, lowband } = buffers;
    let mut masks = [[0u8; 2]; BANDS];
    let mut shape = Shape {
        rd,
        remaining: 0,
        seed: *seed,
        spread: frame.spread,
        intensity: alloc.intensity,
        band: 0,
        tf_change: 0,
    };
    let mut balance = alloc.balance;
    let mut fold_from = 0;
    let mut update_fold = true;
    let mut dual = alloc.dual_stereo;
    let first = m * edge(bands.start);
    for band in bands.range() {
        let lo = m * edge(band);
        let n = m * width(band);
        let tell = shape.rd.tell_frac() as i32;
        if band != bands.start {
            balance -= tell;
        }
        shape.remaining = frame.total - tell - 1;
        let b = if band < alloc.coded {
            let share = balance / (alloc.coded - band).min(3) as i32;
            (shape.remaining + 1)
                .min(at(&alloc.shape, band) + share)
                .clamp(0, 16383)
        } else {
            0
        };
        if (lo >= first + n || band == bands.start + 1) && (update_fold || fold_from == 0) {
            fold_from = band;
        }
        if band == bands.start + 1 {
            // The second band folds from the first, which may be narrower:
            // repeat the end of the first.
            let n1 = m * width(bands.start);
            let n2 = m * width(band);
            for row in norm.iter_mut().take(channels) {
                let from = (first + 2 * n1).saturating_sub(n2);
                row.copy_within(from..(first + n1).max(from), first + n1);
            }
        }
        shape.band = band;
        shape.tf_change = at(frame.tf, band);

        // Where to fold from, and which of its blocks were not empty.
        let mut fold = None;
        let (mut x_mask, mut y_mask) =
            (((1u64 << blocks) - 1) as u32, ((1u64 << blocks) - 1) as u32);
        if fold_from != 0
            && (frame.spread != SPREAD_AGGRESSIVE || blocks > 1 || shape.tf_change < 0)
        {
            let start = first.max((m * edge(fold_from)).saturating_sub(n));
            let mut lowest = fold_from;
            while lowest > 0 {
                lowest -= 1;
                if m * edge(lowest) <= start {
                    break;
                }
            }
            let mut highest = fold_from;
            while highest < band && m * edge(highest) < start + n {
                highest += 1;
            }
            x_mask = 0;
            y_mask = 0;
            let mut i = lowest;
            loop {
                let mask = masks.get(i).copied().unwrap_or_default();
                x_mask |= u32::from(at(&mask, 0));
                y_mask |= u32::from(at(&mask, channels - 1));
                i += 1;
                if i >= highest {
                    break;
                }
            }
            fold = Some(start);
        }
        if dual && band == alloc.intensity {
            // Intensity stereo from here on: fold from the mean.
            dual = false;
            let [left, right] = norm;
            for (l, &r) in left.iter_mut().zip(right.iter()).take(lo).skip(first) {
                *l = 0.5 * (*l + r);
            }
        }

        let (first_channel, rest) = x.split_at_mut(1);
        let xb = first_channel
            .first_mut()
            .and_then(|c| c.get_mut(lo..lo + n))
            .unwrap_or(&mut []);
        let yb = rest.first_mut().and_then(|c| c.get_mut(lo..lo + n));
        let [norm_x, norm_y] = &mut *norm;
        let lm_i = lm as i32;
        if dual {
            let low = folding(norm_x, fold, n, lowband);
            let out = norm_x.get_mut(lo..lo + n);
            x_mask = shape.mono(xb, b / 2, blocks, low, lm_i, out, 1.0, x_mask);
            if let Some(yb) = yb {
                let low = folding(norm_y, fold, n, lowband);
                let out = norm_y.get_mut(lo..lo + n);
                y_mask = shape.mono(yb, b / 2, blocks, low, lm_i, out, 1.0, y_mask);
            }
        } else {
            let low = folding(norm_x, fold, n, lowband);
            let out = norm_x.get_mut(lo..lo + n);
            x_mask = match yb {
                Some(yb) => shape.stereo(xb, yb, b, blocks, low, lm_i, out, x_mask | y_mask),
                None => shape.mono(xb, b, blocks, low, lm_i, out, 1.0, x_mask | y_mask),
            };
            y_mask = x_mask;
        }
        if let Some(mask) = masks.get_mut(band) {
            set(mask, 0, x_mask as u8);
            set(mask, channels - 1, y_mask as u8);
        }
        balance += at(&alloc.shape, band) + tell;
        update_fold = b > (n << 3) as i32;
    }
    *seed = shape.seed;
    masks
}

/// The `n` coefficients to fold from, at `fold` in `row`, copied into
/// `buffer`, so that the band can write `row` while it reads them.
fn folding<'a>(
    row: &[f32],
    fold: Option<usize>,
    n: usize,
    buffer: &'a mut [f32],
) -> Option<&'a mut [f32]> {
    let src = row.get(fold?..)?.get(..n)?;
    let dst = buffer.get_mut(..n)?;
    dst.copy_from_slice(src);
    Some(dst)
}

/// Fills the blocks of transient bands that got no pulse with noise at
/// about the energy they had before (4.3.5).
#[allow(clippy::too_many_arguments)]
pub(super) fn anti_collapse(
    x: &mut [&mut [f32]],
    masks: &[[u8; 2]; BANDS],
    lm: usize,
    bands: Bands,
    energy: &[[f32; BANDS]; 2],
    previous: &[[f32; BANDS]; 2],
    previous2: &[[f32; BANDS]; 2],
    shape: &[i32; BANDS],
    mut seed: u32,
) {
    let channels = x.len();
    for band in bands.range() {
        let n0 = width(band);
        let n = n0 << lm;
        let depth = (1 + at(shape, band)) / n as i32;
        let threshold = 0.5 * crate::math::exp2(-0.125 * depth as f32);
        let scale = 1.0 / (n as f32).sqrt();
        for (c, row) in x.iter_mut().enumerate() {
            let get = |e: &[[f32; BANDS]; 2], c: usize| e.get(c).map_or(0.0, |e| at(e, band));
            let mut p1 = get(previous, c);
            let mut p2 = get(previous2, c);
            if channels == 1 {
                p1 = p1.max(get(previous, 1));
                p2 = p2.max(get(previous2, 1));
            }
            let diff = (get(energy, c) - p1.min(p2)).max(0.0);
            let mut r = 2.0 * crate::math::exp2(-diff);
            if lm == 3 {
                r *= core::f32::consts::SQRT_2;
            }
            let r = r.min(threshold) * scale;
            let lo = edge(band) << lm;
            let Some(xb) = row.get_mut(lo..lo + n) else {
                continue;
            };
            let mask = masks.get(band).map_or(0, |m| at(m, c));
            let mut filled = false;
            for k in 0..1usize << lm {
                if mask & (1 << k) == 0 {
                    for j in 0..n0 {
                        seed = lcg(seed);
                        set(xb, (j << lm) + k, if seed & 0x8000 != 0 { r } else { -r });
                    }
                    filled = true;
                }
            }
            if filled {
                renormalize(xb, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_cache_rows_rise() {
        // bits_to_pulses counts the costs below a budget: each row's costs
        // must not fall with the index, and a row holds at most 64 of them,
        // as many as six halvings search.
        for &start in t::PULSE_CACHE_INDEX.iter().filter(|&&i| i >= 0) {
            let row = &t::PULSE_CACHE_BITS[start as usize..];
            let max = usize::from(row[0]);
            assert!(max <= 64);
            assert!(
                row[1..=max].windows(2).all(|w| w[0] <= w[1]),
                "row at {start}"
            );
        }
    }

    #[test]
    fn bits_to_pulses_is_the_search_by_halves() {
        fn halves(cache: &[u8], bits: i32) -> usize {
            let cost = |q: usize| i32::from(cache[q]);
            let bits = bits - 1;
            let (mut lo, mut hi) = (0, usize::from(cache[0]));
            for _ in 0..6 {
                let mid = (lo + hi + 1) >> 1;
                if cost(mid) >= bits {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            let below = if lo == 0 { -1 } else { cost(lo) };
            if bits - below <= cost(hi) - bits {
                lo
            } else {
                hi
            }
        }
        for &start in t::PULSE_CACHE_INDEX.iter().filter(|&&i| i >= 0) {
            let row = &t::PULSE_CACHE_BITS[start as usize..];
            for bits in -8..2000 {
                assert_eq!(
                    bits_to_pulses(row, bits),
                    halves(row, bits),
                    "row at {start}, {bits} bits"
                );
            }
        }
    }

    #[test]
    fn integer_sqrt() {
        for x in (0..100_000).chain([u32::MAX, u32::MAX - 1, 1 << 31]) {
            let r = u64::from(isqrt(x));
            assert!(
                r * r <= u64::from(x) && (r + 1) * (r + 1) > u64::from(x),
                "{x}"
            );
        }
    }

    #[test]
    fn cosine_endpoints() {
        // Near cos(0), cos(pi/2) and cos(pi/4) in Q15.
        assert!(cos_q15(1) >= 32766);
        assert!(cos_q15(16383) < 10);
        assert!((cos_q15(8192) - 23170).abs() < 8);
    }

    #[test]
    fn pulse_counts() {
        assert_eq!(
            (0..10).map(pulses).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(pulses(40), 128);
    }
}
