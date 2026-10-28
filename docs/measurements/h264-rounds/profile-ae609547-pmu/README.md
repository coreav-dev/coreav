# `main` at `ae609547` against FFmpeg 9.0.2 by stage and by stream, one thread and eight, on a host with a PMU

Diagnosis only: the host is an Emerald Rapids VM, not the 2.90 GHz class the one-thread speed bars name, and every
call runs under `perf`, so nothing here is a speed claim or moves a bar. It is the method of `../profile-acf3ee2f-pmu/`
 again after bundle 2, with two additions: each stream's time against FFmpeg with its causes, and a pass
at eight threads.

## Method (fixed before the run)

- Host: Compute Engine `c4-standard-16`, SPOT, PMU `STANDARD`, one thread a core (`--threads-per-core 1`: 8 vCPUs on
  8 cores, as the eight-thread speed bars run), Debian 13. Created, run and deleted by `scripts/gcp_vm_job.py`, one VM,
  a cap of 2.4 USD (2 hours at an upper bound of 1.20 USD an hour). A preempted or crashed VM is a host fault and the
  run is repeated as is.
- CoreAV `main` at `ae609547`, `dist` with line tables (`CARGO_PROFILE_DIST_DEBUG=line-tables-only`, default
  features). FFmpeg: the BtbN build `n9.0.2-17-g2a571b6068` (the timing reference) and, for attribution, a build of the
  9.0.2 release tarball with `--disable-debug` made on the VM by `job.sh` as in earlier changes (make's output to a log
  deleted unread; source tree, tarball and log deleted right after `make` and before any profiling, recorded in
  `run/build-order.txt`; no `.debug_*` section, so FFmpeg is attributed at function level only).
- The 32 training streams at the class frame counts, CoreAV `--discard` against `ffmpeg -f null`, two rounds.
  `job.sh` runs `rec_pmu.py` twice: `ipc` (one thread pinned to CPU 3, `-threads 1`) into `out/t1`, and `ipc8` (eight
  threads, `--threads 8` and `-threads 8`, unpinned) into `out/t8`. Each runs `perf stat` of cycles, instructions and
  task-clock with the wall time of the call for CoreAV, the BtbN build and the attribution build; `perf record` of
  cycles and instructions (2,000 samples a second) for CoreAV and the attribution build; and the top-down metrics.
- `stages_pmu.py` classifies samples as an earlier change did (CoreAV by `../profile-6c9e240/classify.py`'s `core_stage`, FFmpeg by
  function name) and ranks stages by excess over parity (CoreAV's cycles less 1.00 times FFmpeg's, as shares of
  FFmpeg's); `top5.py` ranks them per class with entropy and the MB layer as one stage. `streams.py` (new) gives each
  stream's wall time over FFmpeg's and its cycles and task-clock, per class the median and the streams below 1.00,
  and the ten slowest streams with their stages by excess and the leading functions of their first two. At eight
  threads a stage's cycles are summed over the threads, so they show work, and task-clock over wall time shows how
  busy the threads keep the cores; the wall ratio is the time.

## Results

Host (`run/host.txt`): Intel Xeon Platinum 8581C @ 2.30 GHz, family 6 model 207, 8 CPUs, one thread a core, kernel
and perf 6.12. The attribution build decodes as the BtbN build does (`run/ffattr-output.txt`) and has no `.debug_*`
section (`run/ffattr-sections.txt`); the build order is in `run/build-order.txt`.

### Time, stream by stream (`run/streams.t1.txt`, `run/streams.t8.txt`)

CoreAV over FFmpeg, wall time, median per stream (one call a round under `perf stat`, two rounds; diagnosis). The wall
time is that of the whole `perf stat` command, so perf's own start-up is in both sides and pulls short calls' ratios
toward 1.00:

| | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| one thread | 1.028, 14 of 32 faster | 0.961 (5) | **1.072** (2) | **1.066** (1) | 0.917 (6) |
| eight threads | 0.903, 29 of 32 faster | 0.921 (8) | 0.906 (7) | 0.943 (6) | 0.694 (8) |
| eight threads, cycles summed over threads | 1.077 | 1.115 | 1.080 | 1.074 | 0.985 |

At one thread the classes above parity are 720p and 1080p; 18 of 32 streams are slower than FFmpeg. At eight
threads every class is below parity on this host, but CoreAV spends 1.077 times FFmpeg's cycles to get there: it keeps
4.43 cores busy against FFmpeg's 3.69 (median task-clock over wall time).

### One thread: stages ranked by excess over parity (`run/top5.t1.txt`)

Cycles as shares of FFmpeg's, mean over the class's streams; entropy and the MB layer as one stage:

| class | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| sd | entropy + MB **+0.075** (1.20) | deblocking +0.017 (1.08) | inter +0.008 (1.05) | reconstruction −0.002 (0.98) | other −0.142 (0.26) |
| 720p | entropy + MB **+0.043** (1.09) | inter +0.023 (1.17) | reconstruction +0.000 (1.00) | deblocking −0.002 (0.99) | other −0.009 (0.84) |
| 1080p | inter **+0.031** (1.22) | entropy + MB +0.028 (1.05) | other +0.014 (1.46) | reconstruction +0.006 (1.07) | deblocking −0.005 (0.96) |
| 4k | inter **+0.059** (1.31) | entropy + MB −0.004 (0.99) | other −0.004 (0.95) | deblocking −0.031 (0.72) | reconstruction −0.070 (0.43) |
| all 32 | entropy + MB **+0.036** (1.07) | inter +0.030 (1.19) | deblocking −0.005 (0.97) | reconstruction −0.016 (0.82) | other −0.035 (0.61) |

Against `acf3ee2f` (the same CPU model on a `c4-standard-8` with two threads a core, one thread pinned), deblocking went from +0.024 to −0.005 and entropy + MB from +0.069 to +0.036; inter is
unchanged (+0.028 then, +0.030 now). Instructions a macroblock (all 32): entropy + MB 2,989 against FFmpeg's 2,119,
inter 1,033 against 993. IPC: entropy 2.34 against 1.70, MB layer 2.84 against 3.03, inter 3.15 against 3.42.
Top-down (`run/topdown.t1.txt`): 1.006 times FFmpeg's slots; CoreAV's excess is retiring (+0.034) and fetch bandwidth
(+0.029), and it is less backend-bound (−0.078).

Where in the two gaps (all 32, `run/stages.t1.txt`, functions by cycles as shares of FFmpeg's): inter is
`prefetch.rs` `for_rows` 0.017, `predict.rs` `place` 0.017, `rows` 0.012 and `predict_body` 0.010, against FFmpeg's
`mc_part_420_simple_8` 0.035; entropy + MB is led by CABAC's `decision` in every class (0.097 at sd), and in the one
sd CAVLC stream (`6f7799768413`) by CAVLC (`block_body`, `code`, `skip`).

### One thread: the ten slowest streams

| stream | wall | cycles | first two stages by excess | leading CoreAV functions of the first |
|---|---|---|---|---|
| `6f7799768413.sd` | 1.154 | 1.157 | entropy + MB +0.124; deblocking +0.083 | `cavlc.rs` `block_body` 0.060, `code` 0.039 |
| `6da34958296c.1080p` | 1.132 | 1.132 | inter +0.084; entropy + MB +0.071 | `prefetch.rs` `for_rows` 0.037, `predict.rs` `place` 0.028 |
| `b41f70a7fa60.1080p` | 1.113 | 1.113 | entropy + MB +0.063; inter +0.035 | `cabac.rs` `decision` 0.218, `significance_map` 0.026 |
| `36501d448eaa.720p` | 1.105 | 1.099 | inter +0.047; entropy + MB +0.042 | `fast/mod.rs` `predict_fast` 0.014, `predict.rs` `place` 0.009 |
| `c7ecda16e4a7.1080p` | 1.101 | 1.103 | entropy + MB +0.046; other +0.029 | `cabac.rs` `decision` 0.285, `significance_map` 0.030 |
| `b71b6b31e54f.1080p` | 1.085 | 1.090 | entropy + MB +0.104; inter +0.060 | `inter.rs` `resolve` 0.028, `cavlc.rs` `block_body` 0.023 |
| `65ec61f95a00.720p` | 1.080 | 1.078 | entropy + MB +0.059; deblocking +0.027 | `cabac.rs` `decision` 0.227, `mb.rs` `decode_mb` 0.014 |
| `76e0b1692494.720p` | 1.078 | 1.072 | inter +0.059; entropy + MB +0.035 | `prefetch.rs` `for_rows` 0.029, `predict.rs` `place` 0.016 |
| `278dbf5a63bb.720p` | 1.078 | 1.061 | entropy + MB +0.064; deblocking +0.021 | `cabac.rs` `decision` 0.209, `mb.rs` `decode_mb` 0.022 |
| `1d4b1775d78a.720p` | 1.065 | 1.072 | entropy + MB +0.074; inter +0.011 | `cabac.rs` `decision` 0.181, `mb.rs` `decode_mb` 0.022 |

Entropy + MB is first or second in all ten, inter in six, and deblocking is second in three (the sd CAVLC stream and
two 720p CABAC streams). The two CAVLC streams (`6f7799768413`, `b71b6b31e54f`) have the largest
entropy + MB excess (+0.124, +0.104).

### Eight threads (`run/top5.t8.txt`, `run/stages.t8.txt`, `run/topdown.t8.txt`)

Cycles summed over the threads, as shares of FFmpeg's at eight threads:

| class | 1 | 2 | 3 |
|---|---|---|---|
| sd | inter **+0.100** (1.63) | entropy + MB +0.045 (1.15) | deblocking +0.015 (1.10) |
| 720p | inter **+0.068** (1.48) | entropy + MB +0.040 (1.09) | other −0.002 (0.99) |
| 1080p | inter **+0.047** (1.33) | entropy + MB +0.021 (1.04) | reconstruction +0.007 (1.08) |
| 4k | inter **+0.082** (1.44) | other +0.020 (1.10) | entropy + MB −0.004 (0.99) |
| all 32 | inter **+0.074** (1.47) | entropy + MB +0.026 (1.06) | deblocking −0.004 (0.97) |

Inter prediction costs much more at eight threads than at one, and FFmpeg's much less so: CoreAV's inter instructions a
macroblock go from 1,033 to 1,137 (FFmpeg 993 to 1,016) and its inter IPC from 3.15 to 2.48 (FFmpeg 3.42 to 3.04), so
its inter cycles a macroblock grow by about 40% (328 to 458) and FFmpeg's by about 15% (290 to 334).
The functions that grow (as shares of FFmpeg's cycles at each thread count, which themselves grow at eight threads) are
`inter.rs` `predict_fast_body` (0.010 to 0.036), `reference_band` (below the one-thread list's last entry, 0.008, to
0.020: at one thread a picture is one band, so a slice builds its view once) and the luma
kernel's stores (`x86.rs` `store`, 0.010 to 0.024 at sd). Top-down at eight threads: CoreAV is memory-bound for 0.177
of FFmpeg's slots against FFmpeg's 0.153 (at one thread 0.038 against 0.063). Waiting is not in inter: a band not yet
published is waited for on a condition variable (`Picture::band`), which lands in the kernel and libc ("other"). This
run sampled cycles and instructions only, so it does not say which accesses miss; the stores' growth and the earlier
apron finding (`../apron-copies/`: the eight-thread copies cost cache misses on a cold destination) point at cold
band buffers and at reference rows written by another core.

The eight-thread ten slowest (`run/streams.t8.txt`): only three are above 1.00 (`65ec61f95a00.720p` 1.054,
`6da34958296c.1080p` 1.014, `c7ecda16e4a7.1080p` 1.002); inter is the first stage by excess in six of the ten
(+0.068 to +0.125), entropy + MB in three (`65ec61f95a00`,
`c7ecda16e4a7`, `b41f70a7fa60`), and reconstruction in one
(`175433794e02.1080p`, `recon.rs` `apply`).

## Spend

0.36 USD (upper bound: 0.303 hours the VM existed times 1.20 USD an hour); the SPOT price is below
it.

## Reproduce

```
# upload directory: job.sh, rec_pmu.py, es/ (32 streams), prof-ae609547, ffmpeg (BtbN)
python3 -I scripts/gcp_vm_job.py --upload UP --job job.sh --download res --zone us-central1-a --machine c4-standard-16 \
  --threads-per-core 1 --provisioning spot --usd-per-hour 1.20 --max-hours 2 --cap-usd 2.4
for t in t1 t8; do
  python3 -I stages_pmu.py res/$t prof-ae609547 > run/stages.$t.txt
  python3 -I top5.py res/$t/stages.json > run/top5.$t.txt
  python3 -I topdown.py res/$t > run/topdown.$t.txt
  python3 -I streams.py res/$t > run/streams.$t.txt
done
```

Raw data (`res/`, the attribution build's symbol table trimmed to function symbols `t`, `T`, `w`, `W`): `raw.tar.gz`
on branch `measured/h264-profile-ae609547-pmu`, sha256 `cfe1813c6b25a7047dc51082e9b5c393ef68e31ff51154bce72a325001fda6dd`.
