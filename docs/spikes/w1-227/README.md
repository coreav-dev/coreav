# Library probe and HLS index against libavformat

Throwaway measurement code, outside the Cargo workspace. It
re-measures, on `main`'s crates as of `1114aa6`, the library's probe with the full packet
index against libavformat opening the same file in process, and measures
the HLS index against the same baseline for the first time.

## Hypothesis and bars

Fixed before the run in the design notes:

- **Library probe and index:** `docs/design/benchmarks.md`, "Library probe
  and index against libavformat". CoreAV's time over libavformat's, the
  median at most 1.00 overall and in each corpus category, on W1's headline
  set.
- **HLS index:** `docs/design/hls.md`, "Index against FFmpeg". The HLS
  index's time over libavformat's, the median at most 1.00 overall and in
  each corpus category, over the inputs both open.

"Faster" is said only where the upper bound of the bootstrap 95% interval
is below 1.00.

## Sets and rounds

- **Library probe:** the 303 held-out files of
  `docs/spikes/safe-probe/corpus/holdout-v1.json`. Round 2 of W1 on this set:
  run-05 measured it on stage 2 code, passed on its first round, and the
  set was never used for tuning.
- **HLS index:** the 265 held-out inputs of
  `docs/spikes/hls-bench/evidence/inputs.json`, all of them among the 303.
  Round 1 of this comparison: `docs/spikes/hls-bench/` reported their index
  times without a baseline.

## Method

`w1.py`, an earlier change's driver with a third engine:

- **A-index:** `probe-bench --index --io=mmap`, probe plus the packet index.
- **A-hls:** `probe-bench --hls --io=mmap`. This is
  `coreav_isobmff_write::hls::build_index` with `coreav hls index`'s
  defaults, with the index held until the request ends.
- **B-lean:** `probe-libav --format-mov --lean --io=mmap --buffer=4096`, the
  pinned LGPL libavformat 7.0.2 opening the file.

Each engine is one long-lived process. The three are pinned to one core
and serve one request at a time; the driver runs on another core. After one warm-up pass come 15 rounds, with file order
and engine order shuffled. Files are downloaded in batches of at most
12 GB, verified by SHA-256. The index check against `ffprobe
-count_packets` is untimed. Everything runs on the development machine
(8 vCPU), with nothing else running.

`summarize.py RUN_DIR` computes both comparisons.

## Results

Run `w1-01` measured the crates of `1114aa6` with the `--hls` engine, from
a clean tree, on all 303 files. The commit recorded in the run's evidence, `90cbbc4`, is this branch before its rebase onto `main`; its crates are those of `1114aa6`, on `main`. Evidence: `evidence/w1-01.tar.xz` (the
batches) and `evidence/summary-w1-01.json`.

### Library probe and index: met

| Class | Files | CoreAV / libavformat, median (95% CI) |
|---|---:|---|
| **All** | 232 | **0.316** (0.294 to 0.343) |
| group-a | 9 | 0.498 (0.461 to 0.912) |
| group-b | 19 | 0.307 (0.282 to 0.337) |
| group-c | 36 | 0.538 (0.356 to 0.596) |
| group-d | 106 | 0.291 (0.284 to 0.310) |
| group-e | 19 | 0.280 (0.251 to 0.335) |
| group-f | 34 | 0.338 (0.294 to 0.360) |
| group-g | 9 | 0.553 (0.262 to 0.720) |

Every category's upper bound is below 1.00, so CoreAV is faster than
libavformat overall and in each category. One file is slower, at 1.01: an
audio-only file. Excluded, as W1 defines the headline set:
- 57 files where CoreAV drops a stream (codecs outside the allowlist);
- 13 files whose index counts differ from `ffprobe`'s;
- 1 file that only one side opens.

Against run-05 on stage 2 code (libavformat's time over CoreAV's 2.33, or
CoreAV's over libavformat's 0.43), the ratio fell to 0.316.

### HLS index: missed

| Class | Inputs | `hls index` / libavformat, median (95% CI) |
|---|---:|---|
| **All** | 232 | **0.699** (0.610 to 0.821) |
| group-a | 11 | **1.967** (1.400 to 3.777) |
| group-b | 9 | 0.652 (0.439 to 0.756) |
| group-c | 34 | **1.406** (1.154 to 1.703) |
| group-d | 108 | 0.573 (0.549 to 0.615) |
| group-e | 21 | 0.549 (0.414 to 0.808) |
| group-f | 33 | 0.740 (0.574 to 0.836) |
| group-g | 16 | **2.971** (1.501 to 8.479) |

The bar is met overall but missed in three categories: group-a, group-c and
group-g. 33 inputs that `hls index` refuses are listed in the summary. The
miss is recorded with its cause; the bar does not change.

- **Group-g: not equal work for fragmented files with a segment index.** The
  seven group-g inputs slowest against libavformat (4.2 to 36.8) are all
  fragmented with a segment index (`sidx`). libavformat's open reads the
  segment index and lists one entry per subsegment, without reading the
  fragments. An HLS index needs every sample's size, time and sync flag,
  so it reads the run of every fragment throughout the file. That is why
  the largest of them, 1.3 GiB, takes 36.8 times as long. The premise of
  the bar, that libavformat's open is the same work, does not hold for
  this layout. Setting them aside does not save the class: the other nine
  group-g inputs, without a segment index, have a median of 1.50.
- **Group-a, group-c and group-g: per-sample cost.** Per input, the HLS index
  takes 3.35 times (group-a), 2.59 times (group-c) and 2.71 times (group-g
  without a segment index) as long as listing the samples with the packet
  index (A-index, the same run), medians over the inputs both engines
  open. The extra cost is per sample, at every file size: segment
  boundaries, the per-sample records and the serialized index, listed one
  sample at a time.

An earlier change tracks speeding up the HLS index build. A result after that fix
needs a held-out set that this run did not use.
