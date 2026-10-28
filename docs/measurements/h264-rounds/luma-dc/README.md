# Kernel round: a macroblock's luma residual in one call

Evidence for `docs/design/h264.md`, "Kernel round: a macroblock's luma residual in one call". The round was
rejected by its primary bar (LD-1) and its code is not in `main`.

- `counts.txt`: how many luma residual blocks have only a DC, per stream, at `6a4ff38` (a counting build, not
  kept).
- `functions.txt`: callgrind per-function instruction differences on two streams, `base` to `k` and `k` to the
  variant that marks DC-only blocks in the entropy layer.
- `variants/`: the three variants measured and not shipped or not kept as measured (the entropy layer's mark,
  the Intra_4x4 test, `k` without the Intra_8x8 test) and their instruction counts.
- `mutations.txt`: the kernel mutations and whether the tests caught each.
- `instructions.txt`: callgrind instruction counts of `k` over `base` on the first pictures of each training
  stream (information, not a bar).
- `run-ab-helper.sh BASE K`: the gating run of bars LD-1 to LD-5 (identity, then per class one call at one
  thread pinned at twenty rounds and one at eight threads at ten, each between two host gates); the gate is
  the class's own, `docs/measurements/h264-rounds/host-gate/hostgate.py` of the branch `measured/host-gate`
  (its sha256 is in the run's `provenance.txt`).
- `analyze.py DIR`: the bars and the ratios to FFmpeg from the run's reports.
- `tools/miri`: the Miri harness of the two kernels' pointer accesses (copy the directory, set the kernels
  crate's path in `Cargo.toml` for `KPATH`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`); `miri.txt`, its output on `k`.
- `fuzz.txt`: the `luma_residual_mb` fuzz target's run on `k`.
- `conformance.txt`: the conformance suite on `k`, detected and scalar kernels, at 1, 2 and 8 threads.
- `gating/`: the run (`run-ab-helper.sh 6a4ff38 e49998b`, branch `measured/h264-luma-dc-gating`): its README,
  the reports, `analysis.txt`, `identity.txt`, `hostgate.log` and `provenance.txt`.
