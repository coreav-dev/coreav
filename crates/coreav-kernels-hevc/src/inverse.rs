use crate::tables::{DCT_BASIS, DST_BASIS};
use crate::{BitDepth, Block, BlockMut, Error};

pub(crate) struct Scratch {
    scaled: [i64; 1024],
    intermediate: [i64; 1024],
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

fn weight(
    kind: InverseTransform,
    n: usize,
    position: usize,
    frequency: usize,
) -> Result<i64, Error> {
    let value = if kind == InverseTransform::IntraLumaDst4 {
        DST_BASIS.get(frequency).and_then(|row| row.get(position))
    } else {
        DCT_BASIS
            .get(frequency * (32 / n))
            .and_then(|row| row.get(position))
    };
    value.copied().map(i64::from).ok_or(Error::Buffer)
}

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
    let mut scratch = Scratch {
        scaled: [0; 1024],
        intermediate: [0; 1024],
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
    for y in 0..n {
        for x in 0..n {
            let index = y * n + x;
            let factor = i64::from(*scaling.get(index).ok_or(Error::Buffer)?);
            let value =
                (i64::from(coefficients.at(x, y)?) * factor * level_scale) << (config.qp / 6);
            *scratch.scaled.get_mut(index).ok_or(Error::Buffer)? =
                ((value + (1 << (scale_shift - 1))) >> scale_shift).clamp(-32768, 32767);
        }
    }
    let shift = 20 - config.depth.bits();
    if config.transform == InverseTransform::Skip {
        let ts_shift = 5 + coefficients.size.log2();
        for y in 0..n {
            for x in 0..n {
                let value = *scratch.scaled.get(y * n + x).ok_or(Error::Buffer)? << ts_shift;
                output.put(x, y, ((value + (1 << (shift - 1))) >> shift) as i32)?;
            }
        }
        return Ok(());
    }
    for x in 0..n {
        for y in 0..n {
            let mut sum = 0_i64;
            for j in 0..n {
                sum += weight(config.transform, n, y, j)?
                    * scratch
                        .scaled
                        .get(j * n + x)
                        .copied()
                        .ok_or(Error::Buffer)?;
            }
            *scratch
                .intermediate
                .get_mut(y * n + x)
                .ok_or(Error::Buffer)? = ((sum + 64) >> 7).clamp(-32768, 32767);
        }
    }
    for y in 0..n {
        for x in 0..n {
            let mut sum = 0_i64;
            for j in 0..n {
                sum += weight(config.transform, n, x, j)?
                    * scratch
                        .intermediate
                        .get(y * n + j)
                        .copied()
                        .ok_or(Error::Buffer)?;
            }
            output.put(x, y, ((sum + (1 << (shift - 1))) >> shift) as i32)?;
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
    mut output: BlockMut<'_, u16>,
) -> Result<(), Error> {
    if prediction.size != residual.size || prediction.size != output.size {
        return Err(Error::BlockSize);
    }
    let n = prediction.size.side();
    for y in 0..n {
        for x in 0..n {
            if prediction.at(x, y)? > depth.max_sample() {
                return Err(Error::Sample);
            }
        }
    }
    for y in 0..n {
        for x in 0..n {
            output.put(
                x,
                y,
                depth.clip(i64::from(prediction.at(x, y)?) + i64::from(residual.at(x, y)?)),
            )?;
        }
    }
    Ok(())
}
