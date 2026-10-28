use coreav_kernels_h264::{Auto, BlockMut, Kernels, Scalar, Simd};
fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 7u32;
    let mut next = move || {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        x >> 8
    };
    let mut n = 0;
    for extra in [0usize, 1, 3, 40] {
        for off in 0..8usize {
            let stride = 8 + extra;
            let len = off + 7 * stride + 8; // plane ends at the block's last sample
            let mut back = vec![0u64; len / 8 + 2];
            let plane: &mut [u8] = unsafe {
                std::slice::from_raw_parts_mut(back.as_mut_ptr().cast::<u8>().add(off % 3), len)
            };
            for v in plane.iter_mut() {
                *v = (next() % 256) as u8;
            }
            let orig = plane.to_vec();
            let mut quad = [[0i32; 16]; 4];
            for c in quad.as_flattened_mut() {
                *c = (next() % 4096) as i32 - 2048;
            }
            for nz in [0xfu8, 0x5, 0x6, 0x9] {
                let mut want = orig.clone();
                for (k, b) in quad.iter().enumerate() {
                    if nz & (1 << k) != 0 {
                        let at = off + (k / 2) * 4 * stride + (k % 2) * 4;
                        Scalar.idct4x4_add(BlockMut::new(&mut want, at, stride, 4, 4).unwrap(), b);
                    }
                }
                assert_ne!(want, orig, "quad changed nothing");
                plane.copy_from_slice(&orig);
                simd.idct4x4_add_quad(BlockMut::new(plane, off, stride, 8, 8).unwrap(), &quad, nz);
                assert_eq!(&plane[..], &want[..]);
                plane.copy_from_slice(&orig);
                auto.idct4x4_add_quad(BlockMut::new(plane, off, stride, 8, 8).unwrap(), &quad, nz);
                assert_eq!(&plane[..], &want[..]);
                n += 1;
            }
            let mut c8 = [0i32; 64];
            for c in c8.iter_mut() {
                *c = (next() % 4096) as i32 - 2048;
            }
            let mut want = orig.clone();
            Scalar.idct8x8_add(BlockMut::new(&mut want, off, stride, 8, 8).unwrap(), &c8);
            assert_ne!(want, orig, "8x8 changed nothing");
            plane.copy_from_slice(&orig);
            simd.idct8x8_add(BlockMut::new(plane, off, stride, 8, 8).unwrap(), &c8);
            assert_eq!(&plane[..], &want[..]);
            plane.copy_from_slice(&orig);
            auto.idct8x8_add(BlockMut::new(plane, off, stride, 8, 8).unwrap(), &c8);
            assert_eq!(&plane[..], &want[..]);
            n += 1;
        }
    }
    println!("ok {n}");
}
