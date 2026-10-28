# A band's view read where it is kept: a stall check before timing

Diagnosis for "Inter round: a band's view read where it is kept" (`docs/design/h264.md`), on the host class of the
profile that found the cut (`../../profile-82ad7c31/`); not a speed claim, and it moves no bar. The local A/B
(`../local/`) runs on a VM of four cores with two threads each and no PMU, which did not show bundle 4's eight-thread
gain either; this check tells, before the gating run, whether the change removes on the gating host's CPU the cycles
the profile attributed to the band view.

## The arms (fixed before the run)

| arm | code |
|---|---|
| `base` | `main` at `82ad7c31` |
| `k` | the branch `perf/h264-band-view-in-place`, its code as committed with this file (now `rejected/h264-band-view`, `dc1d7eda`) |

## Method (fixed before the run)

As `../../bundle4/` (its `job.sh` with these two arms, `../../hints-8t/rec_hints.py` unchanged, `../../bundle4/b4.py`
for the tables): Compute Engine `c4-standard-16`, SPOT, PMU `STANDARD`, one thread a core (8 vCPUs on 8 cores),
Debian 13, made, run and deleted by `scripts/gcp_vm_job.py` (`run-gcp.sh`), capped at 1.2 USD. Binaries from
`build.sh` (`dist` with line tables). Placement `p8` only (eight threads unpinned); the 32 training streams at the
class frame counts, `--discard`, two rounds, the arms' order rotated each round; `perf stat` of both arms and of
FFmpeg (BtbN 9.0.2, stat only), `perf record` of both arms in the three groups (`loads`, `stores`, `stalls`). A
preempted or crashed VM, or a failed host check, is a host fault and the run is repeated as is; nothing else is.

## Criteria (fixed before the run)

Every stage's cycles a macroblock (the sum over `b4.py`'s stages), mean over the streams, at p8; Δ is `k` less
`base`. The profile's shares put the band view's cycles at about 4.5% of CoreAV's at sd and 2.4% to 3.7% in the
other classes, of which the cut should remove most:

| part | criterion | met if |
|---|---|---|
| V1 | Δ, all 32 | at most **−30** |
| V2 | Δ, sd | at most **−50** |
| V3 | Δ, at 720p, at 1080p and at 4k | at most **+10** each (no class pays for another) |

Checks (information): each arm's two rounds within 3% of each other on inter cycles a macroblock over the 32; inter
instructions a macroblock of `k` below `base`'s over the 32 (the lookup's instructions removed).

What follows: with V1 to V3 met, the round goes to its gating run with the bars in the design note. With any of them
missed, it does not; the miss is diagnosed first.

## Files

`build.sh` (the arms' binaries), `run-gcp.sh` (upload and run), `job.sh` (the VM's job).

## Result

Host (`run/host.txt`): Intel Xeon Platinum 8581C, family 6 model 207, 8 CPUs, one thread a core. Every event was
counted and none multiplexed (`run/events.json`). Binaries (sha256 in `host.txt`): `base` `1ba62a47…` (the binary
`../../profile-82ad7c31/` profiled, rebuilt), `k` `6db0dd56…`. Tables: `run/tables.txt` (`../../bundle4/b4.py`).

Every stage's cycles a macroblock at p8:

| arm | sd | 720p | 1080p | 4k | all 32 |
|---|---|---|---|---|---|
| `base` | 2,596.5 | 2,349.3 | 3,349.5 | 1,387.3 | 2,420.6 |
| `k` | 2,544.7 | 2,313.7 | 3,315.9 | 1,357.2 | 2,382.9 |
| Δ | **−51.8** | −35.6 | −33.6 | −30.1 | **−37.8** |

- **V1, met:** −37.8 over the 32 (bar −30).
- **V2, met:** −51.8 at sd (bar −50).
- **V3, met:** −35.6 at 720p, −33.6 at 1080p, −30.1 at 4k (bar +10 each).
- **Checks, met.** Inter cycles a macroblock round by round within 1.9% (`base`) and 0.0% (`k`); inter instructions of
  `k` 101.8 a macroblock below `base`'s over the 32.

Where (all 32, cycles a macroblock): inter −52.2, of which `reference_band` −38.3 (gone) and `predict_fast_body` −37.9
(the load-back gone, the inlined lookup added), the other inter functions within +2.2 each; the MB layer +17.0 (not
diagnosed; the classifier puts `inter_pred` and `motion_compensate`, which call `predict_fast`, there), entropy −5.3. Store-buffer and L1 miss counts are within noise; the stall cycles fall
(`cycle_activity.stalls_total` −15.1).

What follows (the plan's rule): V1 to V3 are met, so the round goes to its gating run as it is. Information, not a
bar: in `perf stat`'s two rounds the median per-stream wall time over `base` is 0.988 for `k` (25 of 32 faster; sd
0.985, 720p 0.996, 1080p 0.992, 4k 0.977), task-clock 0.982; `base` over FFmpeg 0.832, `k` 0.823.

## Spend

0.25 USD (upper bound: 0.209 hours the VM existed times 1.20 USD an hour).

Raw data (`rec/`, `stat/`, `rec.log`): `raw.tar.gz` on branch `measured/h264-band-view-check`, sha256
`721b1f27bdb7f9e8c09cee5ab4c7a2585ede0686016db3bb10159dbc857b3785`. `python3 -I ../../bundle4/b4.py res .` (with the
two `prof-<arm>` binaries from `build.sh` in `.`) gives `run/tables.txt`.
