# Deblocking driver round: gating run on the A/B helper's VM

Arms: `base` = `fb893fd`, `k` = `e2bc974` (`perf/h264-deblock-driver`; the branch has moved on since, this run is of `e2bc974`). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1),
`binaries.sha256` (base `a49a4b18...`, k `19d37a59...`). Script `run-ab-helper.sh` and `analyze.py` of the branch (`bash run-ab-helper.sh fb893fd e2bc974`); FFmpeg 9.0.2 `0d709a0b...`.

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (k at 1 and 8 threads against base at 1). Eight calls between gate
checks (`g`: four pinned `--pin 2 --rounds 20`; `t8`: four unpinned `--threads 8 --rounds 10`), `hostgate.log`: 19 passes and 4 failed checks (**three voids**: `g.1080p` twice and
`g.4k` once, after-checks of 1.059 to 1.094 on the 720p reference stream; each call was run again and passed; the voided calls' files are kept as `*.void1`, `*.void2`), **REBASE count 0**. Spend: none (local VM).

Result (`analysis.txt`): `DD-1 pass, DD-2 pass, DD-3 pass, DD-4 FAIL` (DD-5 is read from `identity.txt` and the conformance report). `k` over `base`, one thread, discard: overall 0.9948 [0.9895, 0.9989]
(22/32 faster), write 0.9944 [0.9907, 0.9976] (25/32); per class discard sd 0.9872, 720p 0.9903, 1080p 1.0013, 4k 1.0039. Eight threads, ten rounds, discard overall 1.0088 [0.9979, 1.0181] (12/32 faster,
bar at most 1.005), write 1.0032 [0.9900, 1.0160] (15/32); 4k discard 1.0197 [1.0135, 1.0526] (0/8).
Against FFmpeg 9.0.2 (median per-stream ratio, share where ours is faster), one thread discard: base 1.145 (6/32), k 1.139 (6/32); write 1.105 (8/32), 1.099 (8/32); eight threads discard 1.079 (11/32), 1.092 (10/32);
write 1.075 (10/32), 1.071 (12/32).
