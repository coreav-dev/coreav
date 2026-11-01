//! The compressed header (clauses 6.3 and 7.3): the transform mode, the
//! reference mode and the frame's probability deltas.

use crate::bool::BoolDecoder;
use crate::header::{FrameHeader, InterpolationFilter};
use crate::probs::Probs;
use crate::tables::INV_MAP_TABLE;

/// The probability with which each probability is updated
/// (`diff_update_prob` and `update_mv_prob`, descriptor `B(252)`).
const UPDATE_PROB: u8 = 252;

/// How a frame chooses its transform sizes (`tx_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TxMode {
    /// `ONLY_4X4`.
    Only4x4,
    /// `ALLOW_8X8`.
    Allow8x8,
    /// `ALLOW_16X16`.
    Allow16x16,
    /// `ALLOW_32X32`.
    Allow32x32,
    /// `TX_MODE_SELECT`: chosen per block, up to 32x32.
    Select,
}

impl TxMode {
    /// `tx_mode_to_biggest_tx_size`, as an index: 0 for 4x4 to 3 for
    /// 32x32.
    pub(crate) fn largest_tx_size(self) -> usize {
        match self {
            TxMode::Only4x4 => 0,
            TxMode::Allow8x8 => 1,
            TxMode::Allow16x16 => 2,
            TxMode::Allow32x32 | TxMode::Select => 3,
        }
    }
}

/// Whether blocks predict from one reference frame or two
/// (`reference_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceMode {
    /// `SINGLE_REFERENCE`.
    Single,
    /// `COMPOUND_REFERENCE`.
    Compound,
    /// `REFERENCE_MODE_SELECT`: chosen per block.
    Select,
}

/// What a frame's compressed header states besides its probabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressedHeader {
    /// `tx_mode`.
    pub tx_mode: TxMode,
    /// `reference_mode`; single for an intra frame.
    pub reference_mode: ReferenceMode,
    /// Whether the boolean decoder ended as clause 9.2 requires: its
    /// marker bit and every padding bit 0, and no bit read past the
    /// compressed header.
    pub ends_cleanly: bool,
}

/// Reads `compressed_header()` from `data`, its `header_size_in_bytes`
/// bytes, and applies its deltas to `probs`.
pub(crate) fn read(data: &[u8], h: &FrameHeader, probs: &mut Probs) -> CompressedHeader {
    let mut d = BoolDecoder::new(data);
    let tx_mode = if h.quantization.lossless() {
        TxMode::Only4x4
    } else {
        match d.literal(2) {
            0 => TxMode::Only4x4,
            1 => TxMode::Allow8x8,
            2 => TxMode::Allow16x16,
            _ if d.literal(1) == 0 => TxMode::Allow32x32,
            _ => TxMode::Select,
        }
    };
    if tx_mode == TxMode::Select {
        for (size, by_size) in probs.tx.iter_mut().enumerate().skip(1) {
            for by_ctx in by_size {
                by_ctx
                    .iter_mut()
                    .take(size)
                    .for_each(|p| diff_update_prob(&mut d, p));
            }
        }
    }
    for by_size in probs.coef.iter_mut().take(tx_mode.largest_tx_size() + 1) {
        if d.literal(1) == 0 {
            continue;
        }
        for by_ref in by_size.iter_mut().flatten() {
            for (band, by_band) in by_ref.iter_mut().enumerate() {
                let contexts = if band == 0 { 3 } else { 6 };
                for p in by_band.iter_mut().take(contexts).flatten() {
                    diff_update_prob(&mut d, p);
                }
            }
        }
    }
    update_all(&mut d, &mut probs.skip);
    let mut reference_mode = ReferenceMode::Single;
    if !h.is_intra() {
        update_all(&mut d, probs.inter_mode.as_flattened_mut());
        if h.interpolation_filter == InterpolationFilter::Switchable {
            update_all(&mut d, probs.interp_filter.as_flattened_mut());
        }
        update_all(&mut d, &mut probs.is_inter);
        reference_mode = frame_reference_mode(&mut d, h.ref_frame_sign_bias);
        if reference_mode == ReferenceMode::Select {
            update_all(&mut d, &mut probs.comp_mode);
        }
        if reference_mode != ReferenceMode::Compound {
            update_all(&mut d, probs.single_ref.as_flattened_mut());
        }
        if reference_mode != ReferenceMode::Single {
            update_all(&mut d, &mut probs.comp_ref);
        }
        update_all(&mut d, probs.y_mode.as_flattened_mut());
        update_all(&mut d, probs.partition.as_flattened_mut());
        mv_probs(&mut d, probs, h.allow_high_precision_mv);
    }
    CompressedHeader {
        tx_mode,
        reference_mode,
        ends_cleanly: d.exit(),
    }
}

/// `frame_reference_mode()` (clause 6.3.12). Compound prediction needs
/// two reference frames of different sign bias.
fn frame_reference_mode(d: &mut BoolDecoder<'_>, sign_bias: [bool; 4]) -> ReferenceMode {
    let [_, last, golden, altref] = sign_bias;
    if (golden == last && altref == last) || d.literal(1) == 0 {
        ReferenceMode::Single
    } else if d.literal(1) == 0 {
        ReferenceMode::Compound
    } else {
        ReferenceMode::Select
    }
}

/// `mv_probs()` (clause 6.3.16).
fn mv_probs(d: &mut BoolDecoder<'_>, probs: &mut Probs, high_precision: bool) {
    probs.mv_joint.iter_mut().for_each(|p| update_mv_prob(d, p));
    for i in 0..2 {
        if let Some(p) = probs.mv_sign.get_mut(i) {
            update_mv_prob(d, p);
        }
        if let Some(class) = probs.mv_class.get_mut(i) {
            class.iter_mut().for_each(|p| update_mv_prob(d, p));
        }
        if let Some(p) = probs.mv_class0_bit.get_mut(i) {
            update_mv_prob(d, p);
        }
        if let Some(bits) = probs.mv_bits.get_mut(i) {
            bits.iter_mut().for_each(|p| update_mv_prob(d, p));
        }
    }
    for i in 0..2 {
        if let Some(fr) = probs.mv_class0_fr.get_mut(i) {
            fr.iter_mut().flatten().for_each(|p| update_mv_prob(d, p));
        }
        if let Some(fr) = probs.mv_fr.get_mut(i) {
            fr.iter_mut().for_each(|p| update_mv_prob(d, p));
        }
    }
    if high_precision {
        for i in 0..2 {
            for p in [probs.mv_class0_hp.get_mut(i), probs.mv_hp.get_mut(i)]
                .into_iter()
                .flatten()
            {
                update_mv_prob(d, p);
            }
        }
    }
}

fn update_all(d: &mut BoolDecoder<'_>, probs: &mut [u8]) {
    probs.iter_mut().for_each(|p| diff_update_prob(d, p));
}

/// `diff_update_prob(prob)` (clause 6.3.3).
fn diff_update_prob(d: &mut BoolDecoder<'_>, prob: &mut u8) {
    if d.read(UPDATE_PROB) {
        *prob = inv_remap_prob(decode_term_subexp(d), *prob);
    }
}

/// `update_mv_prob(prob)` (clause 6.3.17).
fn update_mv_prob(d: &mut BoolDecoder<'_>, prob: &mut u8) {
    if d.read(UPDATE_PROB) {
        *prob = (d.literal(7) << 1) | 1;
    }
}

/// `decode_term_subexp()` (clause 6.3.4): `deltaProb`, 0 to 254.
fn decode_term_subexp(d: &mut BoolDecoder<'_>) -> u8 {
    if d.literal(1) == 0 {
        return d.literal(4);
    }
    if d.literal(1) == 0 {
        return d.literal(4) + 16;
    }
    if d.literal(1) == 0 {
        return d.literal(5) + 32;
    }
    let v = d.literal(7);
    if v < 65 {
        return v + 64;
    }
    (v << 1) - 1 + d.literal(1)
}

/// `inv_remap_prob(deltaProb, prob)` (clause 6.3.5): the probability
/// `deltaProb` names, counted outward from `prob`.
fn inv_remap_prob(delta: u8, prob: u8) -> u8 {
    let v = i32::from(INV_MAP_TABLE.get(usize::from(delta)).copied().unwrap_or(0));
    let m = i32::from(prob) - 1;
    let remapped = if m << 1 <= 255 {
        1 + inv_recenter_nonneg(v, m)
    } else {
        255 - inv_recenter_nonneg(v, 255 - 1 - m)
    };
    u8::try_from(remapped).unwrap_or(u8::MAX)
}

/// `inv_recenter_nonneg(v, m)` (clause 6.3.6).
fn inv_recenter_nonneg(v: i32, m: i32) -> i32 {
    if v > 2 * m {
        v
    } else if v & 1 == 1 {
        m - ((v + 1) >> 1)
    } else {
        m + (v >> 1)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn every_delta_names_a_different_probability() {
        for prob in 1..=255u8 {
            let mut seen = [false; 256];
            for delta in 0..254 {
                let new = inv_remap_prob(delta, prob);
                assert!(new != 0 && new != prob, "prob {prob}, delta {delta}");
                assert!(!seen[usize::from(new)], "prob {prob}, delta {delta}");
                seen[usize::from(new)] = true;
            }
        }
    }

    #[test]
    fn small_deltas_stay_near_the_probability() {
        assert_eq!(inv_remap_prob(20, 100), 99);
        assert_eq!(inv_remap_prob(21, 100), 101);
        assert_eq!(inv_remap_prob(20, 200), 201);
        assert_eq!(inv_remap_prob(21, 200), 199);
    }
}
