//! The polyphase synthesis subband filter (ISO/IEC 11172-3 2.4.3.2,
//! "Synthesis subband filter"): 32 subband samples in, 32 PCM samples out,
//! per time slot.
//!
//! Matrixing gives 64 values V[i] = sum_k N[i][k] S[k] with
//! N[i][k] = cos((16 + i)(2k + 1) pi / 64). With A[m] = sum_k S[k]
//! cos(m (2k + 1) pi / 64), V[i] = A[16 + i], and A[32] = 0, A[64 - m] =
//! -A[m], A[64 + m] = -A[m]: the 32 values A[0..32] give all 64; they are
//! a 32-point DCT-II, computed by the even/odd split (`dct32.rs`). So
//! V[i] is A[16 + i] for i < 16, 0 at 16, -A[48 - i] up to 48 and
//! -A[i - 48] above. The vector U takes the first half of V for the slots
//! of even age and the second half for those of odd age, and output sample
//! j is sum_t D[32 t + j] U[32 t + j] over the 16 last slots t (age t). The
//! window is D[i] = 32 C[i] with C[i] the analysis window of Table 3-C.1.
//!
//! The filter keeps A rather than V, row by row ([m][slot]), and runs four
//! slots at a time, one per lane: the DCT, the window and the history all
//! take four consecutive slots as one vector. Tap t of output j reads row
//! A[16 + j] (j < 16), A[48 - j] (j > 16), or none (j = 16) for an even t,
//! and A[16 - j] (j < 16) or A[j - 16] for an odd t, with the sign folded
//! into the window coefficient.

use super::lanes::F4;
use super::tables;
use std::sync::OnceLock;

/// Time slots per call: at most a frame's 36 (two granules of 18).
pub(crate) const MAX_SLOTS: usize = 36;
/// The slots before the current call that the window reaches.
const HISTORY: usize = 15;
/// Slots per row of A: the history, then the call's slots, rounded up to
/// four.
const WIDTH: usize = HISTORY + MAX_SLOTS;

/// The window, signed and laid out per output.
struct Coefficients {
    /// [j][t]: the coefficient of output j at tap t, in four equal lanes.
    taps: [[F4; 16]; 32],
    /// [j]: the rows of A that the even and the odd taps of output j read.
    rows: [[usize; 2]; 32],
}

fn coefficients() -> &'static Coefficients {
    static TABLE: OnceLock<Coefficients> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut taps = [[F4::ZERO; 16]; 32];
        let mut rows = [[0usize; 2]; 32];
        for (j, (row, out)) in rows.iter_mut().zip(taps.iter_mut()).enumerate() {
            // (row, sign) for the first and the second half of V.
            let (even, even_sign) = match j {
                0..=15 => (16 + j, 1.0),
                16 => (0, 0.0),
                _ => (48 - j, -1.0),
            };
            let (odd, odd_sign) = if j < 16 {
                (16 - j, -1.0)
            } else {
                (j - 16, -1.0)
            };
            *row = [even, odd];
            for (t, tap) in out.iter_mut().enumerate() {
                let sign = if t % 2 == 0 { even_sign } else { odd_sign };
                let d = 32.0
                    * tables::ANALYSIS_WINDOW
                        .get(32 * t + j)
                        .copied()
                        .unwrap_or(0.0);
                *tap = F4([(sign * d) as f32; 4]);
            }
        }
        Coefficients { taps, rows }
    })
}

/// The filter's state for one channel.
#[derive(Clone)]
pub(crate) struct Synthesis {
    /// A by row [m] and slot: slots 0..15 are the 15 before the current
    /// call, the call's slots follow.
    a: [[f32; WIDTH]; 32],
}

impl Default for Synthesis {
    fn default() -> Self {
        Self {
            a: [[0.0; WIDTH]; 32],
        }
    }
}

/// Four lanes of `row` from `at`.
#[inline(always)]
fn lanes<const N: usize>(row: &[f32; N], at: usize) -> F4 {
    match row.get(at..at + 4) {
        Some(&[a, b, c, d]) => F4([a, b, c, d]),
        _ => F4::ZERO,
    }
}

impl Synthesis {
    /// Filters `slots` time slots (at most [`MAX_SLOTS`]) of subband
    /// samples, `input` [subband][slot], into `out` [slot][sample].
    pub(crate) fn filter(&mut self, input: &[[f32; MAX_SLOTS]; 32], slots: usize, out: &mut [f32]) {
        let coefficients = coefficients();
        let slots = slots.min(MAX_SLOTS);
        // All the matrixing first: the window then reads back what the
        // DCTs stored long after the stores.
        for first in (0..slots).step_by(4) {
            let mut x = [F4::ZERO; 32];
            for (lane, row) in x.iter_mut().zip(input.iter()) {
                *lane = lanes(row, first);
            }
            let a = super::dct32::dct32(&x);
            let at = HISTORY + first;
            for (row, value) in self.a.iter_mut().zip(a.iter()) {
                if let Some(slot) = row.get_mut(at..at + 4) {
                    slot.copy_from_slice(&value.0);
                }
            }
        }
        for first in (0..slots).step_by(4) {
            // Tap t of these four slots reads the four slots t back.
            let mut sums = [F4::ZERO; 32];
            for (sum, (taps, &[even, odd])) in sums
                .iter_mut()
                .zip(coefficients.taps.iter().zip(coefficients.rows.iter()))
            {
                // The slots the taps reach: first - 15 to first + 3, with
                // the history's offset.
                let reach = |row: usize| {
                    self.a
                        .get(row)
                        .and_then(|r| r.get(first..first + HISTORY + 4))
                        .and_then(|r| <&[f32; HISTORY + 4]>::try_from(r).ok())
                };
                let (Some(even), Some(odd)) = (reach(even), reach(odd)) else {
                    continue;
                };
                // Four partial sums, to keep the additions independent.
                let mut acc = [F4::ZERO; 4];
                for (quad, c) in taps.chunks_exact(4).enumerate() {
                    if let ([c0, c1, c2, c3], [a0, a1, a2, a3]) = (c, &mut acc) {
                        let t = 4 * quad;
                        *a0 = *a0 + lanes(even, HISTORY - t) * *c0;
                        *a1 = *a1 + lanes(odd, HISTORY - t - 1) * *c1;
                        *a2 = *a2 + lanes(even, HISTORY - t - 2) * *c2;
                        *a3 = *a3 + lanes(odd, HISTORY - t - 3) * *c3;
                    }
                }
                let [a0, a1, a2, a3] = acc;
                *sum = (a0 + a1) + (a2 + a3);
            }
            // Lane l of output j is sample j of slot first + l.
            let block = out.get_mut(first * 32..).unwrap_or_default();
            for (l, slot) in block
                .chunks_exact_mut(32)
                .take((slots - first).min(4))
                .enumerate()
            {
                for (o, sum) in slot.iter_mut().zip(sums.iter()) {
                    *o = sum.0.get(l).copied().unwrap_or(0.0);
                }
            }
        }
        // The last 15 slots become the history.
        for row in self.a.iter_mut() {
            row.copy_within(slots..slots + HISTORY, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flow of ISO/IEC 11172-3 Figure 3-A.2 as written: shift V by 64,
    /// matrix with N[i][k], build U from V, window by D, sum.
    struct Literal {
        v: Vec<f64>,
    }

    impl Literal {
        fn filter(&mut self, s: &[f32; 32]) -> Vec<f64> {
            let pi = std::f64::consts::PI;
            self.v.rotate_right(64);
            for i in 0..64 {
                self.v[i] = (0..32)
                    .map(|k| {
                        ((16 + i) as f64 * (2 * k + 1) as f64 * pi / 64.0).cos() * f64::from(s[k])
                    })
                    .sum();
            }
            let mut u = vec![0.0; 512];
            for i in 0..8 {
                for j in 0..32 {
                    u[i * 64 + j] = self.v[i * 128 + j];
                    u[i * 64 + 32 + j] = self.v[i * 128 + 96 + j];
                }
            }
            let w: Vec<f64> = (0..512)
                .map(|i| u[i] * 32.0 * tables::ANALYSIS_WINDOW[i])
                .collect();
            (0..32)
                .map(|j| (0..16).map(|i| w[j + 32 * i]).sum())
                .collect()
        }
    }

    #[test]
    fn matches_the_literal_flow() {
        let mut fast = Synthesis::default();
        let mut literal = Literal { v: vec![0.0; 1024] };
        let mut seed = 1u32;
        for _ in 0..40 {
            let mut s = [0.0f32; 32];
            for x in &mut s {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *x = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
            }
            let mut input = [[0.0f32; MAX_SLOTS]; 32];
            for (row, &x) in input.iter_mut().zip(&s) {
                row[0] = x;
            }
            let mut out = [0.0f32; 32];
            fast.filter(&input, 1, &mut out);
            let expected = literal.filter(&s);
            for (a, b) in out.iter().zip(&expected) {
                assert!((f64::from(*a) - b).abs() < 1e-5, "{a} {b}");
            }
        }
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn whole_frames_match_the_literal_flow() {
        for slots in [36, 18] {
            let mut fast = Synthesis::default();
            let mut literal = Literal { v: vec![0.0; 1024] };
            let mut seed = 7u32;
            for _ in 0..4 {
                let mut input = [[0.0f32; MAX_SLOTS]; 32];
                let mut expected = Vec::new();
                for t in 0..slots {
                    let mut s = [0.0f32; 32];
                    for (k, x) in s.iter_mut().enumerate() {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        *x = (seed >> 8) as f32 / (1 << 24) as f32 - 0.5;
                        input[k][t] = *x;
                    }
                    expected.extend(literal.filter(&s));
                }
                let mut out = vec![0.0f32; slots * 32];
                fast.filter(&input, slots, &mut out);
                for (i, (a, b)) in out.iter().zip(&expected).enumerate() {
                    assert!((f64::from(*a) - b).abs() < 1e-5, "{slots} {i}: {a} {b}");
                }
            }
        }
    }
}
