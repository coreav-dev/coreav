//! Context-adaptive binary arithmetic coding (clause 9.3): the arithmetic
//! decoding engine, the binarizations and the residual block decoder.
//!
//! The context index increments that depend on neighbouring macroblocks are
//! derived by the macroblock layer and passed in, so this module holds no
//! picture state.
//!
//! Everything here is safe Rust. The table indices
//! are masked to the tables' own power-of-two sizes, which both keeps them
//! in range and lets the compiler drop the bounds check.
#![forbid(unsafe_code)]

use crate::bits::BitReader;
use crate::error::{Error, Result};
use crate::recon::CoeffScale;
use crate::residual::MAX_COEFFS;
use crate::tables as t;
use crate::tier::{Base, Tier};

/// Contexts of clause 9.3.1.1, rounded up to a power of two so that a
/// masked index is always in range.
const CONTEXTS: usize = 1024;
const CTX_MASK: usize = CONTEXTS - 1;

/// Bytes the engine may read past the end of the slice data, as zeros,
/// before the data counts as truncated. A conforming slice needs at most
/// four: the engine reads ahead by up to 39 bits, and the last bins of the
/// last macroblock may sit in the last bytes.
const OVERRUN: usize = 6;

/// Unconsumed bits the engine keeps loaded between operations. No operation
/// consumes more than seven, so none has to check before it consumes, only
/// after.
const RESERVE: u32 = 8;

/// rangeTabLPS (Table 9-44) by `state << 3 | codIRange >> 6`. codIRange
/// is 256 to 510 between decisions, so `codIRange >> 6` is 4 to 7, which is
/// qCodIRangeIdx plus 4: indexing by it directly saves masking it, at the
/// price of rows 0 to 3 of each state being unused. Rows exist for every
/// byte a state could hold, so that the index needs no mask and no bounds
/// check.
const LPS: [u8; 2048] = lps_table();

/// The state after a decision by `state << 1 | binVal` (Table 9-45, with the
/// valMPS switch of clause 9.3.3.2.1.1), for every byte a state could hold.
/// Indexing by the bin rather than by whether it was the less probable one
/// lets a decision compute the bin once, for this and for its caller.
const NEXT: [u8; 512] = next_table();

/// rangeLPS renormalized, by the index of [`LPS`]: `rangeLPS << n | n << 9`
/// for the shift `n` that brings it to 256 or more (clause 9.3.3.2.2).
const LPS_NORM: [u16; 2048] = lps_norm_table();

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn lps_norm_table() -> [u16; 2048] {
    let mut out = [0u16; 2048];
    let mut i = 0;
    while i < 2048 {
        let lps = LPS[i] as u16;
        if lps > 0 {
            let n = lps.leading_zeros() as u16 - 7;
            out[i] = (lps << n) | (n << 9);
        }
        i += 1;
    }
    out
}

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn lps_table() -> [u8; 2048] {
    let mut out = [0u8; 2048];
    let mut i = 0;
    while i < 2048 {
        let (state, q) = ((i >> 3) & 127, i & 7);
        if q >= 4 {
            out[i] = t::RANGE_TAB_LPS_BY_STATE[(state << 2) | (q - 4)];
        }
        i += 1;
    }
    out
}

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn next_table() -> [u8; 512] {
    let mut out = [0u8; 512];
    let mut i = 0;
    while i < 512 {
        let (state, bin) = ((i >> 1) & 127, i & 1);
        // The bin was the less probable symbol where it differs from valMPS.
        let lps = (bin ^ (state & 1)) << 7;
        out[i] = t::NEXT_STATE[state | lps];
        i += 1;
    }
    out
}

/// The last bytes of the slice data, fewer than four, as a big-endian word
/// padded with zeros. Out of line: it is taken only once the slice data
/// has run out.
#[cold]
#[inline(never)]
fn last_word(rest: &[u8]) -> u32 {
    let byte = |k: usize| rest.get(k).copied().unwrap_or(0);
    u32::from_be_bytes([byte(0), byte(1), byte(2), byte(3)])
}

/// The slice data the engine has not loaded yet, and how many bytes it
/// loaded past the end as zeros. Apart from the engine's registers so that
/// a hot loop holds three values for the engine rather than six: this is
/// read once every 32 bits, by [`Source::load`], out of line.
pub(crate) struct Source<'a> {
    rest: &'a [u8],
    past_end: u32,
}

impl Source<'_> {
    /// The next four bytes, big-endian. Bytes past the end of the data read
    /// as zero; [`Cabac::check`] reports a slice that ran further past the
    /// end than it can. Out of line, so that the caller's loop keeps its
    /// registers for the engine and does not hold the data.
    #[inline(never)]
    fn load(&mut self) -> u32 {
        match self.rest.split_first_chunk::<4>() {
            Some((word, rest)) => {
                self.rest = rest;
                u32::from_be_bytes(*word)
            }
            None => {
                let word = last_word(self.rest);
                self.past_end += 4 - self.rest.len() as u32;
                self.rest = &[];
                word
            }
        }
    }
}

/// The arithmetic decoding engine (clauses 9.3.1.2 and 9.3.3.2).
///
/// codIOffset is kept scaled inside a 64-bit window of the slice data:
/// `value >> bits` is codIOffset, and the `bits` low bits are data that has
/// been loaded but not consumed. Between operations `bits` is at least
/// [`RESERVE`] and at most 39. `Copy`, so that a hot loop can keep it in
/// registers and write it back once; the data it loads from is a
/// [`Source`] passed to each operation.
#[derive(Clone, Copy)]
pub(crate) struct Engine {
    pub(crate) value: u64,
    pub(crate) bits: u32,
    pub(crate) range: u32,
}

impl Engine {
    /// Loads four bytes, which fit: below [`RESERVE`] unconsumed bits,
    /// `value` holds less than 17 bits.
    #[inline(always)]
    fn refill(&mut self, src: &mut Source) {
        self.value = (self.value << 32) | u64::from(src.load());
        self.bits += 32;
    }

    /// Consumes `n` bits into codIOffset (`n` is at most 7).
    #[inline(always)]
    fn consume(&mut self, n: u32, src: &mut Source) {
        self.bits -= n;
        self.reserve(src);
    }

    /// Refills where fewer than [`RESERVE`] bits are left, after an
    /// operation consumed its bits.
    #[inline(always)]
    pub(crate) fn reserve(&mut self, src: &mut Source) {
        if self.bits < RESERVE {
            self.refill(src);
        }
    }

    /// codIOffset.
    #[inline(always)]
    fn offset(&self) -> u32 {
        (self.value >> self.bits) as u32
    }

    /// Renormalization (clause 9.3.3.2.2).
    #[inline(always)]
    fn renormalize(&mut self, src: &mut Source) {
        if self.range < 256 {
            let n = self.range.leading_zeros() - 23;
            self.range <<= n;
            self.consume(n, src);
        }
    }

    /// DecodeDecision (clause 9.3.3.2.1) against the context state `state`.
    ///
    /// Written without a branch on the decoded bin: which of the two
    /// sub-intervals was taken is a mask, because the bin is unpredictable
    /// and a mispredicted branch costs more than the arithmetic.
    ///
    /// The twin of the site `decision` in [`crate::fast`], which the v3
    /// instance runs instead ([`Tier::decision`]).
    #[inline(always)]
    pub(crate) fn decision(&mut self, state: &mut u8, src: &mut Source) -> u32 {
        let s = usize::from(*state);
        let at = ((s << 3) | (self.range >> 6) as usize) & 2047;
        let lps = u32::from(LPS.get(at).copied().unwrap_or(0));
        let lps_norm = u32::from(LPS_NORM.get(at).copied().unwrap_or(0));
        let mps_range = self.range - lps;
        let scaled = u64::from(mps_range) << self.bits;
        let is_lps = self.value >= scaled;
        self.value -= scaled & 0u64.wrapping_sub(u64::from(is_lps));
        let mask = 0u32.wrapping_sub(u32::from(is_lps));
        let bin = (s as u32 & 1) ^ u32::from(is_lps);
        *state = NEXT.get((s << 1) | bin as usize).copied().unwrap_or(0);
        // Renormalization, with no count of leading zeros on the chain from
        // one bin to the next. After the less probable symbol the
        // renormalized range and the shift are properties of the table
        // entry; after the more probable one codIRange - rangeLPS is at
        // least 128 (Table 9-44 against codIRange's quarter), so the shift
        // is 0 or 1.
        let mps_shift = u32::from(mps_range < 256);
        let mps_norm = mps_range << mps_shift;
        self.range = mps_norm ^ ((mps_norm ^ (lps_norm & 511)) & mask);
        let n = mps_shift ^ ((mps_shift ^ (lps_norm >> 9)) & mask);
        self.consume(n, src);
        bin
    }

    /// DecodeBypass (clause 9.3.3.2.3), without a branch on the bin.
    #[inline(always)]
    fn bypass(&mut self, src: &mut Source) -> u32 {
        self.bits -= 1;
        let scaled = u64::from(self.range) << self.bits;
        let bin = u64::from(self.value >= scaled);
        self.value -= scaled & 0u64.wrapping_sub(bin);
        if self.bits < RESERVE {
            self.refill(src);
        }
        bin as u32
    }

    /// The Exp-Golomb suffix of a UEGk binarization, in bypass bins
    /// (clause 9.3.2.3).
    #[inline]
    fn exp_golomb_bypass(&mut self, mut k: u32, src: &mut Source) -> Result<u32> {
        let mut v: u32 = 0;
        while self.bypass(src) == 1 {
            v = v
                .checked_add(1 << k)
                .ok_or(Error::Invalid("an Exp-Golomb suffix out of range"))?;
            k += 1;
            if k >= 31 {
                return Err(Error::Invalid("an Exp-Golomb suffix longer than 31 bits"));
            }
        }
        while k > 0 {
            k -= 1;
            v += self.bypass(src) << k;
        }
        Ok(v)
    }
}

/// Which context a bin of a table-driven binarization uses.
#[derive(Clone, Copy)]
pub(crate) enum Bin {
    /// A regular bin with this context index.
    Ctx(usize),
    /// The bin decoded by DecodeTerminate (clause 9.3.3.2.4).
    Terminate,
}

/// The longest bin string of a table-driven binarization (mb_type of a B
/// slice), and the number of codes of that length.
const MAX_BINS: usize = 7;
const MAX_STRING: usize = 1 << MAX_BINS;

/// A prefix-free set of bin strings as a lookup: by the string's length
/// less one and then by its bins, the value it stands for, or
/// [`NO_STRING`]. One load per bin, instead of searching the list of
/// strings after every bin.
pub(crate) type BinStrings = [[i8; MAX_STRING]; MAX_BINS];

/// No bin string of that length has those bins.
const NO_STRING: i8 = i8::MIN;

/// The lookup of a list of (value, length, bins) bin strings, as
/// `tables.rs` holds them.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
pub(crate) const fn bin_strings(list: &[(i8, u8, u8)]) -> BinStrings {
    let mut out = [[NO_STRING; MAX_STRING]; MAX_BINS];
    let mut i = 0;
    while i < list.len() {
        let (value, len, code) = list[i];
        out[len as usize - 1][code as usize] = value;
        i += 1;
    }
    out
}

/// mb_type in I slices and as the suffix in P and B slices (Table 9-36),
/// mb_type in P and B slices, and sub_mb_type in P and B slices
/// (Tables 9-37 and 9-38).
pub(crate) const BINS_MB_TYPE_I: BinStrings = bin_strings(t::BINS_MB_TYPE_I);
pub(crate) const BINS_MB_TYPE_P: BinStrings = bin_strings(t::BINS_MB_TYPE_P);
pub(crate) const BINS_MB_TYPE_B: BinStrings = bin_strings(t::BINS_MB_TYPE_B);
pub(crate) const BINS_SUB_MB_TYPE_P: BinStrings = bin_strings(t::BINS_SUB_MB_TYPE_P);
pub(crate) const BINS_SUB_MB_TYPE_B: BinStrings = bin_strings(t::BINS_SUB_MB_TYPE_B);

/// What a residual block's ctxBlockCat fixes for the whole block: the
/// ctxIdx of coded_block_flag at increment 0, and the first ctxIdx of
/// significant_coeff_flag, last_significant_coeff_flag and
/// coeff_abs_level_minus1, each ctxIdxOffset plus ctxBlockCatOffset
/// (Tables 9-34 and 9-40). Resolved once per category, at compile time,
/// instead of per block from three tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Category {
    pub(crate) cbf: u16,
    pub(crate) sig: u16,
    pub(crate) last: u16,
    pub(crate) abs: u16,
}

/// The contexts of ctxBlockCat 0 to 5 in a frame macroblock.
pub(crate) const CATEGORIES: [Category; 6] = [
    category(0),
    category(1),
    category(2),
    category(3),
    category(4),
    // An 8x8 block has contexts of its own, and no coded_block_flag in a
    // 4:2:0 picture.
    Category {
        cbf: 0,
        sig: 402,
        last: 417,
        abs: 426,
    },
];

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn category(cat: usize) -> Category {
    Category {
        cbf: 85 + t::BLOCK_CAT_OFFSET_CODED_BLOCK_FLAG[cat],
        sig: 105 + t::BLOCK_CAT_OFFSET_SIGNIFICANT_COEFF_FLAG[cat],
        last: 166 + t::BLOCK_CAT_OFFSET_LAST_SIGNIFICANT_COEFF_FLAG[cat],
        abs: 227 + t::BLOCK_CAT_OFFSET_COEFF_ABS_LEVEL_MINUS1[cat],
    }
}

/// The CABAC decoder of one slice.
pub(crate) struct Cabac<'a> {
    /// The slice data, from its first byte.
    data: &'a [u8],
    engine: Engine,
    src: Source<'a>,
    /// `pStateIdx << 1 | valMPS` per context index.
    ctx: [u8; CONTEXTS],
}

impl<'a> Cabac<'a> {
    /// Initializes the contexts (clause 9.3.1.1) and the engine
    /// (clause 9.3.1.2) at the start of slice_data().
    ///
    /// `init_slot` is 0 for I and SI slices and `1 + cabac_init_idc`
    /// otherwise. `t` is the instance of the hot paths it runs in.
    pub(crate) fn new_in<T: Tier>(
        t: T,
        data: &'a [u8],
        bit_offset: usize,
        init_slot: usize,
        slice_qp: i32,
    ) -> Result<Cabac<'a>> {
        t.enter(
            #[inline(always)]
            || Cabac::new_body(data, bit_offset, init_slot, slice_qp),
        )
    }

    /// [`Cabac::new_in`] in the baseline instance, for the tests.
    #[cfg(test)]
    pub(crate) fn new(
        data: &'a [u8],
        bit_offset: usize,
        init_slot: usize,
        slice_qp: i32,
    ) -> Result<Cabac<'a>> {
        Cabac::new_body(data, bit_offset, init_slot, slice_qp)
    }

    #[inline(always)]
    fn new_body(
        data: &'a [u8],
        bit_offset: usize,
        init_slot: usize,
        slice_qp: i32,
    ) -> Result<Cabac<'a>> {
        let mut r = BitReader::new(data);
        r.skip(bit_offset)?;
        while !r.byte_aligned() {
            if r.u(1)? != 1 {
                return Err(Error::Invalid("cabac_alignment_one_bit"));
            }
        }
        let mut c = Cabac {
            data,
            engine: Engine {
                value: 0,
                bits: 0,
                range: 510,
            },
            src: Source {
                rest: data.get(r.pos() / 8..).unwrap_or_default(),
                past_end: 0,
            },
            ctx: [0; CONTEXTS],
        };
        let qp = slice_qp.clamp(0, 51);
        let init = t::CABAC_INIT_MN
            .get(init_slot.min(3))
            .ok_or(Error::Invalid("cabac_init_idc"))?;
        for (state, &(m, n)) in c.ctx.iter_mut().zip(init.iter()) {
            // preCtxState (Equations 9-5 and 9-6).
            let pre = (((i32::from(m) * qp) >> 4) + i32::from(n)).clamp(1, 126);
            *state = if pre <= 63 {
                ((63 - pre) << 1) as u8
            } else {
                (((pre - 64) << 1) | 1) as u8
            };
        }
        c.start_engine()?;
        Ok(c)
    }

    /// Fails once the engine has loaded more than [`OVERRUN`] bytes past
    /// the end of the slice data.
    ///
    /// Bins are decoded without a check of their own; the macroblock layer
    /// calls this after each block and each macroblock, before anything
    /// decoded is used, which rejects the same slices as checking every
    /// load would.
    #[inline]
    pub(crate) fn check(&self) -> Result<()> {
        if self.src.past_end as usize > OVERRUN {
            return Err(Error::Truncated("the slice data"));
        }
        Ok(())
    }

    /// Initialization of the decoding engine (clause 9.3.1.2).
    fn start_engine(&mut self) -> Result<()> {
        self.engine.range = 510;
        self.engine.value = 0;
        self.engine.bits = 0;
        self.engine.refill(&mut self.src);
        self.check()?;
        self.engine.bits -= 9;
        if self.engine.offset() >= 510 {
            return Err(Error::Invalid("codIOffset at the start of a slice"));
        }
        Ok(())
    }

    /// A bit reader positioned after the last bit the engine consumed, for
    /// the samples of an I_PCM macroblock (clause 9.3.1).
    pub(crate) fn pcm_reader(&self) -> BitReader<'a> {
        let loaded = self.data.len() - self.src.rest.len() + self.src.past_end as usize;
        let consumed = loaded * 8 - self.engine.bits as usize;
        let mut r = BitReader::new(self.data);
        // The position is inside the data: the engine has read that far.
        let _ = r.skip(consumed.min(self.data.len() * 8));
        r
    }

    /// Restarts the engine after the samples of an I_PCM macroblock.
    pub(crate) fn resume(&mut self, r: &BitReader<'a>) -> Result<()> {
        if !r.byte_aligned() {
            return Err(Error::Invalid("I_PCM samples that end off a byte"));
        }
        self.src.rest = self.data.get(r.pos() / 8..).unwrap_or_default();
        self.src.past_end = 0;
        self.start_engine()
    }

    /// Decodes one regular bin with the context `ctx_idx`, in the instance
    /// `t`.
    #[inline(always)]
    pub(crate) fn decision<T: Tier>(&mut self, t: T, ctx_idx: usize) -> u32 {
        let Cabac {
            engine, ctx, src, ..
        } = self;
        match ctx.get_mut(ctx_idx & CTX_MASK) {
            Some(state) => t.decision(engine, state, src),
            None => 0,
        }
    }

    /// Decodes one bypass bin.
    #[inline(always)]
    pub(crate) fn bypass(&mut self) -> u32 {
        self.engine.bypass(&mut self.src)
    }

    /// DecodeTerminate (clause 9.3.3.2.4): end_of_slice_flag and the
    /// I_PCM bin.
    pub(crate) fn terminate(&mut self) -> u32 {
        let e = &mut self.engine;
        e.range -= 2;
        let scaled = u64::from(e.range) << e.bits;
        if e.value >= scaled {
            1
        } else {
            e.renormalize(&mut self.src);
            0
        }
    }

    /// A unary or truncated unary binarization (clause 9.3.2.1 and 9.3.2.2)
    /// whose bin `i` uses the context `ctx(i)`.
    pub(crate) fn unary<T: Tier>(
        &mut self,
        t: T,
        c_max: Option<u32>,
        ctx: impl Fn(u32) -> usize,
    ) -> Result<u32> {
        let mut v = 0;
        loop {
            if Some(v) == c_max {
                return Ok(v);
            }
            if self.decision(t, ctx(v)) == 0 {
                return Ok(v);
            }
            v += 1;
            if v > 1 << 16 {
                return Err(Error::Invalid("a unary value out of range"));
            }
        }
    }

    /// Decodes a value from a prefix-free set of bin strings, given as a
    /// [`BinStrings`] lookup; `select` gives the context of bin `bin_idx`
    /// from the bins decoded so far (most significant first).
    #[inline]
    pub(crate) fn table<T: Tier>(
        &mut self,
        t: T,
        strings: &BinStrings,
        select: impl Fn(usize, u32) -> Bin,
    ) -> Result<i8> {
        let mut bins = 0u32;
        for (bin_idx, by_code) in strings.iter().enumerate() {
            let b = match select(bin_idx, bins) {
                Bin::Ctx(c) => self.decision(t, c),
                Bin::Terminate => self.terminate(),
            };
            bins = (bins << 1) | b;
            match by_code.get(bins as usize & (MAX_STRING - 1)) {
                Some(&v) if v != NO_STRING => return Ok(v),
                _ => {}
            }
        }
        Err(Error::Invalid("a bin string that is in no table"))
    }

    /// An Exp-Golomb suffix in bypass bins.
    #[inline(always)]
    pub(crate) fn exp_golomb_bypass(&mut self, k: u32) -> Result<u32> {
        self.engine.exp_golomb_bypass(k, &mut self.src)
    }

    /// Both components of mvd_lX, each a UEG3 binarization with uCoff 9
    /// and a sign bit (clause 9.3.2.3), the horizontal one at ctxIdxOffset
    /// 40 and the vertical one at 47; `inc0` holds each one's
    /// neighbour-derived increment of the first bin. One call into the
    /// instance `t` reads both.
    pub(crate) fn mvd<T: Tier>(&mut self, t: T, inc0: [usize; 2]) -> Result<[i32; 2]> {
        t.enter(
            #[inline(always)]
            || {
                Ok([
                    self.mvd_body(t, 40, inc0[0])?,
                    self.mvd_body(t, 47, inc0[1])?,
                ])
            },
        )
    }

    /// One component of [`Cabac::mvd`]. The prefix, truncated unary with
    /// cMax 9, is read by its own loop: bin 0 at `base + inc0`, bin `b`
    /// from 1 to 8 at `base + min(b + 2, 6)` (clause 9.3.3.1.1.7).
    #[inline(always)]
    fn mvd_body<T: Tier>(&mut self, t: T, base: usize, inc0: usize) -> Result<i32> {
        if self.decision(t, base + inc0) == 0 {
            return Ok(0);
        }
        let mut v = 1;
        while v < 9 && self.decision(t, base + (v as usize + 2).min(6)) == 1 {
            v += 1;
        }
        if v == 9 {
            v = self.exp_golomb_bypass(3)?.saturating_add(9);
        }
        let v = i32::try_from(v).map_err(|_| Error::Invalid("a motion vector difference"))?;
        Ok(if self.bypass() == 1 { -v } else { v })
    }

    /// The significance map and the levels of one residual block, after its
    /// coded_block_flag (clauses 7.3.5.3.3, 9.3.2.3 and 9.3.3.1.3), written
    /// straight into the block: the level at scanning position `k` goes to
    /// `out[scan[k]]`, and the rest of `out` is left as it was, which the
    /// macroblock layer keeps at zero. Returns the number of non-zero
    /// levels.
    ///
    /// The block's shape follows from `N`: 4 is the chroma DC block of a
    /// 4:2:0 picture, 64 an 8x8 luma block, and 16 every 4x4 block. `max`
    /// is maxNumCoeff.
    ///
    /// One call a coded block, in the slice's instance of the hot paths
    /// ([`residual`]); the engine is loaded from `self` and stored back once
    /// for both halves.
    ///
    /// Inlined, as the call it makes is out of line, so that the caller's
    /// test of coded_block_flag is all an empty block costs.
    #[inline(always)]
    pub(crate) fn residual<const N: usize, T: Tier>(
        &mut self,
        t: T,
        cat: Category,
        max: usize,
        out: &mut [i32; N],
        scan: &[u8; MAX_COEFFS],
        scale: &CoeffScale<N>,
    ) -> Result<u8> {
        t.residual::<N>(self, cat, max, out, scan, scale)
    }
}

/// [`Cabac::residual`] in the baseline instance.
#[inline(never)]
pub(crate) fn residual<const N: usize>(
    c: &mut Cabac,
    cat: Category,
    max: usize,
    out: &mut [i32; N],
    scan: &[u8; MAX_COEFFS],
    scale: &CoeffScale<N>,
) -> Result<u8> {
    residual_body::<N, _>(Base, c, cat, max, out, scan, scale)
}

/// [`Cabac::residual`]'s body, which each instance of the hot paths
/// compiles ([`Tier`]).
#[inline(always)]
pub(crate) fn residual_body<const N: usize, T: Tier>(
    t: T,
    c: &mut Cabac,
    cat: Category,
    max: usize,
    out: &mut [i32; N],
    scan: &[u8; MAX_COEFFS],
    scale: &CoeffScale<N>,
) -> Result<u8> {
    // The last scanning position, whose significance is not coded.
    let end = max.clamp(1, N) - 1;
    let map = significance_map::<N, T>(t, c, usize::from(cat.sig), end);
    let found = match levels::<N, T>(t, c, usize::from(cat.abs), map, out, scan, scale) {
        Ok(found) => found,
        Err(err) => return failed(out, err),
    };
    match c.check() {
        Ok(()) => Ok(found),
        Err(err) => failed(out, err),
    }
}

/// A residual block that failed, whether its levels did or the data ran
/// out under them. The macroblock layer clears only the blocks it was told
/// hold levels, and a block that failed is not one of them, so whatever
/// was written into it goes here. Out of line, as a path taken only on an
/// error.
#[cold]
#[inline(never)]
fn failed<const N: usize>(out: &mut [i32; N], err: Error) -> Result<u8> {
    *out = [0; N];
    Err(err)
}

/// significant_coeff_flag and last_significant_coeff_flag of a block whose
/// last scanning position is `end` (clause 7.3.5.3.3), as a map with a bit
/// set for each scanning position that holds a level. `sig` is the first
/// context of significant_coeff_flag; see [`Cabac::residual`] for `N`.
#[inline(always)]
fn significance_map<const N: usize, T: Tier>(t: T, c: &mut Cabac, sig: usize, end: usize) -> u64 {
    let Cabac {
        engine, src, ctx, ..
    } = c;
    // ctxIdxInc by scanning position (clause 9.3.3.1.3): the position
    // itself in a 4x4 block, a table in an 8x8 one, and the position capped
    // at 2 in the chroma DC block, where NumC8x8 is 1.
    let sig_inc = |i: usize| -> usize {
        match N {
            4 => i.min(2),
            64 => usize::from(t::SIG_CTX_INC_8X8_FRAME.get(i).copied().unwrap_or(0)),
            _ => i,
        }
    };
    let last_inc = |i: usize| -> usize {
        match N {
            4 => i.min(2),
            64 => usize::from(t::LAST_CTX_INC_8X8.get(i).copied().unwrap_or(0)),
            _ => i,
        }
    };
    // last_significant_coeff_flag's contexts sit at a fixed distance from
    // significant_coeff_flag's: 61 for ctxBlockCat 0 to 4, whose
    // ctxBlockCatOffsets are the same for both, and 15 for an 8x8 block.
    // Deriving one base from the other keeps a register free.
    let last = sig + if N == 64 { 15 } else { 61 };
    let mut e = *engine;
    let mut map = 0u64;
    let mut i = 0usize;
    while i < end {
        let significant = match ctx.get_mut((sig + sig_inc(i)) & CTX_MASK) {
            Some(state) => t.decision(&mut e, state, src),
            None => 0,
        };
        if significant == 1 {
            map |= 1 << (i & 63);
            let is_last = match ctx.get_mut((last + last_inc(i)) & CTX_MASK) {
                Some(state) => t.decision(&mut e, state, src),
                None => 0,
            };
            if is_last == 1 {
                break;
            }
        }
        i += 1;
    }
    if i == end {
        // No level before the last position was flagged as the last one,
        // so the last position holds one.
        map |= 1 << (end & 63);
    }
    *engine = e;
    map
}

/// The contexts of coeff_abs_level_minus1 depend on how many levels of
/// the block, counting from the last, were 1 and how many above 1
/// (clause 9.3.3.1.3). Only a little of that history matters: until a level
/// above 1 the number of 1s up to 3, after it only the number above 1 up
/// to 4. So it is one of eight states — 0 to 3 for none above 1 and that
/// many 1s, 4 to 7 for one to four or more above 1, kept in one register.
///
/// ctxIdxInc of the first bin by state.
const LEVEL_INC_FIRST: [u8; 8] = [1, 2, 3, 4, 0, 0, 0, 0];

/// ctxIdxInc of the other bins by state: 5 + Min(4, numDecodAbsLevelGt1).
/// The chroma DC block caps numDecodAbsLevelGt1 at 3 instead, which
/// [`levels`] applies by capping the increment at 8.
const LEVEL_INC_REST: [u8; 8] = [5, 5, 5, 5, 6, 7, 8, 9];

/// The state after a level of 1, and after a level above 1.
const LEVEL_AFTER_ONE: [u8; 8] = [1, 2, 3, 3, 4, 5, 6, 7];
const LEVEL_AFTER_MORE: [u8; 8] = [4, 4, 4, 4, 5, 6, 7, 7];

/// coeff_abs_level_minus1 and coeff_sign_flag of each level in `map`, from
/// the last to the first (clause 7.3.5.3.3), written to `out[scan[k]]` for
/// scanning position `k`, scaled by `scale`. coeff_abs_level_minus1 is UEG0 with uCoff 14
/// (Table 9-34); `abs` is its first context. Returns the number of levels.
#[inline(always)]
fn levels<const N: usize, T: Tier>(
    t: T,
    c: &mut Cabac,
    abs: usize,
    mut map: u64,
    out: &mut [i32; N],
    scan: &[u8; MAX_COEFFS],
    scale: &CoeffScale<N>,
) -> Result<u8> {
    let Cabac {
        engine, src, ctx, ..
    } = c;
    let found = map.count_ones() as u8;
    // numDecodAbsLevelGt1 is capped at 3 in the chroma DC block.
    let rest_max = if N == 4 { 8 } else { 9 };
    let mut e = *engine;
    let mut history = 0usize;
    while map != 0 {
        let at = |table: &[u8; 8]| usize::from(table.get(history & 7).copied().unwrap_or(0));
        let first = match ctx.get_mut((abs + at(&LEVEL_INC_FIRST)) & CTX_MASK) {
            Some(state) => t.decision(&mut e, state, src),
            None => 0,
        };
        let mut level = 1u32;
        if first == 1 {
            let inc = at(&LEVEL_INC_REST).min(rest_max);
            level = 2;
            if let Some(state) = ctx.get_mut((abs + inc) & CTX_MASK) {
                while level < 15 && t.decision(&mut e, state, src) == 1 {
                    level += 1;
                }
            }
            if level == 15 {
                // The Exp-Golomb suffix, rare enough to be out of line; the
                // engine goes by value so that its address never escapes
                // and it can stay in registers here.
                let (after, suffix) = exp_golomb_bypass_out_of_line(e, src);
                e = after;
                match suffix {
                    Ok(suffix) if suffix < (1 << 15) - 14 => level += suffix,
                    Ok(_) => {
                        return Err(Error::Invalid("a coefficient level out of range"));
                    }
                    Err(err) => return Err(err),
                }
            }
            history = at(&LEVEL_AFTER_MORE);
        } else {
            history = at(&LEVEL_AFTER_ONE);
        }
        // coeff_sign_flag, applied without a branch: a sign is as
        // unpredictable as a bit can be.
        let negative = e.bypass(src) as i32;
        let value = (level as i32 ^ -negative) + negative;
        let pos = 63 - map.leading_zeros() as usize;
        map ^= 1 << pos;
        let to = usize::from(scan.get(pos & (MAX_COEFFS - 1)).copied().unwrap_or(0));
        let at = to & (N - 1);
        if let Some(slot) = out.get_mut(at) {
            *slot = scale.apply(at, value);
        }
    }
    *engine = e;
    Ok(found)
}

/// The UEG0 suffix of coeff_abs_level_minus1, for [`levels`].
#[cold]
#[inline(never)]
fn exp_golomb_bypass_out_of_line(mut e: Engine, src: &mut Source) -> (Engine, Result<u32>) {
    let suffix = e.exp_golomb_bypass(0, src);
    (e, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tier::Base;

    /// A small deterministic generator, so that the tests need no crate.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// The decoding engine as clause 9.3.3.2 writes it: a 9-bit codIOffset
    /// read one bit at a time. Bits past the end of the data read as zero.
    struct Reference<'a> {
        data: &'a [u8],
        pos: usize,
        range: u32,
        offset: u32,
    }

    impl Reference<'_> {
        fn bit(&mut self) -> u32 {
            let byte = self.data.get(self.pos / 8).copied().unwrap_or(0);
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            self.pos += 1;
            u32::from(bit)
        }

        fn renormalize(&mut self) {
            while self.range < 256 {
                self.range <<= 1;
                self.offset = (self.offset << 1) | self.bit();
            }
        }

        fn decision(&mut self, state: &mut u8) -> u32 {
            let s = usize::from(*state);
            let q = ((self.range >> 6) & 3) as usize;
            let lps = u32::from(t::RANGE_TAB_LPS_BY_STATE[(s << 2) | q]);
            self.range -= lps;
            let mps = u32::from(*state & 1);
            let bin = if self.offset >= self.range {
                self.offset -= self.range;
                self.range = lps;
                *state = t::NEXT_STATE[s | 128];
                mps ^ 1
            } else {
                *state = t::NEXT_STATE[s];
                mps
            };
            self.renormalize();
            bin
        }

        fn bypass(&mut self) -> u32 {
            self.offset = (self.offset << 1) | self.bit();
            if self.offset >= self.range {
                self.offset -= self.range;
                1
            } else {
                0
            }
        }

        fn terminate(&mut self) -> u32 {
            self.range -= 2;
            if self.offset >= self.range {
                1
            } else {
                self.renormalize();
                0
            }
        }
    }

    #[test]
    fn the_engine_decodes_what_the_clause_decodes() {
        engine_against_the_clause(Base);
    }

    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    #[test]
    fn the_site_decision_decodes_what_the_clause_decodes() {
        if let Some(v3) = crate::fast::V3::detect() {
            engine_against_the_clause(v3);
        }
    }

    /// Random operations of the engine, its decisions in the instance `t`,
    /// against [`Reference`].
    fn engine_against_the_clause<T: Tier>(t: T) {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for case in 0..400 {
            let len = 1 + rng.below(600) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let slot = rng.below(4) as usize;
            let qp = rng.below(52) as i32;
            let Ok(mut c) = Cabac::new(&data, 0, slot, qp) else {
                // codIOffset 510 or 511: not a slice a decoder can start.
                continue;
            };
            let mut ctx = c.ctx;
            let mut reference = Reference {
                data: &data,
                pos: 9,
                range: 510,
                offset: (u32::from(data[0]) << 1)
                    | u32::from(data.get(1).copied().unwrap_or(0) >> 7),
            };
            // Runs to a few bytes past the end, where both read zeros.
            for op in 0..len * 10 {
                let (got, want) = match rng.below(40) {
                    0 => (c.terminate(), reference.terminate()),
                    1..=6 => (c.bypass(), reference.bypass()),
                    _ => {
                        let i = rng.below(CONTEXTS as u64) as usize;
                        (c.decision(t, i), reference.decision(&mut ctx[i]))
                    }
                };
                assert_eq!(got, want, "case {case}, operation {op}");
                if got == 1 && reference.range < 256 {
                    // A terminating bin of 1 ends the slice data.
                    break;
                }
                if reference.pos > (len + OVERRUN) * 8 {
                    break;
                }
            }
            assert_eq!(c.ctx, ctx, "case {case}: the context states");
        }
    }

    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    #[test]
    fn the_site_decision_decides_as_its_twin_in_every_state() {
        // Differential test: the site against Engine::decision for every
        // state byte and every codIRange the engine holds between
        // decisions, at random codIOffsets and loaded bits, including the
        // refill that follows: the same bin, state, engine and data left.
        let Some(v3) = crate::fast::V3::detect() else {
            return;
        };
        let mut rng = Rng(0xbb67_ae85_84ca_a73b);
        let data: Vec<u8> = (0..64).map(|_| rng.next() as u8).collect();
        for s in 0..=255u8 {
            for range in 256..=510u32 {
                for _ in 0..4 {
                    let bits = 8 + rng.below(32) as u32;
                    let offset = rng.below(u64::from(range));
                    let low = rng.next() & ((1u64 << bits) - 1);
                    let e = Engine {
                        value: (offset << bits) | low,
                        bits,
                        range,
                    };
                    let skip = rng.below(8) as usize;
                    let start = || Source {
                        rest: &data[skip..],
                        past_end: 0,
                    };
                    let (mut ea, mut sa, mut src_a) = (e, s, start());
                    let (mut eb, mut sb, mut src_b) = (e, s, start());
                    let got = v3.decision(&mut ea, &mut sa, &mut src_a);
                    let want = eb.decision(&mut sb, &mut src_b);
                    let at = format!("state {s}, codIRange {range}, bits {bits}");
                    assert_eq!(got, want, "{at}");
                    assert_eq!(sa, sb, "{at}");
                    assert_eq!(
                        (ea.value, ea.bits, ea.range),
                        (eb.value, eb.bits, eb.range),
                        "{at}"
                    );
                    assert_eq!(src_a.rest.len(), src_b.rest.len(), "{at}");
                }
            }
        }
    }

    #[test]
    fn residual_blocks_decode_what_the_clause_decodes() {
        // The significance map and the levels against a direct reading of
        // clause 7.3.5.3.3 on the reference engine, for every block shape.
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for case in 0..600 {
            let len = 64 + rng.below(400) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let Ok(mut c) = Cabac::new(&data, 0, rng.below(4) as usize, rng.below(52) as i32)
            else {
                continue;
            };
            let mut ctx = c.ctx;
            let mut reference = Reference {
                data: &data,
                pos: 9,
                range: 510,
                offset: (u32::from(data[0]) << 1) | u32::from(data[1] >> 7),
            };
            for block in 0..20 {
                let cat = [0, 1, 2, 3, 4, 5][rng.below(6) as usize];
                let contexts = CATEGORIES[cat];
                let max = [16, 15, 16, 4, 15, 64][cat];
                let want = reference_block(&mut reference, &mut ctx, cat, contexts, max);
                let got = match cat {
                    3 => {
                        let mut out = [0i32; 4];
                        let identity: [u8; MAX_COEFFS] = std::array::from_fn(|k| k as u8);
                        c.residual(
                            Base,
                            contexts,
                            max,
                            &mut out,
                            &identity,
                            &CoeffScale::IDENTITY,
                        )
                        .map(|n| (n, out.to_vec()))
                    }
                    5 => {
                        let mut out = [0i32; 64];
                        let identity: [u8; MAX_COEFFS] = std::array::from_fn(|k| k as u8);
                        c.residual(
                            Base,
                            contexts,
                            max,
                            &mut out,
                            &identity,
                            &CoeffScale::IDENTITY,
                        )
                        .map(|n| (n, out.to_vec()))
                    }
                    _ => {
                        let mut out = [0i32; 16];
                        let identity: [u8; MAX_COEFFS] = std::array::from_fn(|k| k as u8 & 15);
                        c.residual(
                            Base,
                            contexts,
                            max,
                            &mut out,
                            &identity,
                            &CoeffScale::IDENTITY,
                        )
                        .map(|n| (n, out.to_vec()))
                    }
                };
                match (got, want) {
                    (Ok(got), Some((n, mut levels))) => {
                        // maxNumCoeff 15 still fills a 16-entry block.
                        levels.resize(got.1.len(), 0);
                        assert_eq!(got, (n, levels), "case {case}, block {block}");
                    }
                    // Random data runs past the end or out of range; both
                    // sides stop there.
                    _ => break,
                }
                assert_eq!(c.ctx, ctx, "case {case}, block {block}");
            }
        }
    }

    /// residual_block_cabac() of clause 7.3.5.3.3 after coded_block_flag,
    /// with the context increments of clause 9.3.3.1.3 written out:
    /// (the number of levels, the levels by scanning position).
    fn reference_block(
        e: &mut Reference,
        ctx: &mut [u8; CONTEXTS],
        cat: usize,
        contexts: Category,
        max: usize,
    ) -> Option<(u8, Vec<i32>)> {
        let (sig, last, abs) = (
            usize::from(contexts.sig),
            usize::from(contexts.last),
            usize::from(contexts.abs),
        );
        // Chroma DC: Min(numDecod / NumC8x8, 2) with NumC8x8 = 1.
        let sig_inc = |i: usize| match cat {
            3 => i.min(2),
            5 => usize::from(t::SIG_CTX_INC_8X8_FRAME[i]),
            _ => i,
        };
        let last_inc = |i: usize| match cat {
            3 => i.min(2),
            5 => usize::from(t::LAST_CTX_INC_8X8[i]),
            _ => i,
        };
        let mut significant = vec![false; max];
        let mut num_coeff = max;
        let mut i = 0;
        while i < num_coeff - 1 {
            if e.decision(&mut ctx[sig + sig_inc(i)]) == 1 {
                significant[i] = true;
                if e.decision(&mut ctx[last + last_inc(i)]) == 1 {
                    num_coeff = i + 1;
                }
            }
            i += 1;
        }
        significant[num_coeff - 1] = true;
        let mut levels = vec![0i32; max];
        let (mut eq1, mut gt1) = (0usize, 0usize);
        let mut count = 0u8;
        for i in (0..num_coeff).rev() {
            if !significant[i] {
                continue;
            }
            count += 1;
            let inc0 = if gt1 != 0 { 0 } else { (1 + eq1).min(4) };
            let inc = 5 + gt1.min(4 - usize::from(cat == 3));
            let mut prefix = e.decision(&mut ctx[abs + inc0]);
            if prefix == 1 {
                while prefix < 14 && e.decision(&mut ctx[abs + inc]) == 1 {
                    prefix += 1;
                }
            }
            let mut value = prefix;
            if prefix == 14 {
                // UEG0 suffix (clause 9.3.2.3).
                let mut k = 0;
                while e.bypass() == 1 {
                    value += 1 << k;
                    k += 1;
                    if k >= 31 {
                        return None;
                    }
                }
                while k > 0 {
                    k -= 1;
                    value += e.bypass() << k;
                }
                if value >= 1 << 15 {
                    return None;
                }
            }
            let level = value as i32 + 1;
            if level == 1 {
                eq1 += 1;
            } else {
                gt1 += 1;
            }
            levels[i] = if e.bypass() == 1 { -level } else { level };
        }
        Some((count, levels))
    }

    /// The engine's whole state, for comparing two decoders.
    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    fn state(c: &Cabac) -> (u64, u32, u32, usize, u32, [u8; CONTEXTS]) {
        let e = c.engine;
        (
            e.value,
            e.bits,
            e.range,
            c.src.rest.len(),
            c.src.past_end,
            c.ctx,
        )
    }

    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    #[test]
    fn the_v3_instance_starts_and_reads_motion_as_the_baseline_does() {
        // Differential test: the engine's start and mvd_lX in the v3
        // instance against the baseline's, on random data, every
        // cabac_init_idc and slice QP: the same engine state after the
        // start and the same result and state after each mvd_lX.
        let Some(v3) = crate::fast::V3::detect() else {
            return;
        };
        let mut rng = Rng(0x6a09_e667_f3bc_c908);
        for case in 0..2000 {
            let len = 1 + rng.below(300) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let (offset, slot) = (rng.below(16) as usize, rng.below(4) as usize);
            let qp = rng.below(64) as i32 - 6;
            let (a, b) = (
                Cabac::new_in(v3, &data, offset, slot, qp),
                Cabac::new_in(Base, &data, offset, slot, qp),
            );
            let (mut a, mut b) = match (a, b) {
                (Ok(a), Ok(b)) => (a, b),
                (a, b) => {
                    assert_eq!(a.err(), b.err(), "case {case}");
                    continue;
                }
            };
            assert_eq!(state(&a), state(&b), "case {case}");
            for read in 0..64 {
                let inc0 = [rng.below(3) as usize, rng.below(3) as usize];
                let got = a.mvd(v3, inc0);
                assert_eq!(got, b.mvd(Base, inc0), "case {case}, read {read}");
                assert_eq!(state(&a), state(&b), "case {case}, read {read}");
                if got.is_err() {
                    break;
                }
            }
        }
    }

    #[cfg(all(feature = "fast", target_arch = "x86_64"))]
    #[test]
    fn the_v3_instance_decodes_what_the_baseline_decodes() {
        // Differential test: the site's functions against their twins, on
        // random data, every block shape, errors included: the same
        // result, the same block and the same engine state after each.
        let Some(v3) = crate::fast::V3::detect() else {
            return;
        };
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for case in 0..2000 {
            let len = 8 + rng.below(600) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let (slot, qp) = (rng.below(4) as usize, rng.below(52) as i32);
            let (Ok(mut a), Ok(mut b)) = (
                Cabac::new(&data, 0, slot, qp),
                Cabac::new(&data, 0, slot, qp),
            ) else {
                continue;
            };
            let scan: [u8; MAX_COEFFS] = std::array::from_fn(|_| rng.next() as u8);
            for block in 0..40 {
                let cat = rng.below(6) as usize;
                let contexts = CATEGORIES[cat];
                let max = 1 + rng.below(64) as usize;
                let (got, want) = match cat {
                    3 => {
                        let (mut x, mut y) = ([0i32; 4], [0i32; 4]);
                        let r = (
                            a.residual(v3, contexts, max, &mut x, &scan, &CoeffScale::IDENTITY),
                            b.residual(Base, contexts, max, &mut y, &scan, &CoeffScale::IDENTITY),
                        );
                        ((r.0, x.to_vec()), (r.1, y.to_vec()))
                    }
                    5 => {
                        let (mut x, mut y) = ([0i32; 64], [0i32; 64]);
                        let r = (
                            a.residual(v3, contexts, max, &mut x, &scan, &CoeffScale::IDENTITY),
                            b.residual(Base, contexts, max, &mut y, &scan, &CoeffScale::IDENTITY),
                        );
                        ((r.0, x.to_vec()), (r.1, y.to_vec()))
                    }
                    _ => {
                        let (mut x, mut y) = ([0i32; 16], [0i32; 16]);
                        let r = (
                            a.residual(v3, contexts, max, &mut x, &scan, &CoeffScale::IDENTITY),
                            b.residual(Base, contexts, max, &mut y, &scan, &CoeffScale::IDENTITY),
                        );
                        ((r.0, x.to_vec()), (r.1, y.to_vec()))
                    }
                };
                assert_eq!(got, want, "case {case}, block {block}");
                assert_eq!(state(&a), state(&b), "case {case}, block {block}");
                if got.0.is_err() {
                    break;
                }
            }
        }
    }

    #[test]
    fn a_block_that_fails_leaves_nothing_behind() {
        // A block whose levels decode but whose data has run out fails at
        // the end check, after its levels were written. They must not stay:
        // the macroblock layer does not clear a block that failed, and its
        // coefficient arrays outlive the slice.
        let mut rng = Rng(0x5851_f42d_4c95_7f2d);
        let mut failures = 0;
        for _ in 0..2000 {
            let len = 2 + rng.below(12) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let Ok(mut c) = Cabac::new(&data, 0, rng.below(4) as usize, rng.below(52) as i32)
            else {
                continue;
            };
            let identity: [u8; MAX_COEFFS] = std::array::from_fn(|k| k as u8);
            for _ in 0..8 {
                let mut out = [0i32; 64];
                if c.residual(
                    Base,
                    CATEGORIES[5],
                    64,
                    &mut out,
                    &identity,
                    &CoeffScale::IDENTITY,
                )
                .is_err()
                {
                    failures += 1;
                    assert_eq!(out, [0; 64], "a failed block kept levels");
                    break;
                }
            }
        }
        assert!(
            failures > 100,
            "only {failures} blocks failed, so little was tested"
        );
    }

    #[test]
    fn the_widened_tables_agree_with_the_specification_for_every_state() {
        for state in 0..128usize {
            for q in 0..4usize {
                let lps = t::RANGE_TAB_LPS_BY_STATE[(state << 2) | q];
                assert_eq!(
                    LPS[(state << 3) | (q + 4)],
                    lps,
                    "state {state}, qCodIRangeIdx {q}"
                );
                // RenormD (clause 9.3.3.2.2) on codIRange = rangeLPS.
                let (mut range, mut n) = (u16::from(lps), 0);
                while range < 256 {
                    range <<= 1;
                    n += 1;
                }
                assert_eq!(LPS_NORM[(state << 3) | (q + 4)], range | (n << 9));
            }
            // After the more probable symbol codIRange - rangeLPS is at
            // least 128, which `Engine::decision` relies on to shift by at
            // most 1.
            for range in 256..511usize {
                let lps = usize::from(LPS[(state << 3) | (range >> 6)]);
                assert!(range - lps >= 128, "state {state}, codIRange {range}");
            }
            for bin in 0..2usize {
                let lps = (bin ^ (state & 1)) << 7;
                assert_eq!(NEXT[(state << 1) | bin], t::NEXT_STATE[state | lps]);
            }
        }
    }

    #[test]
    fn every_bin_string_has_its_place_in_the_lookup() {
        for list in [
            t::BINS_MB_TYPE_I,
            t::BINS_MB_TYPE_P,
            t::BINS_MB_TYPE_B,
            t::BINS_SUB_MB_TYPE_P,
            t::BINS_SUB_MB_TYPE_B,
        ] {
            let strings = bin_strings(list);
            for &(value, len, code) in list {
                assert_eq!(strings[usize::from(len) - 1][usize::from(code)], value);
            }
            // Nothing else is in it, so no two strings share a place.
            let filled = strings
                .iter()
                .flatten()
                .filter(|&&v| v != NO_STRING)
                .count();
            assert_eq!(filled, list.len());
        }
    }
}
