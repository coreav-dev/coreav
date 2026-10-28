use coreav_kernels_h264::{Auto, EdgeMut, EdgeParams, Kernels, Reach, Scalar, Simd};
fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 7u32;
    let mut n = 0;
    for (reach, r, lines) in [(Reach::Luma, 4usize, 16usize), (Reach::Chroma, 2, 8)] {
        for vertical in [false, true] {
            for extra in [0usize, 1, 3, 40] {
                for off in 0..8usize {
                    let narrow = if vertical { 2 * r } else { lines };
                    let step = narrow + extra;
                    let (across, along) = if vertical { (1, step) } else { (step, 1) };
                    let q0 = r * across + off;
                    let len = (lines - 1) * along + q0 + (r - 1) * across + 1; // plane ends at window end
                    let mut back = vec![0u64; len / 8 + 2];
                    let plane: &mut [u8] = unsafe {
                        std::slice::from_raw_parts_mut(back.as_mut_ptr().cast::<u8>().add(off % 3), len)
                    };
                    for v in plane.iter_mut() {
                        x = x.wrapping_mul(1103515245).wrapping_add(12345);
                        *v = (100 + (x >> 16) % 9) as u8;
                    }
                    let params = EdgeParams { alpha: 40, beta: 8, tc0: [2, 3, 1, 4], bs: [2, 4, 1, 3] };
                    let orig = plane.to_vec();
                    let mut want = orig.clone();
                    Scalar.filter_edge(EdgeMut::new(&mut want, q0, across, along, lines, reach).unwrap(), &params);
                    assert_ne!(want, orig, "filter changed nothing");
                    simd.filter_edge(EdgeMut::new(plane, q0, across, along, lines, reach).unwrap(), &params);
                    assert_eq!(&plane[..], &want[..]);
                    plane.copy_from_slice(&orig);
                    auto.filter_edge(EdgeMut::new(plane, q0, across, along, lines, reach).unwrap(), &params);
                    assert_eq!(&plane[..], &want[..]);
                    n += 1;
                }
            }
        }
    }
    println!("ok {n}");
}
