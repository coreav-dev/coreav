//! [`crate::Kernels::filter_chroma_mb`] on x86-64: the four chroma edges
//! of a macroblock, both planes, in two passes over one checked window,
//! with samples as bytes.
//!
//! The two vertical edges (columns 0 and 4 of the block) touch disjoint
//! columns: edge 0 reads columns −2 to 1 and edge 4 columns 2 to 5. So
//! one transpose of the block's eight rows of columns −2 to 5 gives the
//! samples of both, and both are filtered at once, before anything is
//! written back. The same holds for the horizontal edges, by rows. A pass
//! holds each of p1, p0, q0 and q1 in one vector of 32 bytes, a byte a
//! line: Cb in the low 128-bit half and Cr in the high half, and in each
//! half the first edge's eight lines, then the second's. One run of the
//! filter covers the pass, and a pass runs where any segment of its edges
//! has a boundary strength.
//!
//! The filter works on bytes (`filter`): the threshold tests by saturating
//! differences, the strong filter by rounding averages, and the weak
//! filter's Δ (Equation 8-475) by averages and one signed saturating sum,
//! exact while t'C0 is at most 31 (Table 8-17's chroma entries are at most
//! 25); parameters past that go to the paired kernel edge by edge. The
//! transposes are byte, word, doubleword and quadword interleaves within a
//! 128-bit half, and rows are read by 64-bit broadcasts that a blend
//! joins, so that only the writes of the high half take a shuffle across
//! halves (`docs/design/h264.md`, "Kernel round: chroma deblocking on
//! bytes").
//!
//! Memory is touched as in the paired kernel: one check of each plane's
//! window (ten rows of ten bytes, [`super::rows_at_mut`]), made before any
//! pointer exists, then reads and writes of eight bytes of a row through
//! the pointer. Where a check fails, the edges go to the paired kernel one
//! at a time.

use core::arch::x86_64::{
    __m128d, __m256d, __m256i, _mm_storeh_pd, _mm_storel_pd, _mm256_add_epi8, _mm256_adds_epi8,
    _mm256_adds_epu8, _mm256_and_si256, _mm256_andnot_si256, _mm256_avg_epu8, _mm256_blend_epi32,
    _mm256_castpd256_pd128, _mm256_castps_pd, _mm256_castsi256_pd, _mm256_castsi256_ps,
    _mm256_cmpeq_epi8, _mm256_extractf128_pd, _mm256_loadu2_m128i, _mm256_max_epu8,
    _mm256_min_epu8, _mm256_or_si256, _mm256_set1_epi8, _mm256_setr_epi8, _mm256_setzero_si256,
    _mm256_shuffle_epi8, _mm256_sub_epi8, _mm256_subs_epi8, _mm256_subs_epu8, _mm256_testz_si256,
    _mm256_unpackhi_epi8, _mm256_unpackhi_epi16, _mm256_unpackhi_epi32, _mm256_unpackhi_epi64,
    _mm256_unpackhi_ps, _mm256_unpacklo_epi8, _mm256_unpacklo_epi16, _mm256_unpacklo_epi32,
    _mm256_unpacklo_epi64, _mm256_unpacklo_ps, _mm256_xor_si256,
};

use super::rows_at_mut;
use crate::simd::Simd;
use crate::views::ChromaMbMut;
use crate::{EdgeParams, chroma_mb_by_edges};

/// A pass's parameters as byte lanes, in the layout of its samples: Cb's
/// in the low half, Cr's in the high, the first edge's eight lines before
/// the second's in each half, line `k` of an edge in segment `k / 2`.
struct Lanes {
    alpha: __m256i,
    beta: __m256i,
    bs: __m256i,
    /// t'C0, and tC = t'C0 + 1.
    tc0: __m256i,
    tc: __m256i,
    /// All ones where bS is 4 or more.
    strong: __m256i,
}

/// Whether any line of a pass has a boundary strength.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn filters(l: &Lanes) -> bool {
    _mm256_testz_si256(l.bs, l.bs) == 0
}

/// The lanes of a pass whose edges are `edges[0]` and `edges[1]`, each a
/// pair of Cb's and Cr's parameters, read from the records' bytes:
/// alpha, beta and the four bS of each record go to their lines by byte
/// shuffles, and each segment's t'C0 is its bS's entry of its record's row
/// (modulo 8, as `EdgeParams::tc0_at`), picked by one more.
///
/// The four records are 56 bytes, Cb's and Cr's of the first edge at
/// bytes 0 and 14, of the second at 28 and 42 (`EdgeParams` is
/// `repr(C)`, fourteen bytes). Each edge's are read as one vector: sixteen
/// bytes from its Cb record to the low half, sixteen ending at the end of
/// its Cr record to the high half, where that record starts at byte 2.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn lanes(edges: &[[EdgeParams; 2]; 2]) -> Lanes {
    let at = edges.as_ptr().cast::<u8>();
    // SAFETY: AVX2 is available. `edges` is 56 initialised bytes (four
    // records of fourteen u8 fields, no padding): the reads are of bytes
    // 0 to 15 and 12 to 27 (the first edge) and 28 to 43 and 40 to 55 (the
    // second), unaligned, inside them; the rest are register operations.
    unsafe {
        let first = _mm256_loadu2_m128i(at.add(12).cast(), at.cast());
        let second = _mm256_loadu2_m128i(at.add(40).cast(), at.add(28).cast());
        // Byte `low` of each record, in the low half, or `low + 2`, in the
        // high, to every line of the first edge (from `first`) and of the
        // second (from `second`).
        let field = |low: i8| {
            let high = low + 2;
            let pick = _mm256_setr_epi8(
                low, low, low, low, low, low, low, low, low, low, low, low, low, low, low, low,
                high, high, high, high, high, high, high, high, high, high, high, high, high, high,
                high, high,
            );
            spread(first, second, pick)
        };
        // Segment k / 2 (bytes 10 to 13 of a record) to line k.
        let segments = _mm256_setr_epi8(
            10, 10, 11, 11, 12, 12, 13, 13, 10, 10, 11, 11, 12, 12, 13, 13, 12, 12, 13, 13, 14, 14,
            15, 15, 12, 12, 13, 13, 14, 14, 15, 15,
        );
        let bs = spread(first, second, segments);
        // Each line's t'C0: byte 2 + (bS & 7) of its record.
        let row = _mm256_setr_epi8(
            2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
            4, 4, 4,
        );
        let tc0 = spread(
            first,
            second,
            _mm256_add_epi8(_mm256_and_si256(bs, _mm256_set1_epi8(7)), row),
        );
        Lanes {
            alpha: field(0),
            beta: field(1),
            bs,
            tc0,
            tc: _mm256_add_epi8(tc0, _mm256_set1_epi8(1)),
            strong: _mm256_cmpeq_epi8(_mm256_max_epu8(bs, _mm256_set1_epi8(4)), bs),
        }
    }
}

/// Bytes `pick` of `first` to the first eight bytes of each half and of
/// `second` to the last eight, each half from its own half.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn spread(first: __m256i, second: __m256i, pick: __m256i) -> __m256i {
    _mm256_blend_epi32::<0b1100_1100>(
        _mm256_shuffle_epi8(first, pick),
        _mm256_shuffle_epi8(second, pick),
    )
}

/// Whether every t'C0 the lanes selected is at most 31, where the weak
/// filter's Δ on bytes is exact (`filter`).
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn in_range(l: &Lanes) -> bool {
    _mm256_testz_si256(l.tc0, _mm256_set1_epi8(0xe0u8 as i8)) == 1
}

/// |a − b| of bytes.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn distance(a: __m256i, b: __m256i) -> __m256i {
    _mm256_or_si256(_mm256_subs_epu8(a, b), _mm256_subs_epu8(b, a))
}

/// ⌊(a + b) / 2⌋ of bytes: the rounding average less the bit it rounded
/// up by.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn floor_average(a: __m256i, b: __m256i) -> __m256i {
    let odd = _mm256_and_si256(_mm256_xor_si256(a, b), _mm256_set1_epi8(1));
    _mm256_sub_epi8(_mm256_avg_epu8(a, b), odd)
}

/// The chroma filter (clause 8.7.2.3 with chromaStyleFiltering, and
/// Equations 8-484 and 8-488) of 32 lines, a byte each: returns the new
/// p0 and q0.
///
/// A line is filtered where its bS is not 0 and |p0 − q0| < α,
/// |p1 − p0| < β and |q1 − q0| < β (Equation 8-468): where the saturating
/// differences α − |p0 − q0| and β − max(|p1 − p0|, |q1 − q0|) and bS are
/// all above 0.
///
/// The weak filter's Δ is Clip3(−tC, tC, t) with
/// t = (4(q0 − p0) + (p1 − q1) + 4) >> 3, which equals
/// (d + g) >> 1 for d = q0 − p0 and g = (((p1 − q1) >> 1) + 2) >> 1 (each
/// shift a floor, and nested floors of halves are the floor of the
/// quarter). g + 64 is two rounding averages, of p1 and 255 − q1 and then
/// of that and 1; d is taken as a signed byte, saturating, and d + g as a
/// signed saturating sum. Where either saturates, t is 32 or more in
/// magnitude, as is the sum's half, and with the sign of the true t; so
/// both clip to the same bound while tC = t'C0 + 1 is at most 32. The sum's
/// half, offset by 192, is the rounding average of the sum offset by 128
/// and 255; its parts above and below 192, each clipped to tC, are added
/// to one sample and subtracted from the other with unsigned saturation:
/// Clip1 of Equations 8-476 and 8-477.
///
/// The strong filter's (2p1 + p0 + q1 + 2) >> 2 is the rounding average
/// of p1 and ⌊(p0 + q1) / 2⌋, and likewise for q0.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn filter(s: &[__m256i; 4], l: &Lanes) -> (__m256i, __m256i) {
    let [p1, p0, q0, q1] = *s;
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let zero = _mm256_setzero_si256();
        let near = _mm256_min_epu8(
            _mm256_subs_epu8(l.alpha, distance(p0, q0)),
            _mm256_subs_epu8(l.beta, _mm256_max_epu8(distance(p1, p0), distance(q1, q0))),
        );
        let unfiltered = _mm256_cmpeq_epi8(_mm256_min_epu8(near, l.bs), zero);
        // The weak filter.
        let sign = _mm256_set1_epi8(-128);
        let d = _mm256_subs_epi8(_mm256_xor_si256(q0, sign), _mm256_xor_si256(p0, sign));
        let half = _mm256_avg_epu8(p1, _mm256_xor_si256(q1, _mm256_set1_epi8(-1)));
        let g = _mm256_sub_epi8(
            _mm256_avg_epu8(half, _mm256_set1_epi8(1)),
            _mm256_set1_epi8(64),
        );
        let sum = _mm256_xor_si256(_mm256_adds_epi8(d, g), sign);
        let t = _mm256_avg_epu8(sum, _mm256_set1_epi8(-1));
        // Δ's parts above and below 0, each clipped to tC, which is 0 on
        // the lines not filtered.
        let tc = _mm256_andnot_si256(unfiltered, l.tc);
        let middle = _mm256_set1_epi8(192u8 as i8);
        let up = _mm256_min_epu8(_mm256_subs_epu8(t, middle), tc);
        let down = _mm256_min_epu8(_mm256_subs_epu8(middle, t), tc);
        let weak_p0 = _mm256_subs_epu8(_mm256_adds_epu8(p0, up), down);
        let weak_q0 = _mm256_subs_epu8(_mm256_adds_epu8(q0, down), up);
        // The strong filter, where bS is 4 or more.
        let strong_p0 = _mm256_avg_epu8(p1, floor_average(p0, q1));
        let strong_q0 = _mm256_avg_epu8(q1, floor_average(q0, p1));
        let strong = _mm256_andnot_si256(unfiltered, l.strong);
        // Bits of the strong result where `strong` is set: three
        // single-cycle operations, where a variable blend takes two or
        // three slower ones.
        let pick = |weak: __m256i, strong_value: __m256i| {
            _mm256_xor_si256(
                weak,
                _mm256_and_si256(_mm256_xor_si256(weak, strong_value), strong),
            )
        };
        (pick(weak_p0, strong_p0), pick(weak_q0, strong_q0))
    }
}

/// [`crate::Kernels::filter_chroma_mb`].
///
/// # Safety
///
/// AVX2 must be available.
#[target_feature(enable = "avx2")]
pub(in crate::simd) unsafe fn filter_chroma_mb(
    simd: &Simd,
    mut cb: ChromaMbMut<'_>,
    mut cr: ChromaMbMut<'_>,
    params: &[[EdgeParams; 2]; 4],
) {
    let (Some(vertical_params), Some(horizontal_params)) =
        (params.first_chunk(), params.last_chunk())
    else {
        return;
    };
    // SAFETY: AVX2 is available (this function's contract).
    let (lv, lh) = unsafe { (lanes(vertical_params), lanes(horizontal_params)) };
    // A pass runs where any segment of its edges has a boundary strength.
    // SAFETY: AVX2 is available (this function's contract).
    let (vertical, horizontal, exact) =
        unsafe { (filters(&lv), filters(&lh), in_range(&lv) && in_range(&lh)) };
    if !vertical && !horizontal {
        return;
    }
    let stride = cb.stride();
    // One check per plane: the window's ten rows of ten bytes.
    let (at_cb, at_cr) = if stride >= 10 && cr.stride() == stride && exact {
        (
            rows_at_mut(cb.window(), 0, stride, 10, 10),
            rows_at_mut(cr.window(), 0, stride, 10, 10),
        )
    } else {
        (None, None)
    };
    let (Some(at_cb), Some(at_cr)) = (at_cb, at_cr) else {
        chroma_mb_by_edges(simd, cb, cr, params);
        return;
    };
    // SAFETY: AVX2 is available (this function's contract). The pointers
    // point at the first byte of each plane's window, whose ten rows of ten
    // bytes, `stride` apart, `rows_at_mut` checked lie inside it: bytes 0
    // to 9 * stride + 9 from the pointer. The vertical pass reads sixteen
    // bytes of rows 2 to 9 from column −8, bytes 2 * stride − 8 to
    // 9 * stride + 7, and writes eight bytes of each from column 0; the
    // horizontal pass reads sixteen bytes of rows 0 to 7 from column 2,
    // bytes 2 to 7 * stride + 17, and writes eight bytes of rows 1, 2, 5
    // and 6 from column 2. With a stride of at least ten, every one of
    // these lies inside that extent (2 * stride − 8 ≥ 0, and
    // 7 * stride + 17 < 9 * stride + 10); all are unaligned.
    unsafe {
        if vertical {
            let at = 2 * stride - 8;
            vertical_edges(at_cb.add(at), at_cr.add(at), stride, &lv);
        }
        if horizontal {
            horizontal_edges(at_cb.add(2), at_cr.add(2), stride, &lh);
        }
    }
}

/// Sixteen bytes at `at` of each plane: Cb's in the low half, Cr's in the
/// high half.
///
/// # Safety
///
/// AVX2 must be available, and sixteen bytes at `at` must lie inside the
/// window each pointer points at.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn both(cb: *mut u8, cr: *mut u8, at: usize) -> __m256i {
    // SAFETY: AVX2 is available; the bytes are inside the windows (this
    // function's contract), read unaligned.
    unsafe { _mm256_loadu2_m128i(cr.add(at).cast(), cb.add(at).cast()) }
}

/// Writes quadwords 0 and 1 of `v` to `at` and `at + stride` of Cb's
/// window, and quadwords 2 and 3 to the same rows of Cr's.
///
/// `v` is taken as doubles, so that the high quadword of each half is
/// written by `movhpd` itself rather than moved to the low one first.
///
/// # Safety
///
/// AVX2 must be available, and eight bytes at `at` and at `at + stride`
/// must lie inside the window each pointer points at.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn store_both(cb: *mut u8, cr: *mut u8, at: usize, stride: usize, v: __m256d) {
    // SAFETY: AVX2 is available; the rows are inside the windows (this
    // function's contract), written as unaligned 64-bit values (`movlpd`,
    // `movhpd`).
    unsafe {
        let write = |plane: *mut u8, half: __m128d| {
            _mm_storel_pd(plane.add(at).cast(), half);
            _mm_storeh_pd(plane.add(at + stride).cast(), half);
        };
        write(cb, _mm256_castpd256_pd128(v));
        write(cr, _mm256_extractf128_pd::<1>(v));
    }
}

/// `a` and `b` interleaved by doubleword within each half, as doubles:
/// `_mm256_unpacklo_epi32` (`LOW`) or `_mm256_unpackhi_epi32`, in the
/// domain of [`store_both`].
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn interleave32<const LOW: bool>(a: __m256i, b: __m256i) -> __m256d {
    let (a, b) = (_mm256_castsi256_ps(a), _mm256_castsi256_ps(b));
    _mm256_castps_pd(if LOW {
        _mm256_unpacklo_ps(a, b)
    } else {
        _mm256_unpackhi_ps(a, b)
    })
}

/// Vertical edges 0 and 4: the block's eight rows of columns −2 to 5
/// (window rows 2 to 9, columns 0 to 7), transposed into the eight
/// columns, filtered, transposed back and written as whole rows; columns
/// no edge changes are written as read.
///
/// # Safety
///
/// AVX2 must be available, and rows 2 to 9 of each window, eight bytes
/// from column 0, `stride` apart, must lie inside the window `cb` and `cr`
/// point at.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn vertical_edges(cb: *mut u8, cr: *mut u8, stride: usize, l: &Lanes) {
    // SAFETY: AVX2 is available; the rows are inside the windows (this
    // function's contract).
    unsafe {
        // Row k of each plane from column −8: columns −2 to 5 of the block
        // are bytes 8 to 15 of each half.
        let row = |k: usize| both(cb, cr, k * stride);
        // Rows 2i and 2i + 1 byte by byte, then 16-bit pairs of rows 0 to
        // 3 and of rows 4 to 7, then the 32-bit groups of the two: each
        // half then holds its plane's columns in pairs, eight bytes each.
        let a = _mm256_unpackhi_epi8(row(0), row(1));
        let b = _mm256_unpackhi_epi8(row(2), row(3));
        let c = _mm256_unpackhi_epi8(row(4), row(5));
        let d = _mm256_unpackhi_epi8(row(6), row(7));
        let [top_left, top_right] = [_mm256_unpacklo_epi16(a, b), _mm256_unpackhi_epi16(a, b)];
        let [bottom_left, bottom_right] =
            [_mm256_unpacklo_epi16(c, d), _mm256_unpackhi_epi16(c, d)];
        // Columns −2 and −1, 0 and 1, 2 and 3, 4 and 5.
        let c01 = _mm256_unpacklo_epi32(top_left, bottom_left);
        let c23 = _mm256_unpackhi_epi32(top_left, bottom_left);
        let c45 = _mm256_unpacklo_epi32(top_right, bottom_right);
        let c67 = _mm256_unpackhi_epi32(top_right, bottom_right);
        // Each edge's column of a position: edge 0's in the low eight bytes
        // of each half, edge 4's in the high eight.
        let p1 = _mm256_unpacklo_epi64(c01, c45);
        let q0 = _mm256_unpacklo_epi64(c23, c67);
        let q1 = _mm256_unpackhi_epi64(c23, c67);
        let (p0, q0) = filter(&[p1, _mm256_unpackhi_epi64(c01, c45), q0, q1], l);
        // Back: columns −2 and −1 byte by byte (edge 0's p1 and p0), then
        // 2 and 3 (edge 4's), and so on, then the inverse of the 16- and
        // 32-bit steps.
        let [a, b] = [_mm256_unpacklo_epi8(p1, p0), _mm256_unpackhi_epi8(p1, p0)];
        let [c, d] = [_mm256_unpacklo_epi8(q0, q1), _mm256_unpackhi_epi8(q0, q1)];
        // Columns −2 to 1 of rows 0 to 3 and of rows 4 to 7, then columns
        // 2 to 5.
        let [left_top, left_bottom] = [_mm256_unpacklo_epi16(a, c), _mm256_unpackhi_epi16(a, c)];
        let [right_top, right_bottom] = [_mm256_unpacklo_epi16(b, d), _mm256_unpackhi_epi16(b, d)];
        let at = |k: usize| k * stride + 8;
        let rows = [
            interleave32::<true>(left_top, right_top),
            interleave32::<false>(left_top, right_top),
            interleave32::<true>(left_bottom, right_bottom),
            interleave32::<false>(left_bottom, right_bottom),
        ];
        store_both(cb, cr, at(0), stride, rows[0]);
        store_both(cb, cr, at(2), stride, rows[1]);
        store_both(cb, cr, at(4), stride, rows[2]);
        store_both(cb, cr, at(6), stride, rows[3]);
    }
}

/// Horizontal edges 0 and 4: the block's columns 0 to 7 of rows −2 to 5
/// (window rows 0 to 7, from column 2), one row of each edge a vector;
/// rows p0 and q0 of each edge are written back.
///
/// # Safety
///
/// AVX2 must be available, and rows 0 to 7 of each window, eight bytes
/// from column 2, `stride` apart, must lie inside the window `cb` and `cr`
/// point at.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_edges(cb: *mut u8, cr: *mut u8, stride: usize, l: &Lanes) {
    // SAFETY: AVX2 is available; the rows are inside the windows (this
    // function's contract).
    unsafe {
        // Window row k of edge 0 in quadwords 0 and 2, row k + 4 of edge 4
        // in quadwords 1 and 3.
        let rows = |k: usize| {
            _mm256_unpacklo_epi64(both(cb, cr, k * stride), both(cb, cr, (k + 4) * stride))
        };
        let (p0, q0) = filter(&[rows(0), rows(1), rows(2), rows(3)], l);
        // Quadwords 0 and 1 are window rows k and k + 4 of a plane.
        let write = |k: usize, v: __m256i| {
            store_both(cb, cr, k * stride, 4 * stride, _mm256_castsi256_pd(v));
        };
        write(1, p0);
        write(2, q0);
    }
}

#[cfg(test)]
pub(in crate::simd) mod tests {
    //! The lanes and the filter on bytes against their definitions, lane by
    //! lane, including inputs the decoder never forms but the trait allows.

    use core::arch::x86_64::{__m256i, _mm256_loadu_si256, _mm256_storeu_si256};

    use super::{Lanes, filter, in_range, lanes};
    use crate::EdgeParams;

    /// `count` parameter sets of any byte values, the bS bytes from every
    /// class (in range, above 4, above 7, with the top bit set),
    /// reproducibly.
    pub(in crate::simd) fn extreme_params(count: usize) -> Vec<EdgeParams> {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut pick = |n: usize| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (s >> 33) as usize % n
        };
        let bytes = [0u8, 1, 3, 4, 5, 7, 8, 15, 16, 127, 128, 255];
        (0..count)
            .map(|_| EdgeParams {
                alpha: pick(256) as u8,
                beta: pick(256) as u8,
                tc0: [0; 8].map(|_| pick(256) as u8),
                bs: [0; 4].map(|_| bytes[pick(bytes.len())]),
            })
            .collect()
    }

    pub(in crate::simd) fn words(v: __m256i) -> [i16; 16] {
        let mut out = [0i16; 16];
        // SAFETY: test code; the store writes the 32 bytes of `out`, and
        // AVX is available wherever the callers run it.
        unsafe { _mm256_storeu_si256(out.as_mut_ptr().cast(), v) };
        out
    }

    fn bytes(v: __m256i) -> [u8; 32] {
        let mut out = [0u8; 32];
        // SAFETY: test code; the store writes the 32 bytes of `out`, and
        // AVX2 is available wherever the callers run it.
        unsafe { _mm256_storeu_si256(out.as_mut_ptr().cast(), v) };
        out
    }

    fn vector(b: [u8; 32]) -> __m256i {
        // SAFETY: test code; the load reads the 32 bytes of `b`.
        unsafe { _mm256_loadu_si256(b.as_ptr().cast()) }
    }

    /// The parameters of lane `k` of a pass whose edges are `first` and
    /// `second`, and its segment.
    fn of_lane<'a>(
        first: &'a [EdgeParams; 2],
        second: &'a [EdgeParams; 2],
        k: usize,
    ) -> (&'a EdgeParams, usize) {
        let edge = if (k % 16) < 8 { first } else { second };
        (&edge[k / 16], (k % 8) / 2)
    }

    #[test]
    fn the_lanes_follow_their_definition_on_extreme_inputs() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let all = extreme_params(4000);
        for four in all.chunks_exact(4) {
            let (first, second) = ([four[0], four[1]], [four[2], four[3]]);
            // SAFETY: AVX2 was detected.
            let l = unsafe { lanes(&[first, second]) };
            let [alpha, beta, bs, tc0, tc, strong] =
                [l.alpha, l.beta, l.bs, l.tc0, l.tc, l.strong].map(bytes);
            for k in 0..32 {
                let (p, segment) = of_lane(&first, &second, k);
                let t = p.tc0_at(segment);
                assert_eq!(alpha[k], p.alpha, "{p:?} lane {k}");
                assert_eq!(beta[k], p.beta, "{p:?} lane {k}");
                assert_eq!(bs[k], p.bs[segment], "{p:?} lane {k}");
                assert_eq!(tc0[k], t, "{p:?} lane {k}");
                assert_eq!(tc[k], t.wrapping_add(1), "{p:?} lane {k}");
                assert_eq!(
                    strong[k],
                    if p.bs[segment] >= 4 { 255 } else { 0 },
                    "{p:?} lane {k}"
                );
            }
        }
    }

    /// A line (p1, p0, q0, q1) with its α, β, t'C0 and bS.
    type Line = ([u8; 4], u8, u8, u8, u8);

    /// The filter of one line as the clause gives it (the reference's
    /// arithmetic in `scalar.rs`): new p0 and q0.
    fn by_the_clause(s: [u8; 4], alpha: u8, beta: u8, tc0: u8, bs: u8) -> (u8, u8) {
        let [p1, p0, q0, q1] = s.map(i32::from);
        let (a, b) = (i32::from(alpha), i32::from(beta));
        if bs == 0 || (p0 - q0).abs() >= a || (p1 - p0).abs() >= b || (q1 - q0).abs() >= b {
            return (s[1], s[2]);
        }
        if bs < 4 {
            let tc = i32::from(tc0) + 1;
            let delta = ((((q0 - p0) << 2) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
            (
                (p0 + delta).clamp(0, 255) as u8,
                (q0 - delta).clamp(0, 255) as u8,
            )
        } else {
            (
                ((2 * p1 + p0 + q1 + 2) >> 2) as u8,
                ((2 * q1 + q0 + p1 + 2) >> 2) as u8,
            )
        }
    }

    /// Runs the filter on 32 lines, each with its own parameters.
    fn run(lines: &[Line]) -> Vec<(u8, u8)> {
        let lane = |f: &dyn Fn(&Line) -> u8| {
            let mut b = [0u8; 32];
            for (b, line) in b.iter_mut().zip(lines) {
                *b = f(line);
            }
            vector(b)
        };
        let s = [0, 1, 2, 3].map(|i| lane(&|l| l.0[i]));
        let bs = lane(&|l| l.4);
        let tc0 = bytes(lane(&|l| l.3));
        let l = Lanes {
            alpha: lane(&|l| l.1),
            beta: lane(&|l| l.2),
            bs,
            tc0: vector(tc0),
            tc: vector(tc0.map(|t| t.wrapping_add(1))),
            strong: lane(&|l| if l.4 >= 4 { 255 } else { 0 }),
        };
        // SAFETY: the callers checked that AVX2 is available.
        let (p0, q0) = unsafe { filter(&s, &l) };
        bytes(p0)
            .into_iter()
            .zip(bytes(q0))
            .take(lines.len())
            .collect()
    }

    /// Every p0 and q0, against p1 and q1 from the extremes and a grid,
    /// with t'C0 at 0, the table's largest (25) and the largest the bytes
    /// hold exactly (31), every bS class and thresholds that pass: the
    /// weak filter's saturating sums at both ends of their range.
    #[test]
    fn the_filter_on_bytes_matches_the_clause() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let outer = [0u8, 1, 2, 3, 64, 127, 128, 129, 192, 252, 253, 254, 255];
        let mut lines = Vec::new();
        let check = |lines: &mut Vec<Line>| {
            for (line, got) in lines.iter().zip(run(lines)) {
                let (s, alpha, beta, tc0, bs) = *line;
                assert_eq!(got, by_the_clause(s, alpha, beta, tc0, bs), "{line:?}");
            }
            lines.clear();
        };
        for p0 in 0..=255u8 {
            for q0 in 0..=255u8 {
                for (i, &p1) in outer.iter().enumerate() {
                    let q1 = outer[(i * 7 + usize::from(p0 ^ q0)) % outer.len()];
                    let tc0 = [0, 25, 31][usize::from(p0.wrapping_add(q0)) % 3];
                    let bs = [1, 2, 3, 4, 0, 200][(usize::from(q0) + i) % 6];
                    lines.push(([p1, p0, q0, q1], 255, 255, tc0, bs));
                    if lines.len() == 32 {
                        check(&mut lines);
                    }
                }
            }
        }
        check(&mut lines);
    }

    /// Random lines and thresholds: the threshold tests at and either side
    /// of α and β.
    #[test]
    fn the_filter_on_bytes_matches_the_clause_on_random_lines() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let mut s = 0x9e37_79b9u64;
        let mut next = |n: u32| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((s >> 33) as u32 % n) as u8
        };
        for _ in 0..100_000 {
            let lines: Vec<_> = (0..32)
                .map(|_| {
                    let p0 = next(256);
                    let near = |v: u8, r: u8, n: &mut dyn FnMut(u32) -> u8| {
                        v.saturating_add(n(u32::from(r) + 1))
                            .saturating_sub(n(u32::from(r) + 1))
                    };
                    let q0 = near(p0, 40, &mut next);
                    let p1 = near(p0, 20, &mut next);
                    let q1 = near(q0, 20, &mut next);
                    let alpha = next(64);
                    let beta = next(24);
                    ([p1, p0, q0, q1], alpha, beta, next(32), next(6))
                })
                .collect();
            for (line, got) in lines.iter().zip(run(&lines)) {
                let (s, alpha, beta, tc0, bs) = *line;
                assert_eq!(got, by_the_clause(s, alpha, beta, tc0, bs), "{line:?}");
            }
        }
    }

    #[test]
    fn t_c0_past_31_is_out_of_range() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        // A t'C0 of 32 is out of range where a segment's bS selects it, and
        // not elsewhere.
        for entry in 0..8u8 {
            for at in 0..16 {
                let mut edges = [[EdgeParams::default(); 2]; 2];
                let p = &mut edges[at / 8][(at / 4) % 2];
                p.tc0 = [31; 8];
                p.tc0[usize::from(entry)] = 32;
                p.bs[at % 4] = entry | 8;
                // SAFETY: AVX2 was detected.
                let exact = unsafe { in_range(&lanes(&edges)) };
                assert!(!exact, "{entry} {at}");
                edges[at / 8][(at / 4) % 2].bs[at % 4] = entry.wrapping_add(1) & 7;
                // SAFETY: AVX2 was detected.
                let exact = unsafe { in_range(&lanes(&edges)) };
                // The record's other segments have bS 0 and select entry 0.
                assert_eq!(exact, entry != 0, "{entry} {at}");
            }
        }
    }
}
