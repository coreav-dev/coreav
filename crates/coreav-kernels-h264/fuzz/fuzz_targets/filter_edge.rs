//! Bar C3 for the deblocking filter, over every edge the view allows and
//! the thresholds Tables 8-16 and 8-17 give: the vector kernels (the
//! AVX-512 tier where the CPU has it, and AVX2 alone) and the dispatch
//! against the reference, from the narrowest steps a view takes
//! and at every alignment of the edge's rows.
#![no_main]

use coreav_kernels_h264::{Auto, EdgeMut, EdgeParams, Kernels, Reach, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(12) else {
        return;
    };
    let chroma = head[0] % 2 == 0;
    let reach = if chroma { Reach::Chroma } else { Reach::Luma };
    let lines = (usize::from(head[1] % 6) + 1) * 4;
    let vertical = head[2] % 2 == 0;
    // The narrowest step between lines a view takes (twice the samples on
    // each side for a vertical edge, the line count for a horizontal one),
    // and up to fifteen more.
    let r = if chroma { 2 } else { 4 };
    let narrowest = if vertical { 2 * r } else { lines };
    let stride = narrowest + usize::from(head[3] % 16);
    let mut params = EdgeParams {
        alpha: head[4],
        beta: head[5] % 19,
        tc0: [0; 8],
        bs: [0; 4],
    };
    for (i, bs) in params.bs.iter_mut().enumerate() {
        *bs = head[6 + i] % 5;
    }
    for (b, tc0) in params.tc0.iter_mut().enumerate() {
        *tc0 = head[6 + b % 6] / 8;
    }
    let rows = lines + 16;
    if samples.len() < stride * rows {
        return;
    }
    let plane = samples.get(..stride * rows).unwrap_or_default().to_vec();
    // The first q0 sample, far enough inside the plane for both
    // directions, at any of eight alignments.
    let q0 = 8 * stride + 8 + usize::from(head[11] % 8);
    let (across, along) = if vertical { (1, stride) } else { (stride, 1) };
    let mut want = plane.clone();
    let Ok(edge) = EdgeMut::new(&mut want, q0, across, along, lines, reach) else {
        return;
    };
    Scalar.filter_edge(edge, &params);
    let avx2 = simd.without_avx512();
    for (name, kernels) in [
        ("simd", &simd as &dyn Kernels),
        ("simd without AVX-512", &avx2),
        ("auto", &Auto::detect()),
    ] {
        let mut got = plane.clone();
        let Ok(edge) = EdgeMut::new(&mut got, q0, across, along, lines, reach) else {
            return;
        };
        kernels.filter_edge(edge, &params);
        assert_eq!(
            got, want,
            "{name}: {lines} lines, chroma {chroma}, vertical {vertical}"
        );
    }
});
