use crate::{Error, Result};

#[derive(Clone)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    pub(crate) fn flag(&mut self) -> Result<bool> {
        let byte = self
            .data
            .get(self.pos / 8)
            .ok_or(Error::InvalidData("truncated_header"))?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(bit != 0)
    }

    pub(crate) fn u(&mut self, n: u32) -> Result<u32> {
        if n > 32 {
            return Err(Error::InvalidData("bit_width"));
        }
        let mut value = 0;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.flag()?);
        }
        Ok(value)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<()> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(Error::Limit("bit_position"))?;
        if end > self.data.len().saturating_mul(8) {
            return Err(Error::InvalidData("truncated_header"));
        }
        self.pos = end;
        Ok(())
    }

    pub(crate) fn ue(&mut self, max: u32) -> Result<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                return Err(Error::InvalidData("exp_golomb_overflow"));
            }
        }
        let value = ((1u32 << zeros) - 1) + self.u(zeros)?;
        if value > max {
            return Err(Error::InvalidData("exp_golomb_range"));
        }
        Ok(value)
    }

    pub(crate) fn se(&mut self, min: i32, max: i32) -> Result<i32> {
        let code = self.ue(u32::MAX - 1)?;
        let magnitude = i32::try_from(code.div_ceil(2))
            .map_err(|_| Error::InvalidData("signed_golomb_overflow"))?;
        let value = if code & 1 != 0 { magnitude } else { -magnitude };
        if !(min..=max).contains(&value) {
            return Err(Error::InvalidData("signed_golomb_range"));
        }
        Ok(value)
    }

    pub(crate) fn alignment(&mut self) -> Result<()> {
        if !self.flag()? {
            return Err(Error::InvalidData("alignment_one_bit"));
        }
        while self.pos % 8 != 0 {
            if self.flag()? {
                return Err(Error::InvalidData("alignment_zero_bit"));
            }
        }
        Ok(())
    }

    pub(crate) fn trailing(&mut self) -> Result<()> {
        self.alignment()?;
        if self.pos != self.data.len().saturating_mul(8) {
            return Err(Error::InvalidData("rbsp_trailing_bytes"));
        }
        Ok(())
    }

    pub(crate) fn more_rbsp_data(&self) -> bool {
        let Some(byte) = self.data.get(self.pos / 8) else {
            return false;
        };
        let remaining = 8 - self.pos % 8;
        let mask = (1u16 << remaining) - 1;
        usize::from(*byte) & usize::from(mask) != 1usize << (remaining - 1)
            || self.pos / 8 + 1 != self.data.len()
    }
}
