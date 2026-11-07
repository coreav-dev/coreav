#![forbid(unsafe_code)]

use crate::vlc::{CHROMA_ZEROS, RUNS, TOKENS, ZEROS};
use crate::{Error, Result, bits::Bits};

fn column(nc: i32) -> Result<usize> {
    match nc {
        -1 => Ok(4),
        0..=1 => Ok(0),
        2..=3 => Ok(1),
        4..=7 => Ok(2),
        8.. => Ok(3),
        _ => Err(Error::Internal),
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Cost {
    fixed: usize,
    token: u8,
    pub(crate) total: u8,
}

impl Cost {
    pub(crate) fn new(coefficients: &[i32], nc: i32) -> Result<Self> {
        let mut counter = Bits::count();
        let total = residual(&mut counter, coefficients, nc)?;
        let trailing = coefficients
            .iter()
            .rev()
            .filter(|&&level| level != 0)
            .take(3)
            .take_while(|level| level.unsigned_abs() == 1)
            .count();
        let tokens = TOKENS
            .get(usize::from(total) * 4 + trailing)
            .ok_or(Error::Internal)?;
        let fixed = counter.len - (*tokens.get(column(nc)?).ok_or(Error::Internal)? & 31) as usize;
        Ok(Self {
            fixed,
            token: (usize::from(total) * 4 + trailing) as u8,
            total,
        })
    }

    pub(crate) fn bits(self, nc: i32) -> Result<usize> {
        let width = TOKENS
            .get(usize::from(self.token))
            .and_then(|tokens| tokens.get(column(nc).ok()?))
            .ok_or(Error::Internal)?
            & 31;
        if width == 0 {
            return Err(Error::Syntax);
        }
        Ok(self.fixed + width as usize)
    }
}

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
    let column = column(nc)?;
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod cost_tests {
    use super::*;

    #[test]
    fn cached_cost_matches_the_writer_for_every_context_and_residual_shape() {
        let mut state = 1u32;
        for size in [4, 15, 16] {
            for _ in 0..1024 {
                let mut values = [0; 16];
                for value in values.iter_mut().take(size) {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    *value = match state % 5 {
                        0 => (state % 63) as i32 - 31,
                        1 => 1,
                        2 => -1,
                        _ => 0,
                    };
                }
                let values = &values[..size];
                let first = if size == 4 { -1 } else { 0 };
                let cached = Cost::new(values, first).unwrap();
                for nc in first..=16 {
                    let mut writer = Bits::count();
                    assert_eq!(residual(&mut writer, values, nc).unwrap(), cached.total);
                    assert_eq!(writer.len, cached.bits(nc).unwrap());
                }
            }
        }
        assert!(Cost::new(&[i32::MAX; 16], 0).is_err());
    }
}
