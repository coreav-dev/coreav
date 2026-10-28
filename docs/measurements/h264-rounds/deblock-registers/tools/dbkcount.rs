use coreav_kernels_h264::{Auto, EdgeMut, EdgeParams, Kernels, Reach};
use std::hint::black_box;

// args: luma(1)/chroma(0) rows(1: horizontal edge)/(0: vertical) bs n
#[inline(never)]
fn run_shape(k: &Auto, plane: &mut [u8], luma: bool, rows: bool, bs: u8, n: usize) {
    let stride = 128;
    let (lines, reach) = if luma { (16, Reach::Luma) } else { (8, Reach::Chroma) };
    let params = EdgeParams { alpha: 40, beta: 8, tc0: [2, 3, 1, 4], bs: [bs; 4] };
    for i in 0..n {
        let q0 = 8 * stride + 8 + (i % 4) * 16 + (i % 3) * stride;
        let edge = if rows {
            EdgeMut::new(black_box(&mut *plane), q0, stride, 1, lines, reach)
        } else {
            EdgeMut::new(black_box(&mut *plane), q0, 1, stride, lines, reach)
        }
        .unwrap();
        k.filter_edge(edge, black_box(&params));
    }
}

fn main() {
    let a: Vec<usize> = std::env::args().skip(1).map(|v| v.parse().unwrap()).collect();
    let (luma, rows, bs, n) = (a[0] != 0, a[1] != 0, a[2] as u8, a[3]);
    // A smooth plane with small noise, so that most lines are filtered.
    let mut plane = vec![0u8; 128 * 48];
    let mut x = 12345u32;
    for (i, v) in plane.iter_mut().enumerate() {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        *v = (100 + (i % 128) / 8 + ((x >> 16) % 5) as usize) as u8;
    }
    let k = black_box(Auto::detect());
    run_shape(&k, &mut plane, luma, rows, bs, n);
    println!("{}", plane.iter().map(|v| *v as u64).sum::<u64>());
}
