use crate::tables::{DCT_BASIS, DST_BASIS};
use crate::{BitDepth, Block, BlockMut, Error};

pub(crate) struct Scratch<const N: usize> {
    scaled: [[i32; N]; N],
    intermediate: [[i32; N]; N],
}

/// The already-derived inverse transform path for a transform unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InverseTransform {
    /// Integer inverse DCT for chroma or a luma block larger than 4x4.
    Dct,
    /// The 4x4 intra-luma integer inverse DST.
    IntraLumaDst4,
    /// Transform skip: inverse scale and shift, but no matrix multiplication.
    Skip,
    /// Transquant bypass: coefficients are residuals; no scaling or shifting.
    Bypass,
}

/// Controls for inverse scaling and transformation, with syntax resolved by the caller.
#[derive(Clone, Copy, Debug)]
pub struct Reconstruction {
    /// Sample depth of the component.
    pub depth: BitDepth,
    /// Transform selection, including skip and bypass distinctions.
    pub transform: InverseTransform,
    /// Effective component QP after depth offset and chroma-QP derivation (8.6.1).
    pub qp: u8,
}

#[expect(clippy::indexing_slicing, reason = "compile-time table construction")]
const fn dct<const N: usize>() -> [[i32; N]; N] {
    let mut matrix = [[0; N]; N];
    let mut frequency = 0;
    while frequency < N {
        let mut position = 0;
        while position < N {
            matrix[frequency][position] = DCT_BASIS[frequency * (32 / N)][position] as i32;
            position += 1;
        }
        frequency += 1;
    }
    matrix
}

#[expect(clippy::indexing_slicing, reason = "compile-time table construction")]
const fn dst() -> [[i32; 4]; 4] {
    let mut matrix = [[0; 4]; 4];
    let mut frequency = 0;
    while frequency < 4 {
        let mut position = 0;
        while position < 4 {
            matrix[frequency][position] = DST_BASIS[frequency][position] as i32;
            position += 1;
        }
        frequency += 1;
    }
    matrix
}

const DST_4: [[i32; 4]; 4] = dst();
const DCT_4: [[i32; 4]; 4] = dct();
const DCT_8: [[i32; 8]; 8] = dct();
const DCT_16: [[i32; 16]; 16] = dct();
const DCT_32: [[i32; 32]; 32] = dct();

/// Converts quantized coefficient levels into residual samples (H.265 8.6.2–8.6.4).
///
/// `scaling` contains at least `N*N` nonzero, fully expanded, row-major effective
/// scaling factors, including the substituted DC factor. A disabled scaling list
/// is supplied as all 16s. Bypass ignores `scaling` and `qp` entirely.
/// Main/Main10 transform skip and intra-luma DST are valid only at 4x4.
/// Coefficients and residuals use independent checked strides; their sizes must
/// agree. The signed-16 coefficient domain and intermediate clipping implement
/// the non-extended transform range. Padding is unchanged.
/// Invalid inputs are rejected before output writes.
pub fn inverse_residual(
    config: Reconstruction,
    coefficients: Block<'_, i16>,
    scaling: &[u8],
    mut output: BlockMut<'_, i32>,
) -> Result<(), Error> {
    if coefficients.size != output.size {
        return Err(Error::BlockSize);
    }
    let n = coefficients.size.side();
    if matches!(
        config.transform,
        InverseTransform::Skip | InverseTransform::IntraLumaDst4
    ) && n != 4
    {
        return Err(Error::Transform);
    }
    if config.transform == InverseTransform::Bypass {
        for y in 0..n {
            for x in 0..n {
                output.put(x, y, i32::from(coefficients.at(x, y)?))?;
            }
        }
        return Ok(());
    }
    if config.qp > 51 + 6 * (config.depth.bits() - 8) {
        return Err(Error::Qp);
    }
    let scaling = scaling.get(..n * n).ok_or(Error::Buffer)?;
    if scaling.contains(&0) {
        return Err(Error::Scaling);
    }
    match (n, config.transform) {
        (4, InverseTransform::IntraLumaDst4) => {
            transform(config, &DST_4, coefficients, scaling, output)
        }
        (4, _) => transform(config, &DCT_4, coefficients, scaling, output),
        (8, _) => transform(config, &DCT_8, coefficients, scaling, output),
        (16, _) => transform(config, &DCT_16, coefficients, scaling, output),
        _ => transform(config, &DCT_32, coefficients, scaling, output),
    }
}

fn transform<const N: usize>(
    config: Reconstruction,
    matrix: &[[i32; N]; N],
    coefficients: Block<'_, i16>,
    scaling: &[u8],
    output: BlockMut<'_, i32>,
) -> Result<(), Error> {
    let mut scratch = Scratch::<N> {
        scaled: [[0; N]; N],
        intermediate: [[0; N]; N],
    };
    let scale_shift = u32::from(config.depth.bits()) + coefficients.size.log2() - 5;
    let level_scale = match config.qp % 6 {
        0 => 40,
        1 => 45,
        2 => 51,
        3 => 57,
        4 => 64,
        _ => 72,
    };
    let mut rows = 0;
    let mut columns = 0;
    for (y, (scaled, factors)) in scratch
        .scaled
        .iter_mut()
        .zip(scaling.chunks_exact(N))
        .enumerate()
    {
        let levels = coefficients
            .data
            .get(y * coefficients.stride..)
            .and_then(<[i16]>::first_chunk::<N>)
            .ok_or(Error::Buffer)?;
        for (x, ((value, &level), &factor)) in
            scaled.iter_mut().zip(levels).zip(factors).enumerate()
        {
            let product = (i64::from(level) * i64::from(factor) * level_scale) << (config.qp / 6);
            *value =
                ((product + (1 << (scale_shift - 1))) >> scale_shift).clamp(-32768, 32767) as i32;
            if *value != 0 {
                rows = y + 1;
                columns = columns.max(x + 1);
            }
        }
    }
    let shift = 20 - u32::from(config.depth.bits());
    let round = 1 << (shift - 1);
    if config.transform == InverseTransform::Skip {
        let ts_shift = 5 + coefficients.size.log2();
        for (y, scaled) in scratch.scaled.iter().enumerate() {
            let residuals = output
                .data
                .get_mut(y * output.stride..)
                .and_then(<[i32]>::first_chunk_mut::<N>)
                .ok_or(Error::Buffer)?;
            for (residual, &value) in residuals.iter_mut().zip(scaled) {
                *residual = ((value << ts_shift) + round) >> shift;
            }
        }
        return Ok(());
    }
    for (scaled, weights) in scratch.scaled.iter().zip(matrix).take(rows) {
        for (sums, &weight) in scratch.intermediate.iter_mut().zip(weights) {
            for (sum, &value) in sums.iter_mut().zip(scaled).take(columns) {
                *sum += weight * value;
            }
        }
    }
    for sums in &mut scratch.intermediate {
        for sum in sums.iter_mut().take(columns) {
            *sum = ((*sum + 64) >> 7).clamp(-32768, 32767);
        }
    }
    for (y, intermediate) in scratch.intermediate.iter().enumerate() {
        let mut sums = [0; N];
        for (&value, weights) in intermediate.iter().zip(matrix).take(columns) {
            for (sum, &weight) in sums.iter_mut().zip(weights) {
                *sum += value * weight;
            }
        }
        let residuals = output
            .data
            .get_mut(y * output.stride..)
            .and_then(<[i32]>::first_chunk_mut::<N>)
            .ok_or(Error::Buffer)?;
        for (residual, sum) in residuals.iter_mut().zip(sums) {
            *residual = (sum + round) >> shift;
        }
    }
    Ok(())
}

/// Clips prediction plus residual to the sample depth (H.265 equation 8-325).
///
/// Sizes must agree; input and output strides may differ. Prediction samples
/// must fit `depth`, residuals may span all of `i32`, and padding is untouched.
/// Invalid inputs are rejected before output writes.
pub fn add_residual(
    depth: BitDepth,
    prediction: Block<'_, u16>,
    residual: Block<'_, i32>,
    output: BlockMut<'_, u16>,
) -> Result<(), Error> {
    if prediction.size != residual.size || prediction.size != output.size {
        return Err(Error::BlockSize);
    }
    match (depth.bits(), prediction.size.side()) {
        (8, 4) => add_rows::<4, 255>(prediction, residual, output),
        (8, 8) => add_rows::<8, 255>(prediction, residual, output),
        (8, 16) => add_rows::<16, 255>(prediction, residual, output),
        (8, _) => add_rows::<32, 255>(prediction, residual, output),
        (_, 4) => add_rows::<4, 1023>(prediction, residual, output),
        (_, 8) => add_rows::<8, 1023>(prediction, residual, output),
        (_, 16) => add_rows::<16, 1023>(prediction, residual, output),
        _ => add_rows::<32, 1023>(prediction, residual, output),
    }
}

fn add_rows<const N: usize, const MAX: u16>(
    prediction: Block<'_, u16>,
    residual: Block<'_, i32>,
    output: BlockMut<'_, u16>,
) -> Result<(), Error> {
    for y in 0..N {
        let row = prediction
            .data
            .get(y * prediction.stride..)
            .and_then(<[u16]>::first_chunk::<N>)
            .ok_or(Error::Buffer)?;
        if row.iter().any(|&sample| sample > MAX) {
            return Err(Error::Sample);
        }
    }
    for y in 0..N {
        let prediction = prediction
            .data
            .get(y * prediction.stride..)
            .and_then(<[u16]>::first_chunk::<N>)
            .ok_or(Error::Buffer)?;
        let residual = residual
            .data
            .get(y * residual.stride..)
            .and_then(<[i32]>::first_chunk::<N>)
            .ok_or(Error::Buffer)?;
        let output = output
            .data
            .get_mut(y * output.stride..)
            .and_then(<[u16]>::first_chunk_mut::<N>)
            .ok_or(Error::Buffer)?;
        for ((&prediction, &residual), sample) in prediction.iter().zip(residual).zip(output) {
            *sample = residual
                .saturating_add(i32::from(prediction))
                .clamp(0, i32::from(MAX)) as u16;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
