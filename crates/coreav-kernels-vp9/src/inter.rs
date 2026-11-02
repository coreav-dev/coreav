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
