//! The boundary strengths of a macroblock (`Kernels::strengths`): the
//! vector kernel and the dispatch against the reference, for motion from
//! arbitrary bytes (any vector, any reference index and slot, blocks that
//! differ or repeat), either neighbour present or not, intra or not, any
//! coefficient mask, and either transform size.
#![no_main]

use coreav_kernels_h264::{Auto, Kernels, Motion, Motions, Scalar, Simd, StrengthMb, StrengthSide};
use libfuzzer_sys::fuzz_target;

/// One block's motion from twelve bytes, in `Motion`'s field order.
fn motion(b: &[u8]) -> Motion {
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    let half = |i: usize| i16::from_le_bytes([at(i), at(i + 1)]);
    Motion {
        mv: [[half(0), half(2)], [half(4), half(6)]],
        ref_idx: [at(8).cast_signed(), at(9).cast_signed()],
        ref_slot: [at(10), at(11)],
    }
}

/// A macroblock's sixteen blocks from up to 16 x 12 bytes. Block `i` takes
/// its own bytes where bit `i % 8` of `pick` is set and the bytes are
/// there; otherwise it repeats the first block of its 8x8 quadrant, as
/// partitions do.
fn blocks(b: &[u8], pick: u8) -> [Motion; 16] {
    let mut out = [Motion::default(); 16];
    for i in 0..16 {
        out[i] = match b.get(12 * i..12 * i + 12) {
            Some(bytes) if (pick >> (i % 8)) & 1 == 1 || i == 0 => motion(bytes),
            _ => out[(i & !5) % 16],
        };
    }
    out
}

/// A macroblock's side from its flags (bit 0: sixteen motions, bits 1
/// and 2 both set: intra), blocks and coefficient mask.
fn side((flags, blocks, nz): &(u8, [Motion; 16], u16)) -> StrengthSide<'_> {
    StrengthSide {
        motion: if flags & 1 == 0 {
            Motions::One(&blocks[0])
        } else {
            Motions::Each(blocks)
        },
        nz: *nz,
        intra: flags & 6 == 6,
    }
}

fuzz_target!(|data: &[u8]| {
    // Per macroblock: a flags byte, a pick byte, two bytes of coefficient
    // mask, and 192 bytes of motion.
    const SIDE: usize = 4 + 192;
    let Some((&flags, rest)) = data.split_first() else {
        return;
    };
    let read = |k: usize| {
        let b = rest.get(SIDE * k..).unwrap_or_default();
        let at = |i: usize| b.get(i).copied().unwrap_or(0);
        let motion = b.get(4..).unwrap_or_default();
        (at(0), blocks(motion, at(1)), u16::from_le_bytes([at(2), at(3)]))
    };
    let (q, left, top) = (read(0), read(1), read(2));
    let mb = StrengthMb {
        q: side(&q),
        left: (flags & 1 == 1).then(|| side(&left)),
        top: (flags & 2 == 2).then(|| side(&top)),
        t8x8: flags & 4 == 4,
    };
    let want = Scalar.strengths(&mb);
    assert_eq!(Auto::detect().strengths(&mb), want, "{mb:?}");
    if let Some(simd) = Simd::detect() {
        assert_eq!(simd.strengths(&mb), want, "{mb:?}");
    }
});
