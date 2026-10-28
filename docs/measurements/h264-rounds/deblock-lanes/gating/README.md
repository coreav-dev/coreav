# Deblocking thresholds in vector registers: gating run on the A/B helper's VM

Arms: `base` = main `baa6d0a`, `k` = `d7e7fb0` (`perf/h264-deblock-kernels`; the branch has moved on since, this run is of `d7e7fb0`). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1),
`binaries.sha256` (base `f81c8173...`, k `d2eb1437...`). Script `run-ab-helper.sh` and `analyze.py` of the branch at `49e0682` (`bash run-ab-helper.sh baa6d0a d7e7fb0`); FFmpeg 9.0.2 `0d709a0b...`.

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (k at 1 and 8 threads against base at 1). Eight calls between gate checks
(`g`: four pinned `--pin 2 --rounds 20`; `t8`: four unpinned `--threads 8 --rounds 10`), `hostgate.log`: 16 passes, no failed check, no void, **REBASE count 0**. Spend: none (local VM).

Result (`analysis.txt`): `TL-1 pass, TL-2 pass, TL-3 pass, TL-4 pass` (TL-5 is read from `identity.txt` and the conformance report). `k` over `base`, one thread, discard: overall 0.9954 [0.9923, 0.9985]
(23/32 faster), write 0.9960 [0.9919, 0.9987] (22/32); per class discard sd 0.9918, 720p 0.9934, 1080p 0.9954, 4k 1.0023; write sd 0.9948, 720p 0.9940, 1080p 0.9947, 4k 1.0018.
Eight threads, ten rounds: discard overall 0.9986 [0.9763, 1.0095] (17/32), write 0.9903 [0.9786, 1.0053] (21/32); per class discard sd 1.0007, 720p 0.9770, 1080p 1.0104, 4k 0.9750.
Against FFmpeg 9.0.2 (median per-stream ratio, share where ours is faster), one thread discard: base 1.136 (7/32), k 1.136 (8/32); write 1.088 (10/32), 1.083 (10/32); eight threads discard 1.060 (14/32), 1.049 (13/32);
write 1.029 (15/32), 1.030 (12/32).
