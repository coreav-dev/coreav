// The Miri harness of the chroma macroblock kernel's pointer accesses:
// every edge, then all four, then both vertical and both horizontal edges,
// through `Simd` and `Auto` against the reference (`Scalar`, an edge at a
// time), in planes that start at the window's first sample and end at its
// last, from misaligned bases, at the narrowest stride a window takes and
// wider ones.
use coreav_kernels_h264::{Auto, ChromaMbMut, EdgeParams, Kernels, Scalar, Simd};

fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 11u32;
    let mut next = move || {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        x >> 8
    };
    let mut cases = 0;
    for stride in [10usize, 11, 16, 19] {
        let end = 9 * stride + 10;
        let at = 2 * stride + 2;
        for skew in 0..3usize {
            let mut back_cb = vec![0u64; end / 8 + 2];
            let mut back_cr = vec![0u64; end / 8 + 2];
            // SAFETY (test only): each backing holds `end + skew` bytes and
            // outlives its slice.
            let (cb, cr): (&mut [u8], &mut [u8]) = unsafe {
                (
                    std::slice::from_raw_parts_mut(back_cb.as_mut_ptr().cast::<u8>().add(skew), end),
                    std::slice::from_raw_parts_mut(back_cr.as_mut_ptr().cast::<u8>().add(skew), end),
                )
            };
            // A step every two rows and columns, so that every edge filters.
            for (i, v) in cb.iter_mut().chain(cr.iter_mut()).enumerate() {
                let (row, col) = ((i % end) / stride, (i % end) % stride);
                let side = (row % 4 >= 2) ^ (col % 4 >= 2);
                *v = 100 + if side { 8 } else { 0 } + (next() % 3) as u8;
            }
            let (orig_cb, orig_cr) = (cb.to_vec(), cr.to_vec());
            for mask in [1usize, 2, 4, 8, 15, 3, 12] {
                let p = |alpha: i32, bs: [u8; 4]| EdgeParams { alpha, beta: 18, tc0: [1, 4, 9, 25], bs };
                let mut table = [[EdgeParams::default(); 2]; 4];
                for (slot, pair) in table.iter_mut().enumerate() {
                    if mask >> slot & 1 == 1 {
                        let bs = if slot % 2 == 0 { [4, 4, 4, 4] } else { [1, 2, 3, 0] };
                        *pair = [p(40, bs), p(if slot == 1 { 0 } else { 30 }, bs)];
                    }
                }
                let mut want_cb = orig_cb.clone();
                let mut want_cr = orig_cr.clone();
                Scalar.filter_chroma_mb(
                    ChromaMbMut::new(&mut want_cb, at, stride).unwrap(),
                    ChromaMbMut::new(&mut want_cr, at, stride).unwrap(),
                    &table,
                );
                assert!(want_cb != orig_cb, "nothing filtered");
                for k in [&simd as &dyn Kernels, &auto] {
                    cb.copy_from_slice(&orig_cb);
                    cr.copy_from_slice(&orig_cr);
                    k.filter_chroma_mb(
                        ChromaMbMut::new(cb, at, stride).unwrap(),
                        ChromaMbMut::new(cr, at, stride).unwrap(),
                        &table,
                    );
                    assert_eq!(&cb[..], &want_cb[..]);
                    assert_eq!(&cr[..], &want_cr[..]);
                    cases += 1;
                }
            }
        }
    }
    println!("ok {cases}");
}
