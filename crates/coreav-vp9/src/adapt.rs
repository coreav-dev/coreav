//! Backward probability adaptation (clauses 6.1.2, 7.1.2 and 8.4).

use crate::compressed::TxMode;
use crate::header::{FrameHeader, InterpolationFilter};
use crate::probs::Probs;

const COUNT_SAT: u64 = 20;
const MAX_UPDATE_FACTOR: u64 = 128;

type CoefCounts = [[[[[[[u32; 2]; 3]; 6]; 6]; 2]; 2]; 4];

/// The observed zero and one counts for each branch of `Probs`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) tx: [[[[u32; 2]; 3]; 2]; 4],
    pub(crate) coef: CoefCounts,
    pub(crate) skip: [[u32; 2]; 3],
    pub(crate) inter_mode: [[[u32; 2]; 3]; 7],
    pub(crate) interp_filter: [[[u32; 2]; 2]; 4],
    pub(crate) is_inter: [[u32; 2]; 4],
    pub(crate) comp_mode: [[u32; 2]; 5],
    pub(crate) single_ref: [[[u32; 2]; 2]; 5],
    pub(crate) comp_ref: [[u32; 2]; 5],
    pub(crate) y_mode: [[[u32; 2]; 9]; 4],
    pub(crate) uv_mode: [[[u32; 2]; 9]; 10],
    pub(crate) partition: [[[u32; 2]; 3]; 16],
    pub(crate) mv_joint: [[u32; 2]; 3],
    pub(crate) mv_sign: [[u32; 2]; 2],
    pub(crate) mv_class: [[[u32; 2]; 10]; 2],
    pub(crate) mv_class0_bit: [[u32; 2]; 2],
    pub(crate) mv_bits: [[[u32; 2]; 10]; 2],
    pub(crate) mv_class0_fr: [[[[u32; 2]; 3]; 2]; 2],
    pub(crate) mv_fr: [[[u32; 2]; 3]; 2],
    pub(crate) mv_class0_hp: [[u32; 2]; 2],
    pub(crate) mv_hp: [[u32; 2]; 2],
}

/// The probabilities after `refresh_probs`, before an optional `save_probs`.
pub(crate) fn refresh(
    saved: &Probs,
    updated: &Probs,
    counts: &Counts,
    h: &FrameHeader,
    tx_mode: TxMode,
    previous_key: bool,
) -> Probs {
    if h.error_resilient || h.frame_parallel {
        return updated.clone();
    }
    let mut probs = saved.clone();
    let coef_factor = if !h.is_intra() && previous_key {
        128
    } else {
        112
    };
    for (by_plane, plane_counts) in probs.coef.iter_mut().zip(&counts.coef) {
        for (by_ref, ref_counts) in by_plane.iter_mut().zip(plane_counts) {
            for (by_band, band_counts) in by_ref.iter_mut().zip(ref_counts) {
                for (band, (by_ctx, ctx_counts)) in by_band.iter_mut().zip(band_counts).enumerate()
                {
                    let contexts = if band == 0 { 3 } else { 6 };
                    for (branches, branch_counts) in
                        by_ctx.iter_mut().zip(ctx_counts).take(contexts)
                    {
                        branches.merge(branch_counts, 24, coef_factor);
                    }
                }
            }
        }
    }
    if h.is_intra() {
        probs.tx = updated.tx;
        probs.skip = updated.skip;
        return probs;
    }
    probs
        .is_inter
        .merge(&counts.is_inter, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .comp_mode
        .merge(&counts.comp_mode, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .comp_ref
        .merge(&counts.comp_ref, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .single_ref
        .merge(&counts.single_ref, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .inter_mode
        .merge(&counts.inter_mode, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .y_mode
        .merge(&counts.y_mode, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .uv_mode
        .merge(&counts.uv_mode, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .partition
        .merge(&counts.partition, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs.skip.merge(&counts.skip, COUNT_SAT, MAX_UPDATE_FACTOR);
    if h.interpolation_filter == InterpolationFilter::Switchable {
        probs
            .interp_filter
            .merge(&counts.interp_filter, COUNT_SAT, MAX_UPDATE_FACTOR);
    }
    if tx_mode == TxMode::Select {
        for (size, (by_ctx, ctx_counts)) in probs.tx.iter_mut().zip(&counts.tx).enumerate().skip(1)
        {
            for (branches, branch_counts) in by_ctx.iter_mut().zip(ctx_counts) {
                for (prob, count) in branches.iter_mut().zip(branch_counts).take(size) {
                    prob.merge(count, COUNT_SAT, MAX_UPDATE_FACTOR);
                }
            }
        }
    }
    probs
        .mv_joint
        .merge(&counts.mv_joint, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .mv_sign
        .merge(&counts.mv_sign, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .mv_class
        .merge(&counts.mv_class, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .mv_class0_bit
        .merge(&counts.mv_class0_bit, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .mv_bits
        .merge(&counts.mv_bits, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .mv_class0_fr
        .merge(&counts.mv_class0_fr, COUNT_SAT, MAX_UPDATE_FACTOR);
    probs
        .mv_fr
        .merge(&counts.mv_fr, COUNT_SAT, MAX_UPDATE_FACTOR);
    if h.allow_high_precision_mv {
        probs
            .mv_class0_hp
            .merge(&counts.mv_class0_hp, COUNT_SAT, MAX_UPDATE_FACTOR);
        probs
            .mv_hp
            .merge(&counts.mv_hp, COUNT_SAT, MAX_UPDATE_FACTOR);
    }
    probs
}

trait Merge {
    type Counts;

    fn merge(&mut self, counts: &Self::Counts, count_sat: u64, max_update_factor: u64);
}

impl Merge for u8 {
    type Counts = [u32; 2];

    fn merge(&mut self, counts: &Self::Counts, count_sat: u64, max_update_factor: u64) {
        let [ct0, ct1] = counts.map(u64::from);
        let den = ct0 + ct1;
        if den == 0 {
            return;
        }
        let prob = ((ct0 * 256 + (den >> 1)) / den).clamp(1, 255);
        let factor = max_update_factor * den.min(count_sat) / count_sat;
        *self = ((u64::from(*self) * (256 - factor) + prob * factor + 128) >> 8) as u8;
    }
}

impl<T: Merge, const N: usize> Merge for [T; N] {
    type Counts = [T::Counts; N];

    fn merge(&mut self, counts: &Self::Counts, count_sat: u64, max_update_factor: u64) {
        for (probs, counts) in self.iter_mut().zip(counts) {
            probs.merge(counts, count_sat, max_update_factor);
        }
    }
}

#[cfg(test)]
mod tests;
