//! Sample-rate conversion (`docs/design/audio-decode.md`, "Resampling and
//! downmixing"): a windowed-sinc low-pass evaluated at each output
//! sample's exact position in the input, in polyphase form.
//!
//! Output sample n is the band-limited input at time n / output rate, so
//! the output has no delay and a stream's timeline keeps its times. The
//! kernel is sinc(fc·u)·w(u / 16), u in samples of the lower of the two
//! rates, fc = 0.97 of its Nyquist frequency and w a Kaiser window of β =
//! 9: 32 taps at the lower rate, flat to within 0.002 dB up to 0.80 of the
//! lower Nyquist frequency and at least 89 dB down from 1.16 of it. Each
//! phase's taps are scaled to sum to 1, so a constant input stays constant.
//!
//! Every value is computed without the C math library (the series of
//! `sin` and of the Bessel function below), which `std`'s `sin` would link
//! into every build (the MP3 decoder's tables avoid it for the same
//! reason).

use crate::Error;
use coreav_kernels_audio::{Fir, LANES, dot};

/// Zero crossings of the kernel on each side, at the lower rate.
const HALF: f64 = 16.0;
/// The cutoff, a fraction of the lower rate's Nyquist frequency.
const CUTOFF: f64 = 0.97;
/// The Kaiser window's β.
const BETA: f64 = 9.0;
/// The largest exact phase table, in coefficients (256 KiB): the common
/// rate pairs need at most 21,000. Pairs with more phases evaluate the
/// kernel from a finely sampled table, linearly interpolated.
const TABLE_MAX: usize = 1 << 16;
/// Samples of the interpolated kernel per zero crossing: its linear
/// interpolation is exact to about 10^-6 of the kernel's peak, below the
/// stopband.
const STEPS: usize = 1024;
/// The largest ratio of input to output rate: its taps per output sample
/// are 32 times it.
pub(crate) const MAX_DOWNSAMPLING: u64 = 1024;

/// Kernel values computed together: each value's series are independent
/// of the others', so evaluating [`BLOCK`] of them in step overlaps their
/// latencies (and lets the compiler use vector instructions), every value
/// still computed by the same operations, in the same order, as alone.
const BLOCK: usize = 8;

/// sin(πx), from the nearest integer's parity and the Taylor series of
/// the remainder, |πr| ≤ π/2 (error below 10^-16).
#[cfg(test)]
fn sin_pi(x: f64) -> f64 {
    let [value, ..] = sin_pi_block([x; BLOCK]);
    value
}

/// [`sin_pi`] of each value.
fn sin_pi_block(x: [f64; BLOCK]) -> [f64; BLOCK] {
    let mut r = [0.0f64; BLOCK];
    let mut odd = [false; BLOCK];
    for ((&x, r), odd) in x.iter().zip(&mut r).zip(&mut odd) {
        let mut n = x as i64;
        let mut f = x - n as f64;
        if f > 0.5 {
            n += 1;
            f -= 1.0;
        } else if f < -0.5 {
            n -= 1;
            f += 1.0;
        }
        *r = f;
        *odd = n % 2 != 0;
    }
    let y = r.map(|r| core::f64::consts::PI * r);
    let y2 = y.map(|y| y * y);
    let (mut term, mut sum) = (y, y);
    for k in 1..13 {
        let k = f64::from(k);
        let d = (2.0 * k) * (2.0 * k + 1.0);
        for ((t, s), &y2) in term.iter_mut().zip(&mut sum).zip(&y2) {
            *t *= -y2 / d;
            *s += *t;
        }
    }
    for (s, &odd) in sum.iter_mut().zip(&odd) {
        if odd {
            *s = -*s;
        }
    }
    sum
}

/// The modified Bessel function of the first kind, order 0, from its
/// series Σ ((x/2)^k / k!)².
fn bessel_i0(x: f64) -> f64 {
    let [value, ..] = bessel_i0_block([x; BLOCK]);
    value
}

/// [`bessel_i0`] of each value: a value's series stops at the first term
/// below 10^-17 of its sum, the others go on.
#[expect(
    clippy::neg_cmp_op_on_partial_ord,
    reason = "a NaN series goes on, as in bessel_i0; the plain comparison vectorizes"
)]
fn bessel_i0_block(x: [f64; BLOCK]) -> [f64; BLOCK] {
    let q = x.map(|x| x * x / 4.0);
    let mut term = [1.0f64; BLOCK];
    let mut sum = [1.0f64; BLOCK];
    let mut live = [true; BLOCK];
    for k in 1..200 {
        let k = f64::from(k);
        let kk = k * k;
        for (((t, s), live), &q) in term.iter_mut().zip(&mut sum).zip(&mut live).zip(&q) {
            *t *= q / kk;
            let next = *s + *t;
            *s = if *live { next } else { *s };
            *live = *live && !(*t < *s * 1e-17);
        }
        if !live.contains(&true) {
            break;
        }
    }
    sum
}

/// The kernel at each `u` samples of the lower rate from its centre;
/// `i0_beta` is `bessel_i0(BETA)`, the window's scale.
fn kernel_block(u: [f64; BLOCK], i0_beta: f64) -> [f64; BLOCK] {
    let x = u.map(|u| u / HALF);
    // NaN excluded.
    let inside = x.map(|x| x.abs() < 1.0);
    let mut arg = [0.0f64; BLOCK];
    let mut a = [0.0f64; BLOCK];
    for ((((arg, a), &x), &u), &inside) in arg.iter_mut().zip(&mut a).zip(&x).zip(&u).zip(&inside) {
        if inside {
            *arg = BETA * (1.0 - x * x).sqrt();
            *a = CUTOFF * u;
        }
    }
    let window = bessel_i0_block(arg);
    let sin = sin_pi_block(a);
    let mut out = [0.0f64; BLOCK];
    for ((((o, &w), &s), &a), &inside) in out.iter_mut().zip(&window).zip(&sin).zip(&a).zip(&inside)
    {
        if inside {
            let sinc = if a == 0.0 {
                1.0
            } else {
                s / (core::f64::consts::PI * a)
            };
            *o = sinc * (w / i0_beta);
        }
    }
    out
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// How the taps of a phase are found.
enum Taps {
    /// Every phase's taps, row by row, from `data[at..]`, the first float
    /// on a 32-byte boundary so that no row's 8-float loads straddle a
    /// cache line; and one period of the windows: output j of every
    /// `phases` (from an output of phase 0) has its window `period[j].0`
    /// input samples after the period's first window and its taps at
    /// `period[j].1`. Outputs `phases` apart have the same row, `step`
    /// input samples apart.
    Table {
        data: Vec<f32>,
        at: usize,
        period: Vec<(usize, usize)>,
    },
    /// The kernel sampled [`STEPS`] times per zero crossing; a phase's taps
    /// are interpolated into `row` when needed.
    Interpolated { kernel: Vec<f32>, row: Vec<f32> },
}

/// The resampler of one stream, all its channels at the same position.
pub(crate) struct Resampler {
    /// The rates' ratio in lowest terms: `step` input samples for `phases`
    /// output samples.
    step: u64,
    phases: u64,
    /// The scale from input samples to samples of the lower rate.
    scale: f64,
    /// Taps before the input position (inclusive): the window starts at
    /// `position - (before - 1)`.
    before: usize,
    taps: usize,
    coefficients: Taps,
    /// The filter loop the CPU runs.
    fir: Fir,
    /// Each output's window start and row in the table, for one call of
    /// [`Self::produce`].
    windows: Vec<(usize, usize)>,
    /// Each channel's input from absolute input sample `base` on; the
    /// input before sample 0 and after the last is silence.
    inputs: Vec<Vec<f32>>,
    base: i64,
    /// The next output sample's input position: `position + phase / phases`.
    position: i64,
    phase: u64,
    /// Input samples per channel received.
    received: u64,
    /// Output samples per channel produced.
    produced: u64,
}

impl Resampler {
    /// A resampler from `from` Hz to `to` Hz for `channels` channels.
    pub(crate) fn new(from: u32, to: u32, channels: usize) -> Result<Self, Error> {
        check_rates(from, to)?;
        let (from, to) = (u64::from(from), u64::from(to));
        let g = gcd(from, to);
        let (step, phases) = (from / g, to / g);
        let scale = if to < from {
            to as f64 / from as f64
        } else {
            1.0
        };
        // The kernel's half-width in input samples is W = HALF / scale; the
        // input samples strictly within W of an output's time are the
        // ceil(W) - 1 before its position, the position, and the ceil(W)
        // after it.
        let half_width = HALF / scale;
        let before = match half_width as usize {
            w if (w as f64) < half_width => w + 1,
            w => w,
        };
        let taps = (2 * before).div_ceil(LANES) * LANES;
        let coefficients = match usize::try_from(phases)
            .ok()
            .and_then(|p| p.checked_mul(taps))
            .filter(|&n| n <= TABLE_MAX)
        {
            Some(n) => {
                // Room to start the table on a 32-byte boundary.
                let mut data = vec![0.0f32; n + LANES - 1];
                let at = data.as_ptr().align_offset(32).min(LANES - 1);
                let table = data.get_mut(at..at + n).unwrap_or_default();
                let i0_beta = bessel_i0(BETA);
                for (p, row) in table.chunks_exact_mut(taps).enumerate() {
                    let phase = p as f64 / phases as f64;
                    fill_row(
                        row,
                        |d| kernel_block(d.map(|d| d * scale), i0_beta),
                        phase,
                        before,
                    );
                }
                let period = (0..phases)
                    .map(|j| {
                        let (offset, phase) = (j * step / phases, j * step % phases);
                        let at = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
                        (at(offset), at(phase).saturating_mul(taps))
                    })
                    .collect();
                Taps::Table { data, at, period }
            }
            None => {
                let len = 2 * (HALF as usize) * STEPS + 2;
                let i0_beta = bessel_i0(BETA);
                let mut kernel = vec![0.0f32; len];
                for (b, values) in kernel.chunks_mut(BLOCK).enumerate() {
                    let u = core::array::from_fn(|i| (b * BLOCK + i) as f64 / STEPS as f64 - HALF);
                    for (v, k) in values.iter_mut().zip(kernel_block(u, i0_beta)) {
                        *v = k as f32;
                    }
                }
                Taps::Interpolated {
                    kernel,
                    row: vec![0.0; taps],
                }
            }
        };
        let silence = before - 1;
        Ok(Self {
            step,
            phases,
            scale,
            before,
            taps,
            coefficients,
            fir: Fir::detect(),
            windows: Vec::new(),
            inputs: (0..channels).map(|_| vec![0.0; silence]).collect(),
            base: -(silence as i64),
            position: 0,
            phase: 0,
            received: 0,
            produced: 0,
        })
    }

    /// Each channel's input, to append samples to; every channel gets the
    /// same number before [`Self::available`].
    pub(crate) fn inputs_mut(&mut self) -> &mut [Vec<f32>] {
        &mut self.inputs
    }

    /// Output samples per channel that the input received allows.
    pub(crate) fn available(&mut self) -> usize {
        let len = self.inputs.first().map_or(0, Vec::len) as i64;
        self.received = (self.base + len).max(0) as u64;
        self.count_until(self.base + len, u64::MAX)
    }

    /// Output samples per channel whose windows end before input sample
    /// `end`, at most `limit` more in all.
    fn count_until(&self, end: i64, limit: u64) -> usize {
        // The window of the output at `position` ends at `position -
        // before + taps`; output samples advance by `step / phases` input
        // samples. Counted exactly: the outputs n ≥ produced with
        // floor(n·step / phases) ≤ end - 1 - (taps - before).
        let last = end - 1 - (self.taps - self.before) as i64;
        if last < 0 {
            return 0;
        }
        // Outputs whose position is at most `last`: n·step < (last + 1)·phases.
        let bound = (u128::from(last as u64) + 1) * u128::from(self.phases);
        let total = bound.div_ceil(u128::from(self.step));
        let total = u64::try_from(total).unwrap_or(u64::MAX).min(limit);
        usize::try_from(total.saturating_sub(self.produced)).unwrap_or(usize::MAX)
    }

    /// The output samples per channel the whole input makes: one for each
    /// output time before the input's end, ceil(input × to / from).
    fn total(&self) -> u64 {
        let product = u128::from(self.received) * u128::from(self.phases);
        u64::try_from(product.div_ceil(u128::from(self.step))).unwrap_or(u64::MAX)
    }

    /// The output position of input position `samples`: the first output
    /// sample at or after it, ceil(samples × to / from).
    pub(crate) fn output_position(&self, samples: u64) -> u64 {
        let product = u128::from(samples) * u128::from(self.phases);
        u64::try_from(product.div_ceil(u128::from(self.step))).unwrap_or(u64::MAX)
    }

    /// The end of the input: the samples it still allows.
    pub(crate) fn finish(&mut self) -> usize {
        let len = self.inputs.first().map_or(0, Vec::len);
        self.received = (self.base + len as i64).max(0) as u64;
        let total = self.total();
        for input in &mut self.inputs {
            input.resize(len + self.taps, 0.0);
        }
        let end = self.base + (len + self.taps) as i64;
        self.count_until(end, total)
    }

    /// Writes the next `count` output samples of every channel into `out`,
    /// planar (`count` samples of channel 0, then of channel 1, ...), and
    /// drops the input no later output needs. `count` comes from
    /// [`Self::available`] or [`Self::finish`].
    pub(crate) fn produce(&mut self, count: usize, out: &mut [f32]) {
        let taps = self.taps;
        let first = self.position - (self.before as i64 - 1) - self.base;
        let mut start = usize::try_from(first).unwrap_or(0);
        let (q, r) = (
            usize::try_from(self.step / self.phases).unwrap_or(usize::MAX),
            self.step % self.phases,
        );
        let mut phase = self.phase;
        match &mut self.coefficients {
            Taps::Table { data, at, period } => {
                // Each output's window start and phase row, then the dot
                // products channel by channel. Output n's window starts
                // floor(n·step / phases) - (before - 1) input samples after
                // sample 0; counted from its period's first output, the
                // window's offset and row repeat.
                let table = data.get(*at..).unwrap_or_default();
                let n = self.produced;
                let behind = self.before as i64 - 1 + self.base;
                let mut j = usize::try_from(n % self.phases).unwrap_or(0);
                let mut origin = ((n / self.phases).saturating_mul(self.step) as i64) - behind;
                // A period at a time: each window is its period's start
                // plus the period's offset.
                self.windows.clear();
                // One phase (rates in an integer ratio): every window
                // `step` input samples after the one before, listed in one
                // loop.
                if let [(offset, row)] = period.as_slice() {
                    let step = self.step as i64;
                    self.windows.extend((0..count).map(|k| {
                        let base = origin.saturating_add((k as i64).saturating_mul(step)) as usize;
                        (base.wrapping_add(*offset), *row)
                    }));
                }
                while self.windows.len() < count {
                    let take = (count - self.windows.len()).min(period.len().saturating_sub(j));
                    let Some(part) = period.get(j..j + take).filter(|p| !p.is_empty()) else {
                        break;
                    };
                    // `origin` may be before the buffer, never a window: a
                    // start that wrapped would lie outside the input.
                    let base = origin as usize;
                    self.windows.extend(
                        part.iter()
                            .map(|&(offset, row)| (base.wrapping_add(offset), row)),
                    );
                    j += take;
                    if j == period.len() {
                        j = 0;
                        origin = origin.saturating_add(self.step as i64);
                    }
                }
                // Were the period to end early (it cannot: j < phases), the
                // missing windows would lie outside the input and the
                // outputs stay silent below.
                self.windows.resize(count, (usize::MAX, 0));
                // The next output's window and phase.
                let end = u128::from(n) + count as u128;
                let (step, phases) = (u128::from(self.step), u128::from(self.phases));
                let position = i64::try_from(end * step / phases).unwrap_or(i64::MAX);
                start = usize::try_from(position.saturating_sub(behind)).unwrap_or(0);
                phase = u64::try_from(end * step % phases).unwrap_or(0);
                for (input, plane) in self.inputs.iter().zip(out.chunks_exact_mut(count.max(1))) {
                    // Every window lies inside the input (`count` came from
                    // `available` or `finish`) and the table; were one
                    // not, the outputs would stay silent.
                    if self
                        .fir
                        .run(input, table, taps, &self.windows, plane)
                        .is_err()
                    {
                        plane.fill(0.0);
                    }
                }
            }
            Taps::Interpolated { kernel, row } => {
                for n in 0..count {
                    let fraction = phase as f64 / self.phases as f64;
                    let scale = self.scale;
                    fill_row(
                        row,
                        |d| d.map(|d| interpolate(kernel, d * scale)),
                        fraction,
                        self.before,
                    );
                    for (c, input) in self.inputs.iter().enumerate() {
                        let x = input.get(start..start + taps).unwrap_or_default();
                        if let Some(o) = out.get_mut(c * count + n) {
                            *o = dot(x, row);
                        }
                    }
                    start = start.saturating_add(q);
                    phase += r;
                    if phase >= self.phases {
                        phase -= self.phases;
                        start = start.saturating_add(1);
                    }
                }
            }
        }
        self.phase = phase;
        self.position = self.base + (start as i64) + (self.before as i64 - 1);
        self.produced += count as u64;
        // Input before the next window's start is not needed again; it is
        // dropped once it is half the buffer, so each sample moves O(1)
        // times.
        let len = self.inputs.first().map_or(0, Vec::len);
        if start > 4096 && start * 2 > len {
            let drop = start.min(len);
            for input in &mut self.inputs {
                input.drain(..drop);
            }
            self.base += drop as i64;
        }
    }
}

/// Whether [`Resampler::new`] takes the rates `from` and `to`, without
/// building its table.
pub(crate) fn check_rates(from: u32, to: u32) -> Result<(), Error> {
    let (from, to) = (u64::from(from), u64::from(to));
    if from == 0 || to == 0 {
        return Err(Error::Invalid("a sample rate of 0"));
    }
    if from > to.saturating_mul(MAX_DOWNSAMPLING) {
        return Err(Error::Unsupported(
            "an output sample rate below 1/1024 of the stream's",
        ));
    }
    Ok(())
}

/// Fills one phase's taps: tap j weighs the input sample `phase + before -
/// 1 - j` input samples before the output's time; the taps are then scaled
/// to sum to 1.
fn fill_row(
    row: &mut [f32],
    kernel: impl Fn([f64; BLOCK]) -> [f64; BLOCK],
    phase: f64,
    before: usize,
) {
    let mut sum = 0.0f64;
    for (b, taps) in row.chunks_mut(BLOCK).enumerate() {
        let d = core::array::from_fn(|i| phase + (before as f64 - 1.0) - (b * BLOCK + i) as f64);
        for (tap, value) in taps.iter_mut().zip(kernel(d)) {
            sum += value;
            *tap = value as f32;
        }
    }
    if sum != 0.0 {
        let scale = (1.0 / sum) as f32;
        row.iter_mut().for_each(|t| *t *= scale);
    }
}

/// The sampled kernel at `u`, linearly interpolated; 0 outside it.
fn interpolate(kernel: &[f32], u: f64) -> f64 {
    let at = (u + HALF) * STEPS as f64;
    if at.is_nan() || at < 0.0 {
        return 0.0;
    }
    let i = at as usize;
    let frac = at - i as f64;
    match (kernel.get(i), kernel.get(i + 1)) {
        (Some(&a), Some(&b)) => f64::from(a) + (f64::from(b) - f64::from(a)) * frac,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_match_std() {
        for i in -4000..4000 {
            let x = f64::from(i) / 97.0;
            // std rounds πx first: an error of about |πx| · 2^-53.
            assert!(
                (sin_pi(x) - (core::f64::consts::PI * x).sin()).abs() < 1e-13,
                "{x}"
            );
        }
        // I0(9) and I0(1), to 15 digits.
        assert!((bessel_i0(9.0) / 1_093.588_354_511_374_5 - 1.0).abs() < 1e-14);
        assert!((bessel_i0(1.0) / 1.266_065_877_752_008_4 - 1.0).abs() < 1e-14);
    }

    /// The kernel one value at a time, each series in a plain loop: the
    /// blocks compute every value bit for bit as this does.
    fn kernel_alone(u: f64) -> f64 {
        fn sin_pi(x: f64) -> f64 {
            let mut n = x as i64;
            let mut r = x - n as f64;
            if r > 0.5 {
                n += 1;
                r -= 1.0;
            } else if r < -0.5 {
                n -= 1;
                r += 1.0;
            }
            let y = core::f64::consts::PI * r;
            let y2 = y * y;
            let (mut term, mut sum) = (y, y);
            for k in 1..13 {
                let k = f64::from(k);
                term *= -y2 / ((2.0 * k) * (2.0 * k + 1.0));
                sum += term;
            }
            if n % 2 == 0 { sum } else { -sum }
        }
        fn i0(x: f64) -> f64 {
            let q = x * x / 4.0;
            let (mut term, mut sum) = (1.0, 1.0);
            for k in 1..200 {
                let k = f64::from(k);
                term *= q / (k * k);
                sum += term;
                if term < sum * 1e-17 {
                    break;
                }
            }
            sum
        }
        let x = u / HALF;
        if x.is_nan() || x.abs() >= 1.0 {
            return 0.0;
        }
        let window = i0(BETA * (1.0 - x * x).sqrt()) / i0(BETA);
        let a = CUTOFF * u;
        let sinc = if a == 0.0 {
            1.0
        } else {
            sin_pi(a) / (core::f64::consts::PI * a)
        };
        sinc * window
    }

    #[test]
    fn blocks_match_one_at_a_time() {
        let i0_beta = bessel_i0(BETA);
        let mut u: Vec<f64> = (-180_000..180_000)
            .map(|i| f64::from(i) / 9_973.0)
            .collect();
        u.extend([0.0, -0.0, 16.0, -16.0, 15.999_999, f64::NAN, f64::INFINITY]);
        // Every value in every position of a block.
        for start in 0..BLOCK {
            for chunk in u[start..].chunks(BLOCK) {
                let mut block = [1e300; BLOCK];
                block[..chunk.len()].copy_from_slice(chunk);
                let values = kernel_block(block, i0_beta);
                for (&u, &v) in chunk.iter().zip(&values) {
                    assert_eq!(v.to_bits(), kernel_alone(u).to_bits(), "{u}");
                }
            }
        }
    }

    /// Resamples `input` (one channel) in chunks of `chunk` samples.
    fn run(from: u32, to: u32, input: &[f32], chunk: usize) -> Vec<f32> {
        let mut r = Resampler::new(from, to, 1).unwrap();
        let mut out = Vec::new();
        for piece in input.chunks(chunk.max(1)) {
            r.inputs_mut()[0].extend_from_slice(piece);
            let n = r.available();
            let at = out.len();
            out.resize(at + n, 0.0);
            r.produce(n, &mut out[at..]);
        }
        let n = r.finish();
        let at = out.len();
        out.resize(at + n, 0.0);
        r.produce(n, &mut out[at..]);
        out
    }

    /// Producing the outputs a piece at a time gives the same samples as
    /// all at once.
    #[test]
    fn produced_in_pieces() {
        let input: Vec<f32> = (0..20_011)
            .map(|i| ((i * 7919) % 2003) as f32 / 2003.0 - 0.5)
            .collect();
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (16_000, 44_100),
            (44_100, 44_056),
        ] {
            let whole = run(from, to, &input, input.len());
            let mut r = Resampler::new(from, to, 1).unwrap();
            r.inputs_mut()[0].extend_from_slice(&input);
            let mut out = Vec::new();
            let mut take = |r: &mut Resampler, mut n: usize| {
                let mut piece = 1;
                while n > 0 {
                    let k = piece.min(n);
                    let at = out.len();
                    out.resize(at + k, 0.0);
                    r.produce(k, &mut out[at..]);
                    n -= k;
                    piece = piece * 3 + 1;
                }
            };
            let n = r.available();
            take(&mut r, n);
            let n = r.finish();
            take(&mut r, n);
            assert_eq!(out, whole, "{from} {to}");
        }
    }

    #[test]
    fn length_and_chunking() {
        let input: Vec<f32> = (0..10_007)
            .map(|i| ((i * 7919) % 2003) as f32 / 2003.0 - 0.5)
            .collect();
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (16_000, 48_000),
            (44_100, 48_000),
            (44_100, 7_999),
            (1, 3),
        ] {
            let whole = run(from, to, &input, input.len());
            let expect = (input.len() as u64 * u64::from(to)).div_ceil(u64::from(from));
            assert_eq!(whole.len() as u64, expect, "{from} {to}");
            for chunk in [1, 7, 1000, 4097] {
                assert_eq!(run(from, to, &input, chunk), whole, "{from} {to} {chunk}");
            }
        }
        assert!(run(48_000, 16_000, &[], 1).is_empty());
    }

    #[test]
    fn constant_stays_constant() {
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (16_000, 44_100),
            (44_100, 7_999),
        ] {
            let out = run(from, to, &vec![0.25; 20_000], 999);
            let n = out.len();
            for v in &out[n / 4..3 * n / 4] {
                assert!((v - 0.25).abs() < 1e-6, "{from} {to} {v}");
            }
        }
    }

    /// The fitted amplitude of a tone of `f` cycles per sample in the middle
    /// half of `y`, and the RMS of what is left.
    fn fit(y: &[f32], f: f64) -> (f64, f64) {
        let n = y.len();
        let part = &y[n / 4..3 * n / 4];
        let (mut ss, mut cc, mut sc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, &v) in part.iter().enumerate() {
            let t = 2.0 * core::f64::consts::PI * f * (i + n / 4) as f64;
            let (s, c) = t.sin_cos();
            let v = f64::from(v);
            (ss, cc, sc, ys, yc) = (ss + s * s, cc + c * c, sc + s * c, ys + v * s, yc + v * c);
        }
        let det = ss * cc - sc * sc;
        let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
        let mut rest = 0.0;
        for (i, &v) in part.iter().enumerate() {
            let t = 2.0 * core::f64::consts::PI * f * (i + n / 4) as f64;
            rest += (f64::from(v) - a * t.sin() - b * t.cos()).powi(2);
        }
        ((a * a + b * b).sqrt(), (rest / part.len() as f64).sqrt())
    }

    /// The quality bar of the design note (bar R1): every pair of common
    /// rates, and pairs that take the interpolated kernel. A tone up to 0.80
    /// of the lower Nyquist frequency keeps its level within ±0.005 dB, with
    /// everything else at least 85 dB below it; when downsampling, a tone
    /// from 1.17 of it up leaves at most -85 dB.
    #[test]
    fn passband_and_stopband() {
        let rates = [
            8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 96_000,
        ];
        let mut pairs: Vec<(u32, u32)> = rates
            .iter()
            .flat_map(|&a| rates.iter().map(move |&b| (a, b)))
            .filter(|(a, b)| a != b)
            .collect();
        pairs.extend([
            (44_100, 44_056),
            (7_919, 16_000),
            (192_000, 16_000),
            (16_000, 7_919),
        ]);
        let db = |x: f64| 20.0 * x.log10();
        for (from, to) in pairs {
            let nyquist = f64::from(from.min(to)) / 2.0;
            // About 3,000 output samples.
            let len = (3_000 * u64::from(from) / u64::from(to)) as usize;
            for share in [0.05, 0.3, 0.6, 0.8, 1.17, 1.3, 1.6] {
                let hz = share * nyquist;
                if hz >= 0.98 * f64::from(from) / 2.0 || (share > 1.0 && to > from) {
                    continue;
                }
                let input: Vec<f32> = (0..len)
                    .map(|i| {
                        (0.5 * (2.0 * core::f64::consts::PI * hz * i as f64 / f64::from(from))
                            .sin()) as f32
                    })
                    .collect();
                let out = run(from, to, &input, 1000);
                if share <= 0.8 {
                    let (amplitude, rest) = fit(&out, hz / f64::from(to));
                    let gain = db(amplitude / 0.5);
                    assert!(
                        gain.abs() <= 0.005,
                        "{from} -> {to}, {share}: gain {gain} dB"
                    );
                    assert!(
                        db(rest / amplitude * 2f64.sqrt()) <= -85.0,
                        "{from} -> {to}, {share}: rest {} dB",
                        db(rest / amplitude * 2f64.sqrt())
                    );
                } else {
                    let n = out.len();
                    let rms = (out[n / 4..3 * n / 4]
                        .iter()
                        .map(|&v| f64::from(v).powi(2))
                        .sum::<f64>()
                        / (n / 2) as f64)
                        .sqrt();
                    assert!(
                        db(rms / (0.5 / 2f64.sqrt())) <= -85.0,
                        "{from} -> {to}, {share}: {} dB",
                        db(rms / (0.5 / 2f64.sqrt()))
                    );
                }
            }
        }
    }

    #[test]
    fn limits() {
        assert!(Resampler::new(0, 16_000, 1).is_err());
        assert!(Resampler::new(768_000, 749, 1).is_err());
        assert!(Resampler::new(768_000, 750, 1).is_ok());
    }
}
