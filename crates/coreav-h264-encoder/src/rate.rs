#![forbid(unsafe_code)]

use crate::{Error, Plane, RateMode, Result};

pub(crate) fn picture_qp(mode: RateMode, luma: Plane<'_>, idr: bool) -> Result<u8> {
    let crf = match mode {
        RateMode::FixedQp(qp) => return Ok(qp),
        RateMode::Crf(crf) => crf,
    };
    let mut sad = 0u64;
    for by in (0..luma.height).step_by(8) {
        for bx in (0..luma.width).step_by(8) {
            let end_x = (bx + 8).min(luma.width);
            let end_y = (by + 8).min(luma.height);
            let samples = (end_x - bx) * (end_y - by);
            let mut sum = 0u64;
            for y in by..end_y {
                for x in bx..end_x {
                    sum = sum
                        .checked_add(u64::from(luma.sample(x, y)?))
                        .ok_or(Error::Limit)?;
                }
            }
            let mean = sum / u64::try_from(samples).map_err(|_| Error::Limit)?;
            for y in by..end_y {
                for x in bx..end_x {
                    sad = sad
                        .checked_add(u64::from(luma.sample(x, y)?).abs_diff(mean))
                        .ok_or(Error::Limit)?;
                }
            }
        }
    }
    let samples = luma.width.checked_mul(luma.height).ok_or(Error::Limit)?;
    let activity = sad / u64::try_from(samples).map_err(|_| Error::Limit)?;
    let correction = match activity {
        0..8 => -1i16,
        8..12 => 0,
        12..24 => 1,
        _ => 2,
    };
    let correction = correction
        .checked_sub(i16::from(idr))
        .ok_or(Error::Limit)?
        .max(-1);
    let qp = i16::from(crf).checked_add(correction).ok_or(Error::Limit)?;
    u8::try_from(qp.clamp(10, 51)).map_err(|_| Error::Internal)
}
