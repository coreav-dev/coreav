# PMU profile of `6f7799768413` at `af20b62e`, one thread

`6f7799768413` (sd, CAVLC, 640x480, 1,800 frames: 2.16 M macroblocks) is the slowest of the 32 streams against FFmpeg
at one thread. An earlier change profiled it at `ae609547` (wall 1.154; entropy + MB +0.124, deblocking +0.083). Since then the
boundary-strength kernel is on `main`. This run asks where its gap is now, before the next cut is chosen.

## Method

As `../profile-ae609547-pmu/` (its `job.sh`, `rec_pmu.py` and `stages_pmu.py`), with two changes: only the one-thread
mode (`ipc`), and only `6f7799768413` and `9db41aa16b5a` (the stream `job.sh` checks the attribution build's output
on). CoreAV `main` at `af20b62e`, `dist` with line tables (`prof-af20b62e`); FFmpeg BtbN `n9.0.2-17-g2a571b6068` for
the totals, and the 9.0.2 attribution build made on the VM (function symbols only; source tree, tarball and build log
deleted before profiling, `run/build-order.txt`; no `.debug_*` section, `run/ffattr-sections.txt`). Host: a
`c4-standard-16` with one thread a core, the CPU model of an earlier change (`run/host.txt`). `map.py` maps the samples to
functions (CoreAV by symbol, FFmpeg by the attribution build's symbols); `lines.py` maps CoreAV's samples inside one
function to source lines. `stages_pmu.py` stops with an error at the 1080p and 4k rows, which this run has no stream
for; `run/stages.txt` is its output up to there.

## Results (`run/stages.txt`, `run/functions.txt`)

Stages, cycles as shares of FFmpeg's (the `sd` rows are this one stream):

| | entropy | MB | inter | reconstruction | deblocking | other | total |
|---|---|---|---|---|---|---|---|
| CoreAV | 0.383 | 0.146 | 0.192 | 0.122 | 0.216 | 0.065 | 1.124 |
| FFmpeg | 0.384 | 0.030 | 0.171 | 0.090 | 0.239 | 0.087 | 1.000 |

The total is the sum of the sampled stages (an earlier change's for this stream: 1.191). Against the BtbN build (`perf stat`, the
median of two rounds): cycles 1.102 (earlier profile: 1.157), wall time 1.091 (earlier profile: 1.154).

- **Deblocking is at parity on this stream.** The stage table reads 0.216 against FFmpeg's 0.239 (earlier profile: 0.309
  against 0.225), but the stage rules put CoreAV's boundary-strength kernel (`simd::avx512::strengths::strengths`,
  30 cycles a macroblock) in "other" and FFmpeg's (`ff_h264_loop_filter_strength_mmxext`) in deblocking. Counted
  alike, by function: CoreAV about 375 cycles a macroblock (`deblock_rows` 111, the AVX-512 luma filters 162,
  `filter_chroma_mb` 69, `strengths` 30, `motion_strength` 3), FFmpeg about 365.
- **The gap is in entropy + MB: +0.115** (0.529 against 0.414). FFmpeg's stage rules put `ff_h264_decode_mb_cavlc` in
  entropy, and CoreAV's macroblock layer (`decode_cavlc`) falls in MB, so the two stages are compared together. By
  function, per macroblock: CoreAV's block parse 405 cycles (55 of them the scaling and placement that the stage rules
  count as reconstruction), `decode_cavlc` 214, `BitReader::ue` 58,
  `block_context` 34 and `fill_ctx` 30 (741 together); FFmpeg's `decode_residual` 376, `ff_h264_decode_mb_cavlc`
  199, `fill_decode_caches` 27 and `fill_decode_neighbors` 12 (614).
- Reconstruction +0.032 and inter +0.022 follow.
- Inside `decode_cavlc` (`run/lines-decode_cavlc.txt`) the cycles are spread thin: no source line reaches 17 of its
  214 cycles a macroblock.

## Spend

0.062 USD (upper bound: 0.052 hours times 1.20 USD an hour).

Raw data (`res/`): not committed; sha256 of the tarball and of the profiled binary in `run/sha256.txt`.
