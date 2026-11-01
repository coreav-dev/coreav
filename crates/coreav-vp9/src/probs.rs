//! The probability tables a frame decodes with: their defaults (clause
//! 10.5), the four saved frame contexts, and the rules of clause 7.2 that
//! reset them (`setup_past_independence`, `save_probs`, `load_probs` and
//! `load_probs2`).

use crate::header::FrameHeader;
use crate::tables;

/// Every probability table that `save_probs` saves, by its name in the
/// specification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Probs {
    /// `tx_probs_8x8`, `tx_probs_16x16` and `tx_probs_32x32` at indices 1,
    /// 2 and 3, by context; each has one probability fewer than its index's
    /// transform sizes, and the rest is 0.
    pub(crate) tx: [[[u8; 3]; 2]; 4],
    /// `coef_probs`, by transform size, plane type, reference type, band
    /// and context; band 0 uses contexts 0 to 2 only.
    pub(crate) coef: tables::CoefProbs,
    pub(crate) skip: [u8; 3],
    pub(crate) inter_mode: [[u8; 3]; 7],
    pub(crate) interp_filter: [[u8; 2]; 4],
    pub(crate) is_inter: [u8; 4],
    pub(crate) comp_mode: [u8; 5],
    pub(crate) single_ref: [[u8; 2]; 5],
    pub(crate) comp_ref: [u8; 5],
    pub(crate) y_mode: [[u8; 9]; 4],
    pub(crate) uv_mode: [[u8; 9]; 10],
    pub(crate) partition: [[u8; 3]; 16],
    pub(crate) mv_joint: [u8; 3],
    pub(crate) mv_sign: [u8; 2],
    pub(crate) mv_class: [[u8; 10]; 2],
    pub(crate) mv_class0_bit: [u8; 2],
    pub(crate) mv_bits: [[u8; 10]; 2],
    pub(crate) mv_class0_fr: [[[u8; 3]; 2]; 2],
    pub(crate) mv_fr: [[u8; 3]; 2],
    pub(crate) mv_class0_hp: [u8; 2],
    pub(crate) mv_hp: [u8; 2],
}

impl Probs {
    /// The default values (clause 10.5).
    pub(crate) const DEFAULT: Probs = Probs {
        tx: tables::DEFAULT_TX_PROBS,
        coef: tables::DEFAULT_COEF_PROBS,
        skip: tables::DEFAULT_SKIP_PROB,
        inter_mode: tables::DEFAULT_INTER_MODE_PROBS,
        interp_filter: tables::DEFAULT_INTERP_FILTER_PROBS,
        is_inter: tables::DEFAULT_IS_INTER_PROB,
        comp_mode: tables::DEFAULT_COMP_MODE_PROB,
        single_ref: tables::DEFAULT_SINGLE_REF_PROB,
        comp_ref: tables::DEFAULT_COMP_REF_PROB,
        y_mode: tables::DEFAULT_Y_MODE_PROBS,
        uv_mode: tables::DEFAULT_UV_MODE_PROBS,
        partition: tables::DEFAULT_PARTITION_PROBS,
        mv_joint: tables::DEFAULT_MV_JOINT_PROBS,
        mv_sign: tables::DEFAULT_MV_SIGN_PROB,
        mv_class: tables::DEFAULT_MV_CLASS_PROBS,
        mv_class0_bit: tables::DEFAULT_MV_CLASS0_BIT_PROB,
        mv_bits: tables::DEFAULT_MV_BITS_PROB,
        mv_class0_fr: tables::DEFAULT_MV_CLASS0_FR_PROBS,
        mv_fr: tables::DEFAULT_MV_FR_PROBS,
        mv_class0_hp: tables::DEFAULT_MV_CLASS0_HP_PROB,
        mv_hp: tables::DEFAULT_MV_HP_PROB,
    };
}

/// The four frame contexts `save_probs` saves into and `load_probs` loads
/// from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Contexts([Probs; 4]);

impl Default for Contexts {
    fn default() -> Self {
        Contexts([Probs::DEFAULT; 4])
    }
}

impl Contexts {
    /// What `uncompressed_header()` does to the saved contexts (clauses
    /// 6.2 and 7.2): a frame that sets up past independence saves the
    /// default probabilities into every context (a key frame, an error
    /// resilient frame, or `reset_frame_context` 3), into the context it
    /// coded (`reset_frame_context` 2), or into none.
    pub(crate) fn reset(&mut self, h: &FrameHeader) {
        if !(h.is_intra() || h.error_resilient) {
            return;
        }
        if h.key_frame || h.error_resilient || h.reset_frame_context == 3 {
            self.0 = [Probs::DEFAULT; 4];
        } else if h.reset_frame_context == 2 {
            if let Some(saved) = self.0.get_mut(usize::from(h.coded_frame_context_idx)) {
                *saved = Probs::DEFAULT;
            }
        }
    }

    /// `load_probs(ctx)` and `load_probs2(ctx)` together: every table of
    /// context `ctx`, 0 to 3.
    pub(crate) fn load(&self, ctx: u8) -> Probs {
        self.0
            .get(usize::from(ctx))
            .cloned()
            .unwrap_or(Probs::DEFAULT)
    }

    /// `save_probs(ctx)`.
    pub(crate) fn save(&mut self, ctx: u8, probs: Probs) {
        if let Some(saved) = self.0.get_mut(usize::from(ctx)) {
            *saved = probs;
        }
    }
}
