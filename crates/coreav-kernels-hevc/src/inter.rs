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
    #[cfg(hevc_thread_diagnostics)]
    read_observer: Option<fn(usize, usize)>,
}

impl<'a> RefPlane<'a> {
    /// Checks positive dimensions up to i32::MAX, stride and the last active sample.
    /// Sample depth is checked only over the interpolation support at invocation.
    pub fn new(data: &'a [u16], stride: usize, width: usize, height: usize) -> Result<Self, Error> {
        Self::prefix(data, stride, width, height, height)
    }

    #[doc(hidden)]
    pub fn prefix(
        data: &'a [u16],
        stride: usize,
        width: usize,
        height: usize,
        rows: usize,
    ) -> Result<Self, Error> {
        if height > i32::MAX as usize {
            return Err(Error::Buffer);
        }
        if rows > height {
            return Err(Error::Rectangle);
        }
        check_rectangle(data.len(), stride, width, rows)?;
        Ok(Self {
            data,
            stride,
            width,
            height,
            #[cfg(hevc_thread_diagnostics)]
            read_observer: None,
        })
    }

    #[cfg(hevc_thread_diagnostics)]
    #[doc(hidden)]
    pub fn diagnostic_reads(mut self, observer: fn(usize, usize)) -> Self {
        self.read_observer = Some(observer);
        self
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

#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterSupport {
    start: [i64; 2],
    phase: [i16; 2],
    first: [usize; 2],
    last: [usize; 2],
}

impl InterSupport {
    pub fn rows(self) -> core::ops::RangeInclusive<usize> {
        self.first[1]..=self.last[1]
    }
}

impl InterPrediction {
    #[doc(hidden)]
    pub fn support(
        self,
        reference_width: usize,
        reference_height: usize,
        width: usize,
        height: usize,
    ) -> Result<InterSupport, Error> {
        if self.x > i32::MAX as usize
            || self.y > i32::MAX as usize
            || reference_width == 0
            || reference_height == 0
            || reference_width > i32::MAX as usize
            || reference_height > i32::MAX as usize
        {
            return Err(Error::Buffer);
        }
        let (bits, first_tap, taps, limit) = match self.component {
            Component::Luma => (2, -3_i64, 8_i64, 64),
            Component::Chroma => (3, -1_i64, 4_i64, 32),
        };
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(Error::Rectangle);
        }
        let mask = (1 << bits) - 1;
        let axes = [
            (self.x, self.motion.x, width, reference_width),
            (self.y, self.motion.y, height, reference_height),
        ]
        .map(|(origin, motion, output, reference)| {
            let phase = motion & mask;
            let base = origin as i64 + i64::from(motion >> bits);
            let start = base + if phase == 0 { 0 } else { first_tap };
            let end = base + output as i64 - 1 + if phase == 0 { 0 } else { first_tap + taps - 1 };
            (
                start,
                phase,
                start.clamp(0, reference as i64 - 1) as usize,
                end.clamp(0, reference as i64 - 1) as usize,
            )
        });
        let [(xs, xp, xf, xl), (ys, yp, yf, yl)] = axes;
        Ok(InterSupport {
            start: [xs, ys],
            phase: [xp, yp],
            first: [xf, yf],
            last: [xl, yl],
        })
    }
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
    interpolate(config, reference, output, true, None)
}

/// Uses the first 512 caller-owned i32 samples as an eight-row interpolation ring.
///
/// The samples may contain arbitrary previous values. Every active temporary is
/// overwritten before use; workspace beyond 512 samples and output padding stay unchanged.
pub fn interpolate_inter_with_rows(
    config: InterPrediction,
    reference: RefPlane<'_>,
    output: RectMut<'_, i32>,
    rows: &mut [i32],
) -> Result<(), Error> {
    let rows = rows.get_mut(..512).ok_or(Error::Buffer)?;
    interpolate(config, reference, output, true, Some(rows))
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn interpolate_inter_scalar(
    config: InterPrediction,
    reference: RefPlane<'_>,
    output: RectMut<'_, i32>,
) -> Result<(), Error> {
    interpolate(config, reference, output, false, None)
}

#[cfg(test)]
mod support_tests {
    use super::*;

    #[test]
    fn clipped_support_rows_are_exact_at_both_depths_and_picture_edges() -> Result<(), Error> {
        for bits in [8, 10] {
            for (component, automatic) in [
                (Component::Luma, false),
                (Component::Luma, true),
                (Component::Chroma, false),
                (Component::Chroma, true),
            ] {
                for y in [0, 8, 31] {
                    for motion in [
                        i16::MIN,
                        -9,
                        -8,
                        -7,
                        -4,
                        -3,
                        -1,
                        0,
                        1,
                        3,
                        4,
                        7,
                        8,
                        9,
                        i16::MAX,
                    ] {
                        let config = InterPrediction {
                            depth: BitDepth::new(bits)?,
                            component,
                            x: 8,
                            y,
                            motion: MotionVector {
                                x: motion,
                                y: motion,
                            },
                        };
                        let rows = config.support(32, 32, 8, 4)?.rows();
                        let source = [42u16; 32 * 32];
                        let mut expected = [0; 8 * 4];
                        interpolate(
                            config,
                            RefPlane::new(&source, 32, 32, 32)?,
                            RectMut::new(&mut expected, 8, 8, 4)?,
                            automatic,
                            None,
                        )?;
                        for row in 0..32 {
                            let mut source = [42u16; 32 * 32];
                            source[row * 32..(row + 1) * 32].fill(config.depth.max_sample() + 1);
                            let mut output = [7; 8 * 4];
                            let result = interpolate(
                                config,
                                RefPlane::new(&source, 32, 32, 32)?,
                                RectMut::new(&mut output, 8, 8, 4)?,
                                automatic,
                                None,
                            );
                            if rows.contains(&row) {
                                assert_eq!(result, Err(Error::Sample));
                                assert_eq!(output, [7; 8 * 4]);
                            } else {
                                result?;
                                assert_eq!(output, expected);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn support_retains_fractional_taps_even_when_an_endpoint_coefficient_is_zero()
    -> Result<(), Error> {
        for (component, motion, first, last) in [
            (Component::Luma, 0, 8, 11),
            (Component::Luma, 1, 5, 15),
            (Component::Luma, 2, 5, 15),
            (Component::Luma, 3, 5, 15),
            (Component::Luma, -1, 4, 14),
            (Component::Luma, -4, 7, 10),
            (Component::Chroma, 1, 7, 13),
            (Component::Chroma, 7, 7, 13),
            (Component::Chroma, -1, 6, 12),
            (Component::Luma, i16::MIN, 0, 0),
            (Component::Luma, i16::MAX, 31, 31),
        ] {
            let config = InterPrediction {
                depth: BitDepth::new(8)?,
                component,
                x: 8,
                y: 8,
                motion: MotionVector { x: 0, y: motion },
            };
            assert_eq!(config.support(32, 32, 8, 4)?.rows(), first..=last);
        }
        Ok(())
    }

    #[test]
    fn prefix_references_keep_full_picture_clipping_and_refuse_unpublished_rows()
    -> Result<(), Error> {
        let source = [42u16; 32 * 32];
        for component in [Component::Luma, Component::Chroma] {
            for motion in [i16::MIN, -1, 0, 1, i16::MAX] {
                for y in [0, 8, 31] {
                    let config = InterPrediction {
                        depth: BitDepth::new(8)?,
                        component,
                        x: 8,
                        y,
                        motion: MotionVector { x: 0, y: motion },
                    };
                    let rows = config.support(32, 32, 8, 4)?.rows();
                    let available = *rows.end() + 1;
                    let mut expected = [0; 32];
                    interpolate(
                        config,
                        RefPlane::new(&source, 32, 32, 32)?,
                        RectMut::new(&mut expected, 8, 8, 4)?,
                        false,
                        None,
                    )?;
                    for borrowed in [false, true] {
                        let mut row_storage = [i32::MIN; 512];
                        let mut actual = [0; 32];
                        interpolate(
                            config,
                            RefPlane::prefix(&source[..available * 32], 32, 32, 32, available)?,
                            RectMut::new(&mut actual, 8, 8, 4)?,
                            false,
                            borrowed.then_some(row_storage.as_mut_slice()),
                        )?;
                        assert_eq!(actual, expected);
                        if available > 1 {
                            let mut actual = [99; 32];
                            let before = row_storage;
                            assert_eq!(
                                interpolate(
                                    config,
                                    RefPlane::prefix(
                                        &source[..(available - 1) * 32],
                                        32,
                                        32,
                                        32,
                                        available - 1
                                    )?,
                                    RectMut::new(&mut actual, 8, 8, 4)?,
                                    false,
                                    borrowed.then_some(row_storage.as_mut_slice()),
                                ),
                                Err(Error::Buffer)
                            );
                            assert_eq!(actual, [99; 32]);
                            assert_eq!(row_storage, before);
                        }
                    }
                }
            }
        }
        assert!(RefPlane::prefix(&source, 32, 32, 32, 0).is_err());
        assert!(RefPlane::prefix(&source, 32, 32, 32, 33).is_err());
        Ok(())
    }

    #[test]
    fn support_refuses_invalid_geometry_without_a_reference_buffer() -> Result<(), Error> {
        let config = InterPrediction {
            depth: BitDepth::new(8)?,
            component: Component::Luma,
            x: 0,
            y: 0,
            motion: MotionVector { x: 0, y: 0 },
        };
        for (rw, rh, width, height, error) in [
            (0, 32, 8, 4, Error::Buffer),
            (32, 0, 8, 4, Error::Buffer),
            (usize::MAX, 32, 8, 4, Error::Buffer),
            (32, usize::MAX, 8, 4, Error::Buffer),
            (32, 32, 0, 4, Error::Rectangle),
            (32, 32, 8, 0, Error::Rectangle),
            (32, 32, 65, 4, Error::Rectangle),
            (32, 32, 8, 65, Error::Rectangle),
        ] {
            assert_eq!(config.support(rw, rh, width, height), Err(error));
        }
        assert_eq!(
            InterPrediction {
                x: usize::MAX,
                ..config
            }
            .support(32, 32, 8, 4),
            Err(Error::Buffer)
        );
        assert_eq!(
            InterPrediction {
                component: Component::Chroma,
                ..config
            }
            .support(32, 32, 33, 4),
            Err(Error::Rectangle)
        );
        Ok(())
    }
}

#[cfg(all(test, hevc_thread_diagnostics))]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};

    static READS: AtomicUsize = AtomicUsize::new(usize::MAX);

    fn observed(first: usize, last: usize) {
        READS.store(first * 64 + last, Ordering::Relaxed);
    }

    #[test]
    fn diagnostic_read_bounds_match_interpolation_support() -> Result<(), Error> {
        for (component, mv_x, mv_y, expected) in [
            (Component::Luma, 0, 0, (8, 11)),
            (Component::Luma, 1, 0, (8, 11)),
            (Component::Luma, 0, 1, (5, 15)),
            (Component::Luma, 0, -1, (4, 14)),
            (Component::Luma, 0, -4, (7, 10)),
            (Component::Chroma, 0, 1, (7, 13)),
            (Component::Chroma, 0, -1, (6, 12)),
            (Component::Luma, 0, i16::MIN, (0, 0)),
            (Component::Luma, 0, i16::MAX, (31, 31)),
        ] {
            for automatic in [false, true] {
                let mut source = [42u16; 32 * 32];
                let mut output = [0i32; 8 * 4];
                let config = InterPrediction {
                    depth: BitDepth::new(8)?,
                    component,
                    x: 8,
                    y: 8,
                    motion: MotionVector { x: mv_x, y: mv_y },
                };
                READS.store(usize::MAX, Ordering::Relaxed);
                interpolate(
                    config,
                    RefPlane::new(&source, 32, 32, 32)?.diagnostic_reads(observed),
                    RectMut::new(&mut output, 8, 8, 4)?,
                    automatic,
                    None,
                )?;
                assert_eq!(READS.load(Ordering::Relaxed), expected.0 * 64 + expected.1);
                *source.get_mut(expected.1 * 32 + 8).ok_or(Error::Buffer)? = 256;
                let before = output;
                assert_eq!(
                    interpolate(
                        config,
                        RefPlane::new(&source, 32, 32, 32)?.diagnostic_reads(observed),
                        RectMut::new(&mut output, 8, 8, 4)?,
                        automatic,
                        None,
                    ),
                    Err(Error::Sample)
                );
                assert_eq!(output, before);
            }
        }
        Ok(())
    }
}

fn interpolate(
    config: InterPrediction,
    reference: RefPlane<'_>,
    output: RectMut<'_, i32>,
    _automatic: bool,
    row_storage: Option<&mut [i32]>,
) -> Result<(), Error> {
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    let vector = _automatic.then(crate::simd::Avx2::detect).flatten();
    let support = config.support(
        reference.width,
        reference.height,
        output.width,
        output.height,
    )?;
    let [phase_x, phase_y] = support.phase;
    let [x_start, y_start] = support.start;
    #[cfg(hevc_thread_diagnostics)]
    if let Some(observer) = reference.read_observer {
        observer(*support.rows().start(), *support.rows().end());
    }
    for y in support.rows() {
        let begin = y * reference.stride;
        let left = support.first[0];
        let right = support.last[0];
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
    let mut owned;
    let mut columns;
    let mut source;
    let (columns, source_buffer, intermediate) = if let Some(rows) = row_storage {
        columns = [0; 71];
        source = [0; 71];
        (&mut columns, &mut source, rows)
    } else {
        owned = InterScratch {
            columns: [0; 71],
            source: [0; 71],
            rows: [[0; 64]; 8],
        };
        (
            &mut owned.columns,
            &mut owned.source,
            owned.rows.as_flattened_mut(),
        )
    };
    let source_width = output.width + x_filter.len().saturating_sub(1);
    let consecutive = x_start >= 0 && x_start + source_width as i64 <= reference.width as i64;
    if !consecutive {
        for (x, column) in columns.iter_mut().take(source_width).enumerate() {
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
                let input = source_buffer.get_mut(..source_width).ok_or(Error::Buffer)?;
                for (value, &column) in input.iter_mut().zip(columns.iter()) {
                    *value = *source.get(column).ok_or(Error::Buffer)?;
                }
                &*input
            };
            let row_start = (next_row % 8) * 64;
            let row = intermediate
                .get_mut(row_start..row_start + output.width)
                .ok_or(Error::Buffer)?;
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
                intermediate
                    .get((y % 8) * 64..(y % 8) * 64 + output.width)
                    .ok_or(Error::Buffer)?,
            );
        } else {
            let mut rows: [&[i32]; 8] = [&[]; 8];
            for (j, row) in rows.iter_mut().take(row_count).enumerate() {
                let begin = ((y + j) % 8) * 64;
                *row = intermediate.get(begin..begin + 64).ok_or(Error::Buffer)?;
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
    let scalar_validation = || {
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
        Ok(())
    };
    #[cfg(all(not(feature = "scalar"), target_arch = "x86_64"))]
    {
        let vector = if first.width >= 8 {
            super::simd::Avx2::detect()
        } else {
            None
        };
        if let Some(vector) = vector {
            vector.validate_combine(first, second.map(|(_, row)| row), minimum, maximum)?;
        } else {
            scalar_validation()?;
        }
    }
    #[cfg(any(feature = "scalar", not(target_arch = "x86_64")))]
    scalar_validation()?;
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
