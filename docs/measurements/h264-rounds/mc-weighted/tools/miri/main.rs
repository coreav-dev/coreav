// The Miri harness of the weighted interpolation's pointer accesses:
// luma and chroma, both weightings, through `Simd` and `Auto` against the
// reference, with the source window and the block each in a slice that
// starts at its first sample and ends at its last, from misaligned bases,
// at the narrowest strides a view takes and wider ones.
use coreav_kernels_h264::{Auto, Block, BlockMut, Kernels, Scalar, Simd, Weighting};

/// A slice of `len` bytes starting `skew` bytes into an 8-aligned backing.
fn skewed(back: &mut Vec<u64>, len: usize, skew: usize) -> &mut [u8] {
    back.resize(len / 8 + 2, 0);
    // SAFETY (test only): the backing holds `len + skew` bytes and
    // outlives the slice.
    unsafe { std::slice::from_raw_parts_mut(back.as_mut_ptr().cast::<u8>().add(skew), len) }
}

fn main() {
    let simd = Simd::detect().expect("avx2");
    let auto = Auto::detect();
    let mut x = 17u32;
    let mut next = move || {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        x >> 8
    };
    let weightings = [
        Weighting::Uni { log_wd: 0, w: -128, o: 127 },
        Weighting::Uni { log_wd: 6, w: 97, o: -40 },
        Weighting::Bi { log_wd: 5, w0: 128, w1: -64, o: 0 },
        Weighting::Bi { log_wd: 7, w0: -128, w1: 127, o: -128 },
    ];
    let mut cases = 0;
    for luma in [true, false] {
        let (extra, widths, heights, positions): (usize, &[usize], &[usize], &[(u8, u8)]) = if luma {
            (5, &[8, 16], &[1, 4, 16], &[(0, 0), (1, 0), (2, 2), (3, 1), (0, 3)])
        } else {
            (1, &[2, 4, 8], &[1, 2, 8], &[(0, 0), (3, 5), (7, 1)])
        };
        for &w in widths {
            for &h in heights {
                for (k, &(xf, yf)) in positions.iter().enumerate() {
                    let pad = [0usize, 1, 3][k % 3];
                    let skew = k % 3 + usize::from(luma);
                    let (src_stride, dst_stride) = (w + extra + pad, w + pad);
                    let src_len = src_stride * (h + extra - 1) + w + extra;
                    let dst_len = dst_stride * (h - 1) + w;
                    let (mut back_src, mut back_dst) = (Vec::new(), Vec::new());
                    let src = skewed(&mut back_src, src_len, skew);
                    for v in src.iter_mut() {
                        *v = (next() % 256) as u8;
                    }
                    let source = Block::new(src, 0, src_stride, w + extra, h + extra).unwrap();
                    let dst = skewed(&mut back_dst, dst_len, (skew + 1) % 8);
                    for v in dst.iter_mut() {
                        *v = (next() % 256) as u8;
                    }
                    let orig = dst.to_vec();
                    let weighting = weightings[(k + h) % weightings.len()];
                    let run = |k: &dyn Kernels, out: &mut [u8]| {
                        let block = BlockMut::new(out, 0, dst_stride, w, h).unwrap();
                        if luma {
                            k.luma_mc_weighted(block, source, xf, yf, weighting);
                        } else {
                            k.chroma_mc_weighted(block, source, xf, yf, weighting);
                        }
                    };
                    let mut want = orig.clone();
                    run(&Scalar, &mut want);
                    for k in [&simd as &dyn Kernels, &auto] {
                        dst.copy_from_slice(&orig);
                        run(k, dst);
                        assert_eq!(dst, &want[..], "luma {luma} {w}x{h} at ({xf}, {yf}) {weighting:?}");
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("{cases} cases, no error");
}
