# Bundle 2: local timing (the gate before the gating request)

Host: a development VM, Intel Xeon @ 2.90 GHz, family 6 (the helper VM class). Arms `m-bb72` (`base`, `main`
`bb7255cd`), `b2` (the bundle) and `m-bb72-n` (a second copy of `base`); `../binaries.sha256`.

- One thread (`../ab1t.py 6`): CPU time, pinned to CPU 3, six rounds after a warm-up, class frame counts.
  `b2` over `base`: all 32 **0.9848 [0.9761, 0.9917]**, 30 of 32 faster; sd 0.9656, 720p 0.9772, 1080p 0.9897,
  4k 0.9916; the six CAVLC streams 0.9466. Null arm 0.9997 [0.9956, 1.0019].
- Eight threads (`../ab8.py 5`): unpinned, five rounds, discard, class frame counts (the plan named `--min-seconds 3`; this script runs the class frame counts instead, as the inter placement round did).
  Wall time **0.9540 [0.9379, 0.9693]**, 28 of 32 faster; sd 0.9602, 720p 0.9718, 1080p 0.9568, 4k 0.9243.
  Null arm 0.9922 [0.9716, 1.0241].

Both conditions of `../plan.md` for a gating request hold (one thread at most 0.995 with the upper end below 1.000;
eight threads at most 1.000). Spend: 0 USD.
