//! Bar C3 for the paired chroma filter: the vector kernel and the dispatch
//! against the reference on each plane, for both directions, thresholds
//! that differ between the planes (α 0 in one of them included), strides
//! from the narrowest a view takes, planes of different strides, and edges
//! whose window starts at its plane's first sample.
#![no_main]

use coreav_kernels_h264::{Auto, EdgeMut, EdgeParams, Kernels, Reach, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

/// Thresholds from four bytes, in the ranges of Tables 8-16 and 8-17.
fn params(b: &[u8], bs: [u8; 4]) -> EdgeParams {
    let at = |i: usize| b.get(i).copied().unwrap_or(0);
    EdgeParams {
        alpha: at(0),
        beta: at(1) % 19,
        tc0: [
            at(2) % 26,
            at(3) % 26,
            at(2) / 26 % 26,
            at(3) / 26 % 26,
            at(2) % 7,
            at(3) % 7,
            0,
            0,
        ],
        bs,
    }
}

fn edge(data: &mut [u8], stride: usize, vertical: bool, lines: usize) -> Option<EdgeMut<'_>> {
    let (q0, across, along) = if vertical {
        (2, 1, stride)
    } else {
        (2 * stride, stride, 1)
    };
    EdgeMut::new(data, q0, across, along, lines, Reach::Chroma).ok()
}

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(16) else {
        return;
    };
    let vertical = head[0] % 2 == 0;
    let lines = if head[0] % 7 == 1 { 4 } else { 8 };
    let narrowest = if vertical { 4 } else { lines };
    let stride = narrowest + usize::from(head[1] % 16);
    let stride_cr = if head[0] % 5 == 0 { stride + 1 } else { stride };
    let bs = [head[2] % 5, head[3] % 5, head[4] % 5, head[5] % 5];
    // The decoder gives both planes the same strengths; the trait allows
    // them to differ.
    let bs_cr = if head[15] % 4 == 0 {
        bs.map(|b| (b + head[15] / 4) % 5)
    } else {
        bs
    };
    let params = [params(&head[6..10], bs), params(&head[10..14], bs_cr)];
    // Each plane ends at its window's last sample, or a little past it.
    let len = |stride: usize| {
        let end = if vertical {
            (lines - 1) * stride + 4
        } else {
            3 * stride + lines
        };
        end + usize::from(head[14] % 4)
    };
    let (n_cb, n_cr) = (len(stride), len(stride_cr));
    if samples.len() < n_cb + n_cr {
        return;
    }
    let cb = samples.get(..n_cb).unwrap_or_default().to_vec();
    let cr = samples.get(n_cb..n_cb + n_cr).unwrap_or_default().to_vec();
    let (mut want_cb, mut want_cr) = (cb.clone(), cr.clone());
    let (Some(a), Some(b)) = (
        edge(&mut want_cb, stride, vertical, lines),
        edge(&mut want_cr, stride_cr, vertical, lines),
    ) else {
        return;
    };
    Scalar.filter_edge(a, &params[0]);
    Scalar.filter_edge(b, &params[1]);
    for (name, kernels) in [("simd", &simd as &dyn Kernels), ("auto", &Auto::detect())] {
        let (mut got_cb, mut got_cr) = (cb.clone(), cr.clone());
        let (Some(a), Some(b)) = (
            edge(&mut got_cb, stride, vertical, lines),
            edge(&mut got_cr, stride_cr, vertical, lines),
        ) else {
            return;
        };
        kernels.filter_chroma_pair(a, b, &params);
        assert_eq!(
            got_cb, want_cb,
            "{name}: Cb, vertical {vertical}, {lines} lines"
        );
        assert_eq!(
            got_cr, want_cr,
            "{name}: Cr, vertical {vertical}, {lines} lines"
        );
    }
});
