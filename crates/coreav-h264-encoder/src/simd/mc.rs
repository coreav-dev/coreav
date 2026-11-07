use super::*;
use coreav_kernels_h264::{Block, BlockMut, Kernels, Scalar};

impl Avx2 {
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
            _ => {}
        }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn chroma<const W: usize>(dst: BlockMut<'_>, src: Block<'_>, xf: u8, yf: u8) {
    unsafe {
        match (xf != 0, yf != 0) {
            (false, false) => bilinear::<W, false, false>(dst, src, xf, yf),
            (true, false) => bilinear::<W, true, false>(dst, src, xf, yf),
            (false, true) => bilinear::<W, false, true>(dst, src, xf, yf),
            (true, true) => bilinear::<W, true, true>(dst, src, xf, yf),
        }
    }
}

#[target_feature(enable = "avx2")]
unsafe fn bilinear<const W: usize, const X: bool, const Y: bool>(
    mut dst: BlockMut<'_>,
    src: Block<'_>,
    xf: u8,
    yf: u8,
) {
    let (stride, out_stride, h) = (src.stride(), dst.stride(), dst.h());
    let (from, to) = (src.samples().as_ptr(), dst.samples_mut().as_mut_ptr());
    let (x, y) = (i16::from(xf), i16::from(yf));
    let a = _mm256_set1_epi16((if X { 8 - x } else { 1 }) * (if Y { 8 - y } else { 1 }));
    let b = _mm256_set1_epi16(x * (if Y { 8 - y } else { 1 }));
    let c = _mm256_set1_epi16((if X { 8 - x } else { 1 }) * y);
    let d = _mm256_set1_epi16(x * y);
    unsafe {
        for row in 0..h {
            let p = from.add(row * stride);
            let mut v = if X || Y {
                _mm256_mullo_epi16(load::<W>(p), a)
            } else {
                load::<W>(p)
            };
            if X {
                v = _mm256_add_epi16(v, _mm256_mullo_epi16(load::<W>(p.add(1)), b));
            }
            if Y {
                v = _mm256_add_epi16(v, _mm256_mullo_epi16(load::<W>(p.add(stride)), c));
            }
            if X && Y {
                v = _mm256_add_epi16(v, _mm256_mullo_epi16(load::<W>(p.add(stride + 1)), d));
                v = _mm256_srli_epi16::<6>(_mm256_add_epi16(v, _mm256_set1_epi16(32)));
            } else if X || Y {
                v = _mm256_srli_epi16::<3>(_mm256_add_epi16(v, _mm256_set1_epi16(4)));
            }
            store::<W>(to.add(row * out_stride), v);
        }
    }
}
