# Round 20 gating run: an x86-64-v3 build tier

One commit, `1167d267` (`perf/h264-v3-tier`), built twice with `cargo build --locked --profile dist --example decode -p coreav-h264`, one `CARGO_TARGET_DIR` per arm, rustc 1.99.1: **before** with `RUSTFLAGS`
unset (the x86-64 baseline), **after** with `RUSTFLAGS='-C target-cpu=x86-64-v3'` (`binaries.sha256`: before `5997ede9...`, after `245495ac...`; they differ). `analyze.py` of `1167d267` (`python3 -I analyze.py DIR g t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`** (the gate passed before the run
started; no fallback host was needed). Runner (script not published): fresh reference from three samples five minutes apart (max deviation 0.3% and 0.4%), identity 32 of 32 (after at 1 and 8 threads
against before at 1), eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 10`): 17 passes, 3 failed checks (waits and one void), **one voided call**
(`t8.720p`, after-check failed; `events.json`; re-run after the host settled; the voided files are `t8.720p.json.void1` and `.log.void1`), no resume, **REBASE count 0**. Elapsed 4,147 s. Spend: none.

Result (`analysis.txt`): `bars: T-1 True, T-2 True, T-3 True, T-4 True`. After over before, one thread pinned: discard overall 0.9650 [0.9555, 0.9690] (32/32 faster), write 0.9625 [0.9576, 0.9732]; per class discard sd 0.9611, 720p 0.9675,
1080p 0.9701, 4k 0.9350; six CAVLC streams 0.9685. Eight threads discard overall 0.9638 [0.9468, 0.9925] (23/32).
