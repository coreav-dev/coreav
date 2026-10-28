//! Stereo unmixing (RFC 6716 4.2.8): mid and side to left and right, with
//! the side predicted from the mid by the frame's two weights, which the
//! first 8 ms interpolate from the previous frame's. Mono frames take the
//! same one-sample delay, so that switching between the two is seamless.
//!
//! In the reference's integer arithmetic ([`super::fixed`]), as the
//! frames it unmixes.

use super::fixed::{mul16_low, round_shift, sat16};

/// The unmixing's state across frames.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Unmixer {
    /// The previous frame's weights, `[w0_Q13, w1_Q13]`.
    weights: [i32; 2],
    /// The last two mid samples, older first.
    mid: [i16; 2],
    /// The last side sample.
    side: i16,
}

impl Unmixer {
    /// A switch from mono to stereo: no previous weights or side (4.2.7.1).
    pub(crate) fn start_stereo(&mut self) {
        self.weights = [0; 2];
        self.side = 0;
    }

    /// Unmixes a frame; `weights` are `[w0_Q13, w1_Q13]` (4.2.7.1).
    pub(crate) fn unmix(
        &mut self,
        mid: &[i16],
        side: &[i16],
        weights: [i32; 2],
        khz: usize,
        left: &mut [i16],
        right: &mut [i16],
    ) {
        // The weights step from the previous frame's to these over 8 ms,
        // each step `delta / ramp` rounded in Q16.
        let ramp = 8 * khz;
        let reciprocal = i32::try_from(ramp).map_or(0, |r| (1 << 16) / r.max(1));
        let [step0, step1] =
            [(weights[0], self.weights[0]), (weights[1], self.weights[1])].map(|(to, from)| {
                round_shift(
                    i32::from((to - from) as i16) * i32::from(reciprocal as i16),
                    16,
                )
            });
        let step = [step0, step1];
        let mut current = self.weights;
        let [mut m2, mut m1] = self.mid.map(i32::from);
        let mut s1 = i32::from(self.side);
        for (i, (((&m0, &s0), l), r)) in mid
            .iter()
            .zip(side)
            .zip(left.iter_mut())
            .zip(right.iter_mut())
            .enumerate()
        {
            let m0 = i32::from(m0);
            if i < ramp {
                current = [current[0] + step[0], current[1] + step[1]];
            } else {
                current = weights;
            }
            // The mid's low-passed prediction in Q11, and the mid in Q11.
            let low = (m2 + m0 + (m1 << 1)) << 9;
            let predicted = (s1 << 8)
                .wrapping_add(mul16_low(low, current[0]))
                .wrapping_add(mul16_low(m1 << 11, current[1]));
            let s = i32::from(sat16(round_shift(predicted, 8)));
            *l = sat16(m1 + s);
            *r = sat16(m1 - s);
            (m2, m1, s1) = (m1, m0, i32::from(s0));
        }
        self.mid = [m2, m1].map(sat16);
        self.side = sat16(s1);
        self.weights = weights;
    }

    /// A mono frame, or the mid of a stereo one decoded to mono: delayed
    /// by one sample, as unmixing delays it.
    pub(crate) fn delay(&mut self, mid: &[i16], out: &mut [i16]) {
        let [mut m2, mut m1] = self.mid;
        for (&m0, o) in mid.iter().zip(out.iter_mut()) {
            *o = m1;
            (m2, m1) = (m1, m0);
        }
        self.mid = [m2, m1];
    }
}
