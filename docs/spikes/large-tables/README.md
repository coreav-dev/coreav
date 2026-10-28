# Large sample tables against libavformat

Throwaway measurement code, outside the Cargo workspace.: every measured case where CoreAV was slower than libavformat is a file
with a large sample table. This spike profiles the sample-table path on
tuning files, changes the library, and measures again on a held-out set
this work never tuned on.

## Hypothesis and bars

CoreAV's probe plus a full sample index is not slower than libavformat
opening the same file in-process, on files with large sample tables as on
the rest.
The bars, fixed before the runs (2026-10-19):

- **Equal work (W1's A-index against B-lean, W1's equal-work method).** The
  per-file ratio is CoreAV's time over libavformat's (the inverse of W1's
  speed-up). Bar: the median ratio is at most 1.00
  overall and in every corpus category, on the headline set (both open the
  file, CoreAV drops no stream, and its index has as many entries as
  `ffprobe` lists packets, or the file has a segment index, as W1 defines
  it). "Faster" is said only where the upper bound of the bootstrap 95%
  interval is below 1.00.
- **Every file.** An earlier change's goal: no file slower than libavformat, or each
  remaining one explained below. Reported per file, without a further bar.

## Sets

`sets.py` writes `sets.json` from corpus v1's manifest, before any tuning:

- **Held-out set of an earlier change:** within each `real` category, files ordered by
  `sha256("coreav-corpus-v1-holdout-issue30:" + sha256)`, the first 30%, as
  v1's own held-out set is drawn; ISO base media and QuickTime files only
  (`c:isobmff`), as W1. 320 files, 59.4 GB.
- **Tuning pool:** the other `c:isobmff` files of `real` that are not
  held out in v1 either (525). Profiling and tuning used only these.
- Tuning files (12, `tuning.json`): the four largest audio-only AAC files,
  three AV1 files, three long group-c recordings and two group-e recordings
  of the pool.

## Method

- **Engines:** W1's `probe-bench` (built against this
  branch) with `--index --io=mmap` (A-index), and `probe-libav` with the
  pinned LGPL FFmpeg 7.0.2 libavformat from the lock file of the
  parsing-guard spike's baseline (micromamba 2.3.2, checksum-pinned), with
  `--format-mov --lean --io=mmap --buffer=4096` (B-lean).
- **Timing:** each engine one long-lived process pinned to
  its own core, the driver on a third, the same glibc allocator settings;
  one warm-up pass, then 15 rounds, file order shuffled every round and
  engine order every file; a file's time is the median of its rounds.
- **Batches:** the held-out set does not fit on the development machine's
  disk at once, so it is measured in batches of files of at most 12 GB,
  each batch with its own warm-up and 15 rounds (W1 shuffled across all
  files at once). Files are assigned to batches by their sha256 order.
- **Index check:** CoreAV's index entries against the black-box `ffprobe
  -f mov -ignore_editlist 1 -nofind_stream_info -count_packets` with the
  pinned binary, untimed, as W1.
- **Machine:** the development machine (8 vCPU), otherwise idle, unpinned
  frequency. Statistics as W1: medians over files, bootstrap 95% intervals
  over files.

## What changed (profiling on the tuning files)

Profiled with callgrind on the tuning files (`tuning.json`), never on
held-out ones. The slow cases were per-sample overhead: for a variable-size
AAC track every sample is its own run, and each run went through five
layers of calls, re-checking every table. Changes, all in safe Rust:

- `Packets::next_runs` appends runs in batches, the same runs, checks,
  errors and work as `next_run` one by one, work charged in the same order
  so that a budget running out stops both at the same run (tests/runs.rs
  compares them on synthetic and mutated movies, on generated tables with
  several chunks, multi-sample `stts` and `ctts` entries and hostile `stss`
  under small budgets, mixing both calls, and on 4.3 million runs of the
  tuning files and fixtures). Inside, a cursor loop steps the samples of
  the current chunk and table entries without re-reading the other tables,
  sync samples included. The W1 harness builds its index with it.
- Box payloads and sample tables are read into reserved capacity
  (`Source::read_append`) instead of a zeroed buffer.
- The `stsz` total sums each byte position in its own lane, without a byte
  swap or an overflow check per entry (a unit test compares it with the
  plain sum).

On the slowest tuning file (an audiobook with 981,000 samples), A-index
went from 307 to 209 million instructions, as many as libavformat's; its
probe alone from 9.9 to 4.6. On the tuning AV1 file, whose samples are half
sync samples, from 45.6 to 29.0 million.

## Results

### Equal work (W1's A-index against B-lean)

`evidence/summary-w1-01.json` (records in `evidence/w1-01.tar.xz`), the
harness built from this branch at the commit that updates its lock file
(`0c9ec87` before the branch's last rebase onto `main`), that is with the
batch reader, unzeroed reads and the plain `stsz` sum, before the
sync-sample, lane-sum and SIMD changes and before the batch loop charged
its work units in `next_run`'s order (the same count), on the 320 held-out
files: 220 in the headline set (77 excluded because CoreAV drops a stream, 20 because the index counts
differ, 3 not opened by both; W1 also kept files with a segment index whose
counts differ, which this harness does not detect, so they are among the
20). **Every bar is met.**

| | Files | CoreAV / libavformat, median (95% CI) | Slowest file |
|---|---:|---|---:|
| **All** | 220 | **0.313** (0.295 to 0.332), measured before the sync-sample fix | 1.20 |
| group-a | 10 | 0.497 (0.486 to 0.773) | 1.05 |
| group-b | 14 | 0.306 (0.285 to 0.320) | 0.49 |
| group-c | 31 | 0.510 (0.342 to 0.550) | 0.91 |
| group-d | 105 | 0.286 (0.275 to 0.314) | 1.20 |
| group-e | 16 | 0.308 (0.283 to 0.388) | 0.95 |
| group-f | 32 | 0.319 (0.288 to 0.347) | 0.52 |
| group-g | 12 | 0.293 (0.259 to 0.358) | 0.71 |

CoreAV is faster than libavformat overall and in every category (the upper
bounds are below 1.00). Eight files are slower (`slower_than_libavformat`
in the summary):

- Seven AV1 recordings of one group (1.03 to 1.20): half their
  samples are sync samples, which the batch loop then handed to the general
  step one by one. The change "sync samples included" above came after this
  run: the 0.313 and every number in this table were measured before it. It
  was found from these held-out results and checked on the tuning AV1 file
  only (CoreAV / libavformat 1.35 before, 0.77 after, tuning files only); no
  held-out claim is made for it, since re-measuring it on the same held-out
  set would tune on it.
- One audio-only AAC file (1.05, 77,088 samples): the long variable-size
  AAC tables, where CoreAV and libavformat now execute about as many
  instructions per sample.
