# Exploratory runs (information only, before the bars; none of these builds is the candidate)

All on this VM (Intel Xeon 2.90 GHz, family 6 model 106), `~/tools/abn.py`-style interleaved runs: each
binary pinned to one core, `--discard`, the class frame counts, the median of N rounds per stream, ratio to
`base` = `main` at `3ebeda9`, built alike (`dist`). The host gate (`HOST-GATE.md`) passed before and after
each run. Not a gating method: two or three rounds, no FFmpeg, and the variants below are throwaway code.

| File | Variant (change to `simd/x86/predict.rs` on `3ebeda9`) | Streams, rounds | Result (median, 95% bootstrap interval) |
|---|---|---|---|
| `upper-bounds.json` | `none`: `chroma_block` returns at once (chroma not predicted; output wrong); `copy`: every chroma block copied as at a full-sample position | 16 (every other training stream), 2 | none 0.965 [0.950, 0.970], 4k 0.917; copy 0.984 [0.975, 0.990], 4k 0.976 |
| `loads-stores-4k.json` | the same two, and `ld`: the copy reads the reference rows and writes nothing; `st`: the copy writes the block rows and reads no reference | 8 (4k), 2 | none 0.933, copy 0.977, ld 0.957, st 0.966; `3b1b441e23ae` none 0.84, ld 0.94, st 0.89 |
| `hints-at-entry.json` | a `prefetcht0` at both ends of each row of both chroma windows when `predict` starts, before luma | 16 (1080p, 4k), 3 | 1.014 [1.011, 1.033], 4k 1.017, `3b1b441e23ae` 1.087 |
| `reference-ahead.json` | round 12's part R inside `predict`: at a block whose `px` is a multiple of 64, the window's rows one cache line to the right, luma (both ends) and both chroma planes, `prefetch::rows_for_read`, pictures of at least 2/3 MiB of luma | 24 (720p, 1080p, 4k), 3 | 1.000 [0.990, 1.010]; 720p 1.012, 1080p 0.991, 4k 0.991; streams 0.845 to 1.076 |
