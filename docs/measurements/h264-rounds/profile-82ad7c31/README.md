# `main` at `82ad7c31` against FFmpeg 9.0.2 by function, one thread and eight, on a host with a PMU

Diagnosis only, as `../profile-ae609547-pmu/` was: the host is a Compute Engine VM and every call runs under
`perf`, so nothing here is a speed claim or moves a bar. The question is which gaps are left after bundle 4 and
which of them the open rounds do not already address: bundle 3b (the inter placement, the reference hints' walk,
chroma deblocking on bytes, 4-wide luma blocks) and the CAVLC slice loop.

## Method

`../profile-ae609547-pmu/`'s method and tools, unchanged but for two things:

- `job.sh` is that run's with `--disable-doc` added to the attribution build's configuration. The build is FFmpeg
  9.0.2's pinned release tarball with `--disable-debug --disable-stripping`; make's output went to a log deleted
  unread, and the source tree, the tarball and the log were deleted right after `make` and before any profiling
  (`run/build-order.txt`); there was no `make install`. The build has no `.debug_*` section
  (`run/ffattr-sections.txt`), so FFmpeg is attributed by function symbol only, and decodes as the BtbN build does
  (`run/ffattr-output.txt`).
- `stages_fn.py` is `../profile-ae609547-pmu/stages_pmu.py` with one change of classification: CoreAV's
  `decision.rs` (the CABAC decision in assembly) goes to entropy and `strengths.rs` (the boundary-strength
  kernel) to deblocking, which the older classifier leaves in "other". It also writes every function's weight
  (`functions.json`), which `fn_rank.py` ranks per class with entropy and the MB layer as one stage (the two decoders
  split them differently: FFmpeg's `ff_h264_decode_mb_cabac` does much of what CoreAV's MB layer does). `hot.py`
  annotates CoreAV's own disassembly with its samples.

CoreAV `main` at `82ad7c31`, `dist` with line tables (`prof-82ad7c31`, sha256 in `run/host.txt`); FFmpeg BtbN
`n9.0.2-17-g2a571b6068` for the totals. Host (`run/host.txt`): `c4-standard-16`, SPOT, one thread a core (8 vCPUs on 8
cores), Xeon Platinum 8581C, family 6 model 207 stepping 2. The 32 training streams at the class frame counts,
CoreAV `--discard` against `ffmpeg -f null`, two rounds; one thread pinned (`t1`) and eight threads unpinned (`t8`).

## Results

### Time (`run/streams.t1.txt`, `run/streams.t8.txt`)

CoreAV over FFmpeg, wall time of the `perf stat` call, median per stream (diagnosis):

| | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| one thread | 0.947, 21 of 32 faster | 0.913 | **1.019** | **1.011** | 0.892 |
| eight threads | 0.822, 32 of 32 faster | 0.883 | 0.835 | 0.885 | 0.639 |

At eight threads CoreAV spends 0.992 times FFmpeg's cycles summed over the threads (sd 1.045), keeping 4.40 cores
busy against FFmpeg's 3.72.

### Stages, cycles as shares of FFmpeg's (`run/functions.t1.txt`, `run/functions.t8.txt`)

Entropy and the MB layer as one stage; excess = CoreAV less FFmpeg:

| | entropy + MB | inter | reconstruction | deblocking | other |
|---|---|---|---|---|---|
| one thread, all 32 | 0.487 / 0.489, −0.002 | 0.194 / 0.161, **+0.033** | −0.014 | −0.022 | −0.036 |
| one thread, sd | +0.038 | +0.019 | −0.001 | −0.021 | −0.131 |
| one thread, 720p | +0.001 | **+0.037** | −0.002 | −0.019 | −0.010 |
| one thread, 1080p | −0.016 | **+0.028** | +0.012 | −0.014 | +0.010 |
| one thread, 4k | −0.034 | **+0.050** | −0.065 | −0.034 | −0.012 |
| eight threads, all 32 | 0.410 / 0.429, −0.019 | 0.234 / 0.154, **+0.079** | −0.025 | −0.013 | −0.028 |
| eight threads, sd | +0.022 | **+0.106** | −0.011 | −0.001 | −0.089 |
| eight threads, 720p | −0.014 | **+0.066** | −0.014 | −0.016 | −0.016 |
| eight threads, 1080p | −0.031 | **+0.045** | +0.003 | −0.010 | −0.009 |
| eight threads, 4k | −0.054 | **+0.100** | −0.077 | −0.025 | +0.003 |

Inter prediction is the gap left in every class at both thread counts; entropy + MB is at parity over the 32, and its
excess at sd at one thread is the CAVLC stream `6f7799768413` (+0.096: `cavlc.rs` `block_in`, `code`, `levels`).

### The gaps ranked, and who has them

By function, cycles as shares of FFmpeg's, over the 32:

| | gap | CoreAV functions | open round |
|---|---|---|---|
| 1 | eight-thread inter, +0.079 | `prefetch.rs` `for_rows` 0.035 | bundle 3b (the hints' straight walk) |
| | | `inter.rs` `predict_fast_body` 0.031 (0.011 at one thread) | about two thirds of the growth: the band view's load-back, below |
| | | `inter.rs` `reference_band` 0.022 (none at one thread) | none |
| | | the luma kernels' `b1`, `store`, `load` 0.031 | bundle 3b (4-wide luma blocks) |
| 2 | one-thread inter, +0.033 | `predict.rs` `place` 0.019, `for_rows` 0.017, `rows` 0.010, `predict_body` 0.010 | bundle 3b |
| 3 | one-thread sd entropy + MB, +0.038 | CAVLC on `6f7799768413` | the CAVLC slice loop |
| 4 | one-thread 1080p reconstruction, +0.012 | `recon.rs` `apply` 0.021, `add_residual8` 0.011, `idct8_pass` 0.007 (FFmpeg's `hl_decode_mb_simple_8` 0.029) | none; spread thin |
| 5 | one-thread 1080p other, +0.010 | `picture.rs` `mb_motion` 0.008, `fast/mod.rs` `residual` 0.007 | none; small |

The CABAC decision (`decision.rs`, 0.166 at one thread) is CoreAV's largest function, against FFmpeg's `get_cabac`
and `get_cabac_noinline` (0.142 together, with more decisions inlined into `decode_significance_x86` and the residual
functions); the stage as a whole is at parity.

### The band view at eight threads (`run/band-view.t8.txt`)

At eight threads a reference is banded, and `predict_fast_body` takes each block's view of the band its window is
read from from `reference_band`, which returns the 120-byte `Option<Planes>` by value: it stores it in 16-byte
pieces, and the caller loads it back in 32-byte pieces to copy it to its own stack. A load wider than the stores it
reads cannot be forwarded from the store buffer; the first 32-byte load after the call carries 33% of
`predict_fast`'s cycles at sd (none at one thread, where no reference is banded). Shares of CoreAV's own cycles:

| | `reference_band` | the load-back in `predict_fast` | together |
|---|---|---|---|
| sd | 0.036 | 0.023 | 0.060 |
| 720p | 0.023 | 0.018 | 0.041 |
| 1080p | 0.016 | 0.014 | 0.030 |
| 4k | 0.028 | 0.020 | 0.048 |

Inside `reference_band` at sd, 55% of its cycles are its prologue and the chain of loads to the reference's geometry
(`refs`, the list, the `Arc<Picture>`, `band_mbs`), 23% the kept view's test and copy, and 21% making a view not kept.
This is the cut of "Inter round: a band's view read where it is kept" (`docs/design/h264.md`).

## Spend

0.35 USD (upper bound: 0.294 hours the VM existed times 1.20 USD an hour).

## Reproduce

```
# upload directory: job.sh, rec_pmu.py (../profile-ae609547-pmu/), es/ (32 streams), prof-82ad7c31, ffmpeg (BtbN)
python3 -I scripts/gcp_vm_job.py --upload UP --job job.sh --download res --zone us-central1-a --machine c4-standard-16 \
  --threads-per-core 1 --provisioning spot --usd-per-hour 1.20 --max-hours 2 --cap-usd 2.4
for t in t1 t8; do
  python3 -I stages_fn.py res/$t prof-82ad7c31 > run/stages.$t.txt
  python3 -I fn_rank.py res/$t 30 > run/functions.$t.txt
  P=../profile-ae609547-pmu
  python3 -I $P/top5.py res/$t/stages.json > run/top5.$t.txt   # stages_pmu.py's stages.json
  python3 -I $P/topdown.py res/$t > run/topdown.$t.txt
  python3 -I $P/streams.py res/$t > run/streams.$t.txt
done
python3 -I hot.py res/t8 prof-82ad7c31 reference_band '*.sd'
```

`top5.t*.txt` and `streams.t*.txt` come from `stages_pmu.py`'s `stages.json`, so their stages have the older
classification (the CABAC decision in "other").

Raw data (`res/`, the attribution build's symbol table trimmed to function symbols `t`, `T`, `w`, `W`): `raw.tar.gz`
on branch `measured/h264-profile-82ad7c31`, sha256 in `run/sha256.txt`.
