# Kernel round: Intra_8x8 in vectors

Evidence for `docs/design/h264.md`, "Kernel round: Intra_8x8 in vectors".

- `run-ab-helper.sh BASE K`: the gating run of bars I8-1 to I8-5 (identity, then per class one call at one
  thread pinned and one at eight threads, each between two host gates); the gate is the class's own,
  `docs/measurements/h264-rounds/host-gate/hostgate.py` of the branch `measured/host-gate` (its sha256 is
  in `gating/provenance.txt`); `level-checks.log` holds the checks that chose it.
- `analyze.py DIR`: the bars and the ratios to FFmpeg from the run's reports.
- `tools/miri`: the Miri harness of the kernel's pointer accesses (copy the directory, set the kernels
  crate's path in `Cargo.toml` for `KPATH`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`).
- `gating/`: the run (`run-ab-helper.sh d7bb7ae 0cbac42`): the reports, `analysis.txt` and `summary.json`,
  output identity, conformance, the host gate's log and provenance; `identity-rebased.txt`, the branch
  rebased onto `a7d8e4b` against it; `miri.txt`, the Miri harness's output.
