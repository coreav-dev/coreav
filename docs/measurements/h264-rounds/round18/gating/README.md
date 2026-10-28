# Round 18 gating run, cloud container

before main `051e169`, after `perf/h264-records-kept` `4b3fed7` (rebased onto main `051e169`; no timing was run on an earlier base). Built on the A/B helper's VM (`dist`, `--locked`, rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: before `5c14b670...`,
after `a16d7962...`). `analyze.py` of `4b3fed7` (`python3 -I analyze.py DIR g gm t8`); runner script not published (resumable, preemptible, a per-call environment for the mapped calls).

**Host class: cloud container, AMD EPYC family 25 model 17 (Zen 4), 32 cores visible, 16 CPUs requested, AVX-512 present (active on both arms).** Reference for this
host: three samples five minutes apart (`log.txt`: max deviation 1.9% and 3.5%). Identity 32 of 32 (after at 1 and 8 threads against before at 1). Twelve calls between
gate checks (`g`: four pinned `--pin 2 --rounds 5`; `gm`: the same with `MALLOC_MMAP_THRESHOLD_=131072` in the environment of the whole harness call, so of both decoders **and of
FFmpeg**; `t8`: four unpinned `--threads 8 --rounds 10`): 24 passes, no failed check, no void, no resume, **REBASE count 0** (`events.json`). Elapsed 4,981 s.

Result (`analysis.txt`): `bars: G-1 False, G-2 False, G-3 False, G-4 False, G-5 True`. After over before, one thread pinned: discard overall 1.0061 [1.0040, 1.0122] (8/32
faster), write 1.0070 [1.0008, 1.0128]; discard per class sd 0.9989, 720p 1.0049, 1080p 1.0070, **4k 1.0351 [1.0208, 1.0620]**. Mapped run: discard overall 0.8630 [0.8074, 0.9305]
(29/32), write 0.8543; the mapped environment slows `before` much more than `after` (ratio to FFmpeg 1.428 against 1.186 unmapped for `before`, 1.200 against 1.198 for `after`).
Eight threads: discard overall 1.0028 [0.9983, 1.0102], 720p 1.0154 [1.0005, 1.0279].
