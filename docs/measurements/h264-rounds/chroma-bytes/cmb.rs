//! Chroma MB deblock microbench (local tool, not committed).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
use coreav_kernels_h264::{ChromaMbMut, EdgeParams, Kernels, Simd};
use std::time::Instant;

const N: usize = 60; // macroblocks in a row
const STRIDE: usize = 8 * N + 72;
const ROWS: usize = 16;

fn plane(seed: u64) -> Vec<u8> {
    let mut s = seed;
    let mut next = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (s >> 33) as u32
    };
    let mut base = vec![0u8; (STRIDE / 4) * (ROWS / 4)];
    for b in base.iter_mut() {
        *b = 100 + (next() % 30) as u8;
    }
    (0..STRIDE * ROWS)
        .map(|i| {
            let (x, y) = (i % STRIDE, i / STRIDE);
            base[(y / 4) * (STRIDE / 4) + x / 4] + (next() % 3) as u8
        })
        .collect()
}

fn main() {
    let simd = Simd::detect().expect("simd");
    let mode = std::env::args().nth(1).unwrap_or_default();
    let (a0, b0) = (plane(1), plane(2));
    let len = a0.len();
    // Both planes in one buffer, Cr's start 1344 bytes past a multiple of
    // 4 KiB from Cb's, so that their rows do not alias modulo 4 KiB.
    let gap = (len + 4095) / 4096 * 4096 + 1344;
    let mut buf = vec![0u8; gap + len];
    let (lo, hi) = buf.split_at_mut(gap);
    let (a, b) = (&mut lo[..len], &mut hi[..len]);
    let p = |bs: u8| EdgeParams { alpha: 40, beta: 12, tc0: [0, 1, 2, 3, 4, 0, 0, 0], bs: [bs; 4] };
    let (v, h) = match mode.as_str() {
        "all" => (true, true),
        "v" => (true, false),
        "h" => (false, true),
        "off" => (false, false),
        _ => panic!("mode"),
    };
    let e = |on: bool, bs: u8| if on { p(bs) } else { p(0) };
    let params = [[e(v, 4); 2], [e(v, 2); 2], [e(h, 3); 2], [e(h, 1); 2]];
    let reps: usize = std::env::var("REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let rounds: usize = std::env::var("ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let t = Instant::now();
        for _ in 0..reps {
            a.copy_from_slice(&a0);
            b.copy_from_slice(&b0);
            for i in 0..N {
                // Seven macroblocks apart, so that a call does not read what
                // the one before it wrote (the decoder runs luma between).
                let m = (i * 7) % N;
                let at = 4 * STRIDE + 8 + 8 * m;
                let (cb, cr) = ChromaMbMut::pair(a, b, at, STRIDE).expect("c");
                simd.filter_chroma_mb(cb, cr, &params);
            }
            std::hint::black_box((&mut *a, &mut *b));
        }
        best = best.min(t.elapsed().as_secs_f64() * 1e9 / (reps * N) as f64);
    }
    println!("{mode:<5} {best:7.2} ns per MB");
}
