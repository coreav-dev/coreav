//! One channel's coded data: `ics_info()`, `section_data()`,
//! `scale_factor_data()`, `pulse_data()`, `tns_data()` and
//! `spectral_data()` (ISO/IEC 14496-3 4.4.2.7, 4.5.2.3), decoded to its
//! spectrum: Huffman decoding (4.6.3), pulses, inverse quantization (4.6.1)
//! and scalefactors (4.6.2). Noise, stereo and coupling come after, in
//! `tools`, once every channel of the frame is read.

// `as_chunks` (Rust 1.88) is within the workspace's rust-version, 1.89;
// clippy's version-aware lints keep to 1.85 (clippy.toml).
#![allow(clippy::incompatible_msrv)]

use crate::Error;
use crate::bits::{Bits, Cache};
use crate::tables::{
    BANDS_LONG_960, BANDS_LONG_1024, BANDS_SHORT_120, BANDS_SHORT_128, EIGHTH_POWERS, Lookup,
    POW43, SCALEFACTOR, SIGNS, SPECTRUM_BASE, SPECTRUM_ENTRIES, SPECTRUM_VECTORS, TNS_COEF_3,
    TNS_COEF_4, TNS_MAX_BANDS_LONG, TNS_MAX_BANDS_SHORT, VECTOR_BASE,
};

pub(crate) const LONG_START: u8 = 1;
pub(crate) const EIGHT_SHORT: u8 = 2;
pub(crate) const LONG_STOP: u8 = 3;

pub(crate) const ZERO_HCB: u8 = 0;
pub(crate) const ESC_HCB: u8 = 11;
pub(crate) const NOISE_HCB: u8 = 13;
pub(crate) const INTENSITY_HCB2: u8 = 14;
pub(crate) const INTENSITY_HCB: u8 = 15;

/// Scalefactor bands of a window, at most (51 at 32 kHz).
pub(crate) const MAX_BANDS: usize = 64;
/// TNS_MAX_ORDER of AAC LC, long and short windows (Table 4.156).
const TNS_MAX_ORDER_LONG: usize = 12;
const TNS_MAX_ORDER_SHORT: usize = 7;

/// The band layout of a stream: its frame length and band tables.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bands {
    /// Samples per frame: 1,024 or 960.
    pub(crate) frame: usize,
    /// Band offsets with the end, of long and of short windows.
    pub(crate) long: &'static [u16],
    pub(crate) short: &'static [u16],
    pub(crate) tns_long: usize,
    pub(crate) tns_short: usize,
}

impl Bands {
    pub(crate) fn new(rate_index: usize, frame: usize) -> Self {
        let (long, short) = if frame == 960 {
            (&BANDS_LONG_960, &BANDS_SHORT_120)
        } else {
            (&BANDS_LONG_1024, &BANDS_SHORT_128)
        };
        Self {
            frame,
            long: long.get(rate_index).copied().unwrap_or_default(),
            short: short.get(rate_index).copied().unwrap_or_default(),
            tns_long: TNS_MAX_BANDS_LONG.get(rate_index).copied().unwrap_or(0) as usize,
            tns_short: TNS_MAX_BANDS_SHORT.get(rate_index).copied().unwrap_or(0) as usize,
        }
    }

    /// Samples of a short window.
    pub(crate) fn short_len(&self) -> usize {
        self.frame / 8
    }
}

/// `ics_info()`, with the window groups it gives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Info {
    pub(crate) window_sequence: u8,
    pub(crate) window_shape: u8,
    pub(crate) max_sfb: usize,
    pub(crate) groups: usize,
    /// Windows in each group (one group of one window for long ones).
    pub(crate) group_len: [u8; 8],
}

impl Info {
    pub(crate) fn short(&self) -> bool {
        self.window_sequence == EIGHT_SHORT
    }

    /// Reads `ics_info()` (Table 4.6).
    pub(crate) fn read(bits: &mut Bits<'_>, bands: &Bands) -> Result<Self, Error> {
        if bits.bit() {
            return Err(Error::Invalid("ics_reserved_bit set"));
        }
        let window_sequence = bits.read(2) as u8;
        let window_shape = bits.read(1) as u8;
        let mut info = Self {
            window_sequence,
            window_shape,
            ..Self::default()
        };
        if window_sequence == EIGHT_SHORT {
            info.max_sfb = bits.read(4) as usize;
            let grouping = bits.read(7);
            info.groups = 1;
            info.group_len[0] = 1;
            for w in 1..8 {
                // Bit 6 is window 1's: set, window w joins the group before.
                if (grouping >> (7 - w)) & 1 == 1 {
                    if let Some(len) = info.group_len.get_mut(info.groups - 1) {
                        *len += 1;
                    }
                } else {
                    if let Some(len) = info.group_len.get_mut(info.groups) {
                        *len = 1;
                    }
                    info.groups += 1;
                }
            }
            if info.max_sfb > bands.short.len().saturating_sub(1) {
                return Err(Error::Invalid("max_sfb beyond the short window's bands"));
            }
        } else {
            info.max_sfb = bits.read(6) as usize;
            info.groups = 1;
            info.group_len[0] = 1;
            if bits.bit() {
                // predictor_data_present: AAC Main's prediction or LTP,
                // which an AAC LC stream does not carry (damage).
                return Err(Error::Invalid("prediction in an AAC LC stream"));
            }
            if info.max_sfb > bands.long.len().saturating_sub(1) {
                return Err(Error::Invalid("max_sfb beyond the long window's bands"));
            }
        }
        Ok(info)
    }

    /// The band offsets of this window sequence.
    pub(crate) fn offsets(&self, bands: &Bands) -> &'static [u16] {
        if self.short() {
            bands.short
        } else {
            bands.long
        }
    }
}

/// One TNS filter, its coefficients converted to LPC (4.6.9.3).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TnsFilter {
    pub(crate) length: usize,
    /// Order applied: the coded order capped at TNS_MAX_ORDER.
    pub(crate) order: usize,
    pub(crate) downward: bool,
    /// lpc\[1\] to lpc\[order\] at 0 to order - 1.
    pub(crate) lpc: [f32; TNS_MAX_ORDER_LONG],
}

/// `tns_data()` of a channel.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tns {
    pub(crate) present: bool,
    pub(crate) filters: [[TnsFilter; 3]; 8],
    pub(crate) count: [u8; 8],
}

impl Tns {
    /// Reads `tns_data()` (Table 4.54) and converts each filter to LPC
    /// coefficients (`tns_decode_coef()`).
    fn read(&mut self, bits: &mut Bits<'_>, info: &Info) {
        let short = info.short();
        let windows = if short { 8 } else { 1 };
        let (filt_bits, length_bits, order_bits, max_order) = if short {
            (1, 4, 3, TNS_MAX_ORDER_SHORT)
        } else {
            (2, 6, 5, TNS_MAX_ORDER_LONG)
        };
        self.present = true;
        for w in 0..windows {
            let count = bits.read(filt_bits) as usize;
            if let Some(slot) = self.count.get_mut(w) {
                *slot = count as u8;
            }
            if count == 0 {
                continue;
            }
            let res = bits.read(1) + 3;
            for f in 0..count {
                let length = bits.read(length_bits) as usize;
                let order = bits.read(order_bits) as usize;
                let mut filter = TnsFilter {
                    length,
                    order: order.min(max_order),
                    ..TnsFilter::default()
                };
                if order > 0 {
                    filter.downward = bits.bit();
                    let compress = bits.read(1);
                    let width = res - compress;
                    let mut parcor = [0f32; TNS_MAX_ORDER_LONG];
                    for i in 0..order {
                        let raw = bits.read(width);
                        // Sign-extend from `width` bits, then look up at the
                        // full resolution (the coefficient keeps its value).
                        let value = ((raw << (32 - width)) as i32) >> (32 - width);
                        let index = (value as u32 & ((1 << res) - 1)) as usize;
                        let table: &[f32] = if res == 4 { &TNS_COEF_4 } else { &TNS_COEF_3 };
                        if let Some(slot) = parcor.get_mut(i) {
                            *slot = table.get(index).copied().unwrap_or(0.0);
                        }
                    }
                    filter.lpc = to_lpc(&parcor, filter.order);
                }
                if let Some(slot) = self.filters.get_mut(w).and_then(|ws| ws.get_mut(f)) {
                    *slot = filter;
                }
            }
        }
    }
}

/// The conversion of reflection coefficients to LPC coefficients of
/// `tns_decode_coef()`: a\[1\] to a\[order\] at 0 to order - 1.
fn to_lpc(parcor: &[f32; TNS_MAX_ORDER_LONG], order: usize) -> [f32; TNS_MAX_ORDER_LONG] {
    // a[0] = 1 is implicit; a[i] is at i - 1.
    let mut a = [0f32; TNS_MAX_ORDER_LONG];
    for m in 1..=order {
        let k = parcor.get(m - 1).copied().unwrap_or(0.0);
        let mut b = a;
        for i in 1..m {
            let ai = a.get(i - 1).copied().unwrap_or(0.0);
            let ami = a.get(m - i - 1).copied().unwrap_or(0.0);
            if let Some(slot) = b.get_mut(i - 1) {
                *slot = ai + k * ami;
            }
        }
        a = b;
        if let Some(slot) = a.get_mut(m - 1) {
            *slot = k;
        }
    }
    a
}

/// One channel of a frame: its `ics_info()`, band codebooks and values
/// (scalefactors, intensity positions, noise energies), TNS data, and the
/// spectrum, window after window.
#[derive(Clone)]
pub(crate) struct Channel {
    pub(crate) info: Info,
    pub(crate) global_gain: i32,
    /// The codebook of each band, by group.
    pub(crate) cb: [[u8; MAX_BANDS]; 8],
    /// The scalefactor (0 to 255), the intensity position or the noise
    /// energy of each band, by group.
    pub(crate) sf: [[i32; MAX_BANDS]; 8],
    pub(crate) tns: Tns,
    pub(crate) spec: [f32; 1024],
    quant: [i32; 1024],
    /// Bands coded as noise (codebook 13), all groups.
    pub(crate) noise_bands: u32,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            info: Info::default(),
            global_gain: 0,
            cb: [[0; MAX_BANDS]; 8],
            sf: [[0; MAX_BANDS]; 8],
            tns: Tns::default(),
            spec: [0.0; 1024],
            quant: [0; 1024],
            noise_bands: 0,
        }
    }
}

/// The entry of the codeword at the start of `peek` in `table`: leaf
/// `payload << 5 | length`, the length that of the whole codeword (at most
/// 19 bits).
#[inline(always)]
fn lookup(peek: u32, table: &Lookup) -> u32 {
    let entries = table.entries;
    let level1 = table.level1;
    let entry = entries
        .get((peek >> (32 - level1)) as usize)
        .copied()
        .unwrap_or(0);
    if entry & 31 != 0 {
        return entry;
    }
    let sub = (entry >> 5) & 31;
    let index = (peek << level1).checked_shr(32 - sub).unwrap_or(0) as usize;
    entries
        .get((entry >> 10) as usize + index)
        .copied()
        .unwrap_or(0)
}

/// Decodes one codeword of `table`; its payload.
#[inline(always)]
fn huffman(bits: &mut Bits<'_>, table: &Lookup) -> u32 {
    let entry = lookup(bits.peek32(), table);
    bits.skip((entry & 31) as usize);
    entry >> 5
}

/// What codewords are read from: the reader, or a cache of it.
trait Peek {
    fn peek32(&self) -> u32;
    fn skip(&mut self, n: u32);
}

impl Peek for Bits<'_> {
    #[inline(always)]
    fn peek32(&self) -> u32 {
        Bits::peek32(self)
    }
    #[inline(always)]
    fn skip(&mut self, n: u32) {
        Bits::skip(self, n as usize);
    }
}

impl Peek for Cache<'_> {
    #[inline(always)]
    fn peek32(&self) -> u32 {
        Cache::peek32(self)
    }
    #[inline(always)]
    fn skip(&mut self, n: u32) {
        Cache::skip(self, n);
    }
}

/// A scalefactor difference (Table 4.A.1, index_offset -60).
#[inline(always)]
pub(crate) fn sf_difference(bits: &mut Bits<'_>) -> i32 {
    huffman(bits, &SCALEFACTOR) as i32 - 60
}

/// An escape sequence after a value of 16 (4.6.3.3): N ones, a zero and
/// N + 4 bits; the value 2^(N + 4) plus them. At most 8191.
#[inline(always)]
fn escape(bits: &mut impl Peek) -> Result<i32, Error> {
    let peek = bits.peek32();
    let ones = peek.leading_ones();
    if ones > 8 {
        return Err(Error::Invalid("escape sequence too long"));
    }
    // At most 8 + 1 + 12 bits.
    let n = ones + 4;
    let value = (peek << (ones + 1)) >> (32 - n);
    bits.skip(ones + 1 + n);
    Ok((1 << n) + value as i32)
}

/// 2^(n / 8), flushed to zero below the normal range and saturated above.
#[inline(always)]
pub(crate) fn pow2_eighth(n: i32) -> f32 {
    let q = n >> 3;
    let fraction = EIGHTH_POWERS.get((n & 7) as usize).copied().unwrap_or(1.0);
    if q < -126 {
        0.0
    } else if q > 127 {
        f32::MAX
    } else {
        f32::from_bits(((q + 127) as u32) << 23) * fraction
    }
}

impl Channel {
    /// Reads `individual_channel_stream()` (Table 4.50) of AAC LC, with
    /// `ics_info()` when the element has no common one (`info` is then
    /// ignored), and decodes its spectrum, multiplied by `scale` (the
    /// IMDCT's and the output's normalization, by window length: long,
    /// short).
    pub(crate) fn read(
        &mut self,
        bits: &mut Bits<'_>,
        bands: &Bands,
        common: Option<&Info>,
        scale: [f32; 2],
    ) -> Result<(), Error> {
        self.global_gain = bits.read(8) as i32;
        self.info = match common {
            Some(info) => *info,
            None => Info::read(bits, bands)?,
        };
        self.read_sections(bits)?;
        self.read_scalefactors(bits)?;
        let pulse = if bits.bit() {
            if self.info.short() {
                return Err(Error::Invalid("pulse data in a short window sequence"));
            }
            Some(read_pulse(bits)?)
        } else {
            None
        };
        self.tns.present = false;
        self.tns.count = [0; 8];
        if bits.bit() {
            self.tns.read(bits, &self.info);
        }
        if bits.bit() {
            // AAC SSR's gain control, which an AAC LC stream does not
            // carry (damage).
            return Err(Error::Invalid("gain control in an AAC LC stream"));
        }
        let s = if self.info.short() {
            scale[1]
        } else {
            scale[0]
        };
        let pulses = match pulse {
            Some(pulse) => pulse.positions(bands)?,
            None => Pulses::default(),
        };
        self.read_spectrum(bits, bands, s, &pulses)
    }

    /// `section_data()` (Table 4.52).
    fn read_sections(&mut self, bits: &mut Bits<'_>) -> Result<(), Error> {
        let (len_bits, esc) = if self.info.short() { (3, 7) } else { (5, 31) };
        self.noise_bands = 0;
        for g in 0..self.info.groups {
            let Some(row) = self.cb.get_mut(g) else {
                return Err(Error::Invalid("window group"));
            };
            let mut k = 0;
            while k < self.info.max_sfb {
                let cb = bits.read(4) as u8;
                if cb == 12 {
                    return Err(Error::Invalid("reserved codebook 12"));
                }
                let mut len = 0;
                loop {
                    let incr = bits.read(len_bits) as usize;
                    len += incr;
                    if incr != esc {
                        break;
                    }
                    if bits.overrun() {
                        return Err(Error::Invalid("truncated section data"));
                    }
                }
                if len == 0 || k + len > self.info.max_sfb {
                    return Err(Error::Invalid("section past max_sfb"));
                }
                for slot in row.iter_mut().skip(k).take(len) {
                    *slot = cb;
                }
                if cb == NOISE_HCB {
                    self.noise_bands += len as u32;
                }
                k += len;
            }
        }
        Ok(())
    }

    /// `scale_factor_data()` (Table 4.53) and the decoding of scalefactors
    /// (4.6.2.3.2), intensity positions (4.6.8.2.3) and noise energies
    /// (4.6.13.3), each its own differential chain.
    fn read_scalefactors(&mut self, bits: &mut Bits<'_>) -> Result<(), Error> {
        let mut sf = self.global_gain;
        let mut position = 0i32;
        // NOISE_OFFSET 90; the first energy adds a 9-bit value less 256.
        let mut noise = self.global_gain - 90 - 256;
        let mut first_noise = true;
        for g in 0..self.info.groups {
            let (Some(cbs), Some(values)) = (self.cb.get(g), self.sf.get_mut(g)) else {
                return Err(Error::Invalid("window group"));
            };
            for (cb, value) in cbs.iter().zip(values.iter_mut()).take(self.info.max_sfb) {
                *value = match *cb {
                    ZERO_HCB => 0,
                    INTENSITY_HCB | INTENSITY_HCB2 => {
                        position += sf_difference(bits);
                        position
                    }
                    NOISE_HCB => {
                        if first_noise {
                            first_noise = false;
                            noise += bits.read(9) as i32;
                        } else {
                            noise += sf_difference(bits);
                        }
                        noise
                    }
                    _ => {
                        sf += sf_difference(bits);
                        if !(0..=255).contains(&sf) {
                            return Err(Error::Invalid("scalefactor out of range"));
                        }
                        sf
                    }
                };
            }
        }
        Ok(())
    }

    /// `spectral_data()` (Table 4.56), window after window (de-interleaved,
    /// 4.6.3.3), each band's quantized values decoded, its pulses added
    /// (4.6.3.3), then inverse quantized and scaled (4.6.1.3, 4.6.2.3.3)
    /// into `spec`, multiplied by `scale`; bands of other codebooks are
    /// zero.
    fn read_spectrum(
        &mut self,
        bits: &mut Bits<'_>,
        bands: &Bands,
        scale: f32,
        pulses: &Pulses,
    ) -> Result<(), Error> {
        // One cache over the whole of spectral_data(), its codewords back
        // to back.
        let mut cache = bits.cache();
        let result = self.read_bands(&mut cache, bands, scale, pulses);
        bits.resume(&cache);
        result?;
        if bits.overrun() {
            return Err(Error::Invalid("truncated spectral data"));
        }
        Ok(())
    }

    fn read_bands(
        &mut self,
        cache: &mut Cache<'_>,
        bands: &Bands,
        scale: f32,
        pulses: &Pulses,
    ) -> Result<(), Error> {
        self.spec = [0.0; 1024];
        let offsets = self.info.offsets(bands);
        let short = self.info.short();
        let window_len = if short {
            bands.short_len()
        } else {
            bands.frame
        };
        let mut window = 0usize;
        for g in 0..self.info.groups {
            let len = self.info.group_len.get(g).copied().unwrap_or(0) as usize;
            let (Some(cbs), Some(sfs)) = (self.cb.get(g), self.sf.get(g)) else {
                return Err(Error::Invalid("window group"));
            };
            for (sfb, (&cb, &sf)) in cbs.iter().zip(sfs).enumerate().take(self.info.max_sfb) {
                if cb == ZERO_HCB || cb > ESC_HCB {
                    continue;
                }
                let start = offsets.get(sfb).copied().unwrap_or(0) as usize;
                let end = offsets.get(sfb + 1).copied().unwrap_or(0) as usize;
                let gain = pow2_eighth(2 * (sf - 100)) * scale;
                for w in window..window + len {
                    let base = w * window_len;
                    let Some(out) = self.spec.get_mut(base + start..base + end) else {
                        return Err(Error::Invalid("band past the frame"));
                    };
                    if pulses.within(base + start, base + end) {
                        let Some(quant) = self.quant.get_mut(..end.saturating_sub(start)) else {
                            return Err(Error::Invalid("band past the frame"));
                        };
                        decode_band_quantized(cache, cb, quant)?;
                        pulses.add(quant, base + start);
                        dequantize(quant, out, gain);
                    } else {
                        decode_band(cache, cb, out, gain)?;
                    }
                }
            }
            window += len;
        }
        Ok(())
    }
}

/// |q|^(4/3) times the sign of q, times `gain`, of each value of a band.
#[inline(always)]
fn dequantize(quant: &[i32], out: &mut [f32], gain: f32) {
    for (o, &v) in out.iter_mut().zip(quant) {
        // The sign bit of v onto |v|^(4/3), without a branch (signs are
        // random).
        let magnitude = POW43.get(v.unsigned_abs() as usize).copied().unwrap_or(0.0);
        *o = f32::from_bits(magnitude.to_bits() | (v as u32 & 0x8000_0000)) * gain;
    }
}

/// The pulses of a long window: positions in the spectrum and amplitudes.
#[derive(Default)]
struct Pulses {
    count: usize,
    at: [usize; 4],
    amps: [i32; 4],
}

impl Pulses {
    /// Whether a pulse falls in `start..end`.
    #[inline(always)]
    fn within(&self, start: usize, end: usize) -> bool {
        self.at
            .iter()
            .take(self.count)
            .any(|&k| (start..end).contains(&k))
    }

    /// Adds the pulses that fall in `quant`, the values from `start` on, to
    /// their magnitudes (4.6.3.3).
    #[inline(always)]
    fn add(&self, quant: &mut [i32], start: usize) {
        for (&k, &amp) in self.at.iter().zip(&self.amps).take(self.count) {
            if let Some(q) = k.checked_sub(start).and_then(|i| quant.get_mut(i)) {
                if *q > 0 {
                    *q += amp;
                } else {
                    *q -= amp;
                }
            }
        }
    }
}

/// `pulse_data()` (Table 4.7).
struct Pulse {
    count: usize,
    start_sfb: usize,
    offsets: [u8; 4],
    amps: [u8; 4],
}

impl Pulse {
    /// The pulses' positions in the spectrum, each the offset from the one
    /// before, the first from the start of band `start_sfb`.
    fn positions(&self, bands: &Bands) -> Result<Pulses, Error> {
        let Some(&start) = bands.long.get(self.start_sfb) else {
            return Err(Error::Invalid("pulse_start_sfb past the bands"));
        };
        let mut pulses = Pulses {
            count: self.count,
            ..Pulses::default()
        };
        let mut k = start as usize;
        for ((&offset, &amp), (at, a)) in self
            .offsets
            .iter()
            .zip(&self.amps)
            .zip(pulses.at.iter_mut().zip(pulses.amps.iter_mut()))
            .take(self.count)
        {
            k += offset as usize;
            if k >= 1024 {
                return Err(Error::Invalid("pulse past the frame"));
            }
            *at = k;
            *a = i32::from(amp);
        }
        Ok(pulses)
    }
}

fn read_pulse(bits: &mut Bits<'_>) -> Result<Pulse, Error> {
    let count = bits.read(2) as usize + 1;
    let start_sfb = bits.read(6) as usize;
    let mut pulse = Pulse {
        count,
        start_sfb,
        offsets: [0; 4],
        amps: [0; 4],
    };
    for i in 0..count {
        let offset = bits.read(5) as u8;
        let amp = bits.read(4) as u8;
        if let (Some(o), Some(a)) = (pulse.offsets.get_mut(i), pulse.amps.get_mut(i)) {
            *o = offset;
            *a = amp;
        }
    }
    Ok(pulse)
}

/// Decodes a spectrum codeword of the codebook whose first level starts at
/// `base` in `SPECTRUM_ENTRIES`, and the sign bits after it (one per
/// non-zero value of an unsigned codebook, in order, 1 negative): its
/// vector's place in `SPECTRUM_VECTORS`, the sign mask of its four lanes,
/// and its escape lanes (a bit each, codebook 11). A codeword and its signs
/// take at most 16 + 4 bits, so one peek holds both. Indices are masked to
/// the tables' sizes (powers of two), so no lookup is bounds checked.
#[inline(always)]
fn codeword(bits: &mut impl Peek, base: usize) -> (usize, [u32; 4], u32) {
    let peek = bits.peek32();
    let entries = &SPECTRUM_ENTRIES;
    let first = entries
        .get((base + (peek >> 23) as usize) & 8191)
        .copied()
        .unwrap_or(0);
    let entry = if first & 31 != 0 {
        first
    } else {
        // A subtable: its offset, and how many bits after the first nine
        // index it.
        let sub = (first >> 5) & 31;
        let index = (peek << 9).checked_shr(32 - sub).unwrap_or(0) as usize;
        entries
            .get(((first >> 10) as usize + index) & 8191)
            .copied()
            .unwrap_or(0)
    };
    let length = entry & 31;
    let nonzero = (entry >> 5) & 15;
    let count = (entry >> 9) & 7;
    let escapes = (entry >> 12) & 3;
    let vector = (entry >> 14) as usize & 2047;
    let signs = (u64::from(peek << length) >> (32 - count)) as usize;
    let mask = SIGNS
        .get(((nonzero as usize) << 4 | signs) & 255)
        .copied()
        .unwrap_or_default();
    bits.skip(length + count);
    (vector, mask, escapes)
}

/// The values of a codeword: its vector with the signs of `mask`, times
/// `gain`.
#[inline(always)]
fn scaled(vector: usize, mask: [u32; 4], gain: f32) -> [f32; 4] {
    let [a, b, c, d] = SPECTRUM_VECTORS
        .get(vector & 2047)
        .copied()
        .unwrap_or_default();
    let [ma, mb, mc, md] = mask;
    let lane = |v: f32, m: u32| f32::from_bits(v.to_bits() ^ m) * gain;
    [lane(a, ma), lane(b, mb), lane(c, mc), lane(d, md)]
}

/// Decodes the values of one band of one window with spectrum codebook
/// `cb` (1 to 11), inverse quantized and multiplied by `gain`, into `out`.
#[inline(always)]
fn decode_band(cache: &mut Cache<'_>, cb: u8, out: &mut [f32], gain: f32) -> Result<(), Error> {
    let base = SPECTRUM_BASE
        .get(usize::from(cb).wrapping_sub(1))
        .copied()
        .unwrap_or(0) as usize;
    // A refill every two codewords: each takes at most 20 bits with its
    // signs, a refill leaves at least 56. An escape takes at most 21.
    match cb {
        1..=4 => {
            for (i, quad) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                if i & 1 == 0 {
                    cache.refill();
                }
                let (vector, mask, _) = codeword(cache, base);
                *quad = scaled(vector, mask, gain);
            }
        }
        5..=10 => {
            for (i, pair) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                if i & 1 == 0 {
                    cache.refill();
                }
                let (vector, mask, _) = codeword(cache, base);
                let [a, b, _, _] = scaled(vector, mask, gain);
                *pair = [a, b];
            }
        }
        _ => {
            for (i, pair) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                if i & 1 == 0 {
                    cache.refill();
                }
                let (vector, mask, escapes) = codeword(cache, base);
                let [a, b, _, _] = scaled(vector, mask, gain);
                *pair = [a, b];
                if escapes != 0 {
                    for (lane, (slot, &sign)) in pair.iter_mut().zip(&mask).enumerate() {
                        if escapes >> lane & 1 != 0 {
                            cache.refill();
                            let value = escape(cache)?;
                            let magnitude = POW43.get(value as usize).copied().unwrap_or(0.0);
                            *slot = f32::from_bits(magnitude.to_bits() | sign) * gain;
                        }
                    }
                    // The next codeword may follow two escapes.
                    cache.refill();
                }
            }
        }
    }
    Ok(())
}

/// Decodes the quantized values of one band of one window with spectrum
/// codebook `cb` (1 to 11) into `out`: each codeword's index is its values'
/// digits in base `modulus`, the first value the most significant, less
/// `offset` in a signed codebook (4.6.3.3).
fn decode_band_quantized(cache: &mut Cache<'_>, cb: u8, out: &mut [i32]) -> Result<(), Error> {
    let book = usize::from(cb).wrapping_sub(1);
    let base = SPECTRUM_BASE.get(book).copied().unwrap_or(0) as usize;
    let first = VECTOR_BASE.get(book).copied().unwrap_or(0) as usize;
    let (dimension, modulus, offset) = match cb {
        1 | 2 => (4, 3, 1),
        3 | 4 => (4, 3, 0),
        5 | 6 => (2, 9, 4),
        7 | 8 => (2, 8, 0),
        9 | 10 => (2, 13, 0),
        _ => (2, 17, 0),
    };
    for values in out.chunks_exact_mut(dimension) {
        cache.refill();
        let (vector, mask, escapes) = codeword(cache, base);
        let mut index = vector.wrapping_sub(first);
        for (slot, &sign) in values.iter_mut().zip(&mask).rev() {
            let v = (index % modulus) as i32 - offset;
            index /= modulus;
            *slot = if sign != 0 { -v } else { v };
        }
        for (lane, slot) in values.iter_mut().enumerate() {
            if escapes >> lane & 1 != 0 {
                cache.refill();
                let value = escape(cache)?;
                *slot = if *slot < 0 { -value } else { value };
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighth_powers() {
        assert_eq!(pow2_eighth(0), 1.0);
        assert_eq!(pow2_eighth(8), 2.0);
        assert_eq!(pow2_eighth(-8), 0.5);
        assert!((pow2_eighth(2) - 2f32.powf(0.25)).abs() < 1e-6);
        assert!((pow2_eighth(-3) - 2f32.powf(-3.0 / 8.0)).abs() < 1e-6);
        assert_eq!(pow2_eighth(-2000), 0.0);
    }

    #[test]
    fn escapes_decode_as_the_standard_shows() {
        // 00000 -> 16, 01111 -> 31, 1000000 -> 32, 1011111 -> 63.
        let mut bits = Bits::new(&[0b0000_0011, 0b1110_0000, 0b0101_1111, 0]);
        assert_eq!(escape(&mut bits), Ok(16));
        assert_eq!(escape(&mut bits), Ok(31));
        assert_eq!(escape(&mut bits), Ok(32));
        assert_eq!(escape(&mut bits), Ok(63));
    }

    #[test]
    fn vectors_and_signs_match_the_quantized_values() {
        // Random bits through every codebook: the float path gives the
        // integer path's values inverse quantized, bit for bit, and reads
        // the same bits.
        let mut state = 0x1234_5678u32;
        for cb in 1..=11u8 {
            for _ in 0..200 {
                let data: Vec<u8> = (0..64)
                    .map(|_| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        (state >> 24) as u8
                    })
                    .collect();
                let (mut a, mut b) = (Bits::new(&data), Bits::new(&data));
                let (mut ca, mut cb_) = (a.cache(), b.cache());
                let mut quant = [0i32; 16];
                let mut want = [0f32; 16];
                let mut got = [0f32; 16];
                let ra = decode_band_quantized(&mut ca, cb, &mut quant);
                let rb = decode_band(&mut cb_, cb, &mut got, 0.75);
                a.resume(&ca);
                b.resume(&cb_);
                assert_eq!(ra.is_ok(), rb.is_ok(), "codebook {cb}");
                if ra.is_err() {
                    continue;
                }
                dequantize(&quant, &mut want, 0.75);
                assert_eq!(a.position(), b.position(), "codebook {cb}");
                assert_eq!(
                    want.map(f32::to_bits),
                    got.map(f32::to_bits),
                    "codebook {cb}: {quant:?}"
                );
            }
        }
    }

    #[test]
    fn grouping_bits_give_groups() {
        // 1100101 -> groups of 3, 1, 2, 2 (13818-7 8.10's example).
        let bands = Bands::new(4, 1024);
        // reserved 0, sequence 10, shape 0, max_sfb 0000, grouping 1100101.
        let byte0 = 0b0100_0000u8;
        let byte1 = 0b1100_1010u8;
        let data = [byte0, byte1, 0, 0];
        let mut bits = Bits::new(&data);
        let info = Info::read(&mut bits, &bands).unwrap();
        assert_eq!(info.groups, 4);
        assert_eq!(&info.group_len[..4], &[3, 1, 2, 2]);
    }

    #[test]
    fn huffman_decodes_the_shortest_scalefactor_code() {
        // Index 60 (difference 0) is the one-bit code 0.
        let mut bits = Bits::new(&[0x00, 0x00]);
        assert_eq!(sf_difference(&mut bits), 0);
        assert_eq!(bits.position(), 1);
    }
}
