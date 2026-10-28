# Eight-thread inter: where the extra cycles go (miss probe)

Diagnosis only, on an Emerald Rapids VM with a PMU (not the bar machine's class), so nothing here is a speed claim or
moves a bar. The question comes from the PMU profile at `ae609547` (`../profile-ae609547-pmu/`): at eight threads
inter prediction is CoreAV's first stage by excess over FFmpeg in every class (+0.074 of FFmpeg's cycles over the 32),
its inter cycles a macroblock grow by about 40% from one thread to eight against FFmpeg's 15%, most of it lower IPC
(3.15 to 2.48), in `inter.rs` `predict_fast_body`, `reference_band` and the luma kernel's stores (`x86.rs` `store`),
and CoreAV is more memory-bound than FFmpeg at eight threads only. An earlier change sampled cycles and instructions only. Its
reading, to confirm or refute here: **cold band buffers, and reference rows written by another core**.

## Method (fixed before the run)

- Host: as an earlier change. Compute Engine `c4-standard-16`, SPOT, PMU `STANDARD`, one thread a core (8 vCPUs on 8 cores), Debian
  13, made, run and deleted by `scripts/gcp_vm_job.py` (`run-gcp.sh`), capped at 2.4 USD (2 hours at an upper bound of
  1.20 USD an hour); the probe's budget is 5 USD. A preempted or crashed VM, or a failed host check, is a host fault
  and the run is repeated as is; nothing else is.
- The `ENHANCED` PMU (L3 events, among them `mem_load_l3_hit_retired.xsnp_*`, which tell a line forwarded from another
  core from an L3 hit) is offered only on C4 machines of 144 or 288 vCPUs (Compute Engine's PMU overview), another
  CPU generation and size than the bars' host. So the probe tells cores apart by placement instead (below), with core
  events only.
- Decoders, all `dist` with line tables (`build.sh`): `main` = CoreAV `main` at `df389085`; `ft` = `df389085` with
  an earlier change's code commit (`68c8f059`, `picture.rs` only: a new picture's bands first touched on the worker that decodes
  them), so that the probe shows what an earlier change already changes; `ffa` = FFmpeg 9.0.2's attribution build (function
  symbols only, built on the VM by `job.sh` from the pinned tarball, make's log deleted unread, source tree, tarball
  and log deleted right after `make` and before any profiling; FFmpeg is read at function level only).
- Placements (`rec_miss.py`):
  - `p1`: one thread pinned to CPU 3 (`--threads 1`, `-threads 1`);
  - `p81`: eight threads (`--threads 8`, `-threads 8`), every thread pinned to CPU 3. The banded decode's work and
    footprint (bands, aprons, views, the blocks that leave a band) and every write and read on one core: a row another
    worker just wrote is in this core's caches;
  - `p8`: eight threads unpinned on the eight cores, as the eight-thread speed bars run.
  `main` and `ffa` run in all three, `ft` in `p8`.
- The 32 training streams at the class frame counts, CoreAV `--discard` against `ffmpeg -f null`, two rounds. For
  every decoder and placement: `perf stat` of cycles, instructions and task-clock with the wall time; `perf record`
  (2,000 samples a second an event) of three groups of at most four programmable events, so none is multiplexed:
  - `loads`: cycles, instructions, `mem_load_retired.l1_miss`, `mem_load_retired.l2_miss`, `mem_load_retired.fb_hit`,
    `l1d_pend_miss.fb_full`;
  - `stores`: cycles, `l2_rqsts.all_rfo`, `l2_rqsts.rfo_miss`, `resource_stalls.sb`, `mem_inst_retired.all_stores`;
  - `stalls`: cycles, `cycle_activity.stalls_total`, `cycle_activity.stalls_l1d_miss`,
    `cycle_activity.stalls_l2_miss`, `machine_clears.memory_ordering`.
  The probe step drops an event the VM does not count and counts each group together, to show none is
  multiplexed (`out/events.json`); an earlier host of this class (`../pmu-profile/run1/events.json`) counted all of
  them but `mem_load_retired.fb_hit` and `machine_clears.memory_ordering`, which it did not try.
- `miss.py` classifies samples as `../profile-ae609547-pmu/stages_pmu.py` does (CoreAV by inline chain, FFmpeg by
  function symbol) and reports each event a macroblock in inter, the mean over the streams of a class and over the 32,
  per decoder and placement, and CoreAV's inter functions. On change A's raw records it gives change A's inter instructions a
  macroblock exactly (1,033 and 1,137; FFmpeg 993 and 1,016); its cycles a macroblock (318 and 485; FFmpeg 289 and
  347) are means per stream, which an earlier change's table did not use.

## Questions and criteria (fixed before the run)

Inter cycles a macroblock, mean over the 32 streams. Δ_total = p8 − p1 (the eight-thread growth), Δ_band = p81 − p1
(banding on one core), Δ_cross = p8 − p81 (the cores). "Excess" is CoreAV's Δ less FFmpeg's in the same columns.

| | question | reading |
|---|---|---|
| Q1 | is the growth the cores' | **confirmed** if CoreAV's Δ_cross is at least 0.5 of its Δ_total and its excess in Δ_cross is at least 30 cycles a macroblock; **refuted** if Δ_cross is below 0.25 of Δ_total or the excess below 15; otherwise partly. If Δ_band is at least 0.5 of Δ_total, banding's own work leads instead |
| Q2 | loads or stores | within CoreAV's inter excess in Δ_cross: stores = `resource_stalls.sb` cycles (with `l2_rqsts.rfo_miss`, `l2_rqsts.all_rfo` a macroblock), loads = `cycle_activity.stalls_l1d_miss` cycles (with `mem_load_retired.l1_miss`, `l2_miss`, `fb_hit`). The kind with the larger stall excess leads; it is the cause if its stall excess is at least 0.5 of the cycles excess in Δ_cross |
| Q3 | which functions | Δ_cross by CoreAV function (`predict_fast_body`, `reference_band`, `x86.rs` `store`, the rest); reported, no bar |
| Q4 | Change A's share | `ft` − `main` at p8, inter cycles a macroblock; if it removes at least 0.5 of the excess in Δ_cross, the target is change A's and no cut of this probe's is built for it |

Checks: CoreAV's inter instructions a macroblock at p81 within 3% of p8 (the two placements do the same work; if not,
Q1 is read on cycles less the instruction difference at p1's cycles per instruction, and that is stated); each arm's
two rounds within 3% of each other on inter cycles a macroblock over the 32.

What follows each reading, so that the result cannot pick the cut: stores leading points at the band's own
destination rows (written ahead with `rows_for_write` a macroblock row early, or the pool handing out the picture
released last); loads leading at the reference rows (requested further ahead at eight threads, or workers kept on
their cores); banding leading at the band work itself (views, aprons, blocks that leave a band). Whichever is built is
probed part-only first, with its own plan and bars committed before its timing: a gain bar at eight threads on this
host class and a one-thread guard (the struct-layout lesson of the apron rounds: no field added to `SliceDecoder`
without checking one thread).

## Files

`build.sh` (both binaries), `run-gcp.sh` (upload and run), `job.sh` (the VM's job, a copy of
`../profile-ae609547-pmu/job.sh` that runs `rec_miss.py` once), `rec_miss.py` (probe, stat, record), `miss.py`
(tables). Binaries (`build.sh`, sha256): `prof-main` `8e27b4482aae8e847245a78920d910978af1f6fb3a7e947957c740c52b95dc16`,
`prof-ft` `74fe28e2b3fe62eb64e359fa775097fafaa446f76b0d09ef50b8e338601bb60b`.

## Result

Host (`run/host.txt`): Intel Xeon Platinum 8581C, family 6 model 207, 8 CPUs, one thread a core, kernel and perf
6.12. Every event was counted and none multiplexed (`run/events.json`); the attribution build decodes as the BtbN build
does and has no `.debug_*` section; build order in `run/build-order.txt`. Checks met: CoreAV's inter instructions a
macroblock at p81 are 1.9% below p8 over the 32; every arm's two rounds are within 2.1% (`run/tables.txt`, last
table). Tables: `run/tables.txt` (`miss.py`), numbers below from it and `run/miss.json`.

Inter cycles a macroblock (mean over streams):

| | p1 | p81 | p8 | Δ_band | Δ_cross | Δ_cross sd / 720p / 1080p / 4k |
|---|---|---|---|---|---|---|
| CoreAV `main` | 323.4 | 400.7 | 513.9 | 77.3 | **113.2** | 284.3 / 115.2 / 21.3 / 31.9 |
| FFmpeg | 287.1 | 303.6 | 369.8 | 16.5 | 66.2 | 150.9 / 57.4 / 38.4 / 18.0 |
| excess | | | | 60.8 | **47.0** | 133.4 / 57.8 / −17.1 / 13.9 |

- **Q1, confirmed: the growth is the cores'.** Δ_cross is 0.59 of CoreAV's Δ_total (190.5) and its excess over
  FFmpeg's is 47.0 cycles a macroblock (bar 30). It sits in sd and 720p, the classes of the smallest bands (one and
  four macroblock rows), where a picture's worker reads rows that another core wrote moments before. Banding's own work
  (Δ_band, 0.41 of Δ_total) is the other part: on one core it adds 71 instructions a macroblock, the kind of work
  earlier rounds listed (band views, aprons, blocks that leave a band).
- **Q2, loads.** Of the 47.0, load stalls (`cycle_activity.stalls_l1d_miss`) are 40.2 (CoreAV 74.0 against FFmpeg's
  33.8; 0.86 of the excess, bar 0.5), store-buffer stalls (`resource_stalls.sb`) 21.5 (24.3 against 2.8; the two
  overlap, so they do not add up). The miss counts do not grow from p81 to p8 (CoreAV's retired L1 misses 3.9 to 2.9 a
  macroblock, L2 misses 0.67 to 0.45): the same accesses wait longer when another core holds the line, and CoreAV's
  wait more than FFmpeg's. At sd CoreAV's loads miss L2 1.27 times a macroblock at p8 against FFmpeg's 0.00, and its
  loads that find their line already in flight (`mem_load_retired.fb_hit`: a load to a line whose fetch is already in flight, as when a request comes late or
  a second load follows the first) are 2.8 to 3.4
  a macroblock in every placement against FFmpeg's 1.0 to 1.5. Store RFO misses are under 0.1 a macroblock at p8 for
  both, yet CoreAV's store-buffer stalls at sd are 95.9 against 25.9: its write hints for the picture's rows
  (`request_destination`, `rows_for_write`) issue `prefetcht0`, which fetches a line another core holds as shared, so
  the store still has to take it.
- **Q3, functions** (Δ_cross, cycles a macroblock over the 32): the luma kernel (`x86.rs` `b1` 17.5, `store` 16.5,
  `load` 12.8, `vector8` 7.9, `add` 6.1; mostly `stalls_l1d_miss`, which a sample charges to the instructions after
  the waiting load), `predict.rs` `rows` 9.5, `reference_band` 6.4, `edge_window` 5.7, and `predict_fast` 5.5 and
  `predict_fast_body` 5.1, which are store-buffer stalls (4.7 and 5.3).
- **Q4, not an earlier change's.** `ft` − `main` at p8 is +1.7 cycles a macroblock (sd +1.7, 4k +4.7): first touch of a new
  picture is not this cost, so the target stays this probe's.

What follows (the plan's rule): the loads lead, so the cut is the reference rows' requests. `Planes::request` hints
`h` rows from the block's integer row at one column for the next four macroblocks, `h` that of the block at the left
edge (8 for a partition of 8 rows); the window reaches 2 rows above and 3 below (5 of 21 luma rows unhinted, one of
9 chroma rows), and at eight threads each such line comes from another core. The store-buffer part points at
ownership hints (`prefetchw`) for the picture's rows, the mechanism of `rejected/h264-apron-keep`. Both are designed
and probed part-only in their own plan.

## Spend

0.92 USD (upper bound: 0.763 hours the VM existed times 1.20 USD an hour); the SPOT price is below
it.

Raw data (`res/`, the attribution build's symbol table trimmed to function symbols `t`, `T`, `w`, `W`): `raw.tar.gz`
on branch `measured/h264-miss-8t`, sha256 `453ad647d41de5d0a53f14e72d2172d166dde1478e8c388561f4f0b8177025ec`.
`python3 -I miss.py res .` (with `prof-main` and `prof-ft` from `build.sh` in `.`) gives `run/tables.txt`.
