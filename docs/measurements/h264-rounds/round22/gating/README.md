# Round 22 gating run: neighbour cache and MbInfo at 60 bytes

before main `75d77dc3`, after `perf/h264-nb-cache` `3a6e4707`. Both `dist`, `--locked`, `RUSTFLAGS` unset, rustc 1.99.1, a separate `CARGO_TARGET_DIR` per arm (`binaries.sha256`: before `29c9e094...`, after `955df15f...`).
`analyze.py` of `3a6e4707` (`python3 -I analyze.py DIR g gm t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`** (gate passed before the run; no fallback host was needed).
Runner (script not published): fresh reference from three samples five minutes apart (max deviation 0.7% and 1.0%), identity 32 of 32 (after at 1 and 8 threads against before at 1), twelve calls between gate checks
(`g`: four pinned `--pin 2 --rounds 5`; `gm`: the same with `MALLOC_MMAP_THRESHOLD_=131072` in the environment of the whole call, so of both decoders and FFmpeg; `t8`: four unpinned `--threads 8 --rounds 10`): 24 passes,
no failed check, no void, no resume, **REBASE count 0**. Elapsed 5,594 s. Spend: none.

Result (`analysis.txt`): `bars: N-1 True, N-2 True, N-3 True, N-4 True` (N-5, output, is the identity and the conformance report). After over before, one thread pinned: discard overall 0.9820 [0.9761, 0.9899] (28/32 faster), write
0.9861 [0.9784, 0.9951]; mapped discard 0.9740 [0.9653, 0.9827] (32/32), mapped write 0.9686; eight threads discard 0.9802 [0.9742, 0.9961] (25/32), write 0.9944.
