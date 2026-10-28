//! [`crate::Kernels::filter_chroma_mb`] on x86-64: the four chroma edges
//! of a macroblock, both planes, in two passes over one checked window.
//!
//! The two vertical edges (columns 0 and 4 of the block) touch disjoint
//! columns: edge 0 reads columns −2 to 1 and edge 4 columns 2 to 5. So
//! one transpose of the block's eight rows of columns −2 to 5 gives the
//! samples of both, and both are filtered before anything is written back.
//! The same holds for the horizontal edges, by rows. Each pass puts Cb's
//! eight lines in the low half of a vector and Cr's in the high half, as
//! the paired kernel does ([`super::filter_chroma16`]), and skips an edge
//! whose segments all have bS 0.
//!
//! Memory is touched as in the paired kernel: one check of each plane's
//! window (ten rows of ten bytes, [`super::rows_at_mut`]), made before any
//! pointer exists, then reads and writes of rows, or eight bytes of a row,
//! through the pointer. Where a check fails, the edges go to the paired
//! kernel one at a time.

use core::arch::x86_64::{
    __m128i, __m256i, _mm_cvtsi32_si128, _mm_cvtsi64_si128, _mm_extract_epi64, _mm_loadl_epi64,
    _mm_storel_epi64, _mm_unpacklo_epi64, _mm256_and_si256, _mm256_castsi256_si128,
    _mm256_cvtepu8_epi16, _mm256_extracti128_si256, _mm256_packus_epi16, _mm256_set_m128i,
    _mm256_set1_epi8, _mm256_set1_epi32, _mm256_setr_epi8, _mm256_setzero_si256,
    _mm256_shuffle_epi8, _mm256_shuffle_epi32, _mm256_unpackhi_epi8, _mm256_unpackhi_epi16,
    _mm256_unpackhi_epi32, _mm256_unpackhi_epi64, _mm256_unpacklo_epi8, _mm256_unpacklo_epi16,
    _mm256_unpacklo_epi32, _mm256_unpacklo_epi64,
};

use super::{filter_chroma16, rows_at_mut};
use crate::simd::Simd;
use crate::views::ChromaMbMut;
use crate::{EdgeParams, chroma_mb_by_edges};

/// A pair of edges' parameters as lanes: Cb's in the low half, Cr's in the
/// high half, as [`filter_chroma16`] takes them.
struct Lanes {
    alpha: __m256i,
    beta: __m256i,
    bs: __m256i,
    tc0: __m256i,
}

/// Whether any segment of either plane's edge has a boundary strength.
#[inline(always)]
fn filters(pair: &[EdgeParams; 2]) -> bool {
    pair.iter().any(|p| p.bs != [0; 4])
}

/// The lanes of a pair of edges' parameters, built in vector registers
/// from each plane's fields: alpha and beta (the four bytes in one 32-bit
/// value), the t'C0 row and the four bS.
///
/// Each segment's t'C0 is its bS's entry of the plane's row (modulo 8, as
/// `EdgeParams::tc0_at`), picked by one byte shuffle, and each segment's
/// bS and t'C0 go to its two lines, as `spread8` does.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn lanes(pair: &[EdgeParams; 2]) -> Lanes {
    let [for_cb, for_cr] = pair;
    // Bytes 0 to 3: Cb's alpha and beta, then Cr's.
    let thresholds = i32::from_le_bytes([for_cb.alpha, for_cb.beta, for_cr.alpha, for_cr.beta]);
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let words = _mm256_set1_epi32(thresholds);
        // Lane k of the low half takes byte `low`, of the high half byte
        // `high`, widened.
        let pick = |low: i8, high: i8| {
            _mm256_setr_epi8(
                low, -1, low, -1, low, -1, low, -1, low, -1, low, -1, low, -1, low, -1, high, -1,
                high, -1, high, -1, high, -1, high, -1, high, -1, high, -1, high, -1,
            )
        };
        let alpha = _mm256_shuffle_epi8(words, pick(0, 2));
        let beta = _mm256_shuffle_epi8(words, pick(1, 3));
        // Cb's bS and row in the low half, Cr's in the high.
        let bs = _mm256_set_m128i(
            _mm_cvtsi32_si128(i32::from_le_bytes(for_cr.bs)),
            _mm_cvtsi32_si128(i32::from_le_bytes(for_cb.bs)),
        );
        let rows = _mm256_set_m128i(
            _mm_cvtsi64_si128(i64::from_le_bytes(for_cr.tc0)),
            _mm_cvtsi64_si128(i64::from_le_bytes(for_cb.tc0)),
        );
        let tc0 = _mm256_shuffle_epi8(rows, _mm256_and_si256(bs, _mm256_set1_epi8(7)));
        // Segment k / 2 to line k, both halves.
        let spread = _mm256_setr_epi8(
            0, -1, 0, -1, 1, -1, 1, -1, 2, -1, 2, -1, 3, -1, 3, -1, 0, -1, 0, -1, 1, -1, 1, -1, 2,
            -1, 2, -1, 3, -1, 3, -1,
        );
        Lanes {
            alpha,
            beta,
            bs: _mm256_shuffle_epi8(bs, spread),
            tc0: _mm256_shuffle_epi8(tc0, spread),
        }
    }
}

/// Filters `s` (p1, p0, q0, q1) with `pair`'s parameters, unless no
/// segment of the pair has a boundary strength.
///
/// # Safety
///
/// AVX2 must be available.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn filter_pair(s: &mut [__m256i; 4], pair: &[EdgeParams; 2]) {
    if !filters(pair) {
        return;
    }
    // SAFETY: AVX2 is available; these are register operations.
    unsafe {
        let l = lanes(pair);
        filter_chroma16(s, l.alpha, l.beta, l.bs, l.tc0);
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
    let [v0, v4, h0, h4] = params;
    let vertical = filters(v0) || filters(v4);
    let horizontal = filters(h0) || filters(h4);
    if !vertical && !horizontal {
        return;
    }
    let stride = cb.stride();
    // One check per plane: the window's ten rows of ten bytes.
    let (at_cb, at_cr) = if cr.stride() == stride {
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
    // bytes, `stride` apart, `rows_at_mut` checked lie inside it; every
    // access below is at most eight bytes from row 2 + k, column 0 (the
    // vertical pass, k < 8) or row k, column 2 (the horizontal pass, k < 8)
    // of a window, unaligned, inside that extent.
    unsafe {
        if vertical {
            vertical_edges(at_cb, at_cr, stride, v0, v4);
        }
        if horizontal {
            horizontal_edges(at_cb, at_cr, stride, h0, h4);
        }
    }
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
unsafe fn vertical_edges(
    cb: *mut u8,
    cr: *mut u8,
    stride: usize,
    v0: &[EdgeParams; 2],
    v4: &[EdgeParams; 2],
) {
    // SAFETY: AVX2 is available; the rows are inside the windows (this
    // function's contract).
    unsafe {
        // Row k of each plane: Cb's eight bytes in the low half, Cr's in
        // the high half, each in the low eight bytes of its half.
        let row = |k: usize| {
            let at = (2 + k) * stride;
            _mm256_set_m128i(
                _mm_loadl_epi64(cr.add(at).cast()),
                _mm_loadl_epi64(cb.add(at).cast()),
            )
        };
        let [r0, r1, r2, r3, r4, r5, r6, r7] = [0, 1, 2, 3, 4, 5, 6, 7].map(row);
        // Each half is an 8x8 transpose of its plane's rows, in three
        // steps that each join two vectors element by element: bytes of
        // rows 2i and 2i + 1, then 16-bit pairs of rows 0 to 3 and of rows
        // 4 to 7, then the 32-bit groups of the two.
        let [t01, t23, t45, t67] =
            [(r0, r1), (r2, r3), (r4, r5), (r6, r7)].map(|(a, b)| _mm256_unpacklo_epi8(a, b));
        // Rows 0 to 3, then 4 to 7, four bytes a column: columns −2 to 1
        // in the first vector of each, 2 to 5 in the second.
        let [[top_left, top_right], [bottom_left, bottom_right]] = [(t01, t23), (t45, t67)]
            .map(|(a, b)| [_mm256_unpacklo_epi16(a, b), _mm256_unpackhi_epi16(a, b)]);
        // Columns −2 and −1, 0 and 1, 2 and 3, 4 and 5: eight bytes each,
        // one per row.
        let [[w0, w1], [w2, w3]] = [(top_left, bottom_left), (top_right, bottom_right)]
            .map(|(a, b)| [_mm256_unpacklo_epi32(a, b), _mm256_unpackhi_epi32(a, b)]);
        let w = [w0, w1, w2, w3];
        let zero = _mm256_setzero_si256();
        let wide = |v: __m256i| [_mm256_unpacklo_epi8(v, zero), _mm256_unpackhi_epi8(v, zero)];
        let [c0, c1] = wide(w[0]);
        let [c2, c3] = wide(w[1]);
        let [c4, c5] = wide(w[2]);
        let [c6, c7] = wide(w[3]);
        let mut edge0 = [c0, c1, c2, c3];
        let mut edge4 = [c4, c5, c6, c7];
        filter_pair(&mut edge0, v0);
        filter_pair(&mut edge4, v4);
        let [c0, c1, c2, c3] = edge0;
        let [c4, c5, c6, c7] = edge4;
        // Back to bytes, two columns a vector as `w` held them.
        let w = [
            _mm256_packus_epi16(c0, c1),
            _mm256_packus_epi16(c2, c3),
            _mm256_packus_epi16(c4, c5),
            _mm256_packus_epi16(c6, c7),
        ];
        // Undo the 32-bit interleave: the four-row groups of two columns
        // side by side, rows 0 to 3 in the low eight bytes.
        let by_group = |a: __m256i, b: __m256i| {
            let a = _mm256_shuffle_epi32::<0b11_01_10_00>(a);
            let b = _mm256_shuffle_epi32::<0b11_01_10_00>(b);
            (_mm256_unpacklo_epi64(a, b), _mm256_unpackhi_epi64(a, b))
        };
        let (u0, u2) = by_group(w[0], w[1]);
        let (u1, u3) = by_group(w[2], w[3]);
        // Undo the 16-bit interleave: even words to the low eight bytes.
        let words = _mm256_setr_epi8(
            0, 1, 4, 5, 8, 9, 12, 13, 2, 3, 6, 7, 10, 11, 14, 15, 0, 1, 4, 5, 8, 9, 12, 13, 2, 3,
            6, 7, 10, 11, 14, 15,
        );
        let pairs = |lo: __m256i, hi: __m256i| {
            let lo = _mm256_shuffle_epi8(lo, words);
            let hi = _mm256_shuffle_epi8(hi, words);
            (_mm256_unpacklo_epi64(lo, hi), _mm256_unpackhi_epi64(lo, hi))
        };
        let (t0, t1) = pairs(u0, u1);
        let (t2, t3) = pairs(u2, u3);
        // Undo the 8-bit interleave: two rows a vector, the first in the low
        // eight bytes of each half.
        let bytes = _mm256_setr_epi8(
            0, 2, 4, 6, 8, 10, 12, 14, 1, 3, 5, 7, 9, 11, 13, 15, 0, 2, 4, 6, 8, 10, 12, 14, 1, 3,
            5, 7, 9, 11, 13, 15,
        );
        for (k, t) in [t0, t1, t2, t3].into_iter().enumerate() {
            let rows = _mm256_shuffle_epi8(t, bytes);
            let (low, high) = (
                _mm256_castsi256_si128(rows),
                _mm256_extracti128_si256::<1>(rows),
            );
            store_rows(cb, (2 + 2 * k) * stride, stride, low);
            store_rows(cr, (2 + 2 * k) * stride, stride, high);
        }
    }
}

/// Writes the low eight bytes of `v` at `at` and the high eight at
/// `at + stride`.
///
/// # Safety
///
/// Both eight-byte rows must lie inside the window `plane` points at.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn store_rows(plane: *mut u8, at: usize, stride: usize, v: __m128i) {
    // SAFETY: AVX2 is available; the rows are inside the window (this
    // function's contract), and the high half is written as an unaligned
    // 64-bit value.
    unsafe {
        _mm_storel_epi64(plane.add(at).cast(), v);
        plane
            .add(at + stride)
            .cast::<i64>()
            .write_unaligned(_mm_extract_epi64::<1>(v));
    }
}

/// Horizontal edges 0 and 4: the block's columns 0 to 7 of rows −2 to 5
/// (window rows 0 to 7, from column 2), one row a vector; only the rows an
/// edge filtered are written back.
///
/// # Safety
///
/// AVX2 must be available, and rows 0 to 7 of each window, eight bytes
/// from column 2, `stride` apart, must lie inside the window `cb` and `cr`
/// point at.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_edges(
    cb: *mut u8,
    cr: *mut u8,
    stride: usize,
    h0: &[EdgeParams; 2],
    h4: &[EdgeParams; 2],
) {
    // SAFETY: AVX2 is available; the rows are inside the windows (this
    // function's contract).
    unsafe {
        let row = |k: usize| {
            let at = k * stride + 2;
            _mm256_cvtepu8_epi16(_mm_unpacklo_epi64(
                _mm_loadl_epi64(cb.add(at).cast()),
                _mm_loadl_epi64(cr.add(at).cast()),
            ))
        };
        let store = |k: usize, v: __m256i| {
            let packed = _mm256_packus_epi16(v, v);
            let at = k * stride + 2;
            _mm_storel_epi64(cb.add(at).cast(), _mm256_castsi256_si128(packed));
            _mm_storel_epi64(cr.add(at).cast(), _mm256_extracti128_si256::<1>(packed));
        };
        for (first, pair) in [(0, h0), (4, h4)] {
            if !filters(pair) {
                continue;
            }
            let mut s = [row(first), row(first + 1), row(first + 2), row(first + 3)];
            let l = lanes(pair);
            if filter_chroma16(&mut s, l.alpha, l.beta, l.bs, l.tc0) {
                store(first + 1, s[1]);
                store(first + 2, s[2]);
            }
        }
    }
}

#[cfg(test)]
pub(in crate::simd) mod tests {
    //! The lanes against their definition, lane by lane, for inputs the
    //! decoder never forms but the trait allows.

    use core::arch::x86_64::{__m256i, _mm256_storeu_si256};

    use super::lanes;
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

    #[test]
    fn the_lanes_follow_their_definition_on_extreme_inputs() {
        if !std::is_x86_feature_detected!("avx2") {
            return;
        }
        let all = extreme_params(4000);
        for pair in all.chunks_exact(2) {
            let pair = [pair[0], pair[1]];
            // SAFETY: AVX2 was detected.
            let l = unsafe { lanes(&pair) };
            let (alpha, beta, bs, tc0) = (words(l.alpha), words(l.beta), words(l.bs), words(l.tc0));
            for k in 0..16 {
                // Cb's eight lines in the low half, two lines a segment.
                let p = &pair[k / 8];
                let segment = (k % 8) / 2;
                assert_eq!(alpha[k], i16::from(p.alpha), "{p:?} lane {k}");
                assert_eq!(beta[k], i16::from(p.beta), "{p:?} lane {k}");
                assert_eq!(bs[k], i16::from(p.bs[segment]), "{p:?} lane {k}");
                assert_eq!(tc0[k], i16::from(p.tc0_at(segment)), "{p:?} lane {k}");
            }
        }
    }
}
