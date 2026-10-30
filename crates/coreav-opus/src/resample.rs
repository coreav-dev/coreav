//! SILK's output from its rate (8, 12 or 16 kHz) to 48 kHz.
//!
//! RFC 6716 leaves the resampler to the decoder (4.2.9), but the
//! conformance test compares at 48 kHz with the reference decoder's
//! output, which its resampler has shaped: a non-linear phase that no
//! linear-phase design matches closely enough for `opus_compare`. So this
//! is the reference's resampler, in its integer arithmetic and
//! with its tables ([`crate::tables::resampler`]), sample for sample:
//!
//! 1. The input is delayed by a few samples (0, 4 and 7 at 8, 12 and
//!    16 kHz), which aligns the three rates' total delay.
//! 2. A 2x upsampler of two chains of three first-order allpass sections,
//!    one chain for the even outputs and one for the odd, in Q10.
//! 3. An 8-tap FIR interpolates the 2x signal at the output's positions,
//!    with taps for 12 fractional positions (symmetric: the second half of
//!    the taps is the first half of the mirrored position, reversed).
//!
//! The interpolation's position steps by a ratio rounded up in Q16, and
//! restarts at 0 for each batch of input: the first 1 ms of each SILK frame
//! (which takes the delayed samples), then the rest in 10 ms batches. The
//! output depends on those boundaries through the rounding, so they are
//! kept as the reference has them.

use crate::silk::Rate;
use crate::silk::fixed::{mul16, round_shift, sat16};
use crate::tables::resampler as t;

/// The output rate in kHz.
const OUT_KHZ: usize = 48;

/// The longest batch: 10 ms at 16 kHz.
const MAX_BATCH: usize = 160;

/// The interpolation filter's taps.
const TAPS: usize = 8;

/// The longest input delay.
const MAX_DELAY: usize = 7;

/// The interpolation's taps for each of the 12 positions: the position's
/// half of the table, then the mirrored position's half reversed.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const PHASES: [[i16; TAPS]; 12] = {
    let mut phases = [[0; TAPS]; 12];
    let mut p = 0;
    while p < 12 {
        let mut k = 0;
        while k < TAPS / 2 {
            phases[p][k] = t::INTERPOLATION_FIR[p][k];
            phases[p][TAPS - 1 - k] = t::INTERPOLATION_FIR[11 - p][k];
            k += 1;
        }
        p += 1;
    }
    phases
};

/// An upsampler of one channel from a SILK rate to 48 kHz.
#[derive(Clone, Debug)]
pub(crate) struct Upsampler {
    rate: Rate,
    /// The input delay in samples.
    delay: usize,
    /// The interpolation's step through the 2x signal, Q16.
    step: i32,
    /// The allpass sections' state, Q10: the even chain, then the odd.
    allpass: [[i32; 3]; 2],
    /// The 2x signal's last `TAPS` samples, oldest first.
    tail: [i16; TAPS],
    /// The input's last `delay` samples.
    delayed: [i16; MAX_DELAY],
}

impl Upsampler {
    pub(crate) fn new(rate: Rate) -> Self {
        let khz = rate.khz();
        let delay = match rate {
            Rate::Narrow => 0,
            Rate::Medium => 4,
            Rate::Wide => 7,
        };
        // The 2x signal's samples per output sample, Q16, rounded up: an
        // estimate from a division, raised until it covers the ratio.
        let (input, output) = (1000 * khz as i64, 1000 * OUT_KHZ as i64);
        let mut step = ((input << 15) / output) << 2;
        while (step * output) >> 16 < input << 1 {
            step += 1;
        }
        Self {
            rate,
            delay,
            step: step as i32,
            allpass: [[0; 3]; 2],
            tail: [0; TAPS],
            delayed: [0; MAX_DELAY],
        }
    }

    pub(crate) fn rate(&self) -> Rate {
        self.rate
    }

    /// Upsamples one SILK frame (10 or 20 ms), writing `48 / khz` samples
    /// per input sample to `out`.
    pub(crate) fn process(&mut self, input: &[i16], out: &mut [i16]) {
        let khz = self.rate.khz();
        let delay = self.delay;
        let len = input.len();
        if len < khz {
            return;
        }
        let mut written = 0;

        // The first millisecond: the delayed samples, then the input's
        // first ones.
        let mut first = [0i16; 16];
        let head = khz - delay;
        if let (Some(dst), Some(src)) = (first.get_mut(..delay), self.delayed.get(..delay)) {
            dst.copy_from_slice(src);
        }
        if let (Some(dst), Some(src)) = (first.get_mut(delay..khz), input.get(..head)) {
            dst.copy_from_slice(src);
        }
        written += self.batch(first.get(..khz).unwrap_or(&[]), out);

        // The rest, up to the samples the next frame delays, in batches.
        for batch in input.get(head..len - delay).unwrap_or(&[]).chunks(10 * khz) {
            written += self.batch(batch, out.get_mut(written..).unwrap_or(&mut []));
        }
        if let (Some(dst), Some(src)) = (self.delayed.get_mut(..delay), input.get(len - delay..)) {
            dst.copy_from_slice(src);
        }
    }

    /// Upsamples a batch of at most 10 ms, returning the samples written.
    fn batch(&mut self, input: &[i16], out: &mut [i16]) -> usize {
        let n = input.len().min(MAX_BATCH);
        // The 2x signal, after the last batch's tail.
        let mut up = [0i16; TAPS + 2 * MAX_BATCH];
        if let Some(dst) = up.get_mut(..TAPS) {
            dst.copy_from_slice(&self.tail);
        }
        let [even, odd] = &mut self.allpass;
        for (&x, pair) in input.iter().zip(
            up.get_mut(TAPS..TAPS + 2 * n)
                .unwrap_or(&mut [])
                .chunks_exact_mut(2),
        ) {
            let x = i32::from(x) << 10;
            if let [a, b] = pair {
                *a = sat16(round_shift(
                    allpass_chain(even, x, &t::UP2_ALLPASS_EVEN),
                    10,
                ));
                *b = sat16(round_shift(allpass_chain(odd, x, &t::UP2_ALLPASS_ODD), 10));
            }
        }

        // Interpolation at the output's positions.
        let end = (2 * n as i32) << 16;
        let mut position = 0i32;
        let mut written = 0;
        for o in out.iter_mut() {
            if position >= end {
                break;
            }
            let phase = (((position & 0xffff) * 12) >> 16) as usize;
            let at = (position >> 16) as usize;
            let (Some(window), Some(taps)) = (
                up.get(at..at + TAPS)
                    .and_then(|w| <&[i16; TAPS]>::try_from(w).ok()),
                PHASES.get(phase),
            ) else {
                break;
            };
            let sum = window.iter().zip(taps).fold(0i32, |acc, (&x, &c)| {
                acc.wrapping_add(i32::from(x) * i32::from(c))
            });
            *o = sat16(round_shift(sum, 15));
            written += 1;
            position += self.step;
        }
        if let Some(src) = up.get(2 * n..2 * n + TAPS) {
            self.tail.copy_from_slice(src);
        }
        written
    }
}

/// Three first-order allpass sections in series, Q10; the last
/// coefficient is negative in the table and stands for itself plus 1.0.
fn allpass_chain(state: &mut [i32; 3], input: i32, coefficients: &[i16; 3]) -> i32 {
    let mut value = input;
    for (k, (s, &c)) in state.iter_mut().zip(coefficients).enumerate() {
        let c = i32::from(c) + if k == 2 { 1 << 16 } else { 0 };
        let x = mul16(value.wrapping_sub(*s), c);
        let out = s.wrapping_add(x);
        *s = value.wrapping_add(x);
        value = out;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at `hz` and full scale `amplitude`, upsampled.
    fn tone(rate: Rate, hz: f64, amplitude: f64, len: usize) -> Vec<i16> {
        let khz = rate.khz() as f64;
        let input: Vec<i16> = (0..len)
            .map(|n| {
                (amplitude * (2.0 * core::f64::consts::PI * hz * n as f64 / (khz * 1000.0)).sin())
                    as i16
            })
            .collect();
        let mut up = Upsampler::new(rate);
        let factor = 48 / rate.khz();
        let mut out = vec![0i16; len * factor];
        for (frame, o) in input
            .chunks(20 * rate.khz())
            .zip(out.chunks_mut(20 * rate.khz() * factor))
        {
            up.process(frame, o);
        }
        out
    }

    #[test]
    fn steps_cover_the_ratio() {
        // Rounded up: a batch of 10 ms gives 480 samples, one of 1 ms 48.
        for rate in [Rate::Narrow, Rate::Medium, Rate::Wide] {
            let up = Upsampler::new(rate);
            for ms in [1, 9, 10] {
                let end = (2 * ms * rate.khz() as i64) << 16;
                let outputs = (end + i64::from(up.step) - 1) / i64::from(up.step);
                assert_eq!(outputs, 48 * ms, "{rate:?} {ms} ms");
            }
        }
    }

    #[test]
    fn passes_the_band_and_keeps_the_level() {
        for rate in [Rate::Narrow, Rate::Medium, Rate::Wide] {
            let out = tone(rate, 500.0, 16384.0, 40 * rate.khz());
            let peak = out[960..]
                .iter()
                .map(|&y| i32::from(y).abs())
                .max()
                .unwrap();
            assert!((15500..=17300).contains(&peak), "{rate:?}: {peak}");
        }
    }
}
