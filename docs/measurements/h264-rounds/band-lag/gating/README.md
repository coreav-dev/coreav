# Band-lag threading round: gating run on a cloud container

Base main `9cc143d3`, a `9d7aca35` (decoder code of `544f954e`; `405ef0ef` adds scripts only). Both binaries built on the A/B
helper's VM (`dist`, `--locked`, rustc 1.99.1) and copied unchanged into one container; `binaries.sha256` (a `78c02609...`,
the same as the kernel helper's build). `analyze.py` and `cpu8.py` of `405ef0ef`; the runner script is not published (rules in `host-gate/HOST-GATE.md`).

**Host class: cloud container, AMD EPYC family 25 model 1 (Zen 3), 32 cores visible (one thread each), 16 CPUs requested
(`nproc` 16, no cgroup `cpu.max`), AVX2 only (no AVX-512).** Both arms and FFmpeg 9.0.2 in the same calls; eight-thread calls
therefore ran on 16 requested cores, not throttled by an 8-CPU quota.

Gate: a fresh reference for this host from three samples five minutes apart (`reference.json`: max deviation 0.5% and 1.6%).
Identity (L5): 32 of 32 streams, `decode-a` at 1, 2, 3 and 8 threads against `decode-base` at 1 (`identity.txt`). Then the `t` run
(`--threads 1,8`, unpinned, `--rounds 5`) and the `p` run (`--pin 2`, `--rounds 5`), each call between two gate checks (16
checks passed, no call voided), then `analysis.txt`, then `cpu8` (information, not gated). **REBASE count: 0.** Elapsed 6,035 s
in this container. Earlier attempts of the same script: one on a Zen 4 host (family 25 model 17) failed the stability check
(deviation 17% and 21%); one on a Zen 3 host passed it and ran the `t` sd and 720p calls, then the host got about 8% faster
for over 25 minutes (reference-relative ratios 0.91 to 0.93) and the run was stopped before any
bar-relevant result was kept; the script now has the level-shift rule (six steady failed before-checks become a new
reference, between calls only).

Result (analyze.py bars): **L1 passes, L2 passes, L3 passes, L4 passes**. 8 threads, unpinned, a over base: discard overall
0.9045 [0.8710, 0.9395] (29/32 faster), sd 0.7181 [0.6810, 0.8710], 720p 0.9072, 1080p 0.9380, 4k 0.9390 [0.8601, 0.9961];
write overall 0.9331, sd 0.7374, 720p 0.9242, 1080p 0.9511, 4k 0.9461 [0.9353, 1.0050]. One thread, pinned: discard overall
0.9994 [0.9937, 1.0052], per class 0.9984, 1.0001, 0.9973, 1.0084 (4k); write overall 1.0013, per class 1.0013, 0.9962, 1.0057, 0.9990.
Scaling against FFmpeg's (information): base 0.949 discard overall, a 0.830 (target 1.10). CPU time at 8 threads, a over base
(median): sd 1.119, 720p 0.994, 1080p 0.960, 4k 0.951 (`cpu8.*.txt`).
