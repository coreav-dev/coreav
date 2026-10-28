# Intra_4x4 edges and level scales: gating run on the A/B helper's VM

Arms: `base` = `fb893fd`, `k` = `914729f` (`perf/h264-intra-edges`; the branch has moved on since, this run is of `914729f`). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1),
`binaries.sha256` (base `a49a4b18...`, k `3d2067f0...`). Script `run-ab-helper.sh` and `analyze.py` of the branch (`bash run-ab-helper.sh fb893fd 914729f`); FFmpeg 9.0.2 `0d709a0b...`.

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (k at 1 and 8 threads against base at 1). Eight calls between
gate checks (`g`: four pinned `--pin 2 --rounds 20`; `t8`: four unpinned `--threads 8 --rounds 10`), `hostgate.log`: 16 passes and 6 failed checks (five before-checks of `g.sd` and one of
`t8.1080p`, each waited and passed), no void, **REBASE count 0**. Spend: none (local VM).

Result (`analysis.txt`): `IS-1 FAIL, IS-2 pass, IS-3 pass, IS-4 pass` (IS-5 is read from `identity.txt` and the conformance report). `k` over `base`, one thread, discard: overall 0.9997
[0.9962, 1.0025] (16/32 faster), so the upper end is above 1.000; write 0.9994 [0.9948, 1.0009] (20/32). Eight threads discard 0.9953 [0.9848, 1.0060] (18/32).
Against FFmpeg 9.0.2 (median per-stream ratio, share where ours is faster), one thread discard: base 1.181 (5/32), k 1.181 (6/32); write 1.103 (8/32), 1.112 (9/32); eight threads discard 1.050 (13/32), 1.050 (12/32);
write 1.017 (14/32), 1.037 (14/32).
