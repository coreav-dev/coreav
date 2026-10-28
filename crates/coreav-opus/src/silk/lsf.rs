//! From LSF indices to LPC coefficients (RFC 6716 4.2.7.5.2 to
//! 4.2.7.5.8), in the reference's integer arithmetic: the filter that
//! comes out drives a recursion, so it must match the encoder's bit for bit.
//!
//! The steps: undo the stage-2 backward prediction, weight the residual by
//! the stage-1 vector's IHMW weights, stabilize the spacing, (interpolate
//! with the previous frame,) and convert to a filter whose coefficients
//! fit 16 bits and whose prediction gain is bounded.

use super::frame::Codebook;
use crate::tables::silk as t;

/// The highest LPC order (WB).
pub(crate) const MAX_ORDER: usize = 16;

/// LSFs in Q15, the first `order` entries used.
pub(crate) type Lsf = [i16; MAX_ORDER];

/// LPC coefficients in Q12, the first `order` entries used.
pub(crate) type Lpc = [i16; MAX_ORDER];

/// The adjustment of the stage-2 levels, 0.1 in Q10 (4.2.7.5.2).
const LEVEL_ADJUST_Q10: i32 = 102;

/// The order in which the cosines enter the polynomials (Table 27).
const ORDERING_10: [usize; 10] = [0, 9, 6, 3, 4, 5, 8, 1, 2, 7];
const ORDERING_16: [usize; 16] = [0, 15, 8, 7, 4, 11, 12, 3, 2, 13, 10, 5, 6, 9, 14, 1];

/// Reconstructs a frame's LSFs from its indices (4.2.7.5.2 to 4.2.7.5.4).
pub(crate) fn decode(codebook: &Codebook, stage1: usize, stage2: &[i8; MAX_ORDER]) -> Lsf {
    let order = codebook.order;
    let vector = codebook.stage1_vector(stage1);

    // The residual, undoing the backward prediction from the last
    // coefficient down (4.2.7.5.2).
    let mut residual = [0i32; MAX_ORDER];
    let mut next = 0i32;
    for k in (0..order).rev() {
        let weight = i32::from(codebook.prediction(stage1, k));
        // The reference keeps the previous value in 16 bits here.
        let predicted = (i32::from(next as i16) * weight) >> 8;
        let index = i32::from(stage2.get(k).copied().unwrap_or(0));
        let level = (index << 10) - index.signum() * LEVEL_ADJUST_Q10;
        next = predicted + ((level * codebook.step_q16) >> 16);
        if let Some(r) = residual.get_mut(k) {
            *r = next;
        }
    }

    // Weighted by the stage-1 vector's IHMW weights (4.2.7.5.3).
    let mut lsf = [0i16; MAX_ORDER];
    let mut previous = 0i32;
    for (k, (out, &r)) in lsf.iter_mut().zip(&residual).take(order).enumerate() {
        let here = i32::from(vector.get(k).copied().unwrap_or(0));
        let after = vector.get(k + 1).map_or(256, |&v| i32::from(v));
        let weight = sqrt_approx(laroia(here - previous, after - here) << 16);
        let value = (here << 7) + (r << 14) / weight.max(1);
        *out = value.clamp(0, 32767) as i16;
        previous = here;
    }
    stabilize(&mut lsf, order, codebook.min_spacing);
    lsf
}

/// The IHMW weight of a stage-1 coefficient from its distances in Q8 to
/// its neighbours, in Q2 (4.2.7.5.3), as the reference computes it from
/// the vector in Q15.
fn laroia(before: i32, after: i32) -> i32 {
    let part = |d: i32| (1 << 17) / (d << 7).max(1);
    (part(before) + part(after)).min(i32::from(i16::MAX))
}

/// The square root approximation of 4.2.7.5.3: a Q18 value in, Q9 out.
fn sqrt_approx(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let zeros = x.leading_zeros();
    let frac = x.rotate_right(24u32.wrapping_sub(zeros) & 31) & 0x7f;
    let y = if zeros & 1 == 1 { 32768 } else { 46214 } >> (zeros >> 1);
    y + ((y * (213 * frac)) >> 16)
}

/// Enforces the minimum spacing between LSFs (4.2.7.5.4, with RFC 8251
/// section 7's saturation): up to 20 local adjustments, then a sort and
/// two clamping passes.
fn stabilize(lsf: &mut Lsf, order: usize, min_spacing: &[i16]) {
    let order = order.min(MAX_ORDER);
    let spacing = |k: usize| i32::from(min_spacing.get(k).copied().unwrap_or(0));
    let at = |lsf: &Lsf, k: usize| i32::from(lsf.get(k).copied().unwrap_or(0));
    for _ in 0..20 {
        // The tightest spot, the lowest index on ties.
        let mut worst = at(lsf, 0) - spacing(0);
        let mut index = 0;
        for k in 1..order {
            let gap = at(lsf, k) - (at(lsf, k - 1) + spacing(k));
            if gap < worst {
                worst = gap;
                index = k;
            }
        }
        let gap = (1 << 15) - (at(lsf, order - 1) + spacing(order));
        if gap < worst {
            worst = gap;
            index = order;
        }
        if worst >= 0 {
            return;
        }
        if index == 0 {
            set(lsf, 0, spacing(0));
        } else if index == order {
            set(lsf, order - 1, (1 << 15) - spacing(order));
        } else {
            let half = spacing(index) >> 1;
            let min_center = (0..index).map(spacing).sum::<i32>() + half;
            let max_center = (1 << 15) - (index + 1..=order).map(spacing).sum::<i32>() - half;
            let sum = at(lsf, index - 1) + at(lsf, index);
            let center = limit((sum >> 1) + (sum & 1), min_center, max_center);
            set(lsf, index - 1, center - half);
            set(lsf, index, center - half + spacing(index));
        }
    }
    // The fallback.
    let used = lsf.get_mut(..order).unwrap_or(&mut []);
    used.sort_unstable();
    set(lsf, 0, at(lsf, 0).max(spacing(0)));
    for k in 1..order {
        let floor = (at(lsf, k - 1) + spacing(k)).min(i32::from(i16::MAX));
        set(lsf, k, at(lsf, k).max(floor));
    }
    set(
        lsf,
        order - 1,
        at(lsf, order - 1).min((1 << 15) - spacing(order)),
    );
    for k in (0..order - 1).rev() {
        set(lsf, k, at(lsf, k).min(at(lsf, k + 1) - spacing(k + 1)));
    }
}

/// `value` limited to the bounds in either order, as the reference's
/// `LIMIT` is (the first bound wins when they cross).
fn limit(value: i32, a: i32, b: i32) -> i32 {
    if a > b {
        if value > a {
            a
        } else if value < b {
            b
        } else {
            value
        }
    } else {
        value.clamp(a, b)
    }
}

/// Stores a value as the reference's 16-bit array would.
fn set(lsf: &mut Lsf, k: usize, value: i32) {
    if let Some(v) = lsf.get_mut(k) {
        *v = value as i16;
    }
}

/// The LSFs of the first half of a 20 ms frame (4.2.7.5.5).
pub(crate) fn interpolate(previous: &Lsf, current: &Lsf, weight: u8) -> Lsf {
    let mut out = [0i16; MAX_ORDER];
    for ((o, &p), &c) in out.iter_mut().zip(previous).zip(current) {
        let (p, c) = (i32::from(p), i32::from(c));
        *o = (p + ((i32::from(weight) * (c - p)) >> 2)) as i16;
    }
    out
}

/// Converts LSFs to LPC coefficients in Q12 whose filter is stable
/// (4.2.7.5.6 to 4.2.7.5.8).
pub(crate) fn to_lpc(lsf: &Lsf, order: usize) -> Lpc {
    let order = if order == 16 { 16 } else { 10 };
    let ordering: &[usize] = if order == 16 {
        &ORDERING_16
    } else {
        &ORDERING_10
    };

    // The cosines in Q16, by table lookup and linear interpolation, in
    // the polynomials' order (4.2.7.5.6).
    let mut cosines = [0i32; MAX_ORDER];
    for (&value, &slot) in lsf.iter().zip(ordering) {
        let value = i32::from(value).max(0);
        let index = (value >> 8) as usize;
        let frac = value & 255;
        let low = t::LSF_COS_Q12.get(index).map_or(0, |&c| i32::from(c));
        let high = t::LSF_COS_Q12.get(index + 1).map_or(low, |&c| i32::from(c));
        if let Some(c) = cosines.get_mut(slot) {
            *c = round_shift((low << 8) + (high - low) * frac, 4);
        }
    }
    let half = order / 2;
    let p = polynomial(cosines.iter().step_by(2).take(half));
    let q = polynomial(cosines.iter().skip(1).step_by(2).take(half));

    // The filter in Q17 from the sum and difference of P and Q.
    let mut a = [0i32; MAX_ORDER];
    for k in 0..half {
        let pk = p
            .get(k + 1)
            .copied()
            .unwrap_or(0)
            .wrapping_add(p.get(k).copied().unwrap_or(0));
        let qk = q
            .get(k + 1)
            .copied()
            .unwrap_or(0)
            .wrapping_sub(q.get(k).copied().unwrap_or(0));
        if let Some(v) = a.get_mut(k) {
            *v = qk.wrapping_neg().wrapping_sub(pk);
        }
        if let Some(v) = a.get_mut(order - k - 1) {
            *v = qk.wrapping_sub(pk);
        }
    }
    let a = a.get_mut(..order).unwrap_or(&mut []);

    // Up to 10 rounds of bandwidth expansion to fit 16 bits (4.2.7.5.7).
    let mut fits = false;
    for _ in 0..10 {
        let (index, max) = a.iter().enumerate().fold((0, 0i32), |(i, m), (k, &v)| {
            if v.wrapping_abs() > m {
                (k, v.wrapping_abs())
            } else {
                (i, m)
            }
        });
        let max = round_shift(max, 5);
        if max <= i32::from(i16::MAX) {
            fits = true;
            break;
        }
        let max = max.min(163_838);
        let chirp = 65470 - ((max - 32767) << 14) / ((max * (index as i32 + 1)) >> 2).max(1);
        bandwidth_expand(a, chirp);
    }
    let mut lpc = [0i16; MAX_ORDER];
    if !fits {
        for (v, out) in a.iter_mut().zip(lpc.iter_mut()) {
            *out = round_shift(*v, 5).clamp(-32768, 32767) as i16;
            *v = i32::from(*out) << 5;
        }
    } else {
        to_q12(a, &mut lpc);
    }

    // Up to 16 more rounds while the prediction gain is too high
    // (4.2.7.5.8).
    for round in 0..16 {
        if inverse_prediction_gain(lpc.get(..order).unwrap_or(&[])) >= MIN_INVERSE_GAIN_Q30 {
            break;
        }
        bandwidth_expand(a, 65536 - (2 << round));
        to_q12(a, &mut lpc);
    }
    lpc
}

fn to_q12(a: &[i32], lpc: &mut Lpc) {
    for (&v, out) in a.iter().zip(lpc.iter_mut()) {
        *out = round_shift(v, 5) as i16;
    }
}

/// The coefficients of a product of `(1 - c z^-1 + z^-2)` terms, `c` the
/// 2cos values in Q16, in Q16; only the first half and the middle
/// coefficient, the rest mirroring them (4.2.7.5.6).
fn polynomial<'a>(cosines: impl Iterator<Item = &'a i32>) -> [i32; MAX_ORDER / 2 + 1] {
    let mut out = [0i32; MAX_ORDER / 2 + 1];
    for (k, &c) in cosines.enumerate() {
        if k == 0 {
            out[0] = 1 << 16;
            out[1] = -c;
            continue;
        }
        // Row k from row k - 1, from the top down so each step reads the
        // previous row; the top term mirrors coefficient k - 1.
        let product = |v: i32| round_shift64(i64::from(c) * i64::from(v), 16);
        let below = out.get(k - 1).copied().unwrap_or(0);
        let here = out.get(k).copied().unwrap_or(0);
        if let Some(top) = out.get_mut(k + 1) {
            *top = (below << 1).wrapping_sub(product(here));
        }
        for n in (2..=k).rev() {
            let two_down = out.get(n - 2).copied().unwrap_or(0);
            let one_down = out.get(n - 1).copied().unwrap_or(0);
            if let Some(v) = out.get_mut(n) {
                *v = v.wrapping_add(two_down).wrapping_sub(product(one_down));
            }
        }
        out[1] = out[1].wrapping_sub(c);
    }
    out
}

/// Bandwidth expansion by the chirp factor `chirp` in Q16 (4.2.7.5.7).
fn bandwidth_expand(a: &mut [i32], mut chirp: i32) {
    let step = chirp - 65536;
    let last = a.len().saturating_sub(1);
    for (k, v) in a.iter_mut().enumerate() {
        *v = mul_shift(chirp, *v, 16);
        if k < last {
            chirp = chirp.wrapping_add(round_shift(chirp.wrapping_mul(step), 16));
        }
    }
}

/// 1/10,000 in Q30: the least inverse prediction gain allowed (4.2.7.5.8).
const MIN_INVERSE_GAIN_Q30: i32 = 107_374;

/// The largest reflection coefficient allowed, 0.99975 in Q24.
const MAX_REFLECTION_Q24: i32 = 16_773_022;

/// The inverse of the filter's prediction gain in Q30, or 0 if the filter
/// is unstable (4.2.7.5.8, with RFC 8251 section 6's overflow check).
fn inverse_prediction_gain(lpc: &[i16]) -> i32 {
    let order = lpc.len().min(MAX_ORDER);
    if lpc.iter().map(|&v| i32::from(v)).sum::<i32>() >= 4096 {
        return 0;
    }
    let mut a = [0i32; MAX_ORDER];
    for (q24, &v) in a.iter_mut().zip(lpc) {
        *q24 = i32::from(v) << 12;
    }
    let mut inverse_gain = 1i32 << 30;
    for k in (0..order).rev() {
        let top = a.get(k).copied().unwrap_or(0);
        if top.unsigned_abs() > MAX_REFLECTION_Q24.unsigned_abs() {
            return 0;
        }
        let rc = -(top << 7);
        let denominator = (1 << 30) - mul_shift(rc, rc, 32);
        inverse_gain = mul_shift(inverse_gain, denominator, 32) << 2;
        if k == 0 {
            break;
        }
        // The next row of the Levinson recursion, divided by the
        // denominator through its inverse in Q(bits + 30).
        let bits = 32 - denominator.unsigned_abs().leading_zeros() as i32;
        let inverse = inverse_q(denominator, bits + 30);
        let previous = a;
        for n in 0..k {
            let mirror = previous.get(k - n - 1).copied().unwrap_or(0);
            let here = previous.get(n).copied().unwrap_or(0);
            let numerator =
                here.saturating_sub(round_shift64(i64::from(mirror) * i64::from(rc), 31));
            let next = round_shift64_wide(i64::from(numerator) * i64::from(inverse), bits);
            let Ok(next) = i32::try_from(next) else {
                return 0;
            };
            if let Some(v) = a.get_mut(n) {
                *v = next;
            }
        }
    }
    inverse_gain
}

/// An approximation of `(1 << q) / b` for positive `b` (the reference's
/// inverse with one refinement step).
fn inverse_q(b: i32, q: i32) -> i32 {
    let headroom = b.unsigned_abs().leading_zeros() as i32 - 1;
    let normalized = b << headroom;
    let inverse = (i32::MAX >> 2) / (normalized >> 16).max(1);
    let result = inverse << 16;
    let error = ((1 << 29) - mul_shift(normalized, inverse, 16)) << 3;
    let result = result.wrapping_add(mul_shift(error, inverse, 16));
    let shift = 61 - headroom - q;
    if shift <= 0 {
        let s = (-shift).min(31);
        result.clamp(i32::MIN >> s, i32::MAX >> s) << s
    } else if shift < 32 {
        result >> shift
    } else {
        0
    }
}

/// `(a * b) >> shift` on the full product, truncated to 32 bits.
fn mul_shift(a: i32, b: i32, shift: u32) -> i32 {
    ((i64::from(a) * i64::from(b)) >> shift) as i32
}

/// Rounding right shift by at least 1, as the reference rounds.
fn round_shift(a: i32, shift: u32) -> i32 {
    ((a >> (shift - 1)) + 1) >> 1
}

fn round_shift64(a: i64, shift: u32) -> i32 {
    (((a >> (shift - 1)) + 1) >> 1) as i32
}

fn round_shift64_wide(a: i64, shift: i32) -> i64 {
    let shift = shift.clamp(1, 63) as u32;
    ((a >> (shift - 1)) + 1) >> 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::silk::frame::{NARROW_CODEBOOK, WIDE_CODEBOOK};

    #[test]
    fn weights_stay_in_the_range_the_rfc_gives() {
        // 4.2.7.5.3: the weights of the stage-1 vectors lie in 1819 to
        // 5227 (Q9).
        for codebook in [&NARROW_CODEBOOK, &WIDE_CODEBOOK] {
            for i1 in 0..32 {
                let v = codebook.stage1_vector(i1);
                let mut previous = 0;
                for k in 0..codebook.order {
                    let here = i32::from(v[k]);
                    let after = v.get(k + 1).map_or(256, |&x| i32::from(x));
                    let w = sqrt_approx(laroia(here - previous, after - here) << 16);
                    assert!((1819..=5227).contains(&w), "{w}");
                    previous = here;
                }
            }
        }
    }

    #[test]
    fn stabilized_lsfs_keep_their_spacing() {
        for codebook in [&NARROW_CODEBOOK, &WIDE_CODEBOOK] {
            for i1 in 0..32 {
                for fill in [-10i8, -3, 0, 4, 10] {
                    let lsf = decode(codebook, i1, &[fill; 16]);
                    let mut previous = 0i32;
                    for k in 0..codebook.order {
                        let here = i32::from(lsf[k]);
                        assert!(
                            here - previous >= i32::from(codebook.min_spacing[k]),
                            "{lsf:?}"
                        );
                        previous = here;
                    }
                    assert!(32768 - previous >= i32::from(codebook.min_spacing[codebook.order]));
                    let lpc = to_lpc(&lsf, codebook.order);
                    assert!(
                        inverse_prediction_gain(&lpc[..codebook.order]) >= MIN_INVERSE_GAIN_Q30
                    );
                }
            }
        }
    }

    #[test]
    fn flat_lsfs_give_a_near_flat_filter() {
        // Evenly spaced LSFs are the roots of 1 + z^-(d+1): A(z) = 1.
        for order in [10usize, 16] {
            let mut lsf = [0i16; 16];
            for (k, v) in lsf.iter_mut().enumerate().take(order) {
                *v = ((k as i32 + 1) * 32768 / (order as i32 + 1)) as i16;
            }
            let lpc = to_lpc(&lsf, order);
            assert!(lpc[..order].iter().all(|&a| a.abs() < 64), "{lpc:?}");
        }
    }
}
