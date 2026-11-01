//! The CELT layer (RFC 6716 4.3): a frame's header symbols, its band
//! energies ([`energy`]), the bit allocation ([`alloc`]), the bands' shapes
//! ([`bands`], [`pvq`]), then the synthesis: denormalization, the inverse
//! MDCT ([`mdct`]), the pitch post-filter and de-emphasis.
//!
//! Everything that decides which symbols are read is in the reference's
//! integer arithmetic, so a frame ends in the reference decoder's range
//! state; the signal itself is `f32` (RFC 6716 4.3 allows a decoder's
//! output to differ within the conformance test's tolerance).
//!
//! One [`Celt`] decodes one stream's frames, mono or stereo, to one or two
//! output channels at 48 kHz, as the Opus layer gives them: the coded
//! channels and the band range (the end from the bandwidth, the start at 17
//! in Hybrid mode) per frame.
//!
//! Choices, beyond what the reference code fixes:
//! - The signal is `f32`, as in the reference's floating-point build, whose
//!   output the RFC 8251 vectors' `.dec` set is (the RFC's code with RFC
//!   8251's changes reproduces it; `m.dec` is another decoder's). Rejected:
//!   the fixed-point build's arithmetic, which no bar needs and which is
//!   slower in Rust's scalar code.
//! - The inverse MDCT is ours ([`mdct`]): a DCT-IV through a mixed-radix
//!   FFT, matching the reference's output definition (its window and
//!   scale, measured against the reference's transform to within `f32`
//!   rounding). Rejected: a direct sum, O(n^2).
//! - Pulse vectors are decoded with one row of V(n, k), stepped down a
//!   dimension per coefficient ([`pvq`]); its sums stay below 2^32 as the
//!   pulse cache guarantees. Rejected: a table of V, 22 KB for little gain
//!   until the speed bar asks.
//! - A band's folding source is copied out of the folding buffer before the
//!   band is decoded, so the band can write the buffer it was read from
//!   without aliasing; the reference reads it in place. The copy costs the
//!   band's width.
//! - The post-filter's history is 1,024 samples, its longest reach; the
//!   reference keeps 2,048 for concealment's pitch search, which can grow
//!   it when concealment lands.
//! - Every read past a table or a buffer gives 0 instead of panicking; the
//!   bitstream bounds every index the reference relies on, and fuzzing
//!   (`opus_decode`) covers the rest.

mod alloc;
mod bands;
mod energy;
mod mdct;
mod pvq;

use crate::range::RangeDecoder;
use crate::tables::celt as t;
use mdct::OVERLAP;
pub(crate) use mdct::window;

/// The number of bands (4.3, Table 55).
pub(crate) const BANDS: usize = 21;

/// The samples of the longest frame, 20 ms.
pub(crate) const MAX_FRAME: usize = 960;

/// The samples of a 2.5 ms frame, the short MDCT's size.
const SHORT: usize = 120;

/// The post-filter's longest reach into the past: a period of 1,022 and
/// two more taps (4.3.7.1).
const HISTORY: usize = 1024;

/// The post-filter's shortest period.
const MIN_PERIOD: usize = 15;

/// The de-emphasis filter's pole (4.3.7.2), as the reference's `f32`.
const PREEMPHASIS: f32 = 0.850_006_1;

/// The post-filter's taps for each tapset, Q15 values (4.3.7.1).
const TAPS: [[f32; 3]; 3] = [
    [10048.0 / 32768.0, 7112.0 / 32768.0, 4248.0 / 32768.0],
    [15200.0 / 32768.0, 8784.0 / 32768.0, 0.0],
    [26208.0 / 32768.0, 3280.0 / 32768.0, 0.0],
];

/// `table[i]`, or the default past its end.
fn at<T: Copy + Default>(table: &[T], i: usize) -> T {
    table.get(i).copied().unwrap_or_default()
}

/// Sets `table[i]`, if it exists.
fn set<T>(table: &mut [T], i: usize, value: T) {
    if let Some(v) = table.get_mut(i) {
        *v = value;
    }
}

/// The first bin of `band` at 2.5 ms (the end of the last for 21).
fn edge(band: usize) -> usize {
    usize::try_from(at(&t::BAND_EDGES, band)).unwrap_or(0)
}

/// The width of `band` at 2.5 ms.
fn width(band: usize) -> usize {
    edge(band + 1).saturating_sub(edge(band))
}

/// The coded bands of a frame: `start` is 0, or 17 in Hybrid mode; `end`
/// follows the bandwidth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bands {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Bands {
    fn range(self) -> core::ops::Range<usize> {
        self.start..self.end
    }
}

/// What the Opus layer tells the CELT layer about a frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    /// log2 of the frame's size in 2.5 ms units, 0 to 3.
    pub(crate) lm: usize,
    /// Whether two channels are coded.
    pub(crate) stereo: bool,
    pub(crate) bands: Bands,
}

/// Working buffers of [`Celt::decode`]; each frame writes what it reads.
#[derive(Clone, Debug)]
struct Scratch {
    /// The frame's coefficients, per channel.
    x: [[f32; MAX_FRAME]; 2],
    folding: bands::Folding,
    mdct: mdct::Scratch,
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            x: [[0.0; MAX_FRAME]; 2],
            folding: bands::Folding::default(),
            mdct: mdct::Scratch::default(),
        }
    }
}

/// The pitch post-filter's parameters (4.3.7.1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PostFilter {
    period: usize,
    gain: f32,
    tapset: usize,
}

/// The state of a CELT decoder between frames.
#[derive(Clone, Debug)]
pub(crate) struct Celt {
    /// Per output channel: the post-filtered signal of the last
    /// [`HISTORY`] samples, then room for a frame and the inverse MDCT's
    /// overlap after it.
    signal: Box<[[f32; HISTORY + MAX_FRAME + OVERLAP]; 2]>,
    /// The bands' coefficients and the inverse MDCT's buffers, kept to
    /// spare clearing them for each frame.
    scratch: Box<Scratch>,
    /// The inverse MDCT's tail, to add to the next frame.
    overlap: [[f32; OVERLAP]; 2],
    /// The de-emphasis filter's state.
    deemphasis: [f32; 2],
    /// The post-filter of the previous frame and of the one before.
    filter: PostFilter,
    filter_old: PostFilter,
    /// The last frame's band energies, and the two before for
    /// anti-collapse (log2 of the amplitude, relative to the mean).
    energy: [[f32; BANDS]; 2],
    previous: [[f32; BANDS]; 2],
    previous2: [[f32; BANDS]; 2],
    /// The folding noise's state: the last frame's final range.
    seed: u32,
}

impl Default for Celt {
    fn default() -> Self {
        Self {
            signal: Box::new([[0.0; HISTORY + MAX_FRAME + OVERLAP]; 2]),
            scratch: Box::default(),
            overlap: [[0.0; OVERLAP]; 2],
            deemphasis: [0.0; 2],
            filter: PostFilter::default(),
            filter_old: PostFilter::default(),
            energy: [[0.0; BANDS]; 2],
            previous: [[-28.0; BANDS]; 2],
            previous2: [[-28.0; BANDS]; 2],
            seed: 0,
        }
    }
}

impl Celt {
    /// Forgets everything, as at the start of a stream.
    pub(crate) fn reset(&mut self) {
        self.signal.iter_mut().for_each(|s| s.fill(0.0));
        self.overlap = [[0.0; OVERLAP]; 2];
        self.deemphasis = [0.0; 2];
        self.filter = PostFilter::default();
        self.filter_old = PostFilter::default();
        self.energy = [[0.0; BANDS]; 2];
        self.previous = [[-28.0; BANDS]; 2];
        self.previous2 = [[-28.0; BANDS]; 2];
        self.seed = 0;
    }

    /// Decodes a frame from `rd` to `channels` output channels, 1 or 2,
    /// writing `120 << frame.lm` samples per channel to `out`, at full
    /// scale ±1.0.
    pub(crate) fn decode(
        &mut self,
        rd: &mut RangeDecoder<'_>,
        frame: Frame,
        channels: usize,
        out: &mut [[f32; MAX_FRAME]; 2],
    ) {
        let lm = frame.lm.min(3);
        let n = SHORT << lm;
        let coded = if frame.stereo { 2 } else { 1 };
        let outputs = channels.clamp(1, 2);
        let start = frame.bands.start.min(BANDS - 1);
        let bands = Bands {
            start,
            end: frame.bands.end.clamp(start + 1, BANDS),
        };

        if coded == 1 {
            // A mono frame predicts from the louder channel.
            let [e0, e1] = &mut self.energy;
            for (a, &b) in e0.iter_mut().zip(e1.iter()) {
                *a = a.max(b);
            }
        }

        // The header (4.3.1, 4.3.7.1).
        let total = rd.storage_bits() as i32;
        let mut tell = rd.tell() as i32;
        let silence = if tell >= total {
            true
        } else {
            tell == 1 && rd.bit_logp(15)
        };
        if silence {
            // As if every bit had been read.
            rd.skip_to_end();
            tell = total;
        }
        let mut filter = PostFilter::default();
        if bands.start == 0 && tell + 16 <= total {
            if rd.bit_logp(1) {
                let octave = rd.uint(6);
                filter.period = ((16u32 << octave) + rd.bits(4 + octave) - 1) as usize;
                let gain = rd.bits(3);
                if rd.tell() as i32 + 2 <= total {
                    filter.tapset = rd.icdf(&t::TAPSET_ICDF, 2);
                }
                filter.gain = 0.093_75 * (gain + 1) as f32;
            }
            tell = rd.tell() as i32;
        }
        // `tell` is not refreshed where the reference does not refresh it.
        let mut transient = false;
        if lm > 0 && tell + 3 <= total {
            transient = rd.bit_logp(3);
            tell = rd.tell() as i32;
        }
        let intra = tell + 3 <= total && rd.bit_logp(3);

        let energy = self.energy.get_mut(..coded).unwrap_or(&mut []);
        energy::decode_coarse(rd, bands, lm, intra, energy);
        let tf = time_frequency(rd, bands, transient, lm);
        let spread = if rd.tell() as i32 + 4 <= total {
            rd.icdf(&t::SPREAD_ICDF, 5)
        } else {
            2
        };

        // Boosts of single bands (4.3.3), then the trim.
        let caps = alloc::caps(lm, coded);
        let mut boost = [0; BANDS];
        let mut budget = total << 3;
        let mut tell = rd.tell_frac() as i32;
        let mut first_logp = 6;
        for band in bands.range() {
            let w = ((coded * width(band)) << lm) as i32;
            let quanta = (w << 3).min((6 << 3).max(w));
            let mut logp = first_logp;
            let mut b = 0;
            while tell + (logp << 3) < budget && b < at(&caps, band) {
                let more = rd.bit_logp(logp as u32);
                tell = rd.tell_frac() as i32;
                if !more {
                    break;
                }
                b += quanta;
                budget -= quanta;
                logp = 1;
            }
            set(&mut boost, band, b);
            if b > 0 {
                first_logp = (first_logp - 1).max(2);
            }
        }
        let trim = if tell + (6 << 3) <= budget {
            rd.icdf(&t::TRIM_ICDF, 7) as i32
        } else {
            5
        };

        let mut bits = (total << 3) - rd.tell_frac() as i32 - 1;
        let anti_collapse_reserve = if transient && lm >= 2 && bits >= ((lm as i32) + 2) << 3 {
            8
        } else {
            0
        };
        bits -= anti_collapse_reserve;
        let allocation = alloc::allocate(rd, bands, &boost, &caps, trim, bits, coded, lm);
        let energy = self.energy.get_mut(..coded).unwrap_or(&mut []);
        energy::decode_fine(rd, bands, &allocation.fine, energy);

        // The bands' shapes, then what is left for the energy.
        let Scratch {
            x,
            folding: buffers,
            mdct: scratch,
        } = &mut *self.scratch;
        let [x0, x1] = x;
        let mut rows: [&mut [f32]; 2] = [&mut x0[..], &mut x1[..]];
        let rows = rows.get_mut(..coded).unwrap_or(&mut []);
        let shape = bands::Frame {
            bands,
            lm,
            transient,
            spread,
            allocation: &allocation,
            tf: &tf,
            total: (total << 3) - anti_collapse_reserve,
        };
        let masks = bands::decode(rd, &shape, &mut self.seed, rows, buffers);
        let anti_collapse = anti_collapse_reserve > 0 && rd.bits(1) == 1;
        let left = total - rd.tell() as i32;
        let energy = self.energy.get_mut(..coded).unwrap_or(&mut []);
        energy::decode_final(
            rd,
            bands,
            &allocation.fine,
            &allocation.priority,
            left,
            energy,
        );
        if anti_collapse {
            bands::anti_collapse(
                rows,
                &masks,
                lm,
                bands,
                &self.energy,
                &self.previous,
                &self.previous2,
                &allocation.shape,
                self.seed,
            );
        }

        // Denormalization (4.3.6): each band times its amplitude.
        if silence {
            for e in self.energy.iter_mut().take(coded) {
                e.fill(-28.0);
            }
        }
        let m = 1 << lm;
        for (row, energy) in rows.iter_mut().zip(&self.energy) {
            // The buffer holds the last frame's values outside the coded
            // bands: they are cleared, not scaled.
            for band in 0..BANDS {
                let Some(bins) = row.get_mut(m * edge(band)..m * edge(band + 1)) else {
                    continue;
                };
                if silence || !bands.range().contains(&band) {
                    bins.fill(0.0);
                    continue;
                }
                let mean = f32::from(at(&t::ENERGY_MEANS, band)) / 16.0;
                let amplitude = crate::math::exp2((at(energy, band) + mean).min(32.0));
                bins.iter_mut().for_each(|v| *v *= amplitude);
            }
            if let Some(above) = row.get_mut(m * edge(BANDS)..) {
                above.fill(0.0);
            }
        }
        if outputs == 2 && coded == 1 {
            x1.copy_from_slice(x0);
        } else if outputs == 1 && coded == 2 {
            for (a, &b) in x0.iter_mut().zip(x1.iter()) {
                *a = 0.5 * (*a + b);
            }
        }

        // The inverse MDCT, the post-filter and de-emphasis, per channel.
        let blocks = if transient { m } else { 1 };
        let size = n / blocks;
        let window = mdct::window();
        let old = self.filter_old.clamped();
        let current = self.filter.clamped();
        let x: [&[f32]; 2] = [x0, x1];
        for (c, coeffs) in x.iter().enumerate().take(outputs) {
            let (Some(signal), Some(overlap), Some(memory), Some(out)) = (
                self.signal.get_mut(c),
                self.overlap.get_mut(c),
                self.deemphasis.get_mut(c),
                out.get_mut(c),
            ) else {
                continue;
            };
            // The frame goes after the history, its first samples the last
            // frame's overlap, which the inverse MDCT adds to.
            let frame = signal.get_mut(HISTORY..).unwrap_or(&mut []);
            if let Some(head) = frame.get_mut(..OVERLAP) {
                head.copy_from_slice(overlap);
            }
            for b in 0..blocks {
                let coeffs = coeffs.get(b..).unwrap_or(&[]);
                if let Some(block) = frame.get_mut(b * size..) {
                    mdct::inverse(coeffs, blocks, size, block, scratch);
                }
            }
            if let Some(tail) = frame.get(n..n + OVERLAP) {
                overlap.copy_from_slice(tail);
            }
            comb(signal, HISTORY, SHORT, old, current, window);
            if lm != 0 {
                comb(signal, HISTORY + SHORT, n - SHORT, current, filter, window);
            }
            *memory = deemphasis(
                signal.get(HISTORY..HISTORY + n).unwrap_or(&[]),
                *memory,
                out,
            );
            signal.copy_within(n..n + HISTORY, 0);
        }
        self.filter_old = current;
        self.filter = filter;
        if lm != 0 {
            self.filter_old = filter;
        }

        // The energies the next frames predict from.
        if coded == 1 {
            let [e0, e1] = &mut self.energy;
            e1.copy_from_slice(e0);
        }
        if transient {
            for (p, &e) in self
                .previous
                .iter_mut()
                .flatten()
                .zip(self.energy.iter().flatten())
            {
                *p = p.min(e);
            }
        } else {
            self.previous2 = self.previous;
            self.previous = self.energy;
        }
        for c in 0..2 {
            for band in (0..bands.start).chain(bands.end..BANDS) {
                for (table, value) in [
                    (&mut self.energy, 0.0),
                    (&mut self.previous, -28.0),
                    (&mut self.previous2, -28.0),
                ] {
                    if let Some(row) = table.get_mut(c) {
                        set(row, band, value);
                    }
                }
            }
        }
        self.seed = rd.range();
    }
}

impl PostFilter {
    /// The parameters with the period raised to its minimum, as a frame's
    /// filter is applied after that frame.
    fn clamped(self) -> Self {
        Self {
            period: self.period.max(MIN_PERIOD),
            ..self
        }
    }
}

/// De-emphasis (4.3.7.2), `y[i] = x[i] + PREEMPHASIS y[i - 1]`, of `x`
/// into `out` at full scale 1.0, from `memory`, `PREEMPHASIS` times the
/// last output; returns the new memory. Eight samples at a time, so that
/// the recurrence's chain is one step per eight: within a block each output
/// is its inputs' response plus a power of the pole times the memory.
/// Every frame is a multiple of eight samples.
fn deemphasis(x: &[f32], mut memory: f32, out: &mut [f32]) -> f32 {
    const C: f32 = PREEMPHASIS;
    const SCALE: f32 = 1.0 / 32768.0;
    const BLOCK: usize = 8;
    // C^k for k from 0 to BLOCK - 1: the memory is C times the last output.
    let powers: [f32; BLOCK] = core::array::from_fn(|k| (0..k).fold(1.0, |p, _| p * C));
    for (o, v) in out.chunks_exact_mut(BLOCK).zip(x.chunks_exact(BLOCK)) {
        // The block's response to its own inputs, from a zero memory.
        let mut p = [0f32; BLOCK];
        let mut last = 0.0;
        for (p, &v) in p.iter_mut().zip(v) {
            last = v + C * last;
            *p = last;
        }
        let mut y = 0.0;
        for ((o, &p), &power) in o.iter_mut().zip(&p).zip(&powers) {
            y = p + power * memory;
            *o = y * SCALE;
        }
        memory = C * y;
    }
    memory
}

/// The time-frequency resolution change of each band (4.3.4.5).
fn time_frequency(
    rd: &mut RangeDecoder<'_>,
    bands: Bands,
    transient: bool,
    lm: usize,
) -> [i32; BANDS] {
    let mut budget = rd.storage_bits();
    let mut tell = rd.tell();
    let mut logp = if transient { 2 } else { 4 };
    let reserve = lm > 0 && tell + logp < budget;
    budget -= u32::from(reserve);
    let mut flags = [0usize; BANDS];
    let mut changed = 0;
    let mut current = 0;
    for band in bands.range() {
        if tell + logp <= budget {
            current ^= usize::from(rd.bit_logp(logp));
            tell = rd.tell();
            changed |= current;
        }
        set(&mut flags, band, current);
        logp = if transient { 4 } else { 5 };
    }
    let row = t::TF_SELECT.get(lm).map_or(&[][..], |r| &r[..]);
    let base = 4 * usize::from(transient);
    let mut select = 0;
    if reserve && at(row, base + changed) != at(row, base + 2 + changed) {
        select = usize::from(rd.bit_logp(1));
    }
    let mut tf = [0; BANDS];
    for band in bands.range() {
        set(
            &mut tf,
            band,
            i32::from(at(row, base + 2 * select + at(&flags, band))),
        );
    }
    tf
}

/// The pitch post-filter (4.3.7.1) on `len` samples of `x` from `start`,
/// in place, so that it feeds back its own output: a comb filter of three
/// or five taps around the period, cross-fading from `from` to `to` over
/// the first [`OVERLAP`] samples with the window's square.
fn comb(
    x: &mut [f32],
    start: usize,
    len: usize,
    from: PostFilter,
    to: PostFilter,
    window: &[f32; OVERLAP],
) {
    if from.gain == 0.0 && to.gain == 0.0 {
        return;
    }
    let (from, to) = (from.clamped(), to.clamped());
    let taps = |f: PostFilter| {
        let t = TAPS.get(f.tapset).copied().unwrap_or_default();
        [f.gain * t[0], f.gain * t[1], f.gain * t[2]]
    };
    let (a, b) = (taps(from), taps(to));
    // A period beyond the history leaves the samples as they are.
    let reach = from.period.max(to.period) + 2;
    let Some(base) = start.checked_sub(reach) else {
        return;
    };
    let Some(x) = x.get_mut(base..start + len) else {
        return;
    };
    let overlap = OVERLAP.min(len);
    // Blocks of samples whose taps all lie before the block, at least the
    // period less two back, so they are already filtered, as in the
    // reference's in-place filter; the cross-fade's blocks end where it
    // does.
    let mut done = 0;
    while done < len {
        let fading = done < overlap;
        let period = if fading {
            from.period.min(to.period)
        } else {
            to.period
        };
        let end = if fading { overlap } else { len };
        let n = (end - done).min(period - 2);
        let i = reach + done;
        let (past, rest) = x.split_at_mut(i);
        let Some(block) = rest.get_mut(..n) else {
            return;
        };
        let source = |period: usize| past.get(i - period - 2..i - period + 2 + n).unwrap_or(&[]);
        let (sa, sb) = (source(from.period), source(to.period));
        if fading {
            let fades = window.get(done..done + n).unwrap_or(&[]);
            let taps = sa.windows(5).zip(sb.windows(5));
            for ((y, &w), (wa, wb)) in block.iter_mut().zip(fades).zip(taps) {
                let f = w * w;
                *y += (1.0 - f) * five(wa, a) + f * five(wb, b);
            }
        } else if to.gain == 0.0 {
            return;
        } else {
            let [g0, g1, g2] = b;
            let view = |d: usize| sb.get(d..d + n).unwrap_or(&[]);
            let taps = view(0)
                .iter()
                .zip(view(1))
                .zip(view(2))
                .zip(view(3))
                .zip(view(4));
            for (y, ((((&v0, &v1), &v2), &v3), &v4)) in block.iter_mut().zip(taps) {
                *y += g0 * v2 + g1 * (v1 + v3) + g2 * (v0 + v4);
            }
        }
        done += n;
    }
}

/// The post-filter's five taps around the middle sample of `x`.
#[inline(always)]
fn five(x: &[f32], [g0, g1, g2]: [f32; 3]) -> f32 {
    match x {
        &[x0, x1, x2, x3, x4] => g0 * x2 + g1 * (x1 + x3) + g2 * (x0 + x4),
        _ => 0.0,
    }
}
