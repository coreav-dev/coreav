# Kernel round: chroma intra prediction from one view of both planes

`docs/design/h264.md`, "Kernel round: chroma intra prediction from one view of both planes", states what the probe
measured, the design and bars IC-1 to IC-5, fixed before any timing of `k`.

- `probe/`: the cost probe that chose the round (`plan.txt`, written before the run; `ab.txt`, `ab-classes.txt`;
  `items-recon-by-class.txt` and `ff-pred-modes.txt`, function-level shares against FFmpeg 9.0.2, function names
  only); `ann-chroma.txt` (perf annotation of the kernel on `main` at `23b45d4`, the file's `r28-base`) and `instr-*.txt` (callgrind counts per
  instruction of the kernel, four streams). The probe builds (the call, or `intra16x16`'s, made twice) were not
  kept; `plan.txt` describes them.
- `tools/ab3.py` and `tools/abcls.py`: the interleaved runner and the per-class summary (bootstrap 95% intervals)
  that the plans call `~/h264work/prof/ab3.py` and `abcls.py`. `local-ab/plan.txt`'s `ident.txt` is `identity.txt`.
- `run-ab-helper.sh BASE K` is the gating run and `analyze.py DIR` prints the bars from its reports; the run was not
  made.
- `tools/miri`: the Miri harness of `intra_chroma_mb`'s and `chroma_residual_mb`'s pointer accesses with the one
  view of both planes (copy the directory, set the kernels crate's path in `Cargo.toml` for `KPATH`, and run
  `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`); `miri.txt`, its output on `k`.
- `instructions.txt`: callgrind counts of `base` and `k` on one 720p stream, information.
- `identity.txt`: `k` against `base` on the 32 training streams at 1 and 8 threads (md5 of the decoded output).
- `local-ab/`: the local timing A/B that decided not to queue the round (`plan.txt`, written before it, `ab.txt` from
  the interleaved runner, `ab-classes.txt` per class with bootstrap 95% intervals).
- `conformance.txt` (97 of 97 at 1, 2 and 8 threads, detected kernels, and with scalar kernels), `fuzz.txt` (target
  `chroma_mb`, 600 s), `miri.txt`: on `k`.
- `k.diff`: the change (`git diff b6a0bc6 b9824c9` over the crates); the code is on `rejected/h264-intra-chroma`.

## Result

Not queued. Local timing, `k` over `base`, one thread pinned, six rounds, 32 streams, discard: 0.9986 [0.9969,
1.0030], 17 of 32 faster. The design note's "Result" section has the detail.
