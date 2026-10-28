//! Times the decoder on a stream with each kernel set, in one process.
//!
//! ```text
//! cargo run -p coreav-h264 --release --example time -- [--frames N] [--reps R] [--threads T] STREAM.264 ...
//! ```
//!
//! `--threads T` decodes with T threads ([`Decoder::with_threads`]) in both
//! runs.
//!
//! Both runs share the process, its caches and its pages, and they are
//! interleaved, so the ratio is far steadier than timing two processes.
//! It is a tool for the performance work, not a bar: the speed bars of
//! `docs/design/h264.md` are measured against FFmpeg, from outside.

// A measuring tool, not production code.
#![allow(clippy::expect_used, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

use std::time::Instant;

use coreav_h264::{Auto, Decoder, Kernels, Limits, annex_b};

fn main() -> Result<(), String> {
    let mut frames = 300usize;
    let mut reps = 5usize;
    let mut threads = 1usize;
    let mut streams = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next().and_then(|n| n.parse().ok()).unwrap_or(frames),
            "--reps" => reps = args.next().and_then(|n| n.parse().ok()).unwrap_or(reps),
            "--threads" => threads = args.next().and_then(|n| n.parse().ok()).unwrap_or(threads),
            other => streams.push(other.to_owned()),
        }
    }
    if streams.is_empty() {
        return Err("usage: time [--frames N] [--reps R] [--threads T] STREAM.264 ...".to_owned());
    }
    println!(
        "{:<40} {:>10} {:>10} {:>7}",
        "stream", "reference", "simd", "ratio"
    );
    for path in &streams {
        let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let mut reference = Vec::new();
        let mut vector = Vec::new();
        // The order alternates: whichever decode runs second tends to be
        // a little slower on a busy machine, and alternating cancels it.
        for rep in 0..reps {
            if rep % 2 == 0 {
                reference.push(decode(&data, Auto::scalar(), frames, threads));
                vector.push(decode(&data, Auto::detect(), frames, threads));
            } else {
                vector.push(decode(&data, Auto::detect(), frames, threads));
                reference.push(decode(&data, Auto::scalar(), frames, threads));
            }
        }
        reference.sort_by(f64::total_cmp);
        vector.sort_by(f64::total_cmp);
        let (a, b) = (reference[reps / 2], vector[reps / 2]);
        let name = path.rsplit('/').next().unwrap_or(path);
        println!("{name:<40} {a:>9.3}s {b:>9.3}s {:>7.3}", b / a);
    }
    Ok(())
}

/// Decodes at most `frames` pictures and returns the seconds it took.
fn decode<K: Kernels>(data: &[u8], kernels: K, frames: usize, threads: usize) -> f64 {
    let start = Instant::now();
    let mut decoder = Decoder::with_kernels(kernels, Limits::default()).with_threads(threads);
    let mut made = 0usize;
    let mut sink = 0u64;
    for nal in annex_b(data) {
        let _ = decoder.decode_nal(nal);
        for picture in decoder.take_pictures() {
            // Touch a sample so that nothing is optimized away.
            sink += u64::from(
                picture
                    .luma()
                    .rows()
                    .next()
                    .and_then(|r| r.first())
                    .copied()
                    .unwrap_or(0),
            );
            made += 1;
        }
        if made >= frames {
            break;
        }
    }
    let _ = decoder.flush();
    for picture in decoder.take_pictures() {
        sink += u64::from(
            picture
                .luma()
                .rows()
                .next()
                .and_then(|r| r.first())
                .copied()
                .unwrap_or(0),
        );
    }
    std::hint::black_box(sink);
    start.elapsed().as_secs_f64()
}
