//! Bar C3 for chroma interpolation, and its averaging form.
#![no_main]

use coreav_kernels_h264::{Auto, Block, BlockMut, Kernels, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(5) else {
        return;
    };
    let w = match head[0] % 3 {
        0 => 2,
        1 => 4,
        _ => 8,
    };
    let h = usize::from(head[1] % 8) + 1;
    let (xf, yf) = (head[2] & 7, head[3] & 7);
    let src_stride = w + 1 + usize::from(head[4] % 8);
    let dst_stride = w + usize::from(head[4] / 8);
    if samples.len() < src_stride * h + w + 1 {
        return;
    }
    let source = match Block::new(samples, 0, src_stride, w + 1, h + 1) {
        Ok(block) => block,
        Err(_) => return,
    };
    let plane = vec![0x80u8; dst_stride * h + 16];
    let mut want = plane.clone();
    let mut got = plane;
    let Ok(block) = BlockMut::new(&mut want, 0, dst_stride, w, h) else {
        return;
    };
    Scalar.chroma_mc(block, source, xf, yf);
    let Ok(block) = BlockMut::new(&mut got, 0, dst_stride, w, h) else {
        return;
    };
    simd.chroma_mc(block, source, xf, yf);
    assert_eq!(got, want, "{w}x{h} at ({xf}, {yf})");

    // The averaging form: the prediction averaged into a block that
    // already holds samples, against the trait's own two-step form.
    let base: Vec<u8> = samples
        .iter()
        .rev()
        .cycle()
        .take(dst_stride * h + 16)
        .copied()
        .collect();
    let mut want = base.clone();
    let Ok(block) = BlockMut::new(&mut want, 0, dst_stride, w, h) else {
        return;
    };
    Scalar.chroma_mc_avg(block, source, xf, yf);
    for (name, kernels) in [("simd", &simd as &dyn Kernels), ("auto", &Auto::detect())] {
        let mut got = base.clone();
        let Ok(block) = BlockMut::new(&mut got, 0, dst_stride, w, h) else {
            return;
        };
        kernels.chroma_mc_avg(block, source, xf, yf);
        assert_eq!(got, want, "{name} chroma_mc_avg {w}x{h} at ({xf}, {yf})");
    }
});
