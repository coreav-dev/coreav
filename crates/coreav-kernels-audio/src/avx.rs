//! The x86-64 kernel: AVX, eight lanes, four output samples at a time.
//!
//! `unsafe` here is of two kinds, listed in `docs/unsafe-sites.md`: the
//! call into the `#[target_feature(enable = "avx")]` function, which only an
//! [`Avx`] makes and only [`Avx::detect`] creates where the CPU has AVX;
//! and the unaligned loads of eight floats, at offsets `j < taps` from the
//! start of a window that lies inside its slice (`start + taps <= len`), so
//! every load ends inside it: [`Avx::run`] takes the windows as a
//! [`Checked`], which only `Checked::new` makes, after that check.
#![allow(unsafe_code)]

use crate::Checked;
use crate::fft::{Lane, Twiddles, pass_lanes, rotate_lanes};
use core::arch::x86_64::{
    __m256, _mm_add_ps, _mm_cvtss_f32, _mm_movehl_ps, _mm_shuffle_ps, _mm256_add_ps,
    _mm256_castps256_ps128, _mm256_extractf128_ps, _mm256_hadd_ps, _mm256_loadu_ps, _mm256_mul_ps,
    _mm256_set1_ps, _mm256_setzero_ps, _mm256_storeu_ps, _mm256_sub_ps, _mm256_xor_ps,
};

/// Proof that the CPU has AVX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Avx(());

impl Avx {
    pub(crate) fn detect() -> Option<Self> {
        std::arch::is_x86_feature_detected!("avx").then_some(Self(()))
    }

    /// See [`crate::Fir::run`].
    pub(crate) fn run(self, checked: Checked<'_>, out: &mut [f32]) {
        let (input, table, taps, windows) = (
            checked.input(),
            checked.table(),
            checked.taps(),
            checked.windows(),
        );
        // SAFETY: an `Avx` exists only where the CPU has AVX; a `Checked`
        // exists only once `Checked::new` found `taps` a positive multiple
        // of 8 and every window inside its slices; `out` is written through
        // a slice.
        unsafe { run(input, table, taps, windows, out) }
    }
}

/// The products of one window, eight lanes, lane `i` taking taps `i`, `i +
/// 8`, ... in order.
///
/// # Safety
/// `x` and `h` point to `taps` readable floats, `taps` a multiple of 8.
#[target_feature(enable = "avx")]
#[inline]
unsafe fn lanes(x: *const f32, h: *const f32, taps: usize) -> __m256 {
    let mut acc = _mm256_setzero_ps();
    let mut j = 0;
    while j < taps {
        // SAFETY: j + 8 <= taps, so both loads end inside the windows.
        let (a, b) = unsafe { (_mm256_loadu_ps(x.add(j)), _mm256_loadu_ps(h.add(j))) };
        acc = _mm256_add_ps(acc, _mm256_mul_ps(a, b));
        j += 8;
    }
    acc
}

/// [`lanes`] of four windows in one loop: each output's lanes add the same
/// products in the same order as [`lanes`] would, but the four sums are
/// independent, so their additions overlap instead of each waiting for the
/// one before it.
///
/// # Safety
/// Each `x[i]` and `h[i]` points to `taps` readable floats, `taps` a
/// multiple of 8.
#[target_feature(enable = "avx")]
#[inline]
unsafe fn lanes4(x: [*const f32; 4], h: [*const f32; 4], taps: usize) -> [__m256; 4] {
    let mut acc = [_mm256_setzero_ps(); 4];
    let mut j = 0;
    while j < taps {
        for ((a, &x), &h) in acc.iter_mut().zip(&x).zip(&h) {
            // SAFETY: j + 8 <= taps, so both loads end inside the windows.
            let (u, v) = unsafe { (_mm256_loadu_ps(x.add(j)), _mm256_loadu_ps(h.add(j))) };
            *a = _mm256_add_ps(*a, _mm256_mul_ps(u, v));
        }
        j += 8;
    }
    acc
}

/// One output: the lanes summed as `((l0 + l1) + (l2 + l3)) + ((l4 + l5) +
/// (l6 + l7))`.
#[target_feature(enable = "avx")]
#[inline]
fn sum(acc: __m256) -> f32 {
    // [l0+l1, l2+l3, l0+l1, l2+l3 | l4+l5, l6+l7, ...]
    let pairs = _mm256_hadd_ps(acc, acc);
    // [(l0+l1)+(l2+l3), ... | (l4+l5)+(l6+l7), ...]
    let quads = _mm256_hadd_ps(pairs, pairs);
    let low = _mm256_castps256_ps128(quads);
    let high = _mm256_extractf128_ps::<1>(quads);
    _mm_cvtss_f32(_mm_add_ps(low, high))
}

/// Four outputs: the same sums, four at once.
#[target_feature(enable = "avx")]
#[inline]
fn sum4(a: __m256, b: __m256, c: __m256, d: __m256) -> [f32; 4] {
    // [a01, a23, b01, b23 | a45, a67, b45, b67]
    let ab = _mm256_hadd_ps(a, b);
    let cd = _mm256_hadd_ps(c, d);
    // [a0123, b0123, c0123, d0123 | a4567, b4567, c4567, d4567]
    let all = _mm256_hadd_ps(ab, cd);
    let s = _mm_add_ps(_mm256_castps256_ps128(all), _mm256_extractf128_ps::<1>(all));
    [
        _mm_cvtss_f32(s),
        _mm_cvtss_f32(_mm_shuffle_ps::<1>(s, s)),
        _mm_cvtss_f32(_mm_movehl_ps(s, s)),
        _mm_cvtss_f32(_mm_shuffle_ps::<3>(s, s)),
    ]
}

/// # Safety
/// AVX is available; `taps` is a positive multiple of 8; each `(start,
/// row)` has `start + taps <= input.len()` and `row + taps <= table.len()`.
/// `out` is written through slices, so its length is not a condition (a
/// short one leaves outputs unwritten).
#[target_feature(enable = "avx")]
unsafe fn run(
    input: &[f32],
    table: &[f32],
    taps: usize,
    windows: &[(usize, usize)],
    out: &mut [f32],
) {
    let (x, h) = (input.as_ptr(), table.as_ptr());
    let mut outs = out.chunks_exact_mut(4);
    let mut groups = windows.chunks_exact(4);
    for (o, w) in (&mut outs).zip(&mut groups) {
        if let ([o0, o1, o2, o3], [w0, w1, w2, w3]) = (o, w) {
            // SAFETY: each window was checked to lie inside its slice.
            let [a, b, c, d] = unsafe {
                lanes4(
                    [x.add(w0.0), x.add(w1.0), x.add(w2.0), x.add(w3.0)],
                    [h.add(w0.1), h.add(w1.1), h.add(w2.1), h.add(w3.1)],
                    taps,
                )
            };
            [*o0, *o1, *o2, *o3] = sum4(a, b, c, d);
        }
    }
    for (o, &(start, row)) in outs.into_remainder().iter_mut().zip(groups.remainder()) {
        // SAFETY: as above.
        *o = sum(unsafe { lanes(x.add(start), h.add(row), taps) });
    }
}

/// Eight complex numbers in two registers: the AVX lanes of the FFT
/// kernels. Private to this module, made only inside the
/// `#[target_feature(enable = "avx")]` functions below, which only an
/// [`Avx`] calls: every intrinsic here runs where the CPU has AVX.
#[derive(Clone, Copy)]
struct Oct {
    re: __m256,
    im: __m256,
}

impl Lane for Oct {
    const WIDTH: usize = 8;

    #[inline(always)]
    fn one() -> Self {
        // SAFETY: see `Oct`.
        unsafe {
            Self {
                re: _mm256_set1_ps(1.0),
                im: _mm256_setzero_ps(),
            }
        }
    }

    #[inline(always)]
    fn load(re: &[f32], im: &[f32]) -> Option<Self> {
        let (r, i) = (re.first_chunk::<8>()?, im.first_chunk::<8>()?);
        // SAFETY: see `Oct`; each load reads the eight floats of a `&[f32; 8]`.
        unsafe {
            Some(Self {
                re: _mm256_loadu_ps(r.as_ptr()),
                im: _mm256_loadu_ps(i.as_ptr()),
            })
        }
    }

    #[inline(always)]
    fn store(self, re: &mut [f32], im: &mut [f32]) {
        if let (Some(r), Some(i)) = (re.first_chunk_mut::<8>(), im.first_chunk_mut::<8>()) {
            // SAFETY: see `Oct`; each store writes the eight floats of a
            // `&mut [f32; 8]`.
            unsafe {
                _mm256_storeu_ps(r.as_mut_ptr(), self.re);
                _mm256_storeu_ps(i.as_mut_ptr(), self.im);
            }
        }
    }

    #[inline(always)]
    fn mul(self, o: Self) -> Self {
        // SAFETY: see `Oct`.
        unsafe {
            let rr = _mm256_mul_ps(self.re, o.re);
            let ii = _mm256_mul_ps(self.im, o.im);
            let ri = _mm256_mul_ps(self.re, o.im);
            let ir = _mm256_mul_ps(self.im, o.re);
            Self {
                re: _mm256_sub_ps(rr, ii),
                im: _mm256_add_ps(ri, ir),
            }
        }
    }

    #[inline(always)]
    fn add(self, o: Self) -> Self {
        // SAFETY: see `Oct`.
        unsafe {
            Self {
                re: _mm256_add_ps(self.re, o.re),
                im: _mm256_add_ps(self.im, o.im),
            }
        }
    }

    #[inline(always)]
    fn sub(self, o: Self) -> Self {
        // SAFETY: see `Oct`.
        unsafe {
            Self {
                re: _mm256_sub_ps(self.re, o.re),
                im: _mm256_sub_ps(self.im, o.im),
            }
        }
    }

    #[inline(always)]
    fn scale(self, s: f32) -> Self {
        // SAFETY: see `Oct`.
        unsafe {
            let s = _mm256_set1_ps(s);
            Self {
                re: _mm256_mul_ps(self.re, s),
                im: _mm256_mul_ps(self.im, s),
            }
        }
    }

    #[inline(always)]
    fn times_minus_i(self) -> Self {
        // SAFETY: see `Oct`. The negation flips the sign bit, as `-x` does.
        unsafe {
            Self {
                re: self.im,
                im: _mm256_xor_ps(self.re, _mm256_set1_ps(-0.0)),
            }
        }
    }
}

impl Avx {
    /// See [`crate::Fft::pass`]; `m` is a multiple of 8.
    pub(crate) fn pass(self, m: usize, w: &[Twiddles<'_>], re: &mut [f32], im: &mut [f32]) {
        // SAFETY: an `Avx` exists only where the CPU has AVX.
        unsafe { fft_pass(m, w, re, im) }
    }

    /// See [`crate::Fft::rotate`].
    pub(crate) fn rotate(self, re: &mut [f32], im: &mut [f32], w: Twiddles<'_>) {
        // SAFETY: an `Avx` exists only where the CPU has AVX.
        unsafe { fft_rotate(re, im, w) }
    }
}

#[target_feature(enable = "avx")]
fn fft_pass(m: usize, w: &[Twiddles<'_>], re: &mut [f32], im: &mut [f32]) {
    pass_lanes::<Oct>(m, w, re, im);
}

#[target_feature(enable = "avx")]
fn fft_rotate(re: &mut [f32], im: &mut [f32], w: Twiddles<'_>) {
    rotate_lanes::<Oct>(re, im, w);
}
