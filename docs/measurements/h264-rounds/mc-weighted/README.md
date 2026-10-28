# Kernel round: weighting inside the interpolation

The kernels' part of the round described in `docs/design/h264.md`,
"Kernel round: weighting inside the interpolation". The decoder's call
site, its bars and the gating run belong to the round that changes
`inter.rs`.

- `bench.txt`: `cargo run --release -p coreav-kernels-h264 --example
  bench`, the weighted lines and `combine`, pinned to one core of an Intel
  Xeon @ 2.50GHz (family 6, model 85), not the bar class. Information
  only. "parts" is the decoder's path before the fused kernels: the
  prediction into a 16-sample-stride scratch block, then `Simd::combine`
  of a list-0 block and it. "fused" is `Simd::luma_mc_weighted` or
  `Simd::chroma_mc_weighted`. List 0's own prediction is the same work
  either way and is left out.
- `tools/miri`: the Miri harness for the weighted kernels' pointer
  accesses. To run it, copy `main.rs` to `src/` of a crate with this
  `Cargo.toml`, replace `KPATH` with the checkout's
  `crates/coreav-kernels-h264`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`.
  Result: 114 cases, no error.
