#![allow(clippy::expect_used, missing_docs)]
use coreav_kernels_h264::{Block, BlockMut, Kernels, Simd};
use std::time::Instant;
fn main() {
    let best = Simd::detect().expect("simd");
    let tiers = [best.without_avx512(), best];
    let stride = 64usize;
    let src: Vec<u8> = (0..stride * 64)
        .map(|i| ((i * 37 + (i / 64) * 11) % 256) as u8)
        .collect();
    let mut dst = vec![0u8; stride * 32];
    let mut tot = [0f64; 2];
    for (w, h) in [(16usize, 16usize), (16, 8), (8, 8), (8, 16)] {
        let mut line = format!("{w}x{h}:");
        for pos in 0..16u8 {
            let (xf, yf) = (pos & 3, pos >> 2);
            let mut t = [f64::MAX; 2];
            for (k, simd) in tiers.iter().enumerate() {
                for _ in 0..15 {
                    let n = 4000;
                    let st = Instant::now();
                    for it in 0..n {
                        let at = (it % 8) * 4 + (it % 5) * stride;
                        let s = Block::new(&src, at, stride, w + 5, h + 5).expect("src");
                        let d = BlockMut::new(&mut dst, (it % 4) * 16, stride, w, h).expect("dst");
                        simd.luma_mc(d, s, xf, yf);
                    }
                    t[k] = t[k].min(st.elapsed().as_secs_f64() * 1e9 / n as f64);
                }
            }
            tot[0] += t[0];
            tot[1] += t[1];
            line += &format!(" {pos}:{:.2}", t[1] / t[0]);
        }
        println!("{line}");
    }
    println!(
        "sum avx2 {:.0} ns, avx512 {:.0} ns, ratio {:.3}",
        tot[0],
        tot[1],
        tot[1] / tot[0]
    );
}
