//! Band energies (RFC 6716 4.3.2): the coarse energy, predicted in time and
//! in frequency with a Laplace-like residual, then its fine refinement and
//! the bits left over at the end of the frame.
//!
//! Energies are base-2 logarithms of the band's amplitude relative to the
//! band's mean ([`t::ENERGY_MEANS`]), in `f32`. The arithmetic that
//! decides which symbols are read (the budget tests, the Laplace decoder)
//! is integer and the reference's; the energies themselves only shape the
//! output.

use super::{BANDS, Bands, at};
use crate::range::RangeDecoder;
use crate::tables::celt as t;

/// The most fine-energy bits a band takes (4.3.2.2).
pub(super) const MAX_FINE: i32 = 8;

/// Decodes the coarse energy of `bands` into `energy`, which holds the
/// previous frame's (4.3.2.1).
pub(super) fn decode_coarse(
    rd: &mut RangeDecoder<'_>,
    bands: Bands,
    lm: usize,
    intra: bool,
    energy: &mut [[f32; BANDS]],
) {
    let model = t::ENERGY_MODEL
        .get(lm)
        .and_then(|m| m.get(usize::from(intra)))
        .map_or(&[][..], |m| &m[..]);
    let (alpha, beta) = if intra {
        (0.0, 4915.0 / 32768.0)
    } else {
        (
            f32::from(at(&t::ENERGY_PREDICTION_Q15, lm)) / 32768.0,
            f32::from(at(&t::ENERGY_BETA_Q15, lm)) / 32768.0,
        )
    };
    let budget = i64::from(rd.storage_bits());
    // The prediction from the lower bands, per channel.
    let mut below = [0f32; 2];
    for band in bands.range() {
        for (channel, below) in energy.iter_mut().zip(&mut below) {
            let left = budget - i64::from(rd.tell());
            let q = if left >= 15 {
                let k = 2 * band.min(20);
                laplace(
                    rd,
                    u32::from(at(model, k)) << 7,
                    u32::from(at(model, k + 1)) << 6,
                )
            } else if left >= 2 {
                // 0, -1, 1 (4.3.2.1).
                let s = rd.icdf(&t::SMALL_ENERGY_ICDF, 2) as i32;
                (s >> 1) ^ -(s & 1)
            } else if left >= 1 {
                -i32::from(rd.bit_logp(1))
            } else {
                -1
            } as f32;
            if let Some(e) = channel.get_mut(band) {
                *e = alpha * e.max(-9.0) + *below + q;
            }
            *below += q - beta * q;
        }
    }
}

/// A Laplace-distributed integer with probability `fs0` of 0, out of
/// 32768, and decay `decay`, out of 16384 (4.3.2.1): every value keeps a
/// probability of at least 1/32768.
fn laplace(rd: &mut RangeDecoder<'_>, fs0: u32, decay: u32) -> i32 {
    let fm = rd.decode_bin(15);
    let mut fl = 0;
    let mut fs = fs0;
    let mut value = 0i32;
    if fm >= fs {
        value = 1;
        fl = fs;
        // The probability of ±1, then of each larger magnitude, each with
        // its minimum of 1 added.
        fs = ((32768u32.saturating_sub(32 + fs0) * 16384u32.saturating_sub(decay)) >> 15) + 1;
        while fs > 1 && fm >= fl + 2 * fs {
            fs *= 2;
            fl += fs;
            fs = (((fs - 2) * decay) >> 15) + 1;
            value += 1;
        }
        if fs <= 1 {
            // The tail, where every magnitude has probability 1.
            let run = (fm - fl) >> 1;
            value += run as i32;
            fl += 2 * run;
        }
        if fm < fl + fs {
            value = -value;
        } else {
            fl += fs;
        }
    }
    rd.update(fl, (fl + fs).min(32768), 32768);
    value
}

/// Decodes the fine energy, `fine[band]` bits per channel (4.3.2.2).
pub(super) fn decode_fine(
    rd: &mut RangeDecoder<'_>,
    bands: Bands,
    fine: &[i32; BANDS],
    energy: &mut [[f32; BANDS]],
) {
    for band in bands.range() {
        let bits = at(fine, band);
        if bits <= 0 {
            continue;
        }
        let step = 1.0 / (1u32 << bits) as f32;
        for channel in energy.iter_mut() {
            let q = rd.bits(bits as u32) as f32;
            if let Some(e) = channel.get_mut(band) {
                *e += (q + 0.5) * step - 0.5;
            }
        }
    }
}

/// Spends `left` bits that the frame has not used on one more bit of fine
/// energy per band and channel, the bands of priority 0 first (4.3.2.2).
pub(super) fn decode_final(
    rd: &mut RangeDecoder<'_>,
    bands: Bands,
    fine: &[i32; BANDS],
    priority: &[bool; BANDS],
    mut left: i32,
    energy: &mut [[f32; BANDS]],
) {
    let channels = energy.len() as i32;
    for pass in [false, true] {
        for band in bands.range() {
            if left < channels {
                break;
            }
            let bits = at(fine, band);
            if bits >= MAX_FINE || at(priority, band) != pass {
                continue;
            }
            let step = 1.0 / (2u32 << bits) as f32;
            for channel in energy.iter_mut() {
                let q = rd.bits(1) as f32;
                if let Some(e) = channel.get_mut(band) {
                    *e += (q - 0.5) * step;
                }
                left -= 1;
            }
        }
    }
}
