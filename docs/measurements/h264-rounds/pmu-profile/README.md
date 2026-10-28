# Hardware events of `main` against FFmpeg 9.0.2 by stage, on a host with a PMU

Diagnosis only. The host is an Emerald Rapids VM, not the 2.90 GHz Ice Lake class the speed bars name, so nothing here
is a speed claim or moves a bar. What it adds to the cpu-clock profiles (`../profile-6c9e240/`) is the hardware's
own account of where the cycles go: instructions, stalls by cause, and the top-down breakdown, per decoding stage.

## Host and binaries

- Compute Engine `c4-standard-8`, SPOT, `us-central1-a`, `advancedMachineFeatures.performanceMonitoringUnit=STANDARD`,
  Debian 13 (kernel 6.12.113, perf 6.12.113): `INTEL(R) XEON(R) PLATINUM 8581C CPU @ 2.30GHz`, family 6 model 207
  stepping 2, 4 cores of 2 threads, AVX-512 (`run1/host.txt`). Created, run and deleted by `scripts/gcp_vm_job.py`
  (below); one VM a run, two runs.
- CoreAV `main` at `b3664291`, `dist` with line tables only (`CARGO_PROFILE_DIST_DEBUG=line-tables-only`, default
  features), named `prof-b3664291`.
- FFmpeg: the BtbN build `n9.0.2-17-g2a571b6068` (the timing reference of every round), and for attribution a build of
  the 9.0.2 release (`ffmpeg-9.0.2.tar.xz`, sha256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`)
  with `../profile/`'s configuration plus `--disable-debug` (gcc 14.2.0, nasm 2.16.03). `job.sh` builds it on the VM:
  make's output goes to a log that is deleted unread (only the exit status is kept), and the source tree, the tarball
  and the log are deleted right after `make`, before anything is profiled (`run1/build-order.txt`,
  `run2/build-order.txt`). Its section list has no `.debug_*` section (`run*/ffattr-sections.txt`), so it carries
  function symbols and no line tables, and every FFmpeg number below is at function level. Its output
  equals the BtbN build's (`ffattr-output.txt`: raw frames of `9db41aa16b5a`, 300 pictures, md5
  `e326636fe331266aaebce7b6319e9963` from both, as in `../profile/`); both runs built the same binary (sha256
  `81ef45ca…` in `run*/host.txt`).
- The 32 training streams at the class frame counts (sd 1800, 720p 900, 1080p 300, 4k 150), one thread pinned to CPU 3,
  CoreAV `--discard` against `ffmpeg -threads 1 -f null`. Per-macroblock figures use `dims.txt` (each stream's size and
  access units, by `ffprobe`; frames are the class count or fewer).

## Method

- `rec_pmu.py`, run 1 (`bash job.sh`): which of 25 candidate events the PMU counts (`run1/events.json`: 23; not the
  L3-miss events); `perf stat` of all 23 in passes of at most three beside cycles and instructions, so no event is
  multiplexed, for CoreAV, the BtbN build and the attribution build, two rounds; `perf record` of two groups (`stores`:
  cycles, `exe_activity.bound_on_stores`, `resource_stalls.sb`, `ld_blocks.store_forward`; `misses`:
  `l1d.replacement`, `l2_rqsts.rfo_miss`, `mem_load_retired.l1_miss`, `branch-misses`), 2,000 samples a second an
  event, for CoreAV and the attribution build, two rounds; a sample weighs its period.
- Run 2 (`bash job.sh ipc`): the top-down metrics, which `perf stat -M TopdownL1,TopdownL2` reports for a process on
  this PMU (`run2/topdown-probe.txt`), per stream for CoreAV and the BtbN build; and cycles with instructions sampled
  together (two rounds), for each stage's instructions per cycle.
- `stages_pmu.py`: a CoreAV sample's stage is `../profile-6c9e240/classify.py`'s `core_stage`, read from that file;
  an FFmpeg sample's is its function symbol's by `FF_RULES` (names only). Without debug information a function stands
  for all that is inlined into it: FFmpeg's motion vector prediction and neighbour caches inlined into
  `ff_h264_decode_mb_cabac`/`_cavlc` count as entropy, so its MB layer reads low and its entropy high, and the two are
  compared together below. Cycle-like events are shares of the stream's FFmpeg cycles, counts are per macroblock, and
  each stream counts equally. `topdown.py` prints each metric as a share of the decoder's own slots and as slots over
  FFmpeg's.

## Results (all 32 streams; tables per class in `run1/stages.txt`, `run2/stages.txt`, `run2/topdown.txt`)

Top-down, in FFmpeg's slots (CoreAV uses 1.101 times FFmpeg's slots, median per stream; the rows are means over
streams, so CoreAV's level-1 rows sum to 1.075):

| Metric | CoreAV | FFmpeg | CoreAV − FFmpeg |
|---|---|---|---|
| retiring | 0.517 | 0.435 | **+0.082** |
| of which light operations | 0.501 | 0.412 | +0.089 |
| bad speculation (branch mispredicts) | 0.171 (0.162) | 0.170 (0.162) | +0.001 (−0.001) |
| frontend bound | 0.193 | 0.166 | +0.026 (fetch bandwidth +0.028) |
| backend bound | 0.194 | 0.229 | −0.035 |
| of which core bound / memory bound | 0.126 / 0.068 | 0.157 / 0.072 | −0.031 / −0.004 |

Instructions and cycles by stage (run 2; cycles as a share of FFmpeg's, instructions per macroblock):

| Stage | cycles CoreAV | FFmpeg | instructions CoreAV | FFmpeg | IPC CoreAV | FFmpeg |
|---|---|---|---|---|---|---|
| entropy + MB layer | 0.586 | 0.481 | 3,461 | 2,106 | 2.46 / 3.12 | 1.71 / 3.08 |
| inter prediction | 0.182 | 0.164 | 990 | 989 | 3.20 | 3.44 |
| reconstruction | 0.082 | 0.090 | 486 | 458 | 2.63 | 2.85 |
| deblocking | 0.194 | 0.172 | 1,190 | 1,086 | 3.46 | 3.71 |
| other | 0.054 | 0.093 | 143 | 179 | 1.63 | 0.96 |
| total | 1.098 | 1.000 | 6,269 | 4,818 | 2.76 | 2.29 |

(IPC of entropy and of the MB layer given separately.) By class, entropy and the MB layer together are +0.124, +0.108,
+0.081 and +0.108 of FFmpeg's cycles (sd, 720p, 1080p, 4k); deblocking +0.029, +0.021, +0.018 and +0.020.

Stalls and misses (`perf stat`, per macroblock, CoreAV / BtbN): cycles 2,210 / 2,036; all stall cycles 398 / 441;
cycles bound on stores 23.5 / 17.9; store buffer full (`resource_stalls.sb`) 39.7 / 26.2; loads blocked by store
forwarding 19.3 / 17.0; L1D lines replaced 44.4 / 39.6; L2 RFO misses 0.7 / 0.6; branch misses 16.8 / 16.4; stores
retired 782 / 613; loads retired 1,294 / 1,199.

By stage (run 1, sampled; CoreAV against the attribution build):

| Event | MB layer CoreAV | FFmpeg | deblocking CoreAV | FFmpeg |
|---|---|---|---|---|
| store buffer full, share of FFmpeg's cycles | 0.0136 | 0.0010 | 0.0001 | 0.0000 |
| bound on stores, share of FFmpeg's cycles | 0.0069 | 0.0010 | 0.0001 | 0.0001 |
| loads blocked by store forwarding, per MB | 4.50 | 0.03 | 9.96 | 8.51 |
| L1D lines replaced, per MB | 9.25 | 2.11 | 7.88 | 6.18 |
| branch misses, per MB | 1.79 | 0.59 | 2.02 | 0.81 |

Where in CoreAV (`run1/stages.txt`, items): the MB layer's store-buffer-full cycles are 57% `finish_mb` and 12%
`store_mb`; its store-forwarding blocks 38% `store_mb`, 17% `resolve`, 11% `finish_mb`; its L1D replacements 20%
`finish_mb`. Deblocking's branch misses are 21% `edge_strengths`, 15% `deblock_row`, 11% `new` (`Offsets::new` or `Thresholds::new`);
its store-forwarding blocks are in the filter kernels (FFmpeg's deblocking has about as many, 8.5 a macroblock
against 10.0).

## What it says

- On this host the gap to FFmpeg is mostly executed work, not waiting: the excess slots are retiring (+0.082) and
  fetch bandwidth (+0.028, feeding the extra instructions), while CoreAV is less backend-bound than FFmpeg (−0.035). CoreAV retires 1.30 times FFmpeg's
  instructions at a higher IPC (2.76 against 2.29).
- Nearly all of it is entropy and the MB layer: +1,355 instructions a macroblock of the +1,451, and +0.105 of FFmpeg's
  cycles of the +0.098 (the other stages net out). Deblocking adds +0.022 (+103 instructions, and 1.2 more branch
  misses a macroblock); inter prediction +0.018 at the same instruction count.
- Store stalls exist where the MB layer probe put them (`finish_mb`, `store_mb`), but on this core they are small:
  store-buffer-full cycles in the MB layer are 1.4% of FFmpeg's cycles, and the counts of the whole run add +5.6 cycles
  bound on stores and +13.5 store-buffer-full cycles a macroblock, of 2,210. `store_mb`'s copy of a row it has just stored is the MB layer's main source of blocked
  store forwarding (4.5 a macroblock against FFmpeg's 0.03). The Ice Lake class has a smaller store buffer than this
  core, so store stalls may weigh more there; the timing rounds, not this profile, decide.

## Spend

0.36 USD and 0.08 USD (upper bounds: hours the VM existed times 0.60 USD an hour), 0.43 USD in all;
the SPOT price is below the bound.

## Reproduce

```
# upload directory: job.sh, rec_pmu.py, es/ (32 streams), prof-b3664291, ffmpeg (BtbN)
python3 -I scripts/gcp_vm_job.py --upload UP --job job.sh --download res --max-hours 2.5 --cap-usd 2
python3 -I scripts/gcp_vm_job.py --upload UP --job "job.sh ipc" --download res2 --max-hours 1.5 --cap-usd 2
python3 -I stages_pmu.py res prof-b3664291 > run1/stages.txt
python3 -I stages_pmu.py res2 prof-b3664291 > run2/stages.txt
python3 -I topdown.py res2 > run2/topdown.txt
```

Raw data (`res/` and `res2/`: the per-run counts and samples, and the attribution build's symbol table and program
headers that `stages_pmu.py` reads): `raw.tar.gz` on branch `measured/h264-pmu-profile`, sha256
`c9599443b1cb99a965b8f0e0ddc58e910ca588cf1a20b102dc5288d1b2be8ae3` (the symbol table trimmed to function symbols, `t`,
`T`, `w` and `W`, the only ones `stages_pmu.py` reads).

`scripts/gcp_vm_job.py`: Compute Engine REST API (v1) and the OAuth token endpoint through the standard library (the
service-account JWT is signed in Python, RSASSA-PKCS1-v1_5); `COREAV_GCP_SA_KEY` and `COREAV_GCP_PROJECT` from the
environment, the key never printed or written; an SSH key made for the run; the VM's `maxRunDuration` with
`instanceTerminationAction=DELETE` and a delete in `finally`; it refuses to start when `max-hours` times the hourly
bound exceeds `--cap-usd` (the bound is required for any machine but the default).
