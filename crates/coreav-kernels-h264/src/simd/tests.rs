//! Bar C3, at the kernel itself: every vector kernel against the
//! reference, whether or not the dispatch of [`super::Simd`] sends work to
//! it.
//!
//! `tests/equivalence.rs` checks the same thing through the public
//! `Kernels` implementation, which is what the decoder calls; these tests
//! call the architecture's kernels directly, so a kernel that the
//! measurement currently routes around is still covered.

use super::arch;
use crate::scalar::Scalar;
use crate::views::{
    Block, BlockMut, ChromaIntraMut, ChromaMbMut, EdgeMut, IntraAvail, IntraMut, Reach,
};
use crate::{Combine, EdgeParams, Kernels};

/// A reproducible pseudo-random source (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> usize {
        (self.next() % n.max(1)) as usize
    }

    /// A sample that favours the ends of the range, where kernels clip.
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
            _ => self.below(65536) as i32 - 32768,
        }
    }
}

/// Whether this build has vector kernels at all.
fn available() -> bool {
    super::Simd::detect().is_some()
}

#[test]
fn the_8x8_transform_matches_the_reference() {
    if !available() {
        return;
    }
    let mut rng = Rng(11);
    for case in 0..2000 {
        let stride = 8 + case % 5;
        let offset = case % 3;
        let plane = rng.samples(offset + 8 * stride + 16);
        let mut coeffs = [0i32; 64];
        for (i, c) in coeffs.iter_mut().enumerate() {
            if case % 4 != 0 || i % 7 == 0 {
                *c = rng.coefficient();
            }
        }
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.idct8x8_add(
            BlockMut::new(&mut want, offset, stride, 8, 8).expect("block"),
            &coeffs,
        );
        // SAFETY: the CPU has the kernels (`available`), and the view
        // covers the block.
        unsafe {
            arch::idct8x8_add(
                BlockMut::new(&mut got, offset, stride, 8, 8).expect("block"),
                &coeffs,
            );
        }
        assert_eq!(got, want, "case {case}");
    }
}

#[test]
fn interpolation_matches_the_reference() {
    if !available() {
        return;
    }
    let mut rng = Rng(12);
    for case in 0..3000 {
        let (w, h) = match case % 4 {
            0 => (16, 16),
            1 => (8, 8),
            2 => (16, 1 + case % 16),
            _ => (8, 1 + case % 9),
        };
        let (xf, yf) = ((case % 4) as u8, ((case / 4) % 4) as u8);
        let src_stride = w + 5 + case % 4;
        let src = rng.samples(src_stride * (h + 5) + 8);
        let dst_stride = w + case % 3;
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
        // SAFETY: as above.
        unsafe {
            arch::luma_mc(
                BlockMut::new(&mut got, 0, dst_stride, w, h).expect("block"),
                source,
                xf,
                yf,
            );
        }
        assert_eq!(got, want, "luma {w}x{h} ({xf}, {yf}), case {case}");

        // Chroma, whatever the dispatch does with it.
        // Every width, every height up to 8 (the odd ones leave a pair of
        // rows half filled), and every position: 3 x 8 x 64 = 1,536
        // combinations, each twice over the 3,000 cases.
        let cw = [8, 4, 2][case % 3];
        let ch = 1 + (case / 3) % 8;
        let (xf, yf) = (((case / 24) % 8) as u8, ((case / 192) % 8) as u8);
        let src_stride = cw + 1 + case % 3;
        let src = rng.samples(src_stride * (ch + 1) + 8);
        let dst_stride = cw + case % 2;
        let plane = rng.samples(dst_stride * ch + 8);
        let source = Block::new(&src, 0, src_stride, cw + 1, ch + 1).expect("block");
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.chroma_mc(
            BlockMut::new(&mut want, 0, dst_stride, cw, ch).expect("block"),
            source,
            xf,
            yf,
        );
        #[cfg(target_arch = "x86_64")]
        arch::chroma_mc(
            BlockMut::new(&mut got, 0, dst_stride, cw, ch).expect("block"),
            source,
            xf,
            yf,
            arch::Finish::Write,
        );
        #[cfg(not(target_arch = "x86_64"))]
        // SAFETY: as above.
        unsafe {
            arch::chroma_mc(
                BlockMut::new(&mut got, 0, dst_stride, cw, ch).expect("block"),
                source,
                xf,
                yf,
            );
        }
        assert_eq!(got, want, "chroma {cw}x{ch} ({xf}, {yf}), case {case}");

        // The x86 kernel's averaging form, into a block that already holds
        // other samples.
        #[cfg(target_arch = "x86_64")]
        {
            let mut want = plane.iter().rev().copied().collect::<Vec<_>>();
            let mut got = want.clone();
            Scalar.chroma_mc_avg(
                BlockMut::new(&mut want, 0, dst_stride, cw, ch).expect("block"),
                source,
                xf,
                yf,
            );
            arch::chroma_mc(
                BlockMut::new(&mut got, 0, dst_stride, cw, ch).expect("block"),
                source,
                xf,
                yf,
                arch::Finish::Average,
            );
            assert_eq!(got, want, "chroma avg {cw}x{ch} ({xf}, {yf}), case {case}");
        }
    }
}

/// A 4-wide luma block, which the x86 kernels predict through the 8-wide
/// one on copies: every height and position, written, averaged and
/// weighted, from windows whose rows end at the slice's end (stride 9, and
/// the last row of every stride) as well as from wider ones, into blocks
/// whose neighbours must stay as they were.
#[cfg(target_arch = "x86_64")]
#[test]
fn a_4_wide_luma_block_matches_the_reference() {
    use crate::Weighting;
    if !available() {
        return;
    }
    let mut rng = Rng(41);
    for case in 0..4096 {
        let h = 1 + case % 16;
        let (xf, yf) = (((case / 16) % 4) as u8, ((case / 64) % 4) as u8);
        let src_stride = [9, 10, 13, 16, 24][rng.below(5)];
        // Exactly the window: the last row ends the slice.
        let src = rng.samples(src_stride * (h + 4) + 9);
        let source = Block::new(&src, 0, src_stride, 9, h + 5).expect("window");
        let dst_stride = 4 + rng.below(13);
        let plane = rng.samples(dst_stride * (h + 1) + 4);
        let at = rng.below(dst_stride as u64 - 3);
        let weighting = match rng.below(3) {
            0 => Weighting::Uni {
                log_wd: rng.below(8) as i32,
                w: rng.below(256) as i32 - 128,
                o: rng.below(256) as i32 - 128,
            },
            _ => Weighting::Bi {
                log_wd: rng.below(8) as i32,
                w0: rng.below(256) as i32 - 128,
                w1: rng.below(256) as i32 - 128,
                o: rng.below(256) as i32 - 128,
            },
        };
        for form in 0..3 {
            let mut want = plane.clone();
            let mut got = plane.clone();
            let block = |out| BlockMut::new(out, at, dst_stride, 4, h).expect("block");
            match form {
                0 => Scalar.luma_mc(block(want.as_mut_slice()), source, xf, yf),
                1 => Scalar.luma_mc_avg(block(want.as_mut_slice()), source, xf, yf),
                _ => Scalar.luma_mc_weighted(block(want.as_mut_slice()), source, xf, yf, weighting),
            }
            // SAFETY: as above.
            unsafe {
                match form {
                    0 => arch::luma_mc(block(got.as_mut_slice()), source, xf, yf),
                    1 => arch::luma_mc_avg(block(got.as_mut_slice()), source, xf, yf),
                    _ => {
                        arch::luma_mc_weighted(block(got.as_mut_slice()), source, xf, yf, weighting)
                    }
                }
            }
            assert_eq!(
                got, want,
                "4x{h} ({xf}, {yf}), form {form}, strides {src_stride} and {dst_stride}, case {case}"
            );
        }
    }
}

#[test]
fn prediction_combining_matches_the_reference() {
    if !available() {
        return;
    }
    let mut rng = Rng(13);
    for case in 0..3000 {
        let (w, h) = match case % 7 {
            0 => (16, 16),
            1 => (16, 8),
            2 => (8, 8),
            3 => (4, 4),
            4 => (1, 1),
            5 => (12, 5),
            _ => (15, 16),
        };
        let stride = w + case % 3;
        let p0 = rng.samples(16 * 16);
        let p1 = rng.samples(16 * 16);
        let plane = rng.samples(stride * h + 8);
        let log_wd = (case % 8) as i32;
        let weight = |rng: &mut Rng| rng.below(256) as i32 - 128;
        let mode = match case % 3 {
            0 => Combine::Uni {
                list: case % 2,
                log_wd,
                w: weight(&mut rng),
                o: weight(&mut rng),
            },
            1 => Combine::Bi {
                log_wd,
                w0: weight(&mut rng),
                w1: weight(&mut rng),
                o: weight(&mut rng),
            },
            _ => Combine::Average,
        };
        let a = Block::new(&p0, 0, 16, w, h).expect("block");
        let b = Block::new(&p1, 0, 16, w, h).expect("block");
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.combine(
            BlockMut::new(&mut want, 0, stride, w, h).expect("block"),
            a,
            b,
            mode,
        );
        // SAFETY: as above. The vector kernel leaves a copy to the
        // reference, so the modes it computes are the ones compared here.
        unsafe {
            arch::combine(
                BlockMut::new(&mut got, 0, stride, w, h).expect("block"),
                a,
                b,
                mode,
            );
        }
        if matches!(mode, Combine::Average) {
            // The vector kernel returns without writing for this mode;
            // the comparison is that it leaves the block alone.
            assert_eq!(got, plane, "average, case {case}");
            continue;
        }
        assert_eq!(got, want, "{w}x{h} {mode:?}, case {case}");
    }
}

#[test]
fn the_deblocking_filter_matches_the_reference() {
    if !available() {
        return;
    }
    let mut rng = Rng(14);
    for case in 0..4000 {
        let chroma = case % 3 == 0;
        let lines = match case % 5 {
            0 => 4,
            1 => 8,
            2 => 12,
            3 => 16,
            _ => 20,
        };
        let reach = if chroma { Reach::Chroma } else { Reach::Luma };
        let vertical = case % 2 == 0;
        let stride = 32;
        let plane: Vec<u8> = if case % 2 == 0 {
            rng.samples(stride * (lines + 12) + 8)
        } else {
            // Samples a filter actually works on: the two sides of the
            // edge differ by less than beta.
            (0..stride * (lines + 12) + 8)
                .map(|i| {
                    let step = if (i % stride) < stride / 2 { 100 } else { 118 };
                    step + ((i * 7) % 5) as u8
                })
                .collect()
        };
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
        let (q0, across, along) = if vertical {
            (4 * stride + 8, 1, stride)
        } else {
            (8 * stride + 4, stride, 1)
        };
        let mut want = plane.clone();
        let mut got = plane.clone();
        Scalar.filter_edge(
            EdgeMut::new(&mut want, q0, across, along, lines, reach).expect("edge"),
            &params,
        );
        // SAFETY: as above.
        unsafe {
            arch::filter_edge(
                EdgeMut::new(&mut got, q0, across, along, lines, reach).expect("edge"),
                &params,
            );
        }
        assert_eq!(
            got, want,
            "case {case}: chroma {chroma}, vertical {vertical}, {lines} lines"
        );
    }
}

/// An intra view's availability from four bits.
fn intra_avail(bits: usize) -> IntraAvail {
    IntraAvail {
        left: bits & 1 != 0,
        top: bits & 2 != 0,
        top_right: bits & 4 != 0,
        top_left: bits & 8 != 0,
    }
}

/// A plane for an `n` by `n` intra block at column `x` and row `y` of a
/// plane with `stride`, reaching 2n samples right of the block's row
/// above, and ending at the block's last sample where `tight`; smooth (a
/// ramp) or noisy samples.
fn intra_plane(rng: &mut Rng, stride: usize, x: usize, y: usize, n: usize, tight: bool) -> Vec<u8> {
    let end = (y + n - 1) * stride + x + n;
    let len = if tight {
        end
    } else {
        end + rng.below(3 * stride as u64)
    };
    if rng.below(3) == 0 {
        let base = rng.sample();
        let (dx, dy) = (rng.below(5) as u8, rng.below(5) as u8);
        (0..len)
            .map(|i| {
                base.wrapping_add(dx.wrapping_mul((i % stride) as u8))
                    .wrapping_add(dy.wrapping_mul((i / stride) as u8))
            })
            .collect()
    } else {
        rng.samples(len)
    }
}

/// The intra kernels against the reference: every combination of
/// neighbours, every mode and some out of range, strides from the
/// narrowest a view allows, planes that end at the block's last sample,
/// and coefficients over their whole range.
#[test]
fn intra_kernels_match_the_reference() {
    let Some(simd) = super::Simd::detect() else {
        return;
    };
    let mut rng = Rng(0x1a7a);
    let mut cases = 0;
    for round in 0..3000 {
        let bits = round % 16;
        let avail = intra_avail(bits);
        let tight = round % 3 == 0;
        // Intra_16x16 and Intra_4x4 macroblocks.
        let stride = [16, 17, 20, 32, 48][rng.below(5)];
        let x = 1 + rng.below((stride - 16) as u64 + 1).min(stride - 16);
        let (x, y) = (x.min(stride - 16), 1 + rng.below(2));
        let x = if avail.left || avail.top_left {
            x.max(1)
        } else {
            x
        };
        let plane = intra_plane(&mut rng, stride, x, y, 16, tight);
        let off = y * stride + x;
        let mode = [0, 1, 2, 3, 4, 255][rng.below(6)] as u8;
        let (mut want, mut got) = (plane.clone(), plane.clone());
        if let (Ok(a), Ok(b)) = (
            IntraMut::new(&mut want, off, stride, 16, avail),
            IntraMut::new(&mut got, off, stride, 16, avail),
        ) {
            Scalar.intra16x16(a, mode);
            simd.intra16x16(b, mode);
            assert_eq!(
                got, want,
                "16x16 mode {mode} stride {stride} at ({x}, {y}), {avail:?}"
            );
            cases += 1;
        }
        let modes: [u8; 16] = core::array::from_fn(|_| match rng.below(20) {
            0 => 9,
            1 => 200,
            _ => rng.below(9) as u8,
        });
        let coeffs: [[i32; 16]; 16] = core::array::from_fn(|_| {
            core::array::from_fn(|_| {
                if rng.below(3) == 0 {
                    rng.coefficient()
                } else {
                    rng.below(512) as i32 - 256
                }
            })
        });
        let nz = match round % 4 {
            0 => 0,
            1 => 0xffff,
            _ => rng.below(1 << 16) as u16,
        };
        let modes8: [u8; 4] = core::array::from_fn(|_| match rng.below(20) {
            0 => 9,
            1 => 200,
            _ => rng.below(9) as u8,
        });
        let coeffs8: [[i32; 64]; 4] = core::array::from_fn(|_| {
            core::array::from_fn(|_| {
                if rng.below(3) == 0 {
                    rng.coefficient()
                } else {
                    rng.below(512) as i32 - 256
                }
            })
        });
        let nz8 = match rng.below(4) {
            0 => 0,
            1 => 0xf,
            _ => rng.below(16) as u8,
        };
        let (mut want, mut got) = (plane.clone(), plane.clone());
        if let (Ok(a), Ok(b)) = (
            IntraMut::new(&mut want, off, stride, 16, avail),
            IntraMut::new(&mut got, off, stride, 16, avail),
        ) {
            Scalar.intra8x8_mb(a, &modes8, &coeffs8, nz8);
            simd.intra8x8_mb(b, &modes8, &coeffs8, nz8);
            assert_eq!(
                got, want,
                "8x8 {modes8:?} nz {nz8:#x} stride {stride} at ({x}, {y}), {avail:?}"
            );
            cases += 1;
        }
        let (mut want, mut got) = (plane.clone(), plane);
        if let (Ok(a), Ok(b)) = (
            IntraMut::new(&mut want, off, stride, 16, avail),
            IntraMut::new(&mut got, off, stride, 16, avail),
        ) {
            Scalar.intra4x4_mb(a, &modes, &coeffs, nz);
            simd.intra4x4_mb(b, &modes, &coeffs, nz);
            assert_eq!(
                got, want,
                "4x4 {modes:?} nz {nz:#x} stride {stride} at ({x}, {y}), {avail:?}"
            );
            cases += 1;
        }
        // Chroma.
        let stride = [8, 9, 12, 16, 24][rng.below(5)];
        let (x, y) = (1 + rng.below(2), 1 + rng.below(2));
        let x = x.min(stride - 8);
        let plane = intra_plane(&mut rng, stride, x, y, 8, tight);
        let off = y * stride + x;
        let mode = rng.below(6) as u8;
        let (mut want, mut got) = (plane.clone(), plane);
        if let (Ok(a), Ok(b)) = (
            IntraMut::new(&mut want, off, stride, 8, avail),
            IntraMut::new(&mut got, off, stride, 8, avail),
        ) {
            Scalar.intra_chroma(a, mode);
            simd.intra_chroma(b, mode);
            assert_eq!(
                got, want,
                "chroma mode {mode} stride {stride} at ({x}, {y}), {avail:?}"
            );
            cases += 1;
        }
    }
    assert!(cases > 7000, "{cases} cases");
}

/// The chroma of a macroblock in one call against the reference, the
/// scalar kernels a component at a time, and against the residual built
/// block by block from the trait's description: intra in every mode (and
/// out of range) and inter, every combination of neighbours (and
/// components that differ in them, which the vector kernel hands to the
/// reference), DC and AC coefficients over their whole range, blocks with
/// a DC alone, with AC alone and with neither, every mask, strides from the
/// narrowest a view allows and differing between the components, and
/// planes that end at the block's last sample.
#[test]
fn chroma_mb_matches_the_reference() {
    let Some(simd) = super::Simd::detect() else {
        return;
    };
    let mut rng = Rng(0xc4a0);
    let coefficient = |rng: &mut Rng| {
        if rng.below(3) == 0 {
            rng.coefficient()
        } else {
            rng.below(512) as i32 - 256
        }
    };
    fn view<'a>(
        b: &'a mut [u8],
        r: &'a mut [u8],
        (at, stride, avail): (usize, usize, IntraAvail),
    ) -> Option<ChromaIntraMut<'a>> {
        ChromaIntraMut::new(b, r, at, stride, avail).ok()
    }
    fn blocks<'a>(b: &'a mut [u8], r: &'a mut [u8], at: [(usize, usize); 2]) -> [BlockMut<'a>; 2] {
        let [(ob, sb), (or, sr)] = at;
        [
            BlockMut::new(b, ob, sb, 8, 8).expect("block"),
            BlockMut::new(r, or, sr, 8, 8).expect("block"),
        ]
    }
    let mut cases = 0;
    for round in 0..4000 {
        let avail = intra_avail(round % 16);
        let tight = round % 3 == 0;
        let mode = match rng.below(8) {
            0 => 4 + rng.below(252) as u8,
            _ => rng.below(4) as u8,
        };
        let dc: [[i32; 4]; 2] = core::array::from_fn(|_| {
            let none = rng.below(4) == 0;
            core::array::from_fn(|_| match rng.below(3) {
                _ if none => 0,
                0 => 0,
                _ => coefficient(&mut rng),
            })
        });
        let ac: [[[i32; 16]; 4]; 2] = core::array::from_fn(|_| {
            core::array::from_fn(|_| core::array::from_fn(|_| coefficient(&mut rng)))
        });
        let ac_nz = match round % 5 {
            0 | 1 => 0,
            2 => 0xff,
            _ => rng.below(256) as u8,
        };
        // The components share their geometry (the intra view requires it),
        // except now and then, when the intra view is built with Cb's and
        // Cr's plane may be shorter (no view) or longer than Cb's.
        let mut planes = Vec::new();
        let mut shape = None;
        for c in 0..2 {
            let mut fresh = || {
                let stride = [8, 9, 12, 16, 24][rng.below(5)];
                let (x, y) = (1 + rng.below(2), 1 + rng.below(2));
                (stride, x.min(stride - 8), y)
            };
            let (stride, x, y) = match shape {
                Some(s) if c == 1 && round % 37 != 0 => s,
                _ => fresh(),
            };
            shape = Some((stride, x, y));
            let plane = intra_plane(&mut rng, stride, x, y, 8, tight);
            planes.push((plane, y * stride + x, stride));
        }
        let [(pb, ob, sb), (pr, or, sr)] = <[_; 2]>::try_from(planes).expect("two planes");
        let what = format!(
            "mode {mode} ac_nz {ac_nz:#x} dc {dc:?} strides {sb}, {sr} at {ob}, {or}, {avail:?}"
        );
        // Each component's residual a block at a time, as the trait
        // describes it.
        let residual = |v: &mut BlockMut<'_>, c: usize| {
            for k in 0..4 {
                let mut block = if ac_nz & (1 << (4 * c + k)) != 0 {
                    ac[c][k]
                } else {
                    [0; 16]
                };
                block[0] = dc[c][k];
                if block != [0; 16] {
                    let at = (k / 2) * 4 * v.stride() + (k % 2) * 4;
                    let stride = v.stride();
                    let b = BlockMut::new(v.samples_mut(), at, stride, 4, 4).expect("4x4");
                    Scalar.idct4x4_add(b, &block);
                }
            }
        };
        // Intra.
        let at = (ob, sb, avail);
        let (mut want_b, mut want_r, mut got_b, mut got_r) =
            (pb.clone(), pr.clone(), pb.clone(), pr.clone());
        if let (Some(w), Some(g)) = (
            view(&mut want_b, &mut want_r, at),
            view(&mut got_b, &mut got_r, at),
        ) {
            Scalar.intra_chroma_mb(w, mode, &dc, &ac, ac_nz);
            simd.intra_chroma_mb(g, mode, &dc, &ac, ac_nz);
            assert_eq!(got_b, want_b, "intra Cb: {what}");
            assert_eq!(got_r, want_r, "intra Cr: {what}");
            let (mut b, mut r) = (pb.clone(), pr.clone());
            if let Some(v) = view(&mut b, &mut r, at) {
                for (c, mut v) in v.into_parts().into_iter().enumerate() {
                    let a = v.avail();
                    Scalar.intra_chroma(v.sub(0, 0, 8, a).expect("view"), mode);
                    residual(&mut v.block_mut().expect("block"), c);
                }
            }
            assert_eq!((b, r), (want_b, want_r), "intra by blocks: {what}");
            cases += 1;
        }
        // Inter.
        let at = [(ob, sb), (or, sr)];
        let (mut want_b, mut want_r, mut got_b, mut got_r) =
            (pb.clone(), pr.clone(), pb.clone(), pr.clone());
        let [wb, wr] = blocks(&mut want_b, &mut want_r, at);
        Scalar.chroma_residual_mb(wb, wr, &dc, &ac, ac_nz);
        let [gb, gr] = blocks(&mut got_b, &mut got_r, at);
        simd.chroma_residual_mb(gb, gr, &dc, &ac, ac_nz);
        assert_eq!(got_b, want_b, "inter Cb: {what}");
        assert_eq!(got_r, want_r, "inter Cr: {what}");
        let (mut b, mut r) = (pb, pr);
        for (c, mut v) in blocks(&mut b, &mut r, at).into_iter().enumerate() {
            residual(&mut v, c);
        }
        assert_eq!((b, r), (want_b, want_r), "inter by blocks: {what}");
        cases += 1;
    }
    assert!(cases > 7500, "{cases} cases");
}

/// Thresholds in the ranges of Tables 8-16 and 8-17, sometimes with α 0
/// (an edge the decoder does not filter in that plane).
fn chroma_params(rng: &mut Rng, bs: [u8; 4]) -> EdgeParams {
    let mut tc0 = [0; 8];
    for t in tc0.iter_mut() {
        *t = rng.below(26) as u8;
    }
    EdgeParams {
        alpha: if rng.below(8) == 0 {
            0
        } else {
            rng.below(256) as u8
        },
        beta: rng.below(19) as u8,
        tc0,
        bs,
    }
}

/// The chroma edges of a macroblock in one call against the reference,
/// [`Kernels::filter_chroma_pair`] of the scalar kernels an edge at a time:
/// every subset of the four edges, strengths 0 to 4 per segment, thresholds
/// that differ between the planes (α 0 included), smooth samples a filter
/// works on as well as noise, strides from the narrowest a window takes,
/// planes of different strides, and windows that start at their plane's
/// first sample and end at its last.
#[test]
fn the_chroma_macroblock_matches_the_reference() {
    let Some(simd) = super::Simd::detect() else {
        return;
    };
    let auto = crate::Auto::detect();
    let mut rng = Rng(0x3d5b);
    let mut changed = 0;
    for case in 0..8000 {
        let stride = [10, 11, 16, 24, 40][rng.below(5)];
        let stride_cr = if case % 13 == 0 { 16 } else { stride };
        let smooth = case % 3 != 0;
        // The window's first sample at index 0 of the plane, or a row and a
        // few samples in; its last sample the plane's last, or not.
        let lead = if case % 4 == 0 { 0 } else { stride + 3 };
        let plane = |stride: usize, rng: &mut Rng| -> Vec<u8> {
            let end = lead + 9 * stride + 10;
            let len = if case % 5 == 1 { end } else { end + 2 * stride };
            if smooth {
                let base = [0, 2, 60, 120, 240][rng.below(5)] as u8;
                let step = [3, 9, 20][rng.below(3)] as u8;
                (0..len)
                    .map(|i| {
                        let (row, col) = (
                            (i - i.min(lead)) / stride,
                            (i + stride - lead % stride) % stride,
                        );
                        let side = (row % 4 >= 2) ^ (col % 4 >= 2);
                        base.saturating_add(if side { step } else { 0 })
                            .saturating_add(rng.below(4) as u8)
                    })
                    .collect()
            } else {
                rng.samples(len)
            }
        };
        let cb = plane(stride, &mut rng);
        let cr = plane(stride_cr, &mut rng);
        let mut params = [[EdgeParams::default(); 2]; 4];
        for (slot, pair) in params.iter_mut().enumerate() {
            // Each edge filtered or not, as the decoder's strengths leave it.
            if (case >> slot) & 1 == 1 && case % 11 != 0 {
                continue;
            }
            let mut bs = [0u8; 4];
            for b in bs.iter_mut() {
                *b = rng.below(5) as u8;
            }
            let mut bs_cr = bs;
            if case % 7 == 0 {
                for b in bs_cr.iter_mut() {
                    *b = rng.below(5) as u8;
                }
            }
            *pair = [chroma_params(&mut rng, bs), chroma_params(&mut rng, bs_cr)];
        }
        let at = |stride: usize| lead + 2 * stride + 2;
        let (mut want_cb, mut want_cr) = (cb.clone(), cr.clone());
        crate::chroma_mb_by_edges(
            &Scalar,
            ChromaMbMut::new(&mut want_cb, at(stride), stride).expect("window"),
            ChromaMbMut::new(&mut want_cr, at(stride_cr), stride_cr).expect("window"),
            &params,
        );
        changed += usize::from(want_cb != cb) + usize::from(want_cr != cr);
        let (mut got_cb, mut got_cr) = (cb.clone(), cr.clone());
        simd.filter_chroma_mb(
            ChromaMbMut::new(&mut got_cb, at(stride), stride).expect("window"),
            ChromaMbMut::new(&mut got_cr, at(stride_cr), stride_cr).expect("window"),
            &params,
        );
        assert_eq!(got_cb, want_cb, "case {case}: Cb, stride {stride}");
        assert_eq!(got_cr, want_cr, "case {case}: Cr, stride {stride_cr}");
        let (mut got_cb, mut got_cr) = (cb, cr);
        auto.filter_chroma_mb(
            ChromaMbMut::new(&mut got_cb, at(stride), stride).expect("window"),
            ChromaMbMut::new(&mut got_cr, at(stride_cr), stride_cr).expect("window"),
            &params,
        );
        assert_eq!(got_cb, want_cb, "case {case}: Cb, Auto");
        assert_eq!(got_cr, want_cr, "case {case}: Cr, Auto");
    }
    assert!(changed > 4000, "{changed} planes filtered");
}

/// The chroma edges of a macroblock in one call against the reference on
/// parameters of any bytes (`x86::chroma_mb::tests::extreme_params`): bS
/// past 4 and t'C0 past the 31 the vector kernel on bytes takes, which it
/// hands to the paired kernel, and in-range rows beside them.
#[cfg(target_arch = "x86_64")]
#[test]
fn the_chroma_macroblock_matches_the_reference_on_any_parameters() {
    let Some(simd) = super::Simd::detect() else {
        return;
    };
    let mut rng = Rng(0x51c3);
    let all = super::x86::chroma_mb::tests::extreme_params(8 * 3000);
    for (case, eight) in all.chunks_exact(8).enumerate() {
        let stride = [10, 16, 24][case % 3];
        // Smooth samples, so that lines pass the thresholds that let them.
        let base = rng.below(200) as u8;
        let plane = |rng: &mut Rng| -> Vec<u8> {
            (0..12 * stride)
                .map(|_| base.saturating_add(rng.below(12) as u8))
                .collect()
        };
        let (cb, cr) = (plane(&mut rng), plane(&mut rng));
        let mut params = [[EdgeParams::default(); 2]; 4];
        for (k, p) in params.iter_mut().flatten().enumerate() {
            *p = eight[k];
            // Half the rows within the table's range, so that the vector
            // kernel runs as well as its fallback.
            if case % 2 == 0 {
                p.tc0 = p.tc0.map(|t| t % 32);
            }
        }
        let at = 2 * stride + 2;
        let (mut want_cb, mut want_cr) = (cb.clone(), cr.clone());
        crate::chroma_mb_by_edges(
            &Scalar,
            ChromaMbMut::new(&mut want_cb, at, stride).expect("window"),
            ChromaMbMut::new(&mut want_cr, at, stride).expect("window"),
            &params,
        );
        let (mut got_cb, mut got_cr) = (cb, cr);
        simd.filter_chroma_mb(
            ChromaMbMut::new(&mut got_cb, at, stride).expect("window"),
            ChromaMbMut::new(&mut got_cr, at, stride).expect("window"),
            &params,
        );
        assert_eq!(got_cb, want_cb, "case {case}: Cb, {params:?}");
        assert_eq!(got_cr, want_cr, "case {case}: Cr, {params:?}");
    }
}

/// The paired chroma filter against the reference on each plane: both
/// directions, strengths 0 to 4 per segment shared by the planes and
/// thresholds that differ between them, samples a filter works on as well
/// as noise, edges at the start and the end of their planes, planes of
/// different strides (declined to the single-plane kernel), and edges of
/// other lengths.
#[test]
fn the_chroma_pair_matches_the_reference_on_each_plane() {
    let Some(simd) = super::Simd::detect() else {
        return;
    };
    let mut rng = Rng(0x2c4a);
    let mut changed = 0;
    for case in 0..6000 {
        let vertical = case % 2 == 0;
        let lines = if case % 17 == 0 { 4 } else { 8 };
        let stride = if vertical {
            [4, 5, 8, 24]
        } else {
            [8, 9, 16, 24]
        }[rng.below(4)];
        let stride_cr = if case % 13 == 0 { stride + 1 } else { stride };
        let smooth = case % 3 != 0;
        let plane = |stride: usize, rng: &mut Rng| -> Vec<u8> {
            // The window's end, or a few rows past it.
            let end = if vertical {
                (lines - 1) * stride + 4
            } else {
                3 * stride + 8
            };
            let len = if case % 4 == 1 { end } else { end + 3 * stride };
            if smooth {
                // Near the ends of the range too, where Clip1 saturates.
                let base = [0, 2, 60, 120, 240][rng.below(5)] as u8;
                let step = [3, 9, 20][rng.below(3)] as u8;
                (0..len)
                    .map(|i| {
                        // The step between the two sides is at q0.
                        let side = if vertical {
                            i % stride >= 2
                        } else {
                            i / stride >= 2
                        };
                        base.saturating_add(if side { step } else { 0 })
                            .saturating_add(rng.below(4) as u8)
                    })
                    .collect()
            } else {
                rng.samples(len)
            }
        };
        let cb = plane(stride, &mut rng);
        let cr = plane(stride_cr, &mut rng);
        let mut bs = [0u8; 4];
        for b in bs.iter_mut() {
            *b = rng.below(5) as u8;
        }
        // The decoder gives both planes the same strengths; the trait
        // allows them to differ.
        let mut bs_cr = bs;
        if case % 7 == 0 {
            for b in bs_cr.iter_mut() {
                *b = rng.below(5) as u8;
            }
        }
        let params = [chroma_params(&mut rng, bs), chroma_params(&mut rng, bs_cr)];
        // q0 at column 2 of row 0 (vertical) or row 2 of column 0
        // (horizontal): the window starts at its plane's first sample.
        fn edge(data: &mut [u8], stride: usize, vertical: bool, lines: usize) -> EdgeMut<'_> {
            let (q0, across, along) = if vertical {
                (2, 1, stride)
            } else {
                (2 * stride, stride, 1)
            };
            EdgeMut::new(data, q0, across, along, lines, Reach::Chroma).expect("edge")
        }
        let (mut want_cb, mut want_cr) = (cb.clone(), cr.clone());
        Scalar.filter_edge(edge(&mut want_cb, stride, vertical, lines), &params[0]);
        Scalar.filter_edge(edge(&mut want_cr, stride_cr, vertical, lines), &params[1]);
        changed += usize::from(want_cb != cb) + usize::from(want_cr != cr);
        let (mut got_cb, mut got_cr) = (cb, cr);
        simd.filter_chroma_pair(
            edge(&mut got_cb, stride, vertical, lines),
            edge(&mut got_cr, stride_cr, vertical, lines),
            &params,
        );
        assert_eq!(
            got_cb, want_cb,
            "case {case}: Cb, vertical {vertical}, stride {stride}"
        );
        assert_eq!(
            got_cr, want_cr,
            "case {case}: Cr, vertical {vertical}, stride {stride_cr}"
        );
    }
    assert!(changed > 3000, "{changed} planes filtered");
}
