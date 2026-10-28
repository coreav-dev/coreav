use coreav_kernels_h264::{Auto, Block, BlockMut, Kernels};
use std::hint::black_box;

#[inline(never)]
fn run_shape(k: &Auto, src: &[u8], dst: &mut [u8], w: usize, h: usize, xf: u8, yf: u8, avg: bool, n: usize) {
    for i in 0..n {
        let off = (i % 7) * 64 + (i % 5);
        let s = Block::new(black_box(src), off, 64, w + 5, h + 5).unwrap();
        let d = BlockMut::new(black_box(&mut *dst), (i % 3) * 64, 64, w, h).unwrap();
        if avg { k.luma_mc_avg(d, s, black_box(xf), black_box(yf)); } else { k.luma_mc(d, s, black_box(xf), black_box(yf)); }
    }
}

fn main() {
    let a: Vec<usize> = std::env::args().skip(1).map(|v| v.parse().unwrap()).collect();
    let (w, h, xf, yf, avg, n) = (a[0], a[1], a[2] as u8, a[3] as u8, a[4] != 0, a[5]);
    let mut src = vec![0u8; 64 * 64];
    let mut x = 12345u32;
    for v in src.iter_mut() { x = x.wrapping_mul(1103515245).wrapping_add(12345); *v = (x >> 16) as u8; }
    let mut dst = vec![0u8; 64 * 64];
    let k = black_box(Auto::detect());
    run_shape(&k, &src, &mut dst, w, h, xf, yf, avg, n);
    println!("{}", dst.iter().map(|v| *v as u64).sum::<u64>());
}
