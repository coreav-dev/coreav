//! Bar C3 of `docs/design/h264.md`: every SIMD kernel gives the same
//! samples as the scalar reference, bit for bit, for every input the
//! `Kernels` contract allows.
//!
//! The tests run over every block shape, every fractional position, every
//! boundary strength and every weighting the specification allows, with
//! random samples and with the values at the ends of their ranges. On a
//! machine without the SIMD kernels they check the scalar kernels against
//! themselves, which costs little and keeps the suite green everywhere.

// Test code: the lints that keep production code panic-free are relaxed
// here, as clippy.toml does for unit tests.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use coreav_kernels_h264::{
    Auto, Block, BlockMut, Combine, EdgeMut, EdgeParams, Kernels, MAX_ORIGIN, MAX_SIZE, PlaneOp,
    Planes, PlanesMut, Reach, Scalar, Simd, Weighting,
};

/// A reproducible pseudo-random source.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // SplitMix64.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// A sample that favours the ends of the range, where the kernels
    /// clip.
    fn sample(&mut self) -> u8 {
        match self.below(8) {
            0 => 0,
            1 => 255,
            2 => 1,
            3 => 254,
            _ => (self.next() >> 24) as u8,
        }
    }

    fn samples(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.sample()).collect()
    }

    /// A scaled coefficient, in the range clauses 8.5.10 to 8.5.13 give
    /// one for 8-bit samples.
    fn coefficient(&mut self) -> i32 {
        match self.below(8) {
            0 => 0,
            1 => 32767,
            2 => -32768,
            _ => (self.next() % 65536) as i32 - 32768,
        }
    }
}

/// The SIMD kernels, or `None` where this CPU has none.
fn simd() -> Option<Simd> {
    Simd::detect()
}

#[test]
fn the_4x4_transform_matches_the_scalar_kernel() {
    let Some(simd) = simd() else { return };
    let mut rng = Rng(1);
    for case in 0..4000 {
        let stride = 4 + (case % 5);
        let offset = case % 3;
        let plane: Vec<u8> = rng.samples(offset + 4 * stride + 8);
        let mut coeffs = [0i32; 16];
        // Every third case is a block whose only coefficient is the DC.
        let dc_only = case % 3 == 0;
        for (i, c) in coeffs.iter_mut().enumerate() {
            if !dc_only || i == 0 {
                *c = rng.coefficient();
            }
        }
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.idct4x4_add(
            BlockMut::new(&mut want, offset, stride, 4, 4).expect("block"),
            &coeffs,
        );
        simd.idct4x4_add(
            BlockMut::new(&mut got, offset, stride, 4, 4).expect("block"),
            &coeffs,
        );
        assert_eq!(got, want, "case {case}, stride {stride}, offset {offset}");

        if dc_only {
            let mut want = plane.clone();
            let mut got = plane.clone();
            Scalar.idct4x4_dc_add(
                BlockMut::new(&mut want, offset, stride, 4, 4).expect("block"),
                coeffs[0],
            );
            simd.idct4x4_dc_add(
                BlockMut::new(&mut got, offset, stride, 4, 4).expect("block"),
                coeffs[0],
            );
            assert_eq!(got, want, "DC case {case}");
        }
    }
}

#[test]
fn the_8x8_transform_matches_the_scalar_kernel() {
    let Some(simd) = simd() else { return };
    let mut rng = Rng(2);
    for case in 0..2000 {
        let stride = 8 + (case % 5);
        let offset = case % 3;
        let plane: Vec<u8> = rng.samples(offset + 8 * stride + 16);
        let mut coeffs = [0i32; 64];
        let sparse = case % 4 == 0;
        for (i, c) in coeffs.iter_mut().enumerate() {
            if !sparse || i % 7 == 0 {
                *c = rng.coefficient();
            }
        }
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.idct8x8_add(
            BlockMut::new(&mut want, offset, stride, 8, 8).expect("block"),
            &coeffs,
        );
        simd.idct8x8_add(
            BlockMut::new(&mut got, offset, stride, 8, 8).expect("block"),
            &coeffs,
        );
        assert_eq!(got, want, "case {case}, stride {stride}, offset {offset}");
    }
}

#[test]
fn four_4x4_transforms_in_one_call_match_the_scalar_kernel() {
    let Some(simd) = simd() else { return };
    let mut rng = Rng(7);
    for case in 0..4000 {
        let stride = 8 + (case % 9);
        let offset = case % 5;
        let plane: Vec<u8> = rng.samples(offset + 8 * stride + 16);
        let mut coeffs = [[0i32; 16]; 4];
        let sparse = case % 3 == 0;
        for (i, c) in coeffs.as_flattened_mut().iter_mut().enumerate() {
            if !sparse || i % 5 == 0 {
                *c = rng.coefficient();
            }
        }
        // Every mask, so that a clear bit is seen to add nothing even where
        // its block holds coefficients.
        let nz = (case % 16) as u8;
        let mut want = plane.clone();
        let mut got = plane.clone();
        for (k, block) in coeffs.iter().enumerate() {
            if nz & (1 << k) != 0 {
                let at = offset + (k / 2) * 4 * stride + (k % 2) * 4;
                Scalar.idct4x4_add(
                    BlockMut::new(&mut want, at, stride, 4, 4).expect("block"),
                    block,
                );
            }
        }
        simd.idct4x4_add_quad(
            BlockMut::new(&mut got, offset, stride, 8, 8).expect("block"),
            &coeffs,
            nz,
        );
        assert_eq!(
            got, want,
            "case {case}, stride {stride}, offset {offset}, nz {nz:#x}"
        );
        // The reference's own form, through the trait's default.
        let mut by_blocks = plane.clone();
        Scalar.idct4x4_add_quad(
            BlockMut::new(&mut by_blocks, offset, stride, 8, 8).expect("block"),
            &coeffs,
            nz,
        );
        assert_eq!(by_blocks, want, "case {case}: the scalar default");
    }
}

#[test]
fn prediction_combining_matches_the_scalar_kernel() {
    let Some(simd) = simd() else { return };
    let mut rng = Rng(3);
    for case in 0..4000 {
        // Shapes the decoder uses, and shapes it does not: the trait
        // allows any width up to the source's.
        let (w, h) = match case % 10 {
            0 => (16, 16),
            1 => (16, 8),
            2 => (8, 8),
            3 => (8, 4),
            4 => (4, 8),
            5 => (4, 4),
            6 => (1, 1),
            7 => (2, 3),
            8 => (12, 5),
            _ => (15, 16),
        };
        let stride = w + (case % 3);
        let p0 = rng.samples(16 * 16);
        let p1 = rng.samples(16 * 16);
        let plane = rng.samples(stride * h + 8);
        let log_wd = (case % 8) as i32;
        let mode = match case % 5 {
            0 => Combine::Copy(case % 2),
            1 => Combine::Average,
            2 => Combine::Uni {
                list: case % 2,
                log_wd,
                w: (rng.below(256) as i32) - 128,
                o: (rng.below(256) as i32) - 128,
            },
            _ => Combine::Bi {
                log_wd,
                w0: (rng.below(256) as i32) - 128,
                w1: (rng.below(256) as i32) - 128,
                o: (rng.below(256) as i32) - 128,
            },
        };
        let first = Block::new(&p0, 0, 16, w, h).expect("block");
        let second = Block::new(&p1, 0, 16, w, h).expect("block");
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.combine(
            BlockMut::new(&mut want, 0, stride, w, h).expect("block"),
            first,
            second,
            mode,
        );
        simd.combine(
            BlockMut::new(&mut got, 0, stride, w, h).expect("block"),
            first,
            second,
            mode,
        );
        assert_eq!(got, want, "case {case}, {w}x{h}, {mode:?}");
    }
}

#[test]
fn luma_interpolation_matches_the_scalar_kernel() {
    let Some(simd) = simd() else { return };
    let mut rng = Rng(4);
    for case in 0..2000 {
        let (w, h) = match case % 8 {
            0 => (16, 16),
            1 => (16, 8),
            2 => (8, 16),
            3 => (8, 8),
            4 => (8, 4),
            5 => (4, 4),
            // Heights the decoder does not use, which the trait allows.
            6 => (16, 1 + case % 16),
            _ => (8, 1 + case % 11),
        };
        let (xf, yf) = ((case % 4) as u8, ((case / 4) % 4) as u8);
        let src_stride = w + 5 + (case % 4);
        let src = rng.samples(src_stride * (h + 5) + 8);
        let dst_stride = w + (case % 3);
        let plane = rng.samples(dst_stride * h + 8);
        let source = Block::new(&src, 0, src_stride, w + 5, h + 5).expect("block");
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.luma_mc(
            BlockMut::new(&mut want, 0, dst_stride, w, h).expect("block"),
            source,
            xf,
            yf,
        );
        simd.luma_mc(
            BlockMut::new(&mut got, 0, dst_stride, w, h).expect("block"),
            source,
            xf,
            yf,
        );
        assert_eq!(got, want, "case {case}, {w}x{h}, position ({xf}, {yf})");
    }
}

#[test]
fn chroma_interpolation_matches_the_scalar_kernel() {
    let Some(simd) = simd() else { return };
    let mut rng = Rng(5);
    for case in 0..2000 {
        let (w, h) = match case % 7 {
            0 => (8, 8),
            1 => (8, 4),
            2 => (4, 8),
            3 => (4, 2),
            4 => (2, 2),
            5 => (8, 1 + case % 9),
            _ => (6, 1 + case % 7),
        };
        let (xf, yf) = ((case % 8) as u8, ((case / 8) % 8) as u8);
        let src_stride = w + 1 + (case % 3);
        let src = rng.samples(src_stride * (h + 1) + 8);
        let dst_stride = w + (case % 2);
        let plane = rng.samples(dst_stride * h + 8);
        let source = Block::new(&src, 0, src_stride, w + 1, h + 1).expect("block");
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.chroma_mc(
            BlockMut::new(&mut want, 0, dst_stride, w, h).expect("block"),
            source,
            xf,
            yf,
        );
        simd.chroma_mc(
            BlockMut::new(&mut got, 0, dst_stride, w, h).expect("block"),
            source,
            xf,
            yf,
        );
        assert_eq!(got, want, "case {case}, {w}x{h}, position ({xf}, {yf})");
    }
}

/// `Auto` predicts a full-sample chroma block by copying it before it
/// chooses a kernel, so it has a path of its own to check: at every
/// position, with and without the SIMD kernels, it gives what the scalar
/// kernel gives. Rows and columns of the source differ, so that copying
/// at a position that needs the filter in either direction shows.
#[test]
fn auto_chroma_interpolation_matches_the_scalar_kernel_at_every_position() {
    let mut rng = Rng(11);
    for auto in [Auto::detect(), Auto::scalar()] {
        for case in 0..512 {
            let (w, h) = [
                (8, 8),
                (8, 4),
                (4, 8),
                (4, 4),
                (4, 2),
                (2, 4),
                (2, 2),
                (8, 2),
            ][case % 8];
            let (xf, yf) = (((case / 8) % 8) as u8, ((case / 64) % 8) as u8);
            let src_stride = w + 1;
            let src = rng.samples(src_stride * (h + 1));
            let plane = rng.samples(w * h);
            let source = Block::new(&src, 0, src_stride, w + 1, h + 1).expect("block");
            let mut want = plane.clone();
            let mut got = plane.clone();
            Scalar.chroma_mc(
                BlockMut::new(&mut want, 0, w, w, h).expect("block"),
                source,
                xf,
                yf,
            );
            auto.chroma_mc(
                BlockMut::new(&mut got, 0, w, w, h).expect("block"),
                source,
                xf,
                yf,
            );
            assert_eq!(got, want, "{w}x{h}, position ({xf}, {yf})");
        }
    }
}

#[test]
fn the_deblocking_filter_matches_the_scalar_kernel() {
    let Some(best) = simd() else { return };
    // Both x86-64 tiers where the CPU has AVX-512: the best one, and AVX2
    // alone as a CPU without it runs.
    for simd in [best, best.without_avx512()] {
        deblocking_filter_matches(simd);
    }
}

fn deblocking_filter_matches(simd: Simd) {
    let mut rng = Rng(6);
    for case in 0..4000 {
        let chroma = case % 3 == 0;
        // Sixteen lines for a luma edge and eight for a chroma one is
        // what the decoder asks for; the view allows any positive
        // multiple of four, and the kernels take eight at a time.
        let lines = match case % 5 {
            0 => 4,
            1 => 8,
            2 => 12,
            3 => 16,
            _ => 20,
        };
        let reach = if chroma { Reach::Chroma } else { Reach::Luma };
        let vertical = case % 2 == 0;
        let stride = if chroma { 24 } else { 32 };
        let plane = rng.samples(stride * (lines + 12) + 8);
        let mut params = EdgeParams {
            alpha: rng.below(256) as u8,
            beta: rng.below(19) as u8,
            tc0: [0; 8],
            bs: [0; 4],
        };
        for bs in &mut params.bs {
            *bs = rng.below(5) as u8;
        }
        for tc0 in &mut params.tc0 {
            *tc0 = rng.below(26) as u8;
        }
        // Samples that differ by less than beta on each side of the edge,
        // so that the filter runs rather than stopping at its threshold.
        let plane: Vec<u8> = if case % 2 == 0 {
            plane
        } else {
            (0..plane.len())
                .map(|i| {
                    let step = if (i % stride) < stride / 2 { 100 } else { 118 };
                    step + ((i * 7) % 5) as u8
                })
                .collect()
        };
        let (q0, across, along) = if vertical {
            (4 * stride + 8, 1, stride)
        } else {
            (8 * stride + 4, stride, 1)
        };
        if along == 1 && across < lines {
            continue;
        }
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.filter_edge(
            EdgeMut::new(&mut want, q0, across, along, lines, reach).expect("edge"),
            &params,
        );
        simd.filter_edge(
            EdgeMut::new(&mut got, q0, across, along, lines, reach).expect("edge"),
            &params,
        );
        assert_eq!(
            got,
            want,
            "case {case}, chroma {chroma}, vertical {vertical}, AVX-512 {}",
            simd.has_avx512()
        );
    }
}

/// What `luma_mc_avg` and `chroma_mc_avg` mean, written out: the
/// reference's prediction into a block of its own, then each sample of
/// `dst` averaged with it, `(dst + pred + 1) >> 1`.
fn averaged(
    plane: &mut [u8],
    at: usize,
    stride: usize,
    (w, h): (usize, usize),
    src: Block<'_>,
    (xf, yf): (u8, u8),
    luma: bool,
) {
    let mut pred = vec![0u8; w * h];
    let block = BlockMut::new(&mut pred, 0, w, w, h).expect("pred");
    if luma {
        Scalar.luma_mc(block, src, xf, yf);
    } else {
        Scalar.chroma_mc(block, src, xf, yf);
    }
    for y in 0..h {
        for x in 0..w {
            let o = &mut plane[at + y * stride + x];
            *o = ((u16::from(*o) + u16::from(pred[y * w + x]) + 1) >> 1) as u8;
        }
    }
}

/// Bar E1 for the averaging interpolation: `Auto`, `Simd` and the trait's
/// own default (through `Scalar`) against [`averaged`], for every luma and
/// chroma block size and fractional position, with strides from tight to
/// wide, in a plane with a margin round the block.
#[test]
fn averaging_interpolation_matches_the_reference() {
    let mut rng = Rng(0xa76);
    let auto = Auto::detect();
    let simd = Simd::detect();
    for luma in [true, false] {
        let (sizes, positions, extra): (&[usize], u8, usize) = if luma {
            (&[2, 4, 8, 12, 16], 4, 5)
        } else {
            (&[2, 4, 8, 12, 16], 8, 1)
        };
        for &w in sizes {
            for h in 1..=16 {
                for (xf, yf) in (0..positions).flat_map(|x| (0..positions).map(move |y| (x, y))) {
                    let (src_pad, dst_pad) = [(0, 0), (3, 7)][usize::from(xf + yf) % 2];
                    let src_stride = w + extra + src_pad;
                    let source = rng.samples(src_stride * (h + extra));
                    let src =
                        Block::new(&source, 0, src_stride, w + extra, h + extra).expect("src");
                    let stride = w + dst_pad;
                    let plane = rng.samples(stride * (h + 2) + 2);
                    let at = stride + usize::from(dst_pad > 0);
                    let mut want = plane.clone();
                    averaged(&mut want, at, stride, (w, h), src, (xf, yf), luma);
                    let mut kernels: Vec<(&str, &dyn Kernels)> =
                        vec![("auto", &auto), ("scalar", &Scalar)];
                    if let Some(simd) = simd.as_ref() {
                        kernels.push(("simd", simd));
                    }
                    for (name, k) in kernels {
                        let mut got = plane.clone();
                        let dst = BlockMut::new(&mut got, at, stride, w, h).expect("dst");
                        if luma {
                            k.luma_mc_avg(dst, src, xf, yf);
                        } else {
                            k.chroma_mc_avg(dst, src, xf, yf);
                        }
                        assert_eq!(got, want, "{name}: luma {luma}, {w}x{h} at ({xf}, {yf})");
                    }
                }
            }
        }
    }
}

/// A source of another shape than the window the trait asks for, and a
/// block larger than 16 by 16, leave `dst` unchanged, whatever the kernels.
#[test]
fn averaging_interpolation_declines_what_the_reference_declines() {
    let mut rng = Rng(0xa77);
    let auto = Auto::detect();
    let simd = Simd::detect();
    let source = rng.samples(64 * 64);
    for luma in [true, false] {
        let extra = if luma { 5 } else { 1 };
        // (block, source) shapes, each one the trait declines.
        // Blocks wider or taller than 16 are declined even with a source
        // of the right shape.
        let shapes = [
            ((8, 8), (8 + extra, 8 + extra - 1)),
            ((8, 8), (8 + extra + 1, 8 + extra)),
            ((4, 4), (4, 4)),
            ((24, 8), (24 + extra, 8 + extra)),
            ((8, 17), (8 + extra, 17 + extra)),
            ((4, 17), (4 + extra, 17 + extra)),
            ((16, 17), (16 + extra, 17 + extra)),
        ];
        for ((w, h), (sw, sh)) in shapes {
            let src = Block::new(&source, 0, 48, sw, sh).expect("src");
            let plane = rng.samples(32 * 32);
            let mut kernels: Vec<(&str, &dyn Kernels)> = vec![("auto", &auto), ("scalar", &Scalar)];
            if let Some(simd) = simd.as_ref() {
                kernels.push(("simd", simd));
            }
            for (name, k) in kernels {
                let mut got = plane.clone();
                let dst = BlockMut::new(&mut got, 0, 32, w, h).expect("dst");
                if luma {
                    k.luma_mc_avg(dst, src, 1, 2);
                } else {
                    k.chroma_mc_avg(dst, src, 3, 5);
                }
                assert_eq!(got, plane, "{name}: luma {luma}, {w}x{h} from {sw}x{sh}");
            }
        }
    }
}

/// What `luma_mc_weighted` and `chroma_mc_weighted` mean, written out from
/// Equations 8-286 to 8-288: the reference's prediction into a block of
/// its own, then each sample of `dst` its weighting, with `dst`'s sample
/// as list 0 where both lists are weighted.
fn weighted(
    plane: &mut [u8],
    (at, stride): (usize, usize),
    (w, h): (usize, usize),
    src: Block<'_>,
    (xf, yf): (u8, u8),
    luma: bool,
    weighting: Weighting,
) {
    let mut pred = vec![0u8; w * h];
    let block = BlockMut::new(&mut pred, 0, w, w, h).expect("pred");
    if luma {
        Scalar.luma_mc(block, src, xf, yf);
    } else {
        Scalar.chroma_mc(block, src, xf, yf);
    }
    for y in 0..h {
        for x in 0..w {
            let o = &mut plane[at + y * stride + x];
            let p = i32::from(pred[y * w + x]);
            let v = match weighting {
                Weighting::Uni { log_wd, w, o: off } if log_wd >= 1 => {
                    ((p * w + (1 << (log_wd - 1))) >> log_wd) + off
                }
                Weighting::Uni { w, o: off, .. } => p * w + off,
                Weighting::Bi {
                    log_wd,
                    w0,
                    w1,
                    o: off,
                } => ((i32::from(*o) * w0 + p * w1 + (1 << log_wd)) >> (log_wd + 1)) + off,
            };
            *o = v.clamp(0, 255) as u8;
        }
    }
}

/// A weighting in the ranges of `Weighting::in_range`, favouring their
/// ends, and the implicit weights of clause 8.4.2.3.1 a quarter of the
/// time.
fn weighting(rng: &mut Rng) -> Weighting {
    let weight = |rng: &mut Rng| match rng.below(6) {
        0 => -128,
        1 => 128,
        2 => 127,
        _ => rng.below(257) as i32 - 128,
    };
    let log_wd = rng.below(8) as i32;
    let o = rng.below(256) as i32 - 128;
    match rng.below(4) {
        0 => Weighting::Uni {
            log_wd,
            w: weight(rng),
            o,
        },
        1 => {
            let w1 = rng.below(193) as i32 - 64;
            Weighting::Bi {
                log_wd: 5,
                w0: 64 - w1,
                w1,
                o: 0,
            }
        }
        _ => Weighting::Bi {
            log_wd,
            w0: weight(rng),
            w1: weight(rng),
            o,
        },
    }
}

/// Bar E1 for the weighted interpolation: `Auto`, `Simd` and the trait's
/// own default (through `Scalar`) against [`weighted`], for every luma and
/// chroma block size and fractional position, as for averaging.
#[test]
fn weighted_interpolation_matches_the_reference() {
    let mut rng = Rng(0xa78);
    let auto = Auto::detect();
    let simd = Simd::detect();
    for luma in [true, false] {
        let (positions, extra): (u8, usize) = if luma { (4, 5) } else { (8, 1) };
        for w in [2, 4, 8, 12, 16] {
            for h in 1..=16 {
                for (xf, yf) in (0..positions).flat_map(|x| (0..positions).map(move |y| (x, y))) {
                    let (src_pad, dst_pad) = [(0, 0), (3, 7)][usize::from(xf + yf) % 2];
                    let src_stride = w + extra + src_pad;
                    let source = rng.samples(src_stride * (h + extra));
                    let src =
                        Block::new(&source, 0, src_stride, w + extra, h + extra).expect("src");
                    let stride = w + dst_pad;
                    let plane = rng.samples(stride * (h + 2) + 2);
                    let at = stride + usize::from(dst_pad > 0);
                    let weighting = weighting(&mut rng);
                    let mut want = plane.clone();
                    weighted(
                        &mut want,
                        (at, stride),
                        (w, h),
                        src,
                        (xf, yf),
                        luma,
                        weighting,
                    );
                    let mut kernels: Vec<(&str, &dyn Kernels)> =
                        vec![("auto", &auto), ("scalar", &Scalar)];
                    if let Some(simd) = simd.as_ref() {
                        kernels.push(("simd", simd));
                    }
                    for (name, k) in kernels {
                        let mut got = plane.clone();
                        let dst = BlockMut::new(&mut got, at, stride, w, h).expect("dst");
                        if luma {
                            k.luma_mc_weighted(dst, src, xf, yf, weighting);
                        } else {
                            k.chroma_mc_weighted(dst, src, xf, yf, weighting);
                        }
                        assert_eq!(
                            got, want,
                            "{name}: luma {luma}, {w}x{h} at ({xf}, {yf}), {weighting:?}"
                        );
                    }
                }
            }
        }
    }
}

/// The shapes the averaging forms decline, and a weighting outside
/// `Weighting::in_range`, leave `dst` unchanged, whatever the kernels.
#[test]
fn weighted_interpolation_declines_what_the_reference_declines() {
    let mut rng = Rng(0xa79);
    let auto = Auto::detect();
    let simd = Simd::detect();
    let source = rng.samples(64 * 64);
    let fine = Weighting::Bi {
        log_wd: 3,
        w0: 5,
        w1: -7,
        o: 9,
    };
    let outside = [
        Weighting::Uni {
            log_wd: 8,
            w: 1,
            o: 0,
        },
        Weighting::Uni {
            log_wd: -1,
            w: 1,
            o: 0,
        },
        Weighting::Uni {
            log_wd: 2,
            w: 129,
            o: 0,
        },
        Weighting::Uni {
            log_wd: 2,
            w: 1,
            o: 128,
        },
        Weighting::Bi {
            log_wd: 2,
            w0: -129,
            w1: 1,
            o: 0,
        },
        Weighting::Bi {
            log_wd: 2,
            w0: 1,
            w1: i32::MAX,
            o: 0,
        },
        Weighting::Bi {
            log_wd: 2,
            w0: 1,
            w1: 1,
            o: i32::MIN,
        },
    ];
    for luma in [true, false] {
        let extra = if luma { 5 } else { 1 };
        let mut cases = vec![
            ((8, 8), (8 + extra, 8 + extra - 1), fine),
            ((8, 8), (8 + extra + 1, 8 + extra), fine),
            ((4, 4), (4, 4), fine),
            ((24, 8), (24 + extra, 8 + extra), fine),
            ((8, 17), (8 + extra, 17 + extra), fine),
            ((16, 17), (16 + extra, 17 + extra), fine),
        ];
        for (k, weighting) in outside.into_iter().enumerate() {
            let (w, h) = [(16, 16), (8, 4), (4, 8), (2, 2)][k % 4];
            cases.push(((w, h), (w + extra, h + extra), weighting));
        }
        for ((w, h), (sw, sh), weighting) in cases {
            let src = Block::new(&source, 0, 48, sw, sh).expect("src");
            let plane = rng.samples(32 * 32);
            let mut kernels: Vec<(&str, &dyn Kernels)> = vec![("auto", &auto), ("scalar", &Scalar)];
            if let Some(simd) = simd.as_ref() {
                kernels.push(("simd", simd));
            }
            for (name, k) in kernels {
                let mut got = plane.clone();
                let dst = BlockMut::new(&mut got, 0, 32, w, h).expect("dst");
                if luma {
                    k.luma_mc_weighted(dst, src, 1, 2, weighting);
                } else {
                    k.chroma_mc_weighted(dst, src, 3, 5, weighting);
                }
                assert_eq!(
                    got, plane,
                    "{name}: luma {luma}, {w}x{h} from {sw}x{sh}, {weighting:?}"
                );
            }
        }
    }
}

/// A reference picture for [`predicted`]: three slices, each plane's
/// sample (0, 0) at its origin (luma, chroma), and its size.
#[derive(Clone, Copy)]
struct Reference<'a> {
    planes: [&'a [u8]; 3],
    origin: [usize; 2],
    stride: usize,
    rows: usize,
}

impl Reference<'_> {
    /// The `w` by `h` window at (`x`, `y`) of plane `c` as the
    /// specification reads it: each position clamped into the picture
    /// (Equations 8-228 and 8-229 and their luma forms, 8-233 to 8-236).
    fn clamped(&self, c: usize, x: isize, y: isize, w: usize, h: usize) -> Vec<u8> {
        let (pw, ph, origin) = if c == 0 {
            (self.stride, self.rows, self.origin[0])
        } else {
            (self.stride / 2, self.rows / 2, self.origin[1])
        };
        let mut out = Vec::with_capacity(w * h);
        for j in 0..h as isize {
            for i in 0..w as isize {
                let cx = (x + i).clamp(0, pw as isize - 1) as usize;
                let cy = (y + j).clamp(0, ph as isize - 1) as usize;
                out.push(self.planes[c][origin + cy * pw + cx]);
            }
        }
        out
    }
}

/// What `Kernels::predict` means, written out plane by plane through the
/// scalar kernels on windows built as the specification builds them:
/// `None` where it declines the block (a sample the prediction uses
/// outside the reference, a window that does not lie in its slice, or a
/// block outside the picture's planes), and otherwise the three planes
/// written. A window whose fraction leaves its apron unused may reach past
/// the picture, so the prediction it gives is the specification's only
/// if the kernels honour their contract on the samples used.
#[allow(clippy::too_many_arguments, clippy::int_plus_one)]
fn predicted(
    planes: [&mut Vec<u8>; 3],
    [first, chroma_first]: [usize; 2],
    [origin, chroma_origin]: [usize; 2],
    reference: Reference<'_>,
    (px, py): (usize, usize),
    (w, h): (usize, usize),
    mv: [i16; 2],
    ops: [PlaneOp; 3],
) -> Option<()> {
    let (stride, rows) = (reference.stride, reference.rows);
    let [mx, my] = mv.map(isize::from);
    let (xf, yf) = ((mx & 3) as u8, (my & 3) as u8);
    let (cxf, cyf) = ((mx & 7) as u8, (my & 7) as u8);
    let (xi, yi) = (px as isize + (mx >> 2), py as isize + (my >> 2));
    let cx = (px / 2) as isize + (mx >> 3);
    let cy = (py / 2) as isize + (my >> 3);
    let (cw, ch, cs) = (w / 2, h / 2, stride / 2);
    let (ax, ay) = (2 * isize::from(xf != 0), 2 * isize::from(yf != 0));
    let (bx, by) = (3 * isize::from(xf != 0), 3 * isize::from(yf != 0));
    let used = xi - ax >= 0
        && yi - ay >= 0
        && xi + (w as isize) + bx <= stride as isize
        && yi + (h as isize) + by <= rows as isize
        && cx >= 0
        && cy >= 0
        && cx + (cw as isize) + isize::from(cxf != 0) <= cs as isize
        && cy + (ch as isize) + isize::from(cyf != 0) <= (rows / 2) as isize;
    let luma_src = reference.origin[0] as isize + (yi - 2) * stride as isize + xi - 2;
    let chroma_src = reference.origin[1] as isize + cy * cs as isize + cx;
    let held = luma_src >= 0
        && luma_src as usize + (h + 4) * stride + w + 5 <= reference.planes[0].len()
        && chroma_src >= 0
        && chroma_src as usize + ch * cs + cw + 1
            <= reference.planes[1].len().min(reference.planes[2].len());
    if !used || !held || py < first || py / 2 < chroma_first {
        return None;
    }
    let [y_plane, cb_plane, cr_plane] = planes;
    let luma_at = origin + (py - first) * stride + px;
    let chroma_at = chroma_origin + (py / 2 - chroma_first) * cs + px / 2;
    if luma_at + (h - 1) * stride + w > y_plane.len()
        || chroma_at + (ch - 1) * cs + cw > cb_plane.len().min(cr_plane.len())
    {
        return None;
    }
    let window = reference.clamped(0, xi - 2, yi - 2, w + 5, h + 5);
    let src = Block::new(&window, 0, w + 5, w + 5, h + 5).expect("luma window");
    let dst = BlockMut::new(y_plane, luma_at, stride, w, h).expect("luma block");
    match ops[0] {
        PlaneOp::Put => Scalar.luma_mc(dst, src, xf, yf),
        PlaneOp::Avg => Scalar.luma_mc_avg(dst, src, xf, yf),
        PlaneOp::Weigh(weighting) => Scalar.luma_mc_weighted(dst, src, xf, yf, weighting),
    }
    for (c, plane) in [cb_plane, cr_plane].into_iter().enumerate() {
        let window = reference.clamped(1 + c, cx, cy, cw + 1, ch + 1);
        let src = Block::new(&window, 0, cw + 1, cw + 1, ch + 1).expect("chroma window");
        let dst = BlockMut::new(plane, chroma_at, cs, cw, ch).expect("chroma block");
        match ops[1 + c] {
            PlaneOp::Put => Scalar.chroma_mc(dst, src, cxf, cyf),
            PlaneOp::Avg => Scalar.chroma_mc_avg(dst, src, cxf, cyf),
            PlaneOp::Weigh(weighting) => Scalar.chroma_mc_weighted(dst, src, cxf, cyf, weighting),
        }
    }
    Some(())
}

/// Extreme inputs to the three-plane prediction: vectors at the ends of
/// `i16`, positions far past the planes, and a picture whose stride is not
/// the reference's. Every kernel declines each, writing nothing.
#[test]
fn three_plane_prediction_declines_extreme_inputs() {
    let mut rng = Rng(0x7e57);
    let (stride, rows) = (48, 40);
    let reference: Vec<Vec<u8>> = [stride * rows, stride * rows / 4, stride * rows / 4]
        .into_iter()
        .map(|n| rng.samples(n))
        .collect();
    let src =
        Planes::new(&reference[0], &reference[1], &reference[2], stride, rows).expect("planes");
    let auto = Auto::detect();
    let mut kernels: Vec<(&str, Box<dyn Kernels>)> = vec![
        ("auto", Box::new(auto)),
        ("auto-avx2", Box::new(auto.without_avx512())),
        ("scalar", Box::new(Scalar)),
    ];
    if let Some(simd) = simd() {
        kernels.push(("simd", Box::new(simd)));
        kernels.push(("simd-avx2", Box::new(simd.without_avx512())));
    }
    let picture: Vec<Vec<u8>> = [stride * rows, stride * rows / 4, stride * rows / 4]
        .into_iter()
        .map(|n| rng.samples(n))
        .collect();
    let (lo, hi) = (i16::MIN, i16::MAX);
    let cases: [(usize, usize, usize, [i16; 2]); 8] = [
        (16, 16, stride, [lo, 0]),
        (16, 16, stride, [0, lo]),
        (16, 16, stride, [hi, 0]),
        (16, 16, stride, [0, hi]),
        (16, 16, stride, [lo, hi]),
        (1 << 15, 16, stride, [0, 0]),
        (16, 1 << 15, stride, [0, 0]),
        (16, 16, stride + 2, [0, 0]),
    ];
    for (px, py, dst_stride, mv) in cases {
        for ops in [[PlaneOp::Put; 3], [PlaneOp::Avg; 3]] {
            for (name, k) in &kernels {
                let mut got = picture.clone();
                let [gy, gcb, gcr] = &mut got[..] else {
                    unreachable!()
                };
                let dst = PlanesMut::new(gy, gcb, gcr, dst_stride, [0, 0]);
                let done = k.predict(dst, &src, px, py, 8, 8, mv, ops);
                let what = format!("{name}: ({px}, {py}), stride {dst_stride}, {mv:?}, {ops:?}");
                assert!(!done, "{what}");
                assert_eq!(got, picture, "{what}");
            }
        }
    }
}

/// The kernels `predict` is checked for: the dispatch with and without the
/// AVX-512 tier, the trait's own default through `Scalar`, and the vector
/// kernels where the CPU has them.
fn predict_kernels() -> Vec<(&'static str, Box<dyn Kernels>)> {
    let auto = Auto::detect();
    let mut kernels: Vec<(&str, Box<dyn Kernels>)> = vec![
        ("auto", Box::new(auto)),
        ("auto-avx2", Box::new(auto.without_avx512())),
        ("scalar", Box::new(Scalar)),
    ];
    if let Some(simd) = simd() {
        kernels.push(("simd", Box::new(simd)));
        kernels.push(("simd-avx2", Box::new(simd.without_avx512())));
    }
    kernels
}

/// A reference `stride` by `rows` of random samples, each plane with
/// `slack` rows of random samples before its origin and after its last
/// row (luma, chroma): the slices and the origins.
fn slack_reference(
    rng: &mut Rng,
    stride: usize,
    rows: usize,
    [slack, chroma_slack]: [usize; 2],
) -> ([Vec<u8>; 3], [usize; 2]) {
    let cs = stride / 2;
    let luma = stride * (rows + 2 * slack);
    let chroma = cs * (rows / 2 + 2 * chroma_slack);
    (
        [rng.samples(luma), rng.samples(chroma), rng.samples(chroma)],
        [slack * stride, chroma_slack * cs],
    )
}

/// Bar E1 for `predict`: `Auto`, `Simd` with and without the AVX-512
/// tier, and the trait's own default (through `Scalar`) against
/// [`predicted`], for every block size, every luma and chroma fraction,
/// each operation in each plane, vectors that reach every edge of the
/// reference, and blocks at every edge of the picture's planes: with
/// planes that start at their slices' first sample, and with planes that
/// keep slack rows before and after them, whose windows at the picture's
/// edges are taken where only unused samples lie outside the picture.
#[test]
fn three_plane_prediction_matches_the_reference() {
    let mut rng = Rng(0x3b1a);
    let (stride, rows) = (48, 40);
    let kernels = predict_kernels();
    let op = |rng: &mut Rng| match rng.below(3) {
        0 => PlaneOp::Put,
        1 => PlaneOp::Avg,
        _ => PlaneOp::Weigh(weighting(rng)),
    };
    // No slack, the decoder's (4 luma rows, 2 chroma rows), and too little
    // for every window that reaches past the picture.
    for slack in [[0, 0], [4, 2], [1, 1]] {
        let (reference, origin) = slack_reference(&mut rng, stride, rows, slack);
        let src = Planes::with_origin(
            &reference[0],
            &reference[1],
            &reference[2],
            stride,
            rows,
            origin,
        )
        .expect("planes");
        let model = Reference {
            planes: [&reference[0], &reference[1], &reference[2]],
            origin,
            stride,
            rows,
        };
        let (mut declined, mut predicted_blocks, mut past_the_edge) = (0, 0, 0);
        for (w, h) in [4, 8, 16]
            .into_iter()
            .flat_map(|w| [4, 8, 16].map(|h| (w, h)))
        {
            for case in 0..1200 {
                // The band of the picture the block is written into: luma
                // rows `first` on, `band` rows of them, and chroma rows from
                // `chroma_first`, which is `first / 2` or, as an apron of
                // other sizes would make it, not; with the picture's own
                // slack before them half the time.
                let first = rng.below(8) as usize;
                let band = rows - first - 2 * rng.below(2) as usize;
                let chroma_first = match rng.below(3) {
                    0 => first / 2 + 1,
                    1 => (first / 2).saturating_sub(1),
                    _ => first / 2,
                };
                let chroma_band = (rows / 2 - chroma_first).min(band / 2 + 1);
                let cs = stride / 2;
                let dst_origin = if rng.below(2) == 0 {
                    [0, 0]
                } else {
                    [stride * rng.below(5) as usize, cs * rng.below(3) as usize]
                };
                let px = 4 * rng.below(((stride - w) / 4 + 1) as u64) as usize;
                let py = first + 4 * rng.below(((band.saturating_sub(h)) / 4 + 2) as u64) as usize;
                // Whole samples across the reference and a little past it,
                // and every fraction.
                let whole = |rng: &mut Rng, n: usize| rng.below(n as u64 + 12) as i16 - 6;
                let mut mv = [
                    whole(&mut rng, stride) * 4 - px as i16 * 4 + rng.below(8) as i16,
                    whole(&mut rng, rows) * 4 - py as i16 * 4 + rng.below(8) as i16,
                ];
                if case % 4 == 0 {
                    mv = [rng.below(64) as i16 - 32, rng.below(64) as i16 - 32];
                }
                let mut ops = [op(&mut rng), op(&mut rng), op(&mut rng)];
                // A third of the blocks give Cr Cb's operation, so that each
                // paired form of the x86 chroma pass (both written, averaged
                // or weighted alike) is reached on purpose.
                if case % 3 == 1 {
                    ops[2] = ops[1];
                }
                let picture: Vec<Vec<u8>> = [
                    dst_origin[0] + stride * band,
                    dst_origin[1] + cs * chroma_band,
                    dst_origin[1] + cs * chroma_band,
                ]
                .into_iter()
                .map(|n| rng.samples(n))
                .collect();
                let mut want = picture.clone();
                let [wy, wcb, wcr] = &mut want[..] else {
                    unreachable!()
                };
                let expected = predicted(
                    [wy, wcb, wcr],
                    [first, chroma_first],
                    dst_origin,
                    model,
                    (px, py),
                    (w, h),
                    mv,
                    ops,
                );
                if expected.is_some() {
                    predicted_blocks += 1;
                    // A window the test of the whole window declined.
                    let x = px as isize + isize::from(mv[0] >> 2) - 2;
                    let y = py as isize + isize::from(mv[1] >> 2) - 2;
                    let cx = (px / 2) as isize + isize::from(mv[0] >> 3);
                    let cy = (py / 2) as isize + isize::from(mv[1] >> 3);
                    let inside = x >= 0
                        && y >= 0
                        && x as usize + w + 5 <= stride
                        && y as usize + h + 5 <= rows
                        && cx >= 0
                        && cy >= 0
                        && cx as usize + w / 2 < cs
                        && cy as usize + h / 2 < rows / 2;
                    past_the_edge += usize::from(!inside);
                } else {
                    declined += 1;
                }
                for (name, k) in &kernels {
                    let mut got = picture.clone();
                    let [gy, gcb, gcr] = &mut got[..] else {
                        unreachable!()
                    };
                    let first = [first, chroma_first];
                    let dst = PlanesMut::with_origin(gy, gcb, gcr, stride, first, dst_origin);
                    let done = k.predict(dst, &src, px, py, w, h, mv, ops);
                    let what = format!(
                        "{name}: slack {slack:?}, {w}x{h} at ({px}, {py}), first {first:?}, \
                         origin {dst_origin:?}, {mv:?}, {ops:?}"
                    );
                    assert_eq!(done, expected.is_some(), "{what}");
                    assert_eq!(got, want, "{what}");
                }
            }
        }
        assert!(
            declined > 100 && predicted_blocks > 1000,
            "{slack:?}: {declined} {predicted_blocks}"
        );
        // With slack, windows past the picture's edge are taken.
        assert!(
            slack == [0, 0] || past_the_edge > 100,
            "{slack:?}: {past_the_edge}"
        );
    }
}

/// `predict` from the rows one band of a reference holds (`Planes::band`)
/// against `predict` from the whole reference: every kernel takes exactly
/// the blocks the whole reference's planes take whose luma and chroma
/// windows lie in the band's slices, predicts them alike, and
/// writes nothing for the others. The bands' first luma and chroma rows
/// are cut apart as a band's top aprons are (23 luma rows and 9 chroma
/// rows above its own), and otherwise.
#[test]
fn three_plane_prediction_from_a_band_matches_the_whole_reference() {
    let mut rng = Rng(0xba4d);
    let (stride, rows) = (48, 64);
    let cs = stride / 2;
    let kernels = predict_kernels();
    let reference: Vec<Vec<u8>> = [stride * rows, cs * rows / 2, cs * rows / 2]
        .into_iter()
        .map(|n| rng.samples(n))
        .collect();
    let whole =
        Planes::new(&reference[0], &reference[1], &reference[2], stride, rows).expect("planes");
    let (mut taken, mut declined) = (0, 0);
    for case in 0..10_000 {
        // Luma rows `first..end` and chroma rows `chroma_first..chroma_end`.
        let first = rng.below(rows as u64 - 8) as usize;
        let end = (first + 8 + rng.below(40) as usize).min(rows);
        let chroma_first = match case % 3 {
            0 => (first + 5) / 2,
            1 => first / 2,
            _ => (first / 2 + rng.below(7) as usize).saturating_sub(3),
        };
        let chroma_end = (chroma_first + 6 + rng.below(20) as usize).min(rows / 2);
        let y = &reference[0][first * stride..end * stride];
        let cb = &reference[1][chroma_first * cs..chroma_end * cs];
        let cr = &reference[2][chroma_first * cs..chroma_end * cs];
        let band =
            Planes::band(y, cb, cr, stride, rows, [first, chroma_first], [0, 0]).expect("band");
        let w = [4, 8, 16][rng.below(3) as usize];
        let h = [4, 8, 16][rng.below(3) as usize];
        let px = 4 * rng.below(((stride - w) / 4 + 1) as u64) as usize;
        let py = 4 * rng.below(((rows - h) / 4 + 1) as u64) as usize;
        // Rows across the band and a little past it, and every fraction.
        let target = first as i64 + rng.below((end - first) as u64 + 8) as i64 - 4 - h as i64 / 2;
        let mv = [
            (rng.below(stride as u64 + 12) as i64 - 6 - px as i64) * 4 + rng.below(8) as i64,
            (target - py as i64) * 4 + rng.below(8) as i64,
        ]
        .map(|v| v as i16);
        let ops = match rng.below(3) {
            0 => [PlaneOp::Put; 3],
            1 => [PlaneOp::Avg; 3],
            _ => [
                PlaneOp::Weigh(weighting(&mut rng)),
                PlaneOp::Avg,
                PlaneOp::Weigh(weighting(&mut rng)),
            ],
        };
        // The luma window's first and last samples, two above and two
        // left of the block to three below and three right, and the chroma
        // windows', one below and one right, inside the band's slices (a
        // window at the right edge runs into the next row).
        let (s, c) = (stride as i64, cs as i64);
        let yi = py as i64 + i64::from(mv[1] >> 2);
        let xi = px as i64 + i64::from(mv[0] >> 2);
        let luma_at = (yi - 2 - first as i64) * s + xi - 2;
        let luma_last = luma_at + (h as i64 + 4) * s + w as i64 + 4;
        let cy = (py / 2) as i64 + i64::from(mv[1] >> 3);
        let cx = (px / 2) as i64 + i64::from(mv[0] >> 3);
        let chroma_at = (cy - chroma_first as i64) * c + cx;
        let chroma_last = chroma_at + (h / 2) as i64 * c + (w / 2) as i64;
        let held = luma_at >= 0
            && luma_last < y.len() as i64
            && chroma_at >= 0
            && chroma_last < cb.len() as i64;
        let picture: Vec<Vec<u8>> = [stride * rows, cs * rows / 2, cs * rows / 2]
            .into_iter()
            .map(|n| rng.samples(n))
            .collect();
        for (name, k) in &kernels {
            let mut want = picture.clone();
            let [wy, wcb, wcr] = &mut want[..] else {
                unreachable!()
            };
            let dst = PlanesMut::new(wy, wcb, wcr, stride, [0, 0]);
            let from_whole = k.predict(dst, &whole, px, py, w, h, mv, ops);
            let mut got = picture.clone();
            let [gy, gcb, gcr] = &mut got[..] else {
                unreachable!()
            };
            let dst = PlanesMut::new(gy, gcb, gcr, stride, [0, 0]);
            let from_band = k.predict(dst, &band, px, py, w, h, mv, ops);
            let what = format!(
                "{name}: rows {first}..{end}, chroma {chroma_first}..{chroma_end}, \
                 {w}x{h} at ({px}, {py}), {mv:?}, {ops:?}"
            );
            assert_eq!(from_band, from_whole && held, "{what}");
            if from_band {
                assert_eq!(got, want, "{what}");
            } else {
                assert_eq!(got, picture, "{what}");
            }
        }
        if held {
            taken += 1;
        } else {
            declined += 1;
        }
    }
    // Both sides of the band's edges are reached.
    assert!(taken > 1500 && declined > 1500, "{taken} {declined}");
}

/// A band from row 0 whose planes keep slack before their origins (a
/// picture of one band) predicts as the same slices given to
/// `Planes::with_origin`: the same blocks, alike.
#[test]
fn a_band_from_row_0_is_the_reference_with_its_origin() {
    let mut rng = Rng(0x0b4d);
    let (stride, rows) = (48, 32);
    let kernels = predict_kernels();
    let (reference, origin) = slack_reference(&mut rng, stride, rows, [4, 2]);
    let [y, cb, cr] = [&reference[0], &reference[1], &reference[2]];
    let whole = Planes::with_origin(y, cb, cr, stride, rows, origin).expect("planes");
    let band = Planes::band(y, cb, cr, stride, rows, [0, 0], origin).expect("band");
    let cs = stride / 2;
    for _ in 0..3000 {
        let w = [4, 8, 16][rng.below(3) as usize];
        let h = [4, 8, 16][rng.below(3) as usize];
        let px = 4 * rng.below(((stride - w) / 4 + 1) as u64) as usize;
        let py = 4 * rng.below(((rows - h) / 4 + 1) as u64) as usize;
        let mv = [rng.below(96) as i16 - 48, rng.below(96) as i16 - 48];
        let ops = [[PlaneOp::Put; 3], [PlaneOp::Avg; 3]][rng.below(2) as usize];
        let picture: Vec<Vec<u8>> = [stride * rows, cs * rows / 2, cs * rows / 2]
            .into_iter()
            .map(|n| rng.samples(n))
            .collect();
        for (name, k) in &kernels {
            let mut outs = Vec::new();
            for src in [&whole, &band] {
                let mut got = picture.clone();
                let [gy, gcb, gcr] = &mut got[..] else {
                    unreachable!()
                };
                let dst = PlanesMut::new(gy, gcb, gcr, stride, [0, 0]);
                let done = k.predict(dst, src, px, py, w, h, mv, ops);
                outs.push((done, got));
            }
            assert_eq!(outs[0], outs[1], "{name}: {w}x{h} at ({px}, {py}), {mv:?}");
        }
    }
}

/// `Planes::band` declines what `place` could not keep within its bounds,
/// and an origin past row 0, before which a window above the first row
/// held would be read.
#[test]
fn a_band_of_a_reference_checks_its_rows() {
    let (stride, rows) = (32, 32);
    let luma = vec![7u8; stride * 8];
    let chroma = vec![9u8; stride * 2];
    let band = |first: [usize; 2], origin: [usize; 2]| {
        Planes::band(&luma, &chroma, &chroma, stride, rows, first, origin).is_some()
    };
    assert!(band([8, 4], [0, 0]));
    assert!(band([0, 0], [stride, stride / 2]));
    assert!(band([rows, rows / 2], [0, 0]));
    assert!(!band([rows + 1, 0], [0, 0]));
    assert!(!band([0, rows / 2 + 1], [0, 0]));
    assert!(!band([8, 4], [stride, 0]));
    assert!(!band([8, 4], [0, stride / 2]));
    assert!(!band([0, 0], [luma.len() + 1, 0]));
    assert!(!band([0, 0], [0, chroma.len() + 1]));
    assert!(Planes::band(&luma, &chroma, &chroma, stride + 1, rows, [0, 0], [0, 0]).is_none());
    let far = [usize::MAX / 2, 0];
    assert!(Planes::band(&luma, &chroma, &chroma, stride, rows, far, [0, 0]).is_none());
    assert!(Planes::band(&luma, &chroma, &chroma, stride, 0, [0, 0], [0, 0]).is_none());
    assert!(Planes::band(&luma, &chroma, &chroma, MAX_SIZE + 2, rows, [0, 0], [0, 0]).is_none());
}

/// `predict` declines a weighting outside `Weighting::in_range` and the
/// sizes no H.264 partition has, writing nothing.
#[test]
fn three_plane_prediction_declines_what_it_does_not_take() {
    let (stride, rows) = (32, 32);
    let reference = vec![7u8; stride * rows];
    let chroma = vec![9u8; stride * rows / 4];
    let src = Planes::new(&reference, &chroma, &chroma, stride, rows).expect("planes");
    assert!(Planes::new(&reference, &chroma, &chroma[1..], stride, rows).is_none());
    assert!(Planes::new(&reference, &chroma, &chroma, stride, rows - 1).is_none());
    // An origin the slice does not hold with the rows after it, or one past
    // `MAX_ORIGIN`.
    assert!(Planes::with_origin(&reference, &chroma, &chroma, stride, rows, [1, 0]).is_none());
    assert!(Planes::with_origin(&reference, &chroma, &chroma, stride, rows, [0, 1]).is_none());
    let long = vec![0u8; MAX_ORIGIN + 1 + stride * rows];
    assert!(Planes::with_origin(&long, &chroma, &chroma, stride, rows, [MAX_ORIGIN, 0]).is_some());
    assert!(
        Planes::with_origin(&long, &chroma, &chroma, stride, rows, [MAX_ORIGIN + 1, 0]).is_none()
    );
    let outside = PlaneOp::Weigh(Weighting::Uni {
        log_wd: 8,
        w: 1,
        o: 0,
    });
    let cases = [
        ((8, 8), [PlaneOp::Put, PlaneOp::Put, outside]),
        ((8, 2), [PlaneOp::Put; 3]),
        ((12, 8), [PlaneOp::Put; 3]),
        ((32, 16), [PlaneOp::Avg; 3]),
    ];
    let auto = Auto::detect();
    let kernels: [&dyn Kernels; 3] = [&auto, &Scalar, &auto.without_avx512()];
    for ((w, h), ops) in cases {
        for k in kernels {
            let (mut y, mut cb, mut cr) =
                (vec![1u8; stride * rows], vec![2u8; 256], vec![3u8; 256]);
            let dst = PlanesMut::new(&mut y, &mut cb, &mut cr, stride, [0, 0]);
            assert!(
                !k.predict(dst, &src, 8, 8, w, h, [0, 0], ops),
                "{w}x{h} {ops:?}"
            );
            assert!(y.iter().all(|&v| v == 1) && cb.iter().all(|&v| v == 2));
            assert!(cr.iter().all(|&v| v == 3));
        }
    }
    // A plane narrower than a window row (one macroblock wide): every form
    // declines the 16-wide blocks alike, at full-sample and fractional
    // vectors, and takes the 8-wide ones alike.
    let (narrow, rows) = (16, 32);
    let reference = vec![7u8; narrow * rows];
    let chroma = vec![9u8; narrow * rows / 4];
    let src = Planes::new(&reference, &chroma, &chroma, narrow, rows).expect("planes");
    for ((w, h), mv, takes) in [
        ((16, 4), [0, 0], false),
        ((16, 8), [0, 1], false),
        ((16, 16), [1, 1], false),
        ((8, 4), [0, 0], true),
    ] {
        for k in kernels {
            let (mut y, mut cb, mut cr) = (
                vec![1u8; narrow * rows],
                vec![2u8; narrow * rows / 4],
                vec![3u8; narrow * rows / 4],
            );
            let dst = PlanesMut::new(&mut y, &mut cb, &mut cr, narrow, [0, 0]);
            assert_eq!(
                k.predict(dst, &src, 0, 4, w, h, mv, [PlaneOp::Put; 3]),
                takes,
                "{w}x{h} at {mv:?} in a plane {narrow} wide"
            );
        }
    }
}

/// Whether the luma prediction at fraction (`xf`, `yf`) uses sample
/// (`i`, `j`) of its `w + 5` by `h + 5` window, as `Kernels::luma_mc`'s
/// contract has it: the columns beyond the block's only where `xf` is not
/// 0, the rows beyond only where `yf` is not 0.
fn luma_uses(w: usize, h: usize, xf: u8, yf: u8, i: usize, j: usize) -> bool {
    (xf != 0 || (2..2 + w).contains(&i)) && (yf != 0 || (2..2 + h).contains(&j))
}

/// The same for the chroma prediction of a `w` by `h` block from its
/// `w + 1` by `h + 1` window: the last column only where `xf` is not 0,
/// the last row only where `yf` is not 0 (Equation 8-266).
fn chroma_uses(w: usize, h: usize, xf: u8, yf: u8, i: usize, j: usize) -> bool {
    (xf != 0 || i < w) && (yf != 0 || j < h)
}

/// [`luma_uses`] or [`chroma_uses`].
type Uses = fn(usize, usize, u8, u8, usize, usize) -> bool;

/// The kernels of the contract tests: `Scalar` first, then the dispatch
/// and the vector kernels where the CPU has them.
fn contract_kernels() -> Vec<(&'static str, Box<dyn Kernels>)> {
    let mut kernels = predict_kernels();
    kernels.sort_by_key(|(name, _)| *name != "scalar");
    kernels
}

/// One single-plane form of the interpolation, run on a window.
#[derive(Clone, Copy, Debug)]
enum Form {
    Put,
    Avg,
    Weigh(Weighting),
}

/// The kernels' contract on the samples used (`Kernels::luma_mc`,
/// `Kernels::chroma_mc` and their averaging and weighting forms): for
/// every block size and fraction, a window whose unused samples are made
/// random again gives the same prediction, in `Scalar` and in every other
/// implementation, which also agree with each other.
#[test]
fn interpolation_ignores_the_samples_its_fraction_does_not_use() {
    let mut rng = Rng(0x0dd5);
    let kernels = contract_kernels();
    for luma in [true, false] {
        let sizes: &[usize] = if luma { &[4, 8, 16] } else { &[2, 4, 8] };
        let fractions = if luma { 4u8 } else { 8u8 };
        let (apron, uses): (usize, Uses) = if luma {
            (5, luma_uses)
        } else {
            (1, chroma_uses)
        };
        for &w in sizes {
            for &h in sizes {
                for xf in 0..fractions {
                    for yf in 0..fractions {
                        for case in 0..6 {
                            let form = match case % 3 {
                                0 => Form::Put,
                                1 => Form::Avg,
                                _ => Form::Weigh(weighting(&mut rng)),
                            };
                            let src_stride = w + apron + rng.below(4) as usize;
                            let window = rng.samples(src_stride * (h + apron));
                            let mut other = window.clone();
                            let mut unused = 0;
                            for j in 0..h + apron {
                                for i in 0..w + apron {
                                    if !uses(w, h, xf, yf, i, j) {
                                        other[j * src_stride + i] = rng.sample();
                                        unused += 1;
                                    }
                                }
                            }
                            // Beyond each row's window, nothing is read.
                            for j in 0..h + apron {
                                for i in w + apron..src_stride {
                                    other[j * src_stride + i] = rng.sample();
                                }
                            }
                            let zero = (xf == 0) as usize + (yf == 0) as usize;
                            assert!(zero == 0 || unused > 0);
                            let block = rng.samples(16 * h);
                            let run = |k: &dyn Kernels, samples: &[u8]| {
                                let mut out = block.clone();
                                let dst = BlockMut::new(&mut out, 0, 16, w, h).expect("block");
                                let src = Block::new(samples, 0, src_stride, w + apron, h + apron)
                                    .expect("window");
                                match (luma, form) {
                                    (true, Form::Put) => k.luma_mc(dst, src, xf, yf),
                                    (true, Form::Avg) => k.luma_mc_avg(dst, src, xf, yf),
                                    (true, Form::Weigh(wt)) => {
                                        k.luma_mc_weighted(dst, src, xf, yf, wt)
                                    }
                                    (false, Form::Put) => k.chroma_mc(dst, src, xf, yf),
                                    (false, Form::Avg) => k.chroma_mc_avg(dst, src, xf, yf),
                                    (false, Form::Weigh(wt)) => {
                                        k.chroma_mc_weighted(dst, src, xf, yf, wt)
                                    }
                                }
                                out
                            };
                            let want = run(&Scalar, &window);
                            for (name, k) in &kernels {
                                let what =
                                    format!("{name}: luma {luma}, {w}x{h} ({xf}, {yf}) {form:?}");
                                assert_eq!(run(k.as_ref(), &window), want, "{what}");
                                assert_eq!(run(k.as_ref(), &other), want, "{what}, unused changed");
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The same contract through `Kernels::predict`, whose windows at the
/// picture's edges reach past it where only unused samples lie outside:
/// for every block it takes, every sample of the reference's slices that
/// the prediction does not use (the window's unused samples, its slack
/// before each origin and after each last row, and everything outside the
/// windows) is made random again, and every implementation gives the same
/// planes as before and as `Scalar`.
#[test]
fn three_plane_prediction_ignores_the_samples_it_does_not_use() {
    let mut rng = Rng(0x5ac4);
    let (stride, rows) = (32, 24);
    let cs = stride / 2;
    let kernels = contract_kernels();
    let (reference, origin) = slack_reference(&mut rng, stride, rows, [4, 2]);
    let (mut taken, mut past_the_edge) = (0, 0);
    for case in 0..10_000 {
        let size = |rng: &mut Rng| [4, 8, 16][rng.below(3) as usize];
        let (w, h) = (size(&mut rng), size(&mut rng));
        let px = 4 * rng.below(((stride - w) / 4 + 1) as u64) as usize;
        let py = 4 * rng.below(((rows - h) / 4 + 1) as u64) as usize;
        // Mostly vectors that put the block at an edge of the reference,
        // with a zero fraction across or down half the time.
        let edge = |rng: &mut Rng, p: usize, n: usize, len: usize| -> i16 {
            let at = match rng.below(3) {
                0 => rng.below(3) as i16,
                1 => (n - len) as i16 - rng.below(3) as i16,
                _ => rng.below((n - len + 1) as u64) as i16,
            };
            let frac = if rng.below(2) == 0 {
                0
            } else {
                rng.below(4) as i16
            };
            (at - p as i16) * 4 + frac
        };
        let mv = [edge(&mut rng, px, stride, w), edge(&mut rng, py, rows, h)];
        let op = |rng: &mut Rng| match rng.below(3) {
            0 => PlaneOp::Put,
            1 => PlaneOp::Avg,
            _ => PlaneOp::Weigh(weighting(rng)),
        };
        let ops = [op(&mut rng), op(&mut rng), op(&mut rng)];
        let picture: Vec<Vec<u8>> = [stride * rows, cs * rows / 2, cs * rows / 2]
            .into_iter()
            .map(|n| rng.samples(n))
            .collect();
        let run = |k: &dyn Kernels, planes: &[Vec<u8>]| {
            let src = Planes::with_origin(&planes[0], &planes[1], &planes[2], stride, rows, origin)
                .expect("planes");
            let mut got = picture.clone();
            let [gy, gcb, gcr] = &mut got[..] else {
                unreachable!()
            };
            let dst = PlanesMut::new(gy, gcb, gcr, stride, [0, 0]);
            let done = k.predict(dst, &src, px, py, w, h, mv, ops);
            (done, got)
        };
        let want = run(&Scalar, &reference);
        if !want.0 {
            continue;
        }
        taken += 1;
        // The samples used, in each slice.
        let (xf, yf) = ((mv[0] & 3) as u8, (mv[1] & 3) as u8);
        let (cxf, cyf) = ((mv[0] & 7) as u8, (mv[1] & 7) as u8);
        let x = px as isize + isize::from(mv[0] >> 2) - 2;
        let y = py as isize + isize::from(mv[1] >> 2) - 2;
        let cx = (px / 2) as isize + isize::from(mv[0] >> 3);
        let cy = (py / 2) as isize + isize::from(mv[1] >> 3);
        let mut used: Vec<Vec<bool>> = reference.iter().map(|p| vec![false; p.len()]).collect();
        let mut outside = false;
        for j in 0..h + 5 {
            for i in 0..w + 5 {
                let (sx, sy) = (x + i as isize, y + j as isize);
                let inside = (0..stride as isize).contains(&sx) && (0..rows as isize).contains(&sy);
                if luma_uses(w, h, xf, yf, i, j) {
                    assert!(
                        inside,
                        "case {case}: a luma sample used outside the picture"
                    );
                    used[0][origin[0] + sy as usize * stride + sx as usize] = true;
                } else {
                    outside |= !inside;
                }
            }
        }
        for j in 0..h / 2 + 1 {
            for i in 0..w / 2 + 1 {
                let (sx, sy) = (cx + i as isize, cy + j as isize);
                let inside =
                    (0..cs as isize).contains(&sx) && (0..(rows / 2) as isize).contains(&sy);
                if chroma_uses(w / 2, h / 2, cxf, cyf, i, j) {
                    assert!(
                        inside,
                        "case {case}: a chroma sample used outside the picture"
                    );
                    for plane in &mut used[1..] {
                        plane[origin[1] + sy as usize * cs + sx as usize] = true;
                    }
                } else {
                    outside |= !inside;
                }
            }
        }
        past_the_edge += usize::from(outside);
        let mut other = reference.clone();
        for (plane, used) in other.iter_mut().zip(&used) {
            for (s, &u) in plane.iter_mut().zip(used) {
                if !u {
                    *s = rng.sample();
                }
            }
        }
        for (name, k) in &kernels {
            let what = format!("{name}: case {case}, {w}x{h} at ({px}, {py}), {mv:?}, {ops:?}");
            assert_eq!(run(k.as_ref(), &reference), want, "{what}");
            assert_eq!(run(k.as_ref(), &other), want, "{what}, unused changed");
        }
    }
    assert!(
        taken > 2000 && past_the_edge > 500,
        "{taken} {past_the_edge}"
    );
}

/// The weighted prediction at full-sample positions, which the x86
/// kernels weigh in 16-bit lanes where the weights allow it: weights at
/// and just past the bounds of that form (the positive weights adding to
/// 128 or 129, the negative to -128 or -129, a weight of 128), every
/// logWD, offsets at their ends, and samples mostly 0 and 255, so that
/// the sums reach their ends. Every kernel against [`predicted`].
#[test]
fn full_sample_weighting_at_its_bounds_matches_the_reference() {
    let mut rng = Rng(0xf011);
    let (stride, rows) = (48, 40);
    let extreme = |rng: &mut Rng, n: usize| -> Vec<u8> {
        (0..n)
            .map(|_| match rng.below(4) {
                0 => 0,
                1 | 2 => 255,
                _ => rng.below(256) as u8,
            })
            .collect()
    };
    let reference: Vec<Vec<u8>> = [stride * rows, stride * rows / 4, stride * rows / 4]
        .into_iter()
        .map(|n| extreme(&mut rng, n))
        .collect();
    let src =
        Planes::new(&reference[0], &reference[1], &reference[2], stride, rows).expect("planes");
    let auto = Auto::detect();
    let mut kernels: Vec<(&str, Box<dyn Kernels>)> = vec![
        ("auto", Box::new(auto)),
        ("auto-avx2", Box::new(auto.without_avx512())),
    ];
    if let Some(simd) = simd() {
        kernels.push(("simd", Box::new(simd)));
        kernels.push(("simd-avx2", Box::new(simd.without_avx512())));
    }
    let pairs = [
        (127, 1),
        (127, 2),
        (64, 64),
        (65, 64),
        (128, 0),
        (0, 128),
        (-128, 127),
        (127, -128),
        (-64, -64),
        (-65, -64),
        (-128, 0),
        (0, 0),
        (32, 32),
        (-64, 128),
    ];
    let mut weightings = Vec::new();
    for log_wd in 0..=7 {
        for o in [-128, -1, 0, 127] {
            for w in [-128, -1, 0, 1, 64, 127, 128] {
                weightings.push(Weighting::Uni { log_wd, w, o });
            }
            for (w0, w1) in pairs {
                weightings.push(Weighting::Bi { log_wd, w0, w1, o });
            }
        }
    }
    for (case, weighting) in weightings.into_iter().enumerate() {
        for (w, h) in [(16, 16), (16, 8), (8, 16), (8, 8)] {
            let (px, py) = (8 * rng.below(4) as usize, 8 * rng.below(3) as usize);
            // Whole samples, inside the reference.
            let mv = [
                (rng.below(20) as i16 + 2 - px as i16) * 4,
                (rng.below(10) as i16 + 2 - py as i16) * 4,
            ];
            let ops = [PlaneOp::Weigh(weighting), PlaneOp::Put, PlaneOp::Put];
            let cs = stride / 2;
            let picture: Vec<Vec<u8>> = [stride * rows, cs * rows / 2, cs * rows / 2]
                .into_iter()
                .map(|n| extreme(&mut rng, n))
                .collect();
            let mut want = picture.clone();
            let [wy, wcb, wcr] = &mut want[..] else {
                unreachable!()
            };
            let model = Reference {
                planes: [&reference[0], &reference[1], &reference[2]],
                origin: [0, 0],
                stride,
                rows,
            };
            let expected = predicted(
                [wy, wcb, wcr],
                [0, 0],
                [0, 0],
                model,
                (px, py),
                (w, h),
                mv,
                ops,
            );
            assert!(expected.is_some(), "case {case}: {mv:?}");
            for (name, k) in &kernels {
                let mut got = picture.clone();
                let [gy, gcb, gcr] = &mut got[..] else {
                    unreachable!()
                };
                let dst = PlanesMut::new(gy, gcb, gcr, stride, [0, 0]);
                assert!(k.predict(dst, &src, px, py, w, h, mv, ops));
                assert_eq!(got, want, "{name}: case {case}, {w}x{h}, {weighting:?}");
            }
        }
    }
}
