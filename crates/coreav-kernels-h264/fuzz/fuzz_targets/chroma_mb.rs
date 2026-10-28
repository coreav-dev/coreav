//! Bar C3 for the chroma of a macroblock in one call: the vector kernels
//! and the dispatch against the reference (the scalar kernels a component
//! at a time), for intra macroblocks (`intra_chroma_mb`: every combination
//! of neighbours, every mode and some out of range) and the residual of
//! inter ones (`chroma_residual_mb`), DC and AC coefficients over their
//! whole range, any mask, strides from the narrowest a view allows,
//! planes of different strides (for the residual; an intra view has one
//! geometry for both, so there Cr's plane is only shorter or longer), and
//! planes that end at or near the block's last sample.
#![no_main]

use coreav_kernels_h264::{Auto, BlockMut, ChromaIntraMut, IntraAvail, Kernels, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

/// A coefficient from two bytes, inside -2^15 to 2^15 - 1.
fn coefficient(bytes: &[u8], i: usize) -> i32 {
    let low = bytes.get(2 * i).copied().unwrap_or(0);
    let high = bytes.get(2 * i + 1).copied().unwrap_or(0);
    i32::from(i16::from_le_bytes([low, high]))
}

/// The bytes before the samples: seven of shape, then the DC and AC
/// coefficients, two bytes each.
const HEAD: usize = 7 + 2 * (8 + 128);

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(HEAD) else {
        return;
    };
    let intra = head[0] & 1 != 0;
    let mode = head[0] >> 1;
    let bits = head[1];
    let avail = |bits: u8| IntraAvail {
        left: bits & 1 != 0,
        top: bits & 2 != 0,
        top_right: bits & 4 != 0,
        top_left: bits & 8 != 0,
    };
    let a_cb = avail(bits);
    let stride = 8 + usize::from(head[2] % 32);
    let stride_cr = if head[3] % 5 == 0 {
        8 + usize::from(head[3] / 5 % 32)
    } else {
        stride
    };
    // The block's first sample: from the first neighbour the view names
    // (the plane's first sample for the residual) to a little further in.
    let before =
        |stride: usize, a: IntraAvail| match (intra, a.top_left, a.top || a.top_right, a.left) {
            (false, ..) => 0,
            (true, true, ..) => stride + 1,
            (true, false, true, _) => stride,
            (true, false, false, true) => 1,
            _ => 0,
        };
    let at = |stride: usize| {
        before(stride, a_cb) + usize::from(head[4]) % (stride - 7)
    };
    let len = |stride: usize| at(stride) + 7 * stride + 8 + usize::from(head[5] % 4);
    let ac_nz = head[6];
    let coeffs = &head[7..];
    let dc: [[i32; 4]; 2] =
        core::array::from_fn(|c| core::array::from_fn(|k| coefficient(coeffs, 4 * c + k)));
    let ac: [[[i32; 16]; 4]; 2] = core::array::from_fn(|c| {
        core::array::from_fn(|k| {
            core::array::from_fn(|i| coefficient(coeffs, 8 + 64 * c + 16 * k + i))
        })
    });
    let (n_cb, n_cr) = (len(stride), len(stride_cr));
    if samples.len() < n_cb + n_cr {
        return;
    }
    let cb = samples.get(..n_cb).unwrap_or_default().to_vec();
    let cr = samples.get(n_cb..n_cb + n_cr).unwrap_or_default().to_vec();
    let run = |kernels: &dyn Kernels| -> Option<(Vec<u8>, Vec<u8>)> {
        let (mut b, mut r) = (cb.clone(), cr.clone());
        if intra {
            let v = ChromaIntraMut::new(&mut b, &mut r, at(stride), stride, a_cb).ok()?;
            kernels.intra_chroma_mb(v, mode, &dc, &ac, ac_nz);
        } else {
            let vb = BlockMut::new(&mut b, at(stride), stride, 8, 8).ok()?;
            let vr = BlockMut::new(&mut r, at(stride_cr), stride_cr, 8, 8).ok()?;
            kernels.chroma_residual_mb(vb, vr, &dc, &ac, ac_nz);
        }
        Some((b, r))
    };
    let Some(want) = run(&Scalar) else { return };
    for (name, kernels) in [("simd", &simd as &dyn Kernels), ("auto", &Auto::detect())] {
        let got = run(kernels);
        assert_eq!(
            got.as_ref(),
            Some(&want),
            "{name}: intra {intra}, mode {mode}, strides {stride}, {stride_cr}"
        );
    }
});
