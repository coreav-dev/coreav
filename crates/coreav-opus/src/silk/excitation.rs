//! The excitation of a SILK frame (RFC 6716 4.2.7.8): pulses coded in
//! 16-sample shell blocks, each block's count first, then the positions,
//! the extra LSBs and the signs, each for all blocks before the next.

use super::frame::Signal;
use crate::range::RangeDecoder;
use crate::tables::silk as t;

/// The samples of a shell block (4.2.7.8).
pub(crate) const BLOCK: usize = 16;

/// The most shell blocks of a frame: 20 ms at 16 kHz (Table 44).
pub(crate) const MAX_BLOCKS: usize = 20;

/// The pulse count that announces an extra LSB (4.2.7.8.2).
const MORE_LSBS: usize = 17;

/// At most this many LSBs per coefficient: the rate level after ten of
/// them cannot code another (4.2.7.8.2).
const MAX_LSBS: u32 = 10;

/// Reads the excitation of a frame of `len` samples into `out` as signed
/// integers (`e_raw`, 4.2.7.8.6), whole blocks: a 10 ms MB frame reads
/// 128 samples for its 120 (Table 44).
pub(crate) fn read(
    rd: &mut RangeDecoder<'_>,
    signal: Signal,
    high_offset: bool,
    len: usize,
    out: &mut [i32; BLOCK * MAX_BLOCKS],
) {
    let blocks = len.div_ceil(BLOCK).min(MAX_BLOCKS);
    let voiced = signal == Signal::Voiced;

    // The rate level (4.2.7.8.1), then each block's pulse count and its
    // number of LSBs (4.2.7.8.2).
    let level = t::RATE_LEVEL_ICDF
        .get(usize::from(voiced))
        .map_or(0, |icdf| rd.icdf(icdf, 8));
    let mut counts = [0usize; MAX_BLOCKS];
    let mut lsbs = [0u32; MAX_BLOCKS];
    for (count, extra) in counts.iter_mut().zip(lsbs.iter_mut()).take(blocks) {
        let mut icdf: &[u8] = t::PULSE_COUNT_ICDF.get(level).map_or(&[0], |row| row);
        *count = rd.icdf(icdf, 8);
        while *count == MORE_LSBS {
            *extra += 1;
            // Levels 9 and 10: the last row, and the same shifted by one
            // entry once ten LSBs leave no room for another.
            let last = t::PULSE_COUNT_ICDF.last().map_or(&[0][..], |row| row);
            icdf = last.get(usize::from(*extra == MAX_LSBS)..).unwrap_or(&[0]);
            *count = rd.icdf(icdf, 8);
        }
    }

    // The pulses' positions (4.2.7.8.3).
    for ((block, &count), _) in out.chunks_exact_mut(BLOCK).zip(&counts).zip(0..blocks) {
        block.fill(0);
        if count > 0 {
            shell(rd, count, block);
        }
    }

    // The LSBs (4.2.7.8.4), every coefficient of a block that has any.
    for ((block, &extra), _) in out.chunks_exact_mut(BLOCK).zip(&lsbs).zip(0..blocks) {
        if extra > 0 {
            for value in block.iter_mut() {
                for _ in 0..extra {
                    *value = *value << 1 | rd.icdf(&t::LSB_ICDF, 8) as i32;
                }
            }
        }
    }

    // The signs (4.2.7.8.5): one table per signal type, offset type and
    // block pulse count (LSBs not counted, 6 or more sharing one).
    let row = 7 * (2 * signal.index() + usize::from(high_offset));
    let signs = t::SIGN_ICDF.get(row..row + 7).unwrap_or(&[0; 7]);
    for (((block, &count), &extra), _) in out
        .chunks_exact_mut(BLOCK)
        .zip(&counts)
        .zip(&lsbs)
        .zip(0..blocks)
    {
        if count == 0 && extra == 0 {
            continue;
        }
        let icdf = [signs.get(count.min(6)).copied().unwrap_or(0), 0];
        for value in block.iter_mut().filter(|v| **v > 0) {
            if rd.icdf(&icdf, 8) == 0 {
                *value = -*value;
            }
        }
    }
}

/// Places a block's `count` pulses (4.2.7.8.3): each partition, 16 down to
/// 2 samples, codes how many of its pulses fall in its first half,
/// first half first (preorder).
fn shell(rd: &mut RangeDecoder<'_>, count: usize, block: &mut [i32]) {
    let mut halves = [0usize; 2];
    split(rd, count, &t::SHELL_SPLIT_16_ICDF, &mut halves);
    for (&eight, quarter) in halves.iter().zip(block.chunks_exact_mut(8)) {
        let mut quarters = [0usize; 2];
        split(rd, eight, &t::SHELL_SPLIT_8_ICDF, &mut quarters);
        for (&four, pair) in quarters.iter().zip(quarter.chunks_exact_mut(4)) {
            let mut pairs = [0usize; 2];
            split(rd, four, &t::SHELL_SPLIT_4_ICDF, &mut pairs);
            for (&two, samples) in pairs.iter().zip(pair.chunks_exact_mut(2)) {
                let mut ones = [0usize; 2];
                split(rd, two, &t::SHELL_SPLIT_2_ICDF, &mut ones);
                for (sample, &n) in samples.iter_mut().zip(&ones) {
                    *sample = n as i32;
                }
            }
        }
    }
}

/// Splits `count` pulses between two halves; nothing is read for none.
fn split(rd: &mut RangeDecoder<'_>, count: usize, table: &[u8], out: &mut [usize; 2]) {
    if count == 0 {
        *out = [0, 0];
        return;
    }
    let at = t::SHELL_OFFSETS.get(count).map_or(0, |&o| usize::from(o));
    let first = rd.icdf(table.get(at..).unwrap_or(&[0]), 8).min(count);
    *out = [first, count - first];
}
