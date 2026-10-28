//! Times each kernel, scalar against SIMD, on the shapes the decoder
//! calls it with.
//!
//! ```text
//! cargo run -p coreav-kernels-h264 --release --example bench
//! ```
//!
//! It is a tool for the performance work, not a bar: it says which kernel
//! is worth more work, and the speed bars of `docs/design/h264.md` are
//! measured end to end against FFmpeg.

// A measuring tool, not production code: the lints that keep the decoder
// panic-free are relaxed, as clippy.toml does for tests.
#![allow(clippy::expect_used)]

use std::time::Instant;

use coreav_kernels_h264::{
    Block, BlockMut, Combine, EdgeMut, EdgeParams, Kernels, Reach, Scalar, Simd, Weighting,
};

/// A reproducible pseudo-random source.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn samples(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 24) as u8).collect()
    }
}

/// Runs `body` until it has taken at least a tenth of a second, and
/// returns the nanoseconds one call took.
fn time(mut body: impl FnMut()) -> f64 {
    let mut rounds = 64u64;
    loop {
        let start = Instant::now();
        for _ in 0..rounds {
            body();
        }
        let taken = start.elapsed();
        if taken.as_secs_f64() > 0.1 {
            return taken.as_secs_f64() * 1e9 / rounds as f64;
        }
        rounds *= 4;
    }
}

fn report(name: &str, scalar: f64, simd: f64) {
    println!(
        "{name:<34} scalar {scalar:8.1} ns   simd {simd:8.1} ns   ratio {:.2}",
        simd / scalar
    );
}

fn main() {
    let Some(simd) = Simd::detect() else {
        println!("no SIMD kernels on this CPU");
        return;
    };
    let mut rng = Rng(7);

    for (w, h) in [(16, 16), (8, 8), (4, 4)] {
        let src = rng.samples((w + 5) * (h + 5));
        let mut plane = rng.samples(w * h);
        for (xf, yf) in [(0, 0), (2, 0), (0, 2), (2, 2), (1, 3)] {
            let source = Block::new(&src, 0, w + 5, w + 5, h + 5).expect("block");
            let scalar = time(|| {
                let dst = BlockMut::new(&mut plane, 0, w, w, h).expect("block");
                Scalar.luma_mc(dst, source, xf, yf);
            });
            let fast = time(|| {
                let dst = BlockMut::new(&mut plane, 0, w, w, h).expect("block");
                simd.luma_mc(dst, source, xf, yf);
            });
            report(&format!("luma_mc {w}x{h} ({xf},{yf})"), scalar, fast);
        }
    }

    for (w, h) in [(8, 8), (4, 4), (2, 2)] {
        let src = rng.samples((w + 1) * (h + 1));
        let mut plane = rng.samples(w * h);
        let source = Block::new(&src, 0, w + 1, w + 1, h + 1).expect("block");
        let scalar = time(|| {
            let dst = BlockMut::new(&mut plane, 0, w, w, h).expect("block");
            Scalar.chroma_mc(dst, source, 3, 5);
        });
        let fast = time(|| {
            let dst = BlockMut::new(&mut plane, 0, w, w, h).expect("block");
            simd.chroma_mc(dst, source, 3, 5);
        });
        report(&format!("chroma_mc {w}x{h}"), scalar, fast);
    }

    let mut plane = rng.samples(16 * 16);
    let coeffs4: [i32; 16] = core::array::from_fn(|i| (i as i32 * 37) % 600 - 300);
    let scalar = time(|| {
        let dst = BlockMut::new(&mut plane, 0, 16, 4, 4).expect("block");
        Scalar.idct4x4_add(dst, &coeffs4);
    });
    let fast = time(|| {
        let dst = BlockMut::new(&mut plane, 0, 16, 4, 4).expect("block");
        simd.idct4x4_add(dst, &coeffs4);
    });
    report("idct4x4_add", scalar, fast);

    // Four blocks a call, all four with coefficients, and two of four.
    let quad: [[i32; 16]; 4] =
        core::array::from_fn(|k| core::array::from_fn(|i| ((k * 16 + i) as i32 * 37) % 600 - 300));
    for nz in [0xf_u8, 0x5] {
        let scalar = time(|| {
            let dst = BlockMut::new(&mut plane, 0, 16, 8, 8).expect("block");
            Scalar.idct4x4_add_quad(dst, &quad, nz);
        });
        let fast = time(|| {
            let dst = BlockMut::new(&mut plane, 0, 16, 8, 8).expect("block");
            simd.idct4x4_add_quad(dst, &quad, nz);
        });
        report(&format!("idct4x4_add_quad nz={nz:#x}"), scalar, fast);
    }

    let coeffs8: [i32; 64] = core::array::from_fn(|i| (i as i32 * 53) % 600 - 300);
    let scalar = time(|| {
        let dst = BlockMut::new(&mut plane, 0, 16, 8, 8).expect("block");
        Scalar.idct8x8_add(dst, &coeffs8);
    });
    let fast = time(|| {
        let dst = BlockMut::new(&mut plane, 0, 16, 8, 8).expect("block");
        simd.idct8x8_add(dst, &coeffs8);
    });
    report("idct8x8_add", scalar, fast);

    let p0 = rng.samples(16 * 16);
    let p1 = rng.samples(16 * 16);
    for (name, mode) in [
        ("combine average 16x16", Combine::Average),
        (
            "combine weighted 16x16",
            Combine::Bi {
                log_wd: 5,
                w0: 33,
                w1: 31,
                o: 0,
            },
        ),
    ] {
        let a = Block::new(&p0, 0, 16, 16, 16).expect("block");
        let b = Block::new(&p1, 0, 16, 16, 16).expect("block");
        let scalar = time(|| {
            let dst = BlockMut::new(&mut plane, 0, 16, 16, 16).expect("block");
            Scalar.combine(dst, a, b, mode);
        });
        let fast = time(|| {
            let dst = BlockMut::new(&mut plane, 0, 16, 16, 16).expect("block");
            simd.combine(dst, a, b, mode);
        });
        report(name, scalar, fast);
    }

    // Weighted prediction, the way the decoder made it before the fused
    // kernels (the prediction into a block of its own, then `combine`)
    // against the fused kernel. List 0's own prediction is the same work
    // either way and is left out.
    let list0 = rng.samples(16 * 16);
    for (name, weighting) in [
        (
            "uni",
            Weighting::Uni {
                log_wd: 6,
                w: 70,
                o: -3,
            },
        ),
        (
            "bi",
            Weighting::Bi {
                log_wd: 5,
                w0: 33,
                w1: 31,
                o: 0,
            },
        ),
    ] {
        let mode = match weighting {
            Weighting::Uni { log_wd, w, o } => Combine::Uni {
                list: 1,
                log_wd,
                w,
                o,
            },
            Weighting::Bi { log_wd, w0, w1, o } => Combine::Bi { log_wd, w0, w1, o },
        };
        for (luma, w, h, xf, yf) in [
            (true, 16, 16, 2, 2),
            (true, 16, 16, 0, 0),
            (true, 8, 8, 1, 3),
            (false, 8, 8, 3, 5),
            (false, 4, 4, 3, 5),
        ] {
            let extra = if luma { 5 } else { 1 };
            let src = rng.samples((w + extra) * (h + extra));
            let source = Block::new(&src, 0, w + extra, w + extra, h + extra).expect("block");
            let mut scratch = vec![0u8; 16 * 16];
            let parts = time(|| {
                let pred = BlockMut::new(&mut scratch, 0, 16, w, h).expect("block");
                if luma {
                    simd.luma_mc(pred, source, xf, yf);
                } else {
                    simd.chroma_mc(pred, source, xf, yf);
                }
                let a = Block::new(&list0, 0, 16, w, h).expect("block");
                let b = Block::new(&scratch, 0, 16, w, h).expect("block");
                let dst = BlockMut::new(&mut plane, 0, w, w, h).expect("block");
                simd.combine(dst, a, b, mode);
            });
            let fused = time(|| {
                let dst = BlockMut::new(&mut plane, 0, w, w, h).expect("block");
                if luma {
                    simd.luma_mc_weighted(dst, source, xf, yf, weighting);
                } else {
                    simd.chroma_mc_weighted(dst, source, xf, yf, weighting);
                }
            });
            let plane_name = if luma { "luma" } else { "chroma" };
            println!(
                "{:<34} parts  {parts:8.1} ns   fused {fused:8.1} ns   ratio {:.2}",
                format!("{plane_name} weighted {name} {w}x{h} ({xf},{yf})"),
                fused / parts
            );
        }
    }

    // A plane the filter actually works on: random samples differ by more
    // than beta from their neighbours, so filterSamplesFlag would be false
    // on every line and the kernel would only measure its early-out. Here
    // the two sides of the edge are flat and 15 apart, with a little
    // noise, which is what a real block boundary looks like.
    let mut edge_plane: Vec<u8> = (0..32 * 32)
        .map(|i| {
            let column = i % 32;
            let noise = ((i * 7) % 5) as u8;
            if column < 16 {
                100 + noise
            } else {
                115 + noise
            }
        })
        .collect();
    let params = EdgeParams {
        alpha: 200,
        beta: 12,
        tc0: [0, 3, 3, 3, 0, 0, 0, 0],
        bs: [4, 3, 2, 1],
    };
    for (name, reach, lines, across, along) in [
        ("filter_edge luma vertical", Reach::Luma, 16, 1, 32),
        ("filter_edge chroma vertical", Reach::Chroma, 8, 1, 32),
        ("filter_edge luma horizontal", Reach::Luma, 16, 32, 1),
        ("filter_edge chroma horizontal", Reach::Chroma, 8, 32, 1),
    ] {
        let at = 8 * 32 + 16;
        let scalar = time(|| {
            let edge =
                EdgeMut::new(&mut edge_plane, at, across, along, lines, reach).expect("edge");
            Scalar.filter_edge(edge, &params);
        });
        let fast = time(|| {
            let edge =
                EdgeMut::new(&mut edge_plane, at, across, along, lines, reach).expect("edge");
            simd.filter_edge(edge, &params);
        });
        report(name, scalar, fast);
        if simd.has_avx512() {
            // The AVX-512 tier against AVX2 alone, both vector.
            let avx2 = simd.without_avx512();
            let narrow = time(|| {
                let edge =
                    EdgeMut::new(&mut edge_plane, at, across, along, lines, reach).expect("edge");
                avx2.filter_edge(edge, &params);
            });
            println!(
                "{:<34} avx2   {narrow:8.1} ns   avx512 {fast:8.1} ns   ratio {:.2}",
                format!("{name}, tiers"),
                fast / narrow
            );
        }
    }
}
