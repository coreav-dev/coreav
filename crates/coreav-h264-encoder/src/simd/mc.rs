use super::*;
use coreav_kernels_h264::{Block, BlockMut, Kernels, Scalar};

impl Avx2 {
    pub(crate) fn luma_mc(self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        if !matches!(w, 4 | 8 | 16)
            || !(1..=16).contains(&h)
            || src.w() != w + 5
            || src.h() != h + 5
        {
            return Scalar.luma_mc(dst, src, xf, yf);
        }
        unsafe {
            match w {
                4 => luma::<4>(dst, src, xf & 3, yf & 3),
                8 => luma::<8>(dst, src, xf & 3, yf & 3),
                16 => luma::<16>(dst, src, xf & 3, yf & 3),
                _ => {}
            }
        }
    }

    pub(crate) fn chroma_mc(self, dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
        let (w, h) = (dst.w(), dst.h());
        if !matches!(w, 2 | 4 | 8) || !(1..=8).contains(&h) || src.w() != w + 1 || src.h() != h + 1
        {
            return Scalar.chroma_mc(dst, src, xf, yf);
        }
        unsafe {
            match w {
                2 => chroma::<2>(dst, src, xf & 7, yf & 7),
                4 => chroma::<4>(dst, src, xf & 7, yf & 7),
                8 => chroma::<8>(dst, src, xf & 7, yf & 7),
                _ => {}
            }
        }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn load<const W: usize>(p: *const u8) -> __m256i {
    let bytes = unsafe {
        match W {
            2 => _mm_cvtsi32_si128(i32::from(p.cast::<u16>().read_unaligned())),
            4 => _mm_cvtsi32_si128(p.cast::<i32>().read_unaligned()),
            8 => _mm_loadl_epi64(p.cast()),
            16 => _mm_loadu_si128(p.cast()),
            _ => _mm_setzero_si128(),
        }
    };
    _mm256_cvtepu8_epi16(bytes)
}

#[target_feature(enable = "avx2")]
unsafe fn store<const W: usize>(p: *mut u8, v: __m256i) {
    let bytes = _mm_packus_epi16(_mm256_castsi256_si128(v), _mm256_extracti128_si256::<1>(v));
    unsafe {
        match W {
            2 => p
                .cast::<u16>()
                .write_unaligned(_mm_extract_epi16::<0>(bytes) as u16),
            4 => p.cast::<i32>().write_unaligned(_mm_cvtsi128_si32(bytes)),
            8 => _mm_storel_epi64(p.cast(), bytes),
            16 => _mm_storeu_si128(p.cast(), bytes),
            _ => {}
        }
    }
}

#[target_feature(enable = "avx2")]
fn six(a: __m256i, b: __m256i, c: __m256i, d: __m256i, e: __m256i, f: __m256i) -> __m256i {
    let outer = _mm256_add_epi16(a, f);
    let inner = _mm256_mullo_epi16(_mm256_add_epi16(c, d), _mm256_set1_epi16(20));
    let middle = _mm256_mullo_epi16(_mm256_add_epi16(b, e), _mm256_set1_epi16(5));
    _mm256_sub_epi16(_mm256_add_epi16(outer, inner), middle)
}

#[target_feature(enable = "avx2")]
fn half(v: __m256i) -> __m256i {
    clip(_mm256_srai_epi16::<5>(_mm256_add_epi16(
        v,
        _mm256_set1_epi16(16),
    )))
}

#[target_feature(enable = "avx2")]
fn clip(v: __m256i) -> __m256i {
    _mm256_min_epi16(
        _mm256_max_epi16(v, _mm256_setzero_si256()),
        _mm256_set1_epi16(255),
    )
}

#[target_feature(enable = "avx2")]
unsafe fn horizontal<const W: usize>(p: *const u8) -> __m256i {
    unsafe {
        six(
            load::<W>(p),
            load::<W>(p.add(1)),
            load::<W>(p.add(2)),
            load::<W>(p.add(3)),
            load::<W>(p.add(4)),
            load::<W>(p.add(5)),
        )
    }
}

#[target_feature(enable = "avx2")]
unsafe fn vertical<const W: usize>(p: *const u8, stride: usize) -> __m256i {
    unsafe {
        half(six(
            load::<W>(p),
            load::<W>(p.add(stride)),
            load::<W>(p.add(2 * stride)),
            load::<W>(p.add(3 * stride)),
            load::<W>(p.add(4 * stride)),
            load::<W>(p.add(5 * stride)),
        ))
    }
}

#[target_feature(enable = "avx2")]
fn diagonal([a, b, c, d, e, f]: [__m256i; 6]) -> __m256i {
    let one = _mm256_set1_epi16(1);
    let five = _mm256_set1_epi16(5);
    let twenty = _mm256_set1_epi16(20);
    let sum = |outer, middle, inner| {
        _mm256_srai_epi32::<10>(_mm256_add_epi32(
            _mm256_set1_epi32(512),
            _mm256_sub_epi32(_mm256_add_epi32(outer, inner), middle),
        ))
    };
    let lo = sum(
        _mm256_madd_epi16(_mm256_unpacklo_epi16(a, f), one),
        _mm256_madd_epi16(_mm256_unpacklo_epi16(b, e), five),
        _mm256_madd_epi16(_mm256_unpacklo_epi16(c, d), twenty),
    );
    let hi = sum(
        _mm256_madd_epi16(_mm256_unpackhi_epi16(a, f), one),
        _mm256_madd_epi16(_mm256_unpackhi_epi16(b, e), five),
        _mm256_madd_epi16(_mm256_unpackhi_epi16(c, d), twenty),
    );
    clip(_mm256_packs_epi32(lo, hi))
}

#[target_feature(enable = "avx2")]
unsafe fn luma<const W: usize>(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    macro_rules! phases {
        ($($x:literal, $y:literal;)*) => {
            match (xf, yf) {
                $(($x, $y) => unsafe { interpolate::<W, $x, $y>(dst, src) },)*
                _ => {}
            }
        };
    }
    phases! {
        0, 0; 1, 0; 2, 0; 3, 0;
        0, 1; 1, 1; 2, 1; 3, 1;
        0, 2; 1, 2; 2, 2; 3, 2;
        0, 3; 1, 3; 2, 3; 3, 3;
    }
}

#[target_feature(enable = "avx2")]
unsafe fn interpolate<const W: usize, const X: u8, const Y: u8>(
    mut dst: BlockMut<'_>,
    src: Block<'_>,
) {
    let (stride, out_stride, h) = (src.stride(), dst.stride(), dst.h());
    let (from, to) = (src.samples().as_ptr(), dst.samples_mut().as_mut_ptr());
    let j = X != 0 && Y != 0 && (X == 2 || Y == 2);
    let mut rows = [_mm256_setzero_si256(); 6];
    unsafe {
        if j {
            for (y, row) in rows.iter_mut().take(5).enumerate() {
                *row = horizontal::<W>(from.add(y * stride));
            }
        }
        for y in 0..h {
            let p = from.add(y * stride);
            let b = if X != 0 && Y != 2 {
                half(horizontal::<W>(p.add((2 + usize::from(Y == 3)) * stride)))
            } else {
                _mm256_setzero_si256()
            };
            let h = if Y != 0 && X != 2 {
                vertical::<W>(p.add(2 + usize::from(X == 3)), stride)
            } else {
                _mm256_setzero_si256()
            };
            let d = if j {
                if let Some(last) = rows.last_mut() {
                    *last = horizontal::<W>(p.add(5 * stride));
                }
                diagonal(rows)
            } else {
                _mm256_setzero_si256()
            };
            let int =
                || load::<W>(p.add((2 + usize::from(Y == 3)) * stride + 2 + usize::from(X == 3)));
            let v = match (X, Y) {
                (0, 0) => int(),
                (2, 0) => b,
                (0, 2) => h,
                (2, 2) => d,
                (1 | 3, 0) => _mm256_avg_epu16(int(), b),
                (0, 1 | 3) => _mm256_avg_epu16(int(), h),
                (1 | 3, 1 | 3) => _mm256_avg_epu16(b, h),
                (2, 1 | 3) => _mm256_avg_epu16(d, b),
                (1 | 3, 2) => _mm256_avg_epu16(d, h),
                _ => _mm256_setzero_si256(),
            };
            store::<W>(to.add(y * out_stride), v);
            if j {
                rows.rotate_left(1);
            }
        }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn chroma<const W: usize>(mut dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    let (stride, out_stride, h) = (src.stride(), dst.stride(), dst.h());
    let (from, to) = (src.samples().as_ptr(), dst.samples_mut().as_mut_ptr());
    let (x, y) = (i16::from(xf), i16::from(yf));
    let a = _mm256_set1_epi16((8 - x) * (8 - y));
    let b = _mm256_set1_epi16(x * (8 - y));
    let c = _mm256_set1_epi16((8 - x) * y);
    let d = _mm256_set1_epi16(x * y);
    unsafe {
        for row in 0..h {
            let p = from.add(row * stride);
            let upper = _mm256_add_epi16(
                _mm256_mullo_epi16(load::<W>(p), a),
                _mm256_mullo_epi16(load::<W>(p.add(1)), b),
            );
            let lower = _mm256_add_epi16(
                _mm256_mullo_epi16(load::<W>(p.add(stride)), c),
                _mm256_mullo_epi16(load::<W>(p.add(stride + 1)), d),
            );
            let v = _mm256_srli_epi16::<6>(_mm256_add_epi16(
                _mm256_add_epi16(upper, lower),
                _mm256_set1_epi16(32),
            ));
            store::<W>(to.add(row * out_stride), v);
        }
    }
}
