# Round 14 gating run: cloud container

before main `28b2b22`, after `perf/h264-weighted-call` `c958743` (decoder code of `0e741c2`). Built on the A/B helper's VM
(`dist`, `--locked`, rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: before `2eac85ca...`, after
`bfa87deb...`). `analyze.py` of `c958743` (`python3 -I analyze.py DIR g`); runner script not published
(rules: `host-gate/HOST-GATE.md` of `measured/host-gate`).

**Host class: cloud container, AMD EPYC family 25 model 1 (Zen 3), 24 cores visible, 8 CPUs requested (`nproc` 8), AVX2 only (no
AVX-512); the round does not depend on an ISA tier.** Reference for this host from three samples five minutes apart
(`reference.json`: max deviation 3.7% and 2.9%, within +-5%). Identity 32 of 32 (after at 1 and 8 threads against before at
1, `identity.txt`). Four pinned calls (`--pin 2 --rounds 5`) between gate checks: 8 passes, one failed before-check (waited),
no call voided, **REBASE count 0**. Elapsed 3,253 s. An earlier attempt on a Zen 4 host (family 25 model 17) failed the
stability check.

Result (`analysis.txt`): `bars: W-1 True, W-2 False, W-3 False`. Weighted streams (9): discard 0.9659 [0.9018, 0.9927] (W-1
asks at most 0.97 with the upper end below 1.00: met), write 0.9810 [0.8974, 1.0089] (W-2 asks at most 0.98: missed by 0.001).
W-3 (discard, each class and the other 23 at most 1.005): sd 0.9933, 720p 0.9775, 1080p 0.9961, other 1.0013, **4k 1.0174
[0.9969, 1.0625]: missed.** Overall after over before: discard 0.9973 [0.9869, 1.0068] (20/32 faster), write 0.9997.
