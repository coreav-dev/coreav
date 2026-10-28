//! The side information of one SILK frame (RFC 6716 4.2.7.3 to 4.2.7.7):
//! the frame type, the gains, the LSF indices, the long-term prediction
//! parameters and the seed, read in the order of Table 5. The stereo
//! weights and the mid-only flag that come before them belong to the
//! Opus frame's layout and are read by the caller.
//!
//! Nothing here interprets the values beyond what the reading itself needs
//! (which table comes next): the gains stay indices, the LSFs stay
//! codebook indices, and the pitch lag stays relative to the rate's
//! minimum, as the next frame's relative lag needs it.

use super::Rate;
use crate::range::RangeDecoder;
use crate::tables::silk as t;

/// The signal type of a frame (Table 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Inactive,
    Unvoiced,
    Voiced,
}

impl Signal {
    /// The row of the tables that depend on the signal type.
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Inactive => 0,
            Self::Unvoiced => 1,
            Self::Voiced => 2,
        }
    }
}

/// How a frame's first gain and its pitch lag are coded, and whether it
/// codes an LTP scaling factor (4.2.7.4, 4.2.7.6.1, 4.2.7.6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    /// The first frame of its kind in the Opus frame, or after an uncoded
    /// LBRR frame: absolute gain and lag, LTP scaling coded.
    Independent,
    /// A side channel frame after an uncoded one in the same Opus frame:
    /// absolute gain and lag, but no LTP scaling (its LTP buffer was
    /// cleared, so is in a known state).
    NoLtpScaling,
    /// After a coded frame of the same kind and channel: the first gain is
    /// a delta, and so may be the lag if that frame was voiced.
    Conditional,
}

/// The long-term prediction parameters of a voiced frame (4.2.7.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pitch {
    /// The primary lag less the rate's minimum lag; not clamped, since
    /// the next frame's relative lag builds on it as it is (4.2.7.6.1).
    pub(crate) lag: i32,
    /// The pitch contour's codebook entry.
    pub(crate) contour: usize,
    /// The periodicity index, which picks the filter codebook.
    pub(crate) periodicity: usize,
    /// The filter of each subframe, an entry of that codebook.
    pub(crate) filters: [u8; 4],
    /// The LTP scaling index, 0 when not coded (4.2.7.6.3).
    pub(crate) scaling: usize,
}

/// What the side information of a SILK frame holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Params {
    pub(crate) signal: Signal,
    /// The quantization offset type is "high".
    pub(crate) high_offset: bool,
    /// The gain indices of the subframes: the first an absolute index if
    /// `independent_gain`, else a delta, like the others (4.2.7.4).
    pub(crate) gains: [u8; 4],
    pub(crate) independent_gain: bool,
    /// The LSF stage-1 index, I1 (4.2.7.5.1).
    pub(crate) lsf_stage1: usize,
    /// The LSF stage-2 indices, I2[k], -10 to 10 (4.2.7.5.2).
    pub(crate) lsf_stage2: [i8; 16],
    /// The LSF interpolation weight, w_Q2; 4 (none) for 10 ms frames.
    pub(crate) interpolation: u8,
    pub(crate) pitch: Option<Pitch>,
    /// The LCG seed, 0 to 3 (4.2.7.7).
    pub(crate) seed: u32,
}

impl Params {
    /// Reads a frame's side information. `active` is the frame's VAD flag
    /// (true for every LBRR frame); `previous_lag` is the lag of the
    /// previous frame of the same kind and channel if that frame was coded
    /// and voiced.
    pub(crate) fn read(
        rd: &mut RangeDecoder<'_>,
        rate: Rate,
        subframes: usize,
        active: bool,
        coding: Coding,
        previous_lag: Option<i32>,
    ) -> Self {
        // Frame type (4.2.7.3): 0 and 1 inactive, 2 to 5 active.
        let kind = if active {
            rd.icdf(&t::TYPE_OFFSET_ACTIVE_ICDF, 8) + 2
        } else {
            rd.icdf(&t::TYPE_OFFSET_INACTIVE_ICDF, 8)
        };
        let signal = match kind >> 1 {
            0 => Signal::Inactive,
            1 => Signal::Unvoiced,
            _ => Signal::Voiced,
        };

        // Gains (4.2.7.4): an absolute first gain in two parts, or deltas.
        let independent_gain = coding != Coding::Conditional;
        let mut gains = [0u8; 4];
        for (k, gain) in gains.iter_mut().take(subframes).enumerate() {
            let index = if k == 0 && independent_gain {
                let msb = t::GAIN_MSB_ICDF
                    .get(signal.index())
                    .map_or(0, |icdf| rd.icdf(icdf, 8));
                msb << 3 | rd.icdf(&t::UNIFORM8_ICDF, 8)
            } else {
                rd.icdf(&t::DELTA_GAIN_ICDF, 8)
            };
            *gain = index as u8;
        }

        // LSF indices (4.2.7.5.1, 4.2.7.5.2).
        let codebook = rate.codebook();
        let lsf_stage1 = rd.icdf(codebook.stage1_icdf(signal == Signal::Voiced), 8);
        let mut lsf_stage2 = [0i8; 16];
        for (k, index) in lsf_stage2.iter_mut().take(codebook.order).enumerate() {
            let (icdf, _) = codebook.stage2(lsf_stage1, k);
            let mut value = rd.icdf(icdf, 8) as i32 - 4;
            if value == -4 {
                value -= rd.icdf(&t::NLSF_EXTENSION_ICDF, 8) as i32;
            } else if value == 4 {
                value += rd.icdf(&t::NLSF_EXTENSION_ICDF, 8) as i32;
            }
            *index = value as i8;
        }
        // The interpolation weight (4.2.7.5.5), only in 20 ms frames.
        let interpolation = if subframes == 4 {
            rd.icdf(&t::NLSF_INTERP_ICDF, 8) as u8
        } else {
            4
        };

        let pitch = (signal == Signal::Voiced)
            .then(|| read_pitch(rd, rate, subframes, coding, previous_lag));
        let seed = rd.icdf(&t::UNIFORM4_ICDF, 8) as u32;
        Self {
            signal,
            high_offset: kind & 1 == 1,
            gains,
            independent_gain,
            lsf_stage1,
            lsf_stage2,
            interpolation,
            pitch,
            seed,
        }
    }
}

/// The LTP parameters of a voiced frame (4.2.7.6).
fn read_pitch(
    rd: &mut RangeDecoder<'_>,
    rate: Rate,
    subframes: usize,
    coding: Coding,
    previous_lag: Option<i32>,
) -> Pitch {
    // The primary lag (4.2.7.6.1): relative to the previous frame's when
    // that one was coded and voiced, unless the delta is 0.
    let relative = match (coding, previous_lag) {
        (Coding::Conditional, Some(previous)) => match rd.icdf(&t::PITCH_DELTA_ICDF, 8) {
            0 => None,
            delta => Some(previous + delta as i32 - 9),
        },
        _ => None,
    };
    let lag = relative.unwrap_or_else(|| {
        let high = rd.icdf(&t::PITCH_LAG_HIGH_ICDF, 8) as i32;
        let low = rd.icdf(rate.lag_low_icdf(), 8) as i32;
        high * rate.lag_scale() + low
    });
    let contour = rd.icdf(rate.contour_icdf(subframes), 8);
    // The filters (4.2.7.6.2) and the scaling (4.2.7.6.3).
    let periodicity = rd.icdf(&t::LTP_PERIODICITY_ICDF, 8);
    let filter_icdf: &[u8] = match periodicity {
        0 => &t::LTP_FILTER_ICDF_0,
        1 => &t::LTP_FILTER_ICDF_1,
        _ => &t::LTP_FILTER_ICDF_2,
    };
    let mut filters = [0u8; 4];
    for filter in filters.iter_mut().take(subframes) {
        *filter = rd.icdf(filter_icdf, 8) as u8;
    }
    let scaling = if coding == Coding::Independent {
        rd.icdf(&t::LTP_SCALE_ICDF, 8)
    } else {
        0
    };
    Pitch {
        lag,
        contour,
        periodicity,
        filters,
        scaling,
    }
}

/// The LSF codebook of a rate: NB and MB share one of order 10, WB has
/// one of order 16 (4.2.7.5).
pub(crate) struct Codebook {
    pub(crate) order: usize,
    /// The stage-2 quantization step in Q16 (4.2.7.5.2).
    pub(crate) step_q16: i32,
    /// The minimum spacing of the LSFs, `order + 1` entries (Table 25).
    pub(crate) min_spacing: &'static [i16],
    /// The stage-1 vectors in Q8, `order` entries each (Tables 23, 24).
    stage1: &'static [u8],
    /// The backward prediction weights in Q8: two lists of `order - 1`
    /// (Table 20).
    prediction: &'static [u8],
    stage1_icdf: &'static [u8],
    /// Two 4-bit contexts per byte, one per coefficient, `order / 2`
    /// bytes per stage-1 index: bits 1 to 3 pick the stage-2 table, bit 0
    /// the prediction weight list.
    stage2_select: &'static [u8],
    stage2_icdf: &'static [u8],
}

/// The entries of each stage-2 table: the residuals -4 to 4 (Tables 15
/// and 16).
const STAGE2_SYMBOLS: usize = 9;

/// The stage-1 indices (Table 14).
const STAGE1_VECTORS: usize = 32;

pub(crate) static NARROW_CODEBOOK: Codebook = Codebook {
    order: 10,
    step_q16: 11796,
    min_spacing: &t::NLSF_NB_MIN_SPACING_Q15,
    stage1: &t::NLSF_NB_STAGE1,
    prediction: &t::NLSF_NB_PRED,
    stage1_icdf: &t::NLSF_NB_STAGE1_ICDF,
    stage2_select: &t::NLSF_NB_STAGE2_SELECT,
    stage2_icdf: &t::NLSF_NB_STAGE2_ICDF,
};

pub(crate) static WIDE_CODEBOOK: Codebook = Codebook {
    order: 16,
    step_q16: 9830,
    min_spacing: &t::NLSF_WB_MIN_SPACING_Q15,
    stage1: &t::NLSF_WB_STAGE1,
    prediction: &t::NLSF_WB_PRED,
    stage1_icdf: &t::NLSF_WB_STAGE1_ICDF,
    stage2_select: &t::NLSF_WB_STAGE2_SELECT,
    stage2_icdf: &t::NLSF_WB_STAGE2_ICDF,
};

impl Codebook {
    /// The stage-1 table: one for inactive and unvoiced frames, one for
    /// voiced ones.
    fn stage1_icdf(&self, voiced: bool) -> &'static [u8] {
        let at = if voiced { STAGE1_VECTORS } else { 0 };
        self.stage1_icdf
            .get(at..at + STAGE1_VECTORS)
            .unwrap_or(&[0])
    }

    /// Stage-1 vector `i1`.
    pub(crate) fn stage1_vector(&self, i1: usize) -> &'static [u8] {
        let at = i1 * self.order;
        self.stage1.get(at..at + self.order).unwrap_or(&[])
    }

    /// The backward prediction weight of coefficient `k` from the next
    /// one, under stage-1 index `i1` (Tables 20 to 22); the last
    /// coefficient has none.
    pub(crate) fn prediction(&self, i1: usize, k: usize) -> u8 {
        let (_, second) = self.stage2(i1, k);
        let at = k + usize::from(second) * (self.order - 1);
        if k + 1 < self.order {
            self.prediction.get(at).copied().unwrap_or(0)
        } else {
            0
        }
    }

    /// The stage-2 table of coefficient `k` under stage-1 index `i1`, and
    /// whether it takes its prediction weight from the second list
    /// (Tables 17, 18, 21 and 22).
    pub(crate) fn stage2(&self, i1: usize, k: usize) -> (&'static [u8], bool) {
        let byte = self
            .stage2_select
            .get(i1 * self.order / 2 + k / 2)
            .copied()
            .unwrap_or(0);
        let context = if k % 2 == 0 { byte } else { byte >> 4 };
        let at = usize::from(context >> 1 & 7) * STAGE2_SYMBOLS;
        let icdf = self
            .stage2_icdf
            .get(at..at + STAGE2_SYMBOLS)
            .unwrap_or(&[0]);
        (icdf, context & 1 == 1)
    }
}

impl Rate {
    pub(crate) fn codebook(self) -> &'static Codebook {
        match self {
            Self::Narrow | Self::Medium => &NARROW_CODEBOOK,
            Self::Wide => &WIDE_CODEBOOK,
        }
    }

    /// The low part of an absolute lag (Table 30).
    fn lag_low_icdf(self) -> &'static [u8] {
        match self {
            Self::Narrow => &t::UNIFORM4_ICDF,
            Self::Medium => &t::UNIFORM6_ICDF,
            Self::Wide => &t::UNIFORM8_ICDF,
        }
    }

    /// The scale of the high part of an absolute lag (Table 30).
    fn lag_scale(self) -> i32 {
        self.khz() as i32 / 2
    }

    /// The pitch contour's table (Table 32).
    fn contour_icdf(self, subframes: usize) -> &'static [u8] {
        match (self, subframes) {
            (Self::Narrow, 4) => &t::PITCH_CONTOUR_NB_ICDF,
            (Self::Narrow, _) => &t::PITCH_CONTOUR_10MS_NB_ICDF,
            (_, 4) => &t::PITCH_CONTOUR_ICDF,
            _ => &t::PITCH_CONTOUR_10MS_ICDF,
        }
    }
}
