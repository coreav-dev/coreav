# Kernel round: Intra_8x8 edges read once per macroblock

`docs/design/h264.md`, "Kernel round: Intra_8x8 edges read once per macroblock", states the hypothesis, the design
and bars IE-1 to IE-5, fixed before any timing of `k`. The round was withdrawn before its gating run (a local
timing A/B showed no gain at IE-1's level) and its code is not in `main`: `k.diff` is the change
(`git diff fb893fd a3e9e9a` over the crates).

- `base` = `main` at `fb893fd`; `k` = `a3e9e9a`, the branch's code commit.
- `functions.txt`: the function-level counts that chose the round (on `914729f`, against FFmpeg 9.0.2, function names
  only) and the 30-picture split of `k` against `base` on `1d4b1775d78a.0.720p`.
- `instructions.txt`: instructions of `k` against `base` (callgrind, first pictures of the 32 training streams),
  information, not a bar.
- `conformance.txt` (97 of 97 at 1, 2 and 8 threads, detected and scalar kernels), `fuzz.txt` (target `intra`, 600 s): on `k`.
- `tools/miri`: the Miri harness of `intra8x8_mb`'s pointer accesses, through `Simd` and `Auto` against the
  reference, at strides 16, 17, 19, 20, 23, 24, 25 and 40 (copy the directory, set the kernels crate's path in
  `Cargo.toml` for `KPATH`, and run `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`);
  `miri.txt`, its output on `k`.
- `run-ab-helper.sh BASE K` is the gating run and `analyze.py DIR` prints the bars from its reports; the run was not
  made.
- `local-ab/`: the local timing A/B (`run.sh`, its reports, `sum.py`, `summary.txt`), information.

## Result

Withdrawn before the gating run. Local timing A/B, `k` over `base`, one thread pinned, six rounds, 32 streams:
discard 0.9991 [0.9955, 1.0033], 18 of 32 faster; write 0.9992 [0.9941, 1.0043]. The design note's "Result"
section has the detail.
