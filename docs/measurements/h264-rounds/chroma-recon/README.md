# Kernel round: a macroblock's chroma in one call

Evidence for `docs/design/h264.md`, "Kernel round: a macroblock's chroma in one call".

- `run-ab-helper.sh BASE K`: the gating run of bars CR-1 to CR-5 (identity, then per class one call at one
  thread pinned and one at eight threads, each at ten rounds and between two host gates); the gate is the
  class's own, `docs/measurements/h264-rounds/host-gate/hostgate.py` of the branch `measured/host-gate` (its
  sha256 is in `gating/provenance.txt`); `level-checks.log` holds the checks before the run.
- `analyze.py DIR`: the bars and the ratios to FFmpeg from the run's reports.
- `instructions.txt`: callgrind instruction counts of `k` over `base` on the first pictures of each training
  stream (information, not a bar).
- `tools/miri`: the Miri harness of the two kernels' pointer accesses (copy the directory, set the kernels
  crate's path in `Cargo.toml` for `KPATH`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`).
- `gating/`: the run (`run-ab-helper.sh 93f7d87 f5ca0ec`): the reports, `analysis.txt` and `summary.json`,
  output identity, conformance, the host gate's log and provenance; `miri.txt`, the Miri harness's output,
  and `fuzz.txt`, the `chroma_mb` fuzz target's run, both on `k`; `identity-rebased.txt`, the branch rebased
  onto `174964a` against it.
