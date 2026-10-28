# A band's view read where it is kept: gating

Arms: `base` = main `82ad7c31`, `k` = `dc1d7eda` (`perf/h264-band-view-in-place`, now `rejected/h264-band-view`, which has `crates/` of the request's code commit), null arm = `base`; built on a build host with `dist`, `--locked`, `binaries.sha256`; identity of `k` against `base` on the 32 training streams at 1 and 8 threads, checked before timing: 32 of 32
(`identity.txt`). Seed 20261031, FFmpeg 9.0.2 (`0d709a0b...`), harness of main `82ad7c31` (`scripts/h264_parity_gcp.py`). Bars BV-1 to BV-6 were committed before the run (`f055d510`); `analysis/analyze.py` evaluates BV-1 to BV-5 (BV-6's conformance is not here).

Runs (all Compute Engine, STANDARD, `us-central1-a`, Xeon Platinum 8581C @ 2.30 GHz):
- `gating-gcp-8t-train/`: `c4-standard-16`, one thread a core, 32 training streams, `--threads 8 --min-seconds 3`, ten rounds. Not void (reference drift -1.0%), no stream unresolved. 2.704 h, spend bound 3.244 USD.
- `gating-gcp-8t-heldout/`: the same on the 64 held-out streams, five rounds. Not void (drift -0.3%), none unresolved. 2.733 h, 3.279 USD.
- `gating-gcp-1t/`: `c4-standard-8` through `--one-thread-class`, six rounds, `--pin 2`, 32 training streams. Not void; `g.sd` was run twice (its after-check failed, the 720p reference at 1.055 of the gate's reference, `gate-checks.log`; `*.void1` kept). The three references were 1.175, 1.232 and 1.136 s (720p stream), +4.9% and −3.3% of their median: this VM was noisier than the calibration's. 0.962 h, 0.577 USD.
- `gating-gcp-1t-void1/`: the first one-thread run, **void (a host fault)**: no attempt of `g.1080p` passed its gate checks (`verdict.json`: the host ran about 5% slower than its references, 1.202, 1.173 and 1.135 s), so that class has no report. 1.629 h, 0.978 USD. The run above is the re-run on a new VM.
- Two launches at once on the first attempt collided on the VM's name (a timestamp to the second) and created nothing.

Total spend by the bound: **8.078 USD** (the cap set for each launch was met; the four launches' sum is 0.078 over the 8 planned for the job, by the void one-thread run's 0.978).

Result (`analysis/analysis.txt`): **BV-1 pass, BV-2 FAIL, BV-3 pass, BV-4 pass, BV-5 pass** (BV-6 not evaluated here). `k` over `base`, median per-stream ratio (95% bootstrap interval over streams), faster of the streams:

| Bar | measure | result | bar |
|---|---|---|---|
| BV-1 | 8T discard, all 32 | **0.9874** [0.9842, 0.9907] (31/32) | at most 0.990, upper end below 1.000: met |
| BV-2 | 8T discard, sd (8 streams) | **0.9863** [0.9806, 0.9920] (8/8) | at most 0.985: **not met** (by 0.0013) |
| BV-3 | 8T discard 720p, 1080p, 4k; write all 32 | 0.9907, 0.9893, 0.9794; write 0.9897 [0.9864, 0.9919] (30/32) | each at most 1.000, write at most 0.995: met |
| BV-4 | 1T discard, write, all 32 | discard 0.9974 [0.9950, 1.0012] (19/32), write 0.9970 [0.9953, 0.9988] (24/32) | each at most 1.003: met |
| BV-5 | 8T discard, 64 held-out | 0.9877 [0.9858, 0.9891] (55/64) | at most 1.000: met |

Other cells (information): 8T write per class sd 0.9907, 720p 0.9933, 1080p 0.9881, 4k 0.9868; held-out per class discard sd 0.9859, 720p 0.9919, 1080p 0.9837, 4k 0.9884; 1T discard per class sd 0.9981, 720p 1.0014, 1080p 0.9974, 4k 0.9888; the 26 CABAC streams 0.9872 (8T) and 0.9967 (1T). Largest null-arm deviation: 0.6% at eight threads on the training streams, 1.1% on the held-out streams, 1.9% at one thread.
Against FFmpeg 9.0.2, median per-stream ratio and share of streams where ours is faster, base then k: 8T training discard 0.835 (32/32), 0.821 (32/32), write 0.845 (31/32), 0.836 (31/32); 8T held-out discard 0.830 (62/64), 0.816 (62/64), write 0.836 (61/64), 0.826 (63/64); 1T discard 0.968 (22/32), 0.970 (23/32), write 0.943 (26/32), 0.941 (27/32).
Eight-thread ratios are of the Compute Engine host (8 vCPUs, one thread a core), one-thread ratios of the Compute Engine one-thread class.
