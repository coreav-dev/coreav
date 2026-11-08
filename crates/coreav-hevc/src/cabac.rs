use crate::{Error, Result, bits::Bits};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Initialization {
    I,
    P,
    B,
}

impl Initialization {
    pub(crate) fn select<T>(self, [intra, predictive, bipredictive]: [T; 3]) -> T {
        match self {
            Self::I => intra,
            Self::P => predictive,
            Self::B => bipredictive,
        }
    }

    pub(crate) fn for_slice(kind: u32, swap: bool) -> Result<Self> {
        match (kind, swap) {
            (2, false) => Ok(Self::I),
            (1, false) | (0, true) => Ok(Self::P),
            (0, false) | (1, true) => Ok(Self::B),
            _ => Err(Error::InvalidData("cabac_initialization")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    state: u8,
    mps: bool,
}

impl Context {
    pub(crate) fn new(init_value: u8, slice_qp_y: i32) -> Self {
        let slope = i32::from(init_value >> 4) * 5 - 45;
        let offset = i32::from(init_value & 15) * 8 - 16;
        let pre = (((slope * slice_qp_y.clamp(0, 51)) >> 4) + offset).clamp(1, 126);
        let mps = pre >= 64;
        Self {
            state: if mps { pre - 64 } else { 63 - pre } as u8,
            mps,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Active,
    Terminated,
    Failed,
}

pub(crate) struct Cabac<'a, 'b> {
    input: Bits<'a>,
    range: u16,
    offset: u16,
    last_bit: bool,
    phase: Phase,
    remaining_bin_budget: &'b mut u64,
}

impl<'a, 'b> Cabac<'a, 'b> {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn snapshot(&self) -> (usize, u16, u16, bool, u8, u64) {
        (
            self.position(),
            self.range,
            self.offset,
            self.last_bit,
            self.phase as u8,
            *self.remaining_bin_budget,
        )
    }

    pub(crate) fn max_bins(bytes: usize) -> u64 {
        (bytes as u64)
            .saturating_mul(8)
            .saturating_add(1)
            .saturating_mul(128)
    }

    pub(crate) fn new(bytes: &'a [u8], remaining_bin_budget: &'b mut u64) -> Result<Self> {
        let mut decoder = Self {
            input: Bits::new(bytes),
            range: 510,
            offset: 0,
            last_bit: false,
            phase: Phase::Active,
            remaining_bin_budget,
        };
        decoder.initialize()?;
        Ok(decoder)
    }

    pub(crate) fn position(&self) -> usize {
        self.input.position()
    }

    pub(crate) fn raw_bits(&mut self, width: u32) -> Result<u32> {
        if self.phase != Phase::Terminated || width > 32 {
            self.phase = Phase::Failed;
            return Err(Error::InvalidData("cabac_raw_state"));
        }
        let result = self.input.u(width);
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    pub(crate) fn restart(&mut self) -> Result<()> {
        if self.phase != Phase::Terminated || self.position() % 8 != 0 {
            self.phase = Phase::Failed;
            return Err(Error::InvalidData("cabac_restart_state"));
        }
        self.range = 510;
        self.offset = 0;
        self.last_bit = false;
        let result = self.initialize();
        self.phase = if result.is_ok() {
            Phase::Active
        } else {
            Phase::Failed
        };
        result
    }

    fn initialize(&mut self) -> Result<()> {
        for _ in 0..9 {
            self.offset = (self.offset << 1) | self.read_bit()?;
        }
        if self.offset >= 510 {
            return Err(Error::InvalidData("cabac_initial_offset"));
        }
        Ok(())
    }

    pub(crate) fn decision(&mut self, context: &mut Context) -> Result<bool> {
        self.start_bin()?;
        let result = self.decode_decision(context);
        self.record(result)
    }

    pub(crate) fn bypass(&mut self) -> Result<bool> {
        self.start_bin()?;
        let result = self.decode_bypass();
        self.record(result)
    }

    pub(crate) fn terminate(&mut self) -> Result<bool> {
        self.start_bin()?;
        let result = self.decode_terminate();
        self.record(result)
    }

    fn start_bin(&mut self) -> Result<()> {
        if self.phase != Phase::Active {
            self.phase = Phase::Failed;
            return Err(Error::InvalidData("cabac_ended"));
        }
        let Some(remaining) = self.remaining_bin_budget.checked_sub(1) else {
            self.phase = Phase::Failed;
            return Err(Error::Limit("cabac_bin_budget"));
        };
        *self.remaining_bin_budget = remaining;
        Ok(())
    }

    fn record(&mut self, result: Result<bool>) -> Result<bool> {
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    fn read_bit(&mut self) -> Result<u16> {
        self.last_bit = self
            .input
            .flag()
            .map_err(|_| Error::InvalidData("cabac_truncated"))?;
        Ok(u16::from(self.last_bit))
    }

    fn decode_decision(&mut self, context: &mut Context) -> Result<bool> {
        let column = usize::from((self.range >> 6) & 3);
        let lps_range = RANGE_TAB_LPS
            .get(usize::from(context.state))
            .and_then(|row| row.get(column))
            .copied()
            .ok_or(Error::InvalidData("cabac_context"))?;
        self.range -= u16::from(lps_range);
        let lps = self.offset >= self.range;
        if lps {
            self.offset -= self.range;
            self.range = u16::from(lps_range);
        }
        let bin = context.mps ^ lps;
        let transitions = if lps { &TRANS_IDX_LPS } else { &TRANS_IDX_MPS };
        let state = transitions
            .get(usize::from(context.state))
            .copied()
            .ok_or(Error::InvalidData("cabac_context"))?;
        self.renormalize()?;
        if lps && context.state == 0 {
            context.mps = !context.mps;
        }
        context.state = state;
        Ok(bin)
    }

    fn renormalize(&mut self) -> Result<()> {
        while self.range < 256 {
            self.range <<= 1;
            self.offset = (self.offset << 1) | self.read_bit()?;
        }
        if self.offset >= self.range {
            return Err(Error::InvalidData("cabac_offset"));
        }
        Ok(())
    }

    fn decode_bypass(&mut self) -> Result<bool> {
        self.offset = (self.offset << 1) | self.read_bit()?;
        let bin = self.offset >= self.range;
        if bin {
            self.offset -= self.range;
        }
        if self.offset >= self.range {
            return Err(Error::InvalidData("cabac_offset"));
        }
        Ok(bin)
    }

    fn decode_terminate(&mut self) -> Result<bool> {
        self.range -= 2;
        if self.offset >= self.range {
            if !self.last_bit {
                return Err(Error::InvalidData("cabac_termination_bit"));
            }
            self.phase = Phase::Terminated;
            return Ok(true);
        }
        self.renormalize()?;
        Ok(false)
    }
}

const RANGE_TAB_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

const TRANS_IDX_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21,
    21, 22, 22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34,
    34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];

const TRANS_IDX_MPS: [u8; 64] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 62, 63,
];

#[cfg(test)]
mod tests {
    use super::{Cabac, Context, Initialization};
    use crate::Error;

    #[test]
    fn substream_bound_covers_the_longest_mps_runs_and_truncated_reads() {
        assert!(
            super::RANGE_TAB_LPS
                .iter()
                .flatten()
                .all(|range| *range >= 2)
        );
        for length in 2..=64 {
            let bytes = vec![0; length];
            let allowance = Cabac::max_bins(length);
            let mut remaining = allowance;
            let mut decoder = Cabac::new(&bytes, &mut remaining).unwrap();
            let mut context = Context {
                state: 63,
                mps: false,
            };
            loop {
                match decoder.decision(&mut context) {
                    Ok(_) => {}
                    Err(error) => {
                        assert_eq!(error, Error::InvalidData("cabac_truncated"));
                        break;
                    }
                }
            }
            assert!(remaining > 0);
        }
    }

    #[test]
    fn slice_initialization_and_swap_follow_equation_9_7() {
        for (kind, swap, expected) in [
            (2, false, Initialization::I),
            (1, false, Initialization::P),
            (1, true, Initialization::B),
            (0, false, Initialization::B),
            (0, true, Initialization::P),
        ] {
            assert_eq!(Initialization::for_slice(kind, swap), Ok(expected));
        }
        for (kind, swap) in [(2, true), (3, false), (u32::MAX, true)] {
            assert_eq!(
                Initialization::for_slice(kind, swap),
                Err(Error::InvalidData("cabac_initialization"))
            );
        }
        assert_eq!(Initialization::I.select([0, 1, 2]), 0);
        assert_eq!(Initialization::P.select([0, 1, 2]), 1);
        assert_eq!(Initialization::B.select([0, 1, 2]), 2);
    }

    #[test]
    fn raw_pcm_samples_and_restart_keep_the_same_picture_budget() -> crate::Result<()> {
        let mut budget = 2;
        let mut decoder = Cabac::new(&[0xfe, 0x80, 0xab, 0xfe, 0x80], &mut budget)?;
        assert!(decoder.terminate()?);
        assert_eq!(decoder.raw_bits(7)?, 0);
        assert_eq!(decoder.raw_bits(8)?, 0xab);
        decoder.restart()?;
        assert_eq!(decoder.position(), 33);
        assert!(decoder.terminate()?);
        assert_eq!(budget, 0);
        Ok(())
    }

    #[test]
    fn raw_and_restart_cannot_reopen_a_failed_engine() -> crate::Result<()> {
        let mut budget = 10;
        let mut decoder = Cabac::new(&[0xfe, 0x80], &mut budget)?;
        assert!(decoder.raw_bits(1).is_err());
        assert!(decoder.restart().is_err());
        assert!(decoder.terminate().is_err());
        let mut decoder = Cabac::new(&[0xfe, 0x80], &mut budget)?;
        assert!(decoder.terminate()?);
        assert!(decoder.restart().is_err());
        assert!(decoder.raw_bits(7).is_err());
        let mut decoder = Cabac::new(&[0xfe, 0x80], &mut budget)?;
        assert!(decoder.terminate()?);
        assert_eq!(decoder.raw_bits(7)?, 0);
        assert!(decoder.restart().is_err());
        assert!(decoder.raw_bits(0).is_err());
        Ok(())
    }

    const LPS_COLUMNS: [[u16; 64]; 4] = [
        [
            128, 128, 128, 123, 116, 111, 105, 100, 95, 90, 85, 81, 77, 73, 69, 66, 62, 59, 56, 53,
            51, 48, 46, 43, 41, 39, 37, 35, 33, 32, 30, 29, 27, 26, 24, 23, 22, 21, 20, 19, 18, 17,
            16, 15, 14, 14, 13, 12, 12, 11, 11, 10, 10, 9, 9, 8, 8, 7, 7, 7, 6, 6, 6, 2,
        ],
        [
            176, 167, 158, 150, 142, 135, 128, 122, 116, 110, 104, 99, 94, 89, 85, 80, 76, 72, 69,
            65, 62, 59, 56, 53, 50, 48, 45, 43, 41, 39, 37, 35, 33, 31, 30, 28, 27, 26, 24, 23, 22,
            21, 20, 19, 18, 17, 16, 15, 14, 14, 13, 12, 12, 11, 11, 10, 9, 9, 9, 8, 8, 7, 7, 2,
        ],
        [
            208, 197, 187, 178, 169, 160, 152, 144, 137, 130, 123, 117, 111, 105, 100, 95, 90, 86,
            81, 77, 73, 69, 66, 63, 59, 56, 54, 51, 48, 46, 43, 41, 39, 37, 35, 33, 32, 30, 29, 27,
            26, 25, 23, 22, 21, 20, 19, 18, 17, 16, 15, 15, 14, 13, 12, 12, 11, 11, 10, 10, 9, 9,
            8, 2,
        ],
        [
            240, 227, 216, 205, 195, 185, 175, 166, 158, 150, 142, 135, 128, 122, 116, 110, 104,
            99, 94, 89, 85, 80, 76, 72, 69, 65, 62, 59, 56, 53, 50, 48, 45, 43, 41, 39, 37, 35, 33,
            31, 30, 28, 27, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 14, 13, 12, 12, 11, 11,
            10, 9, 2,
        ],
    ];

    const LPS_NEXT: [u8; 64] = [
        0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19,
        21, 21, 22, 22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33,
        33, 34, 34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
    ];

    #[test]
    fn initialization_clips_qp_and_pre_context_state_at_both_extremes() {
        assert_eq!(
            Context::new(0, i32::MIN),
            Context {
                state: 62,
                mps: false
            }
        );
        assert_eq!(
            Context::new(255, i32::MAX),
            Context {
                state: 62,
                mps: true
            }
        );
        assert_eq!(
            Context::new(154, 26),
            Context {
                state: 0,
                mps: true
            }
        );
        assert_eq!(
            Context::new(139, 26),
            Context {
                state: 0,
                mps: false
            }
        );
        assert_eq!(
            Context::new(184, 26),
            Context {
                state: 0,
                mps: true
            }
        );
        assert_eq!(
            Context::new(63, 51),
            Context {
                state: 55,
                mps: false
            }
        );
    }

    #[test]
    fn every_init_value_matches_equations_9_4_through_9_6() {
        for init in 0..=255u8 {
            for qp in -12..=63i32 {
                let m = i64::from(init / 16) * 5 - 45;
                let n = i64::from(init % 16) * 8 - 16;
                let pre = ((m * i64::from(qp.clamp(0, 51))).div_euclid(16) + n).clamp(1, 126);
                let ctx = Context::new(init, qp);
                assert_eq!(ctx.mps, pre > 63);
                assert_eq!(
                    i64::from(ctx.state),
                    if pre > 63 { pre - 64 } else { 63 - pre }
                );
            }
        }
    }

    #[test]
    fn every_state_column_and_interval_boundary_matches_tables_9_52_and_9_53() {
        for state in 0..64u8 {
            for range in [256, 319, 320, 383, 384, 447, 448, 510u16] {
                let column = usize::from((range - 256) / 64);
                let lps_range = LPS_COLUMNS[column][usize::from(state)];
                let threshold = range - lps_range;
                for offset in [0, threshold - 1, threshold, range - 1] {
                    for mps in [false, true] {
                        let lps = offset >= threshold;
                        let reduced_range = if lps { lps_range } else { threshold };
                        let reduced_offset = if lps { offset - threshold } else { offset };
                        let shift = reduced_range.leading_zeros().saturating_sub(7);
                        let mut budget = 1;
                        let mut engine = Cabac::new(&[0, 0, 0], &mut budget).unwrap();
                        engine.range = range;
                        engine.offset = offset;
                        let mut ctx = Context { state, mps };
                        assert_eq!(engine.decision(&mut ctx).unwrap(), mps ^ lps);
                        assert_eq!(engine.range, reduced_range << shift);
                        assert_eq!(engine.offset, reduced_offset << shift);
                        assert_eq!(engine.position(), 9 + shift as usize);
                        assert_eq!(ctx.mps, mps ^ (lps && state == 0));
                        assert_eq!(
                            ctx.state,
                            if lps {
                                LPS_NEXT[usize::from(state)]
                            } else if state == 63 {
                                63
                            } else {
                                (state + 1).min(62)
                            }
                        );
                        assert_eq!(*engine.remaining_bin_budget, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn arithmetic_sequence_consumes_only_normatively_requested_bits() {
        let mut budget = 4;
        let mut engine = Cabac::new(&[0x87, 0xa0], &mut budget).unwrap();
        let mut ctx = Context {
            state: 0,
            mps: false,
        };
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (510, 271, 9)
        );
        assert_eq!(engine.decision(&mut ctx), Ok(true));
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (480, 2, 10)
        );
        assert_eq!(
            ctx,
            Context {
                state: 0,
                mps: true
            }
        );
        assert_eq!(engine.bypass(), Ok(false));
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (480, 5, 11)
        );
        assert_eq!(engine.decision(&mut ctx), Ok(true));
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (480, 10, 12)
        );
        assert_eq!(
            ctx,
            Context {
                state: 1,
                mps: true
            }
        );
        assert_eq!(engine.terminate(), Ok(false));
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (478, 10, 12)
        );
        assert_eq!(*engine.remaining_bin_budget, 0);
    }

    #[test]
    fn initial_offset_510_and_511_are_forbidden() {
        for bytes in [[255, 0], [255, 128]] {
            let mut budget = 1;
            assert!(matches!(
                Cabac::new(&bytes, &mut budget),
                Err(Error::InvalidData("cabac_initial_offset"))
            ));
            assert_eq!(budget, 1);
        }
        let mut budget = 0;
        let engine = Cabac::new(&[254, 128], &mut budget).unwrap();
        assert_eq!(engine.offset, 509);
        assert_eq!(engine.position(), 9);
    }

    #[test]
    fn initialization_never_pads_a_short_input() {
        for bytes in [&[][..], &[0][..]] {
            let mut budget = 1;
            assert!(matches!(
                Cabac::new(bytes, &mut budget),
                Err(Error::InvalidData("cabac_truncated"))
            ));
            assert_eq!(budget, 1);
        }
    }

    #[test]
    fn bypass_subdivision_uses_the_exact_range_threshold() {
        for (bytes, bin, offset) in [
            ([0, 0], false, 0),
            ([127, 64], false, 509),
            ([127, 128], true, 0),
            ([254, 192], true, 509),
        ] {
            let mut budget = 1;
            let mut engine = Cabac::new(&bytes, &mut budget).unwrap();
            assert_eq!(engine.bypass(), Ok(bin));
            assert_eq!(engine.range, 510);
            assert_eq!(engine.offset, offset);
            assert_eq!(engine.position(), 10);
        }
    }

    #[test]
    fn bypass_exhausts_exactly_at_the_supplied_bit_boundary() {
        let mut budget = 8;
        let mut engine = Cabac::new(&[0, 0], &mut budget).unwrap();
        for pos in 10..=16 {
            assert_eq!(engine.bypass(), Ok(false));
            assert_eq!(engine.position(), pos);
        }
        assert_eq!(engine.bypass(), Err(Error::InvalidData("cabac_truncated")));
        assert_eq!(engine.position(), 16);
        assert_eq!(*engine.remaining_bin_budget, 0);
        assert_eq!(engine.terminate(), Err(Error::InvalidData("cabac_ended")));
    }

    #[test]
    fn seven_bit_renormalization_reads_the_last_real_bit_without_padding() {
        let mut budget = 2;
        let mut engine = Cabac::new(&[254, 128], &mut budget).unwrap();
        let mut ctx = Context {
            state: 63,
            mps: false,
        };
        assert_eq!(engine.decision(&mut ctx), Ok(true));
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (256, 128, 16)
        );
        assert_eq!(
            ctx,
            Context {
                state: 63,
                mps: false
            }
        );
        assert_eq!(
            engine.decision(&mut ctx),
            Err(Error::InvalidData("cabac_truncated"))
        );
        assert_eq!(engine.position(), 16);
        assert_eq!(
            ctx,
            Context {
                state: 63,
                mps: false
            }
        );
    }

    #[test]
    fn truncated_renormalization_does_not_publish_a_context_transition() {
        let mut budget = 1;
        let mut engine = Cabac::new(&[0, 0], &mut budget).unwrap();
        engine.input.skip(7).unwrap();
        engine.offset = 270;
        let mut ctx = Context {
            state: 0,
            mps: false,
        };
        assert_eq!(
            engine.decision(&mut ctx),
            Err(Error::InvalidData("cabac_truncated"))
        );
        assert_eq!(
            ctx,
            Context {
                state: 0,
                mps: false
            }
        );
        assert_eq!(
            engine.decision(&mut ctx),
            Err(Error::InvalidData("cabac_ended"))
        );
        assert_eq!(engine.position(), 16);
    }

    #[test]
    fn termination_one_stops_without_renormalizing_or_reading_another_bit() {
        let mut budget = 3;
        let mut engine = Cabac::new(&[254, 128], &mut budget).unwrap();
        assert_eq!(engine.terminate(), Ok(true));
        assert_eq!(
            (engine.range, engine.offset, engine.position()),
            (508, 509, 9)
        );
        assert_eq!(*engine.remaining_bin_budget, 2);
        let mut ctx = Context::new(154, 26);
        assert_eq!(
            engine.decision(&mut ctx),
            Err(Error::InvalidData("cabac_ended"))
        );
        assert_eq!(engine.bypass(), Err(Error::InvalidData("cabac_ended")));
        assert_eq!(engine.terminate(), Err(Error::InvalidData("cabac_ended")));
        assert_eq!(*engine.remaining_bin_budget, 2);
    }

    #[test]
    fn termination_requires_the_last_inserted_bit_to_be_one() {
        let mut budget = 1;
        let mut engine = Cabac::new(&[254, 0], &mut budget).unwrap();
        assert_eq!(
            engine.terminate(),
            Err(Error::InvalidData("cabac_termination_bit"))
        );
        assert_eq!(engine.position(), 9);
        assert_eq!(engine.bypass(), Err(Error::InvalidData("cabac_ended")));
    }

    #[test]
    fn termination_zero_renormalizes_only_when_range_falls_below_256() {
        for range in [256, 257, 258, 510u16] {
            let mut budget = 1;
            let mut engine = Cabac::new(&[0, 64], &mut budget).unwrap();
            engine.range = range;
            assert_eq!(engine.terminate(), Ok(false));
            if range < 258 {
                assert_eq!(
                    (engine.range, engine.offset, engine.position()),
                    ((range - 2) * 2, 1, 10)
                );
            } else {
                assert_eq!(
                    (engine.range, engine.offset, engine.position()),
                    (range - 2, 0, 9)
                );
            }
        }
    }

    #[test]
    fn each_bin_mode_obeys_the_same_picture_budget_before_mutating_state() {
        for mode in 0..3 {
            let mut budget = 0;
            let mut engine = Cabac::new(&[0, 0], &mut budget).unwrap();
            let mut ctx = Context::new(139, 26);
            let before = ctx;
            let result = match mode {
                0 => engine.decision(&mut ctx),
                1 => engine.bypass(),
                _ => engine.terminate(),
            };
            assert_eq!(result, Err(Error::Limit("cabac_bin_budget")));
            assert_eq!(
                (engine.range, engine.offset, engine.position()),
                (510, 0, 9)
            );
            assert_eq!(ctx, before);
            assert_eq!(engine.bypass(), Err(Error::InvalidData("cabac_ended")));
        }
    }

    #[test]
    fn restarting_arithmetic_does_not_reset_budget_or_copied_contexts() {
        let mut budget = 2;
        let mut ctx = Context::new(154, 26);
        {
            let mut first = Cabac::new(&[0, 0], &mut budget).unwrap();
            assert_eq!(first.decision(&mut ctx), Ok(true));
            assert_eq!(
                ctx,
                Context {
                    state: 1,
                    mps: true
                }
            );
        }
        assert_eq!(budget, 1);
        let saved_context = ctx;
        {
            let mut next = Cabac::new(&[0, 0], &mut budget).unwrap();
            assert_eq!((next.range, next.offset, next.position()), (510, 0, 9));
            assert_eq!(next.decision(&mut ctx), Ok(true));
            assert_eq!(
                ctx,
                Context {
                    state: 2,
                    mps: true
                }
            );
            assert_eq!(next.bypass(), Err(Error::Limit("cabac_bin_budget")));
        }
        assert_eq!(budget, 0);
        assert_eq!(
            saved_context,
            Context {
                state: 1,
                mps: true
            }
        );
    }
}
