//! [`crate::Kernels::predict`] on x86-64: one check of the block's six
//! regions, then the luma interpolation on them (`luma_mc_ptrs`) and
//! both chroma planes in one pass, Cb in the low 128 bits of each vector
//! and Cr in the high.
//!
//! Memory is touched as in the luma interpolation (`luma_mc_into`): one
//! check of each region's whole extent (`crate::predict::place`), made
//! before any pointer exists, and then reads and writes of rows through
//! pointers inside those extents.

use core::arch::x86_64::{
    __m128i, __m256i, _mm_shuffle_epi32, _mm_unpacklo_epi8, _mm256_add_epi16, _mm256_add_epi32,
    _mm256_castsi256_si128, _mm256_extracti128_si256, _mm256_madd_epi16, _mm256_maddubs_epi16,
    _mm256_mulhrs_epi16, _mm256_packs_epi32, _mm256_packus_epi16, _mm256_set_m128i,
    _mm256_set1_epi16, _mm256_setzero_si256, _mm256_srav_epi32, _mm256_unpackhi_epi8,
    _mm256_unpacklo_epi8,
};
use core::ptr;

use super::{Average, Finish, Weigh, Weights, Words, low8, luma_mc_ptrs, vector8, weigh_full};
use crate::predict::{
    Macroblocks, Ops, Part, Placement, PlaneOp, Planes, PlanesMut, place, place_part,
};
use crate::views::{Block, BlockMut};
use crate::{Kernels, Weighting};

/// [`crate::Kernels::predict`]. `four` predicts a 4-wide luma block: the
/// dispatching kernels, so that such a block goes where
/// `Simd::prefers_luma_mc` sends it (`x86::luma4_padded`, or the reference
/// for a block that is only copied).
///
/// # Safety
///
/// AVX2 is available.
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments, reason = "the trait method's arguments")]
pub(in crate::simd) unsafe fn predict<K: Kernels>(
    four: &K,
    dst: PlanesMut<'_>,
    src: &Planes<'_>,
    px: usize,
    py: usize,
    w: usize,
    h: usize,
    mv: [i16; 2],
    ops: [PlaneOp; 3],
) -> bool {
    // SAFETY: AVX2 is available (this function's feature).
    unsafe { predict_body(four, dst, src, px, py, w, h, mv, ops) }
}

/// [`predict`]'s body, without the attribute, so that it is inlined
/// wherever it is called ([`crate::Kernels::predict_inlined`]): into a
/// caller compiled with AVX2 its AVX2 functions can be inlined too.
///
/// # Safety
///
/// AVX2 is available.
#[inline(always)]
#[allow(clippy::too_many_arguments, reason = "the trait method's arguments")]
pub(in crate::simd) unsafe fn predict_body<K: Kernels>(
    four: &K,
    dst: PlanesMut<'_>,
    src: &Planes<'_>,
    px: usize,
    py: usize,
    w: usize,
    h: usize,
    mv: [i16; 2],
    ops: [PlaneOp; 3],
) -> bool {
    let Some(p) = place(&dst, src, px, py, w, h, mv, ops) else {
        return false;
    };
    // SAFETY: AVX2 is available (the contract); `place` found `p` for
    // these planes, `src` and this size.
    unsafe { predict_placed(four, dst.into_planes(), src, &p, (w, h), ops) }
}

/// [`crate::Kernels::predict_part`], as [`predict`] is
/// [`crate::Kernels::predict`].
///
/// # Safety
///
/// AVX2 is available.
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments, reason = "the trait method's arguments")]
pub(in crate::simd) unsafe fn predict_part<K: Kernels>(
    four: &K,
    dst: [&mut [u8]; 3],
    mbs: &Macroblocks,
    mb: [usize; 2],
    part: Part,
    src: &Planes<'_>,
    mv: [i16; 2],
    ops: Ops,
) -> bool {
    // SAFETY: AVX2 is available (this function's feature).
    unsafe { predict_part_body(four, dst, mbs, mb, part, src, mv, ops) }
}

/// [`predict_part`]'s body, inlined wherever it is called, as
/// [`predict_body`].
///
/// # Safety
///
/// AVX2 is available.
#[inline(always)]
#[allow(clippy::too_many_arguments, reason = "the trait method's arguments")]
pub(in crate::simd) unsafe fn predict_part_body<K: Kernels>(
    four: &K,
    dst: [&mut [u8]; 3],
    mbs: &Macroblocks,
    mb: [usize; 2],
    part: Part,
    src: &Planes<'_>,
    mv: [i16; 2],
    ops: Ops,
) -> bool {
    let len = dst.each_ref().map(|plane| plane.len());
    let Some(p) = place_part(len, mbs, mb, part, src, mv) else {
        return false;
    };
    // SAFETY: AVX2 is available (the contract); `place_part` found `p`
    // for slices of these lengths, `src` and this size.
    unsafe { predict_placed(four, dst, src, &p, (part.w(), part.h()), ops.get()) }
}

/// The three planes of a `w` by `h` block at placement `p`.
///
/// # Safety
///
/// AVX2 is available, and `p` is the placement [`place`] or
/// [`place_part`] returned for a `w` by `h` block, slices of the lengths
/// of `[y, cb, cr]` and `src`: every region it names lies inside its
/// slice.
#[inline(always)]
unsafe fn predict_placed<K: Kernels>(
    four: &K,
    [y, cb, cr]: [&mut [u8]; 3],
    src: &Planes<'_>,
    p: &Placement,
    (w, h): (usize, usize),
    ops: [PlaneOp; 3],
) -> bool {
    let [luma, op_cb, op_cr] = ops;
    let stride = p.stride;
    if w == 4 {
        let (Some(luma_from), Some(luma_to)) = (src.y().get(p.luma_src..), y.get_mut(p.luma_dst..))
        else {
            return false;
        };
        let (Ok(from), Ok(to)) = (
            Block::new(luma_from, 0, stride, w + 5, h + 5),
            BlockMut::new(luma_to, 0, stride, w, h),
        ) else {
            return false;
        };
        match luma {
            PlaneOp::Put => four.luma_mc(to, from, p.xf, p.yf),
            PlaneOp::Avg => four.luma_mc_avg(to, from, p.xf, p.yf),
            PlaneOp::Weigh(weighting) => four.luma_mc_weighted(to, from, p.xf, p.yf, weighting),
        }
    } else {
        // SAFETY: AVX2 is available (the contract); the placement holds
        // that the window from `luma_src` holds `h + 5` rows of `w + 5`
        // samples and the block from `luma_dst` `h` rows of `w`, `stride`
        // apart, so both pointers lie inside their slices; `w` is 8 or 16
        // and `h` 4 to 16.
        unsafe {
            luma_block(
                y.as_mut_ptr().wrapping_add(p.luma_dst),
                src.y().as_ptr().wrapping_add(p.luma_src),
                stride,
                (w, h),
                (p.xf, p.yf),
                luma,
            );
        }
    }
    let (from, to) = (p.chroma_src, p.chroma_dst);
    // SAFETY: AVX2 is available; the placement holds that each window from
    // `chroma_src` holds `h / 2 + 1` rows of `w / 2 + 1` samples and each
    // block from `chroma_dst` `h / 2` rows of `w / 2`, `stride / 2` apart,
    // so the pointers lie inside their slices; `w / 2` is 2, 4 or 8 and
    // `h / 2` 2, 4 or 8.
    unsafe {
        chroma_block(
            [
                cb.as_mut_ptr().wrapping_add(to),
                cr.as_mut_ptr().wrapping_add(to),
            ],
            [
                src.cb().as_ptr().wrapping_add(from),
                src.cr().as_ptr().wrapping_add(from),
            ],
            stride / 2,
            (w / 2, h / 2),
            (p.cxf, p.cyf),
            [op_cb, op_cr],
        );
    }
    true
}

/// The luma plane of [`predict`], out of line: its 64 forms by width,
/// position and operation are most of the code, and inlined they would
/// crowd the checks' registers.
///
/// # Safety
///
/// AVX2 is available; `from` holds `h + 5` rows of `w + 5` samples and
/// `to` `h` rows of `w`, `stride` apart; `w` is 8 or 16 and `h` 1 to 16.
#[target_feature(enable = "avx2")]
#[inline(never)]
unsafe fn luma_block(
    to: *mut u8,
    from: *const u8,
    stride: usize,
    (w, h): (usize, usize),
    (xf, yf): (u8, u8),
    op: PlaneOp,
) {
    // SAFETY: the contract.
    unsafe {
        match op {
            PlaneOp::Put => luma_mc_ptrs(to, stride, from, stride, w, h, xf, yf, Average(false)),
            PlaneOp::Avg => luma_mc_ptrs(to, stride, from, stride, w, h, xf, yf, Average(true)),
            PlaneOp::Weigh(weighting) => {
                if xf | yf == 0 {
                    if let Some(words) = Words::new(weighting) {
                        let both = matches!(weighting, Weighting::Bi { .. });
                        return weigh_full(to, stride, from, stride, (w, h), words, both);
                    }
                }
                let finish = Weigh(Weights::new(weighting), weighting);
                luma_mc_ptrs(to, stride, from, stride, w, h, xf, yf, finish);
            }
        }
    }
}

/// Both chroma planes of [`predict`], out of line as [`luma_block`].
///
/// The two planes' operations pick the form of the loop, so that a row
/// is finished without a test of what to do with it: both written, both
/// averaged (the two most common, a list's prediction and the second of
/// two lists), both weighted, or each as its own [`Finish`] says.
///
/// # Safety
///
/// AVX2 is available; each `from` holds `h + 1` rows of `w + 1` samples
/// and each `to` `h` rows of `w`, `stride` apart; `w` is 2, 4 or 8 and
/// `h` 2, 4 or 8.
#[target_feature(enable = "avx2")]
#[inline(never)]
unsafe fn chroma_block(
    to: [*mut u8; 2],
    from: [*const u8; 2],
    stride: usize,
    (w, h): (usize, usize),
    (xf, yf): (u8, u8),
    ops: [PlaneOp; 2],
) {
    let block = Chroma {
        to,
        from,
        stride,
        h,
        xf: xf & 7,
        yf: yf & 7,
    };
    // SAFETY: the contract.
    unsafe {
        match ops {
            [PlaneOp::Put, PlaneOp::Put] => block.by_width(w, Put),
            [PlaneOp::Avg, PlaneOp::Avg] => block.by_width(w, Avg),
            [PlaneOp::Weigh(cb), PlaneOp::Weigh(cr)] => {
                let (cb, cr) = (Weights::new(cb), Weights::new(cr));
                match (cb.both, cr.both) {
                    (false, false) => block.by_width(w, Weighed::<false>::new(cb, cr)),
                    (true, true) => block.by_width(w, Weighed::<true>::new(cb, cr)),
                    _ => block.by_width(w, [Finish::Weigh(cb), Finish::Weigh(cr)]),
                }
            }
            [cb, cr] => block.by_width(w, [finish_of(cb), finish_of(cr)]),
        }
    }
}

/// The chroma kernels' form of a plane's operation.
#[inline(always)]
fn finish_of(op: PlaneOp) -> Finish {
    match op {
        PlaneOp::Put => Finish::Write,
        PlaneOp::Avg => Finish::Average,
        PlaneOp::Weigh(weighting) => Finish::weigh(weighting),
    }
}

/// What [`chroma_block`] does with both planes' predicted rows, `pred`:
/// Cb's in the low eight bytes and Cr's in bytes 16 to 23, as [`round`]
/// leaves them. `old` reads a plane's rows of the block (0 Cb, 1 Cr) and is
/// called only where the operation needs them. Returns the rows to store,
/// Cb's then Cr's, each in the low eight bytes.
trait Finishes: Copy {
    /// # Safety
    ///
    /// AVX2 is available.
    unsafe fn apply(self, pred: __m256i, old: impl Fn(usize) -> __m128i) -> [__m128i; 2];
}

/// The two planes of `v`'s 128-bit halves, Cb's (low) first.
///
/// # Safety
///
/// AVX2 is available.
#[inline(always)]
unsafe fn halves(v: __m256i) -> [__m128i; 2] {
    // SAFETY: AVX2 is available; register operations.
    unsafe { [_mm256_castsi256_si128(v), _mm256_extracti128_si256::<1>(v)] }
}

/// Both planes written.
#[derive(Clone, Copy)]
struct Put;

/// Both planes averaged into the block, `(dst + pred + 1) >> 1`.
#[derive(Clone, Copy)]
struct Avg;

/// Both planes weighted the same way, both from one list (`BI` false,
/// Equations 8-286 and 8-287) or both from two (`BI`, Equation 8-288, list
/// 0's samples the block's), each plane with its own weights.
#[derive(Clone, Copy)]
struct Weighed<const BI: bool> {
    /// Cb's `w0` and `w1` in the low 128 bits ([`Weights`]'s `pair`), Cr's
    /// in the high.
    pair: __m256i,
    /// The rounding and the offset, likewise.
    bias: __m256i,
    /// The shift, likewise, in each 32-bit lane.
    shift: __m256i,
}

impl Finishes for Put {
    #[inline(always)]
    unsafe fn apply(self, pred: __m256i, _: impl Fn(usize) -> __m128i) -> [__m128i; 2] {
        // SAFETY: AVX2 is available (the contract).
        unsafe { halves(pred) }
    }
}

impl Finishes for Avg {
    #[inline(always)]
    unsafe fn apply(self, pred: __m256i, old: impl Fn(usize) -> __m128i) -> [__m128i; 2] {
        // SAFETY: AVX2 is available (the contract).
        let [cb, cr] = unsafe { halves(pred) };
        [
            Finish::Average.apply(cb, || old(0)),
            Finish::Average.apply(cr, || old(1)),
        ]
    }
}

impl<const BI: bool> Weighed<BI> {
    /// The weights of both planes, `cb`'s and `cr`'s, each of [`Weights`]'s
    /// `both` equal to `BI`.
    ///
    /// # Safety
    ///
    /// AVX2 is available.
    #[inline(always)]
    unsafe fn new(cb: Weights, cr: Weights) -> Weighed<BI> {
        // SAFETY: AVX2 is available; register operations. `shift` holds
        // the count in its low 32 bits, which the shuffle repeats.
        unsafe {
            Weighed {
                pair: _mm256_set_m128i(cr.pair, cb.pair),
                bias: _mm256_set_m128i(cr.bias, cb.bias),
                shift: _mm256_set_m128i(
                    _mm_shuffle_epi32::<0>(cr.shift),
                    _mm_shuffle_epi32::<0>(cb.shift),
                ),
            }
        }
    }
}

impl<const BI: bool> Finishes for Weighed<BI> {
    /// `weigh8` of both planes in one pass, each 128-bit half with its own
    /// plane's weights (`vpsravd` shifts each lane by its own count).
    #[inline(always)]
    unsafe fn apply(self, pred: __m256i, old: impl Fn(usize) -> __m128i) -> [__m128i; 2] {
        // SAFETY: AVX2 is available (the contract); register operations.
        unsafe {
            let x = if BI {
                _mm256_set_m128i(old(1), old(0))
            } else {
                pred
            };
            let zero = _mm256_setzero_si256();
            let pairs = _mm256_unpacklo_epi8(x, pred);
            let weigh = |lanes: __m256i| {
                _mm256_srav_epi32(
                    _mm256_add_epi32(_mm256_madd_epi16(lanes, self.pair), self.bias),
                    self.shift,
                )
            };
            let first = weigh(_mm256_unpacklo_epi8(pairs, zero));
            let second = weigh(_mm256_unpackhi_epi8(pairs, zero));
            // Saturating to 16 bits and then to 0 to 255 is the clip.
            let words = _mm256_packs_epi32(first, second);
            halves(_mm256_packus_epi16(words, words))
        }
    }
}

impl Finishes for [Finish; 2] {
    #[inline(always)]
    unsafe fn apply(self, pred: __m256i, old: impl Fn(usize) -> __m128i) -> [__m128i; 2] {
        let [cb, cr] = self;
        // SAFETY: AVX2 is available (the contract).
        let [pred_cb, pred_cr] = unsafe { halves(pred) };
        [cb.apply(pred_cb, || old(0)), cr.apply(pred_cr, || old(1))]
    }
}

/// The `W` bytes at `p`, in the low bytes of a word.
///
/// # Safety
///
/// `p` holds `W` bytes; `W` is at most 8.
#[inline(always)]
unsafe fn read_row<const W: usize>(p: *const u8) -> u64 {
    // SAFETY: the contract; an unaligned read of `W` bytes.
    let row: [u8; W] = unsafe { ptr::read_unaligned(p.cast::<[u8; W]>()) };
    let mut word = [0u8; 8];
    if let Some(low) = word.first_chunk_mut::<W>() {
        *low = row;
    }
    u64::from_le_bytes(word)
}

/// The low `W` bytes of `word`, written at `p`.
///
/// # Safety
///
/// `p` holds `W` bytes; `W` is at most 8.
#[inline(always)]
unsafe fn write_row<const W: usize>(p: *mut u8, word: u64) {
    if let Some(row) = word.to_le_bytes().first_chunk::<W>() {
        // SAFETY: the contract; an unaligned write of `W` bytes.
        unsafe { ptr::write_unaligned(p.cast::<[u8; W]>(), *row) };
    }
}

/// `K` rows of `W` bytes from `p`, `stride` apart, one after the other in
/// a word: row `k` in bytes `k W` to `k W + W - 1`.
///
/// # Safety
///
/// `p` holds `K` rows of `W` bytes, `stride` apart; `K W` is at most 8.
#[inline(always)]
unsafe fn read_rows<const W: usize, const K: usize>(p: *const u8, stride: usize) -> u64 {
    let mut word = 0;
    for k in 0..K {
        // SAFETY: the contract: row `k` lies inside.
        word |= unsafe { read_row::<W>(p.wrapping_add(k * stride)) } << (8 * W * k);
    }
    word
}

/// [`read_rows`]'s inverse: the `K` rows of `word` written from `p`.
///
/// # Safety
///
/// As [`read_rows`], for writing.
#[inline(always)]
unsafe fn write_rows<const W: usize, const K: usize>(p: *mut u8, stride: usize, word: u64) {
    for k in 0..K {
        // SAFETY: the contract: row `k` lies inside.
        unsafe { write_row::<W>(p.wrapping_add(k * stride), word >> (8 * W * k)) };
    }
}

/// The weights of Equation 8-266 as `pmaddubsw` takes them, by yFracC and
/// xFracC: `(8 - xFracC) (8 - yFracC)` and `xFracC (8 - yFracC)` for a
/// row's sample and its right neighbour (`A` and `B`), and `(8 - xFracC)
/// yFracC` and `xFracC yFracC` for the row below's (`C` and `D`), each a
/// pair of bytes in a 16-bit lane. Each weight is at most 64.
#[expect(
    clippy::indexing_slicing,
    reason = "constant evaluation: an index out of range fails the build"
)]
const WEIGHTS: [[[i16; 2]; 8]; 8] = {
    let mut table = [[[0; 2]; 8]; 8];
    let mut yf = 0;
    while yf < 8 {
        let mut xf = 0;
        while xf < 8 {
            let (x, y) = (8 - xf, 8 - yf);
            table[yf][xf] = [
                i16::from_le_bytes([(x * y) as u8, (xf * y) as u8]),
                i16::from_le_bytes([(x * yf) as u8, (xf * yf) as u8]),
            ];
            xf += 1;
        }
        yf += 1;
    }
    table
};

/// [`WEIGHTS`] of a position, in each lane of a vector: `A` and `B`, then
/// `C` and `D`.
///
/// # Safety
///
/// AVX2 is available.
#[inline(always)]
unsafe fn weights(xf: u8, yf: u8) -> [__m256i; 2] {
    let [ab, cd] = WEIGHTS
        .get(usize::from(yf & 7))
        .and_then(|row| row.get(usize::from(xf & 7)))
        .copied()
        .unwrap_or_default();
    // SAFETY: AVX2 is available; register operations.
    unsafe { [_mm256_set1_epi16(ab), _mm256_set1_epi16(cd)] }
}

/// Each of `K` rows' first `W` samples and its right neighbour, as byte
/// pairs, for both planes: Cb's in the low 128 bits, Cr's in the high.
///
/// # Safety
///
/// AVX2 is available, and each `p` holds `K` rows of `W + 1` bytes,
/// `stride` apart.
#[inline(always)]
unsafe fn pairs2<const W: usize, const K: usize>(p: [*const u8; 2], stride: usize) -> __m256i {
    let [cb, cr] = p;
    // SAFETY: the contract.
    unsafe { _mm256_set_m128i(pairs::<W, K>(cr, stride), pairs::<W, K>(cb, stride)) }
}

/// Each of `K` rows' first `W` samples at `p` and its right neighbour, as
/// byte pairs.
///
/// # Safety
///
/// `p` holds `K` rows of `W + 1` bytes, `stride` apart.
#[inline(always)]
unsafe fn pairs<const W: usize, const K: usize>(p: *const u8, stride: usize) -> __m128i {
    // SAFETY: the contract; SSE2 is part of the x86-64 baseline.
    unsafe {
        let left = vector8(read_rows::<W, K>(p, stride).to_le_bytes());
        let right = vector8(read_rows::<W, K>(p.wrapping_add(1), stride).to_le_bytes());
        _mm_unpacklo_epi8(left, right)
    }
}

/// The end of Equation 8-266 for `A a + B b + C c + D d` in each 16-bit
/// lane: `(sum + 32) >> 6` as one `pmulhrsw` by 512, which is `(512 sum +
/// 2^14) >> 15`, the same value. The four weights add to 64, so each
/// `pmaddubsw` before and the sum are at most 64 * 255 and none
/// saturates. Packed to bytes: Cb's in the low eight bytes, Cr's in bytes
/// 16 to 23.
///
/// # Safety
///
/// AVX2 is available.
#[inline(always)]
unsafe fn round(sum: __m256i) -> __m256i {
    // SAFETY: AVX2 is available; register operations.
    unsafe {
        let v = _mm256_mulhrs_epi16(sum, _mm256_set1_epi16(512));
        _mm256_packus_epi16(v, v)
    }
}

/// Both chroma planes of a block: where they are read and written, its
/// rows and its fractions (xFracC and yFracC, 0 to 7).
#[derive(Clone, Copy)]
struct Chroma {
    to: [*mut u8; 2],
    from: [*const u8; 2],
    stride: usize,
    h: usize,
    xf: u8,
    yf: u8,
}

impl Chroma {
    /// [`Chroma::rows`] for the block's width, `K` rows a step: one row of
    /// 8, two of 4 or two of 2, eight bytes or fewer a plane.
    ///
    /// # Safety
    ///
    /// AVX2 is available; each `from` holds `h + 1` rows of `w + 1`
    /// samples and each `to` `h` rows of `w`, `stride` apart; `w` is 2, 4
    /// or 8 and `h` 2, 4 or 8.
    #[inline(always)]
    unsafe fn by_width<F: Finishes>(self, w: usize, finish: F) {
        // SAFETY: the contract; `h` is a multiple of the rows each step
        // takes.
        unsafe {
            match w {
                8 => self.rows::<8, 1, F>(finish),
                4 => self.rows::<4, 2, F>(finish),
                _ => self.rows::<2, 2, F>(finish),
            }
        }
    }

    /// Chroma eighth-sample interpolation (Equation 8-266) of both planes
    /// of a `W` by `h` block, `K` rows a step (`K W` at most 8), each
    /// plane's rows finished as `finish` says. A full-sample position
    /// copies the source rows; any other runs the equation.
    ///
    /// # Safety
    ///
    /// AVX2 is available; each `from` holds `h + 1` rows of `W + 1`
    /// samples, `stride` apart, and each `to` `h` rows of `W`; `h` is a
    /// multiple of `K` and at most 16.
    #[inline(always)]
    unsafe fn rows<const W: usize, const K: usize, F: Finishes>(self, finish: F) {
        let Chroma {
            to,
            from,
            stride,
            h,
            xf,
            yf,
        } = self;
        if h % K != 0 || h > 16 {
            return;
        }
        let step = K * stride;
        let [mut to_cb, mut to_cr] = to;
        let [mut cb, mut cr] = from;
        // SAFETY: AVX2 is available. Step `r` (`r` rows done) reads source
        // rows `r` to `r + K` and writes block rows `r` to `r + K - 1`;
        // with `r + K` at most `h`, the window's `h + 1` rows and the
        // block's `h` hold them (the contract). The pointers step on with
        // `wrapping_add`, so the steps past the last row form no pointer
        // that is used.
        unsafe {
            if xf == 0 && yf == 0 {
                for _ in 0..h / K {
                    let pred = _mm256_set_m128i(
                        vector8(read_rows::<W, K>(cr, stride).to_le_bytes()),
                        vector8(read_rows::<W, K>(cb, stride).to_le_bytes()),
                    );
                    store::<W, K, F>([to_cb, to_cr], stride, pred, finish);
                    (cb, cr) = (cb.wrapping_add(step), cr.wrapping_add(step));
                    (to_cb, to_cr) = (to_cb.wrapping_add(step), to_cr.wrapping_add(step));
                }
                return;
            }
            let [ab, cd] = weights(xf, yf);
            // With one row a step, the next step's rows above are this
            // one's below; with more, the rows overlap and are read again.
            let mut top = if K == 1 {
                pairs2::<W, K>([cb, cr], stride)
            } else {
                _mm256_setzero_si256()
            };
            for _ in 0..h / K {
                if K != 1 {
                    top = pairs2::<W, K>([cb, cr], stride);
                }
                let below = [cb.wrapping_add(stride), cr.wrapping_add(stride)];
                let bottom = pairs2::<W, K>(below, stride);
                let sum = _mm256_add_epi16(
                    _mm256_maddubs_epi16(top, ab),
                    _mm256_maddubs_epi16(bottom, cd),
                );
                store::<W, K, F>([to_cb, to_cr], stride, round(sum), finish);
                top = bottom;
                (cb, cr) = (cb.wrapping_add(step), cr.wrapping_add(step));
                (to_cb, to_cr) = (to_cb.wrapping_add(step), to_cr.wrapping_add(step));
            }
        }
    }
}

/// `K` rows of both planes' prediction `pred` (as [`round`] leaves it),
/// finished into the blocks' rows at `to` as `finish` says.
///
/// # Safety
///
/// AVX2 is available, and each `to` holds `K` rows of `W` bytes, `stride`
/// apart.
#[inline(always)]
unsafe fn store<const W: usize, const K: usize, F: Finishes>(
    to: [*mut u8; 2],
    stride: usize,
    pred: __m256i,
    finish: F,
) {
    let [to_cb, to_cr] = to;
    let old = |plane: usize| {
        let at = if plane == 0 { to_cb } else { to_cr };
        // SAFETY: the contract.
        vector8(unsafe { read_rows::<W, K>(at, stride) }.to_le_bytes())
    };
    // SAFETY: the contract.
    let [cb, cr] = unsafe { finish.apply(pred, old) };
    // SAFETY: the contract.
    unsafe {
        write_rows::<W, K>(to_cb, stride, u64::from_le_bytes(low8(cb)));
        write_rows::<W, K>(to_cr, stride, u64::from_le_bytes(low8(cr)));
    }
}
