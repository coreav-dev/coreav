use crate::bits::Bits;
use crate::{Error, Result};

/// A bounded short-term reference picture set in H.265 S0 then S1 order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rps {
    pub(crate) count: usize,
    pub(crate) negative_count: usize,
    pub(crate) delta: [i32; 16],
    pub(crate) used: [bool; 16],
}

impl Rps {
    /// Number of references marked as used by the current picture.
    pub fn used_count(&self) -> u32 {
        self.used
            .iter()
            .take(self.count)
            .filter(|&&used| used)
            .count() as u32
    }

    /// Signed POC differences, negative entries descending then positive ascending.
    pub fn deltas(&self) -> &[i32] {
        self.delta.get(..self.count).unwrap_or_default()
    }

    /// Current-picture usage flags corresponding to `deltas`.
    pub fn used_flags(&self) -> &[bool] {
        self.used.get(..self.count).unwrap_or_default()
    }

    /// Number of negative POC differences at the start of `deltas`.
    pub fn negative_count(&self) -> usize {
        self.negative_count
    }

    fn push(&mut self, delta: i32, used: bool) -> Result<()> {
        *self
            .delta
            .get_mut(self.count)
            .ok_or(Error::InvalidData("RPS picture count"))? = delta;
        *self
            .used
            .get_mut(self.count)
            .ok_or(Error::InvalidData("RPS picture count"))? = used;
        self.count += 1;
        Ok(())
    }
}

pub(crate) fn parse(
    bits: &mut Bits<'_>,
    previous: &[Rps],
    index: usize,
    in_slice: bool,
) -> Result<Rps> {
    let mut result = Rps::default();
    if index == 0 || !bits.flag()? {
        let negative = bits.ue(16)? as usize;
        let positive = bits.ue(16 - negative as u32)? as usize;
        let mut delta = 0i32;
        for _ in 0..negative {
            delta -= bits.ue(32767)? as i32 + 1;
            result.push(delta, bits.flag()?)?;
        }
        result.negative_count = negative;
        delta = 0;
        for _ in 0..positive {
            delta += bits.ue(32767)? as i32 + 1;
            result.push(delta, bits.flag()?)?;
        }
        return Ok(result);
    }
    let distance = if in_slice {
        bits.ue((index - 1) as u32)? as usize + 1
    } else {
        1
    };
    let reference = previous
        .get(index - distance)
        .ok_or(Error::InvalidData("RPS prediction index"))?;
    if reference.count > 16 || reference.negative_count > reference.count {
        return Err(Error::InvalidData("RPS prediction state"));
    }
    let negative = bits.flag()?;
    let magnitude = bits.ue(32767)? as i32 + 1;
    let shift = if negative { -magnitude } else { magnitude };
    let mut used = [false; 17];
    let mut included = [false; 17];
    for j in 0..=reference.count {
        let current = bits.flag()?;
        *used
            .get_mut(j)
            .ok_or(Error::InvalidData("RPS prediction count"))? = current;
        *included
            .get_mut(j)
            .ok_or(Error::InvalidData("RPS prediction count"))? = current || bits.flag()?;
    }
    for sign in [-1, 1] {
        for j in 0..=reference.count {
            if !included
                .get(j)
                .copied()
                .ok_or(Error::InvalidData("RPS prediction count"))?
            {
                continue;
            }
            let source = if j == reference.count {
                0
            } else {
                *reference
                    .delta
                    .get(j)
                    .ok_or(Error::InvalidData("RPS prediction count"))?
            };
            let delta = source
                .checked_add(shift)
                .ok_or(Error::InvalidData("RPS delta"))?;
            if delta.signum() != sign {
                continue;
            }
            let current = used
                .get(j)
                .copied()
                .ok_or(Error::InvalidData("RPS prediction count"))?;
            let start = if sign < 0 { 0 } else { result.negative_count };
            let mut position = start;
            while position < result.count {
                let existing = *result
                    .delta
                    .get(position)
                    .ok_or(Error::InvalidData("RPS picture count"))?;
                if (sign < 0 && delta > existing) || (sign > 0 && delta < existing) {
                    break;
                }
                position += 1;
            }
            result.push(delta, current)?;
            for k in (position..result.count - 1).rev() {
                let old_delta = *result
                    .delta
                    .get(k)
                    .ok_or(Error::InvalidData("RPS picture count"))?;
                let old_used = *result
                    .used
                    .get(k)
                    .ok_or(Error::InvalidData("RPS picture count"))?;
                *result
                    .delta
                    .get_mut(k + 1)
                    .ok_or(Error::InvalidData("RPS picture count"))? = old_delta;
                *result
                    .used
                    .get_mut(k + 1)
                    .ok_or(Error::InvalidData("RPS picture count"))? = old_used;
            }
            *result
                .delta
                .get_mut(position)
                .ok_or(Error::InvalidData("RPS picture count"))? = delta;
            *result
                .used
                .get_mut(position)
                .ok_or(Error::InvalidData("RPS picture count"))? = current;
        }
        if sign < 0 {
            result.negative_count = result.count;
        }
    }
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::params::tests::Writer;

    #[test]
    fn explicit_and_predicted_sets_keep_standard_order_and_used_flags() {
        let mut w = Writer::default();
        w.ue(2);
        w.ue(2);
        for (difference, used) in [(0, true), (1, false), (1, true), (2, false)] {
            w.ue(difference);
            w.flag(used);
        }
        let bytes = w.bytes();
        let mut bits = Bits::new(&bytes);
        let source = parse(&mut bits, &[], 0, false).unwrap();
        assert_eq!(bits.position(), w.len());
        assert_eq!(&source.delta[..source.count], &[-1, -3, 2, 5]);
        assert_eq!(source.used_count(), 2);
        for (negative, expected) in [(true, vec![-2, -3, -5, 3]), (false, vec![-1, 1, 2, 4, 7])] {
            let mut w = Writer::default();
            w.flag(true);
            w.flag(negative);
            w.ue(1);
            for j in 0..5 {
                let used = j % 2 == 0;
                w.flag(used);
                if !used {
                    w.flag(true);
                }
            }
            let bytes = w.bytes();
            let mut bits = Bits::new(&bytes);
            let result = parse(&mut bits, &[source], 1, false).unwrap();
            assert_eq!(&result.delta[..result.count], expected);
            assert_eq!(bits.position(), w.len());
            assert_eq!(result.negative_count, if negative { 3 } else { 1 });
            assert_eq!(result.used_count(), if negative { 2 } else { 3 });
        }
    }

    #[test]
    fn slice_prediction_can_select_an_earlier_set() {
        let source = Rps {
            count: 1,
            negative_count: 1,
            delta: [-1; 16],
            used: [true; 16],
        };
        let mut w = Writer::default();
        w.flag(true);
        w.ue(1);
        w.flag(false);
        w.ue(0);
        w.flag(true);
        w.flag(true);
        let bytes = w.bytes();
        let mut bits = Bits::new(&bytes);
        let result = parse(&mut bits, &[source, Rps::default()], 2, true).unwrap();
        assert_eq!(&result.delta[..result.count], &[1]);
        assert_eq!(bits.position(), w.len());
    }

    #[test]
    fn excessive_counts_bad_prediction_and_truncation_fail() {
        let mut w = Writer::default();
        w.ue(17);
        let bytes = w.bytes();
        assert!(matches!(
            parse(&mut Bits::new(&bytes), &[], 0, false),
            Err(Error::InvalidData(_))
        ));
        assert!(matches!(
            parse(&mut Bits::new(&[0x80]), &[], 1, false),
            Err(Error::InvalidData(_))
        ));
        assert!(matches!(
            parse(&mut Bits::new(&[]), &[], 0, false),
            Err(Error::InvalidData(_))
        ));
        let mut source = Rps {
            count: 16,
            negative_count: 16,
            ..Rps::default()
        };
        for (i, delta) in source.delta.iter_mut().enumerate() {
            *delta = -(i as i32 + 1);
        }
        let mut w = Writer::default();
        w.flag(true);
        w.flag(true);
        w.ue(0);
        for _ in 0..17 {
            w.flag(true);
        }
        let bytes = w.bytes();
        assert_eq!(
            parse(&mut Bits::new(&bytes), &[source], 1, false).unwrap_err(),
            Error::InvalidData("RPS picture count")
        );
    }
}
