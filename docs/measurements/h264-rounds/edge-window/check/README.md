# The edge window's runs: a stall check

Diagnosis for "Inter round: the edge window's runs" (`docs/design/h264.md`), on the host class of the profile that
found the stall (`../../profile-82ad7c31/`). It is not a speed claim and moves no bar. The change is too small for a
timing bar of its own (the design note); this check tells whether it removes the stall on the gating host's CPU, and
so whether it is a part a later bundle may take.

## The arms (fixed before the run)

| arm | code |
|---|---|
| `base` | `main` at `82ad7c31` (its decoder crates are those of `main` when this file is committed) |
| `k` | the branch `perf/h264-edge-window-runs`, its code as committed with this file (now `rejected/h264-edge-window-runs`, `d3cbc88a`) |

## Method (fixed before the run)

As `../../band-view/check/` (its `job.sh`, `../../hints-8t/rec_hints.py` unchanged, `../../bundle4/b4.py` for the
tables): Compute Engine `c4-standard-16`, SPOT, PMU `STANDARD`, one thread a core (8 vCPUs on 8 cores), Debian 13,
made, run and deleted by `scripts/gcp_vm_job.py` (`run-gcp.sh`), capped at 1.2 USD. Binaries from `build.sh` (`dist`
with line tables). Placement `p8` only (eight threads unpinned); the 32 training streams at the class frame counts,
`--discard`, two rounds, the arms' order rotated each round; `perf stat` of both arms and of FFmpeg (BtbN 9.0.2, stat
only), `perf record` of both arms in the three groups (`loads`, `stores`, `stalls`). A preempted or crashed VM, or a
failed host check, is a host fault and the run is repeated as is; nothing else is.

## Criteria (fixed before the run)

Cycles a macroblock at p8, mean over the 32 streams; Δ is `k` less `base`. In the profile, `edge_window`'s first load
back from the row it built carries 28.8% of its cycles on the stream where it costs most (`f9e973f44570`, sd), and a
second such load 11.7%; the band view's check put `edge_window` at 14.5 cycles a macroblock over the 32.

| part | criterion | met if |
|---|---|---|
| E1 | `inter.rs:edge_window`'s cycles, all 32 | fall by at least **30%** of `base`'s |
| E2 | the inter stage's cycles, all 32 (a stall moved into the kernels that read the copy would show here) | Δ at most **+10** |
| E3 | every stage's cycles (the sum over `b4.py`'s stages), all 32 | Δ at most **+20** |

Checks (information): each arm's two rounds within 3% of each other on inter cycles a macroblock over the 32;
`edge_window`'s cycles at sd.

What follows: with E1 to E3 met, the code goes to `rejected/h264-edge-window-runs` as a part a later bundle may take,
and this record lands in `main` without it. With any of them missed, the record says so and the code is not a part.

## Files

`build.sh` (the arms' binaries), `run-gcp.sh` (upload and run), `job.sh` (the VM's job).

## Result

Host (`run/host.txt`): Intel Xeon Platinum 8581C, family 6 model 207, 8 CPUs, one thread a core. Every event was
counted and none multiplexed (`run/events.json`). Binaries (sha256 in `host.txt`): `base` `1ba62a47…` (the binary
`../../profile-82ad7c31/` profiled, rebuilt), `k` `e3713036…`. Tables: `run/tables.txt` (`../../bundle4/b4.py`).

| part | `base` | `k` | Δ | met |
|---|---|---|---|---|
| E1, `edge_window`'s cycles a macroblock, all 32 | 14.4 | 9.0 | **−5.4 (−37.5%)** | yes (at least −30%) |
| E2, the inter stage's cycles, all 32 | 482.6 | 477.6 | **−4.9** | yes (at most +10) |
| E3, every stage's cycles, all 32 | 2,433.8 | 2,429.0 | **−4.8** | yes (at most +20) |

Checks, met: inter cycles round by round within 0.1% (`base`) and 0.4% (`k`). At sd the inter stage falls by 15.5
cycles a macroblock and every stage by 17.9 (0.7%); at 720p, 1080p and 4k every stage moves by −1.4, −0.3 and +0.4.
The kernels that read the copy are within 0.6 each (`x86.rs` `interpolate` +0.6, `b1` −0.1, `load` +0.2): no stall
moved into them. The inter stage's instructions a macroblock read 29.3 (2.4%) above `base`'s in the samples, against
fewer in callgrind (`../instructions.txt`); the sampled count of `base` itself differs from the band view's check
(1,195 against 1,204), and this is read as sampling noise, not diagnosed further.

What follows (the plan's rule): E1 to E3 are met, so the code goes to `rejected/h264-edge-window-runs` as a part a
later bundle may take, and this record lands in `main` without it. Information, not a bar: in `perf stat`'s two rounds
the median per-stream wall time over `base` is 0.996 over the 32 (19 of 32 faster; sd 1.002), task-clock 1.002; the
change is below what this resolves, as the design note expected.

## Spend

0.25 USD (upper bound: 0.208 hours the VM existed times 1.20 USD an hour).

Raw data (`rec/`, `stat/`, `rec.log`): `raw.tar.gz` on branch `measured/h264-edge-window-check`, sha256
`f481999b61ed4d1cc449b3431472af94f9e9660055203f0be7aaf99d8159605f`. `python3 -I ../../bundle4/b4.py res .` (with the
two `prof-<arm>` binaries from `build.sh` in `.`) gives `run/tables.txt`.
