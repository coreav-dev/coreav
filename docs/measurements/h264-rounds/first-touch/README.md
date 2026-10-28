# Probe PF: a new band's first touch twice, at eight threads

Plan and rule, committed before the run: `plan-probe.md`. Probe code: branch `probe/r39-first-touch` (`f96ef2b8`),
`base` = `main` at `13e4dfc4`. Both are byte-identical on the 32 training streams at 1 and 8 threads
(`identity-probe.txt`, `binaries.sha256`).

## Run

`pf-run1/`: `scripts/h264_parity_gcp.py` at its defaults (eight threads, unpinned), `c4-standard-16` in
`us-central1-a`, one thread a core, 32 training streams, 5 rounds, seed 20261027. The run is not void
(`pf-run1/verdict.json`): the reference moved +4.7% between the first call and the last (the void limit is 5%), and
no stream was unresolved. Null arm: median per stream −0.05% (discard) and −0.09% (write), at most 1.1% either side
(`pf-run1/null.txt`, `null.py`). VM 1.37 h, spend bound 1.64 USD.

## Result (`pf-run1/pf-summary.txt`, `pf.py`)

| `pf2` / `base`, eight threads | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| discard | **1.0382** [1.0295, 1.0779]; `pf2` faster on 1 of 32 | 1.0185 | 1.0593 | 1.0320 | **1.4162** |
| write | 1.0395 [1.0274, 1.0846] | 1.0185 | 1.0414 | 1.0327 | 1.3574 |

**PF-1 is met** (at least 1.015 over the 32, and in every class): a round on the first touch opens, with its own
design note section and bars committed before its timing.

How large the first touch is in these runs (`rss.py`, `rss.txt`, the development VM, information): on the 4k stream
`3b1b441e23ae` at eight threads, `base` takes 43,000 minor page faults in a decode of 0.12 s wall and 0.60 s CPU,
and `pf2` 85,000, at 0.17 s wall and 0.67 s CPU. The 150-frame 4k runs are short, so the pictures made new while
the pipeline fills weigh on them. `pf2` does its second touch on the caller's thread. Relative to the run that
adds 42% to the wall time and 12% to the CPU time, so the touch is likely on the critical path, holding back the next
picture's dispatch. That was not measured, and these are single runs on the development VM, not the bar host. If so,
the probe over-prices a change that keeps the touch but moves it. In `base` the caller's thread already does part of a new picture's first touch
itself. `BandData::fit` fills a new band's chroma with 128 and its motion and heads, while the luma comes zeroed
from the allocator and faults on the workers.

Against FFmpeg 9.0.2 at eight threads on this host (information): `base` 0.9322 [0.8746, 0.9683], 24 of 32
faster (sd 0.9758, 720p 0.9355, 1080p 0.9811, 4k 0.7539).
