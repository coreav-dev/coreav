//! Site `decision`: DecodeDecision (clause 9.3.3.2.1) with RenormD
//! (clause 9.3.3.2.2) as one `asm!` block, for the x86-64-v3 instance
//! (`docs/design/h264.md`, "Entropy round: DecodeDecision in assembly").
//!
//! Its twin is [`Engine::decision`]: the same engine, the same tables, the
//! same results for every state the engine can reach, which is codIRange
//! 256 to 510 before a decision (a codIRange below 256 reads another
//! state's entry, in bounds, where the twin reads zeros). The block reads and
//! writes the safe engine's own `value`, `bits` and `range`, so nothing is
//! converted on the way in or out; the state's load and store, the bin's
//! test and the refill stay in safe Rust around it.
//!
//! The block reads two fixed tables in [`TABLES`] at indices bounded by
//! construction, writes no memory and never touches the slice data.

use core::arch::asm;
use core::mem::offset_of;

use super::V3;
use crate::cabac::{Engine, Source};
use crate::tables as t;

/// The tables the block reads, in one place so that one register addresses
/// both. Aligned to a cache line, so that no 8-byte `lps` entry straddles
/// two lines wherever the linker puts the table.
#[repr(C, align(64))]
struct Tables {
    /// One entry per context state `s` (any byte) and quarter `q` of
    /// codIRange (`codIRange >> 6`, 4 to 7 between decisions), at
    /// `4 * s + q`, of four 16-bit fields:
    ///
    /// 0. rangeLPS (Table 9-44);
    /// 1. rangeLPS renormalized, shifted left until it is 256 or more;
    /// 2. that shift;
    /// 3. the state after the less probable symbol (Table 9-45, with the
    ///    valMPS switch of clause 9.3.3.2.1.1), with the bin in bit 8.
    ///
    /// `q` is 4 to 7, so the entries of state `s` are the four after
    /// `4 * s + 3`; `4 * 255 + 7` is the last index any `s` below 256 and
    /// `q` below 8 reach.
    lps: [[u16; 4]; 1028],
    /// The state after the more probable symbol by state, with the bin in
    /// bit 8.
    mps: [u16; 256],
}

static TABLES: Tables = Tables {
    lps: lps_entries(),
    mps: mps_entries(),
};

const MPS_AT: usize = offset_of!(Tables, mps);

/// rangeLPS of the state byte `s` and the quarter `q` (4 to 7), as the
/// twin's table reads it: by `s & 127`, so for every byte.
#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn range_lps(s: usize, q: usize) -> u16 {
    t::RANGE_TAB_LPS_BY_STATE[((s & 127) << 2) | (q - 4)] as u16
}

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn lps_entries() -> [[u16; 4]; 1028] {
    let mut out = [[0u16; 4]; 1028];
    let mut s = 0;
    while s < 256 {
        let state = s & 127;
        let mut q = 4;
        while q < 8 {
            let lps = range_lps(s, q);
            // rangeLPS is 2 to 240, so the shift is 1 to 7.
            let shift = lps.leading_zeros() as u16 - 7;
            let bin = (state & 1) as u16 ^ 1;
            let next = t::NEXT_STATE[state | 128] as u16;
            out[4 * s + q] = [lps, lps << shift, shift, next | (bin << 8)];
            q += 1;
        }
        s += 1;
    }
    out
}

#[allow(clippy::indexing_slicing)] // Constant evaluation: out of range fails the build.
const fn mps_entries() -> [u16; 256] {
    let mut out = [0u16; 256];
    let mut s = 0;
    while s < 256 {
        let state = s & 127;
        out[s] = t::NEXT_STATE[state] as u16 | (((state & 1) as u16) << 8);
        s += 1;
    }
    out
}

/// DecodeDecision against the context state `state`, as
/// [`Engine::decision`] decodes it. `_v3` is the evidence that the CPU has
/// BMI2 (`rorx`, `shlx`).
#[inline(always)]
pub(super) fn decision(_v3: V3, e: &mut Engine, state: &mut u8, src: &mut Source<'_>) -> u32 {
    let mut value = e.value;
    let mut range = e.range;
    let mut bits = e.bits;
    let out: u64;
    // SAFETY (site `decision`): the block reads `TABLES` only, at
    // `8 * i` to `8 * i + 7` within `lps`, where `i = 4 * s + q` with `s`
    // a byte (below 256) and `q` masked to 0 to 7, so `i` is at most 1027,
    // the last entry; and at `2 * s` within `mps`, which has an entry for
    // every byte. It writes no memory and uses no stack. `_v3` proves the
    // CPU has BMI2. Every other operand is a register.
    unsafe {
        asm!(
            // q, then the entry's index: 4 * s + q.
            "rorx {i:e}, {r:e}, 6",
            "and {i:e}, 7",
            "lea {i}, [{i} + 4*{s}]",
            // The state after the more probable symbol, with its bin.
            "movzx {o:e}, word ptr [{t} + 2*{s} + {mps}]",
            // codIRange - rangeLPS, the more probable symbol's range, and
            // the same at codIOffset's scale. The 32-bit subtraction clears
            // the bits above `r`'s, which a 32-bit operand leaves undefined.
            "movzx {c:e}, word ptr [{t} + 8*{i}]",
            "sub {r:e}, {c:e}",
            "shlx {c}, {r:r}, {b:r}",
            // Its renormalization: one doubling where it is below 256.
            // Only bits 0 to 4 of the shift are clean: `shlx` reads those,
            // and the low byte is all `bits` takes. `shlx` reads bits 0 to 5
            // of `b`, whose bits above 31 are undefined.
            "rorx {n:e}, {r:e}, 8",
            "xor {n:e}, 1",
            "shlx {r:e}, {r:e}, {n:e}",
            // codIOffset - that, which borrows for the more probable
            // symbol. Otherwise the less probable one: codIOffset minus
            // it, and its range, shift and next state from the entry.
            "mov {s}, {v}",
            "sub {s}, {c}",
            "cmovae {v}, {s}",
            "cmovae {r:x}, word ptr [{t} + 8*{i} + 2]",
            "cmovae {n:x}, word ptr [{t} + 8*{i} + 4]",
            "cmovae {o:x}, word ptr [{t} + 8*{i} + 6]",
            // RenormD consumes the shift.
            "sub {b:l}, {n:l}",
            s = inout(reg) u64::from(*state) => _,
            t = in(reg) &TABLES,
            mps = const MPS_AT,
            v = inout(reg) value,
            r = inout(reg) range,
            b = inout(reg) bits,
            o = out(reg) out,
            i = out(reg) _,
            c = out(reg) _,
            n = out(reg) _,
            options(pure, readonly, nostack),
        );
    }
    e.value = value;
    e.range = range;
    e.bits = bits;
    *state = out as u8;
    e.reserve(src);
    (out >> 8) as u32 & 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry against the clause: Table 9-44's rangeLPS, the shift
    /// that RenormD applies to it, Table 9-45's transition with the valMPS
    /// switch, and the bin.
    #[test]
    fn the_tables_hold_what_the_clause_gives() {
        for s in 0..256usize {
            let state = s & 127;
            let mps = state & 1;
            for q in 4..8usize {
                let [lps, norm, shift, next] = TABLES.lps[4 * s + q];
                assert_eq!(
                    lps,
                    u16::from(t::RANGE_TAB_LPS_BY_STATE[(state << 2) | (q - 4)])
                );
                let mut r = lps;
                let mut n = 0;
                while r < 256 {
                    r <<= 1;
                    n += 1;
                }
                assert_eq!((norm, shift), (r, n), "state {s}, quarter {q}");
                assert_eq!(next >> 8, (mps ^ 1) as u16);
                assert_eq!(next & 255, u16::from(t::NEXT_STATE[state | 128]));
            }
            assert_eq!(
                TABLES.mps[s],
                u16::from(t::NEXT_STATE[state]) | ((mps as u16) << 8)
            );
        }
    }
}
