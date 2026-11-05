use crate::Error;

pub(crate) struct Buffer {
    bytes: Vec<u8>,
    max_len: usize,
    max_live: usize,
    base: usize,
}

impl Buffer {
    pub(crate) fn new(max_len: usize, max_live: usize, base: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_len,
            max_live,
            base,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn push(&mut self, byte: u8) -> Result<(), Error> {
        let needed = self
            .bytes
            .len()
            .checked_add(1)
            .ok_or(Error::Limit("packet bytes"))?;
        if needed > self.max_len {
            return Err(Error::Limit("packet bytes"));
        }
        if needed > self.bytes.capacity() {
            let available = self
                .max_live
                .checked_sub(self.base)
                .and_then(|n| n.checked_sub(self.bytes.capacity()))
                .ok_or(Error::Limit("live allocation"))?;
            let target = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(256)
                .min(self.max_len)
                .min(available);
            if target < needed {
                return Err(Error::Limit("live allocation"));
            }
            let mut replacement = Vec::new();
            replacement
                .try_reserve_exact(target)
                .map_err(|_| Error::Allocation)?;
            if replacement.capacity() > available || replacement.capacity() > self.max_len {
                return Err(Error::Limit("live allocation"));
            }
            replacement.extend_from_slice(&self.bytes);
            self.bytes = replacement;
        }
        self.bytes.push(byte);
        Ok(())
    }

    fn carry(&mut self, start: usize) -> Result<(), Error> {
        for byte in self
            .bytes
            .get_mut(start..)
            .ok_or(Error::InvalidFrame)?
            .iter_mut()
            .rev()
        {
            let (next, carry) = byte.overflowing_add(1);
            *byte = next;
            if !carry {
                return Ok(());
            }
        }
        Err(Error::InvalidFrame)
    }

    pub(crate) fn parity(&mut self, start: usize) -> Result<(), Error> {
        let crc = crc(self.bytes.get(start..).ok_or(Error::InvalidFrame)?);
        for byte in crc.to_be_bytes() {
            self.push(byte)?;
        }
        Ok(())
    }

    pub(crate) fn into_vec(self) -> Vec<u8> {
        self.bytes
    }
}

pub(crate) struct Range<'a> {
    out: &'a mut Buffer,
    start: usize,
    low: u32,
    width: u32,
}

impl<'a> Range<'a> {
    pub(crate) fn new(out: &'a mut Buffer) -> Self {
        Self {
            start: out.len(),
            out,
            low: 0,
            width: 65_280,
        }
    }

    pub(crate) fn bit(&mut self, state: &mut u8, bit: bool) -> Result<(), Error> {
        let upper = self.width * u32::from(*state) / 256;
        let lower = self.width - upper;
        if bit {
            self.low += lower;
            self.width = upper;
            *state = transition(*state)?;
        } else {
            self.width = lower;
            *state = 0u8.wrapping_sub(transition(0u8.wrapping_sub(*state))?);
        }
        if self.width < 256 {
            self.emit(self.low >> 8)?;
            self.low = (self.low & 255) << 8;
            self.width <<= 8;
        }
        Ok(())
    }

    fn emit(&mut self, digit: u32) -> Result<(), Error> {
        if self.out.len().saturating_sub(self.start) >= 0xff_ffff {
            return Err(Error::Limit("24-bit slice size"));
        }
        if digit >= 256 {
            self.out.carry(self.start)?;
        }
        self.out.push(digit as u8)
    }

    pub(crate) fn unsigned(&mut self, state: &mut [u8; 32], value: u32) -> Result<(), Error> {
        self.scalar(state, value, None)
    }

    pub(crate) fn signed(&mut self, state: &mut [u8; 32], value: i32) -> Result<(), Error> {
        self.scalar(state, value.unsigned_abs(), Some(value < 0))
    }

    fn scalar(
        &mut self,
        state: &mut [u8; 32],
        value: u32,
        sign: Option<bool>,
    ) -> Result<(), Error> {
        self.bit(state.first_mut().ok_or(Error::InvalidFrame)?, value == 0)?;
        if value == 0 {
            return Ok(());
        }
        let exponent = 31 - value.leading_zeros();
        for e in 0..=exponent {
            self.bit(
                state
                    .get_mut(1 + e.min(9) as usize)
                    .ok_or(Error::InvalidFrame)?,
                e != exponent,
            )?;
        }
        for i in (0..exponent).rev() {
            self.bit(
                state
                    .get_mut(22 + i.min(9) as usize)
                    .ok_or(Error::InvalidFrame)?,
                value & (1 << i) != 0,
            )?;
        }
        if let Some(sign) = sign {
            self.bit(
                state
                    .get_mut(11 + exponent.min(10) as usize)
                    .ok_or(Error::InvalidFrame)?,
                sign,
            )?;
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), Error> {
        self.emit(self.low.div_ceil(256))
    }
}

pub(crate) fn crc(bytes: &[u8]) -> u32 {
    let mut remainder = 0u32;
    for &byte in bytes {
        remainder ^= u32::from(byte) << 24;
        for _ in 0..8 {
            remainder = (remainder << 1)
                ^ if remainder & 0x8000_0000 != 0 {
                    0x04c1_1db7
                } else {
                    0
                };
        }
    }
    remainder
}

fn transition(state: u8) -> Result<u8, Error> {
    TRANSITIONS
        .get(usize::from(state))
        .copied()
        .ok_or(Error::InvalidFrame)
}

const TRANSITIONS: [u8; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37,
    37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 56, 57, 58, 59,
    60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 75, 76, 77, 78, 79, 80, 81, 82,
    83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103,
    104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 114, 115, 116, 117, 118, 119, 120, 121,
    122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 133, 134, 135, 136, 137, 138, 139,
    140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 150, 151, 152, 152, 153, 154, 155, 156, 157,
    158, 159, 160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 171, 172, 173, 174, 175,
    176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 190, 191, 192, 194,
    194, 195, 196, 197, 198, 199, 200, 201, 202, 202, 204, 205, 206, 207, 208, 209, 209, 210, 211,
    212, 213, 215, 215, 216, 217, 218, 219, 220, 220, 222, 223, 224, 225, 226, 227, 227, 229, 229,
    230, 231, 232, 234, 234, 235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248,
    248, 0, 0, 0, 0, 0, 0, 0,
];
