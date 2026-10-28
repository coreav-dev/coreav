//! The reconstruction of a SILK frame (RFC 6716 4.2.7.4, 4.2.7.6.1,
//! 4.2.7.8.6 and 4.2.7.9): gains and pitch lags from their indices, the
//! excitation, then long-term (LTP) and short-term (LPC) synthesis.
//!
//! In the reference's integer arithmetic ([`super::fixed`]), so that the
//! output at the SILK rate is the reference decoder's, sample for sample:
//! 4.2.7.9 lets a decoder differ, but the conformance test compares at
//! 48 kHz, where `opus_compare` is sensitive to small differences.
//!
//! The arrangement is CoreAV's: one buffer of 16-bit output that runs from
//! the history into the current frame, and beside it, at the same
//! positions, the LTP state in Q15 (the residual scaled by the inverse of
//! the subframe's gain), so that rewhitening (4.2.7.9.1) writes the LTP
//! state of the positions it covers and the filter reads it with no
//! separate history. The LTP state is rebuilt by rewhitening at the start
//! of every voiced frame and so is not kept between frames.

use super::Rate;
use super::excitation::{BLOCK, MAX_BLOCKS};
use super::fixed::{divide, inverse, mul16, mul16_low, round_shift, sat16};
use super::frame::{Params, Signal};
use super::lsf::{self, Lsf, MAX_ORDER};
use crate::tables::silk as t;

/// The output kept for long-term prediction: 20 ms at 16 kHz, more than
/// the longest lag, the filter's taps and the LPC order reach back
/// (288 + 2 + 16 samples, 4.2.7.9.1).
const HISTORY: usize = 320;

/// The longest frame: 20 ms at 16 kHz.
pub(crate) const MAX_FRAME: usize = 320;

/// The gain index a channel starts from after a reset (4.2.7.4: no clamp
/// applies then, which a previous index of 10 also gives).
const RESET_GAIN_INDEX: i32 = 10;

/// The gain, Q16, before a channel's first frame.
const INITIAL_GAIN_Q16: i32 = 1 << 16;

/// The state of one channel's SILK decoder.
#[derive(Clone, Debug)]
pub(crate) struct Channel {
    /// The previous frame's LSFs, for interpolation (4.2.7.5.5).
    previous_lsf: Lsf,
    /// The last subframe's gain index (4.2.7.4).
    gain_index: i32,
    /// The last subframe's gain, Q16, against which the next rescales the
    /// filters' state.
    gain_q16: i32,
    /// No frame decoded since the last reset: no LSF interpolation.
    after_reset: bool,
    /// The LPC synthesis filter's last `MAX_ORDER` outputs, Q14.
    lpc: [i32; MAX_ORDER],
    /// The last `HISTORY` output samples.
    out: [i16; HISTORY],
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            previous_lsf: [0; MAX_ORDER],
            gain_index: RESET_GAIN_INDEX,
            gain_q16: INITIAL_GAIN_Q16,
            after_reset: true,
            lpc: [0; MAX_ORDER],
            out: [0; HISTORY],
        }
    }
}

impl Channel {
    /// Clears the channel's prediction state: at a new rate (4.2.9), and
    /// for a side channel coded again after mid-only frames (4.2.7.2).
    /// The last gain stays, as in the reference decoder.
    pub(crate) fn reset(&mut self) {
        *self = Self {
            previous_lsf: self.previous_lsf,
            gain_q16: self.gain_q16,
            ..Self::default()
        };
    }

    /// Reconstructs a frame from its parameters and excitation (`pulses`,
    /// the `e_raw` of 4.2.7.8.6) into `out`, one sample per entry.
    pub(crate) fn decode(
        &mut self,
        rate: Rate,
        subframes: usize,
        params: &Params,
        pulses: &[i32; BLOCK * MAX_BLOCKS],
        out: &mut [i16],
    ) {
        let n = 5 * rate.khz();
        let subframes = subframes.min(4);
        let len = (n * subframes).min(MAX_FRAME);
        let codebook = rate.codebook();
        let order = codebook.order;

        // Gains (4.2.7.4), Q16.
        let mut gains = [0i32; 4];
        let mut index = self.gain_index;
        for (k, (gain, &coded)) in gains
            .iter_mut()
            .zip(&params.gains)
            .take(subframes)
            .enumerate()
        {
            let coded = i32::from(coded);
            if k == 0 && params.independent_gain {
                index = coded.max(index - 16);
            } else {
                let delta = coded - 4;
                let threshold = 8 + index;
                index += if delta > threshold {
                    2 * delta - threshold
                } else {
                    delta
                };
            }
            index = index.clamp(0, 63);
            *gain = log2lin((((0x1D_1C71 * index) >> 16) + 2090).min(3967));
        }
        self.gain_index = index;

        // The LPC filters of the two halves (4.2.7.5.5, 4.2.7.9), Q12.
        let lsf = lsf::decode(codebook, params.lsf_stage1, &params.lsf_stage2);
        let second = lsf::to_lpc(&lsf, order);
        let interpolate = params.interpolation < 4 && !self.after_reset;
        let first = if interpolate {
            let mid = lsf::interpolate(&self.previous_lsf, &lsf, params.interpolation);
            lsf::to_lpc(&mid, order)
        } else {
            second
        };
        self.previous_lsf = lsf;
        self.after_reset = false;

        // The excitation (4.2.7.8.6), Q14.
        let offset = t::QUANT_OFFSETS_Q10
            .get(usize::from(params.signal == Signal::Voiced))
            .and_then(|row| row.get(usize::from(params.high_offset)))
            .map_or(0, |&o| i32::from(o));
        let mut excitation = [0i32; MAX_FRAME];
        let mut seed = params.seed;
        for (e, &raw) in excitation.iter_mut().zip(pulses).take(len) {
            let mut value = (raw << 14) - raw.signum() * (80 << 4) + (offset << 4);
            seed = seed.wrapping_mul(196_314_165).wrapping_add(907_633_515);
            if seed & 0x8000_0000 != 0 {
                value = -value;
            }
            seed = seed.wrapping_add(raw as u32);
            *e = value;
        }

        // The output from the history on, the LTP state at the same
        // positions, and the LPC filter's outputs from its state on.
        let mut history = [0i16; HISTORY + MAX_FRAME];
        if let Some(past) = history.get_mut(..HISTORY) {
            past.copy_from_slice(&self.out);
        }
        let mut ltp = [0i32; HISTORY + MAX_FRAME];
        let mut lpc = [0i32; MAX_ORDER + MAX_FRAME];
        if let Some(past) = lpc.get_mut(..MAX_ORDER) {
            past.copy_from_slice(&self.lpc);
        }

        let pitch = params.pitch.as_ref();
        let lags = pitch.map(|p| pitch_lags(rate, subframes, p.lag, p.contour));
        let mut residual = [0i32; MAX_FRAME / 4];
        for s in 0..subframes {
            let start = s * n;
            let j = HISTORY + start;
            let a = if s < 2 { &first } else { &second };
            let a = a.get(..order).unwrap_or(&[]);
            let gain = gains.get(s).copied().unwrap_or(INITIAL_GAIN_Q16);
            let excitation = excitation.get(start..start + n).unwrap_or(&[]);

            // A new gain rescales the filters' state to it.
            let adjust = (gain != self.gain_q16).then(|| divide(self.gain_q16, gain, 16));
            self.gain_q16 = gain;
            if let Some(adjust) = adjust {
                for v in lpc.get_mut(start..start + MAX_ORDER).unwrap_or(&mut []) {
                    *v = mul16(adjust, *v);
                }
            }

            let residual = residual.get_mut(..n).unwrap_or(&mut []);
            match (pitch, lags) {
                (Some(pitch), Some(lags)) => {
                    let lag = lags.get(s).copied().unwrap_or(0);
                    let reach = j.saturating_sub(lag + 2);
                    if s == 0 || (s == 2 && interpolate) {
                        // Rewhiten the output before this subframe with
                        // its filter (4.2.7.9.1), into Q15 at its gain,
                        // scaled down at the start of the frame.
                        let mut inverse_gain = inverse(gain, 47);
                        if s == 0 {
                            let scale = t::LTP_SCALES_Q14
                                .get(pitch.scaling)
                                .map_or(0, |&v| i32::from(v));
                            inverse_gain = mul16_low(inverse_gain, scale) << 2;
                        }
                        for i in reach..j {
                            let whitened = whiten(&history, i, a);
                            if let Some(v) = ltp.get_mut(i) {
                                *v = mul16_low(inverse_gain, i32::from(whitened));
                            }
                        }
                    } else if let Some(adjust) = adjust {
                        for v in ltp.get_mut(reach..j).unwrap_or(&mut []) {
                            *v = mul16(adjust, *v);
                        }
                    }

                    // Long-term synthesis (4.2.7.9.1), Q14.
                    let taps = ltp_taps(
                        pitch.periodicity,
                        pitch.filters.get(s).copied().unwrap_or(0),
                    );
                    for (i, (r, &e)) in (j..).zip(residual.iter_mut().zip(excitation)) {
                        let newest = i + 2 - lag;
                        let past = ltp.get(newest - 4..=newest).unwrap_or(&[]);
                        let prediction = taps
                            .iter()
                            .zip(past.iter().rev())
                            .fold(2i32, |acc, (&b, &x)| acc.wrapping_add(mul16_low(x, b)));
                        *r = e.wrapping_add(prediction << 1);
                        if let Some(v) = ltp.get_mut(i) {
                            *v = *r << 1;
                        }
                    }
                }
                _ => residual.copy_from_slice(excitation.get(..n).unwrap_or(&[])),
            }

            // Short-term synthesis (4.2.7.9.2), Q14, and the output.
            let gain_q10 = gain >> 6;
            for (i, &r) in (start..).zip(residual.iter()) {
                let p = MAX_ORDER + i;
                let past = lpc.get(p - order..p).unwrap_or(&[]);
                let prediction = a
                    .iter()
                    .zip(past.iter().rev())
                    .fold((order / 2) as i32, |acc, (&a, &x)| {
                        acc.wrapping_add(mul16_low(x, i32::from(a)))
                    });
                let value = r.wrapping_add(prediction << 4);
                if let Some(v) = lpc.get_mut(p) {
                    *v = value;
                }
                if let Some(v) = history.get_mut(HISTORY + i) {
                    *v = sat16(round_shift(mul16(value, gain_q10), 8));
                }
            }
        }

        if let Some(frame) = history.get(HISTORY..HISTORY + len) {
            if let Some(out) = out.get_mut(..len) {
                out.copy_from_slice(frame);
            }
        }
        if let Some(kept) = history.get(len..len + HISTORY) {
            self.out.copy_from_slice(kept);
        }
        if let Some(kept) = lpc.get(len..len + MAX_ORDER) {
            self.lpc.copy_from_slice(kept);
        }
    }
}

/// The LPC residual of `signal` at `i` (4.2.7.9.1): the sample less its
/// prediction from the `a.len()` before it, rounded and saturated.
fn whiten(signal: &[i16], i: usize, a: &[i16]) -> i16 {
    let past = signal.get(i.saturating_sub(a.len())..i).unwrap_or(&[]);
    let prediction = a.iter().zip(past.iter().rev()).fold(0i32, |acc, (&a, &x)| {
        acc.wrapping_add(i32::from(a) * i32::from(x))
    });
    let now = signal.get(i).map_or(0, |&x| i32::from(x));
    sat16(round_shift((now << 12).wrapping_sub(prediction), 12))
}

/// `2**(log / 128)` approximately, in the reference's integer form
/// (4.2.7.4); `log` from 0 to 3967.
fn log2lin(log: i32) -> i32 {
    if log < 0 {
        return 0;
    }
    let out = 1i32 << (log >> 7).min(30);
    let frac = log & 0x7f;
    let curve = frac + ((frac * (128 - frac) * -174) >> 16);
    if log < 2048 {
        out + ((out * curve) >> 7)
    } else {
        out + (out >> 7) * curve
    }
}

/// The pitch lag of each subframe (4.2.7.6.1): the primary lag plus the
/// contour's offset, within the rate's range.
fn pitch_lags(rate: Rate, subframes: usize, lag: i32, contour: usize) -> [usize; 4] {
    let (min, max) = (2 * rate.khz() as i32, 18 * rate.khz() as i32);
    let offset = |s: usize| -> i32 {
        let value = match (rate, subframes) {
            (Rate::Narrow, 4) => t::PITCH_CODEBOOK_20MS_NB
                .get(s)
                .and_then(|r| r.get(contour)),
            (Rate::Narrow, _) => t::PITCH_CODEBOOK_10MS_NB
                .get(s)
                .and_then(|r| r.get(contour)),
            (_, 4) => t::PITCH_CODEBOOK_20MS.get(s).and_then(|r| r.get(contour)),
            _ => t::PITCH_CODEBOOK_10MS.get(s).and_then(|r| r.get(contour)),
        };
        value.map_or(0, |&v| i32::from(v))
    };
    let mut lags = [min as usize; 4];
    for (s, out) in lags.iter_mut().enumerate().take(subframes) {
        *out = (min + lag + offset(s)).clamp(min, max) as usize;
    }
    lags
}

/// The five taps of an LTP filter (4.2.7.6.2), Q14.
fn ltp_taps(periodicity: usize, filter: u8) -> [i32; 5] {
    let row = usize::from(filter);
    let taps = match periodicity {
        0 => t::LTP_TAPS_0.get(row),
        1 => t::LTP_TAPS_1.get(row),
        _ => t::LTP_TAPS_2.get(row),
    };
    taps.map_or([0; 5], |taps| taps.map(|b| i32::from(b) << 7))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gains_span_the_range_the_rfc_gives() {
        // 4.2.7.4: Q16 gains from 81920 to 1686110208.
        let gain = |index: i32| log2lin((((0x1D_1C71 * index) >> 16) + 2090).min(3967));
        assert_eq!(gain(0), 81920);
        assert_eq!(gain(63), 1_686_110_208);
        assert!((1..64).all(|i| gain(i) > gain(i - 1)));
    }
}
