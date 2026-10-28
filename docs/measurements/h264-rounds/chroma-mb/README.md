# Kernel round: a macroblock's chroma edges in one call

`docs/design/h264.md`, "Kernel round: a macroblock's chroma edges in one call".

- `calls.txt`: chroma pair calls a macroblock on `main` (a counting build, not for merge).
- `harness.txt` and `tools/pairbench`: the kernel harness, the paired path against one call a macroblock (information;
  set `KPATH` in its `Cargo.toml` to the kernels crate, `cargo build --release`).
- `instructions.txt`: callgrind instruction counts of the whole decode, `main` against the branch.
- `local-timing.txt`: local wall times, `main` against the branch (information, not a bar run).
- `identity.txt`: the branch's output against `main`'s; `conformance.txt`: the JVT suite on the branch.
- `fuzz.txt`: a local run of the `filter_chroma_mb` fuzz target.
- `tools/miri`: the Miri harness of the kernel's pointer accesses (set `KPATH` in its `Cargo.toml`, then
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`): 168 cases, no error.
- `analyze.py`: the bars CM-1 to CM-4 from the A/B run's JSON files.
- `gating/`: the A/B run of the bars CM-1 to CM-4, from branch `measured/h264-chroma-mb` `a6624948` (its README,
  `analysis.txt`, the JSON and logs of each call).
