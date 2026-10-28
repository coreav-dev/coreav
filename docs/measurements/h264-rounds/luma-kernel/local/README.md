# Exploratory runs (information only, before the bars; neither variant is the candidate)

On this VM (Intel Xeon 2.90 GHz, family 6 model 106), `~/tools/abn.py`-style interleaved runs: each binary
pinned to one core, `--discard`, the class frame counts, the median of 2 rounds per stream, ratio to `lbase`
= the chroma branch at `e807f49` (the code `main` has at `3443dfc`), built alike (`dist`). The host gate
(`HOST-GATE.md`) passed before and after. Not a gating method: two rounds, no FFmpeg, throwaway code.

| File | Variant (change to `simd/x86/predict.rs`) | Streams, rounds | Result (median, 95% bootstrap interval) |
|---|---|---|---|
| `upper-bounds.json` | `lnone`: `luma_block` returns at once (luma not predicted for 8- and 16-wide blocks; output wrong); `lcopy`: every such block copied as at a full-sample position (`xf` and `yf` masked to 0) | 16 (every other training stream), 2 | lnone 0.937 [0.914, 0.945], 4k 0.892; lcopy 0.957 [0.946, 0.978]: sd 0.969, 720p 0.982, 1080p 0.957, 4k 0.941 |
