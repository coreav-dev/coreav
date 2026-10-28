// The Miri harness of the paired chroma filter's pointer accesses: both
// directions, through `Simd` and `Auto` against the reference on each
// plane, in planes that start at the edge's first sample and end at its
// last, from misaligned bases, at the narrowest strides a view takes and
// wider ones.
use coreav_kernels_h264::{Auto, EdgeMut, EdgeParams, Kernels, Reach, Scalar, Simd};

fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 11u32;
    let mut next = move || {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        x >> 8
    };
    let mut cases = 0;
    for vertical in [true, false] {
        for extra in [0usize, 1, 3, 9] {
            let stride = if vertical { 4 } else { 8 } + extra;
            let end = if vertical { 7 * stride + 4 } else { 3 * stride + 8 };
            for skew in 0..3usize {
                let mut back_cb = vec![0u64; end / 8 + 2];
                let mut back_cr = vec![0u64; end / 8 + 2];
                // SAFETY (test only): each backing holds `end + skew` bytes
                // and outlives its slice.
                let (cb, cr): (&mut [u8], &mut [u8]) = unsafe {
                    (
                        std::slice::from_raw_parts_mut(back_cb.as_mut_ptr().cast::<u8>().add(skew), end),
                        std::slice::from_raw_parts_mut(back_cr.as_mut_ptr().cast::<u8>().add(skew), end),
                    )
                };
                // Two flat sides a small step apart, so that lines filter.
                for (i, v) in cb.iter_mut().chain(cr.iter_mut()).enumerate() {
                    let side = if vertical { (i % end) % stride >= 2 } else { (i % end) / stride >= 2 };
                    *v = 100 + if side { 8 } else { 0 } + (next() % 3) as u8;
                }
                let (orig_cb, orig_cr) = (cb.to_vec(), cr.to_vec());
                for bs in [[1u8, 2, 3, 4], [4, 4, 4, 4], [0, 2, 0, 1]] {
                    let p = |alpha: i32| EdgeParams { alpha, beta: 18, tc0: [1, 4, 9, 25], bs };
                    let params = [p(40), p(if bs[0] == 0 { 0 } else { 30 })];
                    let (q0, across, along) = if vertical { (2, 1, stride) } else { (2 * stride, stride, 1) };
                    let mut want_cb = orig_cb.clone();
                    let mut want_cr = orig_cr.clone();
                    Scalar.filter_edge(EdgeMut::new(&mut want_cb, q0, across, along, 8, Reach::Chroma).unwrap(), &params[0]);
                    Scalar.filter_edge(EdgeMut::new(&mut want_cr, q0, across, along, 8, Reach::Chroma).unwrap(), &params[1]);
                    assert!(want_cb != orig_cb, "nothing filtered");
                    for k in [&simd as &dyn Kernels, &auto] {
                        cb.copy_from_slice(&orig_cb);
                        cr.copy_from_slice(&orig_cr);
                        k.filter_chroma_pair(
                            EdgeMut::new(cb, q0, across, along, 8, Reach::Chroma).unwrap(),
                            EdgeMut::new(cr, q0, across, along, 8, Reach::Chroma).unwrap(),
                            &params,
                        );
                        assert_eq!(&cb[..], &want_cb[..]);
                        assert_eq!(&cr[..], &want_cr[..]);
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("ok {cases}");
}
