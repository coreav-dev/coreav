# The kernels against FFmpeg's assembly, by class, from an earlier change's PMU records (`main` at `acf3ee2f`)

Diagnosis only, from the records of `../profile-acf3ee2f-pmu/` (an earlier change: Compute Engine `c4-standard-8`, Emerald
Rapids, one thread pinned, two rounds, 32 training streams, CoreAV with the detected kernels, so the AVX-512 tier).
No new run, no spend. Nothing here is a speed claim or moves a bar; it chooses the next SIMD round.

## Method

`kernels_pmu.py` re-reads an earlier change's `res/` (`raw.tar.gz` on `measured/h264-profile-acf3ee2f-pmu`, sha256 `a949a3b5…`)
and splits an earlier change's stages into kernel families (the rules are in its docstring):

- CoreAV: the first frame, from the innermost inline frame outwards, in `coreav-kernels-h264` whose function name
  matches a family; helpers (`b1`, `store`, `bytes`) match nothing and so go to the kernel entry that inlined them.
  Boundary strengths (`bs`) are `deblock.rs`'s strengths functions in `coreav-h264`. The rest stays by an earlier change's stage
  (`glue:<stage>`).
- FFmpeg: its function symbols at function level (the attribution build has no debug information), matched by name:
  `ff_deblock_[vh]_chroma*` is `deblock-chroma`, `ff_pred8x8l*` `intra8x8`, `ff_pred8x8_*` `intra-chroma`, and so on.
  Its C drivers (`mc_part_420_simple_8`, `loop_filter`, `ff_h264_filter_mb_fast`, `hl_decode_mb_simple_8`) are glue,
  except `mc_part`/`hl_motion`, which pair with CoreAV's motion-compensation driver (`mc-driver`, `predict.rs`).
- The CoreAV binary is a rebuild of `acf3ee2f` with line tables. Its hash differs from the profiled one (the build
  path is in the line tables), but an earlier change's own `stages_pmu.py` on it reproduces `run/stages.txt` byte for byte, so the
  code layout is the profiled one.

`kernels.txt` has, per class, every family's cycles as a share of FFmpeg's (mean over streams, as in an earlier change), the
ratio, the excess over parity and instructions per macroblock; then each family's share of CoreAV's own cycles,
median per stream: the part-only bound, the most that removing the whole part could save. `kernels.json` has the
first table's numbers.

## Kernels slower than FFmpeg's counterpart

Ratio of cycles, CoreAV's over FFmpeg's, with the excess in FFmpeg's cycles; listed where the ratio is above 1.00
and the excess at least 0.001.

| family | sd | 720p | 1080p | 4k | all 32 |
|---|---|---|---|---|---|
| boundary strengths (`bs`; an earlier change in gating) | 1.75 +0.029 | 1.09 +0.003 | 1.41 +0.009 | 2.17 +0.022 | 1.56 +0.016 |
| prefetch (not SIMD) | (none; FFmpeg 0.009) | 2.98 +0.014 | 2.55 +0.013 | 2.50 +0.024 | 2.03 +0.010 |
| chroma deblocking | **1.94 +0.014** | **1.51 +0.015** | 1.72 +0.008 | 0.98 | 1.58 +0.010 |
| chroma motion compensation | 1.36 +0.009 | 1.05 +0.001 | 1.17 +0.004 | 0.95 | 1.10 +0.003 |
| intra chroma prediction | 2.97 +0.002 | 1.97 +0.005 | 1.53 +0.001 | 3.36 +0.003 | 2.15 +0.003 |
| intra 4x4 | 1.09 | 1.73 +0.003 | 1.53 +0.002 | 1.73 | 1.41 +0.002 |
| intra 8x8 | | 1.80 +0.002 | 1.66 +0.002 | 1.42 | 1.71 +0.001 |
| intra 16x16 | | 1.88 +0.002 | 1.62 | 1.67 | 1.69 |
| chroma IDCT | 0.81 | 2.01 +0.002 | 1.00 | 0.74 | 1.01 |
| luma deblocking | 1.06 +0.004 | 0.93 | 1.00 | 0.67 | 0.95 |
| weighted prediction | 0.50 | 0.61 | 1.56 +0.001 | 0.10 | 0.49 |

At or below parity in every class: luma motion compensation (0.82 to 0.92), the 4x4 IDCT (0.38 to 0.97), the 8x8
IDCT (0.71 to 0.93). Outside the kernels, the motion-compensation driver and the decoder's inter glue together are
1.88 times FFmpeg's `mc_part` driver (+0.032; +0.049 at 4k), and the MB layer is +0.093; neither is a SIMD
kernel.

## The part-only bound and an earlier change's threshold

Each family's share of CoreAV's cycles, median per stream (`kernels.txt`, last table), against an earlier change's 1.5% in a
class:

- **Chroma deblocking: 3.16% sd, 4.13% 720p, 1.62% 1080p, 0.50% 4k.** Above the threshold in three classes, with the
  largest excess of the SIMD kernels at sd and 720p once boundary strengths are set aside. It runs the same
  instructions a macroblock as FFmpeg's (122 against 120, all 32) in 1.58 times the cycles (CoreAV's AVX2 kernel
  against FFmpeg's AVX and SSE2 ones), which points at cycles per instruction rather than work. **The next SIMD
  round.**
- Prefetch: 1.85% 720p, 1.29% 1080p, 4.91% 4k. Above the threshold at 720p and 4k, but it is the reference fetch's
  hint, not a SIMD kernel, and the sampled cycles at a prefetch include the fill-buffer waits it causes; it needs its
  own probe (what it fetches and how far ahead).
- Intra prediction, the four families' medians added: 1.06% sd, 1.41% 720p, 0.86% 1080p, 0.82% 4k. Below the threshold in
  every class even if the whole part went: not built.
- Chroma motion compensation: 3.23% sd, 3.67% 4k, but near parity outside sd and 1080p (1.05 at 720p, 0.95 at 4k; 1.10 overall);
  second in line after chroma deblocking.
- Chroma IDCT: at most 0.47%; not built.

## Reproduce

```
git show origin/measured/h264-profile-acf3ee2f-pmu:<path>/raw.tar.gz > raw.tar.gz   # sha256 a949a3b5...
tar -xzf raw.tar.gz
(cd <worktree at acf3ee2f> && CARGO_PROFILE_DIST_DEBUG=line-tables-only cargo build --locked --profile dist \
  --example decode -p coreav-h264) && cp <target>/dist/examples/decode prof-acf3ee2f
python3 -I ../profile-acf3ee2f-pmu/stages_pmu.py res prof-acf3ee2f | cmp - ../profile-acf3ee2f-pmu/run/stages.txt
KERNELS_JSON=kernels.json python3 -I kernels_pmu.py res prof-acf3ee2f > kernels.txt
```
