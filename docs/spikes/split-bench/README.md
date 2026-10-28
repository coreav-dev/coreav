# Keyframe splits on the corpus

Measurement code, outside the Cargo workspace. It measures
`split_mp4` of `crates/coreav-isobmff-write` and the `coreav split` command
against the corpus bars of [`docs/design/split.md`](../../design/split.md)
on corpus v1. Results are added below as the runs finish.

## Hypothesis and bars

`split_mp4` splits a file at clean keyframes into MP4 segments that
together hold every packet of the kept streams exactly once, each segment
presenting its frames at their source times, at least as fast as
`ffmpeg -f segment` and with memory bounded by one segment's tables. The
bars, quoted from the design note's "Corpus bars", were fixed on
2026-10-19 in the commit that adds this README, before any run:

- **Inputs**: every corpus v1 file of
  `docs/spikes/clip-bench/evidence/inputs.json` (`set: real`, extension mp4,
  mov, m4v or 3gp, a `v:h264`, `v:hevc`, `v:av1` or `v:vp9` tag: 869
  files). Each is split with `--every 10` through `split_mp4` with the
  default limits, allowlist and audio choice; a split that the default
  audio refuses because the audio is not AAC is made again without audio
  and marked `no_audio`, as the clip run did.
- **Checks**: on every file, the CoreAV round trip of the tests and the
  packet coverage of `split_check.py` (ffprobe data hashes), plus the
  per-segment `ffprobe` stream, duration and MediaInfo checks. On files of
  at most 500 MB (790 files), also the decoded checks (`framemd5` of the
  video, decoded audio).
- **Coverage: 100% of split files.** For every split file, every source
  packet of the kept streams is in exactly one segment, in order, by both
  the round trip and `ffprobe`'s packet hashes. Any miss is a bug and
  blocks the merge.
- At least **98% of split files** agree on every check, directly or
  through a known divergence.
- At least **99% of packets** agree.
- Files the writer refuses, and objects that fail their SHA-256 check on
  download, are counted and listed apart, in neither denominator. Every
  other failure counts as disagreement and is investigated.
- **Speed against FFmpeg** (held-out files only): per file, the wall time
  of `coreav split --every 10 SRC DIR/%03d.mp4` against `ffmpeg -i SRC -map
  0:V -map 0:A -c copy -f segment -segment_time 10 -reset_timestamps 1
  DIR/%03d.mp4` with the same streams, both as processes writing to the
  same local disk, the median of 5 rounds with the order alternated, after
  one warm-up of each. Bars: the median of ours ÷ ffmpeg over all held-out
  files at most **1.0**; the median of each class (non-fragmented,
  fragmented, longer than one hour) at most **1.5**.
- **Memory** (held-out files): peak resident memory of `coreav split`
  minus that of `coreav probe` on the same file at most the largest
  segment's `moov` box plus 24 bytes per source sync sample plus 2 MiB, on
  every held-out file.
- Misses on speed or memory block the release, not the merge; the cause is
  recorded before any bar changes.

## Method

- `sbench split SRC OUTDIR` (`src/main.rs`) splits through `split_mp4`
  with `BufferedSource(FileSource)` in and 64 KiB `BufWriter`s out, then
  runs the round trip of `crates/coreav-isobmff-write/tests/split.rs` on
  the segments.
- `check.py` downloads each file from the private corpus bucket (SHA-256
  verified, with `../clip-bench/bench_common.py`), runs `sbench split` and
  `scripts/split_check.py`, writes one JSON record per file and deletes the
  media. Runs on the development machine with the pinned black boxes;
  correctness does not depend on the machine.
- `perf.py` (held-out files) times both engines and records peak resident
  memory with `/usr/bin/time -f %M`. `--all` takes the non-held-out files
  too, which the several-edit class needs (only two of its six files are
  held out; the held-out part is reported apart).
- `speed.py` times two builds of `coreav split` against each other in one
  run, the builds alternated: the "not worse than `main`" instrument of
  [`edit-list-writers.md`](../../design/edit-list-writers.md).
- `identical.py` splits every corpus file with two builds and compares the
  segments byte for byte: the instrument for "every other corpus file
  keeps its split result".
- Spend: the runs use the development machine and R2 reads, which have no
  egress charge; the cloud host is not used.

## Reproduce

```sh
cargo build --release                      # sbench
(cd ../.. && cargo build --release -p coreav-cli)
CLIP_CHECK_THREADS=2 python3 -I check.py --inputs ../clip-bench/evidence/inputs.json \
  --out evidence/runs/check-01 --work /tmp/sbwork --bin /opt/coreav-oracle-bin \
  --sbench target/release/sbench --workers 4
python3 -I summarize.py check evidence/runs/check-01 > evidence/summary-check-01.json
python3 -I perf.py --inputs ../clip-bench/evidence/inputs.json --out evidence/runs/perf-01 \
  --work /tmp/sbperf --bin /opt/coreav-oracle-bin --coreav ../../target/release/coreav \
  --sbench target/release/sbench
python3 -I summarize.py perf evidence/runs/perf-01 > evidence/summary-perf-01.json
cargo run --release --bin sbench-mem -- SRC 10   # live heap of split_mp4

# Sources with several edits (docs/design/edit-list-writers.md).
mkdir -p evidence/runs/check-me-15
git rev-parse HEAD > evidence/runs/check-me-15/commit.txt  # summarize.py keeps it
CLIP_CHECK_THREADS=2 python3 -I check.py \
  --inputs ../clip-bench/evidence/inputs-multi-edit.json \
  --out evidence/runs/check-me-15 --work /tmp/sbwork --bin /opt/coreav-oracle-bin \
  --sbench target/release/sbench --workers 2
python3 -I summarize.py check evidence/runs/check-me-15 > evidence/summary-check-me-15.json
python3 -I perf.py --all --inputs ../clip-bench/evidence/inputs-multi-edit.json \
  --out evidence/runs/perf-me-06 --work /tmp/sbperf --bin /opt/coreav-oracle-bin \
  --coreav ../../target/release/coreav --sbench target/release/sbench
python3 -I liveheap.py --inputs ../clip-bench/evidence/inputs-multi-edit.json \
  --out evidence/runs/mem-me-06 --work /tmp/sbheap --sbench-mem target/release/sbench-mem
# This writer against main (`git worktree add` at the branch point, then
# `cargo build --release -p coreav-cli`), both timed in one run.
python3 -I speed.py --inputs ../clip-bench/evidence/inputs.json \
  --out evidence/runs/speed-me-08 --work /tmp/sbspeed \
  --head ../../target/release/coreav --base /tmp/coreav-base
python3 -I summarize.py speed evidence/runs/speed-me-08 > evidence/summary-speed-me-08.json
# Every corpus file: the same segments from both builds, byte for byte.
python3 -I identical.py --inputs ../clip-bench/evidence/inputs.json \
  --out evidence/runs/same-me-06 --work /tmp/sbsame \
  --head ../../target/release/coreav --base /tmp/coreav-base --workers 3
python3 -I summarize.py identical evidence/runs/same-me-06 > evidence/summary-same-me-06.json
```

## Results

| Bar | Result | Status |
|---|---|---|
| Coverage: 100% of split files | **814 of 814** split files: every packet of the kept streams in exactly one segment, in order, by the round trip and by `ffprobe`'s data hashes (`check-03-final`) | **Met** |
| At least 98% of split files agree | **811 of 814** (99.63%): 637 directly, 174 through known divergences | **Met** |
| At least 99% of packets agree | **15,293,730 of 15,293,730** | **Met** |
| Speed: median ours ÷ ffmpeg at most 1.0 (held out) | **0.194** (95% CI 0.170 to 0.221) over 238 files; p90 0.762, max 1.431 | **Met** |
| Speed: each class's median at most 1.5 | non-fragmented 0.192 (233 files), longer than one hour 0.252 (6), fragmented 0.289 (5) | **Met** |
| Memory: growth within the bound on every held-out file | `perf-01`: 223 of 238; the 15 misses re-measured after the fix (`perf-02`): **15 of 15** within, so 238 of 238 (the fix only lowers the other 223) | **Met** after the fix |

### Correctness (`check-03`, with `check-03b`)

All 869 inputs, split every 10 s by `split_mp4` and checked by the round
trip and `scripts/split_check.py`, with the final writer and checker
(`check-03.commit`). One file, `371baf1bdb08` (AV1), stopped the checker:
`ffmpeg` 7.0.2 aborts (`Assertion pkt failed`) decoding that source with
its default threads. `check-03` was started without
`CLIP_CHECK_THREADS=2`, which makes the checker retry such an abort with
one thread, as the earlier runs and the clip runs did; `check-03b` is that
file again with it (it agrees through known divergences), and
`check-03-final` is `check-03` with that record replaced. Earlier runs:
`check-01` was stopped when `sbench` was rebuilt during the run;
`check-02` ran an earlier writer and checker (the same 811 of 814 files
agree, 15,292,970 packets; the checker then listed the source with its
edits and the segments without, which `check-03` corrects).

- **Split**: 814 files (740 also decoded, those of at most 500 MB; 62
  without audio because it is not AAC).
- **Refused** (counted apart, in neither denominator): 50. 21 with no
  allowed video track (encrypted `encv` video, which the allowlist drops),
  13 for video other than `avc1`, `hvc1`, AV1 or VP9 (HEVC as `hev1`), 16
  for an edit list outside the supported model.
- **Malformed sources** (refused by the parser): 4. A movie box without a
  movie header, a second movie box, and two truncated files. Counted as disagreements they would make
  811 of 818 (99.14%).
- **Download failed**: 1. The object `184becaa…`, a damaged duplicate
  that an earlier change removes from the corpus manifest, did not match its SHA-256 in
  `check-02` and was no longer in the bucket in `check-03`; it is listed
  apart, not split.
- **The 3 disagreements**, all in the decoded video, none in packets:
  - `4c82d16e5037` (held out) and `b145779633e3`: the source's `stss`
    marks frames as sync that a decoder cannot start from. The segments
    copy the container's statement and start on such a frame; `ffmpeg`
    decoding a segment from its start drops or conceals frames up to the
    next frame it can start from (as `KD-C-SYNC-FLAGS` describes for
    fragment defaults). Every packet agrees.
  - `e5715db399f5`: the source's edit list ends inside its last frame;
    `ffmpeg` shows that frame for the source and drops it for the one
    segment, whose edit ends at the same time. Every packet agrees.
- **Known divergences** covering files: `KD-S-AUDIO-RESTART` 2,057
  segments, `KD-C-AAC-PROFILE` 74, `KD-C-EMPTY-EDIT-DURATION` 57,
  `KD-C-STTS-SHORT` 40, `KD-S-EDIT-MIDFRAME` 17, `KD-S-PTS-SHIFT` 2.

### Speed (`perf-01`)

254 held-out files, 16 refused, 238 timed; Intel Xeon at 2.9 GHz, 8 vCPU,
no noisy file (no engine with a round 3 times its fastest). Median wall
time per file 4.7 ms for `coreav split` against 26 ms for `ffmpeg -f
segment`. The slowest ratios (1.43, 1.42, 1.36) are files of 0.9 to 2.6
GB.

### Speed again on current code (`perf-03`)

Round 2 of the speed measurement: the same 254 held-out files as `perf-01`,
which passed on its first round and was never used for tuning. Same method
and machine, on the crates of `1114aa6` (a `release` build, as `perf-01`).
The commit recorded in the run's evidence, `dd60869`, is this branch before its rebase onto `main`; its crates are those of `1114aa6`, on `main`. The bar is the default speed bar against FFmpeg, fixed in
`docs/design/split.md` before the run: at most 1.00 overall and per class (the class bar of 1.5
had no written reason).

| Class | Files | Ours / ffmpeg, median (95% CI) |
|---|---:|---|
| **All** | 241 | **0.208** (0.186 to 0.250) |
| Non-fragmented | 234 | 0.197 (0.184 to 0.238) |
| Fragmented | 7 | 0.304 (0.254 to 0.514) |
| Longer than one hour | 6 | 0.220 (0.136 to 0.391) |

**The bar is met.** Split is faster than FFmpeg's segment muxer in every
class, every upper bound below 1.00.
- 13 files are refused, against 16 in `perf-01`: the cut-point and
  edit-list work since let three more split.
- Median wall times are 4.9 ms for `coreav split` and 24 ms for `ffmpeg -f
  segment`.
- 12 files are slower than FFmpeg, 1.01 to 1.43 (the summary lists the
  five slowest, 1.30 to 1.43, as `perf-01` did).
- The memory bound holds on all 241 files.
- The `npx` rounds 1 and 2 (`docs/measurements/npx-coreav.md`) also timed
  `coreav split` on held-out files of at most 128 MiB, most of them in this
  set.

Evidence: `evidence/perf-03.json.gz`, `evidence/perf-03-machine.json` and
`evidence/summary-perf-03.json`.

### Memory (`perf-01`, `perf-02`)

`perf-01` missed the bar on 15 of 238 held-out files, all with many
segments (135 to 786): the growth was about 15 KB per segment, 11.6 MB
for 748 segments against a bound of 2.1 MB. Cause, measured before any
change: `split_mp4` was not it. `sbench-mem` (`src/bin/sbench_mem.rs`, the
counting allocator of `clip-bench`) puts the live heap of a split of a
generated two-hour file into 720 segments at 0.59 MB above its probe's,
within the bound. `coreav split` built its whole `coreav.split/1`
summary as a `serde_json` value tree and then as text, about 17 KB per
segment: on the same file, 18.9 MB peak resident with the summary and 6.7
MB without it, against 5.9 MB for `coreav probe`. The command now writes
the summary one segment at a time, with the same bytes ("fix(cli): stream
the split summary one segment at a time"): 6.7 MB for 720 segments and
7.2 MB for 1,440 (`evidence/memory-generated.json`, with the command that
generates the file). The bar is unchanged. `perf-02` re-measured the 15
files with the fixed command (one timing round; memory is measured
apart from the timing rounds): all 15 within the bound,
the closest at 1.36 MB against 2.13 MB (786 segments, 64% of the bound). The other 223 files passed with the
summary built whole, and the fix only lowers memory.

### Spend

0 USD: the development machine and R2 reads (no egress charge); the cloud host was
not used.

## Sources with several edits

The split writer takes edit lists with several media edits, each segment
carrying the windows in its range. The bars and the method are
[`docs/design/edit-list-writers.md`](../../design/edit-list-writers.md),
the split section, fixed before these runs. Machine: the development
machine, Intel Xeon at 2.9 GHz, 8 vCPU
(`evidence/perf-me-06-machine.json`); correctness does not depend on it.

| Bar | Result | Status |
|---|---|---|
| Fixtures: every `edit_multi_*` fixture with a video track, 100%, directly | **8 of 8** splits (`every1` and `at`), no known divergence; all 128 fixture splits pass, 36,014 of 36,014 packets | **Met** |
| Corpus: the six files with several edits accepted, 100% | **6 of 6** split, **634,122 of 634,122** packets in exactly one segment in order; 5 directly, 1 through `KD-C-EMPTY-EDIT-DURATION` (`check-me-15`) | **Met** |
| Corpus: every other file keeps its split result | **824 of 824** compared files give the same segments, byte for byte, from this writer and from `main`; 38 refused by both with the same message; the 6 above taken by this writer only. 868 of the 869 inputs: `184becaa…`, the damaged duplicate an earlier change removes from the manifest, is no longer in the bucket, as `check-03` also found (`same-me-06`) | **Met** |
| Split then join | The segments of a spliced source join back into the source's samples, byte for byte, with their times and the source's two windows; a segment that carries several windows is reported as not joinable (tests) | **Met** |
| Speed, class `several edits`, median at most 1.00 against `ffmpeg -f segment` | **0.180** (95% CI 0.096 to 0.210), 6 files, **100% faster than FFmpeg** (`perf-me-06`) | **Met** |
| Speed, class `several edits, held out` | **0.096** (0.035 to 0.158), 2 files, 100% faster | **Met** |
| Not worse than `main` on sources without several edits | **0.992**, **0.991** and **0.990** over three runs of 20 files, both builds in each run with the order alternated (`speed-me-08` to `speed-me-10`) | **Met** |
| Memory within the bound of [`split.md`](../../design/split.md) | **6 of 6**; the closest is 6e67814d336c at 0.43 of its bound by resident memory (`perf-me-06`) | **Met** |

### Correctness (`check-me-15`)

The six corpus files of `../clip-bench/evidence/inputs-multi-edit.json`,
split every 10 s by `sbench split` and checked by the round trip and
`scripts/split_check.py` with the final writer and checker (the commit
is the `commit` field of `evidence/summary-check-me-15.json`). 471 and 405 segments for the two
long files; the 1.2 GB file is over the 500 MB decode limit, so its
decoded checks are skipped, as the protocol has it.

- The one known divergence is `KD-C-EMPTY-EDIT-DURATION` on the last
  segment of 6e67814d336c (audio duration 6.258345 s against edits
  6.256701 s), an existing clip entry; its audio has one edit.
- **Second view**, reported: every difference is named by ffmpeg's
  measured edit model — "times anchored on decode times", "an empty edit
  between media edits listed as media", "the first edit snapped to its
  first composition time" and "frames emitted out of presentation order,
  their times clamped by ffmpeg's output". None is unexplained.

### Speed and memory (`perf-me-06`, `mem-me-06`)

Median wall time per file 4.5 ms for `coreav split` against 87 ms for
`ffmpeg -f segment`; no noisy file. The slowest ratio of the six is 0.223
(a166d09989f9, 1.2 GB: 0.67 s against 3.00 s).

**Against `main`.** Three runs of `speed.py` on the same 20 files, this
writer and `main` built from the same tree and timed in each run with
the order alternated: 0.992 (95% bootstrap 0.980 to 0.998), 0.991 (0.981
to 0.997) and 0.990 (0.972 to 0.999). The segments themselves are byte
for byte `main`'s on all 824 corpus files without several edits.

Live heap of `split_mp4` above its probe's, under the test-only counting
allocator (`sbench-mem`), beside the resident-memory growth `perf.py`
measures:

| File | Segments | Live heap | Resident | Bound |
|---|---:|---:|---:|---:|
| ceb55bb5fb87 | 1 | 74,642 | 417,792 | 2,105,208 |
| 5de2646e4ea0 | 3 | 134,759 | 172,032 | 2,104,953 |
| 1ae48e7b157e | 2 | 138,662 | 294,912 | 2,105,821 |
| ea690039d902 | 5 | 177,467 | 8,192 | 2,102,834 |
| 6e67814d336c | 471 | 507,295 | 909,312 | 2,126,292 |
| a166d09989f9 | 405 | 1,879,776 | −3,096,576 | 2,181,231 |

Bytes; a negative resident growth means `coreav probe` holds more than
the split does on that file. a166d09989f9's live heap is the largest of
the six, 0.86 of its bound. Keeping one timeline rather than a copy of
the edit list's checkpoints in every segment's plan took 155,008 bytes
off it; its audio has 6,069 edits (measured with `sbench-mem` before and
after that change, not committed evidence).

### Fuzzing

The `split` target of `crates/coreav-isobmff-write/fuzz`, seeded with the
`edit_multi_*` fixtures: 460,000 executions, no crash and no failed
assertion.

### Spend

0 USD: the development machine and R2 reads (no egress charge); the cloud host was
not used.
