//! Bar C3 for the chroma edges of a macroblock in one call: the vector
//! kernel and the dispatch against the reference (the scalar kernels an
//! edge at a time), for any subset of the four edges, strengths 0 to 4 per
//! segment, thresholds that differ between the planes (α 0 included),
//! strides from the narrowest a window takes, planes of different strides,
//! and windows that start at their plane's first sample and end at or
//! near its last.
#![no_main]

use coreav_kernels_h264::{Auto, ChromaMbMut, EdgeParams, Kernels, Scalar, Simd};
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

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(44) else {
        return;
    };
    let stride = 10 + usize::from(head[0] % 32);
    let stride_cr = if head[1] % 5 == 0 {
        10 + usize::from(head[1] / 5 % 32)
    } else {
        stride
    };
    // The block's first sample, two rows and two columns in at least.
    let at = |stride: usize| 2 * stride + 2 + usize::from(head[2] % 4);
    let len = |stride: usize| at(stride) + 7 * stride + 8 + usize::from(head[3] % 4);
    let mut table = [[EdgeParams::default(); 2]; 4];
    for (slot, pair) in table.iter_mut().enumerate() {
        let b = &head[4 + 10 * slot..14 + 10 * slot];
        if b[0] % 3 == 0 {
            continue;
        }
        let bs = [b[1] % 5, b[1] / 5 % 5, b[1] / 25 % 5, b[0] / 3 % 5];
        let bs_cr = if b[0] % 7 == 1 {
            bs.map(|v| (v + 1) % 5)
        } else {
            bs
        };
        *pair = [params(&b[2..6], bs), params(&b[6..10], bs_cr)];
    }
    let (n_cb, n_cr) = (len(stride), len(stride_cr));
    if samples.len() < n_cb + n_cr {
        return;
    }
    let cb = samples.get(..n_cb).unwrap_or_default().to_vec();
    let cr = samples.get(n_cb..n_cb + n_cr).unwrap_or_default().to_vec();
    let (mut want_cb, mut want_cr) = (cb.clone(), cr.clone());
    let (Ok(a), Ok(b)) = (
        ChromaMbMut::new(&mut want_cb, at(stride), stride),
        ChromaMbMut::new(&mut want_cr, at(stride_cr), stride_cr),
    ) else {
        return;
    };
    Scalar.filter_chroma_mb(a, b, &table);
    for (name, kernels) in [("simd", &simd as &dyn Kernels), ("auto", &Auto::detect())] {
        let (mut got_cb, mut got_cr) = (cb.clone(), cr.clone());
        let (Ok(a), Ok(b)) = (
            ChromaMbMut::new(&mut got_cb, at(stride), stride),
            ChromaMbMut::new(&mut got_cr, at(stride_cr), stride_cr),
        ) else {
            return;
        };
        kernels.filter_chroma_mb(a, b, &table);
        assert_eq!(got_cb, want_cb, "{name}: Cb, stride {stride}");
        assert_eq!(got_cr, want_cr, "{name}: Cr, stride {stride_cr}");
    }
});
