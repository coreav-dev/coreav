# Band views kept for the slice: gating run on the A/B helper's VM

Arms: `base` = main `145232d5`, `k` = `a9e568bc` (`perf/h264-band-views`; the script of the run is the one at the branch's earlier head, the later `bc97fa55` changes only its failure handling). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1),
`binaries.sha256` (base `d2eb1437...`, k `29db314e...`). `analyze.py` of the branch; FFmpeg 9.0.2 `0d709a0b...`. Bars BV-1 to BV-5 were committed before the run and before an earlier change, so the earlier method applies (fixed order, no null arm).

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (k at 1 and 8 threads against base at 1). Eight calls between gate checks
(`g`: four pinned `--pin 2 --rounds 20`; `t8`: four unpinned `--threads 8 --rounds 10`), `hostgate.log`: 18 passes and 9 failed checks (seven failed before-checks, on three calls, waited and passed; **two voids**: `g.1080p` and `g.4k`, after-checks, each call
run again and passed, files kept as `*.void1`), **REBASE count 0**. Spend: none (local VM).

Result (`analysis.txt`): `BV-1 FAIL, BV-2 pass, BV-3 pass, BV-4 pass` (BV-5 is read from `identity.txt` and the conformance report). `k` over `base`, eight threads, ten rounds, discard: overall 0.9915 [0.9823, 1.0038] (21/32 faster; bar at most 0.990 and the upper end below 1.000),
write 0.9945 [0.9793, 1.0034] (20/32); per class discard sd 0.9946, 720p 0.9872, 1080p 0.9957, 4k 0.9896. One thread pinned, 20 rounds: discard overall 1.0012 [0.9972, 1.0072] (15/32), write 1.0021 [0.9942, 1.0076] (14/32).
Against FFmpeg 9.0.2 (median per-stream ratio, share where ours is faster), eight threads discard: base 1.036 (14/32), k 1.024 (13/32); write 1.037 (13/32), 1.009 (16/32); one thread discard 1.122 (8/32), 1.126 (8/32); write 1.071 (10/32), 1.080 (11/32).
