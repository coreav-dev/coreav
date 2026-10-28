# Dispatch-wide round: gating run on the A/B helper's VM

Arms: `main` `e18f85cf` (baseline build), `v3` (main built for x86-64-v3), `after` `perf/h264-dispatch-wide` `ea40d05e` (default features), `safe` (after without the feature `fast`).
Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1); `binaries.sha256` (main `a49a4b18...`, v3 `f7e6595f...`, after `f81c8173...`, safe `6bc27ce0...`, FFmpeg 9.0.2 `0d709a0b...`).
`analyze.py` of `ea40d05e` (`python3 -I analyze.py DIR g gm t8`); runner script not published.

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106, Ice Lake), 8 logical CPUs.** Reference for this host: three samples five minutes apart
(`log.txt`: max deviation 2.7% and 0.7%, inside +-5%). Identity 32 of 32 (after and safe at 1 and 8 threads against main at 1). Twelve calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`,
four arms; `gm`: four pinned, main and after, with `MALLOC_MMAP_THRESHOLD_=131072`; `t8`: four unpinned `--threads 8 --rounds 10`, main, v3 and after): 24 passes and one failed check
(the first before-check, which waited and passed 77 s later), no void, no resume, **REBASE count 0**. Elapsed 7,106 s. Spend: none (local VM).

Result (`analysis.txt`): `bars: WD-1 to WD-8 True`. One thread pinned, discard, median per-stream ratio (95% bootstrap CI): after/main 0.9657 [0.9557, 0.9695] (30/32 faster),
after/v3 1.0011 [0.9954, 1.0027], v3/main 0.9670, after/safe 0.9640; write after/main 0.9616 [0.9537, 0.9677] (32/32). Eight threads, discard: after/main 0.9594 [0.9385, 0.9708] (29/32).
Against FFmpeg 9.0.2 (median per-stream ratio, share of inputs where ours is faster): one thread discard `main` 1.178 (6/32), `after` 1.142 (7/32); write 1.150 (7/32), 1.106 (8/32);
eight threads discard 1.025 (13/32), 0.996 (19/32); write 1.005 (15/32), 0.989 (18/32).
