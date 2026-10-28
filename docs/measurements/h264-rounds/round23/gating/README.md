# Round 23 gating run: the CAVLC level loop

before main `ddb34e92`, after `perf/h264-cavlc-levels` `1a282b6c` (the branch merged with `ddb34e92`). Main moved on to `e18f85cf` before the run, with changes in `crates/` (3 files, +137 lines, from an earlier change); `before` stays `ddb34e92`, the base the
branch was merged with, so the pair differs only in this round's change. Both `dist`, `--locked`, `RUSTFLAGS` unset, rustc 1.99.1, a separate `CARGO_TARGET_DIR` per arm (`binaries.sha256`: before `955df15f...`, after `cb8b3a86...`). `analyze.py` of `1a282b6c` (`python3 -I analyze.py DIR g t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`.** Runner (script not published): fresh reference from three samples five minutes apart (max deviation 1.0% and 0.1%), identity 32 of 32
(after at 1 and 8 threads against before at 1), eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 15`; no gm call): 16 passes, no failed check, no void, no resume, **REBASE count 0**. Elapsed 4,125 s. Spend: none.

Result (`analysis.txt`): `bars: L-1 False, L-2 False, L-3 True`. After over before, one thread pinned: discard overall 0.9902 [0.9817, 0.9958] (25/32 faster), write overall 0.9957 [0.9906, 1.0019]; per class discard sd 0.9863, 720p 0.9909, 1080p 0.9906, 4k 0.9900; the six CAVLC streams 0.9797.
Eight threads discard overall 0.9849 [0.9814, 0.9979] (22/32), write 0.9943 [0.9728, 1.0207]; the 1080p write at one thread is 1.0332 [0.9961, 1.0619].
