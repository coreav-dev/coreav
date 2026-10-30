//! The range decoder (RFC 6716 section 4.1), which every symbol of SILK and
//! CELT goes through, and the raw bits CELT packs at the end of a frame.
//!
//! Written from the section's prose, which fixes all of its arithmetic.
//! Choices:
//! - The table-driven decode (4.1.3.3) finds its symbol without a
//!   division: with `r = rng >> ftb`, the symbol is the first `k` with
//!   `val >= r * icdf[k]`, which is the prose's search on `fs` multiplied
//!   through by `r`, and the update then needs only `r * icdf[k]` and
//!   `r * icdf[k - 1]`.
//!   Rejected: `decode_bin` and a search of the table, a division per
//!   symbol on SILK's hottest path.
//! - Raw bits are taken from the back through a 64-bit window, so one call
//!   can return up to 32 bits. Rejected: a 32-bit window, which limits a
//!   call to 25 bits and needs a check of its own.
//! - The front and the back read the same bytes independently: the range
//!   coder reads into the raw bits' bytes, and the raw bits return them
//!   too (4.1.2.1, 4.1.4). Past the end of the frame both read zeros; a
//!   value of `uint` that lands outside its range sets
//!   [`RangeDecoder::corrupt`] and saturates (4.1.5).
//! - Arithmetic that a caller's inconsistent arguments could overflow wraps,
//!   as the specification's unsigned arithmetic does, and an empty range
//!   marks the frame corrupt, rather than panicking; with consistent
//!   arguments none of it overflows. Rejected: a `Result` from every read,
//!   a branch on the hot path for states valid streams never reach.

/// The range above which the state is normalized (4.1.2.1): `rng > 2**23`.
const NORM: u32 = 1 << 23;

/// A range decoder over one frame (4.1).
#[derive(Clone, Debug)]
pub(crate) struct RangeDecoder<'a> {
    data: &'a [u8],
    /// The next byte the range coder reads, from the front.
    front: usize,
    /// How many bytes the raw bits have taken from the back.
    back: usize,
    /// The difference between the top of the range and the coded value,
    /// minus one (4.1).
    val: u32,
    /// The size of the range.
    rng: u32,
    /// The last byte read; its low bit is the next symbol byte's high bit.
    last: u32,
    /// `rng / ft` of the last `decode` or `decode_bin`, for `update`.
    scale: u32,
    /// Raw bits read from the back and not yet returned, low bit first.
    window: u64,
    window_bits: u32,
    /// Whole bits used so far, the buffered ones included (`nbits_total`,
    /// 4.1.6).
    total: u32,
    corrupt: bool,
}

impl<'a> RangeDecoder<'a> {
    /// Starts decoding a frame (4.1.1); an empty frame reads as zeros.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut dec = Self {
            data,
            front: 0,
            back: 0,
            val: 0,
            rng: 128,
            last: 0,
            scale: 1,
            window: 0,
            window_bits: 0,
            total: 9,
            corrupt: false,
        };
        dec.last = dec.next_front();
        dec.val = 127 - (dec.last >> 1);
        dec.normalize();
        dec
    }

    fn next_front(&mut self) -> u32 {
        match self.data.get(self.front) {
            Some(&b) => {
                self.front += 1;
                u32::from(b)
            }
            None => 0,
        }
    }

    fn next_back(&mut self) -> u32 {
        let byte = self
            .data
            .len()
            .checked_sub(self.back + 1)
            .and_then(|i| self.data.get(i));
        match byte {
            Some(&b) => {
                self.back += 1;
                u32::from(b)
            }
            None => 0,
        }
    }

    /// Renormalization (4.1.2.1): until `rng > 2**23`, shift in one byte.
    fn normalize(&mut self) {
        // Only an update with an empty range (fh <= fl) leaves rng at 0,
        // which shifting would never raise.
        if self.rng == 0 {
            self.corrupt = true;
            self.rng = 1;
        }
        while self.rng <= NORM {
            self.total = self.total.wrapping_add(8);
            self.rng <<= 8;
            let byte = self.next_front();
            let sym = ((self.last << 8 | byte) >> 1) & 0xFF;
            self.last = byte;
            self.val = (self.val << 8).wrapping_add(255 - sym) & 0x7FFF_FFFF;
        }
    }

    /// The first step of decoding a symbol of a context with total `ft`
    /// (at most 65,535): the value `fs` that lies in the symbol's range
    /// (4.1.2). [`Self::update`] must follow.
    pub(crate) fn decode(&mut self, ft: u32) -> u32 {
        self.scale = (self.rng / ft.max(1)).max(1);
        ft.saturating_sub((self.val / self.scale + 1).min(ft))
    }

    /// [`Self::decode`] for `ft = 1 << ftb` (4.1.3.1).
    pub(crate) fn decode_bin(&mut self, ftb: u32) -> u32 {
        let ft = 1u32 << ftb.min(16);
        self.scale = (self.rng >> ftb.min(16)).max(1);
        ft - (self.val / self.scale + 1).min(ft)
    }

    /// The second step: updates the state with the decoded symbol's
    /// `(fl, fh, ft)` (4.1.2), `ft` as given to the `decode` or
    /// `decode_bin` just before it, which no other read may come between.
    pub(crate) fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.scale.wrapping_mul(ft.wrapping_sub(fh));
        self.val = self.val.wrapping_sub(s);
        self.rng = if fl > 0 {
            self.scale.wrapping_mul(fh.wrapping_sub(fl))
        } else {
            self.rng.wrapping_sub(s)
        };
        self.normalize();
    }

    /// A binary symbol whose "1" has probability `2**-logp` (4.1.3.2).
    pub(crate) fn bit_logp(&mut self, logp: u32) -> bool {
        let s = self.rng >> logp.min(31);
        let one = self.val < s;
        if one {
            self.rng = s;
        } else {
            self.val -= s;
            self.rng -= s;
        }
        self.normalize();
        one
    }

    /// A symbol of a context given by its inverse cumulative table
    /// `icdf` with total `1 << ftb` (4.1.3.3). A table that does not end
    /// in 0 decodes as if it did.
    pub(crate) fn icdf(&mut self, icdf: &[u8], ftb: u32) -> usize {
        let r = self.rng >> ftb.min(16);
        // `prev` is r * icdf[k - 1], or rng for k = 0.
        let mut prev = self.rng;
        for (k, &v) in icdf.iter().enumerate() {
            // Wrapping only for a table with values of 1 << ftb or more or
            // out of order, which no table of the specification has.
            let cur = r.wrapping_mul(u32::from(v));
            if self.val >= cur {
                self.val -= cur;
                self.rng = prev.wrapping_sub(cur);
                self.normalize();
                return k;
            }
            prev = cur;
        }
        // Only a table without its terminating 0 gets here.
        self.rng = prev;
        self.normalize();
        icdf.len().saturating_sub(1)
    }

    /// An integer in `0..ft`, each value equally likely (4.1.5). `ft` must
    /// be at least 2; below that the result is 0 and nothing is read. A
    /// value of `ft` or more marks the frame [corrupt](Self::corrupt) and
    /// reads as `ft - 1`.
    pub(crate) fn uint(&mut self, ft: u32) -> u32 {
        if ft < 2 {
            return 0;
        }
        let ftb = ilog(ft - 1);
        if ftb <= 8 {
            let s = self.decode(ft);
            self.update(s, s + 1, ft);
            return s;
        }
        let shift = ftb - 8;
        let top = ((ft - 1) >> shift) + 1;
        let s = self.decode(top);
        self.update(s, s + 1, top);
        let t = s << shift | self.bits(shift);
        if t >= ft {
            self.corrupt = true;
            return ft - 1;
        }
        t
    }

    /// `n` raw bits from the back of the frame, at most 32, the first bit
    /// packed the least significant (4.1.4).
    pub(crate) fn bits(&mut self, n: u32) -> u32 {
        let n = n.min(32);
        while self.window_bits < n {
            self.window |= u64::from(self.next_back()) << self.window_bits;
            self.window_bits += 8;
        }
        let out = (self.window & ((1u64 << n) - 1)) as u32;
        self.window >>= n;
        self.window_bits -= n;
        self.total = self.total.wrapping_add(n);
        out
    }

    /// Takes the last `bytes` bytes off the frame, as a redundant CELT
    /// frame's size does once it is known (4.5.1.3): raw bits then come
    /// from the new end, and reads past it give zeros.
    pub(crate) fn shrink(&mut self, bytes: usize) {
        self.data = self
            .data
            .get(..self.data.len().saturating_sub(bytes))
            .unwrap_or(&[]);
    }

    /// The size of the frame in bits.
    pub(crate) fn storage_bits(&self) -> u32 {
        (8 * self.data.len()) as u32
    }

    /// Counts every bit of the frame as used, as CELT does for a silent
    /// frame (4.3.1); later reads past the budget read zeros.
    pub(crate) fn skip_to_end(&mut self) {
        let used = self.tell();
        self.total = self
            .total
            .wrapping_add(self.storage_bits().wrapping_sub(used));
    }

    /// Whole bits used so far, an upper bound (4.1.6.1).
    pub(crate) fn tell(&self) -> u32 {
        tell(self.total, self.rng)
    }

    /// Bits used so far in eighths, an upper bound (4.1.6.2).
    pub(crate) fn tell_frac(&self) -> u32 {
        tell_frac(self.total, self.rng)
    }

    /// The size of the range, which a frame's decoding ends with: RFC 6716
    /// section 6 compares it with the reference decoder's.
    pub(crate) fn range(&self) -> u32 {
        self.rng
    }

    /// Whether a value decoded so far was outside its range (4.1.5), or an
    /// update was given an empty range.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the reference decodes a corrupt frame to its end; tests read it"
        )
    )]
    pub(crate) fn corrupt(&self) -> bool {
        self.corrupt
    }
}

/// `tell` of a coder that has counted `total` bits and holds the range
/// `rng` (4.1.6.1); the encoder's is the same (5.1.6).
fn tell(total: u32, rng: u32) -> u32 {
    total.wrapping_sub(ilog(rng))
}

/// `tell_frac` of the same (4.1.6.2): the three fraction bits that
/// squaring the top 16 bits of `rng` three times gives grow with those bits,
/// so they are the count of [`FRACTION_STEPS`] at or below them.
fn tell_frac(total: u32, rng: u32) -> u32 {
    let lg = ilog(rng);
    // rng > 2**23 after normalization, so lg >= 24 and r >= 2**15.
    let r = rng >> lg.saturating_sub(16);
    let fraction = if lg >= 16 {
        FRACTION_STEPS.iter().filter(|&&step| r >= step).count() as u32
    } else {
        fraction_by_squaring(r)
    };
    total.wrapping_mul(8).wrapping_sub(8 * lg + fraction)
}

/// The three bits below the binary point of log2 of `r` / 2^15, by
/// squaring three times (4.1.6.2).
const fn fraction_by_squaring(mut r: u32) -> u32 {
    let mut fraction = 0;
    let mut i = 0;
    while i < 3 {
        r = r.wrapping_mul(r) >> 15;
        let bit = r >> 16;
        fraction = 2 * fraction + bit;
        r >>= bit;
        i += 1;
    }
    fraction
}

/// For each fraction from 1 to 7, the least `r` from 2^15 that
/// [`fraction_by_squaring`] gives it or more.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const FRACTION_STEPS: [u32; 7] = {
    let mut steps = [1 << 16; 7];
    let mut r = 1 << 15;
    while r < 1 << 16 {
        let f = fraction_by_squaring(r) as usize;
        let mut j = 0;
        while j < f {
            if r < steps[j] {
                steps[j] = r;
            }
            j += 1;
        }
        r += 1;
    }
    steps
};

/// The number of bits needed to store `x`: 0 for 0 (`ilog`, RFC 6716
/// section 1.1.10).
pub(crate) fn ilog(x: u32) -> u32 {
    32 - x.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tell_frac_counts_the_squarings() {
        for r in 1u32 << 15..1 << 16 {
            for shift in [8, 15] {
                let rng = r << shift;
                let lg = ilog(rng);
                assert_eq!(
                    tell_frac(100, rng),
                    800 - 8 * lg - fraction_by_squaring(r),
                    "rng {rng}"
                );
            }
        }
    }

    /// A range encoder written from RFC 6716 section 5.1, for round trips:
    /// the encoder's `rng` after each symbol must equal the decoder's
    /// (5.1), and so must its count of bits, which it keeps on its own: 33
    /// at the start (`tell` 1 with `rng = 2**31`), 8 for each byte its
    /// renormalization sends out, and each raw bit (5.1.6, 4.1.6). One correction to the prose: for a first symbol (`fl == 0`)
    /// 5.1.1 prints `rng - rng/ft * (fh - fl)`, while the decoder's update
    /// of 4.1.2, which the encoder must mirror, is `rng - rng/ft * (ft -
    /// fh)`; the encoder here uses the latter.
    #[derive(Clone)]
    struct Encoder {
        val: u64,
        rng: u32,
        rem: Option<u32>,
        ext: u32,
        front: Vec<u8>,
        back: Vec<u8>,
        window: u64,
        window_bits: u32,
        total: u32,
    }

    impl Encoder {
        fn new() -> Self {
            Self {
                val: 0,
                rng: 1 << 31,
                rem: None,
                ext: 0,
                front: Vec::new(),
                back: Vec::new(),
                window: 0,
                window_bits: 0,
                total: 33,
            }
        }

        fn carry_out(&mut self, c: u32) {
            if c == 255 {
                self.ext += 1;
                return;
            }
            let b = c >> 8;
            if let Some(rem) = self.rem {
                self.front.push((rem + b) as u8);
            }
            for _ in 0..self.ext {
                self.front.push(if b == 1 { 0 } else { 255 });
            }
            self.ext = 0;
            self.rem = Some(c & 255);
        }

        fn normalize(&mut self) {
            while self.rng <= NORM {
                self.total += 8;
                self.carry_out((self.val >> 23) as u32);
                self.val = (self.val << 8) & 0x7FFF_FFFF;
                self.rng <<= 8;
            }
        }

        fn encode(&mut self, fl: u32, fh: u32, ft: u32) {
            let r = self.rng / ft;
            if fl > 0 {
                self.val += u64::from(self.rng - r * (ft - fl));
                self.rng = r * (fh - fl);
            } else {
                self.rng -= r * (ft - fh);
            }
            self.normalize();
        }

        fn bits(&mut self, value: u32, n: u32) {
            self.total += n;
            self.window |= u64::from(value) << self.window_bits;
            self.window_bits += n;
            while self.window_bits >= 8 {
                self.back.push(self.window as u8);
                self.window >>= 8;
                self.window_bits -= 8;
            }
        }

        /// Finalizes (5.1.5) into a frame of `size` bytes, the raw bits
        /// ORed in from the back, over the range coder's last bytes where
        /// the frame is tight. Range coder bytes past `size` must be zeros.
        fn finish(mut self, size: usize) -> Vec<u8> {
            if self.window_bits > 0 {
                self.back.push(self.window as u8);
            }
            let (lo, hi) = (self.val, self.val + u64::from(self.rng));
            let mut end = lo;
            for b in (0..32).rev() {
                let mask = (1u64 << b) - 1;
                let e = (lo + mask) & !mask;
                if e + mask < hi {
                    end = e;
                    break;
                }
            }
            while end != 0 {
                self.carry_out((end >> 23) as u32);
                end = (end << 8) & 0x7FFF_FFFF;
            }
            if self.rem.is_some_and(|r| r != 0) || self.ext > 0 {
                self.carry_out(0);
            }
            assert!(self.back.len() <= size, "raw bits past the frame");
            let mut out = vec![0u8; size];
            for (i, &b) in self.front.iter().enumerate() {
                if i < size {
                    out[i] |= b;
                } else {
                    assert_eq!(b, 0, "range coder data past the frame");
                }
            }
            for (i, &b) in self.back.iter().enumerate() {
                out[size - 1 - i] |= b;
            }
            out
        }
    }

    /// A small deterministic generator (xorshift64*).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: u32) -> u32 {
            (self.next() >> 32) as u32 % n
        }
    }

    #[derive(Debug)]
    enum Op {
        /// A symbol `k` of the frequencies `f`.
        Symbol(Vec<u32>, u32),
        /// A symbol of `1 << ftb` given by `(fl, fh)`.
        Bin(u32, u32, u32),
        Logp(u32, bool),
        /// An icdf table with `ftb` and a symbol.
        Icdf(Vec<u8>, u32, usize),
        Uint(u32, u32),
        Bits(u32, u32),
    }

    fn random_ops(rng: &mut Rng, count: usize) -> Vec<Op> {
        (0..count)
            .map(|_| match rng.below(6) {
                0 => {
                    let n = 2 + rng.below(20) as usize;
                    let f: Vec<u32> = (0..n).map(|_| 1 + rng.below(3000)).collect();
                    let k = rng.below(n as u32);
                    Op::Symbol(f, k)
                }
                1 => {
                    let ftb = 1 + rng.below(15);
                    let a = rng.below(1 << ftb);
                    let b = rng.below(1 << ftb);
                    Op::Bin(ftb, a.min(b), a.max(b) + 1)
                }
                2 => {
                    let logp = 1 + rng.below(15);
                    Op::Logp(logp, rng.below(1 << logp.min(4)) == 0)
                }
                3 => {
                    let ftb = 1 + rng.below(8);
                    let ft = 1u32 << ftb;
                    let n = 1 + rng.below(ft.min(16)) as usize;
                    // Strictly decreasing from below ft to 0.
                    let mut cuts: Vec<u32> = (0..n - 1).map(|_| 1 + rng.below(ft - 1)).collect();
                    cuts.sort_unstable_by(|a, b| b.cmp(a));
                    cuts.dedup();
                    cuts.push(0);
                    let k = rng.below(cuts.len() as u32) as usize;
                    Op::Icdf(cuts.iter().map(|&c| c as u8).collect(), ftb, k)
                }
                4 => {
                    let ft = ((rng.next() >> 32) as u32 >> rng.below(31)).max(2);
                    Op::Uint(ft, rng.below(ft))
                }
                _ => {
                    let n = rng.below(33);
                    let v = if n == 32 {
                        rng.next() as u32
                    } else {
                        rng.next() as u32 & ((1u64 << n) - 1) as u32
                    };
                    Op::Bits(n, v)
                }
            })
            .collect()
    }

    fn icdf_range(icdf: &[u8], ftb: u32, k: usize) -> (u32, u32) {
        let ft = 1u32 << ftb;
        let fl = if k == 0 {
            0
        } else {
            ft - u32::from(icdf[k - 1])
        };
        (fl, ft - u32::from(icdf[k]))
    }

    /// Encodes `ops`, then decodes them from a frame with room to spare and
    /// from the tightest frame `tell` allows, where the range coder's last
    /// bytes and the raw bits share bytes, and checks every value, and the
    /// range and the bit count after every operation against the encoder's.
    fn round_trip(ops: &[Op]) {
        let mut enc = Encoder::new();
        // (rng, total) after each operation.
        let mut states = Vec::new();
        for op in ops {
            match op {
                Op::Symbol(f, k) => {
                    let fl: u32 = f[..*k as usize].iter().sum();
                    enc.encode(fl, fl + f[*k as usize], f.iter().sum());
                }
                Op::Bin(ftb, fl, fh) => enc.encode(*fl, *fh, 1 << ftb),
                Op::Logp(logp, one) => {
                    let ft = 1 << logp;
                    if *one {
                        enc.encode(ft - 1, ft, ft)
                    } else {
                        enc.encode(0, ft - 1, ft)
                    }
                }
                Op::Icdf(icdf, ftb, k) => {
                    let (fl, fh) = icdf_range(icdf, *ftb, *k);
                    enc.encode(fl, fh, 1 << ftb);
                }
                Op::Uint(ft, t) => {
                    let ftb = ilog(ft - 1);
                    if ftb <= 8 {
                        enc.encode(*t, t + 1, *ft);
                    } else {
                        let s = ftb - 8;
                        enc.encode(t >> s, (t >> s) + 1, ((ft - 1) >> s) + 1);
                        enc.bits(t & ((1 << s) - 1), s);
                    }
                }
                Op::Bits(n, v) => enc.bits(*v, *n),
            }
            states.push((enc.rng, enc.total));
        }
        let tight = (tell(enc.total, enc.rng) as usize).div_ceil(8);
        let loose = enc.front.len() + enc.back.len() + 6;
        for size in [tight, loose] {
            decode_all(ops, &enc.clone().finish(size), &states);
        }
    }

    fn decode_all(ops: &[Op], frame: &[u8], states: &[(u32, u32)]) {
        let size = frame.len();
        let mut dec = RangeDecoder::new(frame);
        assert_eq!(dec.tell(), 1, "a new decoder has used one bit (4.1.6.1)");
        for (i, op) in ops.iter().enumerate() {
            match op {
                Op::Symbol(f, k) => {
                    let ft: u32 = f.iter().sum();
                    let fs = dec.decode(ft);
                    let mut fl = 0;
                    let mut got = 0;
                    for (j, &fj) in f.iter().enumerate() {
                        if fs < fl + fj {
                            got = j;
                            break;
                        }
                        fl += fj;
                    }
                    assert_eq!(got as u32, *k, "op {i}");
                    dec.update(fl, fl + f[got], ft);
                }
                Op::Bin(ftb, fl, fh) => {
                    let fs = dec.decode_bin(*ftb);
                    assert!(*fl <= fs && fs < *fh, "op {i}");
                    dec.update(*fl, *fh, 1 << ftb);
                }
                Op::Logp(logp, one) => assert_eq!(dec.bit_logp(*logp), *one, "op {i}"),
                Op::Icdf(icdf, ftb, k) => assert_eq!(dec.icdf(icdf, *ftb), *k, "op {i}"),
                Op::Uint(ft, t) => assert_eq!(dec.uint(*ft), *t, "op {i}"),
                Op::Bits(n, v) => assert_eq!(dec.bits(*n), *v, "op {i}"),
            }
            let (rng, total) = states[i];
            assert_eq!(dec.range(), rng, "range after op {i}");
            assert_eq!(dec.tell(), tell(total, rng), "tell after op {i}");
            assert_eq!(
                dec.tell_frac(),
                tell_frac(total, rng),
                "tell_frac after op {i}"
            );
            assert!(dec.tell() as usize <= size * 8, "tell after op {i}");
            assert_eq!(
                dec.tell(),
                dec.tell_frac().div_ceil(8),
                "tell is ceil(tell_frac / 8) (4.1.6)"
            );
        }
        assert!(!dec.corrupt());
    }

    #[test]
    fn round_trips() {
        let mut rng = Rng(0x0123_4567_89AB_CDEF);
        for _ in 0..2000 {
            let count = 1 + rng.below(60) as usize;
            let ops = random_ops(&mut rng, count);
            round_trip(&ops);
        }
    }

    #[test]
    fn initial_state() {
        // 4.1.1: rng = 128, val = 127 - (b0 >> 1), then three
        // renormalizations to rng = 2**31, 33 bits counted.
        let dec = RangeDecoder::new(&[]);
        assert_eq!(dec.range(), 1 << 31);
        assert_eq!(dec.tell(), 1);
        assert_eq!(dec.tell_frac(), 8);
        let dec = RangeDecoder::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
        // val = 127 - 127, then bytes of sym 255 each shift in 0.
        assert_eq!(dec.val, 0);
    }

    #[test]
    fn raw_bits_from_the_back() {
        // 4.1.4: the first value's low bit is the last byte's low bit.
        let mut dec = RangeDecoder::new(&[0, 0, 0b1010_0000, 0b0000_0110]);
        assert_eq!(dec.bits(1), 0);
        assert_eq!(dec.bits(2), 0b11);
        assert_eq!(dec.bits(5), 0);
        assert_eq!(dec.bits(8), 0b1010_0000);
        // Past the start: zeros.
        assert_eq!(dec.bits(32), 0);
        assert_eq!(dec.bits(32), 0);
    }

    #[test]
    fn uint_out_of_range_is_corrupt() {
        // ft = 257: a range-coded top of 129 values, then one raw bit. All
        // ones keep val at 0, which decodes the last top value, 128, and
        // the raw bit is 1: t = 257, out of range, saturated to 256.
        let mut dec = RangeDecoder::new(&[0xFF; 4]);
        assert_eq!(dec.uint(257), 256);
        assert!(dec.corrupt());
        let mut dec = RangeDecoder::new(&[0xFF; 4]);
        assert_eq!(dec.uint(258), 257);
        assert!(!dec.corrupt());
    }

    #[test]
    fn garbage_never_panics() {
        // Any bytes, any sequence of calls, even inconsistent ones: no
        // panic (overflow included, in debug builds).
        let mut rng = Rng(42);
        for _ in 0..3000 {
            let len = rng.below(40) as usize;
            let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let mut dec = RangeDecoder::new(&data);
            for _ in 0..200 {
                match rng.below(8) {
                    0 => {
                        let ft = rng.below(70_000);
                        let fs = dec.decode(ft);
                        dec.update(fs, fs.wrapping_add(rng.below(3)), ft);
                    }
                    1 => {
                        let ftb = rng.below(20);
                        let fs = dec.decode_bin(ftb);
                        dec.update(fs, fs + 1, 1 << ftb.min(16));
                    }
                    2 => {
                        dec.bit_logp(rng.below(40));
                    }
                    3 => {
                        let n = rng.below(10) as usize;
                        let t: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                        dec.icdf(&t, rng.below(20));
                    }
                    4 => {
                        dec.uint(rng.next() as u32);
                    }
                    5 => {
                        dec.bits(rng.below(40));
                    }
                    _ => {
                        dec.tell();
                        dec.tell_frac();
                    }
                }
            }
        }
    }
}
