use crate::cabac::{Cabac, Context, Initialization};
use crate::{Error, Result};

const LAST_INIT: [u8; 18] = [
    110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63,
];
const SUBBLOCK_INIT: [u8; 4] = [91, 171, 134, 141];
const SIGNIFICANCE_INIT: [u8; 42] = [
    111, 111, 125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179,
    153, 125, 107, 125, 141, 179, 153, 125, 140, 139, 182, 182, 152, 136, 152, 136, 153, 136, 139,
    111, 136, 139, 111,
];
const GREATER1_INIT: [u8; 24] = [
    140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140, 179, 166,
    182, 140, 227, 122, 197,
];
const GREATER2_INIT: [u8; 6] = [138, 153, 136, 167, 152, 152];
const LAST_P_INIT: [u8; 18] = [
    125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108,
];
const LAST_B_INIT: [u8; 18] = [
    125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126, 111, 111, 79, 108, 123, 93,
];
const SUBBLOCK_INTER_INIT: [u8; 4] = [121, 140, 61, 154];
const SIGNIFICANCE_P_INIT: [u8; 42] = [
    155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136,
    153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 123, 123, 107, 121, 107, 121, 167, 151, 183,
    140, 151, 183, 140,
];
const SIGNIFICANCE_B_INIT: [u8; 42] = [
    170, 154, 139, 153, 139, 123, 123, 63, 124, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136,
    153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 138, 138, 122, 121, 122, 121, 167, 151, 183,
    140, 151, 183, 140,
];
const GREATER1_P_INIT: [u8; 24] = [
    154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 137, 169, 194, 166,
    167, 154, 167, 137, 182,
];
const GREATER1_B_INIT: [u8; 24] = [
    154, 196, 167, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 122, 169, 208, 166,
    167, 154, 152, 167, 182,
];
const GREATER2_P_INIT: [u8; 6] = [107, 167, 91, 122, 107, 167];
const GREATER2_B_INIT: [u8; 6] = [107, 167, 91, 107, 107, 167];
const SIGNIFICANCE_4X4: [usize; 15] = [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8];

#[derive(Clone, Copy)]
pub(crate) struct Contexts {
    transform_skip: [Context; 2],
    last_x: [Context; 18],
    last_y: [Context; 18],
    subblock: [Context; 4],
    significance: [Context; 42],
    greater1: [Context; 24],
    greater2: [Context; 6],
}

impl Contexts {
    pub(crate) fn new(slice_qp_y: i32, initialization: Initialization) -> Self {
        let (last, subblock, significance, greater1, greater2) = match initialization {
            Initialization::I => (
                LAST_INIT,
                SUBBLOCK_INIT,
                SIGNIFICANCE_INIT,
                GREATER1_INIT,
                GREATER2_INIT,
            ),
            Initialization::P => (
                LAST_P_INIT,
                SUBBLOCK_INTER_INIT,
                SIGNIFICANCE_P_INIT,
                GREATER1_P_INIT,
                GREATER2_P_INIT,
            ),
            Initialization::B => (
                LAST_B_INIT,
                SUBBLOCK_INTER_INIT,
                SIGNIFICANCE_B_INIT,
                GREATER1_B_INIT,
                GREATER2_B_INIT,
            ),
        };
        Self {
            transform_skip: [Context::new(139, slice_qp_y); 2],
            last_x: last.map(|value| Context::new(value, slice_qp_y)),
            last_y: last.map(|value| Context::new(value, slice_qp_y)),
            subblock: subblock.map(|value| Context::new(value, slice_qp_y)),
            significance: significance.map(|value| Context::new(value, slice_qp_y)),
            greater1: greater1.map(|value| Context::new(value, slice_qp_y)),
            greater2: greater2.map(|value| Context::new(value, slice_qp_y)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Component {
    Luma,
    Chroma,
}

impl Component {
    fn chroma(self) -> usize {
        usize::from(self == Self::Chroma)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    pub(crate) log2_size: u8,
    pub(crate) component: Component,
    pub(crate) intra_mode: Option<u8>,
    pub(crate) transform_skip_enabled: bool,
    pub(crate) transquant_bypass: bool,
    pub(crate) sign_data_hiding_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scan {
    Diagonal,
    Horizontal,
    Vertical,
}

impl Block {
    fn scan(self) -> Scan {
        if self.log2_size == 2 || (self.log2_size == 3 && self.component == Component::Luma) {
            match self.intra_mode {
                Some(6..=14) => Scan::Vertical,
                Some(22..=30) => Scan::Horizontal,
                _ => Scan::Diagonal,
            }
        } else {
            Scan::Diagonal
        }
    }
}

trait Bins {
    fn decision(&mut self, context: &mut Context) -> Result<bool>;
    fn bypass(&mut self) -> Result<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Decoded {
    pub transform_skip: bool,
    pub sign_hidden_groups: u64,
}

struct LevelState {
    greater1: u8,
    hidden_groups: u64,
}

impl LevelState {
    fn new(greater1: u8) -> Self {
        Self {
            greater1,
            hidden_groups: 0,
        }
    }
}

impl Bins for Cabac<'_, '_> {
    fn decision(&mut self, context: &mut Context) -> Result<bool> {
        Cabac::decision(self, context)
    }

    fn bypass(&mut self) -> Result<bool> {
        Cabac::bypass(self)
    }
}

pub(crate) fn decode(
    cabac: &mut Cabac<'_, '_>,
    contexts: &mut Contexts,
    block: Block,
    output: &mut [i16],
) -> Result<Decoded> {
    decode_block(cabac, contexts, block, output)
}

fn decode_block(
    bins: &mut impl Bins,
    contexts: &mut Contexts,
    block: Block,
    output: &mut [i16],
) -> Result<Decoded> {
    if !(2..=5).contains(&block.log2_size) || block.intra_mode.is_some_and(|mode| mode > 34) {
        return Err(Error::InvalidData("residual_block"));
    }
    let side = 1usize << block.log2_size;
    let output = output
        .get_mut(..side * side)
        .ok_or(Error::InvalidData("residual_output"))?;
    output.fill(0);
    let transform_skip =
        if block.transform_skip_enabled && !block.transquant_bypass && block.log2_size == 2 {
            decision(bins, &mut contexts.transform_skip, block.component.chroma())?
        } else {
            false
        };
    let x_prefix = last_prefix(bins, &mut contexts.last_x, block)?;
    let y_prefix = last_prefix(bins, &mut contexts.last_y, block)?;
    let mut last_x = last_coordinate(bins, x_prefix)?;
    let mut last_y = last_coordinate(bins, y_prefix)?;
    let scan = block.scan();
    if scan == Scan::Vertical {
        core::mem::swap(&mut last_x, &mut last_y);
    }
    let group_side = side / 4;
    let last_group = scan_index(group_side, scan, (last_x / 4, last_y / 4))?;
    let last_position = scan_index(4, scan, (last_x % 4, last_y % 4))?;
    let mut coded_groups = [false; 64];
    let mut level_state = LevelState::new(1);
    for group in (0..=last_group).rev() {
        let (gx, gy) = scan_coordinate(group_side, scan, group)?;
        let neighbours = group_neighbours(&coded_groups, group_side, gx, gy)?;
        let coded = if group == last_group || group == 0 {
            true
        } else {
            decision(
                bins,
                &mut contexts.subblock,
                subblock_context(block.component, neighbours),
            )?
        };
        *coded_groups
            .get_mut(gy * group_side + gx)
            .ok_or(Error::InvalidData("residual_subblock"))? = coded;
        if !coded {
            continue;
        }
        let mut coefficients = [Coefficient::default(); 16];
        let end = if group == last_group {
            coefficients
                .get_mut(last_position)
                .ok_or(Error::InvalidData("residual_last"))?
                .significant = true;
            last_position
        } else {
            16
        };
        let mut infer_dc = group != last_group && group != 0;
        for (position, coefficient) in coefficients.iter_mut().enumerate().take(end).rev() {
            let (x, y) = scan_coordinate(4, scan, position)?;
            coefficient.significant = if position == 0 && infer_dc {
                true
            } else {
                decision(
                    bins,
                    &mut contexts.significance,
                    significance_context(block, gx * 4 + x, gy * 4 + y, neighbours)?,
                )?
            };
            if coefficient.significant {
                infer_dc = false;
            }
        }
        let levels = decode_levels(
            bins,
            contexts,
            &mut coefficients,
            block.component,
            group,
            block.sign_data_hiding_enabled && !block.transquant_bypass,
            &mut level_state,
        )?;
        for (position, level) in levels.into_iter().enumerate() {
            let (x, y) = scan_coordinate(4, scan, position)?;
            *output
                .get_mut((gy * 4 + y) * side + gx * 4 + x)
                .ok_or(Error::InvalidData("residual_output"))? = level;
        }
    }
    Ok(Decoded {
        transform_skip,
        sign_hidden_groups: level_state.hidden_groups,
    })
}

fn decision(bins: &mut impl Bins, contexts: &mut [Context], index: usize) -> Result<bool> {
    bins.decision(
        contexts
            .get_mut(index)
            .ok_or(Error::InvalidData("residual_context"))?,
    )
}

fn scan_coordinate(side: usize, scan: Scan, mut position: usize) -> Result<(usize, usize)> {
    if !matches!(side, 1 | 2 | 4 | 8) || position >= side * side {
        return Err(Error::InvalidData("residual_scan"));
    }
    match scan {
        Scan::Horizontal => return Ok((position % side, position / side)),
        Scan::Vertical => return Ok((position / side, position % side)),
        Scan::Diagonal => {}
    }
    for sum in 0..2 * side - 1 {
        let first_x = (sum + 1).saturating_sub(side);
        let count = sum.min(side - 1) - first_x + 1;
        if position < count {
            let x = first_x + position;
            return Ok((x, sum - x));
        }
        position -= count;
    }
    Err(Error::InvalidData("residual_scan"))
}

fn scan_index(side: usize, scan: Scan, coordinate: (usize, usize)) -> Result<usize> {
    for position in 0..side * side {
        if scan_coordinate(side, scan, position)? == coordinate {
            return Ok(position);
        }
    }
    Err(Error::InvalidData("residual_last"))
}

fn last_context(block: Block, bin_index: usize) -> usize {
    let size = usize::from(block.log2_size);
    let (offset, shift) = if block.component == Component::Luma {
        (3 * (size - 2) + ((size - 1) >> 2), (size + 1) >> 2)
    } else {
        (15, size - 2)
    };
    offset + (bin_index >> shift)
}

fn last_prefix(bins: &mut impl Bins, contexts: &mut [Context; 18], block: Block) -> Result<u8> {
    let maximum = 2 * block.log2_size - 1;
    for bin_index in 0..maximum {
        if !decision(bins, contexts, last_context(block, usize::from(bin_index)))? {
            return Ok(bin_index);
        }
    }
    Ok(maximum)
}

fn bypass_bits(bins: &mut impl Bins, width: u8) -> Result<u32> {
    let mut value = 0;
    for _ in 0..width {
        value = (value << 1) | u32::from(bins.bypass()?);
    }
    Ok(value)
}

fn last_coordinate(bins: &mut impl Bins, prefix: u8) -> Result<usize> {
    if prefix <= 3 {
        return Ok(usize::from(prefix));
    }
    let width = (prefix >> 1) - 1;
    Ok(((2 + usize::from(prefix & 1)) << width) + bypass_bits(bins, width)? as usize)
}

fn group_neighbours(groups: &[bool; 64], side: usize, x: usize, y: usize) -> Result<u8> {
    let right = x + 1 < side
        && *groups
            .get(y * side + x + 1)
            .ok_or(Error::InvalidData("residual_subblock"))?;
    let below = y + 1 < side
        && *groups
            .get((y + 1) * side + x)
            .ok_or(Error::InvalidData("residual_subblock"))?;
    Ok(u8::from(right) + 2 * u8::from(below))
}

fn subblock_context(component: Component, neighbours: u8) -> usize {
    component.chroma() * 2 + usize::from(neighbours != 0)
}

fn significance_context(block: Block, x: usize, y: usize, neighbours: u8) -> Result<usize> {
    let chroma = block.component.chroma();
    if block.log2_size == 2 {
        return SIGNIFICANCE_4X4
            .get(y * 4 + x)
            .map(|value| value + chroma * 27)
            .ok_or(Error::InvalidData("residual_significance"));
    }
    if x + y == 0 {
        return Ok(chroma * 27);
    }
    let xp = x & 3;
    let yp = y & 3;
    let context = match neighbours {
        0 => {
            if xp + yp == 0 {
                2
            } else {
                usize::from(xp + yp < 3)
            }
        }
        1 => 2usize.saturating_sub(yp),
        2 => 2usize.saturating_sub(xp),
        3 => 2,
        _ => return Err(Error::InvalidData("residual_neighbours")),
    };
    if block.component == Component::Luma {
        Ok(context
            + 3 * usize::from(x / 4 + y / 4 > 0)
            + if block.log2_size == 3 {
                if block.scan() == Scan::Diagonal {
                    9
                } else {
                    15
                }
            } else {
                21
            })
    } else {
        Ok(27 + context + if block.log2_size == 3 { 9 } else { 12 })
    }
}

#[derive(Clone, Copy, Default)]
struct Coefficient {
    significant: bool,
    greater1: bool,
    greater2: bool,
    negative: bool,
}

fn greater_context_set(component: Component, group: usize, previous: u8) -> usize {
    2 * usize::from(component == Component::Luma && group > 0) + usize::from(previous == 0)
}

fn advance_greater_context(context: u8, flag: bool) -> u8 {
    if context == 0 || flag {
        0
    } else {
        (context + 1).min(3)
    }
}

fn decode_levels(
    bins: &mut impl Bins,
    contexts: &mut Contexts,
    coefficients: &mut [Coefficient; 16],
    component: Component,
    group: usize,
    sign_data_hiding: bool,
    state: &mut LevelState,
) -> Result<[i16; 16]> {
    let set = greater_context_set(component, group, state.greater1);
    let mut greater_context = 1;
    let mut count = 0;
    let mut first = 16;
    let mut last = 0;
    let mut first_greater1 = None;
    for (position, coefficient) in coefficients.iter_mut().enumerate().rev() {
        if !coefficient.significant {
            continue;
        }
        if count == 0 {
            last = position;
        }
        first = position;
        if count < 8 {
            coefficient.greater1 = decision(
                bins,
                &mut contexts.greater1,
                component.chroma() * 16 + set * 4 + usize::from(greater_context),
            )?;
            greater_context = advance_greater_context(greater_context, coefficient.greater1);
            if coefficient.greater1 && first_greater1.is_none() {
                first_greater1 = Some(position);
            }
        }
        count += 1;
    }
    if count == 0 {
        return Ok([0; 16]);
    }
    state.greater1 = greater_context;
    if let Some(position) = first_greater1 {
        coefficients
            .get_mut(position)
            .ok_or(Error::InvalidData("residual_greater2"))?
            .greater2 = decision(bins, &mut contexts.greater2, component.chroma() * 4 + set)?;
    }
    let hidden = sign_data_hiding && last - first > 3;
    state.hidden_groups += u64::from(hidden);
    for (position, coefficient) in coefficients.iter_mut().enumerate().rev() {
        if coefficient.significant && !(hidden && position == first) {
            coefficient.negative = bins.bypass()?;
        }
    }
    let mut levels = [0; 16];
    let mut significant_count = 0;
    let mut sum = 0;
    let mut rice = 0;
    let mut previous_abs = 0;
    for (position, (coefficient, level)) in
        coefficients.iter().zip(levels.iter_mut()).enumerate().rev()
    {
        if !coefficient.significant {
            continue;
        }
        let base = 1 + u32::from(coefficient.greater1) + u32::from(coefficient.greater2);
        let threshold = if significant_count < 8 {
            if Some(position) == first_greater1 {
                3
            } else {
                2
            }
        } else {
            1
        };
        let remaining = if base == threshold {
            rice = next_rice(rice, previous_abs);
            let value = remaining_level(bins, rice)?;
            previous_abs = base + value;
            value
        } else {
            0
        };
        let magnitude = base + remaining;
        if magnitude > 32768 {
            return Err(Error::InvalidData("coefficient_level"));
        }
        sum += magnitude;
        let negative = coefficient.negative || (hidden && position == first && sum & 1 != 0);
        *level = signed_level(magnitude, negative)?;
        significant_count += 1;
    }
    Ok(levels)
}

fn signed_level(magnitude: u32, negative: bool) -> Result<i16> {
    let magnitude =
        i32::try_from(magnitude).map_err(|_| Error::InvalidData("coefficient_level"))?;
    i16::try_from(if negative { -magnitude } else { magnitude })
        .map_err(|_| Error::InvalidData("coefficient_level"))
}

fn next_rice(previous_rice: u8, previous_abs: u32) -> u8 {
    (previous_rice + u8::from(previous_abs > (3 << previous_rice))).min(4)
}

fn remaining_level(bins: &mut impl Bins, rice: u8) -> Result<u32> {
    for quotient in 0..4 {
        if !bins.bypass()? {
            return Ok((quotient << rice) + bypass_bits(bins, rice)?);
        }
    }
    let mut value = 4 << rice;
    for order in rice + 1..=15 {
        if !bins.bypass()? {
            value += bypass_bits(bins, order)?;
            if value > 32767 {
                return Err(Error::InvalidData("coefficient_remaining"));
            }
            return Ok(value);
        }
        value += 1 << order;
        if value > 32767 {
            return Err(Error::InvalidData("coefficient_remaining"));
        }
    }
    Err(Error::InvalidData("coefficient_remaining"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    enum Bin {
        Decision(bool),
        Bypass(bool),
    }

    struct SyntaxBins {
        bins: std::vec::IntoIter<Bin>,
        contexts: Vec<Context>,
    }

    impl SyntaxBins {
        fn new(bins: Vec<Bin>) -> Self {
            Self {
                bins: bins.into_iter(),
                contexts: Vec::new(),
            }
        }

        fn finished(&self) -> bool {
            self.bins.len() == 0
        }
    }

    impl Bins for SyntaxBins {
        fn decision(&mut self, context: &mut Context) -> Result<bool> {
            self.contexts.push(*context);
            match self.bins.next() {
                Some(Bin::Decision(value)) => Ok(value),
                _ => Err(Error::InvalidData("test_decision_bin")),
            }
        }

        fn bypass(&mut self) -> Result<bool> {
            match self.bins.next() {
                Some(Bin::Bypass(value)) => Ok(value),
                _ => Err(Error::InvalidData("test_bypass_bin")),
            }
        }
    }

    fn block(log2_size: u8, component: Component, intra_mode: u8) -> Block {
        Block {
            log2_size,
            component,
            intra_mode: Some(intra_mode),
            transform_skip_enabled: false,
            transquant_bypass: false,
            sign_data_hiding_enabled: false,
        }
    }

    fn fixed(bins: &mut Vec<Bin>, value: u32, width: u8) {
        for bit in (0..width).rev() {
            bins.push(Bin::Bypass((value >> bit) & 1 != 0));
        }
    }

    fn remainder(bins: &mut Vec<Bin>, value: u32, rice: u8) {
        let maximum = 4 << rice;
        let quotient = (value >> rice).min(4);
        for _ in 0..quotient {
            bins.push(Bin::Bypass(true));
        }
        if value < maximum {
            bins.push(Bin::Bypass(false));
            fixed(bins, value & ((1 << rice) - 1), rice);
            return;
        }
        let mut value = value - maximum;
        for order in rice + 1..=16 {
            if value < 1 << order {
                bins.push(Bin::Bypass(false));
                fixed(bins, value, order);
                return;
            }
            bins.push(Bin::Bypass(true));
            value -= 1 << order;
        }
    }

    #[test]
    fn intra_initialization_tables_9_25_through_9_31() {
        assert_eq!(
            LAST_INIT,
            [
                110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123,
                63
            ]
        );
        assert_eq!(SUBBLOCK_INIT, [91, 171, 134, 141]);
        assert_eq!(
            SIGNIFICANCE_INIT.get(..16),
            Some(
                [
                    111, 111, 125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153, 125, 107
                ]
                .as_slice()
            )
        );
        assert_eq!(
            SIGNIFICANCE_INIT.get(16..32),
            Some(
                [
                    125, 141, 179, 153, 125, 107, 125, 141, 179, 153, 125, 140, 139, 182, 182, 152
                ]
                .as_slice()
            )
        );
        assert_eq!(
            SIGNIFICANCE_INIT.get(32..),
            Some([136, 152, 136, 153, 136, 139, 111, 136, 139, 111].as_slice())
        );
        assert_eq!(
            GREATER1_INIT,
            [
                140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140,
                179, 166, 182, 140, 227, 122, 197
            ]
        );
        assert_eq!(GREATER2_INIT, [138, 153, 136, 167, 152, 152]);
        for qp in [-12, 0, 26, 51, 63] {
            let contexts = Contexts::new(qp, Initialization::I);
            let copy = contexts;
            assert_eq!(contexts.transform_skip, [Context::new(139, qp); 2]);
            for ((x, y), init) in copy.last_x.iter().zip(copy.last_y.iter()).zip(LAST_INIT) {
                assert_eq!(*x, Context::new(init, qp));
                assert_eq!(x, y);
            }
            for (context, init) in copy.subblock.iter().zip(SUBBLOCK_INIT) {
                assert_eq!(*context, Context::new(init, qp));
            }
            for (context, init) in copy.significance.iter().zip(SIGNIFICANCE_INIT) {
                assert_eq!(*context, Context::new(init, qp));
            }
            for (context, init) in copy.greater1.iter().zip(GREATER1_INIT) {
                assert_eq!(*context, Context::new(init, qp));
            }
            for (context, init) in copy.greater2.iter().zip(GREATER2_INIT) {
                assert_eq!(*context, Context::new(init, qp));
            }
        }
    }

    #[test]
    fn p_initialization_tables_9_25_through_9_31() {
        for qp in (-12..=63).filter(|qp| *qp == -12 || (0..=51).contains(qp) || *qp == 63) {
            let contexts = Contexts::new(qp, Initialization::P);
            let last = [
                125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108,
            ]
            .map(|init| Context::new(init, qp));
            assert_eq!(contexts.transform_skip, [Context::new(139, qp); 2]);
            assert_eq!(contexts.last_x, last);
            assert_eq!(contexts.last_y, last);
            assert_eq!(
                contexts.subblock,
                [121, 140, 61, 154].map(|init| Context::new(init, qp))
            );
            assert_eq!(
                contexts.significance,
                [
                    155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183, 140, 136, 153, 154, 166,
                    183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 123, 123, 107,
                    121, 107, 121, 167, 151, 183, 140, 151, 183, 140,
                ]
                .map(|init| Context::new(init, qp))
            );
            assert_eq!(
                contexts.greater1,
                [
                    154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 137,
                    169, 194, 166, 167, 154, 167, 137, 182,
                ]
                .map(|init| Context::new(init, qp))
            );
            assert_eq!(
                contexts.greater2,
                [107, 167, 91, 122, 107, 167].map(|init| Context::new(init, qp))
            );
        }
    }

    #[test]
    fn b_initialization_tables_9_25_through_9_31() {
        for qp in (-12..=63).filter(|qp| *qp == -12 || (0..=51).contains(qp) || *qp == 63) {
            let contexts = Contexts::new(qp, Initialization::B);
            let last = [
                125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126, 111, 111, 79, 108, 123, 93,
            ]
            .map(|init| Context::new(init, qp));
            assert_eq!(contexts.transform_skip, [Context::new(139, qp); 2]);
            assert_eq!(contexts.last_x, last);
            assert_eq!(contexts.last_y, last);
            assert_eq!(
                contexts.subblock,
                [121, 140, 61, 154].map(|init| Context::new(init, qp))
            );
            assert_eq!(
                contexts.significance,
                [
                    170, 154, 139, 153, 139, 123, 123, 63, 124, 166, 183, 140, 136, 153, 154, 166,
                    183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 138, 138, 122,
                    121, 122, 121, 167, 151, 183, 140, 151, 183, 140,
                ]
                .map(|init| Context::new(init, qp))
            );
            assert_eq!(
                contexts.greater1,
                [
                    154, 196, 167, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 122,
                    169, 208, 166, 167, 154, 152, 167, 182,
                ]
                .map(|init| Context::new(init, qp))
            );
            assert_eq!(
                contexts.greater2,
                [107, 167, 91, 107, 107, 167].map(|init| Context::new(init, qp))
            );
        }
    }

    #[test]
    fn scan_coordinates_follow_6_11_through_6_13() -> Result<()> {
        let diagonal = [
            (0, 0),
            (0, 1),
            (1, 0),
            (0, 2),
            (1, 1),
            (2, 0),
            (0, 3),
            (1, 2),
            (2, 1),
            (3, 0),
            (1, 3),
            (2, 2),
            (3, 1),
            (2, 3),
            (3, 2),
            (3, 3),
        ];
        for (position, expected) in diagonal.into_iter().enumerate() {
            assert_eq!(scan_coordinate(4, Scan::Diagonal, position)?, expected);
        }
        for side in [1, 2, 4, 8] {
            for scan in [Scan::Diagonal, Scan::Horizontal, Scan::Vertical] {
                let mut visited = [false; 64];
                for position in 0..side * side {
                    let (x, y) = scan_coordinate(side, scan, position)?;
                    assert!(x < side && y < side);
                    assert_eq!(visited.get(y * side + x), Some(&false));
                    if let Some(entry) = visited.get_mut(y * side + x) {
                        *entry = true;
                    }
                    assert_eq!(scan_index(side, scan, (x, y))?, position);
                    match scan {
                        Scan::Horizontal => assert_eq!((x, y), (position % side, position / side)),
                        Scan::Vertical => assert_eq!((x, y), (position / side, position % side)),
                        Scan::Diagonal => {
                            if position > 0 {
                                let (px, py) = scan_coordinate(side, scan, position - 1)?;
                                assert!(x + y >= px + py);
                                if x + y == px + py {
                                    assert_eq!((x, y + 1), (px + 1, py));
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(scan_coordinate(0, Scan::Diagonal, 0).is_err());
        assert!(scan_coordinate(4, Scan::Diagonal, 16).is_err());
        Ok(())
    }

    #[test]
    fn intra_scan_selection_7_4_9_11() {
        for size in 2..=5 {
            for component in [Component::Luma, Component::Chroma] {
                for mode in 0..=34 {
                    let directional = size == 2 || (size == 3 && component == Component::Luma);
                    let expected = if directional && (6..=14).contains(&mode) {
                        Scan::Vertical
                    } else if directional && (22..=30).contains(&mode) {
                        Scan::Horizontal
                    } else {
                        Scan::Diagonal
                    };
                    assert_eq!(block(size, component, mode).scan(), expected);
                }
            }
        }
    }

    #[test]
    fn inter_scan_and_eight_by_eight_chroma_follow_7_4_9_11() -> Result<()> {
        for size in 2..=5 {
            for component in [Component::Luma, Component::Chroma] {
                let inter = Block {
                    intra_mode: None,
                    ..block(size, component, 0)
                };
                assert_eq!(inter.scan(), Scan::Diagonal);
            }
        }
        for (mode, scan) in [(10, Scan::Vertical), (26, Scan::Horizontal)] {
            assert_eq!(block(2, Component::Chroma, mode).scan(), scan);
            assert_eq!(block(3, Component::Luma, mode).scan(), scan);
            let chroma = block(3, Component::Chroma, mode);
            assert_eq!(chroma.scan(), Scan::Diagonal);
            assert_eq!(significance_context(chroma, 1, 0, 0)?, 37);
            let inter = Block {
                intra_mode: None,
                ..block(3, Component::Luma, mode)
            };
            assert_eq!(significance_context(inter, 1, 0, 0)?, 10);
            assert_eq!(
                significance_context(block(3, Component::Luma, mode), 1, 0, 0)?,
                16
            );
        }
        Ok(())
    }

    #[test]
    fn inter_residual_syntax_places_a_diagonal_scan_coefficient() -> Result<()> {
        for initialization in [Initialization::P, Initialization::B] {
            for size in 2..=5 {
                for component in [Component::Luma, Component::Chroma] {
                    let mut source = SyntaxBins::new(vec![
                        Bin::Decision(true),
                        Bin::Decision(false),
                        Bin::Decision(false),
                        Bin::Decision(false),
                        Bin::Decision(false),
                        Bin::Decision(false),
                        Bin::Bypass(false),
                    ]);
                    let mut output = [99; 1025];
                    let inter = Block {
                        intra_mode: None,
                        ..block(size, component, 0)
                    };
                    decode_block(
                        &mut source,
                        &mut Contexts::new(26, initialization),
                        inter,
                        &mut output,
                    )?;
                    assert!(source.finished());
                    let length = 1usize << (2 * size);
                    for (index, coefficient) in output.iter().take(length).enumerate() {
                        assert_eq!(*coefficient, i16::from(index == 1));
                    }
                    assert!(
                        output
                            .get(length..)
                            .is_some_and(|tail| tail.iter().all(|&value| value == 99))
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn last_context_indices_and_coordinate_ranges() -> Result<()> {
        for (size, luma, chroma) in [
            (2, vec![0, 1, 2], vec![15, 16, 17]),
            (3, vec![3, 3, 4, 4, 5], vec![15, 15, 16, 16, 17]),
            (
                4,
                vec![6, 6, 7, 7, 8, 8, 9],
                vec![15, 15, 15, 15, 16, 16, 16],
            ),
            (
                5,
                vec![10, 10, 11, 11, 12, 12, 13, 13, 14],
                vec![15, 15, 15, 15, 15, 15, 15, 15, 16],
            ),
        ] {
            for (component, indices) in [(Component::Luma, luma), (Component::Chroma, chroma)] {
                for (index, expected) in indices.into_iter().enumerate() {
                    assert_eq!(last_context(block(size, component, 0), index), expected);
                }
            }
        }
        for (prefix, minimum, maximum) in [
            (0, 0, 0),
            (1, 1, 1),
            (2, 2, 2),
            (3, 3, 3),
            (4, 4, 5),
            (5, 6, 7),
            (6, 8, 11),
            (7, 12, 15),
            (8, 16, 23),
            (9, 24, 31),
        ] {
            for expected in minimum..=maximum {
                let mut bins = Vec::new();
                if prefix > 3 {
                    fixed(&mut bins, expected - minimum, (prefix >> 1) - 1);
                }
                let mut source = SyntaxBins::new(bins);
                assert_eq!(last_coordinate(&mut source, prefix)?, expected as usize);
                assert!(source.finished());
            }
        }
        Ok(())
    }

    #[test]
    fn subblock_and_significance_indices_9_35_through_9_55() -> Result<()> {
        for component in [Component::Luma, Component::Chroma] {
            for neighbours in 0..=3 {
                assert_eq!(
                    subblock_context(component, neighbours),
                    component.chroma() * 2 + usize::from(neighbours != 0)
                );
                for size in 3..=5 {
                    for y in 0..1usize << size {
                        for x in 0..1usize << size {
                            let b = block(size, component, 0);
                            let context = significance_context(b, x, y, neighbours)?;
                            assert!(context < 42);
                            if x + y == 0 {
                                assert_eq!(context, component.chroma() * 27);
                            } else {
                                let xp = x % 4;
                                let yp = y % 4;
                                let local = match neighbours {
                                    0 if xp + yp == 0 => 2,
                                    0 if xp + yp < 3 => 1,
                                    0 => 0,
                                    1 if yp == 0 => 2,
                                    1 if yp == 1 => 1,
                                    1 => 0,
                                    2 if xp == 0 => 2,
                                    2 if xp == 1 => 1,
                                    2 => 0,
                                    _ => 2,
                                };
                                let offset = if component == Component::Chroma {
                                    if size == 3 { 36 } else { 39 }
                                } else {
                                    (if size == 3 { 9 } else { 21 })
                                        + if x >= 4 || y >= 4 { 3 } else { 0 }
                                };
                                assert_eq!(context, local + offset);
                            }
                        }
                    }
                }
            }
            for (index, expected) in SIGNIFICANCE_4X4.into_iter().enumerate() {
                assert_eq!(
                    significance_context(block(2, component, 0), index % 4, index / 4, 0)?,
                    component.chroma() * 27 + expected
                );
            }
        }
        let groups = [false; 64];
        assert_eq!(group_neighbours(&groups, 2, 0, 0)?, 0);
        let mut groups = groups;
        if let Some(value) = groups.get_mut(1) {
            *value = true;
        }
        assert_eq!(group_neighbours(&groups, 2, 0, 0)?, 1);
        if let Some(value) = groups.get_mut(2) {
            *value = true;
        }
        assert_eq!(group_neighbours(&groups, 2, 0, 0)?, 3);
        assert_eq!(group_neighbours(&groups, 2, 1, 1)?, 0);
        assert_eq!(
            significance_context(block(3, Component::Luma, 26), 1, 0, 0)?,
            16
        );
        assert_eq!(
            significance_context(block(3, Component::Luma, 10), 4, 0, 0)?,
            20
        );
        Ok(())
    }

    #[test]
    fn greater_context_sets_and_state_equations() {
        for component in [Component::Luma, Component::Chroma] {
            for group in 0..64 {
                for previous in 0..=3 {
                    let expected = if component == Component::Luma && group > 0 {
                        2
                    } else {
                        0
                    } + usize::from(previous == 0);
                    assert_eq!(greater_context_set(component, group, previous), expected);
                }
            }
        }
        assert_eq!(advance_greater_context(1, false), 2);
        assert_eq!(advance_greater_context(2, false), 3);
        assert_eq!(advance_greater_context(3, false), 3);
        for state in 0..=3 {
            assert_eq!(advance_greater_context(state, true), 0);
        }
        assert_eq!(advance_greater_context(0, false), 0);
    }

    #[test]
    fn rice_and_escape_codewords_9_11_through_9_28() -> Result<()> {
        for (rice, value, bits) in [
            (0, 0, "0"),
            (0, 1, "10"),
            (0, 2, "110"),
            (0, 3, "1110"),
            (0, 4, "111100"),
            (0, 5, "111101"),
            (0, 6, "11111000"),
            (1, 0, "00"),
            (1, 1, "01"),
            (1, 2, "100"),
            (1, 6, "11100"),
            (1, 7, "11101"),
            (1, 8, "1111000"),
        ] {
            let mut source =
                SyntaxBins::new(bits.bytes().map(|b| Bin::Bypass(b == b'1')).collect());
            assert_eq!(remaining_level(&mut source, rice)?, value);
            assert!(source.finished());
        }
        for rice in 0..=4 {
            for value in (0..256).chain([511, 1023, 4095, 16383, 32764, 32765, 32766, 32767]) {
                let mut bins = Vec::new();
                remainder(&mut bins, value, rice);
                let mut source = SyntaxBins::new(bins);
                assert_eq!(remaining_level(&mut source, rice)?, value);
                assert!(source.finished());
            }
            for value in [32768, 65535] {
                let mut bins = Vec::new();
                remainder(&mut bins, value, rice);
                assert_eq!(
                    remaining_level(&mut SyntaxBins::new(bins), rice),
                    Err(Error::InvalidData("coefficient_remaining"))
                );
            }
            let mut ones = SyntaxBins::new((0..32).map(|_| Bin::Bypass(true)).collect());
            assert_eq!(
                remaining_level(&mut ones, rice),
                Err(Error::InvalidData("coefficient_remaining"))
            );
            assert!(ones.bins.len() > 0);
        }
        assert_eq!(next_rice(0, 3), 0);
        assert_eq!(next_rice(0, 4), 1);
        assert_eq!(next_rice(1, 6), 1);
        assert_eq!(next_rice(1, 7), 2);
        assert_eq!(next_rice(3, 24), 3);
        assert_eq!(next_rice(3, 25), 4);
        assert_eq!(next_rice(4, 32768), 4);
        assert!(remaining_level(&mut SyntaxBins::new(Vec::new()), 0).is_err());
        Ok(())
    }

    #[test]
    fn signed_coefficient_bounds_include_negative_extremum() -> Result<()> {
        assert_eq!(signed_level(32768, true)?, i16::MIN);
        assert_eq!(signed_level(32767, false)?, i16::MAX);
        assert_eq!(signed_level(32767, true)?, -32767);
        assert!(signed_level(32768, false).is_err());
        assert!(signed_level(32769, true).is_err());
        assert!(signed_level(u32::MAX, true).is_err());
        Ok(())
    }

    #[test]
    fn syntax_dc_and_transform_skip_presence() -> Result<()> {
        for size in 2..=5 {
            for component in [Component::Luma, Component::Chroma] {
                for bypass in [false, true] {
                    let mut b = block(size, component, 0);
                    b.transform_skip_enabled = true;
                    b.transquant_bypass = bypass;
                    let mut bins = Vec::new();
                    let skip_present = size == 2 && !bypass;
                    if skip_present {
                        bins.push(Bin::Decision(true));
                    }
                    bins.extend([
                        Bin::Decision(false),
                        Bin::Decision(false),
                        Bin::Decision(false),
                        Bin::Bypass(false),
                    ]);
                    let mut source = SyntaxBins::new(bins);
                    let mut output = [99; 1025];
                    assert_eq!(
                        decode_block(
                            &mut source,
                            &mut Contexts::new(26, Initialization::I),
                            b,
                            &mut output
                        )?
                        .transform_skip,
                        skip_present
                    );
                    assert!(source.finished());
                    assert_eq!(output.first(), Some(&1));
                    let length = 1usize << (size * 2);
                    assert!(
                        output
                            .get(1..length)
                            .is_some_and(|tail| tail.iter().all(|&v| v == 0))
                    );
                    assert!(
                        output
                            .get(length..)
                            .is_some_and(|tail| tail.iter().all(|&v| v == 99))
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn syntax_prefixes_precede_suffixes_and_vertical_swaps_coordinates() -> Result<()> {
        for (mode, expected_position) in [(26, 2 * 8 + 5), (10, 5 * 8 + 2)] {
            let mut bins = Vec::new();
            bins.extend((0..4).map(|_| Bin::Decision(true)));
            bins.push(Bin::Decision(false));
            bins.extend([
                Bin::Decision(true),
                Bin::Decision(true),
                Bin::Decision(false),
                Bin::Bypass(true),
            ]);
            bins.extend((0..9).map(|_| Bin::Decision(false)));
            bins.extend([Bin::Decision(false), Bin::Bypass(true)]);
            bins.extend((0..16).map(|_| Bin::Decision(false)));
            let mut source = SyntaxBins::new(bins);
            let mut output = [0; 64];
            decode_block(
                &mut source,
                &mut Contexts::new(26, Initialization::I),
                block(3, Component::Luma, mode),
                &mut output,
            )?;
            assert!(source.finished());
            for (index, coefficient) in output.into_iter().enumerate() {
                assert_eq!(coefficient, if index == expected_position { -1 } else { 0 });
            }
        }
        Ok(())
    }

    #[test]
    fn syntax_infers_subblock_dc_or_skips_an_uncoded_group() -> Result<()> {
        for coded in [false, true] {
            let mut bins = Vec::new();
            bins.extend((0..4).map(|_| Bin::Decision(true)));
            bins.extend([
                Bin::Decision(false),
                Bin::Decision(false),
                Bin::Bypass(false),
                Bin::Decision(false),
                Bin::Bypass(false),
                Bin::Decision(coded),
            ]);
            if coded {
                bins.extend((0..15).map(|_| Bin::Decision(false)));
                bins.extend([Bin::Decision(false), Bin::Bypass(false)]);
            }
            bins.extend((0..16).map(|_| Bin::Decision(false)));
            let mut source = SyntaxBins::new(bins);
            let mut output = [0; 256];
            decode_block(
                &mut source,
                &mut Contexts::new(26, Initialization::I),
                block(4, Component::Luma, 0),
                &mut output,
            )?;
            assert!(source.finished());
            for (index, coefficient) in output.into_iter().enumerate() {
                let expected = i16::from(index == 4 || (coded && index == 4 * 16));
                assert_eq!(coefficient, expected);
            }
        }
        Ok(())
    }

    #[test]
    fn syntax_sign_hiding_span_parity_and_extremum() -> Result<()> {
        for (last, hiding, large_first) in [
            (3, true, false),
            (4, true, false),
            (4, false, false),
            (4, true, true),
        ] {
            let mut coefficients = [Coefficient::default(); 16];
            for position in [0, last] {
                if let Some(coefficient) = coefficients.get_mut(position) {
                    coefficient.significant = true;
                }
            }
            let mut bins = vec![
                Bin::Decision(!large_first),
                Bin::Decision(large_first),
                Bin::Decision(large_first),
                Bin::Bypass(false),
            ];
            if !hiding || last < 4 {
                bins.push(Bin::Bypass(false));
            }
            if large_first {
                remainder(&mut bins, 32765, 0);
            }
            let mut source = SyntaxBins::new(bins);
            let mut previous = LevelState::new(1);
            let levels = decode_levels(
                &mut source,
                &mut Contexts::new(26, Initialization::I),
                &mut coefficients,
                Component::Luma,
                0,
                hiding,
                &mut previous,
            )?;
            assert!(source.finished());
            assert_eq!(levels.get(last), Some(if large_first { &1 } else { &2 }));
            assert_eq!(
                levels.first(),
                Some(if large_first {
                    &i16::MIN
                } else if hiding && last >= 4 {
                    &-1
                } else {
                    &1
                })
            );
            assert_eq!(previous.greater1, 0);
            assert_eq!(previous.hidden_groups, u64::from(hiding && last >= 4));
        }
        Ok(())
    }

    #[test]
    fn syntax_even_sum_keeps_the_hidden_sign_positive() -> Result<()> {
        let mut coefficients = [Coefficient::default(); 16];
        for position in [0, 4] {
            if let Some(coefficient) = coefficients.get_mut(position) {
                coefficient.significant = true;
            }
        }
        let mut source = SyntaxBins::new(vec![
            Bin::Decision(false),
            Bin::Decision(false),
            Bin::Bypass(true),
        ]);
        let levels = decode_levels(
            &mut source,
            &mut Contexts::new(26, Initialization::I),
            &mut coefficients,
            Component::Luma,
            0,
            true,
            &mut LevelState::new(1),
        )?;
        assert_eq!(levels.first(), Some(&1));
        assert_eq!(levels.get(4), Some(&-1));
        assert!(source.finished());
        Ok(())
    }

    #[test]
    fn syntax_bypass_disables_sign_hiding_but_transform_skip_does_not() -> Result<()> {
        for bypass in [false, true] {
            let mut b = block(2, Component::Luma, 0);
            b.transform_skip_enabled = true;
            b.transquant_bypass = bypass;
            b.sign_data_hiding_enabled = true;
            let mut bins = Vec::new();
            if !bypass {
                bins.push(Bin::Decision(true));
            }
            bins.extend([
                Bin::Decision(true),
                Bin::Decision(false),
                Bin::Decision(true),
                Bin::Decision(false),
            ]);
            bins.extend((0..3).map(|_| Bin::Decision(false)));
            bins.extend([
                Bin::Decision(true),
                Bin::Decision(true),
                Bin::Decision(false),
                Bin::Decision(false),
                Bin::Bypass(false),
            ]);
            if bypass {
                bins.push(Bin::Bypass(false));
            }
            let mut source = SyntaxBins::new(bins);
            let mut output = [0; 16];
            let decoded = decode_block(
                &mut source,
                &mut Contexts::new(26, Initialization::I),
                b,
                &mut output,
            )?;
            assert_eq!(decoded.transform_skip, !bypass);
            assert_eq!(decoded.sign_hidden_groups, u64::from(!bypass));
            assert_eq!(output.first(), Some(if bypass { &1 } else { &-1 }));
            assert_eq!(output.get(5), Some(&2));
            assert!(source.finished());
        }
        Ok(())
    }

    #[test]
    fn syntax_positive_extremum_is_rejected_after_sign_decoding() {
        for negative in [false, true] {
            let mut coefficients = [Coefficient::default(); 16];
            if let Some(coefficient) = coefficients.first_mut() {
                coefficient.significant = true;
            }
            let mut bins = vec![
                Bin::Decision(true),
                Bin::Decision(true),
                Bin::Bypass(negative),
            ];
            remainder(&mut bins, 32765, 0);
            let mut source = SyntaxBins::new(bins);
            let result = decode_levels(
                &mut source,
                &mut Contexts::new(26, Initialization::I),
                &mut coefficients,
                Component::Luma,
                0,
                false,
                &mut LevelState::new(1),
            );
            if negative {
                assert!(result.is_ok_and(|levels| levels.first() == Some(&i16::MIN)));
            } else {
                assert_eq!(result, Err(Error::InvalidData("coefficient_level")));
            }
            assert!(source.finished());
        }
    }

    #[test]
    fn syntax_only_eight_greater1_flags_and_rice_reset() -> Result<()> {
        for component in [Component::Luma, Component::Chroma] {
            let mut coefficients = [Coefficient {
                significant: true,
                ..Coefficient::default()
            }; 16];
            let mut bins: Vec<Bin> = (0..8).map(|_| Bin::Decision(false)).collect();
            bins.extend((0..16).map(|_| Bin::Bypass(false)));
            for _ in 0..8 {
                remainder(&mut bins, 0, 0);
            }
            let mut source = SyntaxBins::new(bins);
            let mut previous = LevelState::new(0);
            let output = decode_levels(
                &mut source,
                &mut Contexts::new(26, Initialization::I),
                &mut coefficients,
                component,
                1,
                false,
                &mut previous,
            )?;
            assert_eq!(output, [1; 16]);
            assert_eq!(previous.greater1, 3);
            assert!(source.finished());
            let set = if component == Component::Luma { 3 } else { 1 };
            for (ordinal, context) in source.contexts.into_iter().enumerate() {
                let index = component.chroma() * 16 + set * 4 + (ordinal + 1).min(3);
                assert_eq!(
                    Some(context),
                    GREATER1_INIT.get(index).map(|&v| Context::new(v, 26))
                );
            }
        }
        let mut coefficients = [Coefficient::default(); 16];
        for coefficient in coefficients.iter_mut().take(4) {
            coefficient.significant = true;
        }
        let mut bins: Vec<Bin> = (0..5).map(|_| Bin::Decision(true)).collect();
        bins.extend((0..4).map(|_| Bin::Bypass(false)));
        for (rice, value) in [(0, 1), (1, 5), (2, 11), (3, 23)] {
            remainder(&mut bins, value, rice);
        }
        let mut source = SyntaxBins::new(bins);
        let output = decode_levels(
            &mut source,
            &mut Contexts::new(26, Initialization::I),
            &mut coefficients,
            Component::Luma,
            1,
            false,
            &mut LevelState::new(1),
        )?;
        assert_eq!(output.get(..4), Some([25, 13, 7, 4].as_slice()));
        assert!(source.finished());
        Ok(())
    }

    #[test]
    fn invalid_block_domains_and_output_fail_before_bins() {
        let mut source = SyntaxBins::new(Vec::new());
        let mut contexts = Contexts::new(26, Initialization::I);
        for size in [0, 1, 6, 255] {
            assert_eq!(
                decode_block(
                    &mut source,
                    &mut contexts,
                    block(size, Component::Luma, 0),
                    &mut [0; 16]
                ),
                Err(Error::InvalidData("residual_block"))
            );
        }
        assert_eq!(
            decode_block(
                &mut source,
                &mut contexts,
                block(2, Component::Luma, 35),
                &mut [0; 16]
            ),
            Err(Error::InvalidData("residual_block"))
        );
        assert_eq!(
            decode_block(
                &mut source,
                &mut contexts,
                block(2, Component::Luma, 0),
                &mut [0; 15]
            ),
            Err(Error::InvalidData("residual_output"))
        );
        assert!(source.contexts.is_empty());
    }

    #[test]
    fn invalid_intra_modes_preserve_output_contexts_and_bins() {
        for initialization in [Initialization::I, Initialization::P, Initialization::B] {
            for mode in 35..=u8::MAX {
                let mut contexts = Contexts::new(26, initialization);
                let before = contexts;
                let mut output = [99; 16];
                let mut source = SyntaxBins::new(vec![Bin::Decision(true)]);
                let mut invalid = block(2, Component::Luma, mode);
                invalid.transform_skip_enabled = true;
                assert_eq!(
                    decode_block(&mut source, &mut contexts, invalid, &mut output),
                    Err(Error::InvalidData("residual_block"))
                );
                assert_eq!(output, [99; 16]);
                assert_eq!(source.bins.len(), 1);
                assert!(source.contexts.is_empty());
                assert_eq!(contexts.transform_skip, before.transform_skip);
                assert_eq!(contexts.last_x, before.last_x);
                assert_eq!(contexts.last_y, before.last_y);
                assert_eq!(contexts.subblock, before.subblock);
                assert_eq!(contexts.significance, before.significance);
                assert_eq!(contexts.greater1, before.greater1);
                assert_eq!(contexts.greater2, before.greater2);
            }
        }
    }

    #[test]
    fn residual_uses_the_callers_arithmetic_bin_budget() -> Result<()> {
        let mut budget = 0;
        let mut cabac = Cabac::new(&[0; 8], &mut budget)?;
        let mut output = [99; 16];
        assert_eq!(
            decode(
                &mut cabac,
                &mut Contexts::new(26, Initialization::I),
                block(2, Component::Luma, 0),
                &mut output,
            ),
            Err(Error::Limit("cabac_bin_budget"))
        );
        assert_eq!(cabac.position(), 9);
        assert_eq!(output, [0; 16]);
        Ok(())
    }
}
