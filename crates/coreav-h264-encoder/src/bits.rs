#![forbid(unsafe_code)]

use crate::{Error, Result};

pub(crate) struct Bits {
    bytes: Option<Vec<u8>>,
    pub(crate) len: usize,
    limit: usize,
}

impl Bits {
    pub(crate) fn count() -> Self {
        Self {
            bytes: None,
            len: 0,
            limit: usize::MAX,
        }
    }

    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: Some(Vec::new()),
            len: 0,
            limit,
        }
    }

    #[inline]
    pub(crate) fn u(&mut self, width: u32, value: u64) -> Result<()> {
        if width > 64 || (width < 64 && value >> width != 0) {
            return Err(Error::Syntax);
        }
        let end = self.len.checked_add(width as usize).ok_or(Error::Limit)?;
        if end.div_ceil(8) > self.limit {
            return Err(Error::Limit);
        }
        if self.bytes.is_none() {
            self.len = end;
            return Ok(());
        }
        self.write(width, value, end)
    }

    #[inline(never)]
    fn write(&mut self, width: u32, value: u64, end: usize) -> Result<()> {
        if width == 0 {
            return Ok(());
        }
        if let Some(bytes) = &mut self.bytes {
            bytes
                .try_reserve(end.div_ceil(8).saturating_sub(bytes.len()))
                .map_err(|_| Error::Allocation)?;
            let used = self.len % 8;
            let encoded = (u128::from(value) << (128 - used - width as usize)).to_be_bytes();
            let count = (used + width as usize).div_ceil(8);
            let start = if used == 0 {
                0
            } else {
                *bytes.last_mut().ok_or(Error::Internal)? |=
                    *encoded.first().ok_or(Error::Internal)?;
                1
            };
            bytes.extend_from_slice(encoded.get(start..count).ok_or(Error::Internal)?);
        }
        self.len = end;
        Ok(())
    }

    pub(crate) fn flag(&mut self, value: bool) -> Result<()> {
        self.u(1, u64::from(value))
    }

    pub(crate) fn ue(&mut self, value: u32) -> Result<()> {
        let number = u64::from(value) + 1;
        let width = 64 - number.leading_zeros();
        if width == 33 {
            self.u(width - 1, 0)?;
            self.u(width, number)
        } else {
            self.u(width * 2 - 1, number)
        }
    }

    pub(crate) fn se(&mut self, value: i32) -> Result<()> {
        let mapped = if value <= 0 {
            -2 * i64::from(value)
        } else {
            2 * i64::from(value) - 1
        };
        self.ue(u32::try_from(mapped).map_err(|_| Error::Syntax)?)
    }

    pub(crate) fn code(&mut self, packed: u32) -> Result<()> {
        let width = packed & 31;
        if width == 0 {
            return Err(Error::Syntax);
        }
        self.u(width, u64::from(packed >> 5))
    }

    pub(crate) fn nal(mut self, header: u8) -> Result<Vec<u8>> {
        self.flag(true)?;
        while self.len % 8 != 0 {
            self.flag(false)?;
        }
        let bytes = self.bytes.ok_or(Error::Internal)?;
        let mut output = Vec::new();
        output
            .try_reserve(bytes.len().saturating_mul(3) / 2 + 1)
            .map_err(|_| Error::Allocation)?;
        output.push(header);
        let mut zeros = 0;
        for b in bytes {
            if zeros == 2 && b <= 3 {
                output.push(3);
                zeros = 0;
            }
            output.push(b);
            zeros = if b == 0 { zeros + 1 } else { 0 };
            if output.len() > self.limit {
                return Err(Error::Limit);
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn wide_writes_match_individual_bits_at_every_alignment() {
        let mut state = 1u64;
        for offset in 0..8 {
            for width in 0..=64 {
                for _ in 0..32 {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let value = if width == 64 {
                        state
                    } else {
                        state & ((1u64 << width) - 1)
                    };
                    let mut wide = Bits::new(64);
                    let mut reference = Bits::new(64);
                    wide.u(offset, (1 << offset) - 1).unwrap();
                    reference.u(offset, (1 << offset) - 1).unwrap();
                    wide.u(width, value).unwrap();
                    for bit in (0..width).rev() {
                        reference.u(1, (value >> bit) & 1).unwrap();
                    }
                    assert_eq!(wide.len, reference.len);
                    assert_eq!(wide.bytes, reference.bytes);
                    wide.u(7, 85).unwrap();
                    reference.u(7, 85).unwrap();
                    assert_eq!(wide.nal(1).unwrap(), reference.nal(1).unwrap());
                }
            }
        }
    }

    #[test]
    fn golomb_codes_and_refusals_keep_exact_bits() {
        for value in [0, 1, 2, 3, 255, 256, u32::MAX - 1, u32::MAX] {
            let mut packed = Bits::new(64);
            let mut reference = Bits::new(64);
            packed.ue(value).unwrap();
            let number = u64::from(value) + 1;
            let width = 64 - number.leading_zeros();
            reference.u(width - 1, 0).unwrap();
            reference.u(width, number).unwrap();
            assert_eq!(packed.len, reference.len);
            assert_eq!(packed.bytes, reference.bytes);
        }
        let mut writer = Bits::new(1);
        writer.u(7, 127).unwrap();
        assert_eq!(writer.u(2, 0), Err(Error::Limit));
        assert_eq!(writer.u(1, 2), Err(Error::Syntax));
        assert_eq!(writer.u(65, 0), Err(Error::Syntax));
        assert_eq!(writer.len, 7);
        writer.u(1, 1).unwrap();
        assert_eq!(writer.bytes, Some(vec![255]));
    }
}
