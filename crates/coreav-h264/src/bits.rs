//! The RBSP bit reader: fixed-length codes, Exp-Golomb codes and
//! `more_rbsp_data()` (clauses 7.2 and 9.1).
#![forbid(unsafe_code)]

use core::cell::Cell;

use crate::error::{Error, Result};

/// A most-significant-bit-first reader over one RBSP.
///
/// Reading past the end is an error, never a panic: the reader knows how
/// many bits it has.
#[derive(Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Bits consumed so far.
    pos: usize,
    len_bits: usize,
    /// Position of the rbsp_stop_one_bit, found when first asked for.
    stop_bit: Cell<Option<Option<usize>>>,
}

impl<'a> BitReader<'a> {
    /// A reader over `data`.
    pub fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader {
            data,
            pos: 0,
            len_bits: data.len().saturating_mul(8),
            stop_bit: Cell::new(None),
        }
    }

    /// Bits consumed so far.
    pub const fn pos(&self) -> usize {
        self.pos
    }

    /// Bits left before the end of the RBSP.
    pub const fn bits_left(&self) -> usize {
        self.len_bits.saturating_sub(self.pos)
    }

    /// The RBSP this reader reads.
    pub const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Whether the next bit starts a byte.
    pub const fn byte_aligned(&self) -> bool {
        self.pos % 8 == 0
    }

    /// The next 32 bits, zero-padded past the end of the RBSP.
    #[inline]
    pub fn peek32(&self) -> u32 {
        let byte = self.pos / 8;
        let shift = (self.pos % 8) as u32;
        let window = self
            .data
            .get(byte..byte + 8)
            .and_then(|w| <[u8; 8]>::try_from(w).ok());
        let v = match window {
            Some(w) => u64::from_be_bytes(w),
            // Near the end: five bytes are enough for 32 bits at any offset.
            None => {
                (0..5).fold(0u64, |v, i| {
                    (v << 8) | u64::from(self.data.get(byte + i).copied().unwrap_or(0))
                }) << 24
            }
        };
        ((v << shift) >> 32) as u32
    }

    /// Skips `n` bits.
    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<()> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.len_bits)
            .ok_or(Error::Truncated("a syntax element"))?;
        self.pos = end;
        Ok(())
    }

    /// `u(n)` for `n` up to 32 (clause 7.2).
    #[inline]
    pub fn u(&mut self, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 || self.pos.saturating_add(n as usize) > self.len_bits {
            return Err(Error::Truncated("a fixed-length code"));
        }
        let v = self.peek32();
        self.pos += n as usize;
        Ok(if n == 32 { v } else { v >> (32 - n) })
    }

    /// `u(1)` as a flag.
    #[inline]
    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.u(1)? == 1)
    }

    /// `ue(v)`: an unsigned Exp-Golomb code (clause 9.1).
    #[inline]
    pub fn ue(&mut self) -> Result<u32> {
        let peeked = self.peek32();
        let zeros = peeked.leading_zeros();
        if zeros >= 32 {
            // Zero bits to the end of the RBSP are a truncated code; zeros
            // with data behind them are a code no value fits in.
            return Err(if self.bits_left() <= 32 {
                Error::Truncated("an Exp-Golomb code")
            } else {
                Error::Invalid("an Exp-Golomb code longer than 32 bits")
            });
        }
        if zeros < 16 {
            // The whole code is inside the 32 bits already read.
            let len = 2 * zeros + 1;
            if self.pos.saturating_add(len as usize) > self.len_bits {
                return Err(Error::Truncated("an Exp-Golomb code"));
            }
            self.pos += len as usize;
            return Ok((peeked >> (32 - len)) - 1);
        }
        self.skip(zeros as usize + 1)?;
        let rest = self.u(zeros)?;
        let v = (1u64 << zeros) - 1 + u64::from(rest);
        u32::try_from(v).map_err(|_| Error::Invalid("an Exp-Golomb value above 2^32 - 1"))
    }

    /// `ue(v)` with an inclusive upper bound, which most of the
    /// specification's `ue(v)` elements have.
    #[inline]
    pub fn ue_max(&mut self, max: u32, what: &'static str) -> Result<u32> {
        let v = self.ue()?;
        if v > max {
            return Err(Error::Invalid(what));
        }
        Ok(v)
    }

    /// `se(v)`: a signed Exp-Golomb code (clause 9.1.1).
    #[inline]
    pub fn se(&mut self) -> Result<i32> {
        let k = i64::from(self.ue()?);
        let v = if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) };
        i32::try_from(v).map_err(|_| Error::Invalid("a signed Exp-Golomb value out of range"))
    }

    /// `te(v)` with the range `0..=max` (clause 9.1).
    #[inline]
    pub fn te(&mut self, max: u32) -> Result<u32> {
        if max > 1 {
            self.ue_max(max, "a truncated Exp-Golomb value above its range")
        } else if max == 1 {
            Ok(1 - self.u(1)?)
        } else {
            Ok(0)
        }
    }

    /// `more_rbsp_data()` (clause 7.2): whether data remains before the
    /// rbsp_stop_one_bit.
    pub fn more_rbsp_data(&self) -> bool {
        let stop = match self.stop_bit.get() {
            Some(stop) => stop,
            None => {
                // The last set bit of the RBSP is the rbsp_stop_one_bit.
                let stop = self.data.iter().rposition(|&b| b != 0).map(|last| {
                    let trailing = self
                        .data
                        .get(last)
                        .map_or(0, |b| b.trailing_zeros() as usize);
                    last * 8 + 7 - trailing
                });
                self.stop_bit.set(Some(stop));
                stop
            }
        };
        stop.is_some_and(|stop| self.pos < stop)
    }
}

/// The position of the rbsp_stop_one_bit of `data`, the last set bit of
/// the RBSP (clause 7.2), or `None` where no bit is set: data remains
/// before it ([`BitReader::more_rbsp_data`], which finds it as this does,
/// in place, so that its code does not change) while the position is
/// below it.
pub(crate) fn stop_bit(data: &[u8]) -> Option<usize> {
    data.iter().rposition(|&b| b != 0).map(|last| {
        let trailing = data.get(last).map_or(0, |b| b.trailing_zeros() as usize);
        last * 8 + 7 - trailing
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_length_codes() {
        let mut r = BitReader::new(&[0b1011_0010, 0xff, 0x00, 0x12]);
        assert_eq!(r.u(1), Ok(1));
        assert_eq!(r.u(3), Ok(0b011));
        assert_eq!(r.u(4), Ok(0b0010));
        assert!(r.byte_aligned());
        assert_eq!(r.u(16), Ok(0xff00));
        assert_eq!(r.bits_left(), 8);
        assert_eq!(r.u(9), Err(Error::Truncated("a fixed-length code")));
        assert_eq!(r.u(8), Ok(0x12));
        assert_eq!(r.u(1), Err(Error::Truncated("a fixed-length code")));
    }

    #[test]
    fn exp_golomb_codes() {
        // 1, 010, 011, 00100, 00101 (Table 9-2), most significant bit first.
        let data = [0b1010_0110, 0b0100_0010, 0b1000_0000];
        let mut r = BitReader::new(&data);
        let got: Vec<u32> = (0..5).map(|_| r.ue().expect("code")).collect();
        assert_eq!(got, [0, 1, 2, 3, 4]);
        // se(v) maps 0, 1, 2, 3, 4 to 0, 1, -1, 2, -2.
        let mut r = BitReader::new(&data);
        let got: Vec<i32> = (0..5).map(|_| r.se().expect("code")).collect();
        assert_eq!(got, [0, 1, -1, 2, -2]);
    }

    #[test]
    fn a_long_exp_golomb_code_crosses_the_peek_window() {
        // 31 zeros, a one, then 31 bits of value: 2^31 - 1 + 1 = 2^31.
        let mut bits = vec![0u8; 4];
        bits.extend_from_slice(&[0b0000_0001, 0, 0, 0, 0x80]);
        let mut r = BitReader::new(&bits);
        assert_eq!(
            r.ue(),
            Err(Error::Invalid("an Exp-Golomb code longer than 32 bits"))
        );
        // 16 zeros, a one, then 16 bits: the slow path.
        let data = [0, 0, 0b1000_0000, 0b0000_0000, 0b1000_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.ue(), Ok((1 << 16) - 1 + 1));
    }

    #[test]
    fn a_truncated_code_is_an_error_not_a_panic() {
        // 16 zeros and nothing else: the code cannot be complete.
        let data = [0u8, 0];
        let mut r = BitReader::new(&data);
        assert_eq!(r.ue(), Err(Error::Truncated("an Exp-Golomb code")));
        let data = [0b0000_0001u8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.ue(), Err(Error::Truncated("an Exp-Golomb code")));
        let r = BitReader::new(&[]);
        assert_eq!(r.bits_left(), 0);
        assert!(!r.more_rbsp_data());
    }

    #[test]
    fn more_rbsp_data_stops_at_the_stop_bit() {
        // Two bits of data, then rbsp_stop_one_bit and padding.
        let data = [0b1110_0000];
        let mut r = BitReader::new(&data);
        assert!(r.more_rbsp_data());
        r.skip(1).expect("skip");
        assert!(r.more_rbsp_data());
        r.skip(1).expect("skip");
        assert!(!r.more_rbsp_data());
        // An RBSP of zeros has no stop bit.
        assert!(!BitReader::new(&[0, 0]).more_rbsp_data());
    }

    /// The slice loop's more_rbsp_data(), a compare of the position
    /// against [`stop_bit`] found once, against the reader's, at every
    /// position of random RBSPs, those ending in zero bytes or of zeros
    /// alone included.
    #[test]
    fn more_rbsp_data_is_a_compare_against_the_stop_bit() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for case in 0..20_000 {
            let len = (next() % 12) as usize;
            let data: Vec<u8> = (0..len)
                .map(|_| match next() % 4 {
                    0 => 0,
                    1 => 1 << (next() % 8),
                    _ => next() as u8,
                })
                .collect();
            let stop = stop_bit(&data).unwrap_or(0);
            for pos in 0..=len * 8 {
                let mut r = BitReader::new(&data);
                r.skip(pos).expect("inside the data");
                assert_eq!(
                    pos < stop,
                    r.more_rbsp_data(),
                    "case {case}, {data:02x?} at {pos}"
                );
            }
        }
    }

    #[test]
    fn te_reads_one_bit_inverted_when_the_range_is_one() {
        let mut r = BitReader::new(&[0b0100_0000]);
        assert_eq!(r.te(1), Ok(1));
        assert_eq!(r.te(1), Ok(0));
        let mut r = BitReader::new(&[0b0110_0000]);
        assert_eq!(r.te(7), Ok(2));
        assert_eq!(r.te(0), Ok(0));
    }
}
