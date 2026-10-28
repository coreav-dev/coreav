//! The AVX-512 tier's boundary strengths (clause 8.7.2.1): the masks of
//! `crate::strengths` built a direction at a time in general registers,
//! and the motion of every boundary of a macroblock that carries more than
//! one motion compared at once, in vectors (`docs/design/h264.md`, "Kernel
//! round: boundary strengths in vector registers").
//!
//! Boundaries are numbered by the block after them, in raster order: bit
//! `i` of a direction's 16-bit mask is the boundary before block `i`,
//! across a vertical edge (`i` and `i - 1`) or a horizontal one (`i` and
//! `i - 4`), the blocks of edge 0 taken from the neighbour. The two
//! directions' masks make a word: vertical in bits 0 to 15, horizontal in
//! 16 to 31.
//!
//! The motion is compared in lanes of 32 bits, a block a lane and eight
//! blocks a vector, each of a block's three 32-bit fields in its own
//! vector: the vector of list 0, that of list 1, and that of the reference
//! indices and pictures. A lane of the last holds the bytes (ref_idx[0],
//! ref_idx[1], ref_slot[0], ref_slot[1]) as `Motion` stores them, which
//! [`canonical`] turns into (unused 0, unused 1, picture 0, picture 1): a
//! list's byte "unused" is 0xff where its index is negative, and its
//! picture is 0 there. Two blocks then refer to the same pictures with the
//! same lists where their canonical fields are equal, and with the lists
//! crossed where one equals the other with its lists swapped; the vectors
//! are compared for each list in use. A boundary's motion gives bS 0 where
//! either pairing matches and its vectors are close enough, which is
//! [`crate::strengths::motion_strength`]'s rule (the tests check every
//! case against it).

use core::arch::x86_64::{
    __m256i, _mm_loadu_si128, _mm_mask_blend_epi32, _mm_permutex2var_epi32, _mm_setr_epi32,
    _mm256_abs_epi16, _mm256_andnot_si256, _mm256_cmpeq_epi32_mask, _mm256_cmpgt_epi8,
    _mm256_loadu_si256, _mm256_mask_blend_epi16, _mm256_mask_mov_epi8,
    _mm256_mask_permutexvar_epi32, _mm256_mask_test_epi32_mask, _mm256_maskz_mov_epi8,
    _mm256_permutex2var_epi32, _mm256_set1_epi8, _mm256_set1_epi32, _mm256_setr_epi8,
    _mm256_setr_epi32, _mm256_setzero_si256, _mm256_shuffle_epi8, _mm256_slli_epi32,
    _mm256_srli_epi16, _mm256_storeu_si256, _mm256_subs_epi16, _mm256_testn_epi32_mask,
    _mm256_zextsi128_si256,
};

use crate::strengths::{Motion, Motions, StrengthMb, StrengthSide, motion_strength, same_motion};

/// The boundaries of edge 0 in the word of both directions' masks.
const EDGE0: u32 = 0x000f_1111;

/// [`crate::Kernels::strengths`] in this tier.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[target_feature(enable = "avx2,avx512f,avx512bw,avx512vl")]
pub(in crate::simd) unsafe fn strengths(mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
    let q = &mb.q;
    let (left, top) = (mb.left.as_ref(), mb.top.as_ref());
    let q_nz = u32::from(q.nz);
    let left_nz = left.map_or(0, |s| u32::from(s.nz));
    let top_nz = top.map_or(0, |s| u32::from(s.nz));
    // bS 2 where either block holds a coefficient: the block after the
    // boundary, or the one before it, which for edge 0 is the
    // neighbour's last column (bits 3, 7, 11, 15 to 0, 4, 8, 12) or last
    // row (bits 12 to 15 to 0 to 3).
    let coef_v = q_nz | ((q_nz << 1) & 0xeeee) | ((left_nz >> 3) & 0x1111);
    let coef_h = (q_nz | (q_nz << 4) | (top_nz >> 12)) & 0xffff;
    let coef = coef_v | (coef_h << 16);
    // The boundaries that are filtered: edges 0 and 2 only with the 8x8
    // transform, and edge 0 only with its neighbour.
    let mut on = if mb.t8x8 { 0x0f0f_5555 } else { 0xffff_ffff };
    if left.is_none() {
        on &= !0x1111;
    }
    if top.is_none() {
        on &= !0x000f_0000;
    }
    let (moved, three, four) = if q.intra {
        (0, !EDGE0, EDGE0)
    } else {
        let mut four = 0;
        if left.is_some_and(|s| s.intra) {
            four |= 0x1111;
        }
        if top.is_some_and(|s| s.intra) {
            four |= 0x000f_0000;
        }
        let one = |s: Option<&StrengthSide<'_>>| s.is_none_or(|s| s.motion.is_one());
        let moved = if q.motion.is_one() && one(left) && one(top) {
            heads_moved(q, left, top, coef)
        } else {
            // SAFETY: this function's features are the helper's contract.
            unsafe { split_moved(q.motion, left, top) }
        };
        (moved, 0, four)
    };
    // SAFETY: as above.
    unsafe { assemble(coef & on, moved & !coef & on, three & on, four & on) }
}

/// The boundaries of edge 0 whose motion gives bS 1 where the macroblock
/// and its neighbours carry one motion each: every internal boundary
/// compares a motion with itself. Each edge 0 compares two motions, once;
/// an edge settled by coefficients or an intra neighbour is not compared.
#[inline(always)]
fn heads_moved(
    q: &StrengthSide<'_>,
    left: Option<&StrengthSide<'_>>,
    top: Option<&StrengthSide<'_>>,
    coef: u32,
) -> u32 {
    let qm = q.motion.block(0);
    let edge = |p: Option<&StrengthSide<'_>>, bits: u32| match p {
        Some(p) if !p.intra && coef & bits != bits => {
            let pm = p.motion.block(0);
            if same_motion(pm, qm) {
                0
            } else {
                bits * u32::from(motion_strength(pm, qm))
            }
        }
        _ => 0,
    };
    edge(left, 0x1111) | edge(top, 0x000f_0000)
}

/// The three fields of eight blocks, a block a lane: list 0's vector,
/// list 1's, and the canonical reference fields.
type Fields = [__m256i; 3];

/// The boundaries whose motion gives bS 1, both directions, where the
/// macroblock or a neighbour carries more than one motion. Lanes of an
/// absent or intra neighbour hold anything; [`assemble`] masks them.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[inline(always)]
unsafe fn split_moved(
    q: Motions<'_>,
    left: Option<&StrengthSide<'_>>,
    top: Option<&StrengthSide<'_>>,
) -> u32 {
    // SAFETY (every block of this function): the caller's contract, and
    // the helpers' reads are inside the arrays they are handed.
    let [q_lo, q_hi] = unsafe { blocks(q) };
    let swap = unsafe { swap_lists() };
    let (qx_lo, qx_hi) = unsafe {
        (
            _mm256_shuffle_epi8(q_lo[2], swap),
            _mm256_shuffle_epi8(q_hi[2], swap),
        )
    };
    let column = match left {
        Some(s) => unsafe { column3(s.motion) },
        None => unsafe { absent() },
    };
    let row = match top {
        Some(s) => unsafe { row3(s.motion) },
        None => unsafe { absent() },
    };
    // The p block of each vertical boundary: the block before in its row,
    // or the left neighbour's block of that row (column lanes 0 to 3).
    let v_lo = unsafe { _mm256_setr_epi32(8, 0, 1, 2, 9, 4, 5, 6) };
    let v_hi = unsafe { _mm256_setr_epi32(10, 0, 1, 2, 11, 4, 5, 6) };
    // Of each horizontal one: the block above, or the top neighbour's
    // block of that column (row lanes 0 to 3).
    let h_lo = unsafe { _mm256_setr_epi32(8, 9, 10, 11, 0, 1, 2, 3) };
    let h_hi = unsafe { _mm256_setr_epi32(4, 5, 6, 7, 8, 9, 10, 11) };
    let mut p = [unsafe { _mm256_setzero_si256() }; 3];
    for (f, p) in p.iter_mut().enumerate() {
        *p = unsafe { _mm256_permutex2var_epi32(field(&q_lo, f), v_lo, field(&column, f)) };
    }
    let v = unsafe { moved8(&p, &q_lo, qx_lo) };
    for (f, p) in p.iter_mut().enumerate() {
        *p = unsafe { _mm256_permutex2var_epi32(field(&q_hi, f), v_hi, field(&column, f)) };
    }
    let v = v | unsafe { moved8(&p, &q_hi, qx_hi) } << 8;
    for (f, p) in p.iter_mut().enumerate() {
        *p = unsafe { _mm256_permutex2var_epi32(field(&q_lo, f), h_lo, field(&row, f)) };
    }
    let h = unsafe { moved8(&p, &q_lo, qx_lo) };
    for (f, p) in p.iter_mut().enumerate() {
        *p = unsafe { _mm256_permutex2var_epi32(field(&q_lo, f), h_hi, field(&q_hi, f)) };
    }
    let h = h | unsafe { moved8(&p, &q_hi, qx_hi) } << 8;
    v | h << 16
}

/// Field `f` of `fields`.
#[inline(always)]
fn field(fields: &Fields, f: usize) -> __m256i {
    match fields.get(f) {
        Some(&x) => x,
        None => fields[0],
    }
}

/// The eight boundaries (lanes) of one vector whose motion gives bS 1,
/// as bits: `p` the fields of the blocks before them, `q` those after,
/// and `qx` the reference field of `q` with its lists swapped.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[inline(always)]
unsafe fn moved8(p: &Fields, q: &Fields, qx: __m256i) -> u32 {
    // SAFETY: the caller's contract.
    unsafe {
        let [p0, p1, pr] = *p;
        let [q0, q1, qr] = *q;
        let straight = _mm256_cmpeq_epi32_mask(pr, qr);
        let crossed = _mm256_cmpeq_epi32_mask(pr, qx);
        // Where a pairing's references match, p uses a list exactly where
        // q uses the list it pairs with.
        let used0 = _mm256_testn_epi32_mask(pr, _mm256_set1_epi32(0xff));
        let used1 = _mm256_testn_epi32_mask(pr, _mm256_set1_epi32(0xff00));
        let straight_far = far(straight & used0, p0, q0) | far(straight & used1, p1, q1);
        let crossed_far = far(crossed & used0, p0, q1) | far(crossed & used1, p1, q0);
        let close = (straight & !straight_far) | (crossed & !crossed_far);
        u32::from(!close)
    }
}

/// The lanes of `k` where a component of vector `a` lies 4 or more
/// quarter samples from that of `b`. The saturating difference keeps the
/// magnitude of any difference of 32768 or more at 32767 or above, and
/// the absolute value of -32768, read without sign, is 32768.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F and VL.
#[inline(always)]
unsafe fn far(k: u8, a: __m256i, b: __m256i) -> u8 {
    // SAFETY: the caller's contract.
    unsafe {
        let d = _mm256_srli_epi16(_mm256_abs_epi16(_mm256_subs_epi16(a, b)), 2);
        _mm256_mask_test_epi32_mask(k, d, d)
    }
}

/// The byte shuffle that swaps the lists of a reference field.
///
/// # Safety
///
/// The CPU must support AVX.
#[inline(always)]
unsafe fn swap_lists() -> __m256i {
    // SAFETY: the caller's contract.
    unsafe {
        _mm256_setr_epi8(
            1, 0, 3, 2, 5, 4, 7, 6, 9, 8, 11, 10, 13, 12, 15, 14, 1, 0, 3, 2, 5, 4, 7, 6, 9, 8, 11,
            10, 13, 12, 15, 14,
        )
    }
}

/// The reference fields of eight blocks as [`Motion`] stores them, made
/// canonical: (unused 0, unused 1, picture 0, picture 1).
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 BW and VL.
#[inline(always)]
unsafe fn canonical(r: __m256i) -> __m256i {
    // SAFETY: the caller's contract.
    unsafe {
        // 0xff in the bytes of negative indices (and of slots of 128 or
        // more, which the shift below drops and the blend does not take).
        let unused = _mm256_cmpgt_epi8(_mm256_setzero_si256(), r);
        let pictures = _mm256_andnot_si256(_mm256_slli_epi32(unused, 16), r);
        _mm256_mask_blend_epi16(0xaaaa, unused, pictures)
    }
}

/// The fields of a macroblock's sixteen blocks, blocks 0 to 7 and 8 to 15.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[inline(always)]
unsafe fn blocks(m: Motions<'_>) -> [Fields; 2] {
    // SAFETY (every block): the caller's contract; the reads are inside
    // the array, as `load` says.
    match m {
        Motions::One(one) => {
            let f = unsafe { broadcast(one) };
            [f, f]
        }
        Motions::Each(all) => {
            let r = unsafe { load(all) };
            let [r0, r1, r2, r3, r4, r5] = r;
            unsafe { [deinterleave(r0, r1, r2), deinterleave(r3, r4, r5)] }
        }
    }
}

/// The 192 bytes of sixteen blocks, as six vectors: block `i`'s fields are
/// 32-bit lanes `3i` to `3i + 2` of their concatenation.
///
/// # Safety
///
/// The CPU must support AVX.
#[inline(always)]
unsafe fn load(all: &[Motion; 16]) -> [__m256i; 6] {
    let at = all.as_ptr().cast::<__m256i>();
    // SAFETY: `Motion` is `repr(C)`, twelve bytes without padding, so the
    // array is 192 initialised bytes from `at`; the six unaligned reads of
    // 32 bytes at offsets 0 to 160 end at 192.
    unsafe {
        [
            _mm256_loadu_si256(at),
            _mm256_loadu_si256(at.add(1)),
            _mm256_loadu_si256(at.add(2)),
            _mm256_loadu_si256(at.add(3)),
            _mm256_loadu_si256(at.add(4)),
            _mm256_loadu_si256(at.add(5)),
        ]
    }
}

/// The fields of eight blocks from the three vectors that hold them,
/// 24 lanes: field `f` of block `j` is lane `3j + f`.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[inline(always)]
unsafe fn deinterleave(a: __m256i, b: __m256i, c: __m256i) -> Fields {
    // SAFETY: the caller's contract.
    unsafe {
        // Lanes 0 to 15 from `a` and `b`, the rest from `c` (lane - 16).
        let mv0 = _mm256_mask_permutexvar_epi32(
            _mm256_permutex2var_epi32(a, _mm256_setr_epi32(0, 3, 6, 9, 12, 15, 0, 0), b),
            0xc0,
            _mm256_setr_epi32(0, 0, 0, 0, 0, 0, 2, 5),
            c,
        );
        let mv1 = _mm256_mask_permutexvar_epi32(
            _mm256_permutex2var_epi32(a, _mm256_setr_epi32(1, 4, 7, 10, 13, 0, 0, 0), b),
            0xe0,
            _mm256_setr_epi32(0, 0, 0, 0, 0, 0, 3, 6),
            c,
        );
        let refs = _mm256_mask_permutexvar_epi32(
            _mm256_permutex2var_epi32(a, _mm256_setr_epi32(2, 5, 8, 11, 14, 0, 0, 0), b),
            0xe0,
            _mm256_setr_epi32(0, 0, 0, 0, 0, 1, 4, 7),
            c,
        );
        [mv0, mv1, canonical(refs)]
    }
}

/// One motion's fields in every lane.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 BW and VL.
#[inline(always)]
unsafe fn broadcast(m: &Motion) -> Fields {
    let mv = |v: [i16; 2]| {
        let [x, y] = v.map(|c| u32::from(c.cast_unsigned()));
        (x | (y << 16)).cast_signed()
    };
    let [i0, i1] = m.ref_idx.map(|r| u32::from(r.cast_unsigned()));
    let [s0, s1] = m.ref_slot.map(u32::from);
    let refs = (i0 | (i1 << 8) | (s0 << 16) | (s1 << 24)).cast_signed();
    // SAFETY: the caller's contract.
    unsafe {
        [
            _mm256_set1_epi32(mv(m.mv[0])),
            _mm256_set1_epi32(mv(m.mv[1])),
            canonical(_mm256_set1_epi32(refs)),
        ]
    }
}

/// The fields of a missing neighbour's blocks: never read into a result.
///
/// # Safety
///
/// The CPU must support AVX.
#[inline(always)]
unsafe fn absent() -> Fields {
    // SAFETY: the caller's contract.
    unsafe { [_mm256_setzero_si256(); 3] }
}

/// The fields of a left neighbour's last column, blocks 3, 7, 11 and 15,
/// in lanes 0 to 3.
///
/// Read as four 16-byte pieces that lie apart in memory: two adjacent
/// 32-byte reads that feed one two-source permute are what the compiler
/// may widen into a 64-byte read and a 512-bit permute, which this tier
/// avoids (`docs/design/h264.md`, "Kernel round: boundary strengths in
/// vector registers").
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[inline(always)]
unsafe fn column3(m: Motions<'_>) -> Fields {
    let all = match m {
        // SAFETY: the caller's contract.
        Motions::One(one) => return unsafe { broadcast(one) },
        Motions::Each(all) => all,
    };
    let at = all.as_ptr().cast::<i32>();
    // SAFETY: the caller's contract. `Motion` is `repr(C)`, twelve bytes
    // without padding, so the array is 48 initialised lanes of 32 bits
    // from `at`; the reads of four lanes from lanes 8, 20, 32 and 44 end
    // at lane 48. Block 4r + 3 is lanes 12r + 9 to 12r + 11, lanes 1 to 3
    // of the read from lane 12r + 8.
    unsafe {
        let [x0, x1, x2, x3] = [8, 20, 32, 44].map(|l| _mm_loadu_si128(at.add(l).cast()));
        // Field k of the four blocks: lane 1 + k of each read, the first
        // two reads' into lanes 0 and 1, the last two's into lanes 2 and 3.
        let field = |k: i32| {
            let upper = _mm_permutex2var_epi32(x0, _mm_setr_epi32(1 + k, 5 + k, 0, 0), x1);
            let lower = _mm_permutex2var_epi32(x2, _mm_setr_epi32(0, 0, 1 + k, 5 + k), x3);
            _mm256_zextsi128_si256(_mm_mask_blend_epi32(0b1100, upper, lower))
        };
        [field(0), field(1), canonical(field(2))]
    }
}

/// The fields of a top neighbour's last row, blocks 12 to 15, in lanes 0
/// to 3.
///
/// Read as 32 bytes and 16 bytes, for the reason [`column3`] gives.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW and VL.
#[inline(always)]
unsafe fn row3(m: Motions<'_>) -> Fields {
    let all = match m {
        // SAFETY: the caller's contract.
        Motions::One(one) => return unsafe { broadcast(one) },
        Motions::Each(all) => all,
    };
    let at = all.as_ptr().cast::<i32>();
    // SAFETY: the caller's contract. As in `column3`, the array is 48
    // lanes from `at`; the read of eight lanes from lane 36 and that of
    // four from lane 44 end at lanes 44 and 48. Field k of block 12 + j is
    // lane 36 + 3j + k: lane 3j + k of the two reads together.
    unsafe {
        let a = _mm256_loadu_si256(at.add(36).cast());
        let b = _mm256_zextsi128_si256(_mm_loadu_si128(at.add(44).cast()));
        let field = |k: i32| {
            _mm256_permutex2var_epi32(a, _mm256_setr_epi32(k, 3 + k, 6 + k, 9 + k, 0, 0, 0, 0), b)
        };
        [field(0), field(1), canonical(field(2))]
    }
}

/// The words of [`crate::Kernels::strengths`] from the masks of the
/// boundaries that take bS 2, 1, 3 and 4, each a later one overriding the
/// earlier.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 BW and VL.
#[inline(always)]
unsafe fn assemble(two: u32, one: u32, three: u32, four: u32) -> [[u32; 4]; 2] {
    let mut out = [[0u32; 4]; 2];
    // SAFETY: the caller's contract; `out` is 32 bytes, which the one
    // unaligned write fills.
    unsafe {
        let v = _mm256_maskz_mov_epi8(two, _mm256_set1_epi8(2));
        let v = _mm256_mask_mov_epi8(v, one, _mm256_set1_epi8(1));
        let v = _mm256_mask_mov_epi8(v, three, _mm256_set1_epi8(3));
        let v = _mm256_mask_mov_epi8(v, four, _mm256_set1_epi8(4));
        // A vertical boundary's bit is 4s + e for segment s of edge e; its
        // word takes the edge's segments together, byte 4e + s. The
        // horizontal boundaries are in that order already.
        let v = _mm256_shuffle_epi8(
            v,
            _mm256_setr_epi8(
                0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9,
                10, 11, 12, 13, 14, 15,
            ),
        );
        _mm256_storeu_si256(out.as_mut_ptr().cast(), v);
    }
    out
}
