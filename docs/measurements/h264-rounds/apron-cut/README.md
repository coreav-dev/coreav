# The aprons' rows asked for ahead (apron cut)

The round of `docs/design/h264.md`, "Threading round: the aprons' rows asked for ahead", after probe AP
(`../apron-copies/`). Bars AC-1 to AC-5 are in that section, committed before any timing.

- `identity.txt`: `k` byte-identical to `base` on the 32 training streams at 1 and 8 threads (AC-5).
- `conformance/`: 97 of 97 at 1, 2 and 8 threads, detected and scalar kernels, on `k` (AC-5).
- `ab8.py`, `local-8t.txt` (`.json`): the local eight-thread timing that the request rule reads; `local-8t-arms.txt`:
  a second local run with the information arms (within the host's noise).
- `run-1t.sh`, `t1/` (`summary.txt` by `sumx.py`): one thread pinned, six rounds, on the development VM (AC-4).
- `run-gcp.sh`, `gate3/`: the Compute Engine run (AC-1 to AC-3, and the information arms); `gate1.void1/` and
  `gate2.void1/`: two attempts that created no VM (C4 quota taken in `us-central1` and `us-east4`).

Result: `k` 1.0006 [0.9982, 1.0026] at eight threads (sd 1.0088), AC-1, AC-2 and AC-3 (write 1.0019) missed, rejected (code on
`rejected/h264-apron-cut`); the information arm `kw` (`prefetchw`) 0.9904 [0.9882, 0.9915], sd 0.9829, 32 of 32
faster, which the next round builds. Spend 2.75 USD (bound).
