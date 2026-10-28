# Eight-thread inter: a banded picture's lines asked for ahead (part-only probe)

Diagnosis for the cut of `docs/design/h264.md`, "Threading round: a banded picture's lines asked for ahead", on the
host class of the miss probe (`../miss-8t/`); not a speed claim, and it moves no bar. The miss probe found CoreAV's
inter cycles a macroblock growing by 113 from eight threads on one core (p81) to eight threads on eight cores (p8),
47 more than FFmpeg's, mostly at sd (133 more) and 720p (58): load stalls on lines another core wrote (40.2 of the
47) and store-buffer stalls (21.5; sd 95.9 against FFmpeg's 25.9). The cut asks for those lines ahead in a banded
picture (a picture decoded at more than one thread); this probe tells which of its parts remove the stalls they aim
at, before any timing.

## The arms (fixed before the run)

All on `main` at `7af5d4f5`; the cut was `perf/h264-8t-hints` (`f35798c7`; after gating kept on `rejected/h264-banded-hints`): `SliceDecoder::hints` (one byte, in place
of the `ahead` flag) is set by `mb::hints` from the picture's size and its band count.

| arm | a picture in one band (every picture at one thread; a non-reference picture at any count) | a banded picture (a reference picture at more threads) |
|---|---|---|
| `base` | `main`: hints where the picture holds more than 1 MiB of samples (`ahead`) | the same |
| `E` | as `main` | every hint `main` issues above 1 MiB, at every size: the reference's rows (`Planes::request`) and the picture's (`request_destination`, read hints) |
| `EW` | as `main` | `E`, and the reference's request covers the window's rows, two above the block's and three below (`h + 5` rows from `y - 2`; chroma follows, `h / 2 + 2` rows from `(y - 2) >> 1`) |
| `EO` | as `main` | `E`, and the picture's lines are asked to be owned (`prefetchw`, `prefetch::Ownership`, the mechanism of `rejected/h264-apron-keep`) |
| `EWO` | as `main` | all three: the cut |

Why these parts: `ahead` (round 12) leaves a picture of 1 MiB or less unhinted because "it stays in [this core's
second-level] cache from one macroblock row to the next, with the part of its reference a row reads". At more than
one thread neither holds: the reference was written by another core, and the picture's buffer was last written for
an older picture and read since by other cores. Every sd picture of the training set, and three of the eight 720p
ones (848x480 to 960x540), are below 1 MiB, so at eight threads they ask for nothing: the two classes with the excess. The window's two rows above and three below were
left out at one thread because only interpolated blocks read them (0.936 against 0.915 on the 4k class, "Motion
round: the reference's lines requested ahead, cheaply"); at eight threads each such line comes from another core.
A read hint (`prefetcht0`) fetches a line other cores hold as shared, and the store then has to take it from them;
`prefetchw` takes it once.

## Method (fixed before the run)

- Host: as `../miss-8t/`. Compute Engine `c4-standard-16`, SPOT, PMU `STANDARD`, one thread a core (8 vCPUs on 8
  cores), Debian 13, made, run and deleted by `scripts/gcp_vm_job.py` (`run-gcp.sh`), capped at 2.4 USD. A preempted or
  crashed VM, or a failed host check, is a host fault and the run is repeated as is; nothing else is.
- Binaries (`build.sh`): `dist` with line tables, one target directory each; `E`, `EW` and `EO` are the cut with its
  `BANDED_HINTS` narrowed (one line each). sha256: `base` `fd946003…`, `E` `79dcbcdc…`, `EW` `5b9cbedf…`, `EO`
  `5ad1c455…`, `EWO` `17894903…` (in full in `host.txt` of the run).
- Placement `p8` only (eight threads unpinned on the eight cores); one thread is a picture in one band, where every
  arm issues `main`'s hints, and is guarded by timing, not here.
- The 32 training streams at the class frame counts, `--discard`, two rounds, the arms' order rotated each round
  (`rec_hints.py`). `perf stat` (cycles, instructions, task-clock, wall time) of every arm and of FFmpeg (BtbN 9.0.2,
  `-f null`, `-threads 8`, stat only); `perf record` of every CoreAV arm in the miss probe's three groups (`loads`,
  `stores`, `stalls`), 2,000 samples a second an event, none multiplexed.
- `hints.py` classifies samples as `../miss-8t/miss.py` does and reports each event a macroblock in inter and every
  stage's cycles, mean over the streams of a class and over the 32.

## Criteria (fixed before the run)

Over the 32 streams at p8, per macroblock, from the records; Δ is one arm less another. The round-to-round spread of
inter cycles in the miss probe was up to 2.1% (about 11 cycles a macroblock at p8), so each bar is at least that.

| part | criterion | met if |
|---|---|---|
| E (every size) | `E` − `base`, sd class | inter cycles a macroblock fall by at least 20, and inter `cycle_activity.stalls_l1d_miss` fall |
| W (the window's rows) | `EW` − `E`, all 32 | inter cycles fall by at least 10, and inter `cycle_activity.stalls_l1d_miss` fall |
| O (owned lines) | `EO` − `E`, all 32 | the cycles of every stage together fall by at least 10 (the picture's lines are written by inter, intra, the residual and the deblocking filter), and inter `resource_stalls.sb` fall |

Each part is read on its own pair; `EWO` is reported against `base` and the parts, with no bar of its own. What
follows: the arm taken to timing holds the parts that met their criterion (with `E` failing and W or O met, a build of
W and O on `main`'s size rule is made); none met, and the cut stops here as a diagnosis. The timing bars (eight threads
on this host class, a one-thread guard, a held-out guard) are committed in the design note before any timing.

Checks: each arm's two rounds within 3% of each other on inter cycles a macroblock over the 32; inter instructions a
macroblock reported for each arm (the hints' cost).

## Files

`build.sh` (the arms' binaries), `run-gcp.sh` (upload and run), `job.sh` (the VM's job), `rec_hints.py` (probe,
stat, record; a variant of `../miss-8t/rec_miss.py`), `hints.py` (tables).

## Result

Host (`run/host.txt`): Intel Xeon Platinum 8581C, family 6 model 207, 8 CPUs, one thread a core, kernel and perf
6.12. Every event was counted and none multiplexed (`run/events.json`). Checks met: each arm's two rounds within 1.8%
on inter cycles a macroblock over the 32 (`run/tables.txt`, last table). Tables: `run/tables.txt` (`hints.py`),
`run/hints.json`.

Inter at p8, per macroblock, mean over the streams (all 32, and sd):

| arm | cycles | sd | `stalls_l1d_miss` | `resource_stalls.sb` | instructions | every stage's cycles |
|---|---|---|---|---|---|---|
| `base` | 517.6 | 727.2 | 99.5 | 38.1 | 1,168.9 | 2,591.6 |
| `E` | 501.5 | 681.0 | 75.6 | 29.6 | 1,194.1 | 2,592.4 |
| `EW` | 501.1 | 690.5 | 70.1 | 31.3 | 1,211.7 | 2,598.9 |
| `EO` | 493.3 | 671.7 | 76.8 | 20.0 | 1,201.7 | 2,552.7 |
| `EWO` | 497.2 | 678.3 | 71.7 | 20.2 | 1,205.9 | 2,557.0 |

- **E, met.** `E` − `base` at sd: inter cycles −46.2 a macroblock (bar 20), `stalls_l1d_miss` −66.1, `stalls_l2_miss`
  −73.9; sd's loads that miss L2 fall from 1.30 to 0.73 a macroblock. Over the 32 inter falls by 16.1 cycles, but the
  hints' own cost (the walk `prefetch.rs` `for_rows`, +18.7 cycles a macroblock; 25 instructions more) leaves every
  stage's cycles where they were (+0.8).
- **W, not met.** `EW` − `E` over the 32: inter cycles −0.4 (bar 10); the load stalls fall by 5.5 and the instructions
  rise by 17.6, which the window's extra rows cost.
- **O, met.** `EO` − `E` over the 32: every stage's cycles −39.7 (bar 10), inter `resource_stalls.sb` −9.6 (sd −16.9,
  4k −9.8); inter cycles −8.2, and the entropy and macroblock stages, which write the picture too, −8.2 and −15.7.

What follows (the plan's rule): the arm taken to timing is `E` with `O` (`EO`, every hint of `main` at every size in a
banded picture, the picture's lines owned); the window's rows are left out. Information, not a bar: in `perf stat`'s
two rounds the median per-stream wall time over `base` is 0.994 for `EO` (20 of 32 faster; sd 0.993, 720p 0.985,
1080p 0.996, 4k 0.995), 1.007 for `E`, 1.009 for `EW`, 0.992 for `EWO`; `base` over FFmpeg 0.842 (31 of 32 faster).

## Spend

0.54 USD (upper bound: 0.446 hours the VM existed times 1.20 USD an hour).

Raw data (`res/`): `raw.tar.gz` on branch `measured/h264-hints-8t`, sha256
`dce90424f8d63d2aa780d82944ea79a2eba5e0929662ee5f4568d16692e7cae0`. `python3 -I hints.py res .` (with the five
`prof-<arm>` binaries from `build.sh` in `.`) gives `run/tables.txt`.
