//! Bar C3 for prediction combining, over the weights clause 7.4.3.2
//! allows.
#![no_main]

use coreav_kernels_h264::{Block, BlockMut, Combine, Kernels, Scalar, Simd};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(8) else {
        return;
    };
    let w = usize::from(head[0] % 16) + 1;
    let h = usize::from(head[1] % 16) + 1;
    let stride = w + usize::from(head[2] % 8);
    let weight = |b: u8| i32::from(b as i8);
    let log_wd = i32::from(head[3] % 8);
    let mode = match head[4] % 4 {
        0 => Combine::Copy(usize::from(head[5] % 2)),
        1 => Combine::Average,
        2 => Combine::Uni {
            list: usize::from(head[5] % 2),
            log_wd,
            w: weight(head[6]),
            o: weight(head[7]),
        },
        _ => Combine::Bi {
            log_wd,
            w0: weight(head[5]),
            w1: weight(head[6]),
            o: weight(head[7]),
        },
    };
    if samples.len() < 2 * (16 * h + w) {
        return;
    }
    let (first, second) = samples.split_at(samples.len() / 2);
    let (Ok(p0), Ok(p1)) = (
        Block::new(first, 0, 16, w, h),
        Block::new(second, 0, 16, w, h),
    ) else {
        return;
    };
    let plane = vec![0x80u8; stride * h + 16];
    let mut want = plane.clone();
    let mut got = plane;
    let Ok(block) = BlockMut::new(&mut want, 0, stride, w, h) else {
        return;
    };
    Scalar.combine(block, p0, p1, mode);
    let Ok(block) = BlockMut::new(&mut got, 0, stride, w, h) else {
        return;
    };
    simd.combine(block, p0, p1, mode);
    assert_eq!(got, want, "{w}x{h} {mode:?}");
});
