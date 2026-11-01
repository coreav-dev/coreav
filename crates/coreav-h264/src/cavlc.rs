//! Context-adaptive variable-length coding of residual blocks
//! (clauses 7.3.5.3.2 and 9.2).
#![forbid(unsafe_code)]

use crate::bits::BitReader;
use crate::error::{Error, Result};
use crate::recon::CoeffScale;
use crate::residual::MAX_COEFFS;
use crate::tables as t;

/// The prefix codes of clause 9.2 as tables built at compile time.
///
/// A code word of clause 9.2 is a run of zeros, a one and at most five
/// more bits, or a run of zeros alone. Its slot in a table is found from
/// the count of leading zero bits, at most `CAP`, and the `S` bits after
/// the first one bit: row `min(zeros, CAP)`, `1 << S` slots a row, so a
/// table of codes up to 16 bits long has a hundred slots where one indexed
/// by the next 16 bits has 64 K. The tables of one syntax element share
/// `S` and `CAP`, so that the shifts are constants and a table starts at
/// a multiple of its size; a table whose own rows end sooner repeats its
/// last row, its code word of only zeros or no code word, up to `CAP`.
///
/// A slot holds the code word's length in its low five bits (0 where the
/// bits start no code word) and its values above.
mod lut {
    use super::t;

    /// The values of a slot: its length, `a` and `b`.
    #[inline(always)]
    pub(super) const fn slot(len: u8, a: u8, b: u8) -> u16 {
        len as u16 | (a as u16) << 5 | (b as u16) << 10
    }

    /// The row after a code's last and its widest row, over its entries
    /// (length, code word, a, b).
    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn shape(entries: &[(u8, u32, u8, u8)]) -> (u32, u32) {
        let (mut cap, mut s) = (0, 0);
        let mut i = 0;
        while i < entries.len() {
            let (len, code, _, _) = entries[i];
            let len = len as u32;
            let (row, rest_len) = if code == 0 {
                (len, 0)
            } else {
                let zeros = len - (32 - code.leading_zeros());
                (zeros + 1, len - zeros - 1)
            };
            if row > cap {
                cap = row;
            }
            if rest_len > s {
                s = rest_len;
            }
            i += 1;
        }
        (cap, s)
    }

    /// Writes the slots of `entries` into `lut` from `base`, in rows of
    /// `1 << s` slots up to row `cap`.
    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn fill(lut: &mut [u16], base: usize, entries: &[(u8, u32, u8, u8)], s: u32, cap: u32) {
        let (own_cap, own_s) = shape(entries);
        assert!(own_cap <= cap && own_s <= s, "a code wider than its table");
        let mut i = 0;
        while i < entries.len() {
            let (len, code, a, b) = entries[i];
            let value = slot(len, a, b);
            let len = len as u32;
            let (row, rest, rest_len) = if code == 0 {
                (len, 0, 0)
            } else {
                let zeros = len - (32 - code.leading_zeros());
                let rest_len = len - zeros - 1;
                (zeros, code & ((1 << rest_len) - 1), rest_len)
            };
            let first = base + ((row << s) | (rest << (s - rest_len))) as usize;
            let mut k = 0;
            while k < 1 << (s - rest_len) {
                lut[first + k] = value;
                k += 1;
            }
            i += 1;
        }
        // Past its own last row, a run of zeros reads that row.
        let mut row = own_cap + 1;
        while row <= cap {
            let mut k = 0;
            while k < 1 << s {
                lut[base + (row << s) as usize + k] = lut[base + (own_cap << s) as usize + k];
                k += 1;
            }
            row += 1;
        }
    }

    /// [`fill`] for a code of one value, `b` left 0.
    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn fill_one(lut: &mut [u16], base: usize, entries: &[(u8, u32, u8)], s: u32, cap: u32) {
        let mut wide = [(0u8, 0u32, 0u8, 0u8); 16];
        let mut i = 0;
        while i < entries.len() {
            let (len, code, v) = entries[i];
            wide[i] = (len, code, v, 0);
            i += 1;
        }
        fill(lut, base, wide.split_at(entries.len()).0, s, cap);
    }

    /// The offset of the slot of the code word at the top of `bits` in a
    /// table of rows up to `CAP` of `1 << S` slots.
    #[inline(always)]
    pub(super) fn index<const S: u32, const CAP: u32>(bits: u64) -> usize {
        let row = bits.leading_zeros().min(CAP);
        let rest = ((bits << row) << 1) >> (64 - S);
        ((row << S) as u64 | rest) as usize
    }

    /// coeff_token (Table 9-5): for each nC class but 3, rows up to 15 of
    /// 8 slots, 128 slots at `class * 128`. Class 3 (8 <= nC) is six bits
    /// of fixed length, its slot the six bits, at 384.
    pub(super) const CT_S: u32 = 3;
    pub(super) const CT_CAP: u32 = 15;
    pub(super) const CT_SIZE: usize = 128;
    pub(super) const CT_FIXED: usize = 3;
    pub(super) static COEFF_TOKEN: [u16; 1024] = coeff_token();

    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn coeff_token() -> [u16; 1024] {
        let mut lut = [0u16; 1024];
        let mut class = 0;
        while class < t::COEFF_TOKEN.len() {
            let table = t::COEFF_TOKEN[class];
            if class == CT_FIXED {
                let mut e = 0;
                while e < table.len() {
                    let (len, code, a, b) = table[e];
                    assert!(len == 6 && code < 64, "class 3 is six bits");
                    lut[CT_FIXED * CT_SIZE + code as usize] = slot(len, a, b);
                    e += 1;
                }
            } else {
                fill(&mut lut, class * CT_SIZE, table, CT_S, CT_CAP);
            }
            class += 1;
        }
        lut
    }

    /// total_zeros (Tables 9-7, 9-8 and 9-9 (a)): rows up to 9 of 4
    /// slots, 64 slots a table; the 4x4 blocks' table for tzVlcIndex `i`
    /// at `(i - 1) * 64`, chroma DC's of 4:2:0 at `(15 + i - 1) * 64`.
    pub(super) const TZ_S: u32 = 2;
    pub(super) const TZ_CAP: u32 = 9;
    pub(super) const TZ_SIZE: usize = 64;
    pub(super) const TZ_CHROMA_DC: usize = 15;
    pub(super) static TOTAL_ZEROS: [u16; 2048] = total_zeros();

    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn total_zeros() -> [u16; 2048] {
        let mut lut = [0u16; 2048];
        let mut i = 0;
        while i < t::TOTAL_ZEROS_4X4.len() {
            fill_one(&mut lut, i * TZ_SIZE, t::TOTAL_ZEROS_4X4[i], TZ_S, TZ_CAP);
            i += 1;
        }
        let mut i = 0;
        while i < t::TOTAL_ZEROS_CHROMA_DC_2X2.len() {
            let base = (TZ_CHROMA_DC + i) * TZ_SIZE;
            fill_one(
                &mut lut,
                base,
                t::TOTAL_ZEROS_CHROMA_DC_2X2[i],
                TZ_S,
                TZ_CAP,
            );
            i += 1;
        }
        lut
    }

    /// run_before (Table 9-10) by min(zerosLeft, 7) - 1 and the next three
    /// bits, each slot `len | run_before << 5`. Every code word for
    /// zerosLeft up to 6 is at most three bits; for more, three zeros
    /// start a code word of more zeros (run_before above 6), which the
    /// slot leaves 0 and [`run_past_six`] reads.
    pub(super) static RUN_BEFORE: [u16; 64] = run_before();

    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn run_before() -> [u16; 64] {
        let mut lut = [0u16; 64];
        let mut zl = 0;
        while zl < t::RUN_BEFORE.len() {
            let table = t::RUN_BEFORE[zl];
            let mut bits = 0;
            while bits < 8 {
                let mut e = 0;
                while e < table.len() {
                    let (len, code, run) = table[e];
                    if len <= 3 && code == bits >> (3 - len) {
                        lut[zl * 8 + bits as usize] = slot(len, run, 0);
                    }
                    e += 1;
                }
                bits += 1;
            }
            zl += 1;
        }
        lut
    }

    /// The run_before code words of more than three bits (zerosLeft above
    /// 6): `zeros` zeros and a one for a run of `zeros + 4`, from 3 to
    /// this many zeros.
    pub(super) const RUN_MAX_ZEROS: u32 = run_max_zeros();

    #[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
    const fn run_max_zeros() -> u32 {
        let table = t::RUN_BEFORE[6];
        let mut most = 0;
        let mut e = 0;
        while e < table.len() {
            let (len, code, run) = table[e];
            if len > 3 {
                assert!(
                    code == 1 && run as u32 == len as u32 + 3,
                    "a run of zeros and a one"
                );
                most = len as u32 - 1;
            }
            e += 1;
        }
        most
    }
}

/// Levels beyond this cannot occur in a conforming 8-bit stream
/// (clause 8.5.12.1) and would overflow the scaling arithmetic.
const MAX_LEVEL: i64 = 1 << 15;

/// The bits of a CAVLC slice's data: a window of the next 64 bits of the
/// RBSP, most significant first, refilled with one load when a syntax
/// element might not fit in what is left of it. One window reads the whole
/// slice_data(), from mb_skip_run to the last residual block: the slice
/// loop holds it as a local and moves it into the residual's out-of-line
/// instance and back ([`crate::syntax::CavlcSyntax`]), so that no read
/// goes through memory, and the functions it calls on a cold path take it
/// by value or take its fields.
///
/// Past the end of the RBSP the window reads zeros, and the position is
/// checked against the end once a macroblock and once after each
/// mb_skip_run ([`Window::check`]), so that no read in between needs a
/// bounds check of its own. Zeros start no Exp-Golomb code; a block reads
/// at most about 1,100 bits and a residual at most 25 blocks, so a
/// macroblock cannot run far past the end before that check.
#[derive(Clone, Copy)]
pub(crate) struct Window<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u64,
    /// The bits of `bits` loaded from the position on and not consumed;
    /// past the end of the RBSP they are zeros. A full word, as every
    /// field: the window has no padding, which would make the compiler
    /// copy the field with it as one word, shifted in and out of a
    /// register, where the window is moved.
    left: u64,
}

impl<'a> Window<'a> {
    /// A window from `r`'s position on.
    #[cfg(test)]
    pub(crate) fn new(r: &BitReader<'a>) -> Window<'a> {
        Window::from(r.data(), r.pos())
    }

    /// A window over `data` from bit `pos` on, which must not be past its
    /// end.
    #[inline(always)]
    pub(crate) fn at(data: &'a [u8], pos: usize) -> Result<Window<'a>> {
        if pos > data.len().saturating_mul(8) {
            return Err(Error::Truncated("a syntax element"));
        }
        Ok(Window::from(data, pos))
    }

    #[inline(always)]
    fn from(data: &'a [u8], pos: usize) -> Window<'a> {
        let mut w = Window {
            data,
            pos,
            bits: 0,
            left: 0,
        };
        w.fill();
        w
    }

    /// Bits consumed so far, from the start of the RBSP.
    #[inline(always)]
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Loads the 64 bits from the position on, at least 57 of them past
    /// a byte boundary.
    #[inline(always)]
    fn fill(&mut self) {
        let byte = self.pos / 8;
        let shift = (self.pos % 8) as u32;
        let v = match self
            .data
            .get(byte..byte + 8)
            .and_then(|w| <[u8; 8]>::try_from(w).ok())
        {
            Some(w) => u64::from_be_bytes(w),
            None => (0..8).fold(0u64, |v, i| {
                (v << 8) | u64::from(self.data.get(byte + i).copied().unwrap_or(0))
            }),
        };
        self.bits = v << shift;
        self.left = 64 - u64::from(shift);
    }

    /// Makes at least `n` bits, at most 57, loaded.
    #[inline(always)]
    fn need(&mut self, n: u32) {
        if self.left < u64::from(n) {
            self.fill();
        }
    }

    /// Consumes `n` bits, at most `left`.
    #[inline(always)]
    fn skip(&mut self, n: u32) {
        self.bits <<= n;
        self.left -= u64::from(n);
        self.pos += n as usize;
    }

    /// The next `n` bits, 1 to 32 of them, consumed.
    #[inline(always)]
    fn take(&mut self, n: u32) -> u32 {
        let v = (self.bits >> (64 - n)) as u32;
        self.skip(n);
        v
    }

    /// An error at the current position: a truncated block if the reads
    /// already ran past the end of the RBSP.
    #[inline(always)]
    fn error(&self, what: &'static str) -> Error {
        error_at(self.pos, self.data.len(), what)
    }

    /// `e`, or a truncated syntax element where the reads ran past the end
    /// of the RBSP: what failed then read zeros that are not in the
    /// stream, and the first element past the end is what a reader that
    /// checks every element fails at.
    #[inline(always)]
    pub(crate) fn past_end_or(&self, e: Error) -> Error {
        past_end_or(self.pos, self.data.len(), e)
    }

    /// Fails if the reads ran past the end of the RBSP.
    #[inline(always)]
    pub(crate) fn check(&self) -> Result<()> {
        // pos > 8 * len, without the product: the position is a few
        // macroblocks past the end at most, far from wrapping.
        if self.pos.wrapping_add(7) / 8 > self.data.len() {
            return Err(Error::Truncated("a syntax element"));
        }
        Ok(())
    }

    /// Moves `r` past the bits read, or fails if they ran past the end. A
    /// window behind `r` (one not started from it) fails too.
    #[cfg(test)]
    pub(crate) fn finish(&self, r: &mut BitReader) -> Result<()> {
        let n = self
            .pos
            .checked_sub(r.pos())
            .ok_or(Error::Truncated("a syntax element"))?;
        r.skip(n)
    }

    /// The code word whose slot in `lut` is at `i`, consumed: its slot,
    /// or an error where the bits start no code word.
    #[inline(always)]
    fn code<const LEN: usize>(&mut self, lut: &[u16; LEN], i: usize) -> Result<u16> {
        let slot = lut.get(i & (LEN - 1)).copied().unwrap_or(0);
        let len = u32::from(slot & 31);
        if len == 0 {
            return Err(self.error("a variable-length code that is not in the table"));
        }
        self.skip(len);
        Ok(slot)
    }

    /// `u(n)`, `n` from 1 to 32 (clause 7.2).
    #[inline(always)]
    pub(crate) fn u(&mut self, n: u32) -> u32 {
        self.need(n);
        self.take(n)
    }

    /// `u(1)` as a flag.
    #[inline(always)]
    pub(crate) fn flag(&mut self) -> bool {
        self.u(1) == 1
    }

    /// `ue(v)` (clause 9.1). A code of fewer than 16 leading zeros, every
    /// one a conforming stream's macroblock layer codes but the largest
    /// mvd, is read from the window; a longer one, and the zeros past the
    /// end of the RBSP, by [`BitReader::ue`] on a cold path.
    #[inline(always)]
    pub(crate) fn ue(&mut self) -> Result<u32> {
        self.need(32);
        let zeros = self.bits.leading_zeros();
        if zeros < 16 {
            // The whole code, at most 31 bits, is in the 32 loaded.
            return Ok(self.take(2 * zeros + 1) - 1);
        }
        let (v, pos) = ue_long(self.data, self.pos)?;
        self.pos = pos;
        self.fill();
        Ok(v)
    }

    /// `ue(v)` with an inclusive upper bound.
    #[inline(always)]
    pub(crate) fn ue_max(&mut self, max: u32, what: &'static str) -> Result<u32> {
        let v = self.ue()?;
        if v > max {
            return Err(self.error(what));
        }
        Ok(v)
    }

    /// `se(v)` (clause 9.1.1), as [`Window::ue`] reads its code.
    #[inline(always)]
    pub(crate) fn se(&mut self) -> Result<i32> {
        self.need(32);
        let zeros = self.bits.leading_zeros();
        if zeros < 16 {
            // codeNum + 1, below 2^16: its half is the magnitude, and
            // codeNum is even, the value negative, where it is odd.
            let k = self.take(2 * zeros + 1);
            let (magnitude, negative) = ((k >> 1) as i32, (k & 1) as i32);
            return Ok((magnitude ^ -negative) + negative);
        }
        let (v, pos) = se_long(self.data, self.pos)?;
        self.pos = pos;
        self.fill();
        Ok(v)
    }

    /// `te(v)` with the range `0..=max` (clause 9.1).
    #[inline(always)]
    pub(crate) fn te(&mut self, max: u32) -> Result<u32> {
        if max > 1 {
            self.ue_max(max, "a truncated Exp-Golomb value above its range")
        } else if max == 1 {
            Ok(1 - self.u(1))
        } else {
            Ok(0)
        }
    }

    /// pcm_alignment_zero_bit and the samples of an I_PCM macroblock
    /// (clause 7.3.5), as many as `samples` holds, from the window's
    /// position: the window comes back past them, or where it was if they
    /// fail.
    #[cold]
    #[inline(never)]
    pub(crate) fn pcm(self, samples: &mut [u8]) -> (Window<'a>, Result<()>) {
        match pcm_samples(self.data, self.pos, samples) {
            Ok(end) => (Window::from(self.data, end), Ok(())),
            Err(e) => (self, Err(e)),
        }
    }
}

/// pcm_alignment_zero_bit and the samples of an I_PCM macroblock from bit
/// `pos` of `data`, failing as [`BitReader`] reads fail: the position past
/// them. The samples are whole bytes, after the alignment bits.
fn pcm_samples(data: &[u8], pos: usize, samples: &mut [u8]) -> Result<usize> {
    if pos > data.len().saturating_mul(8) {
        return Err(Error::Truncated("a syntax element"));
    }
    if pos % 8 != 0 {
        // The rest of the byte at `pos`, which is in the data.
        let rest = data.get(pos / 8).map_or(0, |&b| b & (0xff >> (pos % 8)));
        crate::error::require(rest == 0, "pcm_alignment_zero_bit")?;
    }
    let start = pos.div_ceil(8);
    let src = data
        .get(start..start.saturating_add(samples.len()))
        .ok_or(Error::Truncated("a fixed-length code"))?;
    samples.copy_from_slice(src);
    Ok((start + samples.len()) * 8)
}

/// An Exp-Golomb code of 16 or more leading zeros at bit `pos` of `data`,
/// read by [`BitReader::ue`], which fails as the bit reader fails: its
/// value and the position past it.
#[cold]
#[inline(never)]
fn ue_long(data: &[u8], pos: usize) -> Result<(u32, usize)> {
    let mut r = BitReader::new(data);
    r.skip(pos)?;
    let v = r.ue()?;
    Ok((v, r.pos()))
}

/// [`ue_long`] of `se(v)`, read by [`BitReader::se`].
#[cold]
#[inline(never)]
fn se_long(data: &[u8], pos: usize) -> Result<(i32, usize)> {
    let mut r = BitReader::new(data);
    r.skip(pos)?;
    let v = r.se()?;
    Ok((v, r.pos()))
}

/// [`Window::error`] of a window at `pos` over `len` bytes; out of line,
/// with the fields as arguments, so that the window is not taken by
/// reference.
#[cold]
#[inline(never)]
fn error_at(pos: usize, len: usize, what: &'static str) -> Error {
    if pos > len.saturating_mul(8) {
        Error::Truncated("a syntax element")
    } else {
        Error::Invalid(what)
    }
}

/// [`Window::past_end_or`] of a window at `pos` over `len` bytes.
#[cold]
#[inline(never)]
fn past_end_or(pos: usize, len: usize, e: Error) -> Error {
    if pos > len.saturating_mul(8) {
        Error::Truncated("a syntax element")
    } else {
        e
    }
}

/// The coeff_token table of a block whose neighbours give `nc`
/// (clause 9.2.1): -1 for chroma DC of 4:2:0, -2 for chroma DC of 4:2:2.
const fn coeff_token_class(nc: i32) -> usize {
    match nc {
        -1 => 4,
        i32::MIN..=-2 => 5,
        0..=1 => 0,
        2..=3 => 1,
        4..=7 => 2,
        _ => 3,
    }
}

/// Where the coeff_token table of [`coeff_token_class`] starts in
/// [`lut::COEFF_TOKEN`], by `nc + 2` for nC from -2 to 16.
static COEFF_TOKEN_BASE: [u16; 19] = coeff_token_base();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn coeff_token_base() -> [u16; 19] {
    let mut base = [0u16; 19];
    let mut i = 0;
    while i < base.len() {
        base[i] = (coeff_token_class(i as i32 - 2) * lut::CT_SIZE) as u16;
        i += 1;
    }
    base
}

/// A run_before of zerosLeft above 6 whose code word starts with three
/// zeros (run_before 7 to 14), from the window's `bits` at `pos` over
/// `len` bytes: its length and value. The window's fields, not the window,
/// so that it is not taken by reference.
#[cold]
#[inline(never)]
fn run_past_six(bits: u64, pos: usize, len: usize, zeros_left: usize) -> Result<(u32, usize)> {
    let zeros = bits.leading_zeros();
    if zeros > lut::RUN_MAX_ZEROS {
        return Err(error_at(
            pos,
            len,
            "a variable-length code that is not in the table",
        ));
    }
    let run = zeros as usize + 4;
    if run > zeros_left {
        return Err(error_at(
            pos + zeros as usize + 1,
            len,
            "run_before past the zeros left",
        ));
    }
    Ok((zeros + 1, run))
}

/// suffixLength's step up (clause 9.2.2.1), by suffixLength: after a level,
/// suffixLength becomes at least 1 and then grows by one where the level's
/// magnitude is above 3 << (suffixLength - 1), up to 6. At suffixLength 0
/// the comparison is the one of suffixLength 1; at 6 there is no step. Eight
/// entries, so that a suffixLength masked to three bits finds one.
const SUFFIX_STEP: [u32; 8] = [3, 3, 6, 12, 24, 48, u32::MAX, u32::MAX];

/// Decodes the levels of a block into `level`, highest scanning position
/// first: `trailing_ones` of them are +-1, the rest are coded as a prefix
/// and a suffix (clause 9.2.2).
///
/// Below a level_prefix of 14, level_suffix is suffixLength bits, so the
/// prefix, its terminating one and the suffix are read as one field of
/// `prefix + 1 + suffixLength` bits, at most 20, and levelCode is
/// `(prefix << suffixLength) + level_suffix`. A longer prefix goes to
/// [`escaped_level`].
#[inline(always)]
fn levels(w: &mut Window, trailing_ones: usize, total: usize, level: &mut [i32; 16]) -> Result<()> {
    // The sign bits of the trailing ones, at most three, come first.
    w.need(3);
    for slot in level.iter_mut().take(trailing_ones) {
        *slot = 1 - 2 * w.take(1) as i32;
    }
    // suffixLength starts at 1 for blocks with many coefficients.
    let mut suffix_len: u32 = u32::from(total > 10 && trailing_ones < 3);
    // levelCode of the first level after fewer than three trailing ones
    // is 2 more (clause 9.2.2.1).
    let mut bump: i32 = if trailing_ones < 3 { 2 } else { 0 };
    for slot in level.iter_mut().take(total).skip(trailing_ones) {
        w.need(32);
        let prefix = w.bits.leading_zeros();
        let (v, magnitude) = if prefix < 14 {
            let field = w.take(prefix + 1 + suffix_len) as i32;
            // The field is the terminating one above level_suffix.
            let code = ((prefix as i32) << suffix_len) + (field - (1 << suffix_len)) + bump;
            // levelVal: the magnitude is levelCode / 2 + 1 for either
            // parity, negative where levelCode is odd.
            let odd = code & 1;
            let magnitude = (code >> 1) + 1;
            ((magnitude ^ -odd) + odd, magnitude as u32)
        } else {
            let (v, past) = escaped_level(w.data, w.pos, prefix, suffix_len, bump)?;
            w.pos = past;
            w.fill();
            (v, v.unsigned_abs())
        };
        *slot = v;
        bump = 0;
        let step = SUFFIX_STEP
            .get((suffix_len & 7) as usize)
            .copied()
            .unwrap_or(u32::MAX);
        suffix_len = suffix_len.max(1) + u32::from(magnitude > step);
    }
    Ok(())
}

/// A level whose level_prefix, `prefix` bits of zeros not yet consumed, is
/// 14 or more: the three sizes of level_suffix and the terms of levelCode
/// as clause 9.2.2.1 gives them, with its limits, read from bit `pos` of
/// `data`: the level and the position past it. The window's fields, not
/// the window, so that it is not taken by reference.
#[cold]
#[inline(never)]
fn escaped_level(
    data: &[u8],
    pos: usize,
    prefix: u32,
    suffix_len: u32,
    bump: i32,
) -> Result<(i32, usize)> {
    let mut w = Window::from(data, pos);
    if prefix >= 32 {
        return Err(w.error("level_prefix longer than 32 bits"));
    }
    w.skip(prefix + 1);
    let suffix_size = if prefix == 14 && suffix_len == 0 {
        4
    } else if prefix >= 15 {
        prefix - 3
    } else {
        suffix_len
    };
    if suffix_size > 28 {
        return Err(w.error("level_suffix longer than 28 bits"));
    }
    let suffix = if suffix_size > 0 {
        w.need(suffix_size);
        w.take(suffix_size)
    } else {
        0
    };
    // levelCode (Equations 9-5 to 9-9).
    let mut code = (i64::from(prefix.min(15)) << suffix_len) + i64::from(suffix) + i64::from(bump);
    if prefix >= 15 && suffix_len == 0 {
        code += 15;
    }
    if prefix >= 16 {
        code += (1i64 << (prefix - 3)) - 4096;
    }
    let v = if code % 2 == 0 {
        (code + 2) >> 1
    } else {
        (-code - 1) >> 1
    };
    if v.abs() > MAX_LEVEL {
        return Err(w.error("a coefficient level out of range"));
    }
    Ok((v as i32, w.pos))
}

/// [`block_body`], for the tests.
#[cfg(test)]
pub(crate) fn block<const N: usize>(
    r: &mut BitReader,
    nc: i32,
    out: &mut [i32; N],
    scan: &[u8; MAX_COEFFS],
    max: usize,
    scale: &CoeffScale<N>,
) -> Result<u8> {
    block_body(r, nc, out, scan, max, scale)
}

/// residual_block_cavlc() (clause 7.3.5.3.2) of a block of maxNumCoeff
/// `max`, with the predicted number of coefficients `nc` from its
/// neighbours: the level at scanning position `k` is scaled by `scale` and
/// written to `out[scan[k]]`, the rest of `out` is left alone, and
/// TotalCoeff(coeff_token) is returned.
///
/// The clause's steps in one pass: coeff_token, the levels into a
/// 16-entry array, then total_zeros and each run_before, which place the
/// levels from the highest scanning position down, each written where it
/// goes as soon as its position is known.
///
/// Reads from `r` through a window of its own; the residual reads its
/// blocks through one window with [`block_in`].
#[cfg(test)]
pub(crate) fn block_body<const N: usize>(
    r: &mut BitReader,
    nc: i32,
    out: &mut [i32; N],
    scan: &[u8; MAX_COEFFS],
    max: usize,
    scale: &CoeffScale<N>,
) -> Result<u8> {
    let mut w = Window::new(r);
    let n = block_in(&mut w, nc, out, scan, max, scale)?;
    w.finish(r)?;
    Ok(n)
}

/// [`block_body`] from the window `w`, which the caller finishes.
///
/// Inlined into each instance of the hot paths ([`crate::tier::Tier`]),
/// in the macroblock's residual loop.
#[inline(always)]
pub(crate) fn block_in<const N: usize>(
    w: &mut Window,
    nc: i32,
    out: &mut [i32; N],
    scan: &[u8; MAX_COEFFS],
    max: usize,
    scale: &CoeffScale<N>,
) -> Result<u8> {
    const { assert!(N.is_power_of_two(), "a block of a power of two") };
    // coeff_token, at most 16 bits.
    w.need(16);
    let base = COEFF_TOKEN_BASE
        .get((nc.clamp(-2, 16) + 2) as usize)
        .copied()
        .map_or(0, usize::from);
    let i = if base == lut::CT_FIXED * lut::CT_SIZE {
        base | (w.bits >> 58) as usize
    } else {
        base | lut::index::<{ lut::CT_S }, { lut::CT_CAP }>(w.bits)
    };
    let token = w.code(&lut::COEFF_TOKEN, i)?;
    let (trailing_ones, total) = (usize::from(token >> 5 & 31), usize::from(token >> 10));
    if total == 0 {
        return Ok(0);
    }
    if total > max.min(16) {
        return Err(w.error("TotalCoeff above maxNumCoeff"));
    }
    let mut level = [0i32; 16];
    levels(w, trailing_ones, total, &mut level)?;
    let mut zeros_left = if total < max {
        // total_zeros, at most 9 bits, by tzVlcIndex (TotalCoeff).
        w.need(9);
        let table = if max == 4 { lut::TZ_CHROMA_DC } else { 0 } + total - 1;
        let i = (table * lut::TZ_SIZE) | lut::index::<{ lut::TZ_S }, { lut::TZ_CAP }>(w.bits);
        usize::from(w.code(&lut::TOTAL_ZEROS, i)? >> 5 & 31)
    } else {
        0
    };
    if total + zeros_left > max {
        return Err(w.error("total_zeros past the end of the block"));
    }
    // The levels go from the highest scanning position down, each run_before
    // below the one before; `at` stays at least the levels and zeros left
    // below it, so it does not wrap. Positions are below 16 and rasters
    // below N.
    let mut put = |at: usize, v: i32| {
        let raster = usize::from(scan.get(at & (MAX_COEFFS - 1)).copied().unwrap_or(0)) & (N - 1);
        if let Some(slot) = out.get_mut(raster) {
            *slot = scale.apply(raster, v);
        }
    };
    let mut at = total + zeros_left - 1;
    let mut placed = 0;
    while zeros_left > 0 {
        put(at, level.get(placed & 15).copied().unwrap_or(0));
        placed += 1;
        if placed == total {
            break;
        }
        // run_before, at most 11 bits.
        w.need(lut::RUN_MAX_ZEROS + 1);
        let i = (zeros_left.min(7) - 1) << 3 | (w.bits >> 61) as usize;
        let run = match lut::RUN_BEFORE.get(i & 63).copied().unwrap_or(0) {
            0 => {
                let (n, run) = run_past_six(w.bits, w.pos, w.data.len(), zeros_left)?;
                w.skip(n);
                run
            }
            slot => {
                w.skip(u32::from(slot & 31));
                usize::from(slot >> 5 & 31)
            }
        };
        zeros_left -= run;
        at -= run + 1;
    }
    // No zeros left: the rest are at the positions below, one each.
    while placed < total {
        put(at, level.get(placed & 15).copied().unwrap_or(0));
        placed += 1;
        at = at.wrapping_sub(1);
    }
    Ok(total as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scanning position k to raster position k.
    const IN_ORDER: [u8; MAX_COEFFS] = {
        let mut scan = [0u8; MAX_COEFFS];
        let mut k = 0;
        while k < MAX_COEFFS {
            scan[k] = k as u8;
            k += 1;
        }
        scan
    };

    fn decode(data: &[u8], nc: i32, max: usize) -> (Result<u8>, [i32; 16], usize) {
        let mut r = BitReader::new(data);
        let mut out = [0i32; 16];
        let got = block(&mut r, nc, &mut out, &IN_ORDER, max, &CoeffScale::IDENTITY);
        (got, out, r.pos())
    }

    #[test]
    fn an_empty_block_reads_one_code_word() {
        // coeff_token for nC class 0 with TotalCoeff 0 is the single bit 1.
        let (got, out, pos) = decode(&[0b1000_0000u8], 0, 16);
        assert_eq!(got, Ok(0));
        assert_eq!(pos, 1);
        assert_eq!(out, [0; 16]);
    }

    #[test]
    fn a_block_with_one_trailing_one() {
        // nC class 0: coeff_token "01" is TrailingOnes 1, TotalCoeff 1;
        // then the sign bit (1 = negative), then total_zeros for
        // tzVlcIndex 1 ("1" = 0 zeros).
        let (got, out, pos) = decode(&[0b0111_0000u8], 0, 16);
        assert_eq!(got, Ok(1));
        assert_eq!(pos, 4);
        assert_eq!(out.first(), Some(&-1));
        assert_eq!(out.get(1), Some(&0));
    }

    #[test]
    fn an_unknown_code_word_is_an_error() {
        // 16 zero bits are not a coeff_token of class 0.
        assert!(decode(&[0u8; 4], 0, 16).0.is_err());
    }

    #[test]
    fn a_block_past_the_end_is_truncated() {
        // coeff_token "0001 01" (TrailingOnes 0, TotalCoeff 1 is
        // "0001 01" in class 0) ends past one byte of zeros and ones.
        assert_eq!(
            decode(&[0b0000_0101u8], 0, 16).0,
            Err(Error::Truncated("a syntax element"))
        );
    }

    /// The code word of `entries` that starts `bits`, as (length, a, b),
    /// the way a table indexed by every bit of the longest code word finds
    /// it; length 0 for none.
    fn flat(entries: &[(u8, u32, u8, u8)], bits: u64) -> (u8, u8, u8) {
        entries
            .iter()
            .find(|&&(len, code, _, _)| bits >> (64 - u32::from(len)) == u64::from(code))
            .map_or((0, 0, 0), |&(len, _, a, b)| (len, a, b))
    }

    fn unpack(slot: u16) -> (u8, u8, u8) {
        (
            (slot & 31) as u8,
            (slot >> 5 & 31) as u8,
            (slot >> 10) as u8,
        )
    }

    fn one(table: &[(u8, u32, u8)]) -> Vec<(u8, u32, u8, u8)> {
        table
            .iter()
            .map(|&(len, code, v)| (len, code, v, 0))
            .collect()
    }

    /// Every bit pattern a code word of at most 16 bits can start, with
    /// the bits after them all zeros or all ones.
    fn patterns() -> impl Iterator<Item = u64> {
        (0..1u64 << 16).flat_map(|top| [top << 48, top << 48 | ((1u64 << 48) - 1)])
    }

    #[test]
    fn every_code_table_finds_what_a_flat_table_finds() {
        for (class, entries) in t::COEFF_TOKEN.iter().enumerate() {
            let base = class * lut::CT_SIZE;
            for bits in patterns() {
                let i = if class == lut::CT_FIXED {
                    base | (bits >> 58) as usize
                } else {
                    base | lut::index::<{ lut::CT_S }, { lut::CT_CAP }>(bits)
                };
                assert!(i < base + lut::CT_SIZE);
                let got = unpack(lut::COEFF_TOKEN[i]);
                assert_eq!(got, flat(entries, bits), "class {class} {bits:#x}");
            }
        }
        let tables = t::TOTAL_ZEROS_4X4.iter().enumerate().chain(
            t::TOTAL_ZEROS_CHROMA_DC_2X2
                .iter()
                .enumerate()
                .map(|(i, table)| (lut::TZ_CHROMA_DC + i, table)),
        );
        for (table, entries) in tables {
            let entries = one(entries);
            for bits in patterns() {
                let i = lut::index::<{ lut::TZ_S }, { lut::TZ_CAP }>(bits);
                assert!(i < lut::TZ_SIZE);
                let got = unpack(lut::TOTAL_ZEROS[(table * lut::TZ_SIZE) | i]);
                assert_eq!(got, flat(&entries, bits), "total_zeros {table} {bits:#x}");
            }
        }
    }

    #[test]
    fn run_before_is_three_bits_or_a_run_of_zeros() {
        for (zl, entries) in t::RUN_BEFORE.iter().enumerate() {
            let entries = one(entries);
            for bits in patterns() {
                let want = flat(&entries, bits);
                let slot = lut::RUN_BEFORE[zl << 3 | (bits >> 61) as usize];
                if slot == 0 {
                    // Only zerosLeft above 6 with three zeros goes past the
                    // table, to a run of zeros and a one.
                    assert_eq!(zl, 6);
                    let zeros = bits.leading_zeros();
                    let len = zeros as u8 + 1;
                    let got = if zeros <= lut::RUN_MAX_ZEROS {
                        (len, len + 3, 0)
                    } else {
                        (0, 0, 0)
                    };
                    assert_eq!(got, want, "{bits:#x}");
                } else {
                    assert_eq!(unpack(slot), want, "zerosLeft {} {bits:#x}", zl + 1);
                    // A run_before from the table never passes zerosLeft.
                    assert!(usize::from(want.1) <= zl + 1);
                }
            }
        }
        assert_eq!(lut::RUN_MAX_ZEROS, 10);
    }

    #[test]
    fn the_coeff_token_bases_follow_the_classes() {
        for nc in -40..40 {
            let base = COEFF_TOKEN_BASE[(nc.clamp(-2, 16) + 2) as usize];
            assert_eq!(
                usize::from(base),
                coeff_token_class(nc) * lut::CT_SIZE,
                "nC {nc}"
            );
        }
    }

    /// The clause's steps one at a time, as the decoder had them before
    /// they were fused: the reference the fused block is tested against.
    mod clause {
        use super::super::{MAX_LEVEL, coeff_token_class};
        use crate::bits::BitReader;
        use crate::error::{Error, Result};
        use crate::tables as t;

        fn read(table: &[(u8, u32, u8, u8)], r: &mut BitReader) -> Result<(u8, u8)> {
            let bits = u64::from(r.peek32()) << 32;
            let (len, a, b) = table
                .iter()
                .find(|&&(len, code, _, _)| bits >> (64 - u32::from(len)) == u64::from(code))
                .map(|&(len, _, a, b)| (len, a, b))
                .ok_or(Error::Invalid(
                    "a variable-length code that is not in the table",
                ))?;
            r.skip(usize::from(len))?;
            Ok((a, b))
        }

        fn read_one(table: &[(u8, u32, u8)], r: &mut BitReader) -> Result<u8> {
            let table: Vec<_> = table.iter().map(|&(l, c, v)| (l, c, v, 0)).collect();
            Ok(read(&table, r)?.0)
        }

        fn levels(
            r: &mut BitReader,
            trailing_ones: usize,
            total: usize,
            out: &mut [i32; 16],
        ) -> Result<()> {
            let mut suffix_len: u32 = u32::from(total > 10 && trailing_ones < 3);
            for (i, level) in out.iter_mut().enumerate().take(total) {
                if i < trailing_ones {
                    *level = if r.u(1)? == 1 { -1 } else { 1 };
                    continue;
                }
                let prefix = r.peek32().leading_zeros();
                if prefix >= 32 {
                    return Err(Error::Invalid("level_prefix longer than 32 bits"));
                }
                r.skip(prefix as usize + 1)?;
                let suffix_size = if prefix == 14 && suffix_len == 0 {
                    4
                } else if prefix >= 15 {
                    prefix - 3
                } else {
                    suffix_len
                };
                if suffix_size > 28 {
                    return Err(Error::Invalid("level_suffix longer than 28 bits"));
                }
                let suffix = if suffix_size > 0 {
                    r.u(suffix_size)?
                } else {
                    0
                };
                let mut code = (i64::from(prefix.min(15)) << suffix_len) + i64::from(suffix);
                if prefix >= 15 && suffix_len == 0 {
                    code += 15;
                }
                if prefix >= 16 {
                    code += (1i64 << (prefix - 3)) - 4096;
                }
                if i == trailing_ones && trailing_ones < 3 {
                    code += 2;
                }
                let v = if code % 2 == 0 {
                    (code + 2) >> 1
                } else {
                    (-code - 1) >> 1
                };
                if v.abs() > MAX_LEVEL {
                    return Err(Error::Invalid("a coefficient level out of range"));
                }
                *level = v as i32;
                if suffix_len == 0 {
                    suffix_len = 1;
                }
                if v.abs() > (3i64 << (suffix_len - 1)) && suffix_len < 6 {
                    suffix_len += 1;
                }
            }
            Ok(())
        }

        /// residual_block_cavlc() into `coeff`, in scanning order.
        pub(super) fn residual_block(r: &mut BitReader, nc: i32, coeff: &mut [i32]) -> Result<u8> {
            let max = coeff.len();
            let (trailing_ones, total) = read(t::COEFF_TOKEN[coeff_token_class(nc)], r)?;
            coeff.fill(0);
            let total = usize::from(total);
            if total == 0 {
                return Ok(0);
            }
            if total > max {
                return Err(Error::Invalid("TotalCoeff above maxNumCoeff"));
            }
            let mut level = [0i32; 16];
            levels(r, usize::from(trailing_ones), total, &mut level)?;
            let mut zeros_left = if total < max {
                let table = if max == 4 {
                    t::TOTAL_ZEROS_CHROMA_DC_2X2[total - 1]
                } else {
                    t::TOTAL_ZEROS_4X4[total - 1]
                };
                usize::from(read_one(table, r)?)
            } else {
                0
            };
            if total + zeros_left > max {
                return Err(Error::Invalid("total_zeros past the end of the block"));
            }
            let mut runs = [0usize; 16];
            for run in runs.iter_mut().take(total - 1) {
                if zeros_left == 0 {
                    break;
                }
                let value = usize::from(read_one(t::RUN_BEFORE[zeros_left.min(7) - 1], r)?);
                if value > zeros_left {
                    return Err(Error::Invalid("run_before past the zeros left"));
                }
                *run = value;
                zeros_left -= value;
            }
            runs[total - 1] = zeros_left;
            let mut at = 0;
            for i in (0..total).rev() {
                at += if i + 1 < total { runs[i] + 1 } else { runs[i] };
                coeff[at] = level[i];
            }
            Ok(total as u8)
        }
    }

    /// Bits, most significant first, as bytes, padded with `pad` bytes of
    /// zeros.
    fn pack(bits: &[bool], pad: usize) -> Vec<u8> {
        let mut data = vec![0u8; bits.len().div_ceil(8) + pad];
        for (i, &b) in bits.iter().enumerate() {
            if b {
                data[i / 8] |= 0x80 >> (i % 8);
            }
        }
        data
    }

    /// `n` bits of `v`, most significant first.
    fn push(bits: &mut Vec<bool>, v: u64, n: u32) {
        bits.extend((0..n).rev().map(|i| v >> i & 1 == 1));
    }

    /// The window's Exp-Golomb and fixed-length codes against the bit
    /// reader's, on random data with codes of every length, those of 16 or
    /// more leading zeros (the cold path) and codes cut by the end of the
    /// data included: where the reader reads a value, the window reads the
    /// same and stops at the same position; where the reader fails, the
    /// window fails the same way, or has read past the end, which its check
    /// reports as a truncated element, where the reader's error is a
    /// truncation too.
    #[test]
    fn window_codes_read_as_the_bit_reader_reads_them() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut long, mut cut, mut same_error) = (0, 0, 0);
        for case in 0..100_000 {
            let len = 1 + (next() % 24) as usize;
            let mut data: Vec<u8> = (0..len)
                .map(|_| match next() {
                    // A third of the bytes zero, for long codes.
                    v if v % 3 == 0 => 0,
                    v => (v >> 8) as u8,
                })
                .collect();
            // Codes of 14 to 18 leading zeros at the start of a tenth.
            if case % 10 == 0 {
                let zeros = 14 + (next() % 5) as usize;
                let mut bits = vec![false; zeros];
                bits.push(true);
                push(&mut bits, next(), zeros as u32);
                let packed = pack(&bits, 0);
                for (d, p) in data.iter_mut().zip(packed.iter()) {
                    *d = *p;
                }
            }
            let start = (next() % 9) as usize;
            let mut r = BitReader::new(&data);
            if r.skip(start).is_err() {
                continue;
            }
            let mut w = Window::at(&data, start).expect("a window");
            for element in 0..16 {
                let at = format!("case {case}, element {element}");
                let (got, want) = match next() % 6 {
                    0 => (w.ue().map(i64::from), r.ue().map(i64::from)),
                    1 => (w.se().map(i64::from), r.se().map(i64::from)),
                    2 => {
                        let max = (next() % 4) as u32;
                        (w.te(max).map(i64::from), r.te(max).map(i64::from))
                    }
                    3 => {
                        let max = (next() % 64) as u32;
                        (
                            w.ue_max(max, "a bound").map(i64::from),
                            r.ue_max(max, "a bound").map(i64::from),
                        )
                    }
                    4 => (Ok(i64::from(w.flag())), r.flag().map(i64::from)),
                    _ => {
                        let n = 1 + (next() % 32) as u32;
                        (Ok(i64::from(w.u(n))), r.u(n).map(i64::from))
                    }
                };
                long += usize::from(want.is_ok_and(|v| v.unsigned_abs() >= 32_767));
                match want {
                    Ok(_) => {
                        assert_eq!(got, want, "{at}");
                        assert_eq!(w.pos(), r.pos(), "{at}");
                        assert_eq!(w.check(), Ok(()), "{at}");
                    }
                    Err(e) => {
                        if got == Err(e) {
                            same_error += 1;
                        } else {
                            // Read past the end: what the slice loop
                            // reports is a truncation, as the reader's.
                            cut += 1;
                            assert!(w.pos() > data.len() * 8, "{at}: {got:?} for {e:?}");
                            assert!(matches!(e, Error::Truncated(_)), "{at}: {e:?}");
                            let reported = match got {
                                Err(g) => w.past_end_or(g),
                                Ok(_) => w.check().expect_err("past the end"),
                            };
                            assert_eq!(reported, Error::Truncated("a syntax element"), "{at}");
                        }
                        break;
                    }
                }
            }
        }
        assert!(long > 1_000, "{long} codes of 16 or more leading zeros");
        assert!(cut > 1_000, "{cut} codes cut by the end");
        assert!(same_error > 1_000, "{same_error} errors as the reader's");
    }

    /// The I_PCM read of the window (alignment bits, then the samples as
    /// whole bytes) against the bit reader reading them one by one, at
    /// every position of random RBSPs, those too short included: the same
    /// samples and position, or the same error with the window where it
    /// was.
    #[test]
    fn pcm_reads_as_the_bit_reader_reads() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut read, mut failed) = (0, 0);
        for case in 0..400 {
            let len = 370 + (next() % 30) as usize;
            let data: Vec<u8> = (0..len)
                .map(|_| if next() % 2 == 0 { 0 } else { next() as u8 })
                .collect();
            for pos in (0..=len * 8 + 1).step_by(1 + (next() % 13) as usize) {
                let mut r = BitReader::new(&data);
                let mut want = [0u8; 384];
                let reader = r.skip(pos).and_then(|()| {
                    while !r.byte_aligned() {
                        crate::error::require(r.u(1)? == 0, "pcm_alignment_zero_bit")?;
                    }
                    for s in want.iter_mut() {
                        *s = r.u(8)? as u8;
                    }
                    Ok(())
                });
                let w = Window::from(&data, pos);
                let mut got = [0u8; 384];
                let (after, window) = w.pcm(&mut got);
                assert_eq!(window, reader, "case {case} at {pos}");
                if reader.is_ok() {
                    read += 1;
                    assert_eq!(got, want, "case {case} at {pos}");
                    assert_eq!(after.pos, r.pos(), "case {case} at {pos}");
                } else {
                    failed += 1;
                    assert_eq!(after.pos, pos, "case {case} at {pos}");
                }
            }
        }
        assert!(
            read > 1_000 && failed > 1_000,
            "{read} read, {failed} failed"
        );
    }

    /// The levels of a block of `total` coefficients and `trailing_ones`
    /// trailing ones read from `bits`, and the bits consumed.
    fn read_levels(
        bits: &[bool],
        trailing_ones: usize,
        total: usize,
    ) -> (Result<[i32; 16]>, usize) {
        let data = pack(bits, 8);
        let r = BitReader::new(&data);
        let mut w = Window::new(&r);
        let mut level = [0i32; 16];
        let got = levels(&mut w, trailing_ones, total, &mut level).map(|()| level);
        (got, w.pos)
    }

    #[test]
    fn escaped_levels_take_the_clause_s_suffix_sizes_and_terms() {
        // Three trailing ones (signs 000), then one level at suffixLength
        // 0 with no +2: level_prefix zeros, a one, level_suffix.
        let fourth = |prefix: u32, suffix: u64, size: u32| {
            let mut bits = vec![false; 3 + prefix as usize];
            bits.push(true);
            push(&mut bits, suffix, size);
            let (got, pos) = read_levels(&bits, 3, 4);
            (got.map(|l| l[3]), pos == bits.len())
        };
        // Below 14, suffixLength 0: levelCode is the prefix.
        assert_eq!(fourth(13, 0, 0), (Ok(-7), true));
        // 14 at suffixLength 0: a 4-bit suffix.
        assert_eq!(fourth(14, 0, 4), (Ok(8), true));
        assert_eq!(fourth(14, 15, 4), (Ok(-15), true));
        // 15: a 12-bit suffix, levelCode 15 + suffix + 15.
        assert_eq!(fourth(15, 0, 12), (Ok(16), true));
        assert_eq!(fourth(15, 4095, 12), (Ok(-2063), true));
        // 16: a 13-bit suffix and (1 << 13) - 4096 more.
        assert_eq!(fourth(16, 0, 13), (Ok(2064), true));
        // 19: the largest magnitude, 1 << 15, either sign, and one past it.
        assert_eq!(fourth(19, 4_064, 16), (Ok(32_768), true));
        assert_eq!(fourth(19, 4_065, 16), (Ok(-32_768), true));
        assert_eq!(
            fourth(19, 4_066, 16).0,
            Err(Error::Invalid("a coefficient level out of range"))
        );
        // 32 zeros are no level_prefix.
        assert_eq!(
            fourth(32, 0, 0).0,
            Err(Error::Invalid("level_prefix longer than 32 bits"))
        );
        // 14 at suffixLength 1 (eleven coefficients, no trailing ones):
        // suffixLength bits, and the first level's +2; ones after it
        // code the other ten levels.
        let mut bits = vec![false; 14];
        bits.extend([true; 66]);
        let (got, _) = read_levels(&bits, 0, 11);
        assert_eq!(got.map(|l| l[0]), Ok(-16));
    }

    #[test]
    fn long_level_prefixes_decode_what_the_clause_decodes() {
        // Random bits rarely reach a prefix of 14: blocks of class 0 whose
        // levels have prefixes of 0 to 40 zeros, against the clause.
        let mut seed = 0x6a09_e667_f3bc_c909u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let tokens = t::COEFF_TOKEN[0];
        let mut escaped = 0;
        for _ in 0..100_000 {
            let &(len, code, trailing_ones, total) =
                &tokens[(next() % tokens.len() as u64) as usize];
            let mut bits = Vec::new();
            push(&mut bits, u64::from(code), u32::from(len));
            push(&mut bits, next(), u32::from(trailing_ones));
            let mut long = false;
            for _ in trailing_ones..total {
                let prefix = if next() % 2 == 0 {
                    next() % 14
                } else {
                    14 + next() % 27
                };
                long |= prefix >= 14;
                bits.extend(std::iter::repeat_n(false, prefix as usize));
                bits.push(true);
                push(&mut bits, next(), (next() % 29) as u32);
            }
            push(&mut bits, next(), 64);
            let data = pack(&bits, 0);
            let mut r = BitReader::new(&data);
            let mut want = [0i32; 16];
            let expected = clause::residual_block(&mut r, 0, &mut want);
            let (got, out, pos) = decode(&data, 0, 16);
            match expected {
                Ok(n) => {
                    assert_eq!(got, Ok(n), "{data:02x?}");
                    assert_eq!(out, want, "{data:02x?}");
                    assert_eq!(pos, r.pos(), "{data:02x?}");
                    escaped += usize::from(long);
                }
                Err(e) => assert_eq!(got.err(), Some(e), "{data:02x?}"),
            }
        }
        assert!(escaped > 5_000, "{escaped} blocks with an escaped level");
    }

    #[test]
    fn an_8x8_block_takes_its_interleaved_positions_and_scale() {
        // CAVLC codes an 8x8 block as four 4x4 lists, each written to every
        // fourth position of the 8x8 scan (clause 7.3.5.3.1).
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut factors = [0i32; 64];
        for (i, f) in factors.iter_mut().enumerate() {
            *f = 16 + i as i32;
        }
        // QP 20 is below the 8x8 regime, so the scaling rounds and shifts.
        let scale = CoeffScale::<64>::new(&factors, 20);
        let mut decoded = 0;
        for case in 0..20_000 {
            let data: Vec<u8> = (0..12).map(|_| (next() as u8) | (next() as u8)).collect();
            let i4 = case % 4;
            let scan = crate::mb::SCAN_8X8_CAVLC[i4];
            let nc = [0, 2, 4, 8][(next() % 4) as usize];
            let mut r = BitReader::new(&data);
            let mut coeff = [0i32; 16];
            let expected = clause::residual_block(&mut r, nc, &mut coeff);
            let mut want = [0i32; 64];
            for (k, &v) in coeff.iter().enumerate() {
                if v != 0 {
                    let at = usize::from(scan[k]);
                    want[at] = scale.apply(at, v);
                }
            }
            let mut got_r = BitReader::new(&data);
            let mut out = [0i32; 64];
            let got = block(&mut got_r, nc, &mut out, &scan, 16, &scale);
            match expected {
                Ok(n) => {
                    assert_eq!(got, Ok(n), "{data:02x?} nC {nc} block {i4}");
                    assert_eq!(out, want, "{data:02x?} nC {nc} block {i4}");
                    assert_eq!(got_r.pos(), r.pos());
                    decoded += usize::from(n > 0);
                }
                Err(_) => assert!(got.is_err(), "{data:02x?} nC {nc} block {i4}"),
            }
        }
        assert!(decoded > 5_000, "{decoded} blocks with coefficients");
    }

    #[test]
    fn the_fused_block_decodes_what_the_clause_decodes() {
        // Random bits, biased towards ones so that short code words, and so
        // whole blocks, are common; every nC class and maxNumCoeff.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut decoded = 0;
        for case in 0..200_000 {
            let len = 1 + (next() % 24) as usize;
            let data: Vec<u8> = (0..len)
                .map(|_| {
                    let (x, y) = (next() as u8, next() as u8);
                    if case % 2 == 0 { x | y } else { x }
                })
                .collect();
            let nc = [-1, 0, 1, 2, 3, 4, 7, 8, 16][(next() % 9) as usize];
            let max = if nc == -1 {
                4
            } else {
                [15, 16][(next() % 2) as usize]
            };
            let mut r = BitReader::new(&data);
            let mut want = [0i32; 16];
            let expected = clause::residual_block(&mut r, nc, &mut want[..max]);
            let (got, out, pos) = decode(&data, nc, max);
            match expected {
                Ok(n) => {
                    assert_eq!(got, Ok(n), "{data:02x?} nC {nc} max {max}");
                    assert_eq!(out, want, "{data:02x?} nC {nc} max {max}");
                    assert_eq!(pos, r.pos(), "{data:02x?} nC {nc} max {max}");
                    decoded += usize::from(n > 0);
                }
                // The fused block checks the end of the RBSP once, so a
                // truncated block names no element; the kind is the same.
                Err(e) => {
                    let got = got.err().map(|g| core::mem::discriminant(&g));
                    assert_eq!(
                        got,
                        Some(core::mem::discriminant(&e)),
                        "{data:02x?} nC {nc} max {max}"
                    );
                }
            }
        }
        assert!(decoded > 50_000, "{decoded} blocks with coefficients");
    }

    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    #[test]
    fn the_v3_instance_decodes_what_the_baseline_decodes() {
        // Differential test: the CAVLC residual block of the v3 instance
        // against the baseline's, through the reader each slice uses, on
        // random bits, every block shape, nC class and maxNumCoeff, errors
        // included: the same result, block and position.
        use crate::fast::V3;
        use crate::residual::Blk;
        use crate::syntax::{CavlcSyntax, Syntax};
        use crate::tier::{Base, Tier};

        /// One block of shape `N` read by both readers; false where both
        /// failed.
        fn both<const N: usize>(
            v3: V3,
            a: &mut CavlcSyntax<'_, V3, true>,
            b: &mut CavlcSyntax<'_, Base, true>,
            nc: i32,
            max: usize,
            at: &str,
        ) -> bool {
            let (mut x, mut y) = ([0i32; N], [0i32; N]);
            let got = v3.enter(|| {
                a.block(
                    Blk::Luma4x4(0),
                    nc,
                    &mut x,
                    &IN_ORDER,
                    max,
                    &CoeffScale::IDENTITY,
                )
            });
            let want = b.block(
                Blk::Luma4x4(0),
                nc,
                &mut y,
                &IN_ORDER,
                max,
                &CoeffScale::IDENTITY,
            );
            assert_eq!((got, x), (want, y), "{at}");
            assert_eq!(a.w.pos, b.w.pos, "{at}");
            got.is_ok()
        }

        let Some(v3) = V3::detect() else {
            return;
        };
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut compared = 0;
        for case in 0..20_000 {
            let len = 1 + (next() % 24) as usize;
            let data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // One residual a case: the blocks below read through one
            // window, checked at the end.
            let w = Window::at(&data, 0).expect("a window");
            let mut a = CavlcSyntax::new(w, v3);
            let mut b = CavlcSyntax::new(w, Base);
            for block in 0..8 {
                let at = format!("case {case}, block {block}");
                // The shapes the macroblock layer reads: chroma DC (nC -1),
                // the 4x4 blocks, and the 4x4 blocks of an 8x8 one.
                let ok = match next() % 3 {
                    0 => both::<4>(v3, &mut a, &mut b, -1, 4, &at),
                    1 => {
                        let nc = (next() % 18) as i32;
                        let max = [15, 16][(next() % 2) as usize];
                        both::<16>(v3, &mut a, &mut b, nc, max, &at)
                    }
                    _ => both::<64>(v3, &mut a, &mut b, (next() % 18) as i32, 16, &at),
                };
                compared += 1;
                if !ok {
                    break;
                }
            }
            let at = format!("case {case}, end");
            assert_eq!(a.check(), b.check(), "{at}");
            assert_eq!(a.w.pos, b.w.pos, "{at}");
        }
        assert!(compared > 50_000, "{compared} blocks");
    }
}
