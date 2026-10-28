# Round 21 gating run: two entropy loops

before main `40f29a6d`, after `perf/h264-entropy-small` `fb4d3787` (code `1395e3b8`). Both `dist`, `--locked`, `RUSTFLAGS` unset, rustc 1.99.1, a separate `CARGO_TARGET_DIR` per arm
(`binaries.sha256`: before `16bc3dcb...`, after `7c97c06d...`). `analyze.py` of `fb4d3787` (`python3 -I analyze.py DIR g t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`** (gate passed before the run
started; no fallback host was needed). Runner (script not published): fresh reference from three samples five minutes apart (max deviation 1.7% and 1.1%), identity 32 of 32 (after at 1 and 8 threads against
before at 1), eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 10`): 16 passes, no failed check, no void, no resume, **REBASE count 0**.
Elapsed 3,653 s. Spend: none.

Result (`analysis.txt`): `bars: E-1 True, E-2 False, E-3 True, E-4 False`. After over before, one thread pinned: discard overall 0.9982 [0.9926, 1.0015] (18/32 faster), write overall 0.9989; over the six CAVLC streams discard
0.9849 [0.9798, 0.9923]; over the thirteen mvd-heavy CABAC streams discard 0.9996 [0.9947, 1.0025]; the other CABAC streams 1.0012. Eight threads discard overall 1.0005 [0.9942, 1.0087], the non-mvd CABAC streams 1.0116 [1.0033, 1.0168].
