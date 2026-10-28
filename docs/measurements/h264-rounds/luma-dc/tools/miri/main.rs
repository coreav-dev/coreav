// The Miri harness of the luma residual kernels' pointer accesses:
// `luma4x4_residual_mb` and `luma8x8_residual_mb` through `Simd` and `Auto`
// against the reference, in planes that start at the macroblock's first
// sample and end at its last, from a misaligned base, at the narrowest
// stride and wider ones, for blocks with coefficients, with a DC alone, with
// a DC and one other coefficient, and with none, under masks that mark
// blocks as having their DC alone or not.
use coreav_kernels_h264::{Auto, BlockMut, Kernels, Scalar, Simd};

/// A plane of `len` bytes starting `skew` bytes into `back`.
fn plane(back: &mut Vec<u64>, len: usize, skew: usize) -> &mut [u8] {
    back.resize(len / 8 + 2, 0);
    // SAFETY (test only): `back` holds at least `len + skew` bytes and
    // outlives the slice.
    unsafe { std::slice::from_raw_parts_mut(back.as_mut_ptr().cast::<u8>().add(skew), len) }
}

fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 7u32;
    let mut next = move || {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        x >> 8
    };
    let mut cases = 0;
    let mut back = Vec::new();
    for stride in [16usize, 17, 40] {
        for skew in 0..3 {
            let len = 15 * stride + 16;
            let p = plane(&mut back, len, skew);
            for v in p.iter_mut() {
                *v = (next() % 256) as u8;
            }
            let original = p.to_vec();
            for round in 0..8u32 {
                // Block kinds by round: coefficients, a DC alone, a DC and
                // one other, none.
                let mut kind = |n: usize| -> Vec<i32> {
                    let k = (next() + round) % 4;
                    let one = 1 + (next() as usize) % (n - 1);
                    (0..n)
                        .map(|i| match k {
                            0 => (next() % 8192) as i32 - 4096,
                            1 if i == 0 => (next() % 8192) as i32 - 4096,
                            2 if i == 0 || i == one => (next() % 8192) as i32 - 4096,
                            _ => 0,
                        })
                        .collect()
                };
                let c4: [[i32; 16]; 16] = core::array::from_fn(|_| kind(16).try_into().unwrap());
                let c8: [[i32; 64]; 4] = core::array::from_fn(|_| kind(64).try_into().unwrap());
                let nz = match round % 3 {
                    0 => 0xffff,
                    _ => (next() & 0xffff) as u16,
                };
                let dc = match round % 4 {
                    0 => 0,
                    1 => 0xffff,
                    _ => (next() & 0xffff) as u16,
                };
                let four = |k: &dyn Kernels, p: &mut [u8]| {
                    k.luma4x4_residual_mb(BlockMut::new(p, 0, stride, 16, 16).unwrap(), &c4, nz, dc);
                };
                let eight = |k: &dyn Kernels, p: &mut [u8]| {
                    let (nz, dc) = (nz as u8 & 0xf, (dc >> 4) as u8 & 0xf);
                    k.luma8x8_residual_mb(BlockMut::new(p, 0, stride, 16, 16).unwrap(), &c8, nz, dc);
                };
                for (what, run) in [("4x4", &four as &dyn Fn(&dyn Kernels, &mut [u8])), ("8x8", &eight)] {
                    let mut want = original.clone();
                    run(&Scalar, &mut want);
                    for (name, k) in [("simd", &simd as &dyn Kernels), ("auto", &auto)] {
                        p.copy_from_slice(&original);
                        run(k, p);
                        assert_eq!(&p[..], &want[..], "{what} {name} stride {stride} skew {skew} round {round}");
                    }
                    cases += 1;
                }
            }
        }
    }
    println!("ok {cases}");
}
