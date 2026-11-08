use crate::{BitDepth, Component, Error, Rect, RectMut, domain::check_rectangle};

static LUMA_FILTERS: [[i32; 8]; 3] = [
    [-1, 4, -10, 58, 17, -5, 1, 0],
    [-1, 4, -11, 40, 40, -11, 4, -1],
    [0, 1, -5, 17, 58, -10, 4, -1],
];
static CHROMA_FILTERS: [[i32; 4]; 7] = [
    [-2, 58, 10, -2],
    [-4, 54, 16, -2],
    [-6, 46, 28, -4],
    [-4, 36, 36, -4],
    [-4, 28, 46, -6],
    [-2, 16, 54, -4],
    [-2, 10, 58, -2],
];

/// A checked full coded reference component, without cropped or extended borders.
#[derive(Clone, Copy, Debug)]
pub struct RefPlane<'a> {
    data: &'a [u16],
    stride: usize,
    width: usize,
    height: usize,
}

impl<'a> RefPlane<'a> {
    /// Checks positive dimensions up to i32::MAX, stride and the last active sample.
    /// Sample depth is checked only over the interpolation support at invocation.
    pub fn new(data: &'a [u16], stride: usize, width: usize, height: usize) -> Result<Self, Error> {
        check_rectangle(data.len(), stride, width, height)?;
        Ok(Self {
            data,
            stride,
            width,
            height,
        })
    }
}

/// A resolved signed motion vector, without reference-list or candidate syntax.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionVector {
    /// Horizontal quarter-luma or eighth-chroma displacement.
    pub x: i16,
    /// Vertical quarter-luma or eighth-chroma displacement.
    pub y: i16,
}

/// Resolved Main/Main10 interpolation controls for one reference list and component.
#[derive(Clone, Copy, Debug)]
pub struct InterPrediction {
    /// Component sample depth.
    pub depth: BitDepth,
    /// Luma or 4:2:0 chroma interpolation rules.
    pub component: Component,
    /// Coded destination origin in full component samples, not cropped coordinates.
    pub x: usize,
    /// Coded destination origin in full component samples, not cropped coordinates.
    pub y: usize,
    /// Quarter-luma or eighth-chroma displacement; equal numerically for 4:2:0.
    pub motion: MotionVector,
}

fn coefficients(component: Component, phase: i16) -> Result<&'static [i32], Error> {
    let index = usize::try_from(phase - 1).map_err(|_| Error::Buffer)?;
    match component {
        Component::Luma => LUMA_FILTERS.get(index).map(|v| v.as_slice()),
        Component::Chroma => CHROMA_FILTERS.get(index).map(|v| v.as_slice()),
    }
    .ok_or(Error::Buffer)
}

pub(crate) struct InterScratch {
    columns: [usize; 71],
    source: [u16; 71],
    rows: [[i32; 64]; 8],
}

pub(crate) fn horizontal_row(
    source: &[u16],
    filter: &[i32],
    shift: u8,
    output: &mut [i32],
) -> Result<(), Error> {
    for (x, value) in output.iter_mut().enumerate() {
        let mut sum = 0;
        for (i, &coefficient) in filter.iter().enumerate() {
            sum += coefficient * i32::from(*source.get(x + i).ok_or(Error::Buffer)?);
        }
        *value = sum >> shift;
    }
    Ok(())
}

pub(crate) fn vertical_row(
    rows: &[&[i32]],
    filter: &[i32],
    output: &mut [i32],
) -> Result<(), Error> {
    for (x, value) in output.iter_mut().enumerate() {
        let mut sum = 0;
        for (&row, &coefficient) in rows.iter().zip(filter) {
            sum += coefficient * *row.get(x).ok_or(Error::Buffer)?;
        }
        *value = sum >> 6;
    }
    Ok(())
}

/// Interpolates H.265 8.5.3.3.3, equations 8-214–8-261 and Tables 8-8/8-9.
///
/// The output is the reusable signed, uncentred normative internal predictor:
/// integer samples are scaled by 2^(14-depth), with no rounding or clipping.
/// i32 is necessary: two-axis luma interpolation can exceed i16 at both depths.
/// The maximum absolute coefficient sum is 112, so even the conservative
/// two-axis bound 112*112*1023/256 fits i32. Fixed scratch caches eight rows.
/// Luma rectangles are at most 64x64 and chroma rectangles at most 32x32.
/// Origins must fit i32. Each tap replicates the full coded picture edge.
/// All samples in the clipped rectangular filter support are depth-checked
/// before any write. Output padding and outputs on rejected inputs are unchanged.
pub fn interpolate_inter(
    config: InterPrediction,
    reference: RefPlane<'_>,
    output: RectMut<'_, i32>,
) -> Result<(), Error> {
    interpolate(config, reference, output, true)
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn interpolate_inter_scalar(
    config: InterPrediction,
    reference: RefPlane<'_>,
    output: RectMut<'_, i32>,
) -> Result<(), Error> {
    interpolate(config, reference, output, false)
}

fn interpolate(
    config: InterPrediction,
    reference: RefPlane<'_>,
    output: RectMut<'_, i32>,
    _automatic: bool,
) -> Result<(), Error> {
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    let vector = _automatic.then(crate::simd::Avx2::detect).flatten();
    if config.x > i32::MAX as usize || config.y > i32::MAX as usize {
        return Err(Error::Buffer);
    }
    let (fraction_bits, start, taps, limit) = match config.component {
        Component::Luma => (2, -3_i64, 8_i64, 64),
        Component::Chroma => (3, -1_i64, 4_i64, 32),
    };
    if output.width > limit || output.height > limit {
        return Err(Error::Rectangle);
    }
    let mask = (1 << fraction_bits) - 1;
    let phase_x = config.motion.x & mask;
    let phase_y = config.motion.y & mask;
    let x_base = config.x as i64 + i64::from(config.motion.x >> fraction_bits);
    let y_base = config.y as i64 + i64::from(config.motion.y >> fraction_bits);
    let x_start = x_base + if phase_x == 0 { 0 } else { start };
    let y_start = y_base + if phase_y == 0 { 0 } else { start };
    let x_end = x_base + output.width as i64 - 1 + if phase_x == 0 { 0 } else { start + taps - 1 };
    let y_end = y_base + output.height as i64 - 1 + if phase_y == 0 { 0 } else { start + taps - 1 };
    for y in
        y_start.clamp(0, reference.height as i64 - 1)..=y_end.clamp(0, reference.height as i64 - 1)
    {
        let begin = y as usize * reference.stride;
        let left = x_start.clamp(0, reference.width as i64 - 1) as usize;
        let right = x_end.clamp(0, reference.width as i64 - 1) as usize;
        let row = reference
            .data
            .get(begin + left..=begin + right)
            .ok_or(Error::Buffer)?;
        if row.iter().any(|&sample| sample > config.depth.max_sample()) {
            return Err(Error::Sample);
        }
    }
    let first_shift = config.depth.bits() - 8;
    let internal_shift = 14 - config.depth.bits();
    let x_filter = if phase_x == 0 {
        &[][..]
    } else {
        coefficients(config.component, phase_x)?
    };
    let y_filter = if phase_y == 0 {
        &[][..]
    } else {
        coefficients(config.component, phase_y)?
    };
    let mut scratch = InterScratch {
        columns: [0; 71],
        source: [0; 71],
        rows: [[0; 64]; 8],
    };
    let source_width = output.width + x_filter.len().saturating_sub(1);
    let consecutive = x_start >= 0 && x_start + source_width as i64 <= reference.width as i64;
    if !consecutive {
        for (x, column) in scratch.columns.iter_mut().take(source_width).enumerate() {
            *column = (x_start + x as i64).clamp(0, reference.width as i64 - 1) as usize;
        }
    }
    let row_count = y_filter.len().max(1);
    let mut next_row = 0;
    for y in 0..output.height {
        while next_row < y + row_count {
            let source_y =
                (y_start + next_row as i64).clamp(0, reference.height as i64 - 1) as usize;
            let begin = source_y * reference.stride;
            let source = reference
                .data
                .get(begin..begin + reference.width)
                .ok_or(Error::Buffer)?;
            let input = if consecutive {
                source
                    .get(x_start as usize..x_start as usize + source_width)
                    .ok_or(Error::Buffer)?
            } else {
                let input = scratch
                    .source
                    .get_mut(..source_width)
                    .ok_or(Error::Buffer)?;
                for (value, &column) in input.iter_mut().zip(&scratch.columns) {
                    *value = *source.get(column).ok_or(Error::Buffer)?;
                }
                &*input
            };
            let row = scratch.rows.get_mut(next_row % 8).ok_or(Error::Buffer)?;
            let row = row.get_mut(..output.width).ok_or(Error::Buffer)?;
            if x_filter.is_empty() {
                for (value, &sample) in row.iter_mut().zip(input.iter()) {
                    *value = i32::from(sample) << internal_shift;
                }
            } else {
                #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
                if let Some(vector) = vector {
                    vector.horizontal(input, x_filter, first_shift, row)?;
                } else {
                    horizontal_row(input, x_filter, first_shift, row)?;
                }
                #[cfg(any(feature = "scalar", not(target_arch = "x86_64")))]
                horizontal_row(input, x_filter, first_shift, row)?;
            }
            next_row += 1;
        }
        let begin = y * output.stride;
        let destination = output
            .data
            .get_mut(begin..begin + output.width)
            .ok_or(Error::Buffer)?;
        if y_filter.is_empty() {
            destination.copy_from_slice(
                scratch
                    .rows
                    .get(y % 8)
                    .and_then(|row| row.get(..output.width))
                    .ok_or(Error::Buffer)?,
            );
        } else {
            let mut rows: [&[i32]; 8] = [&[]; 8];
            for (j, row) in rows.iter_mut().take(row_count).enumerate() {
                *row = scratch.rows.get((y + j) % 8).ok_or(Error::Buffer)?;
            }
            #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
            if let Some(vector) = vector {
                vector.vertical(&rows, y_filter, destination)?;
            } else {
                vertical_row(&rows, y_filter, destination)?;
            }
            #[cfg(any(feature = "scalar", not(target_arch = "x86_64")))]
            vertical_row(&rows, y_filter, destination)?;
        }
    }
    Ok(())
}

/// An effective component weight and unscaled eight-bit-domain offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PredictionWeight {
    denom: u8,
    weight: i16,
    offset: i16,
}

impl PredictionWeight {
    /// Checks denom in 0..=7, weight−2^denom in −128..=127 and offset in −128..=127.
    /// The caller resolves luma/chroma syntax and defaults under H.265 7.4.7.3;
    /// offsets here precede the depth−8 shift of equations 8-268/8-273.
    pub fn new(denom: u8, weight: i16, offset: i16) -> Result<Self, Error> {
        if denom > 7 || !(-128..=127).contains(&offset) {
            return Err(Error::Weight);
        }
        if !(-128..=127).contains(&(i32::from(weight) - (1 << denom))) {
            return Err(Error::Weight);
        }
        Ok(Self {
            denom,
            weight,
            offset,
        })
    }

    /// Returns the effective component log2 weight denominator.
    pub const fn denom(self) -> u8 {
        self.denom
    }

    /// Returns the effective component weight, including its default base.
    pub const fn weight(self) -> i16 {
        self.weight
    }

    /// Returns the offset before depth scaling.
    pub const fn offset(self) -> i16 {
        self.offset
    }
}

const DEFAULT_WEIGHT: PredictionWeight = PredictionWeight {
    denom: 0,
    weight: 1,
    offset: 0,
};

/// Rounds and clips one internal predictor using H.265 equation 8-262 or 8-263.
/// Active predictors must be in −16830..=33150 at eight bits or
/// −16880..=33247 at ten bits, the enclosing range of both component filters.
/// Dimensions must match; rejected inputs leave output unchanged.
pub fn predict_uni(
    depth: BitDepth,
    predictor: Rect<'_, i32>,
    output: RectMut<'_, u16>,
) -> Result<(), Error> {
    weight_uni(depth, DEFAULT_WEIGHT, predictor, output)
}

/// Averages, rounds and clips two internal predictors using H.265 equation 8-264.
/// Predictors must satisfy the same internal range as `predict_uni`.
/// Dimensions must match; rejected inputs leave output unchanged.
pub fn predict_bi(
    depth: BitDepth,
    first: Rect<'_, i32>,
    second: Rect<'_, i32>,
    output: RectMut<'_, u16>,
) -> Result<(), Error> {
    weight_bi(depth, DEFAULT_WEIGHT, DEFAULT_WEIGHT, first, second, output)
}

/// Weights, rounds and clips one internal predictor using H.265 equation 8-275/8-276.
/// Predictors must satisfy the same internal range as `predict_uni`.
/// Dimensions must match; rejected inputs leave output unchanged.
pub fn weight_uni(
    depth: BitDepth,
    weight: PredictionWeight,
    predictor: Rect<'_, i32>,
    output: RectMut<'_, u16>,
) -> Result<(), Error> {
    combine(depth, weight, predictor, None, output)
}

/// Weights and clips two internal predictors using H.265 equation 8-277.
/// Denominators and dimensions must match before any output is written.
/// Offsets scale by depth−8; signed offset averaging occurs in the final shift.
/// Predictors must satisfy the same internal range as `predict_uni`.
pub fn weight_bi(
    depth: BitDepth,
    first_weight: PredictionWeight,
    second_weight: PredictionWeight,
    first: Rect<'_, i32>,
    second: Rect<'_, i32>,
    output: RectMut<'_, u16>,
) -> Result<(), Error> {
    combine(
        depth,
        first_weight,
        first,
        Some((second_weight, second)),
        output,
    )
}

fn combine(
    depth: BitDepth,
    weight: PredictionWeight,
    first: Rect<'_, i32>,
    second: Option<(PredictionWeight, Rect<'_, i32>)>,
    output: RectMut<'_, u16>,
) -> Result<(), Error> {
    if first.width != output.width || first.height != output.height {
        return Err(Error::Rectangle);
    }
    if let Some((other_weight, other)) = second {
        if other.width != output.width || other.height != output.height {
            return Err(Error::Rectangle);
        }
        if other_weight.denom != weight.denom {
            return Err(Error::Weight);
        }
    }
    let max_sample = i32::from(depth.max_sample());
    let lower = (-24 * max_sample) >> (depth.bits() - 8);
    let upper = (88 * max_sample) >> (depth.bits() - 8);
    let minimum = (88 * lower - 24 * upper) >> 6;
    let maximum = (88 * upper - 24 * lower) >> 6;
    for row in first.data.chunks(first.stride).take(first.height) {
        if row
            .get(..first.width)
            .ok_or(Error::Buffer)?
            .iter()
            .any(|value| !(minimum..=maximum).contains(value))
        {
            return Err(Error::Sample);
        }
    }
    if let Some((_, other)) = second {
        for row in other.data.chunks(other.stride).take(other.height) {
            if row
                .get(..other.width)
                .ok_or(Error::Buffer)?
                .iter()
                .any(|value| !(minimum..=maximum).contains(value))
            {
                return Err(Error::Sample);
            }
        }
    }
    let width = output.width;
    let height = output.height;
    let shift = weight.denom + 14 - depth.bits();
    let offset = i32::from(weight.offset) << (depth.bits() - 8);
    let first_weight = i32::from(weight.weight);
    if let Some((other_weight, other)) = second {
        let second_weight = i32::from(other_weight.weight);
        let other_offset = i32::from(other_weight.offset) << (depth.bits() - 8);
        let bias = (offset + other_offset + 1) << shift;
        for ((first, other), output) in first
            .data
            .chunks(first.stride)
            .zip(other.data.chunks(other.stride))
            .zip(output.data.chunks_mut(output.stride))
            .take(height)
        {
            for ((&first, &other), output) in first
                .get(..width)
                .ok_or(Error::Buffer)?
                .iter()
                .zip(other.get(..width).ok_or(Error::Buffer)?)
                .zip(output.get_mut(..width).ok_or(Error::Buffer)?)
            {
                let sample = (first * first_weight + other * second_weight + bias) >> (shift + 1);
                *output = sample.clamp(0, max_sample) as u16;
            }
        }
    } else {
        let rounding = 1 << (shift - 1);
        for (first, output) in first
            .data
            .chunks(first.stride)
            .zip(output.data.chunks_mut(output.stride))
            .take(height)
        {
            for (&first, output) in first
                .get(..width)
                .ok_or(Error::Buffer)?
                .iter()
                .zip(output.get_mut(..width).ok_or(Error::Buffer)?)
            {
                let sample = ((first * first_weight + rounding) >> shift) + offset;
                *output = sample.clamp(0, max_sample) as u16;
            }
        }
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn combine_reference(
    depth: BitDepth,
    weight: PredictionWeight,
    first: Rect<'_, i32>,
    second: Option<(PredictionWeight, Rect<'_, i32>)>,
    mut output: RectMut<'_, u16>,
) -> Result<(), Error> {
    if first.width != output.width || first.height != output.height {
        return Err(Error::Rectangle);
    }
    if let Some((other_weight, other)) = second {
        if other.width != output.width || other.height != output.height {
            return Err(Error::Rectangle);
        }
        if other_weight.denom != weight.denom {
            return Err(Error::Weight);
        }
    }
    let max_sample = i32::from(depth.max_sample());
    let lower = (-24 * max_sample) >> (depth.bits() - 8);
    let upper = (88 * max_sample) >> (depth.bits() - 8);
    let minimum = (88 * lower - 24 * upper) >> 6;
    let maximum = (88 * upper - 24 * lower) >> 6;
    for y in 0..first.height {
        for x in 0..first.width {
            if !(minimum..=maximum).contains(&first.at(x, y)?) {
                return Err(Error::Sample);
            }
            if let Some((_, other)) = second {
                if !(minimum..=maximum).contains(&other.at(x, y)?) {
                    return Err(Error::Sample);
                }
            }
        }
    }
    let shift = weight.denom + 14 - depth.bits();
    let offset = i64::from(weight.offset) << (depth.bits() - 8);
    for y in 0..output.height {
        for x in 0..output.width {
            let value = i64::from(first.at(x, y)?) * i64::from(weight.weight);
            let sample = if let Some((other_weight, other)) = second {
                let other_value = i64::from(other.at(x, y)?) * i64::from(other_weight.weight);
                let other_offset = i64::from(other_weight.offset) << (depth.bits() - 8);
                (value + other_value + ((offset + other_offset + 1) << shift)) >> (shift + 1)
            } else {
                ((value + (1_i64 << (shift - 1))) >> shift) + offset
            };
            output.put(x, y, depth.clip(sample))?;
        }
    }
    Ok(())
}
