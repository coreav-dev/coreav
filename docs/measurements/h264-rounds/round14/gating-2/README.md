# Round 14 re-gating, cloud container

before main `e856e4d` (an earlier change merged), after `perf/h264-weighted-call` `e9e4800` (code of `d440615`). Built on the A/B helper's VM
(`dist`, `--locked`, rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: before `0fc7399c...`, after
`26286fd8...`). `analyze.py` of `e9e4800`; runner script not published.

**Host class: cloud container, Intel Xeon family 6 model 85 (the Cascade Lake class), 24 cores visible, 8 CPUs requested (`nproc`
8), AVX-512 F/BW/CD/DQ/VL present, no VBMI (`cpu.txt`).** This is not the Zen 3 host of the first gating run of this round; the
round does not depend on an ISA tier. Reference for this host: three samples five minutes apart (`reference.json`, max
deviation 1.2% and 3.6%); the first attempt of this container run (`attempt1-unstable.txt`) failed the stability check by 5.8%
and timed nothing. Identity 32 of 32 (after at 1 and 8 threads against before at 1). Four pinned calls (`--pin 2 --rounds 5`)
between gate checks: 8 passes, no failed check, no void, **REBASE count 0**. Elapsed 3,191 s.

Result (`analysis.txt`): `bars: W-1 False, W-2 False, W-3 False`. Weighted streams (9): discard 0.9727 [0.8641, 0.9801] (W-1: at most
0.97), write 0.9806 [0.8660, 0.9835] (W-2: at most 0.98). Per class discard (W-3, at most 1.005): sd 0.9890, 720p 0.9902, 1080p
0.9980, **4k 1.0176 [0.9953, 1.0391]**, the other 23 streams **1.0052 [0.9979, 1.0126]**. All 9 weighted streams are faster; 4k write
is 1.0108 [1.0008, 1.0291].
