//! Decoder-private CABAC site `hevc-decision`; input access remains checked Rust.

use core::arch::asm;

use super::{Cabac, Context, RANGE_TAB_LPS, TRANS_IDX_LPS, TRANS_IDX_MPS};
use crate::{Error, Result};

#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    lps: u16,
    normalized: u16,
    shift: u8,
    next_lps: u8,
    next_mps: u8,
    reserved: u8,
}

const _: () = assert!(core::mem::size_of::<Entry>() == 8);
const _: () = assert!(core::mem::offset_of!(Entry, normalized) == 2);
const _: () = assert!(core::mem::offset_of!(Entry, shift) == 4);
const _: () = assert!(core::mem::offset_of!(Entry, next_lps) == 5);
const _: () = assert!(core::mem::offset_of!(Entry, next_mps) == 6);

#[allow(clippy::indexing_slicing)]
const fn entries() -> [Entry; 256] {
    let mut table = [Entry {
        lps: 0,
        normalized: 0,
        shift: 0,
        next_lps: 0,
        next_mps: 0,
        reserved: 0,
    }; 256];
    let mut state = 0;
    while state < 64 {
        let mut column = 0;
        while column < 4 {
            let lps = RANGE_TAB_LPS[state][column] as u16;
            let shift = lps.leading_zeros() - 7;
            assert!(shift > 0);
            assert!(shift <= 7);
            assert!(256 + 64 * column as u16 - lps >= 128);
            table[state * 4 + column] = Entry {
                lps,
                normalized: lps << shift,
                shift: shift as u8,
                next_lps: TRANS_IDX_LPS[state],
                next_mps: TRANS_IDX_MPS[state],
                reserved: 0,
            };
            column += 1;
        }
        state += 1;
    }
    table
}

static TABLE: [Entry; 256] = entries();

/// Site `hevc-decision` reads only TABLE at an index below 256. Its integer
/// instructions are baseline x86-64; it writes no memory and touches no input.
#[inline(always)]
pub(super) fn decision(decoder: &mut Cabac<'_, '_>, context: &mut Context) -> Result<bool> {
    if context.state >= 64 {
        return Err(Error::InvalidData("cabac_context"));
    }
    let index = usize::from(context.state) * 4 + usize::from((decoder.range >> 6) & 3);
    let mut range = u32::from(decoder.range);
    let mut offset = u32::from(decoder.offset);
    let shift: u32;
    let next: u32;
    let lps: u8;
    unsafe {
        asm!(
            "movzx {temporary:e}, word ptr [{table} + 8*{index}]",
            "sub {range:e}, {temporary:e}",
            "mov {temporary:e}, {offset:e}",
            "sub {temporary:e}, {range:e}",
            "setae {lps}",
            "jae 2f",
            "movzx {next:e}, byte ptr [{table} + 8*{index} + 6]",
            "xor {shift:e}, {shift:e}",
            "cmp {range:e}, 256",
            "jae 3f",
            "shl {range:e}, 1",
            "mov {shift:e}, 1",
            "jmp 3f",
            "2:",
            "mov {offset:e}, {temporary:e}",
            "movzx {range:e}, word ptr [{table} + 8*{index} + 2]",
            "movzx {shift:e}, byte ptr [{table} + 8*{index} + 4]",
            "movzx {next:e}, byte ptr [{table} + 8*{index} + 5]",
            "3:",
            table = in(reg) TABLE.as_ptr(),
            index = in(reg) index,
            range = inout(reg) range,
            offset = inout(reg) offset,
            shift = out(reg) shift,
            next = out(reg) next,
            lps = out(reg_byte) lps,
            temporary = out(reg) _,
            options(pure, readonly, nostack),
        );
    }
    if shift == 0 {
        decoder.range = range as u16;
        context.state = next as u8;
        return Ok(context.mps);
    }
    let Some(bits) = decoder.input.cabac_window(shift) else {
        return decoder.decode_decision(context);
    };
    decoder.range = range as u16;
    decoder.offset = ((offset << shift) | u32::from(bits)) as u16;
    decoder.last_bit = bits & 1 != 0;
    if decoder.offset >= decoder.range {
        return Err(Error::InvalidData("cabac_offset"));
    }
    let bin = context.mps ^ (lps != 0);
    context.mps ^= lps != 0 && context.state == 0;
    context.state = next as u8;
    Ok(bin)
}
