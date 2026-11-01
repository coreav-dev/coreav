//! The boolean decoder (clause 9.2): `init_bool`, `read_bool`,
//! `read_literal` and `exit_bool`.

/// Reads bools from `sz` bytes, as clause 9.2 does one bit at a time.
///
/// `window` holds `BoolValue` followed by the `lookahead` bits of the
/// data that clause 9.2.2 has not shifted into it yet. Shifting a bit in
/// leaves `window` unchanged and takes one bit from `lookahead`, so
/// `BoolValue` is `window >> lookahead` throughout.
///
/// That holds while `BoolValue` is less than `BoolRange`, which every
/// read keeps once it is true. Only a first byte of 255 starts with them
/// equal; its marker bit is then 1, the data does not conform, and the
/// reads after it are not clause 9.2's, though they never panic.
pub(crate) struct BoolDecoder<'a> {
    data: &'a [u8],
    /// The bytes of `data` moved into `window`, with any 0 bytes read past
    /// its end counted too.
    loaded: usize,
    window: u64,
    lookahead: u32,
    /// `BoolRange`, 128 to 255 between reads.
    range: u32,
    marker_zero: bool,
}

impl<'a> BoolDecoder<'a> {
    /// `init_bool(sz)` (clause 9.2.1), `sz` being `data.len()`; it reads
    /// the marker bit.
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut d = BoolDecoder {
            data,
            loaded: 1,
            window: u64::from(data.first().copied().unwrap_or(0)),
            lookahead: 0,
            range: 255,
            marker_zero: true,
        };
        d.marker_zero = !d.read(128);
        d
    }

    /// `read_bool(p)` (clause 9.2.2): a bool whose probability of being 0
    /// is `p` / 256.
    pub(crate) fn read(&mut self, p: u8) -> bool {
        if self.lookahead < 8 {
            self.fill();
        }
        let split = 1 + (((self.range - 1) * u32::from(p)) >> 8);
        let scaled = u64::from(split) << self.lookahead;
        let bit = self.window >= scaled;
        if bit {
            self.range -= split;
            self.window -= scaled;
        } else {
            self.range = split;
        }
        let shift = self.range.leading_zeros() - 24;
        self.range <<= shift;
        self.lookahead -= shift;
        bit
    }

    /// `read_literal(n)` (clause 9.2.4): `n` bools of probability 128,
    /// most significant first.
    pub(crate) fn literal(&mut self, n: u32) -> u8 {
        let mut x = 0u8;
        for _ in 0..n {
            x = (x << 1) | u8::from(self.read(128));
        }
        x
    }

    /// `exit_bool()` (clause 9.2.3): whether the marker bit was 0, no
    /// bool needed a bit past the data (`BoolMaxBits` never ran out), and
    /// every padding bit is 0.
    pub(crate) fn exit(&self) -> bool {
        let max_bits = (8 * self.data.len()).saturating_sub(8);
        let shifted_in = 8 * self.loaded - 8 - self.lookahead as usize;
        let lookahead_zero = self.window & ((1u64 << self.lookahead) - 1) == 0;
        let rest_zero = self
            .data
            .get(self.loaded..)
            .is_none_or(|rest| rest.iter().all(|&b| b == 0));
        self.marker_zero
            && !self.data.is_empty()
            && shifted_in <= max_bits
            && lookahead_zero
            && rest_zero
    }

    /// Moves bytes into `window` until it holds more than 48 bits after
    /// `BoolValue`; past the end of the data, the bytes are 0.
    fn fill(&mut self) {
        while self.lookahead <= 48 {
            let byte = self.data.get(self.loaded).copied().unwrap_or(0);
            self.window = (self.window << 8) | u64::from(byte);
            self.loaded += 1;
            self.lookahead += 8;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    /// Clause 9.2 as written: one bit at a time, with `BoolMaxBits`.
    struct Spec<'a> {
        data: &'a [u8],
        pos: usize,
        value: u32,
        range: u32,
        max_bits: usize,
        ran_out: bool,
    }

    impl<'a> Spec<'a> {
        fn f1(&mut self) -> u32 {
            let bit = self
                .data
                .get(self.pos >> 3)
                .map_or(0, |b| u32::from(b >> (7 - (self.pos & 7)) & 1));
            self.pos += 1;
            bit
        }

        fn init(data: &'a [u8]) -> (Self, bool) {
            let mut s = Spec {
                data,
                pos: 0,
                value: 0,
                range: 255,
                max_bits: 8 * data.len() - 8,
                ran_out: false,
            };
            for _ in 0..8 {
                s.value = 2 * s.value + s.f1();
            }
            let marker = s.read(128);
            (s, marker)
        }

        fn read(&mut self, p: u8) -> bool {
            let split = 1 + (((self.range - 1) * u32::from(p)) >> 8);
            let bit = if self.value < split {
                self.range = split;
                false
            } else {
                self.range -= split;
                self.value -= split;
                true
            };
            while self.range < 128 {
                let new_bit = if self.max_bits > 0 {
                    self.max_bits -= 1;
                    self.f1()
                } else {
                    self.ran_out = true;
                    0
                };
                self.range *= 2;
                self.value = (self.value << 1) + new_bit;
            }
            bit
        }

        fn exit(&mut self) -> bool {
            let mut padding = 0;
            for _ in 0..self.max_bits {
                padding |= self.f1();
            }
            padding == 0 && !self.ran_out
        }
    }

    /// A xorshift generator, so the cases are the same on every run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn it_reads_what_clause_9_2_reads_bit_by_bit() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for case in 0..4000 {
            let len = 1 + (rng.next() % 40) as usize;
            let mut data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            data[0] = data[0].min(0xfe);
            if case % 4 == 0 {
                data[0] &= 0x7f;
                let keep = (rng.next() % len as u64) as usize;
                data[keep + 1..].iter_mut().for_each(|b| *b = 0);
            }
            let reads = (rng.next() % (12 * len as u64 + 8)) as usize;
            let probs: Vec<u8> = (0..reads).map(|_| rng.next() as u8).collect();
            let mut ours = BoolDecoder::new(&data);
            let (mut spec, marker) = Spec::init(&data);
            for &p in &probs {
                assert_eq!(ours.read(p), spec.read(p), "case {case}");
            }
            assert_eq!(ours.exit(), !marker && spec.exit(), "case {case}");
        }
    }

    #[test]
    fn a_literal_is_read_most_significant_first() {
        let mut d = BoolDecoder::new(&[0x00, 0xff, 0xff, 0x00]);
        let bits: Vec<u8> = (0..4).map(|_| d.literal(1)).collect();
        let mut again = BoolDecoder::new(&[0x00, 0xff, 0xff, 0x00]);
        let packed = again.literal(4);
        assert_eq!(
            packed,
            bits.iter().fold(0, |acc, &b| (acc << 1) | b),
            "{bits:?}"
        );
    }

    #[test]
    fn exit_refuses_a_marker_of_1_and_padding_that_is_not_0() {
        assert!(BoolDecoder::new(&[0x00, 0x00]).exit());
        assert!(!BoolDecoder::new(&[0xff, 0x00]).exit());
        assert!(!BoolDecoder::new(&[0x00, 0x01]).exit());
        assert!(!BoolDecoder::new(&[]).exit());
        assert!(!BoolDecoder::new(&[0xff, 0x00, 0x00]).exit());
    }

    #[test]
    fn exit_refuses_reads_past_the_data() {
        let mut d = BoolDecoder::new(&[0x00]);
        assert!(d.exit());
        for _ in 0..8 {
            d.read(128);
        }
        assert!(!d.exit());
    }
}
