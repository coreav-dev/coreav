# Kernel round: Intra_4x4 edges in vectors, and level scales built once

`docs/design/h264.md`, "Kernel round: Intra_4x4 edges in vectors, and level scales built once", states the hypothesis,
the design and bars IS-1 to IS-5, fixed before any timing of `k`. The round was rejected by its primary bar (IS-1)
and its code is not in `main`.

- `base` = `main` at `fb893fd`; `k` = `914729f`, the branch's code commit.
- `profile-by-class.txt`: the reconstruction and deblocking items at 720p and 1080p that chose the round
  (`items_by_class.py`, which reads `../profile-40f29a6/classify.py`; CoreAV's samples are `../profile-40f29a6`'s,
  FFmpeg's a recording of the 9.0.2 attribution build of `../profile` with `../profile/rec.py`, function level only,
  `samples-ff.tar.gz` on branch `measured/h264-intra-scales`).
- `instructions.txt`, `functions.txt`: instructions of `k` against `base` (callgrind), information, not a bar.
- `conformance.txt` (97 of 97 at 1, 2 and 8 threads, detected and scalar kernels), `fuzz.txt` (target `intra`, 600 s): on `k`.
- `tools/miri`: the Miri harness of `intra4x4_mb`'s pointer accesses, through `Simd` and `Auto` against the
  reference, at strides 16, 17, 19, 20, 21 and 40 (copy the directory, set the kernels crate's path in `Cargo.toml`
  for `KPATH`, and run `RUSTFLAGS="-C target-feature=+avx2,+sse4.1" cargo +nightly miri run`); `miri.txt`, its
  output on `k` (864 cases, clean).
- `run-ab-helper.sh BASE K` is the gating run, `analyze.py DIR` prints the bars from its reports; the run's output goes
  to `gating/`.

## Result

Rejected by IS-1. `gating/` (one run of `run-ab-helper.sh fb893fd 914729f` on the bar class host, Intel Xeon @
2.90 GHz, family 6 model 106; branch `measured/h264-intra-scales` `e31590c`): `k` over `base`, one thread, discard
0.9997 [0.9962, 1.0025], 16 of 32 faster (IS-1 missed: the upper end is above 1.000); write 0.9994 [0.9948,
1.0009]; per class within 1.005 (sd 0.9882 the only clear gain); eight threads discard 0.9953, write 1.0029; output
identical on 32 of 32. Against FFmpeg 9.0.2, one thread discard: 1.181 for both `base` and `k`. The design note's
"Result" section has the table.
