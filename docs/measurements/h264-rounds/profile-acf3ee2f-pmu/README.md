# `main` at `acf3ee2f` against FFmpeg 9.0.2 by stage, ranked by excess over parity, on a host with a PMU

Diagnosis only: the host is an Emerald Rapids VM, not the 2.90 GHz class the speed bars name, so nothing here is a
speed claim or moves a bar. This is the method of `../pmu-profile/`, again after the MB layer round 2,
with each stage's excess measured against parity (1.00 times FFmpeg) rather than the 1.10 of `../profile-6c9e240/`.

## Host and binaries

- Compute Engine `c4-standard-8`, SPOT, `us-central1-a`, PMU `STANDARD`, Debian 13 (kernel and perf 6.12.113):
  Intel Xeon Platinum 8581C @ 2.30 GHz, family 6 model 207, 4 cores of 2 threads (`run/host.txt`). Created, run and
  deleted by `scripts/gcp_vm_job.py` (`run/gcp-job.log`), one VM, one run.
- CoreAV `main` at `acf3ee2f` (the decoder's code is `16c3e239`'s, an earlier change; later commits change scripts only), `dist`
  with line tables (`CARGO_PROFILE_DIST_DEBUG=line-tables-only`, default features), `prof-acf3ee2f`.
- FFmpeg: the BtbN build `n9.0.2-17-g2a571b6068` (the timing reference), and for attribution a build of the 9.0.2
  release tarball (sha256 `8c385028…`) with `--disable-debug`, built on the VM by `job.sh` as in an earlier change: make's output to
  a log deleted unread, the source tree, tarball and log deleted right after `make` and before any profiling
  (`run/build-order.txt`). No `.debug_*` section (`run/ffattr-sections.txt`), so every FFmpeg number is at function
  level; its output equals the BtbN build's (`run/ffattr-output.txt`).
- The 32 training streams at the class frame counts, one thread pinned to CPU 3, CoreAV `--discard` against
  `ffmpeg -threads 1 -f null`, two rounds.

## Method

`job.sh ipc` runs `rec_pmu.py ipc`: cycles and instructions sampled together (2,000 a second) for CoreAV and the
attribution build; `perf stat` of cycles and instructions for CoreAV, the BtbN build and the attribution build (the
one change from an earlier change's copy: its `ipc` mode had no `perf stat` pass); the top-down metrics per stream for CoreAV and
the BtbN build. `stages_pmu.py` (a copy of an earlier change's with two additions: the parity table, and each stage's functions on
both sides) classifies CoreAV samples by `../profile-6c9e240/classify.py`'s `core_stage` and FFmpeg samples by
function name. A stage's cycles are a share of the same stream's FFmpeg cycles, averaged over the class's streams;
excess is CoreAV's share less FFmpeg's. `top5.py` ranks the stages per class with **entropy and the MB layer as one
stage**: FFmpeg inlines its motion vector prediction and neighbour caches into `ff_h264_decode_mb_cabac`/`_cavlc`, so
at function level its MB layer reads low and its entropy high, and only their sum compares (`run/stages.txt` keeps
them apart). `topdown.py` is an earlier change's.

## Results

CoreAV over the BtbN build, cycles (`perf stat`, median per stream): sd 1.026, 720p 1.130, 1080p 1.136, 4k 1.011,
all 32 1.092. Top-down (`run/topdown.txt`): CoreAV uses 1.093 times FFmpeg's slots; the excess is retiring (+0.078 of
FFmpeg's slots) and fetch bandwidth (+0.036), and CoreAV is less backend-bound (−0.064), as at `b3664291`.

Stages ranked by excess over parity (cycles as shares of FFmpeg's; `run/top5.txt`):

| class | 1 | 2 | 3 | 4 | 5 |
|---|---|---|---|---|---|
| sd | entropy + MB **+0.098** (1.27) | deblocking +0.038 (1.19) | reconstruction +0.009 (1.12) | inter +0.004 (1.02) | other −0.131 (0.31) |
| 720p | entropy + MB **+0.074** (1.16) | inter +0.025 (1.17) | deblocking +0.017 (1.06) | reconstruction +0.010 (1.13) | other −0.007 (0.86) |
| 1080p | entropy + MB **+0.076** (1.12) | inter +0.029 (1.21) | deblocking +0.017 (1.14) | reconstruction +0.011 (1.12) | other +0.010 (1.35) |
| 4k | inter **+0.056** (1.29) | entropy + MB +0.028 (1.06) | deblocking +0.025 (1.22) | other −0.012 (0.86) | reconstruction −0.057 (0.50) |
| all 32 | entropy + MB **+0.069** (1.14) | inter +0.028 (1.18) | deblocking +0.024 (1.14) | reconstruction −0.007 (0.92) | other −0.035 (0.61) |

Instructions a macroblock, CoreAV against FFmpeg (all 32): entropy + MB 3,279 / 2,114, inter 1,032 / 987, deblocking
1,185 / 1,087, reconstruction 506 / 460. IPC (all 32): entropy 2.40 / 1.71, MB layer 2.90 / 3.06, inter 3.23 / 3.48,
deblocking 3.37 / 3.65, reconstruction 2.62 / 2.77.

Where in each gap (`run/stages.txt`, functions by cycles as a share of FFmpeg's):

- **Entropy + MB layer** (all 32): 1,165 more instructions a macroblock over the two. Split as measured, the MB layer
  is +0.094 (0.137 against 0.043) and entropy −0.025, but FFmpeg's side of that split is not comparable (Method);
  CoreAV's own split puts most of its cycles of the pair in entropy (0.416) and 0.137 in the MB layer. The MB layer's
  largest functions: `reset_body` 0.009, `resolve` 0.009, `finish_mb` 0.009, `predict_partitions` 0.008,
  `inter.rs` `{closure#0}` 0.008, `mv_predictor` 0.007, `start_mb_body` 0.006; FFmpeg's functions classed MB layer
  are `fill_decode_caches` 0.020, `fill_decode_neighbors` 0.008, `ff_h264_pred_direct_motion` 0.008 and
  `decode_slice` 0.006. In entropy, CABAC's `decision` is 0.202; FFmpeg's `get_cabac` and `get_cabac_noinline` are
  0.142, and the rest of its CABAC is inlined into `ff_h264_decode_mb_cabac` 0.074,
  `decode_cabac_residual_nondc_internal` 0.073 and `decode_significance_x86` 0.066.
- **Inter prediction** (first at 4k, second at 720p and 1080p): at 4k, `prefetch.rs` `for_rows` 0.034 against
  FFmpeg's `ff_prefetch_mmxext` 0.016, `predict.rs` `place` 0.026 and `rows` 0.018, against `mc_part_420_simple_8`
  0.052; same instruction count as FFmpeg's within 6% (898 against 849), lower IPC (3.30 against 3.61).
- **Deblocking** (all 32): `deblock_row` 0.047 against FFmpeg's `loop_filter` 0.041, `edge_strengths` 0.013 against
  `ff_h264_loop_filter_strength_mmxext` 0.028, and `new` (`Offsets::new` or `Thresholds::new`, the per-edge setup)
  0.010.
- **Reconstruction** is at or below parity overall (0.92) and well below at 4k (0.50); **other** is below parity
  except at 1080p (+0.010).

## Spend

0.13 USD (upper bound: 0.216 hours the VM existed times 0.60 USD an hour); the SPOT price is below it.

## Reproduce

```
# upload directory: job.sh, rec_pmu.py, es/ (32 streams), prof-acf3ee2f, ffmpeg (BtbN)
python3 -I scripts/gcp_vm_job.py --upload UP --job "job.sh ipc" --download res --max-hours 1.5 --cap-usd 1
python3 -I stages_pmu.py res prof-acf3ee2f > run/stages.txt
python3 -I top5.py res/stages.json > run/top5.txt
python3 -I topdown.py res > run/topdown.txt
```

Raw data (`res/`, the attribution build's symbol table trimmed to function symbols `t`, `T`, `w`, `W`): `raw.tar.gz`
on branch `measured/h264-profile-acf3ee2f-pmu`, sha256 `a949a3b599809745f52f49895729e4ac19d4d5c33fe6ae6b1e64f2a486307e1e`.
