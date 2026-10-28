# Round 19 gating run: the CAVLC residual in one pass

before main `27b504ab` (decoder code of `051e169`), after `perf/h264-cavlc-fused` `9c8067c5` (the bars commit; code `1186b7c9`). Built on this VM (`dist`, `--locked`, rustc 1.99.1;
`binaries.sha256`: before `5c14b670...`, after `05474419...`). `analyze.py` of `9c8067c5` (`python3 -I analyze.py DIR g t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"` (gate passed before the run
started; no fallback host was needed).** Runner (script not published): fresh reference from three samples five minutes apart (max deviation 0.5% and 0.1%),
identity 32 of 32 (after at 1 and 8 threads against before at 1), eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 10`): 16 passes, no failed check,
no void, no resume, **REBASE count 0** (`events.json`). Elapsed 3,712 s. Spend: none.

Result (`analysis.txt`): `bars: V-1 True, V-2 True, V-3 True, V-4 True, V-5 True`. After over before, one thread pinned: discard overall 0.9923 [0.9850, 0.9935] (26/32 faster), write overall 0.9938 [0.9857, 1.0009];
over the six CAVLC streams discard 0.9228 [0.8846, 0.9504], write 0.9283; per class discard sd 0.9687, 720p 0.9917, 1080p 0.9924, 4k 0.9890. Eight threads discard overall 0.9940 [0.9862, 1.0007] (21/32), CAVLC streams 0.8880.
