//! The Huffman decoding of a granule's spectral values (ISO/IEC 11172-3
//! 2.4.2.7 `huffmancodebits()`, 2.4.3.4): pairs in three regions of the
//! big-values part, then quadruples of the count1 part until
//! `part2_3_length` runs out.

use super::layer3::{Bands, GranuleInfo};
use super::tables;
use crate::Error;

/// A bit reader over a frame's main data that never fails: reads past the
/// end give zero bits, and the caller compares [`MainBits::position`] with
/// the granule's end. It keeps the next bits in a cache, left-aligned, and
/// refills it without a branch: an unaligned load of eight bytes at `next`,
/// shifted below the valid bits, tops it up to at least 56.
pub(crate) struct MainBits<'a> {
    data: &'a [u8],
    /// The first byte not yet in `cache`.
    next: usize,
    /// The valid bits, left-aligned; the bits below them are zero.
    cache: u64,
    /// Valid bits in `cache`, at least 56 after a refill.
    have: u32,
}

impl<'a> MainBits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut bits = Self {
            data,
            next: 0,
            cache: 0,
            have: 0,
        };
        bits.seek(0);
        bits
    }

    /// Bits read so far.
    pub(crate) fn position(&self) -> usize {
        (self.next * 8).saturating_sub(self.have as usize)
    }

    /// Moves to bit `position`.
    pub(crate) fn seek(&mut self, position: usize) {
        self.next = position / 8;
        self.cache = 0;
        self.have = 0;
        self.refill();
        self.skip((position % 8) as u32);
    }

    /// Eight bytes from `byte` on, big-endian, zero past the data.
    #[inline(always)]
    fn load(&self, byte: usize) -> u64 {
        match self.data.get(byte..byte.saturating_add(8)) {
            Some(chunk) => u64::from_be_bytes(<[u8; 8]>::try_from(chunk).unwrap_or([0; 8])),
            None => {
                let mut bytes = [0u8; 8];
                for (slot, &b) in bytes
                    .iter_mut()
                    .zip(self.data.get(byte..).unwrap_or_default())
                {
                    *slot = b;
                }
                u64::from_be_bytes(bytes)
            }
        }
    }

    /// Makes at least 56 bits available.
    #[inline(always)]
    fn refill(&mut self) {
        self.cache |= self.load(self.next) >> self.have;
        self.next += ((63 - self.have) >> 3) as usize;
        self.have |= 56;
    }

    /// The next 32 bits, zero past the end, without reading them; at least
    /// 56 bits are then available to [`MainBits::skip`].
    #[inline(always)]
    pub(crate) fn peek32(&mut self) -> u32 {
        self.refill();
        (self.cache >> 32) as u32
    }

    /// Drops `n` bits, at most those available since the last refill.
    #[inline(always)]
    pub(crate) fn skip(&mut self, n: u32) {
        // `have` is at most 63, so the shift stays below 64.
        let n = n.min(self.have);
        self.cache <<= n & 63;
        self.have -= n;
    }

    /// Reads `n` bits, 0 to 32.
    #[inline(always)]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let value = self.peek32() >> (32 - n.min(32));
        self.skip(n);
        value
    }
}

/// The line where each region of the big-values part ends.
fn regions(gi: &GranuleInfo, bands: &Bands, big_end: usize, mpeg1: bool) -> [usize; 3] {
    let unit_end = |units: usize| -> usize {
        let mut line = 0usize;
        let mut left = units;
        if gi.window_switching && gi.block_type == 2 {
            let long_bands = if gi.mixed {
                if mpeg1 { 8 } else { 6 }
            } else {
                0
            };
            for &w in bands.long.iter().take(long_bands) {
                if left == 0 {
                    return line;
                }
                line += usize::from(w);
                left -= 1;
            }
            let first_short = if gi.mixed { 3 } else { 0 };
            for &w in bands.short.iter().skip(first_short) {
                for _ in 0..3 {
                    if left == 0 {
                        return line;
                    }
                    line += usize::from(w);
                    left -= 1;
                }
            }
            line
        } else {
            for &w in bands.long.iter() {
                if left == 0 {
                    return line;
                }
                line += usize::from(w);
                left -= 1;
            }
            line
        }
    };
    let region0 = unit_end(gi.region0_count as usize + 1).min(big_end);
    if gi.window_switching {
        return [region0, big_end, big_end];
    }
    let region1 = unit_end(gi.region0_count as usize + gi.region1_count as usize + 2).min(big_end);
    [region0, region1, big_end]
}

/// `pow` (|x|^(4/3)) times `gain`, with the sign bit `sign` (1 negative).
#[inline(always)]
fn line(pow: &[f32], magnitude: u32, gain: f32, sign: u32) -> f32 {
    let value = pow.get(magnitude as usize).copied().unwrap_or(0.0) * gain;
    f32::from_bits(value.to_bits() | (sign << 31))
}

/// Decodes and requantizes the spectral values of one granule and channel
/// into `xr`, each line |is|^(4/3) times its gain from `gains` (runs of
/// `(end, gain)`, `layer3::Gains`); returns one past the last line
/// decoded. `end` is the bit position where the granule's part2_3 data
/// ends.
pub(crate) fn decode(
    bits: &mut MainBits,
    gi: &GranuleInfo,
    bands: &Bands,
    end: usize,
    mpeg1: bool,
    gains: &[(usize, f32)],
    xr: &mut [f32; 576],
) -> Result<usize, Error> {
    let pow = super::layer3::pow43();
    let last = pow.len().saturating_sub(1) as u32;
    // The run of the current line; band widths and region ends are even,
    // so a pair never straddles two runs.
    let mut run = 0usize;
    let (mut run_end, mut gain) = gains.first().copied().unwrap_or((576, 0.0));
    let big_end = (gi.big_values as usize * 2).min(576);
    let regions = regions(gi, bands, big_end, mpeg1);
    let mut line = 0usize;
    for (region, &region_end) in regions.iter().enumerate() {
        let select = gi.table_select.get(region).copied().unwrap_or(0) as usize;
        let (base, linbits) = tables::PAIR_TABLES
            .get(select)
            .copied()
            .unwrap_or((u32::MAX, 0));
        if base == u32::MAX {
            // Tables 0, 4 and 14 code only zeros.
            for slot in xr.get_mut(line..region_end.max(line)).unwrap_or_default() {
                *slot = 0.0;
            }
            line = region_end.max(line);
            continue;
        }
        let base = base as usize;
        let pairs = xr.get_mut(line..region_end.max(line)).unwrap_or_default();
        for pair in pairs.chunks_exact_mut(2) {
            // The granule's bits end here: its other lines are zero (a
            // `big_values` that its `part2_3_length` cannot hold).
            if bits.position() >= end {
                pair.fill(0.0);
                line += 2;
                continue;
            }
            while line >= run_end {
                run += 1;
                (run_end, gain) = gains.get(run).copied().unwrap_or((576, 0.0));
            }
            line += 2;
            // One peek holds the code (at most 19 bits) and, without
            // linbits, both sign bits.
            let peek = bits.peek32();
            let mut entry = tables::PAIR_LOOKUP
                .get(base + (peek >> 24) as usize)
                .copied()
                .unwrap_or(0);
            if entry & (1 << 31) != 0 {
                let width = (entry >> 24) & 0x7F;
                let offset = (entry & 0x00FF_FFFF) as usize;
                let index = ((peek << 8) >> (32 - width)) as usize;
                entry = tables::PAIR_LOOKUP
                    .get(base + offset + index)
                    .copied()
                    .unwrap_or(0);
            }
            let len = (entry >> 16) & 0x1F;
            let (x, y) = ((entry >> 4) & 0xF, entry & 0xF);
            if linbits == 0 || (x < 15 && y < 15) {
                // The sign bits follow the code, one per nonzero value; a
                // zero value takes none, and its "sign" leaves it zero.
                let nonzero_x = u32::from(x != 0);
                let nonzero_y = u32::from(y != 0);
                let sign_x = (peek << len) >> 31;
                let sign_y = (peek << (len + nonzero_x)) >> 31;
                if let [a, b] = pair {
                    *a = self::line(pow, x, gain, sign_x);
                    *b = self::line(pow, y, gain, sign_y);
                }
                bits.skip(len + nonzero_x + nonzero_y);
            } else {
                // hcod, then linbitsx and signx, then linbitsy and signy.
                bits.skip(len);
                for (slot, v) in pair.iter_mut().zip([x, y]) {
                    let mut v = v;
                    if v == 15 {
                        v += bits.read(linbits);
                    }
                    let sign = if v != 0 { bits.read(1) } else { 0 };
                    *slot = self::line(pow, v.min(last), gain, sign);
                }
            }
        }
        line = region_end.max(line);
    }
    // Count1: quadruples while bits remain.
    let quads: &[u32] = if gi.count1_table_b {
        &tables::QUAD_B_LOOKUP
    } else {
        &tables::QUAD_A_LOOKUP
    };
    while line + 4 <= 576 && bits.position() < end {
        let start = bits.position();
        // A count1 code is at most 6 bits, its sign bits at most 4.
        let peek = bits.peek32();
        let entry = quads.get((peek >> 24) as usize).copied().unwrap_or(0);
        let len = (entry >> 16) & 0xFF;
        let mut rest = peek << len;
        let mut used = len;
        let mut quad = [0.0f32; 4];
        for (i, slot) in quad.iter_mut().enumerate() {
            // A quadruple may straddle two runs.
            while line + i >= run_end {
                run += 1;
                (run_end, gain) = gains.get(run).copied().unwrap_or((576, 0.0));
            }
            let nonzero = (entry >> (3 - i)) & 1;
            *slot = self::line(pow, nonzero, gain, (rest >> 31) & nonzero);
            rest <<= nonzero;
            used += nonzero;
        }
        bits.skip(used);
        if bits.position() > end {
            // The last code ran past the granule's data: it is not part of
            // the spectrum.
            bits.seek(start);
            break;
        }
        if let Some(dst) = xr.get_mut(line..line + 4) {
            dst.copy_from_slice(&quad);
        }
        line += 4;
    }
    for slot in xr.get_mut(line..).unwrap_or_default() {
        *slot = 0.0;
    }
    Ok(line)
}
