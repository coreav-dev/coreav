# MB layer round 2: gating run on the A/B helper's VM

Arms: `before` = main `b3664291`, `after` = `4d102260` (`perf/h264-mb-layout-2`). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1), `binaries.sha256` (before `d2eb1437...`, after `fde0d5d0...`).
`run-ab-helper.sh` and `analyze.py` of the branch; FFmpeg 9.0.2 `0d709a0b...`. Bars ML2-1 to ML2-7 were committed before the run and before an earlier change, so the earlier method applies (fixed order, no null arm, no in-call gate).

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 96 of 96 (`identity.txt`: the 32 training streams with `after` at 1 and 8 threads against `before` at 1, and the 64 held-out streams at 1 thread).
Sixteen calls between gate checks (`g`: pinned `--pin 2 --rounds 10`; `t8`: `--threads 8 --rounds 10` unpinned; `gm`: as `g` with `MALLOC_MMAP_THRESHOLD_=131072`; `h`: as `g` on the 64 held-out streams; one call per class), `hostgate.log`:
33 passes and 4 failed checks (three before-checks waited and passed; **one void**: `gm.720p`, an after-check, run again and passed, files kept as `gm.720p.*.void1`), **REBASE count 0**. Spend: none (local VM).

Result (`analysis.txt`): `ML2-1 to ML2-6 True` (ML2-7 is read from `identity.txt` and the conformance reports). After over before, median per-stream ratio (95% bootstrap interval), discard unless stated:

| Call | all | write, all | per class (sd, 720p, 1080p, 4k) |
|---|---|---|---|
| `g`, 1 thread, 32 training | 0.9754 [0.9590, 0.9913] (26/32 faster) | 0.9806 [0.9643, 0.9913] | 0.9891, 0.9744, 0.9941, 0.9228 |
| `t8`, 8 threads, 32 training | 0.9766 [0.9505, 0.9910] (24/32) | 0.9856 [0.9492, 0.9994] | 0.9896, 0.9623, 0.9846, 0.9501 |
| `gm`, 1 thread, malloc threshold | 0.9566 [0.9339, 0.9807] (27/32) | 0.9615 [0.9402, 0.9800] | 0.9884, 0.9540, 0.9769, 0.8794 |
| `h`, 1 thread, 64 held-out | 0.9801 [0.9764, 0.9893] (55/64) | 0.9767 [0.9689, 0.9858] | 0.9943, 0.9853, 0.9821, 0.9644 |

Against FFmpeg 9.0.2 (median per-stream ratio, `before` then `after`; share where ours is faster): `g` discard 1.140 then 1.100 (8/32, 9/32), write 1.093 then 1.056 (9/32, 11/32); `t8` discard 1.039 then 0.995 (11/32, 16/32),
write 1.044 then 0.988 (13/32, 16/32); `gm` discard 1.152 then 1.091 (4/32, 11/32); `h` discard 1.102 then 1.091 (14/64, 19/64), write 1.079 then 1.068 (21/64, 22/64).
