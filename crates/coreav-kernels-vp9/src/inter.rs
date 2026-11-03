//! Sub-sample interpolation and compound prediction (clause 8.5.2.4).

use crate::{BitDepth, Pixel, round2, tables::SUBPEL_FILTERS};

/// The interpolation filter, in the order of `subpel_filters` (8.5.2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpFilter {
    /// The regular eight-tap filter.
    EightTap,
    /// The smooth eight-tap filter.
    EightTapSmooth,
    /// The sharp eight-tap filter.
    EightTapSharp,
    /// Linear interpolation between two samples.
    Bilinear,
}

/// A nonempty reference plane whose visible samples fit in its storage.
#[derive(Clone, Copy, Debug)]
pub struct ReferencePlane<'a, P> {
    samples: &'a [P],
    stride: usize,
    width: usize,
    height: usize,
}

impl<'a, P: Pixel> ReferencePlane<'a, P> {
    /// The visible `width` by `height` samples, or `None` if the dimensions
    /// are zero, exceed signed 32-bit coordinates, overlap or leave `samples`.
    pub fn new(samples: &'a [P], stride: usize, width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return None;
        }
        let (width, height) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
        if stride < width {
            return None;
        }
        let end = (height - 1).checked_mul(stride)?.checked_add(width)?;
        Some(Self {
            samples: samples.get(..end)?,
            stride,
            width,
            height,
        })
    }

    fn sample(&self, x: i64, y: i64) -> i64 {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.samples
            .get(y * self.stride + x)
            .map_or(0, |p| i64::from(p.get()))
    }
}

/// A writable prediction region of at most 64 by 64 samples.
#[derive(Debug)]
pub struct PredictionMut<'a, P> {
    samples: &'a mut [P],
    stride: usize,
    width: usize,
    height: usize,
}

impl<'a, P: Pixel> PredictionMut<'a, P> {
    /// The region at `offset`, or `None` if a dimension is outside 1 to
    /// 64, its rows overlap or its samples leave the plane.
    pub fn new(
        samples: &'a mut [P],
        offset: usize,
        stride: usize,
        width: u8,
        height: u8,
    ) -> Option<Self> {
        if !(1..=64).contains(&width) || !(1..=64).contains(&height) {
            return None;
        }
        let (width, height) = (usize::from(width), usize::from(height));
        if stride < width {
            return None;
        }
        let end = offset.checked_add((height - 1).checked_mul(stride)?.checked_add(width)?)?;
        Some(Self {
            samples: samples.get_mut(offset..end)?,
            stride,
            width,
            height,
        })
    }
}

/// A reference position and steps in sixteenths of a sample. Signed
/// coordinates allow a block to extend beyond the reference's edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampling {
    x: i32,
    y: i32,
    step_x: u8,
    step_y: u8,
}

impl Sampling {
    /// A sampling grid whose steps are 1 to 80, the bound of 8.5.2.4,
    /// or `None`. Equal-size references use steps of 16.
    pub fn new(x: i32, y: i32, step_x: u8, step_y: u8) -> Option<Self> {
        if !(1..=80).contains(&step_x) || !(1..=80).contains(&step_y) {
            return None;
        }
        Some(Self {
            x,
            y,
            step_x,
            step_y,
        })
    }
}

/// Predicts a region from a reference plane (8.5.2.4). The horizontal
/// result is rounded and clipped before the vertical pass. Coordinates
/// beyond the reference repeat its last visible sample on that side.
///
/// For the first reference, `compound` is false and the result replaces
/// the destination. For the second, it is true and the result is averaged
/// with the first prediction, rounded as clause 8.5.2 specifies.
pub fn predict_inter<P: Pixel>(
    dst: PredictionMut<'_, P>,
    src: ReferencePlane<'_, P>,
    sampling: Sampling,
    filter: InterpFilter,
    bit_depth: BitDepth,
    compound: bool,
) {
    if sampling.step_x == 16 && sampling.step_y == 16 {
        let interior = Interior::new(&dst, src, sampling, bit_depth);
        unscaled(dst, src, sampling, filter, bit_depth, compound, interior);
    } else {
        scaled(dst, src, sampling, filter, bit_depth, compound);
    }
}

#[derive(Clone, Copy)]
struct Interior {
    left: usize,
    top: usize,
    #[cfg(all(target_arch = "x86_64", not(feature = "safe")))]
    horizontal: Option<crate::simd::Avx2>,
}

impl Interior {
    fn new<P: Pixel>(
        dst: &PredictionMut<'_, P>,
        src: ReferencePlane<'_, P>,
        sampling: Sampling,
        bit_depth: BitDepth,
    ) -> Option<Self> {
        if sampling.step_x != 16 || sampling.step_y != 16 {
            return None;
        }
        let horizontal = sampling.x & 15 != 0;
        let vertical = sampling.y & 15 != 0;
        let left = (i64::from(sampling.x) >> 4) - if horizontal { 3 } else { 0 };
        let top = (i64::from(sampling.y) >> 4) - if vertical { 3 } else { 0 };
        let right = left + dst.width as i64 + if horizontal { 7 } else { 0 };
        let bottom = top + dst.height as i64 + if vertical { 7 } else { 0 };
        if left < 0 || top < 0 || right > src.width as i64 || bottom > src.height as i64 {
            return None;
        }
        #[cfg(all(target_arch = "x86_64", not(feature = "safe")))]
        let horizontal_kernel = if bit_depth == BitDepth::Eight
            && horizontal
            && dst.width >= 8
            && P::bytes(src.samples).is_some()
        {
            crate::simd::Avx2::detect()
        } else {
            None
        };
        #[cfg(not(all(target_arch = "x86_64", not(feature = "safe"))))]
        let _ = bit_depth;
        Some(Self {
            left: left as usize,
            top: top as usize,
            #[cfg(all(target_arch = "x86_64", not(feature = "safe")))]
            horizontal: horizontal_kernel,
        })
    }
}

fn combine<P: Pixel>(sample: &mut P, predicted: P, bit_depth: BitDepth, compound: bool) {
    *sample = if compound {
        P::clip(
            round2(i64::from(predicted.get()) + i64::from(sample.get()), 1),
            bit_depth,
        )
    } else {
        predicted
    };
}

fn horizontal_row<P: Pixel>(
    output: &mut [P],
    src: ReferencePlane<'_, P>,
    position: (i64, i64),
    weights: &[i16; 8],
    phase: usize,
    bit_depth: BitDepth,
    interior: Option<Interior>,
) {
    if let Some(interior) = interior {
        let start = interior.top * src.stride + interior.left;
        let count = output.len() + if phase == 0 { 0 } else { 7 };
        let row = src.samples.get(start..start + count).unwrap_or_default();
        if phase == 0 {
            for (sample, source) in output.iter_mut().zip(row) {
                *sample = P::clip(i64::from(source.get()), bit_depth);
            }
        } else {
            #[allow(unused_mut)]
            let mut completed = 0;
            #[cfg(all(target_arch = "x86_64", not(feature = "safe")))]
            if let Some((kernel, bytes)) = interior.horizontal.zip(P::bytes(row)) {
                for (group, window) in output.chunks_exact_mut(8).zip(bytes.windows(15).step_by(8))
                {
                    let Some(window) = window.first_chunk::<15>() else {
                        break;
                    };
                    for (sample, sum) in group.iter_mut().zip(kernel.horizontal(window, weights)) {
                        *sample = P::clip(round2(i64::from(sum), 7), bit_depth);
                    }
                    completed += 8;
                }
            }
            for (sample, window) in output
                .iter_mut()
                .skip(completed)
                .zip(row.windows(8).skip(completed))
            {
                let sum = window
                    .iter()
                    .zip(weights)
                    .map(|(p, &w)| i64::from(p.get()) * i64::from(w))
                    .sum();
                *sample = P::clip(round2(sum, 7), bit_depth);
            }
        }
        return;
    }
    let (x, y) = position;
    for (c, sample) in output.iter_mut().enumerate() {
        let sum = if phase == 0 {
            src.sample(x + c as i64, y) * 128
        } else {
            weights
                .iter()
                .enumerate()
                .map(|(t, &w)| i64::from(w) * src.sample(x + c as i64 + t as i64 - 3, y))
                .sum()
        };
        *sample = P::clip(round2(sum, 7), bit_depth);
    }
}

fn unscaled<P: Pixel>(
    dst: PredictionMut<'_, P>,
    src: ReferencePlane<'_, P>,
    sampling: Sampling,
    filter: InterpFilter,
    bit_depth: BitDepth,
    compound: bool,
    interior: Option<Interior>,
) {
    let phase_x = (sampling.x & 15) as usize;
    let phase_y = (sampling.y & 15) as usize;
    let x = i64::from(sampling.x) >> 4;
    let y = i64::from(sampling.y) >> 4;
    if phase_x == 0 && phase_y == 0 {
        for (r, row) in dst
            .samples
            .chunks_mut(dst.stride)
            .take(dst.height)
            .enumerate()
        {
            let source = interior.and_then(|p| {
                let start = (p.top + r) * src.stride + p.left;
                src.samples.get(start..start + dst.width)
            });
            for (c, sample) in row.iter_mut().take(dst.width).enumerate() {
                let value = source.and_then(|s| s.get(c)).map_or_else(
                    || src.sample(x + c as i64, y + r as i64),
                    |p| i64::from(p.get()),
                );
                combine(sample, P::clip(value, bit_depth), bit_depth, compound);
            }
        }
        return;
    }
    let taps = SUBPEL_FILTERS.get(filter as usize);
    let weights_x = taps
        .and_then(|t| t.get(phase_x))
        .copied()
        .unwrap_or_default();
    let weights_y = taps
        .and_then(|t| t.get(phase_y))
        .copied()
        .unwrap_or_default();
    let extra = if phase_y == 0 { 0 } else { 7 };
    let mut intermediate = [[P::default(); 64]; 8];
    for r in 0..dst.height + extra {
        let row = intermediate
            .get_mut(r & 7)
            .and_then(|row| row.get_mut(..dst.width))
            .unwrap_or_default();
        horizontal_row(
            row,
            src,
            (x, y + r as i64 - if phase_y == 0 { 0 } else { 3 }),
            &weights_x,
            phase_x,
            bit_depth,
            interior.map(|p| Interior {
                top: p.top + r,
                ..p
            }),
        );
        if r < extra {
            continue;
        }
        let output_row = r - extra;
        let start = output_row * dst.stride;
        let output = dst
            .samples
            .get_mut(start..start + dst.width)
            .unwrap_or_default();
        for (c, sample) in output.iter_mut().enumerate() {
            let predicted = if phase_y == 0 {
                intermediate
                    .get(r & 7)
                    .and_then(|row| row.get(c))
                    .copied()
                    .unwrap_or_default()
            } else {
                let sum = weights_y
                    .iter()
                    .enumerate()
                    .map(|(t, &w)| {
                        i64::from(w)
                            * intermediate
                                .get((output_row + t) & 7)
                                .and_then(|row| row.get(c))
                                .map_or(0, |p| i64::from(p.get()))
                    })
                    .sum();
                P::clip(round2(sum, 7), bit_depth)
            };
            combine(sample, predicted, bit_depth, compound);
        }
    }
}

fn scaled<P: Pixel>(
    dst: PredictionMut<'_, P>,
    src: ReferencePlane<'_, P>,
    sampling: Sampling,
    filter: InterpFilter,
    bit_depth: BitDepth,
    compound: bool,
) {
    let taps = SUBPEL_FILTERS.get(filter as usize);
    let weights = |phase: usize| {
        taps.and_then(|table| table.get(phase))
            .copied()
            .unwrap_or_default()
    };
    let height = ((dst.height - 1) * usize::from(sampling.step_y)).div_ceil(16) + 8;
    let mut intermediate = vec![P::default(); height * dst.width];
    for (r, row) in intermediate.chunks_mut(dst.width).enumerate() {
        for (c, sample) in row.iter_mut().enumerate() {
            let p = i64::from(sampling.x) + i64::from(sampling.step_x) * c as i64;
            let y = (i64::from(sampling.y) >> 4) + r as i64 - 3;
            let sum: i64 = weights((p & 15) as usize)
                .iter()
                .enumerate()
                .map(|(t, &weight)| i64::from(weight) * src.sample((p >> 4) + t as i64 - 3, y))
                .sum();
            *sample = P::clip(round2(sum, 7), bit_depth);
        }
    }
    for (r, row) in dst
        .samples
        .chunks_mut(dst.stride)
        .take(dst.height)
        .enumerate()
    {
        let p = i64::from(sampling.y & 15) + i64::from(sampling.step_y) * r as i64;
        for (c, sample) in row.iter_mut().take(dst.width).enumerate() {
            let sum: i64 = weights((p & 15) as usize)
                .iter()
                .enumerate()
                .map(|(t, &weight)| {
                    let at = ((p >> 4) as usize + t) * dst.width + c;
                    i64::from(weight) * intermediate.get(at).map_or(0, |s| i64::from(s.get()))
                })
                .sum();
            let predicted = P::clip(round2(sum, 7), bit_depth);
            *sample = if compound {
                P::clip(
                    round2(i64::from(predicted.get()) + i64::from(sample.get()), 1),
                    bit_depth,
                )
            } else {
                predicted
            };
        }
    }
}

#[cfg(test)]
mod tests;
