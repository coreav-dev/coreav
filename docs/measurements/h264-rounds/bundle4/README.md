# Bundle 4: the banded hints and the aprons' rows owned, a stall check before timing

Diagnosis for bundle 4 (`docs/design/h264.md`, "Bundle 4: a banded picture's lines and its aprons' rows owned
ahead"), on the host class of the miss probe (`../miss-8t/`) and of the banded hints' probe (`../hints-8t/`); not a
speed claim, and it moves no bar. The bundle puts two rejected rounds' code into one `SliceDecoder::hints` byte and
one out-of-line request: the banded hints (`rejected/h264-banded-hints`, `../hints-8t/`) and the aprons' rows owned
ahead (`rejected/h264-apron-keep`, "Threading round: the aprons' rows owned ahead, the decoder's layout kept"). This
check tells, before any timing, that the merged code still removes the stalls each part removed alone.

## The arms (fixed before the run)

| arm | code |
|---|---|
| `base` | `main` at `41c62136` |
| `H` | the banded hints alone, `rejected/h264-banded-hints` at `bb9547a6` (its code is that of `main` at `564a0c20`, whose crates `41c62136` did not change, with the round's) |
| `k` | bundle 4, `perf/h264-bundle4` at `db25cc6a` |

## Method (fixed before the run)

As `../hints-8t/` (its `rec_hints.py`, unchanged): Compute Engine `c4-standard-16`, SPOT, PMU `STANDARD`, one thread
a core (8 vCPUs on 8 cores), Debian 13, made, run and deleted by `scripts/gcp_vm_job.py` (`run-gcp.sh`), capped at
1.2 USD. Binaries from `build.sh` (`dist` with line tables, one target directory each). Placement `p8` only (eight
threads unpinned); the 32 training streams at the class frame counts, `--discard`, two rounds, the arms' order rotated
each round; `perf stat` of every arm and of FFmpeg (BtbN 9.0.2, stat only), `perf record` of every CoreAV arm in the
three groups (`loads`, `stores`, `stalls`). `b4.py` makes the tables (`../hints-8t/hints.py`, with every stage's cycles
by class). A preempted or crashed VM, or a failed host check, is a host fault and the run is repeated as is; nothing
else is.

## Criteria (fixed before the run)

Per macroblock, mean over the streams, at p8; Δ is one arm less another. Every stage's cycles are the sum over
`b4.py`'s stages (the picture's lines are written by the entropy, macroblock, inter, reconstruction and deblocking
stages, and the aprons by the copies the deblocking stage makes).

| part | criterion | met if |
|---|---|---|
| P1 (the banded hints kept) | `k` − `base`, all 32 | every stage's cycles fall by at least 20 (the round alone: −38.9 from its `base`), and inter `resource_stalls.sb` falls |
| P2 (the aprons' part adds) | `k` − `H`, sd | every stage's cycles fall by at least 20 (the aprons' round alone: 0.9871 of sd's time at eight threads, about 1.3%) |

Checks: each arm's two rounds within 3% of each other on inter cycles a macroblock over the 32; inter instructions a
macroblock of `k` within 5 of `H`'s over the 32 (the merged request adds nothing to the inter path).

What follows: with P1 and P2 met, bundle 4 goes to timing as it is, with the bars in the design note. With either
missed, the bundle does not go to timing; the missing part is diagnosed first (or left out, with the bars set again
before any timing).

## Files

`build.sh` (the arms' binaries), `run-gcp.sh` (upload and run), `job.sh` (the VM's job), `b4.py` (tables).

## Result

Host (`run/host.txt`): Intel Xeon Platinum 8581C, family 6 model 207, 8 CPUs, one thread a core, kernel and perf
6.12. Every event was counted and none multiplexed (`run/events.json`). Binaries (sha256, in full in `host.txt`):
`base` `a923a4a7…`, `H` `6fc7ebfb…`, `k` `3f0861a1…`. Tables: `run/tables.txt` (`b4.py`), `run/b4.json`.

Every stage's cycles a macroblock at p8, and inter's events (all 32; sd):

| arm | every stage | sd | inter cycles | inter `stalls_l1d_miss` | inter `resource_stalls.sb` | inter instructions |
|---|---|---|---|---|---|---|
| `base` | 2,543.2 | 2,841.6 | 518.0 | 98.3 | 38.2 | 1,158.4 |
| `H` | 2,500.2 | 2,788.7 | 491.1 | 75.1 | 18.4 | 1,207.7 |
| `k` | 2,483.4 | 2,740.8 | 520.5 | 75.6 | 18.9 | 1,223.9 |

- **P1, met.** `k` − `base` over the 32: every stage's cycles −59.8 a macroblock (bar 20); inter
  `resource_stalls.sb` −19.4 (`H` alone −19.8), `stalls_l1d_miss` −22.6 (`H` −23.2).
- **P2, met.** `k` − `H` at sd: every stage's cycles −47.9 (bar 20): the stage `other`, where the aprons' copies are
  counted, −115.3 (1,012.5 → 897.2), and inter, where their hints are (below), +67.9.
- **Checks.** Rounds within 1.4% of each other on inter cycles over the 32 (met). Inter instructions of `k` are 16.2
  above `H`'s over the 32 (66.4 at sd), so the second check (at most 5) is **missed**: its premise was wrong. The
  classifier counts every hint (`prefetch.rs`) as inter, wherever it is issued, and the aprons' hints, which the
  plan's premise left out, are `request_apron`'s ownership hints over 22 luma and 10 chroma rows in each band's first
  row, which at sd is every row. They are the `for_rows` walk's +29.7 cycles a macroblock over `H`'s (`run/tables.txt`,
  inter functions), the aprons' round's own cost and not the merge's; the reference and destination hints cost what
  they cost in `H`. A check is information and moves no bar.

What follows (the plan's rule): P1 and P2 are met, so bundle 4 goes to timing as it is. Information, not a bar: in
`perf stat`'s two rounds the median per-stream wall time over `base` is 0.981 for `k` (29 of 32 faster; sd 0.979,
720p 0.982, 1080p 0.990, 4k 0.989) and 0.990 for `H`; `base` over FFmpeg 0.810 (32 of 32 faster).

## Spend

0.34 USD (upper bound: 0.287 hours the VM existed times 1.20 USD an hour).

Raw data (`rec/`, `stat/`, `rec.log`): `raw.tar.gz` on branch `measured/h264-bundle4-check`, sha256
`171b55e33834652aff22e2b4bb26b3cf516bc1b38126e67058cd566773e6a136`. `python3 -I b4.py res .` (with the three
`prof-<arm>` binaries from `build.sh` in `.`) gives `run/tables.txt`.
