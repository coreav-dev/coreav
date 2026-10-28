# Motion round: the reference's lines requested ahead, cheaply: gating run on the Intel 2.90 GHz VM

before main `4c457c91`, after `perf/h264-mc-ahead` `0dda825b`. Both `dist`, `--locked`, rustc 1.99.1, `RUSTFLAGS` unset, a separate `CARGO_TARGET_DIR` per arm (`binaries.sha256`: before `29c9e094...`, after `a48af27d...`). `analyze.py` of `0dda825b`
(`python3 -I analyze.py DIR g t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`.** Runner (script not published): a fresh reference from three samples five minutes
apart (max deviation 2.3% and 4.6%, inside +-5%), identity 32 of 32 (after at 1 and 8 threads against before at 1), eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 10`): 16 passes,
no failed check, no void, no resume, **REBASE count 0** (`events.json` is empty). Elapsed 3,673 s. Spend: none.

Result (`analysis.txt`): `bars: MA-1 True, MA-2 True, MA-3 True, MA-4 True, MA-5 True` (MA-6, output, is the identity and the conformance report). After over before, one thread pinned: discard overall 0.9967 [0.9846, 0.9990] (23/32 faster), write 0.9972
[0.9853, 1.0007]; per class discard sd 1.0007, 720p 0.9975, 1080p 0.9884, 4k 0.9569 [0.8951, 0.9983]. Eight threads discard overall 0.9936 [0.9725, 1.0080] (18/32), write 0.9916.
