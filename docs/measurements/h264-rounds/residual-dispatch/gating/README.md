# Entropy round: the residual's block dispatch: gating run on the A/B helper's VM

Arms: `base` = main `e2271b3a`, `k` = `fded8560` (`perf/h264-residual-dispatch`), `knofast` = `k` built with `--no-default-features` (information, one-thread call only). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1), `binaries.sha256`
(base `fde0d5d0...`, k `3cd2a75f...`, knofast `f107bbc9...`). `run-ab-helper.sh` and `analyze.py` (written for this round from its bars RD-1 to RD-6; the branch has none); harness `h264_parity_acf3ee2f.py` (main at `acf3ee2f`, which has changes A and B but not change C);
FFmpeg 9.0.2 `0d709a0b...`. Method as the bars section states: `--shuffle-seed 20261026 --null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-every 4`, one call per class: `g` (32 training streams, one thread, `--pin 2 --rounds 20`, arms base, k, knofast),
`t8` (eight threads unpinned, `--rounds 10 --min-seconds 3`, in-call gate one thread on core 2 at 900 frames repeated to 3 s), `h` (the 64 held-out streams, one thread pinned, 5 rounds).

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (`k` and `knofast` at 1 and 8 threads against `base` at 1). Twelve calls between gate checks: `hostgate.log` 25 passes and 3 failed checks (each waited and passed),
**one void call**: `g.1080p`, an after-check failure (a before-check of 1.131 of its reference), run again and passed (`g.1080p.*.void1` kept), **REBASE count 0**. Spend: none (local VM). One-thread calls ended with unresolved streams that the bars' method keeps in the medians and lists
(`analysis.txt`): `g.sd` 6 of 8 (four of its in-call gate samples drifted: the gate's baseline of one sample, before an earlier change), `g.1080p` 1, `g.4k` 2; the eight-thread calls none; the held-out calls none, with 5, 4, 0, 1 streams timed again.
The eight-thread arms ran on this VM as the bars were committed (8 vCPUs with SMT; the same-binary median noise there is about 1.5% either side).

Result (`analysis.txt`): `RD-1 pass, RD-2 pass, RD-3 pass, RD-4 pass, RD-5 pass` (RD-6, output, is read from `identity.txt` and the conformance reports). `k` over `base`, median per-stream ratio (95% bootstrap interval), faster of the streams:

| Call | discard, all | write, all | per class, discard (sd, 720p, 1080p, 4k) |
|---|---|---|---|
| `g`, 1 thread, 32 training | 0.9861 [0.9813, 0.9899] (26/32) | 0.9849 [0.9791, 0.9885] (28/32) | 0.9898, 0.9850, 0.9727, 0.9831 |
| `t8`, 8 threads, 32 training | 0.9848 [0.9759, 0.9942] (22/32) | 0.9835 [0.9710, 0.9891] (27/32) | 0.9901, 0.9912, 0.9720, 0.9832 |
| `h`, 1 thread, 64 held-out | 0.9864 [0.9834, 0.9895] (56/64) | 0.9878 [0.9864, 0.9907] (54/64) | 0.9915, 0.9849, 0.9829, 0.9861 |

Against FFmpeg 9.0.2 (median per-stream ratio, base then k; share where ours is faster): `g` discard 1.101 (10/32) then 1.090 (10/32), write 1.074 (11/32), 1.055 (13/32); `t8` discard 0.982 (18/32), 0.972 (20/32), write 0.993 (17/32), 0.973 (23/32); `h` discard 1.091 (18/64), 1.074 (20/64), write 1.055 (21/64), 1.044 (22/64).
`knofast` over `k` at one thread (information): discard 1.0267 [1.0230, 1.0321], write 1.0235 [1.0213, 1.0294]; `knofast` against FFmpeg: discard 1.123 (8/32 faster), write 1.069 (10/32).
