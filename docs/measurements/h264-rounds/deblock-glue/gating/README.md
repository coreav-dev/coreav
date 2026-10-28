# Deblocking glue round: gating run on a cloud container

before main `814eb2a1` (main did not move), after `perf/h264-deblock-glue` `33d4f551`. Built on the A/B helper's VM (`dist`, `--locked`,
rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: before `f05e63e5...`, after `3a5e8d45...`). `analyze.py` of `33d4f551`
(`python3 -I analyze.py DIR g t8`); runner script not published.

**Host class: cloud container, AMD EPYC family 25 model 1 (Zen 3), 32 cores visible, 16 CPUs requested, AVX2 only; the round is decoder glue
with no ISA tier.** Reference for this host: three samples five minutes apart (`log.txt`: max deviation 0.9% and 2.2%). Identity 32 of 32
(after at 1 and 8 threads against before at 1). Eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8
--rounds 3`): 16 passes, no failed check, no void, no resume, **REBASE count 0** (`events.json`). Elapsed 3,193 s.

Earlier attempts on the A/B helper's own VM (`local-attempts/`; first one lost to a VM suspend, then a host that drifted 5 to 7% from its reference and
failed the stability check or voided a call three times) timed nothing usable and are not used.

Result (`analysis.txt`): `bars: D-1 True, D-2 True, D-3 True, D-4 True`. After over before: one thread pinned discard overall 0.9941 [0.9909, 0.9979]
(24/32 faster), write overall 0.9952 [0.9926, 0.9973]; per class discard sd 0.9936, 720p 0.9948, 1080p 0.9919, 4k 1.0033 [0.9938, 1.0505]. Eight threads
discard overall 0.9836 [0.9706, 0.9891] (24/32).
