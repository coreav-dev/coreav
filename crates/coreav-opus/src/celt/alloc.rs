//! The bit allocation (RFC 6716 4.3.3): how many bits each band gets for
//! its fine energy and for its shape, which the decoder computes from the
//! frame's size and a few coded parameters exactly as the encoder did.
//!
//! All of it is integer arithmetic in eighth bits, and all of it decides
//! which symbols are read next, so it is the reference's to the bit: the
//! search over the allocation vectors, the interpolation between two of
//! them, band skipping, and the split of each band's bits between fine
//! energy and the shape.

use super::energy::MAX_FINE;
use super::{BANDS, Bands, at, set, width};
use crate::range::RangeDecoder;
use crate::tables::celt as t;

/// Eighth bits per bit (`BITRES`).
pub(super) const EIGHTHS: i32 = 8;

/// The fine-energy offset, in eighth bits (4.3.3).
const FINE_OFFSET: i32 = 21;

/// What the allocation gives each band.
#[derive(Clone, Copy, Debug)]
pub(super) struct Allocation {
    /// The bands above this one get no shape bits.
    pub(super) coded: usize,
    /// The first band coded with intensity stereo.
    pub(super) intensity: usize,
    pub(super) dual_stereo: bool,
    /// Bits over the caps, carried into the shape decoding.
    pub(super) balance: i32,
    /// The shape's bits per band, in eighth bits.
    pub(super) shape: [i32; BANDS],
    /// The fine energy's bits per band and channel.
    pub(super) fine: [i32; BANDS],
    /// Whether a band's bit from the leftover bits comes in the second
    /// pass rather than the first.
    pub(super) priority: [bool; BANDS],
}

/// The most bits, in eighth bits, that each band's shape can use
/// (4.3.3, `cache_caps50`).
pub(super) fn caps(lm: usize, channels: usize) -> [i32; BANDS] {
    let row = BANDS * (2 * lm + channels - 1);
    let mut caps = [0; BANDS];
    for (band, cap) in caps.iter_mut().enumerate() {
        let n = width(band) << lm;
        *cap = ((i32::from(at(&t::PULSE_CAPS, row + band)) + 64) * (channels * n) as i32) >> 2;
    }
    caps
}

/// The band's share of allocation vector `q`, in eighth bits, before any
/// boost and with the trim applied; above the last vector, the band's cap.
fn vector(q: usize, band: usize, channels: i32, lm: usize, trim: i32, cap: &[i32]) -> i32 {
    let bits = if q * BANDS >= t::ALLOCATION.len() {
        at(cap, band)
    } else {
        (channels * width(band) as i32 * i32::from(at(&t::ALLOCATION, q * BANDS + band))) << lm >> 2
    };
    if bits > 0 { (bits + trim).max(0) } else { bits }
}

/// The allocation of a frame with `total` eighth bits for the bands
/// (4.3.3), reading the skip, intensity and dual stereo symbols it codes.
#[allow(clippy::too_many_arguments)]
pub(super) fn allocate(
    rd: &mut RangeDecoder<'_>,
    bands: Bands,
    boost: &[i32; BANDS],
    cap: &[i32; BANDS],
    trim: i32,
    total: i32,
    channels: usize,
    lm: usize,
) -> Allocation {
    let c = channels as i32;
    let mut total = total.max(0);
    // Reserved: a bit to end the skipping, then the intensity band and the
    // dual stereo flag.
    let skip_reserve = if total >= EIGHTHS { EIGHTHS } else { 0 };
    total -= skip_reserve;
    let mut intensity_reserve = 0;
    let mut dual_reserve = 0;
    if channels == 2 {
        intensity_reserve = i32::from(at(&t::LOG2_FRAC, bands.end - bands.start));
        if intensity_reserve > total {
            intensity_reserve = 0;
        } else {
            total -= intensity_reserve;
            dual_reserve = if total >= EIGHTHS { EIGHTHS } else { 0 };
            total -= dual_reserve;
        }
    }

    let mut threshold = [0; BANDS];
    let mut trim_offset = [0; BANDS];
    for band in bands.range() {
        let w = width(band) as i32;
        // Below this, a band gets no shape bits.
        set(
            &mut threshold,
            band,
            (c * EIGHTHS).max(((3 * w) << lm << 3) >> 4),
        );
        // The trim tilts the allocation; single-bin bands get less.
        let mut offset =
            (c * w * (trim - 5 - lm as i32) * (bands.end - band - 1) as i32 * (1 << (lm + 3))) >> 6;
        if w << lm == 1 {
            offset -= c * EIGHTHS;
        }
        set(&mut trim_offset, band, offset);
    }

    // The highest allocation vector whose total fits.
    let (mut lo, mut hi) = (1usize, t::ALLOCATION.len() / BANDS - 1);
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let mut sum = 0;
        let mut on = false;
        for band in bands.range().rev() {
            let bits = vector(mid, band, c, lm, at(&trim_offset, band), cap) + at(boost, band);
            if bits >= at(&threshold, band) || on {
                on = true;
                sum += bits.min(at(cap, band));
            } else if bits >= c * EIGHTHS {
                sum += c * EIGHTHS;
            }
        }
        if sum > total {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    // Interpolate between vectors `lo - 1` and `lo` (the cap above the
    // last).
    let upper = lo;
    let lower = lo - 1;
    let mut base = [0; BANDS];
    let mut step = [0; BANDS];
    let mut skip_start = bands.start;
    for band in bands.range() {
        let b = at(boost, band);
        let mut low = vector(lower, band, c, lm, at(&trim_offset, band), cap);
        let mut high = vector(upper, band, c, lm, at(&trim_offset, band), cap);
        if lower > 0 {
            low += b;
        }
        high += b;
        if b > 0 {
            skip_start = band;
        }
        set(&mut base, band, low);
        set(&mut step, band, (high - low).max(0));
    }

    let mut a = Allocation {
        coded: bands.end,
        intensity: 0,
        dual_stereo: false,
        balance: 0,
        shape: [0; BANDS],
        fine: [0; BANDS],
        priority: [false; BANDS],
    };
    let floor = c * EIGHTHS;
    // The interpolation weight, in 64ths, by bisection.
    let (mut lo, mut hi) = (0, 64);
    for _ in 0..6 {
        let mid = (lo + hi) / 2;
        let mut sum = 0;
        let mut on = false;
        for band in bands.range().rev() {
            let bits = at(&base, band) + ((mid * at(&step, band)) >> 6);
            if bits >= at(&threshold, band) || on {
                on = true;
                sum += bits.min(at(cap, band));
            } else if bits >= floor {
                sum += floor;
            }
        }
        if sum > total {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let bits = &mut a.shape;
    let mut sum = 0;
    let mut on = false;
    for band in bands.range().rev() {
        let mut b = at(&base, band) + ((lo * at(&step, band)) >> 6);
        if b < at(&threshold, band) && !on {
            b = if b >= floor { floor } else { 0 };
        } else {
            on = true;
        }
        b = b.min(at(cap, band));
        set(bits, band, b);
        sum += b;
    }

    // Skip bands from the top while the encoder says so, giving their bits
    // back to the lower bands.
    let edge = |band: usize| at(&t::BAND_EDGES, band) as i32;
    let start_edge = edge(bands.start);
    let mut coded = bands.end;
    loop {
        let band = coded - 1;
        if band <= skip_start {
            total += skip_reserve;
            break;
        }
        let span = edge(coded) - start_edge;
        let mut left = total - sum;
        let per_bin = left / span;
        left -= span * per_bin;
        let rest = (left - (edge(band) - start_edge)).max(0);
        let mut band_bits = at(bits, band) + per_bin * (edge(coded) - edge(band)) + rest;
        if band_bits >= at(&threshold, band).max(floor + EIGHTHS) {
            if rd.bit_logp(1) {
                break;
            }
            sum += EIGHTHS;
            band_bits -= EIGHTHS;
        }
        sum -= at(bits, band) + intensity_reserve;
        if intensity_reserve > 0 {
            intensity_reserve = i32::from(at(&t::LOG2_FRAC, band - bands.start));
        }
        sum += intensity_reserve;
        let kept = if band_bits >= floor { floor } else { 0 };
        sum += kept;
        set(bits, band, kept);
        coded -= 1;
    }
    a.coded = coded;

    if intensity_reserve > 0 {
        a.intensity = bands.start + rd.uint((coded + 1 - bands.start) as u32) as usize;
    }
    if a.intensity <= bands.start {
        total += dual_reserve;
        dual_reserve = 0;
    }
    a.dual_stereo = dual_reserve > 0 && rd.bit_logp(1);

    // Spread what is left over the coded bands, by width, then the
    // remainder from the bottom.
    let span = edge(coded) - start_edge;
    let mut left = total - sum;
    let per_bin = left / span;
    left -= span * per_bin;
    for band in bands.start..coded {
        let w = width(band) as i32;
        let extra = left.min(w);
        left -= extra;
        if let Some(b) = bits.get_mut(band) {
            *b += per_bin * w + extra;
        }
    }

    // Split each coded band's bits between fine energy and the shape.
    let stereo = i32::from(channels == 2);
    let log_m = (lm as i32) << 3;
    let mut balance = 0;
    for band in bands.start..coded {
        let n = (width(band) << lm) as i32;
        let mut b = at(bits, band) + balance;
        let mut fine;
        let mut priority;
        let mut excess;
        if n > 1 {
            excess = (b - at(cap, band)).max(0);
            b -= excess;
            // One more degree of freedom for a stereo band not coded with
            // intensity or dual stereo.
            let den =
                c * n + i32::from(channels == 2 && n > 2 && !a.dual_stereo && band < a.intensity);
            let nc_log_n = den * (i32::from(at(&t::LOG_N, band)) + log_m);
            let mut offset = (nc_log_n >> 1) - den * FINE_OFFSET;
            if n == 2 {
                offset += den << 3 >> 2;
            }
            if b + offset < (den * 2) << 3 {
                offset += nc_log_n >> 2;
            } else if b + offset < (den * 3) << 3 {
                offset += nc_log_n >> 3;
            }
            fine = ((b + offset + (den << 2)) / (den << 3)).max(0);
            if c * fine > b >> 3 {
                fine = b >> stereo >> 3;
            }
            fine = fine.min(MAX_FINE);
            priority = fine * (den << 3) >= b + offset;
            b -= (c * fine) << 3;
        } else {
            // A single bin: all to fine energy, but for a sign bit.
            excess = (b - (c << 3)).max(0);
            b -= excess;
            fine = 0;
            priority = true;
        }
        if excess > 0 {
            // What the shape cannot use goes to fine energy, then on.
            let extra = (excess >> (stereo + 3)).min(MAX_FINE - fine);
            fine += extra;
            let extra_bits = (extra * c) << 3;
            priority = extra_bits >= excess - balance;
            excess -= extra_bits;
        }
        balance = excess;
        set(bits, band, b);
        set(&mut a.fine, band, fine);
        set(&mut a.priority, band, priority);
    }
    a.balance = balance;
    // Skipped bands keep their bits for fine energy.
    for band in coded..bands.end {
        let fine = at(bits, band) >> stereo >> 3;
        set(&mut a.fine, band, fine);
        set(bits, band, 0);
        set(&mut a.priority, band, fine < 1);
    }
    a
}
