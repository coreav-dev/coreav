//! A big-endian bit reader that never fails: bits past the end read as
//! zero, and the caller checks [`Bits::overrun`] where a syntax element
//! ends. Every read loads the eight bytes at the current byte and shifts,
//! so a read costs one bounds check and no refill state.

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Bits read so far.
    pos: usize,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Bits read so far.
    #[cfg(test)]
    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// Bits in the data.
    pub(crate) fn len(&self) -> usize {
        self.data.len().saturating_mul(8)
    }

    /// Bits left, zero once past the end.
    pub(crate) fn left(&self) -> usize {
        self.len().saturating_sub(self.pos)
    }

    /// Whether more bits were read than the data holds.
    pub(crate) fn overrun(&self) -> bool {
        self.pos > self.len()
    }

    /// The next 32 bits, left-aligned, without reading them.
    #[inline(always)]
    pub(crate) fn peek32(&self) -> u32 {
        let word = load(self.data, self.pos >> 3) << (self.pos & 7);
        (word >> 32) as u32
    }

    /// A [`Cache`] from the current position; [`Bits::resume`] takes its
    /// position back.
    #[inline(always)]
    pub(crate) fn cache(&self) -> Cache<'a> {
        let mut cache = Cache {
            data: self.data,
            word: 0,
            count: 0,
            byte: self.pos >> 3,
        };
        cache.refill();
        cache.skip((self.pos & 7) as u32);
        cache
    }

    /// Moves to the position `cache` reached.
    #[inline(always)]
    pub(crate) fn resume(&mut self, cache: &Cache<'_>) {
        self.pos = cache
            .byte
            .saturating_mul(8)
            .saturating_sub(cache.count as usize);
    }

    /// Moves `n` bits on.
    #[inline(always)]
    pub(crate) fn skip(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }

    /// Reads `n` bits, 0 to 32.
    #[inline(always)]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let value = self.peek32() >> (32 - n.min(32));
        self.skip(n as usize);
        value
    }

    /// Reads one bit.
    #[inline(always)]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Moves to the next multiple of 8 bits counted from `origin`.
    pub(crate) fn align(&mut self, origin: usize) {
        let rem = self.pos.wrapping_sub(origin) & 7;
        if rem != 0 {
            self.skip(8 - rem);
        }
    }
}

/// Eight bytes of `data` from `byte` on, big-endian, zero past the data.
#[inline(always)]
fn load(data: &[u8], byte: usize) -> u64 {
    match data.get(byte..byte.wrapping_add(8)) {
        Some(chunk) => {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(chunk);
            u64::from_be_bytes(bytes)
        }
        None => {
            let mut bytes = [0u8; 8];
            for (slot, &b) in bytes.iter_mut().zip(data.get(byte..).unwrap_or_default()) {
                *slot = b;
            }
            u64::from_be_bytes(bytes)
        }
    }
}

/// The next bits in a register, for runs of short reads (spectrum
/// codewords): a refill tops the register up to 56 to 63 bits without a
/// branch, so a read is a shift, and a codeword's lookup does not wait for
/// a load. `word` holds `count` bits, left-aligned; the bit after them
/// starts byte `byte` (refills add whole bytes). Bits past the end read as
/// zero.
pub(crate) struct Cache<'a> {
    data: &'a [u8],
    word: u64,
    count: u32,
    byte: usize,
}

impl Cache<'_> {
    /// Tops the register up to at least 56 bits.
    #[inline(always)]
    pub(crate) fn refill(&mut self) {
        self.word |= load(self.data, self.byte) >> self.count;
        self.byte = self.byte.wrapping_add(((63 - self.count) >> 3) as usize);
        self.count |= 56;
    }

    /// The next 32 bits, left-aligned, without reading them; at least as
    /// many are valid as `count`.
    #[inline(always)]
    pub(crate) fn peek32(&self) -> u32 {
        (self.word >> 32) as u32
    }

    /// Moves `n` bits on: at most the bits held, which callers keep by
    /// refilling (a wrong count gives a wrong position, which the frame's
    /// checks catch, never a panic).
    #[inline(always)]
    pub(crate) fn skip(&mut self, n: u32) {
        self.word = self.word.wrapping_shl(n);
        self.count = self.count.wrapping_sub(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_bytes_and_past_the_end() {
        let mut bits = Bits::new(&[0b1010_1100, 0xff]);
        assert_eq!(bits.read(3), 0b101);
        assert_eq!(bits.read(7), 0b011_0011);
        assert!(!bits.overrun());
        assert_eq!(bits.read(10), 0b11_1111_0000);
        assert!(bits.overrun());
        assert_eq!(bits.read(32), 0);
    }

    #[test]
    fn a_cache_reads_as_the_reader() {
        let data: Vec<u8> = (0..40u32).map(|i| (i * 37 + 11) as u8).collect();
        for start in 0..20 {
            let mut bits = Bits::new(&data);
            bits.skip(start);
            let mut cache = bits.cache();
            let mut plain = Bits::new(&data);
            plain.skip(start);
            for n in [3u32, 17, 9, 20, 1, 13, 20, 20, 5] {
                cache.refill();
                assert_eq!(cache.peek32(), plain.peek32(), "start {start}");
                cache.skip(n);
                plain.skip(n as usize);
            }
            bits.resume(&cache);
            assert_eq!(bits.position(), plain.position());
            assert_eq!(bits.peek32(), plain.peek32());
        }
    }

    #[test]
    fn aligns_from_an_origin() {
        let mut bits = Bits::new(&[0; 4]);
        bits.skip(3);
        bits.align(1);
        assert_eq!(bits.position(), 9);
        bits.align(1);
        assert_eq!(bits.position(), 9);
    }
}
