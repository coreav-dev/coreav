// The Miri harness of the Intra_8x8 kernel's pointer accesses: the
// macroblock call through `Simd` and `Auto` against the reference, in
// planes that start at the first neighbour the view names and end at the
// macroblock's last sample, from a misaligned base, at the strides around
// the ones that go to the reference (below 17, below 24 with a top right)
// and wider ones, for every combination of neighbours,
// every mode in each block position, and residuals on all, some or none
// of the blocks.
use coreav_kernels_h264::{Auto, IntraAvail, IntraMut, Kernels, Scalar, Simd};

fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 7u32;
    let mut next = move || {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        x >> 8
    };
    let mut cases = 0;
    let n = 16;
    for bits in 0..16u8 {
        let avail = IntraAvail {
            left: bits & 1 != 0,
            top: bits & 2 != 0,
            top_right: bits & 4 != 0,
            top_left: bits & 8 != 0,
        };
        for stride in [16usize, 17, 19, 20, 23, 24, 25, 40] {
            let before = if avail.top_left {
                stride + 1
            } else if avail.top || avail.top_right {
                stride
            } else if avail.left {
                1
            } else {
                0
            };
            let len = before + (n - 1) * stride + n;
            let mut back = vec![0u64; len / 8 + 2];
            let skew = usize::from(bits % 3) + stride % 2;
            // SAFETY (test only): `back` holds `len + skew` bytes and
            // outlives the slice.
            let plane: &mut [u8] = unsafe {
                std::slice::from_raw_parts_mut(back.as_mut_ptr().cast::<u8>().add(skew), len)
            };
            for v in plane.iter_mut() {
                *v = (next() % 256) as u8;
            }
            let orig = plane.to_vec();
            for round in 0..9u8 {
                // Every mode in every block position, and the residual on
                // all, some or none of the blocks, some of them a DC alone.
                let modes: [u8; 4] = core::array::from_fn(|k| (round + 5 * k as u8) % 9);
                let coeffs: [[i32; 64]; 4] = core::array::from_fn(|b| {
                    let dc_only = (b + usize::from(round)) % 3 == 0;
                    core::array::from_fn(|i| {
                        let c = (next() % 4096) as i32 - 2048;
                        if dc_only && i > 0 { 0 } else { c }
                    })
                });
                let nz = match round % 3 {
                    0 => 0xf,
                    1 => (next() & 0xf) as u8,
                    _ => 0,
                };
                let mut want = orig.clone();
                let run = |k: &dyn Kernels, p: &mut [u8]| {
                    k.intra8x8_mb(IntraMut::new(p, before, stride, n, avail).unwrap(), &modes, &coeffs, nz)
                };
                run(&Scalar, &mut want);
                plane.copy_from_slice(&orig);
                run(&simd, plane);
                assert_eq!(&plane[..], &want[..], "simd {bits} {stride} {round}");
                plane.copy_from_slice(&orig);
                run(&auto, plane);
                assert_eq!(&plane[..], &want[..], "auto {bits} {stride} {round}");
                cases += 1;
            }
        }
    }
    println!("ok {cases}");
}
