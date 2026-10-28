// The Miri harness of the chroma macroblock kernels' pointer accesses:
// `intra_chroma_mb` (one `ChromaIntraMut` of both planes) and
// `chroma_residual_mb` through `Simd` and `Auto` against the reference, in
// planes that start at the first neighbour the view names (the block's
// first sample for the residual) and end at the block's last sample, from
// a misaligned base, at the narrowest stride and wider ones; for the
// residual the two components of different strides, for intra one stride
// and Cr's plane as long as Cb's or longer (the kernel checks the shorter);
// for every combination of neighbours, every mode, and residuals with AC
// coefficients, with a DC alone and with neither.
use coreav_kernels_h264::{Auto, BlockMut, ChromaIntraMut, IntraAvail, Kernels, Scalar, Simd};

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
    let (mut back_b, mut back_r) = (Vec::new(), Vec::new());
    for bits in 0..16u8 {
        let avail = IntraAvail {
            left: bits & 1 != 0,
            top: bits & 2 != 0,
            top_right: bits & 4 != 0,
            top_left: bits & 8 != 0,
        };
        let before = |stride: usize| {
            if avail.top_left {
                stride + 1
            } else if avail.top || avail.top_right {
                stride
            } else if avail.left {
                1
            } else {
                0
            }
        };
        for (sb, sr) in [(8usize, 8usize), (9, 24), (24, 9), (17, 17)] {
            let skew = usize::from(bits % 3);
            let (lb, lr) = (before(sb) + 7 * sb + 8, before(sr) + 7 * sr + 8);
            let pb = plane(&mut back_b, lb, skew);
            let pr = plane(&mut back_r, lr, 2 - skew);
            for v in pb.iter_mut().chain(pr.iter_mut()) {
                *v = (next() % 256) as u8;
            }
            let (ob, or) = (pb.to_vec(), pr.to_vec());
            for mode in 0..6u8 {
                let dc: [[i32; 4]; 2] = core::array::from_fn(|c| {
                    core::array::from_fn(|_| if (mode + c as u8) % 3 == 0 { 0 } else { (next() % 4096) as i32 - 2048 })
                });
                let ac: [[[i32; 16]; 4]; 2] = core::array::from_fn(|_| {
                    core::array::from_fn(|_| core::array::from_fn(|_| (next() % 4096) as i32 - 2048))
                });
                let ac_nz = (next() & 0xff) as u8 & if mode % 2 == 0 { 0 } else { 0xff };
                // Intra: Cb's geometry for both; Cr's plane is at least as
                // long where its stride is at least Cb's.
                let intra = |k: &dyn Kernels, b: &mut [u8], r: &mut [u8]| {
                    if sr < sb {
                        return;
                    }
                    let v = ChromaIntraMut::new(b, r, before(sb), sb, avail).unwrap();
                    k.intra_chroma_mb(v, mode, &dc, &ac, ac_nz);
                };
                let (mut wb, mut wr) = (ob.clone(), or.clone());
                intra(&Scalar, &mut wb, &mut wr);
                for (name, k) in [("simd", &simd as &dyn Kernels), ("auto", &auto)] {
                    pb.copy_from_slice(&ob);
                    pr.copy_from_slice(&or);
                    intra(k, pb, pr);
                    assert_eq!((&pb[..], &pr[..]), (&wb[..], &wr[..]), "intra {name} {bits} {sb} {sr} {mode}");
                }
                // The residual alone, in planes that start at the block.
                let (b0, r0) = (before(sb), before(sr));
                let inter = |k: &dyn Kernels, b: &mut [u8], r: &mut [u8]| {
                    let vb = BlockMut::new(&mut b[b0..], 0, sb, 8, 8).unwrap();
                    let vr = BlockMut::new(&mut r[r0..], 0, sr, 8, 8).unwrap();
                    k.chroma_residual_mb(vb, vr, &dc, &ac, ac_nz);
                };
                let (mut wb, mut wr) = (ob.clone(), or.clone());
                inter(&Scalar, &mut wb, &mut wr);
                for (name, k) in [("simd", &simd as &dyn Kernels), ("auto", &auto)] {
                    pb.copy_from_slice(&ob);
                    pr.copy_from_slice(&or);
                    inter(k, pb, pr);
                    assert_eq!((&pb[..], &pr[..]), (&wb[..], &wr[..]), "inter {name} {bits} {sb} {sr} {mode}");
                }
                cases += 1;
            }
        }
    }
    println!("ok {cases}");
}
