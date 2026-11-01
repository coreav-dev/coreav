//! The reader of bits that are not arithmetic coded: `f(n)` and `s(n)`
//! (clauses 4.9.1, 4.9.2 and 9.1).

use crate::error::{Error, Result};

/// Reads bits from the most significant bit of the first byte on.
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// The position of the next bit, counted from the first bit of `data`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    /// The position of the next bit (`get_position()`).
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// One bit, `f(1)`.
    pub(crate) fn bit(&mut self, what: &'static str) -> Result<bool> {
        let byte = self.data.get(self.pos >> 3).ok_or(Error::Truncated(what))?;
        let bit = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(bit == 1)
    }

    /// An unsigned number of `n` bits, `f(n)`, `n` at most 32.
    pub(crate) fn f(&mut self, n: u32, what: &'static str) -> Result<u32> {
        let mut x = 0u32;
        for _ in 0..n {
            x = (x << 1) | u32::from(self.bit(what)?);
        }
        Ok(x)
    }

    /// A signed number: `n` bits of magnitude, then a sign bit, `s(n)`.
    pub(crate) fn s(&mut self, n: u32, what: &'static str) -> Result<i32> {
        let value = i32::try_from(self.f(n, what)?).unwrap_or(i32::MAX);
        Ok(if self.bit(what)? { -value } else { value })
    }

    /// Reads up to the next byte boundary (`trailing_bits()`, clause
    /// 6.1.1) and says whether every bit read was 0, as `zero_bit` must be.
    pub(crate) fn byte_align(&mut self, what: &'static str) -> Result<bool> {
        let mut zero = true;
        while self.pos & 7 != 0 {
            zero &= !self.bit(what)?;
        }
        Ok(zero)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn bits_are_read_from_the_most_significant_on() {
        let mut r = BitReader::new(&[0b1010_0000, 0xFF]);
        assert_eq!(r.f(3, "x"), Ok(0b101));
        assert_eq!(r.position(), 3);
        assert_eq!(r.byte_align("x"), Ok(true));
        assert_eq!(r.f(8, "x"), Ok(0xFF));
        assert_eq!(r.bit("end"), Err(Error::Truncated("end")));
    }

    #[test]
    fn a_signed_number_has_its_sign_after_its_magnitude() {
        assert_eq!(BitReader::new(&[0b0101_1000]).s(4, "x"), Ok(-5));
        assert_eq!(BitReader::new(&[0b0101_0000]).s(4, "x"), Ok(5));
    }

    #[test]
    fn alignment_reports_a_bit_that_is_not_zero() {
        let mut r = BitReader::new(&[0b1000_0100]);
        r.bit("x").unwrap();
        assert_eq!(r.byte_align("x"), Ok(false));
        assert_eq!(r.position(), 8);
    }
}
