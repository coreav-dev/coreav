//! A most-significant-bit-first bit reader over a byte slice.
//!
//! The reader keeps up to 64 bits in a cache, left-aligned, with every bit
//! below the valid ones zero. It refills eight bytes at a time while eight
//! remain, then byte by byte, and fails with [`Error::Truncated`] instead
//! of reading past the slice.

use crate::Error;

/// The reader.
#[derive(Clone)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Index of the first byte not yet in the cache.
    next: usize,
    /// Valid bits, left-aligned; the bits below them are zero.
    cache: u64,
    /// How many bits of `cache` are valid, 0 to 64.
    have: u32,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            next: 0,
            cache: 0,
            have: 0,
        }
    }

    /// Loads as many whole bytes as fit into the cache.
    #[inline(always)]
    fn refill(&mut self) {
        let room = (64 - self.have) / 8;
        if room == 0 {
            return;
        }
        let end = self.next.saturating_add(8);
        if let Some(chunk) = self.data.get(self.next..end) {
            let Ok(bytes) = <[u8; 8]>::try_from(chunk) else {
                return;
            };
            let word = u64::from_be_bytes(bytes);
            let loaded = room * 8;
            // `have` is at most 56 here, so both shifts are below 64.
            let total = self.have + loaded;
            let keep = if total >= 64 {
                u64::MAX
            } else {
                !(u64::MAX >> total)
            };
            self.cache |= (word >> self.have) & keep;
            self.have += loaded;
            self.next += room as usize;
        } else {
            while self.have <= 56 {
                let Some(&byte) = self.data.get(self.next) else {
                    break;
                };
                self.cache |= u64::from(byte) << (56 - self.have);
                self.have += 8;
                self.next += 1;
            }
        }
    }

    /// Makes at least `n` bits (at most 57) available.
    #[inline(always)]
    fn need(&mut self, n: u32) -> Result<(), Error> {
        if self.have < n {
            self.refill();
            if self.have < n {
                return Err(Error::Truncated);
            }
        }
        Ok(())
    }

    /// Drops `n` bits, at most the valid ones.
    #[inline(always)]
    fn consume(&mut self, n: u32) {
        // n <= have <= 64; a shift by 64 must be done in two steps.
        self.cache = (self.cache << (n / 2)) << (n - n / 2);
        self.have -= n;
    }

    /// Reads `n` bits, 0 to 32, as an unsigned number.
    #[inline(always)]
    pub(crate) fn read(&mut self, n: u32) -> Result<u32, Error> {
        if n == 0 {
            return Ok(0);
        }
        self.need(n)?;
        let value = (self.cache >> (64 - n)) as u32;
        self.consume(n);
        Ok(value)
    }

    /// Reads one bit.
    #[inline(always)]
    pub(crate) fn bit(&mut self) -> Result<bool, Error> {
        Ok(self.read(1)? == 1)
    }

    /// Reads `n` bits, 0 to 64, as an unsigned number.
    pub(crate) fn read_u64(&mut self, n: u32) -> Result<u64, Error> {
        if n > 32 {
            let high = u64::from(self.read(n - 32)?);
            Ok((high << 32) | u64::from(self.read(32)?))
        } else {
            Ok(u64::from(self.read(n)?))
        }
    }

    /// Reads `n` bits, 0 to 32, as a two's complement number.
    #[inline(always)]
    pub(crate) fn signed(&mut self, n: u32) -> Result<i32, Error> {
        if n == 0 {
            return Ok(0);
        }
        let value = self.read(n)?;
        let unused = 32 - n;
        Ok(((value << unused) as i32) >> unused)
    }

    /// Reads `n` bits, 0 to 64, as a two's complement number.
    pub(crate) fn signed_wide(&mut self, n: u32) -> Result<i64, Error> {
        if n == 0 {
            return Ok(0);
        }
        let value = self.read_u64(n)?;
        let unused = 64 - n;
        Ok(((value << unused) as i64) >> unused)
    }

    /// Counts zero bits up to the next one bit and consumes them and the
    /// one; fails once more than `max` zeros were counted.
    #[inline(always)]
    pub(crate) fn unary(&mut self, max: u32) -> Result<u32, Error> {
        let mut count: u32 = 0;
        loop {
            if self.have == 0 {
                self.refill();
                if self.have == 0 {
                    return Err(Error::Truncated);
                }
            }
            let zeros = self.cache.leading_zeros();
            if zeros < self.have {
                count = count.saturating_add(zeros);
                if count > max {
                    return Err(Error::Invalid("unary number too large"));
                }
                self.consume(zeros + 1);
                return Ok(count);
            }
            count = count.saturating_add(self.have);
            if count > max {
                return Err(Error::Invalid("unary number too large"));
            }
            self.cache = 0;
            self.have = 0;
        }
    }

    /// Decodes Rice-coded residuals with parameter `k` (0 to 31) into
    /// `out` (RFC 9639 9.2.7.2). A folded value that does not fit 32 bits
    /// is an error.
    #[inline]
    pub(crate) fn rice(&mut self, k: u32, out: &mut [i32]) -> Result<(), Error> {
        // The largest quotient whose folded value still fits 32 bits.
        let max_quotient = u32::MAX >> k;
        for slot in out.iter_mut() {
            if self.have < 32 {
                self.refill();
            }
            let zeros = self.cache.leading_zeros();
            let folded = if zeros < self.have && zeros + 1 + k <= self.have {
                // The whole code word is in the cache.
                let rest = self.cache << zeros << 1;
                // A shift by 64 - k; k = 0 reads nothing.
                let low = (rest >> 1) >> (63 - k);
                self.cache = rest << k;
                self.have -= zeros + 1 + k;
                if zeros > max_quotient {
                    return Err(Error::Invalid("Rice code too large"));
                }
                (zeros << k) | low as u32
            } else {
                let quotient = self.unary(max_quotient)?;
                (quotient << k) | self.read(k)?
            };
            *slot = ((folded >> 1) as i32) ^ -((folded & 1) as i32);
        }
        Ok(())
    }

    /// Skips to the next byte boundary.
    pub(crate) fn align(&mut self) {
        let partial = self.have % 8;
        self.consume(partial);
    }

    /// Whole bytes read so far, after [`Bits::align`].
    pub(crate) fn byte_position(&self) -> usize {
        self.next - (self.have / 8) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_refills() {
        let data: Vec<u8> = (0..40u8).collect();
        let mut bits = Bits::new(&data);
        for expected in 0..40u32 {
            assert_eq!(bits.read(8).unwrap(), expected);
        }
        assert!(matches!(bits.read(1), Err(Error::Truncated)));
    }

    #[test]
    fn odd_widths_match_a_naive_reader() {
        let data: Vec<u8> = (0..97u32).map(|i| (i * 37 + 11) as u8).collect();
        let naive = |pos: u64, n: u32| -> u32 {
            let mut v = 0u32;
            for i in 0..u64::from(n) {
                let bit = pos + i;
                let byte = data[(bit / 8) as usize];
                v = (v << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
            }
            v
        };
        let mut bits = Bits::new(&data);
        let mut pos = 0u64;
        let widths = [1, 7, 13, 32, 3, 31, 5, 17, 9, 24, 2, 30, 11, 32, 32, 6];
        for &n in widths.iter().cycle().take(40) {
            if pos + u64::from(n) > 97 * 8 {
                break;
            }
            assert_eq!(bits.read(n).unwrap(), naive(pos, n), "at {pos} width {n}");
            pos += u64::from(n);
        }
    }

    #[test]
    fn signed_and_unary() {
        let data = [0b1110_0000, 0b0001_0000, 0xFF];
        let mut bits = Bits::new(&data);
        assert_eq!(bits.signed(3).unwrap(), -1);
        assert_eq!(bits.unary(100).unwrap(), 8);
        assert_eq!(bits.read(4).unwrap(), 0);
        let mut bits = Bits::new(&[0u8; 4]);
        assert!(matches!(bits.unary(10), Err(Error::Invalid(_))));
        let mut bits = Bits::new(&[0u8; 2]);
        assert!(matches!(bits.unary(100), Err(Error::Truncated)));
    }

    #[test]
    fn rice_example_of_rfc_9639() {
        // Section 9.2.7.2: parameter 3, folded 38 is 0b00001_110.
        let data = [0b0000_1110, 0b1000_0000];
        let mut bits = Bits::new(&data);
        let mut out = [0i32; 2];
        bits.rice(3, &mut out).unwrap();
        // 38 unfolds to 19; the next word 0b1_000 is folded 0, value 0.
        assert_eq!(out, [19, 0]);
    }
}
