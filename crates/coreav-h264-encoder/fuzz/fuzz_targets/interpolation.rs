#![no_main]
#![deny(unsafe_code)]

use coreav_h264_encoder::{Error, Result};
use coreav_kernels_h264::{Block, BlockMut, Kernels, Scalar};
use libfuzzer_sys::fuzz_target;

#[path = "../../src/kernels.rs"]
mod kernels;
#[cfg(all(target_arch = "x86_64", not(feature = "scalar")))]
#[path = "../../src/simd.rs"]
mod simd;

fuzz_target!(|data: &[u8]| {
    let byte = |i| data.get(i).copied().unwrap_or(0);
    let chroma = byte(0) & 1 != 0;
    let w = 1 + usize::from(byte(1) % 18);
    let h = 1 + usize::from(byte(2) % 18);
    let extra = if chroma { 1 } else { 5 };
    let offset = usize::from(byte(3) % 32);
    let stride = w + extra + usize::from(byte(4) % 8);
    let out_stride = w + usize::from(byte(5) % 8);
    let mut source = vec![0; offset + (h + extra - 1) * stride + w + extra];
    for (i, v) in source.iter_mut().enumerate() {
        *v = byte(8 + i);
    }
    let mut actual = vec![137; offset + (h - 1) * out_stride + w];
    let mut expected = actual.clone();
    let src = Block::new(&source, offset, stride, w + extra, h + extra).unwrap();
    let dst = BlockMut::new(&mut actual, offset, out_stride, w, h).unwrap();
    let scalar = BlockMut::new(&mut expected, offset, out_stride, w, h).unwrap();
    if chroma {
        kernels::chroma_mc(dst, src, byte(6), byte(7));
        Scalar.chroma_mc(scalar, src, byte(6), byte(7));
    } else {
        kernels::pixels().luma_mc(dst, src, byte(6), byte(7));
        Scalar.luma_mc(scalar, src, byte(6), byte(7));
    }
    assert_eq!(actual, expected);
});
