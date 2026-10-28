# Deblocking edges read into registers: counts and timing

Evidence behind `docs/design/h264.md`, "Kernel round: deblocking edges
read into registers". Callgrind counts, not timing.

- `shapes.txt`: instructions a call of `Auto::filter_edge` per plane,
  direction and bS, from `tools/count.sh TREE OUT` (the harness
  `tools/dbkcount.rs`, `lto = "fat"`, one codegen unit; the difference of
  1,100 and 100 calls over 1,000). About 65 instructions of each count
  are the harness's own. Columns: `main`, the first version (the direct path
  for every standard shape, the high half written with `_mm_storeh_pd`),
  and this round (horizontal luma edges keep `gather`; the high half an
  unaligned 64-bit write).
- `instructions.txt`: the whole decode of four streams under callgrind,
  for `main` and this round.
- `tools/miri`: the Miri harness for the pointer accesses (copy it to a
  crate, replace `KPATH` with the checkout's
  `crates/coreav-kernels-h264`, and run `RUSTFLAGS="-C
  target-feature=+avx2,+sse4.1" cargo +nightly miri run`).

The gating run (bars DR-1 to DR-4), on the VM that reports the 2.90 GHz
class: `run-ab-helper.sh` (two decoders built alike from one tree each,
outputs checked, then one `scripts/h264_parity.py --pin 2 --rounds 5`
call per class with FFmpeg 9.0.2 as the third command); `provenance.txt`
(binaries' SHA-256, compiler, CPU, FFmpeg, harness); `identity.txt`;
`g.<class>.json`, the harness's per-stream times; `analyze.py`, which
writes `g.summary.json` and `analysis.txt`.
