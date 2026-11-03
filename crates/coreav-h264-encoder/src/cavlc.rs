use crate::vlc::{CHROMA_ZEROS, RUNS, TOKENS, ZEROS};
use crate::{Error, Result, bits::Bits};

pub(crate) fn residual(w: &mut Bits, coefficients: &[i32], nc: i32) -> Result<u8> {
    if !matches!(coefficients.len(), 4 | 15 | 16) {
        return Err(Error::Internal);
    }
    let mut values = [0i32; 16];
    let mut positions = [0usize; 16];
    let mut total = 0;
    for (position, &level) in coefficients.iter().enumerate() {
        if level != 0 {
            *values.get_mut(total).ok_or(Error::Internal)? = level;
            *positions.get_mut(total).ok_or(Error::Internal)? = position;
            total += 1;
        }
    }
    let values = values.get(..total).ok_or(Error::Internal)?;
    let mut trailing = 0;
    for &level in values.iter().rev().take(3) {
        if level.unsigned_abs() != 1 {
            break;
        }
        trailing += 1;
    }
    let column = match nc {
        -1 => 4,
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        8.. => 3,
        _ => return Err(Error::Internal),
    };
    let token = TOKENS
        .get(total * 4 + trailing)
        .and_then(|row| row.get(column))
        .ok_or(Error::Internal)?;
    w.code(*token)?;
    for &level in values.iter().rev().take(trailing) {
        w.flag(level < 0)?;
    }
    let mut suffix = u32::from(total > 10 && trailing < 3);
    for (i, &level) in values.iter().rev().enumerate().skip(trailing) {
        let abs = u64::from(level.unsigned_abs());
        let mut code = abs * 2 - 2 + u64::from(level < 0);
        if i == trailing && trailing < 3 {
            code = code.checked_sub(2).ok_or(Error::Syntax)?;
        }
        let mut found = false;
        for prefix in 0..=15u32 {
            let size = if prefix == 14 && suffix == 0 {
                4
            } else if prefix == 15 {
                12
            } else {
                suffix
            };
            let base =
                (u64::from(prefix) << suffix) + if prefix == 15 && suffix == 0 { 15 } else { 0 };
            if code >= base && code - base < 1u64 << size {
                w.u(prefix, 0)?;
                w.flag(true)?;
                w.u(size, code - base)?;
                found = true;
                break;
            }
        }
        if !found {
            return Err(Error::Coefficient);
        }
        suffix = suffix.max(1);
        if suffix < 6 && abs > 3u64 << (suffix - 1) {
            suffix += 1;
        }
    }
    if total != 0 && total < coefficients.len() {
        let zeros = positions.get(total - 1).ok_or(Error::Internal)? + 1 - total;
        let table = if coefficients.len() == 4 {
            &CHROMA_ZEROS[..]
        } else {
            &ZEROS[..]
        };
        w.code(
            *table
                .get(total - 1)
                .and_then(|r| r.get(zeros))
                .ok_or(Error::Internal)?,
        )?;
    }
    if total > 1 {
        let mut left = positions.get(total - 1).ok_or(Error::Internal)? + 1 - total;
        for i in (1..total).rev() {
            if left == 0 {
                break;
            }
            let run = positions.get(i).ok_or(Error::Internal)?
                - positions.get(i - 1).ok_or(Error::Internal)?
                - 1;
            w.code(
                *RUNS
                    .get(left.min(7) - 1)
                    .and_then(|r| r.get(run))
                    .ok_or(Error::Internal)?,
            )?;
            left -= run;
        }
    }
    Ok(total as u8)
}
