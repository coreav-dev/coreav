//! Miri harness for the AVX-512 tier's luma deblocking edges: each
//! standard shape at the narrowest steps a view takes and wider ones, at
//! eight offsets from alignment, the plane ending at the edge window's
//! last byte; the tier, AVX2 alone and `Auto` against the reference.
//! Run from a copy with KPATH set to the crate:
//! RUSTFLAGS="-C target-feature=+avx2,+avx512f,+avx512bw,+avx512vl,+avx512vbmi" cargo +nightly miri run -- rows
//! (`columns` for vertical edges, which Miri stops at their first byte
//! permute: it does not implement `vpermi2b`).
use coreav_kernels_h264::{Auto, EdgeMut, EdgeParams, Kernels, Reach, Scalar, Simd};
fn main() {
    let simd = Simd::detect().expect("avx2");
    assert!(simd.has_avx512(), "run with the tier's target features");
    let kernels: [(&str, &dyn Kernels); 3] =
        [("avx512", &simd), ("avx2", &simd.without_avx512()), ("auto", &Auto::detect())];
    let mut x = 7u32;
    let mut n = 0;
    let (r, lines) = (4usize, 16usize);
    let which = std::env::args().nth(1).unwrap_or_default();
    let directions: &[bool] = match which.as_str() {
        "rows" => &[false],
        "columns" => &[true],
        _ => &[false, true],
    };
    for &vertical in directions {
        for extra in [0usize, 1, 3, 40] {
            for off in 0..8usize {
                let narrow = if vertical { 2 * r } else { lines };
                let step = narrow + extra;
                let (across, along) = if vertical { (1, step) } else { (step, 1) };
                let q0 = r * across + off;
                let len = (lines - 1) * along + q0 + (r - 1) * across + 1;
                let mut back = vec![0u64; len / 8 + 2];
                // SAFETY: `back` holds at least `len + 2` bytes past the offset.
                let plane: &mut [u8] = unsafe {
                    std::slice::from_raw_parts_mut(back.as_mut_ptr().cast::<u8>().add(off % 3), len)
                };
                for v in plane.iter_mut() {
                    x = x.wrapping_mul(1103515245).wrapping_add(12345);
                    *v = (100 + (x >> 16) % 9) as u8;
                }
                for bs in [[2, 4, 1, 3], [4, 4, 4, 4], [1, 2, 3, 2]] {
                    let params = EdgeParams { alpha: 40, beta: 8, tc0: [2, 3, 1, 4], bs };
                    let orig = plane.to_vec();
                    let mut want = orig.clone();
                    Scalar.filter_edge(EdgeMut::new(&mut want, q0, across, along, lines, Reach::Luma).unwrap(), &params);
                    assert_ne!(want, orig, "filter changed nothing");
                    for (name, k) in kernels {
                        plane.copy_from_slice(&orig);
                        k.filter_edge(EdgeMut::new(plane, q0, across, along, lines, Reach::Luma).unwrap(), &params);
                        assert_eq!(&plane[..], &want[..], "{name}");
                    }
                    plane.copy_from_slice(&orig);
                    n += 1;
                }
            }
        }
    }
    println!("ok {n}");
}
