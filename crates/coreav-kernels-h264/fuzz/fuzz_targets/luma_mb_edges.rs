//! The edges of a luma window (`LumaMbMut::vertical`, `horizontal`), which
//! carry the whole window as their slice, against the same edges checked
//! one at a time (`EdgeMut::new`), through the vector kernels (the AVX-512
//! tier where the CPU has it, and AVX2 alone) and the reference, at every
//! stride from the narrowest the window takes, for each of the eight edges
//! in turn. Both must give the reference's samples, and nothing outside
//! the edge's lines may change.
#![no_main]

use coreav_kernels_h264::{EdgeMut, EdgeParams, Kernels, LumaMbMut, Reach, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(16) else {
        return;
    };
    let stride = 20 + usize::from(head[0] % 24);
    let vertical = head[1] % 2 == 0;
    let e = usize::from(head[2] % 4);
    let mut params = EdgeParams {
        alpha: head[3],
        beta: head[4],
        tc0: [0; 8],
        bs: [0; 4],
    };
    for (i, bs) in params.bs.iter_mut().enumerate() {
        *bs = head[5 + i];
    }
    for (b, tc0) in params.tc0.iter_mut().enumerate() {
        *tc0 = head[8 + b];
    }
    // The block's first sample, with up to seven samples of slack before
    // the window, and the plane ending with the window or a little later.
    let at = 4 * stride + 4 + usize::from(head[15] % 8);
    let len = at + 15 * stride + 16 + usize::from(head[15] / 8 % 4);
    if samples.len() < len {
        return;
    }
    let plane = samples.get(..len).unwrap_or_default().to_vec();
    let (q0, across, along) = if vertical {
        (at + 4 * e, 1, stride)
    } else {
        (at + 4 * e * stride, stride, 1)
    };
    let mut want = plane.clone();
    let Ok(edge) = EdgeMut::new(&mut want, q0, across, along, 16, Reach::Luma) else {
        return;
    };
    Scalar.filter_edge(edge, &params);
    let avx2 = simd.without_avx512();
    for (name, kernels) in [
        ("scalar", &Scalar as &dyn Kernels),
        ("simd", &simd),
        ("simd without AVX-512", &avx2),
    ] {
        let mut got = plane.clone();
        let Ok(mut window) = LumaMbMut::new(&mut got, at, stride) else {
            return;
        };
        let edge = if vertical {
            window.vertical(e)
        } else {
            window.horizontal(e)
        };
        let Some(edge) = edge else { return };
        kernels.filter_edge(edge, &params);
        assert_eq!(
            got, want,
            "{name}: stride {stride}, vertical {vertical}, edge {e}"
        );
    }
});
