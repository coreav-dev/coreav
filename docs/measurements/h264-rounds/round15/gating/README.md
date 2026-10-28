# Round 15 gating run, cloud container

before main `e955517` (decoder code of `678bd69`), after `perf/h264-mb-r15` `7f4348a`. Built on the A/B helper's VM (`dist`,
`--locked`, rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: before `83de8e5e...`, after `143943f3...`).
`analyze.py` of `7f4348a` (`python3 -I analyze.py DIR g t8`); runner script not published.

**Host class: cloud container, AMD EPYC family 25 model 1 (Zen 3), 32 cores visible, 16 CPUs requested (`nproc` 16), AVX2 only;
the round does not depend on an ISA tier.** Reference for this host: three samples five minutes apart (`reference.json`, max
deviation 2.5% and 0.3%). Identity 32 of 32 (after at 1 and 8 threads against before at 1). Eight calls between gate checks
(`g`: four classes, `--pin 2 --rounds 5`; `t8`: four classes, `--threads 8 --rounds 3`, unpinned): 16 passes, no failed
check, no void, **REBASE count 0**. Elapsed 3,544 s.

Result (`analysis.txt`): `bars: R15-1 False, R15-2 False, R15-3 False, R15-4 True`. After over before, one thread pinned:
discard overall 0.9997 [0.9938, 1.0032] (R15-1 asks at most 0.993), write overall 1.0003 [0.9945, 1.0046] (R15-2: 0.995); per class
discard sd 1.0077 [0.9933, 1.0117] (R15-3 asks at most 1.005), 720p 1.0002, 1080p 0.9973, 4k 1.0000. Eight threads, discard, all
32 streams: 0.9895 [0.9781, 1.0093] (R15-4: at most 1.005, met).
