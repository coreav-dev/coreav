//! Bar C3 for the weighted interpolation, luma and chroma: the vector
//! kernels and the dispatch give the same samples as the trait's two-step
//! form through the reference, for every shape, position, stride and
//! weighting, out-of-range weightings included (all leave the block
//! alone).
#![no_main]

use coreav_kernels_h264::{Auto, Block, BlockMut, Kernels, Scalar, Simd, Weighting};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(10) else {
        return;
    };
    let luma = head[0] & 1 == 0;
    let (w, h, extra) = if luma {
        (
            [4, 8, 16][usize::from(head[0] >> 1) % 3],
            usize::from(head[1] % 16) + 1,
            5,
        )
    } else {
        (
            [2, 4, 8][usize::from(head[0] >> 1) % 3],
            usize::from(head[1] % 8) + 1,
            1,
        )
    };
    let (xf, yf) = (head[2], head[3]);
    let src_stride = w + extra + usize::from(head[4] % 8);
    let dst_stride = w + usize::from(head[4] / 8);
    // Weights and offsets over a little more than their ranges, so that
    // the ends and the rejections are both reached.
    let value = |b: u8| i32::from(b as i8) * 9 / 8;
    let log_wd = i32::from(head[5] % 10) - 1;
    let weighting = if head[5] & 0x80 == 0 {
        Weighting::Uni {
            log_wd,
            w: value(head[6]),
            o: value(head[8]),
        }
    } else {
        Weighting::Bi {
            log_wd,
            w0: value(head[6]),
            w1: value(head[7]),
            o: value(head[8]),
        }
    };
    if samples.len() < src_stride * (h + extra - 1) + w + extra {
        return;
    }
    let Ok(source) = Block::new(samples, 0, src_stride, w + extra, h + extra) else {
        return;
    };
    let base: Vec<u8> = samples
        .iter()
        .rev()
        .cycle()
        .skip(usize::from(head[9]))
        .take(dst_stride * h + 16)
        .copied()
        .collect();
    let run = |kernels: &dyn Kernels| -> Option<Vec<u8>> {
        let mut out = base.clone();
        let block = BlockMut::new(&mut out, 0, dst_stride, w, h).ok()?;
        if luma {
            kernels.luma_mc_weighted(block, source, xf, yf, weighting);
        } else {
            kernels.chroma_mc_weighted(block, source, xf, yf, weighting);
        }
        Some(out)
    };
    let Some(want) = run(&Scalar) else { return };
    if !weighting.in_range() {
        assert_eq!(want, base, "an out-of-range weighting wrote");
    }
    for (name, kernels) in [("simd", &simd as &dyn Kernels), ("auto", &Auto::detect())] {
        assert_eq!(
            run(kernels),
            Some(want.clone()),
            "{name} luma {luma} {w}x{h} at ({xf}, {yf}), {weighting:?}"
        );
    }
});
