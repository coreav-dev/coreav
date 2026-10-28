//! The AVX-512 tier of the x86-64 kernels: the same kernels as `x86`, for
//! the CPUs that also have AVX-512 F, BW, VL and VBMI (Ice Lake and later,
//! Zen 4 and later), which [`super::Simd::detect`] establishes.
//!
//! Design (`docs/design/h264.md`, "Kernel round: an AVX-512 tier"):
//!
//! - Vectors stay 256 bits wide. A luma edge is sixteen lines, one 16-bit
//!   lane each, which is one 256-bit vector; what the tier adds is the
//!   mask registers, the byte permutes of VBMI and sixteen more vector
//!   registers, not width. 512-bit instructions would also move the core
//!   to a lower clock on some of these parts for a while after they run.
//!   The compiler may still widen two 256-bit operations into one on a
//!   `zmm` register, since nothing tunes these functions against it; the
//!   CI check `vector-width` fails on any `zmm` register in the decoder.
//! - Every kernel gives the samples of the AVX2 kernel and so of the
//!   reference, for every input the trait allows; a shape this tier has no
//!   kernel for goes to the AVX2 kernel, unchanged.
//! - The algorithms come from the specification's equations and the
//!   reference in `scalar.rs`, and the instruction choices from the Intel
//!   instruction set reference, never from another decoder.
//!
//! Its `unsafe` is of the kinds `x86` has (`docs/unsafe-sites.md`): calls
//! into `#[target_feature]` functions, which a `Simd` with its AVX-512 flag
//! set proves the CPU supports; register intrinsics; and loads and stores
//! of a standard edge's rows through the pointer `x86::rows_at_mut`
//! returns after one check of the whole edge's extent.

use core::arch::x86_64::{
    __m256i, __mmask16, _mm_castsi128_pd, _mm_cvtsd_f64, _mm_loadu_si128, _mm_storel_epi64,
    _mm_storeu_si128, _mm_unpackhi_pd, _mm256_abs_epi16, _mm256_add_epi16, _mm256_and_si256,
    _mm256_castsi256_si128, _mm256_cvtepu8_epi16, _mm256_extracti128_si256, _mm256_loadu_si256,
    _mm256_mask_add_epi16, _mm256_mask_cmpgt_epi16_mask, _mm256_mask_cmplt_epi16_mask,
    _mm256_mask_mov_epi16, _mm256_mask_sub_epi16, _mm256_maskz_permutex2var_epi8, _mm256_max_epi16,
    _mm256_min_epi16, _mm256_packus_epi16, _mm256_permute4x64_epi64, _mm256_permutex2var_epi8,
    _mm256_set_epi64x, _mm256_set1_epi16, _mm256_setzero_si256, _mm256_slli_epi16,
    _mm256_srai_epi16, _mm256_srli_epi16, _mm256_sub_epi16, _mm256_test_epi16_mask,
};

use super::x86;
use crate::EdgeParams;
use crate::views::{EdgeMut, Reach};

mod strengths;
pub(super) use strengths::strengths;

/// The deblocking filter of one edge (clauses 8.7.2.3 and 8.7.2.4): a
/// standard luma edge (sixteen lines) in this tier, every other edge in
/// the AVX2 kernel. Inlined into the caller, where the plane and length
/// of the edge are usually constants and the choice folds, as in
/// `x86::filter_edge`.
///
/// # Safety
///
/// The CPU must support AVX2, AVX-512 F, BW, VL and VBMI.
#[inline(always)]
pub(super) unsafe fn filter_edge(edge: EdgeMut<'_>, params: &EdgeParams) {
    // As in `x86::filter_edge`: the lines run along a row exactly where
    // `across` is not 1.
    let rows = edge.across() != 1;
    // SAFETY: the caller guarantees the features.
    unsafe {
        match (edge.reach(), rows, edge.lines()) {
            (Reach::Luma, true, 16) => luma_rows(edge, params),
            (Reach::Luma, false, 16) => luma_columns(edge, params),
            _ => x86::filter_edge(edge, params),
        }
    }
}

/// The luma filter of sixteen lines (clauses 8.7.2.3 and 8.7.2.4), with
/// the decisions in mask registers.
///
/// `s` holds p3, p2, p1, p0, q0, q1, q2 and q3, one 16-bit lane per line,
/// and is written back with the filtered samples; `alpha4` is
/// `(alpha >> 2) + 2` of clause 8.7.2.4. Returns whether any line was
/// filtered: where none was, `s` is unchanged and nothing needs storing.
///
/// p0 and q0 of a line filtered with bS below 4 may leave 0 to 255 here;
/// the caller's saturating pack is their Clip1 of clause 8.7.2.3.
/// Every other output stays inside the range by construction.
#[inline]
#[target_feature(enable = "avx2,avx512f,avx512bw,avx512vl")]
unsafe fn filter_luma16(
    s: &mut [__m256i; 8],
    alpha: __m256i,
    beta: __m256i,
    alpha4: __m256i,
    bs: __m256i,
    tc0: __m256i,
) -> bool {
    // SAFETY: the features are available; these are register operations.
    unsafe {
        let [p3, p2, p1, p0, q0, q1, q2, q3] = *s;
        let diff = |a: __m256i, b: __m256i| _mm256_abs_epi16(_mm256_sub_epi16(a, b));
        let edge_step = diff(p0, q0);
        // filterSamplesFlag (Equation 8-468), and bS = 0 filters nothing:
        // each compare is made only in the lanes the one before it kept.
        let filtered: __mmask16 = _mm256_test_epi16_mask(bs, bs);
        let filtered = _mm256_mask_cmplt_epi16_mask(filtered, edge_step, alpha);
        let filtered = _mm256_mask_cmplt_epi16_mask(filtered, diff(p1, p0), beta);
        let filtered = _mm256_mask_cmplt_epi16_mask(filtered, diff(q1, q0), beta);
        if filtered == 0 {
            return false;
        }
        // ap and aq below beta (clause 8.7.2.3), and the lines
        // of bS 4, all inside the filtered lines.
        let ap = _mm256_mask_cmplt_epi16_mask(filtered, diff(p2, p0), beta);
        let aq = _mm256_mask_cmplt_epi16_mask(filtered, diff(q2, q0), beta);
        let strong = _mm256_mask_cmpgt_epi16_mask(filtered, bs, _mm256_set1_epi16(3));
        let weak = filtered & !strong;
        let (mut new_p2, mut new_p1, mut new_p0) = (p2, p1, p0);
        let (mut new_q0, mut new_q1, mut new_q2) = (q0, q1, q2);
        let zero = _mm256_setzero_si256();
        let two = _mm256_set1_epi16(2);
        let four = _mm256_set1_epi16(4);

        // bS below 4 (clause 8.7.2.3).
        if weak != 0 {
            let one = _mm256_set1_epi16(1);
            // tC = tC0 + (ap < beta) + (aq < beta), for luma.
            let tc = _mm256_mask_add_epi16(tc0, ap, tc0, one);
            let tc = _mm256_mask_add_epi16(tc, aq, tc, one);
            // delta = Clip3(-tC, tC, (((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).
            let raw = _mm256_srai_epi16(
                _mm256_add_epi16(
                    _mm256_add_epi16(
                        _mm256_slli_epi16(_mm256_sub_epi16(q0, p0), 2),
                        _mm256_sub_epi16(p1, q1),
                    ),
                    four,
                ),
                3,
            );
            let delta = _mm256_max_epi16(_mm256_min_epi16(raw, tc), _mm256_sub_epi16(zero, tc));
            new_p0 = _mm256_mask_add_epi16(p0, weak, p0, delta);
            new_q0 = _mm256_mask_sub_epi16(q0, weak, q0, delta);
            // p1 and q1 (clause 8.7.2.3): (p2 + ((p0 + q0 + 1) >> 1) -
            // (p1 << 1)) >> 1 is ((2 p2 + p0 + q0 + 1) >> 2) - p1 on
            // integers: p1 << 1 is even, and halving twice floors as
            // quartering once does.
            let sum = _mm256_add_epi16(_mm256_add_epi16(p0, q0), one);
            let low = _mm256_sub_epi16(zero, tc0);
            let clamp = |v: __m256i| _mm256_max_epi16(_mm256_min_epi16(v, tc0), low);
            let step = |x2: __m256i, x1: __m256i| {
                let quarter = _mm256_srli_epi16(_mm256_add_epi16(_mm256_add_epi16(x2, x2), sum), 2);
                clamp(_mm256_sub_epi16(quarter, x1))
            };
            let (step_p, step_q) = (step(p2, p1), step(q2, q1));
            new_p1 = _mm256_mask_add_epi16(p1, weak & ap, p1, step_p);
            new_q1 = _mm256_mask_add_epi16(q1, weak & aq, q1, step_q);
        }

        // bS of 4 (clause 8.7.2.4). Every sum stays within 0 to 255 once
        // shifted.
        if strong != 0 {
            // The long filter of clause 8.7.2.4 where ap (aq) is below beta and
            // |p0 - q0| < (alpha >> 2) + 2.
            let long = _mm256_mask_cmplt_epi16_mask(strong, edge_step, alpha4);
            // One side's outputs: x3 to x0 its samples outwards from the
            // edge, y0 and y1 the other side's, `current` its p1 (q1) and
            // p0 (q0) after the filter of bS below 4, `long` its lines of
            // the long filter. Returns its p2, p1 and p0 (q2, q1, q0).
            let side = |[x3, x2, x1, x0]: [__m256i; 4],
                        [y0, y1]: [__m256i; 2],
                        [now1, now0]: [__m256i; 2],
                        long: __mmask16| {
                // x1 + x0 + y0, which the three long outputs share.
                let inner = _mm256_add_epi16(_mm256_add_epi16(x1, x0), y0);
                // (2 x1 + x0 + y1 + 2) >> 2 where the long filter does not
                // apply...
                let short0 = _mm256_srli_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(_mm256_add_epi16(x1, x1), x0),
                        _mm256_add_epi16(y1, two),
                    ),
                    2,
                );
                // ... and (x2 + 2 x1 + 2 x0 + 2 y0 + y1 + 4) >> 3 where it
                // does.
                let long0 = _mm256_srli_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(_mm256_add_epi16(inner, inner), x2),
                        _mm256_add_epi16(y1, four),
                    ),
                    3,
                );
                let out0 =
                    _mm256_mask_mov_epi16(_mm256_mask_mov_epi16(now0, strong, short0), long, long0);
                // (x2 + x1 + x0 + y0 + 2) >> 2
                let long1 =
                    _mm256_srli_epi16(_mm256_add_epi16(_mm256_add_epi16(x2, inner), two), 2);
                // (2 x3 + 3 x2 + x1 + x0 + y0 + 4) >> 3
                let long2 = _mm256_srli_epi16(
                    _mm256_add_epi16(
                        _mm256_add_epi16(_mm256_slli_epi16(_mm256_add_epi16(x3, x2), 1), x2),
                        _mm256_add_epi16(inner, four),
                    ),
                    3,
                );
                [
                    _mm256_mask_mov_epi16(x2, long, long2),
                    _mm256_mask_mov_epi16(now1, long, long1),
                    out0,
                ]
            };
            [new_p2, new_p1, new_p0] =
                side([p3, p2, p1, p0], [q0, q1], [new_p1, new_p0], long & ap);
            [new_q2, new_q1, new_q0] =
                side([q3, q2, q1, q0], [p0, p1], [new_q1, new_q0], long & aq);
        }
        *s = [p3, new_p2, new_p1, new_p0, new_q0, new_q1, new_q2, q3];
        true
    }
}

/// The thresholds of a luma edge as lanes: alpha, beta, `(alpha >> 2) +
/// 2`, and the boundary strength and tC0 of each line, built in vector
/// registers: alpha and beta are broadcast from one 16-bit value.
#[inline]
#[target_feature(enable = "avx2,avx512f,avx512bw,avx512vl")]
unsafe fn thresholds(params: &EdgeParams) -> [__m256i; 5] {
    // SAFETY: the features are available; these are register operations.
    unsafe {
        let pair = _mm256_set1_epi16(i16::from_le_bytes([params.alpha, params.beta]));
        let alpha = _mm256_and_si256(pair, _mm256_set1_epi16(0xff));
        let beta = _mm256_srli_epi16(pair, 8);
        let alpha4 = _mm256_add_epi16(_mm256_srli_epi16(alpha, 2), _mm256_set1_epi16(2));
        let (bs, tc0) = x86::spread16(params);
        [alpha, beta, alpha4, bs, tc0]
    }
}

/// A horizontal luma edge of sixteen lines: each sample position is a
/// run of sixteen bytes along a row, so no transpose is needed.
#[target_feature(enable = "avx2,avx512f,avx512bw,avx512vl")]
unsafe fn luma_rows(mut edge: EdgeMut<'_>, params: &EdgeParams) {
    // A segment whose boundary strength is zero is not filtered (clause
    // 8.7.2.1), and most interior edges are all such.
    if u32::from_le_bytes(params.bs) == 0 {
        return;
    }
    let across = edge.across();
    let (data, q0) = edge.window();
    // The eight rows p3 to q3, sixteen bytes each.
    let Some(base) = x86::rows_at_mut(data, q0.wrapping_sub(4 * across), across, 8, 16) else {
        // SAFETY: the features include AVX2.
        return unsafe { x86::filter_edge(edge, params) };
    };
    // SAFETY: the features are available. Every access is sixteen bytes
    // from the start of one of the eight rows `rows_at_mut` checked, none
    // needing alignment.
    unsafe {
        let [alpha, beta, alpha4, bs, tc0] = thresholds(params);
        let mut lanes = [_mm256_setzero_si256(); 8];
        for (j, slot) in lanes.iter_mut().enumerate() {
            *slot = _mm256_cvtepu8_epi16(_mm_loadu_si128(base.add(j * across).cast()));
        }
        if !filter_luma16(&mut lanes, alpha, beta, alpha4, bs, tc0) {
            return;
        }
        // Back to bytes, two rows a pack; the pack works within each half
        // of the vector, and the permute puts the first row in the low
        // half. p3 and q3 never change.
        for (pair, j) in [
            (lanes[1], lanes[2]),
            (lanes[3], lanes[4]),
            (lanes[5], lanes[6]),
        ]
        .into_iter()
        .zip([1, 3, 5])
        {
            let packed =
                _mm256_permute4x64_epi64(_mm256_packus_epi16(pair.0, pair.1), 0b11_01_10_00);
            _mm_storeu_si128(base.add(j * across).cast(), _mm256_castsi256_si128(packed));
            _mm_storeu_si128(
                base.add((j + 1) * across).cast(),
                _mm256_extracti128_si256(packed, 1),
            );
        }
    }
}

/// Byte indices for [`luma_columns`]' transposes, built once at compile
/// time. `TO_POSITIONS` takes two vectors of four lines each (eight bytes
/// a line, p3 first) to four sample positions of those eight lines (eight
/// bytes a position, line first); the second index does the same for the
/// other four positions.
#[allow(
    clippy::indexing_slicing,
    reason = "evaluated at compile time: an index out of range fails the build"
)]
const TO_POSITIONS: [[u8; 32]; 2] = {
    let mut idx = [[0u8; 32]; 2];
    let mut half = 0;
    while half < 2 {
        let mut j = 0;
        while j < 4 {
            let mut line = 0;
            while line < 8 {
                idx[half][j * 8 + line] = (line * 8 + half * 4 + j) as u8;
                line += 1;
            }
            j += 1;
        }
        half += 1;
    }
    idx
};

/// Byte indices that take the four-positions vectors of lines 0 to 7
/// and of lines 8 to 15 ([`TO_POSITIONS`]) to one sample position of all
/// sixteen lines, in the low byte of each 16-bit lane; position `j` of
/// the four is index `j`.
#[allow(
    clippy::indexing_slicing,
    reason = "evaluated at compile time: an index out of range fails the build"
)]
const TO_LANES: [[u8; 32]; 4] = {
    let mut idx = [[0u8; 32]; 4];
    let mut j = 0;
    while j < 4 {
        let mut lane = 0;
        while lane < 16 {
            let from = if lane < 8 {
                j * 8 + lane
            } else {
                32 + j * 8 + lane - 8
            };
            idx[j][2 * lane] = from as u8;
            lane += 1;
        }
        j += 1;
    }
    idx
};

/// Byte indices for the way back. A pack of positions `2m` and `2m + 1`
/// holds line `L` of position `2m + e` at byte `(L / 8) * 16 + e * 8 +
/// L % 8`. `TO_QUARTERS[h]` takes two such packs (positions 0 to 3, or 4
/// to 7) to those four positions of lines `8h` to `8h + 7`, four bytes a
/// line.
#[allow(
    clippy::indexing_slicing,
    reason = "evaluated at compile time: an index out of range fails the build"
)]
const TO_QUARTERS: [[u8; 32]; 2] = {
    let mut idx = [[0u8; 32]; 2];
    let mut h = 0;
    while h < 2 {
        let mut line = 0;
        while line < 8 {
            let mut j = 0;
            while j < 4 {
                idx[h][line * 4 + j] = ((j / 2) * 32 + h * 16 + (j % 2) * 8 + line) as u8;
                j += 1;
            }
            line += 1;
        }
        h += 1;
    }
    idx
};

/// Byte indices that take the quarters of positions 0 to 3 and 4 to 7 of
/// eight lines ([`TO_QUARTERS`]) to four whole lines, eight bytes each:
/// `TO_LINES[g]` gives lines `4g` to `4g + 3` of the eight.
#[allow(
    clippy::indexing_slicing,
    reason = "evaluated at compile time: an index out of range fails the build"
)]
const TO_LINES: [[u8; 32]; 2] = {
    let mut idx = [[0u8; 32]; 2];
    let mut g = 0;
    while g < 2 {
        let mut r = 0;
        while r < 4 {
            let line = 4 * g + r;
            let mut j = 0;
            while j < 8 {
                idx[g][r * 8 + j] = if j < 4 {
                    line * 4 + j
                } else {
                    32 + line * 4 + j - 4
                } as u8;
                j += 1;
            }
            r += 1;
        }
        g += 1;
    }
    idx
};

/// A vertical luma edge of sixteen lines: each line is eight bytes of a
/// row, p3 to q3. The lines are transposed into one vector per sample
/// position with two rounds of byte permutes, filtered, and permuted back
/// the same way.
#[target_feature(enable = "avx2,avx512f,avx512bw,avx512vl,avx512vbmi")]
unsafe fn luma_columns(mut edge: EdgeMut<'_>, params: &EdgeParams) {
    if u32::from_le_bytes(params.bs) == 0 {
        return;
    }
    let along = edge.along();
    let (data, q0) = edge.window();
    // Sixteen rows of eight bytes, from p3 of line 0.
    let Some(base) = x86::rows_at_mut(data, q0.wrapping_sub(4), along, 16, 8) else {
        // SAFETY: the features include AVX2.
        return unsafe { x86::filter_edge(edge, params) };
    };
    // SAFETY: the features are available. Every access is eight bytes from
    // the start of one of the sixteen rows `rows_at_mut` checked, read or
    // written as an unaligned 64-bit value or with `movq`, none needing
    // alignment.
    unsafe {
        let line = |k: usize| base.add(k * along).cast::<i64>().read_unaligned();
        // Four lines a vector, line 4g + r at bytes 8r to 8r + 7.
        let rows: [__m256i; 4] = core::array::from_fn(|g| {
            _mm256_set_epi64x(
                line(4 * g + 3),
                line(4 * g + 2),
                line(4 * g + 1),
                line(4 * g),
            )
        });
        let index = |bytes: &[u8; 32]| _mm256_loadu_si256(bytes.as_ptr().cast());
        let [to_low, to_high] = TO_POSITIONS.each_ref().map(index);
        // Positions 0 to 3 and 4 to 7 of lines 0 to 7, then of lines 8 to
        // 15.
        let quarters = [
            _mm256_permutex2var_epi8(rows[0], to_low, rows[1]),
            _mm256_permutex2var_epi8(rows[0], to_high, rows[1]),
            _mm256_permutex2var_epi8(rows[2], to_low, rows[3]),
            _mm256_permutex2var_epi8(rows[2], to_high, rows[3]),
        ];
        let to_lanes = TO_LANES.each_ref().map(index);
        // The zeroing mask clears the high byte of each 16-bit lane.
        let [low_first, high_first, low_second, high_second] = quarters;
        let spread = |first: __m256i, second: __m256i| {
            to_lanes.map(|to| _mm256_maskz_permutex2var_epi8(0x5555_5555, first, to, second))
        };
        let [p3, p2, p1, p0] = spread(low_first, low_second);
        let [q0, q1, q2, q3] = spread(high_first, high_second);
        let mut lanes = [p3, p2, p1, p0, q0, q1, q2, q3];
        let [alpha, beta, alpha4, bs, tc0] = thresholds(params);
        if !filter_luma16(&mut lanes, alpha, beta, alpha4, bs, tc0) {
            return;
        }
        // Back: the saturating pack clips, then the two rounds of permutes
        // in reverse.
        let [p3, p2, p1, p0, q0, q1, q2, q3] = lanes;
        let packs = [
            _mm256_packus_epi16(p3, p2),
            _mm256_packus_epi16(p1, p0),
            _mm256_packus_epi16(q0, q1),
            _mm256_packus_epi16(q2, q3),
        ];
        let [to_q_low, to_q_high] = TO_QUARTERS.each_ref().map(index);
        let [to_first, to_second] = TO_LINES.each_ref().map(index);
        // Positions 0 to 3, then 4 to 7, of lines 0 to 7 and of 8 to 15.
        let low = [
            _mm256_permutex2var_epi8(packs[0], to_q_low, packs[1]),
            _mm256_permutex2var_epi8(packs[0], to_q_high, packs[1]),
        ];
        let high = [
            _mm256_permutex2var_epi8(packs[2], to_q_low, packs[3]),
            _mm256_permutex2var_epi8(packs[2], to_q_high, packs[3]),
        ];
        for (h, (first, second)) in [(low[0], high[0]), (low[1], high[1])]
            .into_iter()
            .enumerate()
        {
            for (g, to) in [to_first, to_second].into_iter().enumerate() {
                let four = _mm256_permutex2var_epi8(first, to, second);
                let k = 8 * h + 4 * g;
                let (a, b) = (
                    _mm256_castsi256_si128(four),
                    _mm256_extracti128_si256(four, 1),
                );
                // The high halves with `movhpd`, an unaligned 64-bit store
                // that needs no shuffle, as an `f64` written unaligned:
                // `_mm_storeh_pd` would require the row to be aligned.
                let high =
                    |v| _mm_cvtsd_f64(_mm_unpackhi_pd(_mm_castsi128_pd(v), _mm_castsi128_pd(v)));
                _mm_storel_epi64(base.add(k * along).cast(), a);
                base.add((k + 1) * along)
                    .cast::<f64>()
                    .write_unaligned(high(a));
                _mm_storel_epi64(base.add((k + 2) * along).cast(), b);
                base.add((k + 3) * along)
                    .cast::<f64>()
                    .write_unaligned(high(b));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Bar C3 for this tier's deblocking filter: the sweeps of the AVX2
    //! kernel's tests, every threshold and strength and the tightest
    //! edges, through this tier's dispatch, where the CPU has it.

    use super::super::x86::tests::{EdgeKernel, every_threshold_and_strength, tightest_edges};

    /// This tier's kernel, where the CPU has its features.
    fn kernel() -> Option<EdgeKernel> {
        let tier = super::super::Simd::detect().is_some_and(|simd| simd.has_avx512());
        // `filter_edge` is `#[inline(always)]`; the pointer is to a copy.
        tier.then_some(super::filter_edge as EdgeKernel)
    }

    #[test]
    fn every_threshold_and_strength_matches_the_reference() {
        if let Some(kernel) = kernel() {
            every_threshold_and_strength(kernel);
        }
    }

    #[test]
    fn the_tightest_edges_match_the_reference() {
        if let Some(kernel) = kernel() {
            tightest_edges(kernel);
        }
    }

    /// The thresholds against their definition, lane by lane, for inputs
    /// the decoder never forms but the trait allows.
    #[test]
    fn the_thresholds_follow_their_definition_on_extreme_inputs() {
        use super::super::x86::chroma_mb::tests::{extreme_params, words};
        if kernel().is_none() {
            return;
        }
        for p in extreme_params(2000) {
            // SAFETY: the CPU has this tier's features (`kernel`).
            let lanes = unsafe { super::thresholds(&p) }.map(words);
            let [alpha, beta, alpha4, bs, tc0] = lanes;
            for k in 0..16 {
                // Four lines a segment.
                let segment = k / 4;
                assert_eq!(alpha[k], i16::from(p.alpha), "{p:?} lane {k}");
                assert_eq!(beta[k], i16::from(p.beta), "{p:?} lane {k}");
                assert_eq!(alpha4[k], (i16::from(p.alpha) >> 2) + 2, "{p:?} lane {k}");
                assert_eq!(bs[k], i16::from(p.bs[segment]), "{p:?} lane {k}");
                assert_eq!(tc0[k], i16::from(p.tc0_at(segment)), "{p:?} lane {k}");
            }
        }
    }
}
