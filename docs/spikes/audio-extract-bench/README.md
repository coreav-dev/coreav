# Release measurements: audio extraction to M4A

Throwaway measurement code, outside the Cargo workspace. It
measures `extract_audio` of `crates/coreav-isobmff-write`
against its release bars on corpus v1. The
first runs found two timing cases the record did not settle and a memory
miss; the second found one file 1.58 times slower than libavformat because
of its read pattern. The design was amended and the code changed after
each; the final runs measure commit `c80db9d`. All three rounds are
reported.

## Hypothesis and bars

The design of the extractor claims it copies an AAC track into a valid M4A file
with the source's packets and timing, at least as fast as libavformat
remuxing the same track, with memory bounded by the output tables. The bars,
quoted from its design record and fixed before these runs (the
amendment changes none of them):

- **Correctness (point 9).** "Round trip: `coreav-isobmff` probes every
  output; stream metadata and packets (sizes, decode times, durations) equal
  the source track's." Black-box oracles: "`ffprobe` and MediaInfo read the
  output as a valid M4A with the source's codec, sample rate, channels and
  duration; packet hashes from `ffprobe -show_packets -show_data_hash` equal
  the source stream's; and `ffmpeg -f framemd5` of the decoded output equals
  that of the source's audio stream." Threshold: "on the real corpus, at
  least 98% of files with an extractable AAC track, with every failure
  investigated and recorded."
- **Speed (point 10).** "Baseline: libavformat remuxing the same track
  in-process (stream copy, black-box benchmark boundary), and
  the `ffmpeg -vn -c:a copy` command line for reference. Bar: median
  per-file time at most 1.10 times the in-process baseline, and no file
  class (non-fragmented, fragmented, longer than one hour) above 1.5 times.
  Reported separately: time to the first output byte."
- **Memory (point 10).** "Peak memory growth beyond the parser's own use is
  bounded by the output sample tables plus one fixed copy buffer,
  independent of the file size; checked with the test-only counting allocator
  on a file longer than one hour and on a 2 GB fragmented file."
- **Remote input (point 10).** Bytes read over `RangeReader`
  as a share of the object, and the time against a full download plus local
  extraction. "Reported, not gated."
- "Misses on speed or memory block the release, not the merge of the code;
  the cause is recorded before any bar changes."

## Results at a glance (final code)

| Bar | Result | Status |
|---|---|---|
| Correctness: 98% of the 518 files with an extractable AAC track | **518 of 518 (100%)**: 274 (52.9%) agree directly, 244 through the five known divergences of `docs/known-divergences-extract.md`; none unexplained. The round trip finds no difference in 16,369,353 samples | **Met** |
| Speed: median ≤ 1.10× in-process libavformat (152 held-out files) | **0.384×** (95% CI 0.368 to 0.398), p90 0.573 | **Met** |
| Speed: no class above 1.5× | non-fragmented 0.386× (148), fragmented 0.339× (4), longer than one hour 0.407× (10) | **Met** |
| Memory: growth ≤ output tables + 64 KiB copy buffer | **153 of 153** within the bound (closest: 44 bytes under); 7.6-hour file 4.82 MB against a 4.89 MB bound; 2.2 GB fragmented file 0.50 MB against 0.51 MB | **Met** |
| Remote (reported) | the range path requests 0.66 of the object at the median, up to 21×; never faster than a download plus local extraction (measured 40.5× slower at the median on 41 files, modelled 98.7× on 152); measured before the third round | Reported |

## Three rounds

| | First round (`bccc98d`) | Second round (`4fb70e0`) | Final (`c80db9d`) |
|---|---|---|---|
| Runs | `check-01`, `perf-01` + `perf-02`, `remote-01`, `remote-02` | `check-04`, `perf-04` (`check-02`, `check-03`, `perf-03` before) | `check-05`, `perf-05` (choice made in `exp-window-01` to `04`, development files) |
| Correctness | 45.2% direct; 90.9% with candidate divergences; 47 files (9.1%) an open question | 52.9% direct; 100% with the accepted divergences | the same |
| Speed (median, max) | 0.534×, 1.406× | 0.589×, 1.577× (3.70 GHz machine) | **0.384×**, 3.006× (timing noise, below); `0388073c…` 0.573× |
| Memory | 0 of 153 within the bound, growth up to 2.09× | 153 of 153 within | 153 of 153 within |

What the first round found, and what changed when the design was amended
on 2026-10-18 (the code changes are in this pull request):

1. **Declared media duration.** 47 sources declare a media header duration
   that differs from the sum of their sample durations (the duration
   conflict warning), by under 1 ms on most and up to 161 ms
   on some files.
   The extractor wrote the sum, so the round trip lost the warning and
   ffprobe reported a different duration on 24 tracks. It now copies the
   declared duration (sources without fragments). In the final round 37 of
   these files agree directly and 10 through `KD-X-TKHD` only.
2. **Encoder delay in `iTunSMPB`.** 16 tracks give their 2,112-sample
   encoder delay only in the iTunes `iTunSMPB` tag, which the extractor did
   not copy (point 6), so their outputs started 48 ms late. Where the source
   has no edit list and the tag adds up exactly to the samples (9 tracks),
   the output now gets an edit list from it; the decoded audio is then
   identical to the source's frame for frame. ffmpeg still places the
   source's content at the delay and the output's at 0, which
   `KD-X-GAPLESS-EDIT` covers. The other 7 tracks (iTunes previews whose tag
   describes the full original recording and disagrees with their edit
   list) keep the edit list: `KD-X-GAPLESS-CONFLICT`.
3. **framemd5 origin.** 41 framemd5 differences came only from ffmpeg moving
   the earliest start among all of a file's streams (often video) to 0; the
   comparison now runs with `-copyts` on both files.
4. **Memory.** The sample size and chunk tables grew by doubling their
   capacity (growth values sat on power-of-two steps), and each chunk took
   16 bytes in memory against 4 in `stco`. The tables are now kept
   serialized in 16 KiB blocks trimmed after the first pass, sizes are
   listed only once two differ, chunks are recomputed when written, the
   movie box is sized without building the chunk tables twice, and the
   sample entry is freed before copying. `check-03`/`perf-03` still showed
   one file 50 KB over (the second set of 8-byte chunk offsets used to size
   the movie box) and one fragmented source whose 1 ms MediaInfo difference
   `KD-X-TKHD` did not yet cover; `check-04`/`perf-04` measure the fix.
5. **Copy reads (second round, amendment point 5).** `0388073c…` (1.0 GB,
   57 min, audio 10% of the bytes, finely interleaved with video) took 1.58×
   libavformat's time: the copy pass made 99,713 reads of about 5 KB (one
   per one or two samples), where libavformat read the whole file in 15,412
   reads of 64 KiB. Each read has a fixed cost that is high in the cloud host
   container (about three times this repository's development machine, where
   a similar development file stays at 0.52×), and serverless platforms are
   sandboxed alike. The 64 KiB copy buffer now also serves as a read window:
   a sample outside it refills it from the sample's start, so the samples
   that follow within 64 KiB need no read of their own; this reads the other
   tracks' bytes in between. It was chosen on 45 development files only
   (21 long interleaved recordings and 24 others, `evidence/only/`):

   | Run (development files, the cloud host) | Variant | Median | Max | Long interleaved median |
   |---|---|---:|---:|---:|
   | `exp-window-01` | second-round code | 0.523× | 1.476× | 0.657× |
   | | 64 KiB window | 0.414× | 0.689× | 0.427× |
   | | window growing from 4 KiB while samples stay close | 0.446× | 0.960× | 0.462× |
   | `exp-window-04` | second-round code | 0.529× | 1.437× | 0.634× |
   | | **final: 64 KiB window, large reads past the `BufferedSource` window** | **0.344×** | **0.657×** | **0.401×** |

   The window's 64 KiB reads made the caller's `BufferedSource` keep a
   second, growing buffer; `exp-window-03` showed one development file 1 MiB
   over the memory bound when such a read started inside its window and the
   tail counted as a sequential read. `BufferedSource` now reads requests of
   at least 64 KiB straight into the caller's buffer wherever its window does
   not hold them (`exp-window-04`: 45 of 45 within the bound; no file more
   than 5% slower than the second-round code). `exp-window-02` failed on a
   DNS error while downloading and measured nothing. `xbench-mem`'s parser
   phase reads the samples with the same window, so the baseline does the
   same source work as the extraction.
6. **Harness.** `perf-01`'s discovery skipped tracks with a round-trip
   finding (11 held-out files, unrelated to speed); `perf-02` timed them, and
   `perf.py` now times the first track that extracts. The first remote run
   (`remote-01`) was stopped after two files (below); its `BrokenPipeError`,
   also seen in the earlier local dry run, was the time limit killing the
   driver.

## Method

### Inputs

- **Correctness:** every `set: real` file of the corpus v1 manifest
  with the tag `a:aac` and an ISO base media
  extension (`mp4`, `mov`, `m4a`, `m4b`, `m4v`, `3gp`): **544 files** (157 held out, 63.1 GB),
  `evidence/inputs.json` (`make_inputs.py`, manifest sha256 `5aac0e74…`).
  About 537 were expected; the difference is the seven `m4b` files, which
  are kept.
- **Performance and remote:** the 157 held-out files only. The
  2 GB fragmented file is generated: no held-out file and no file of the
  large fragmented corpus is a fragmented file of
  at least 2 GB with AAC, and the requirement's own test of streamed
  fragments uses a generated ("virtual") file.
  It is the pinned ProRes fixture of `scripts/gen-fixtures.sh` looped by
  stream copy, with an AAC sine track and 1 s fragments: 2,202,489,078
  bytes, 38.5 minutes of audio, reported apart from the held-out numbers.
  Nothing was tuned on the held-out files; local smoke runs used
  development files only.
- Media came from the private corpus bucket, every
  object verified by SHA-256, and was deleted after use. No media is
  committed; files are named by sha256.

### Engines

| Engine | What runs | Role |
|---|---|---|
| `ours` | `xbench time … extract`: `extract_audio` with default limits and allowlist, `BufferedSource(FileSource)` in, 64 KiB `BufWriter` out | measured |
| `ours_probe` | open and probe only | sanity: never slower than `ours` |
| `lav_avio` | `baseline/remux.c`: libavformat 7.0.2 (pinned LGPL package of Spike C) remuxes the stream to `ipod` (M4A) by stream copy, other streams discarded, custom I/O with 64 KiB buffers on both sides | **headline baseline** |
| `lav_native` | the same with libavformat's own file I/O | sensitivity |
| `lav_faststart` | `lav_native` with `movflags +faststart` (`moov` first, as ours) | sensitivity |
| `cli` | `ffmpeg -vn -c:a copy -f ipod` (7.0.2 static), wall time of the process | reference only |

Both in-process engines time from opening the source to the flushed output
inside the process (no process start-up), with the same output buffer size;
bytes read and read calls are counted on both sides. `remux.c` checks at
start that it runs libavformat 7.0.2 under the LGPL; it was written from the
public API documentation only.

### Runs (the cloud host)

| Run | Container | What | Wall time |
|---|---|---|---|
| `check-01` | 4 × (2 CPU, 4 GiB), one shard each | `check.py`, all 544 files, first round | 7 to 12 min per shard |
| `perf-01` | 2 CPU, 16 GiB, GCP us-west, 2.45 GHz | `perf.py`, held-out and generated files, first round | 20 min |
| `perf-02` | 2 CPU, 16 GiB, GCP us-east | the 11 held-out files `perf-01` skipped | 11 s |
| `remote-01` | 2 CPU, 4 GiB, GCP eu-south | first remote attempt, stopped after 2 files | 7 min |
| `remote-02` | 2 CPU, 4 GiB, GCP eu-west | `remote.py` with exact request counts | 46 min |
| `check-02`, `check-03` | as `check-01` | the amended code; predicate fixes in between | 5 to 11 min per shard |
| `perf-03` | 2 CPU, 16 GiB, GCP us-west, 3.09 GHz | the amended code before the movie-box sizing fix | 19 min |
| `check-04` | as `check-01` | second-round correctness | 6 to 10 min per shard |
| `perf-04` | 2 CPU, 16 GiB, GCP us-east, 3.70 GHz, AVX-512 | second-round speed, first byte and memory | 16 min |
| `exp-window-01`, `-03`, `-04` | 2 CPU, 16 GiB, 3.09 to 3.10 GHz | read-window variants on 45 development files (not reported as results) | 9 min each |
| `exp-window-02` | as above | failed on a DNS error while downloading | 1.5 min |
| `check-05` | as `check-01` | **final** correctness | 4 to 10 min per shard |
| `perf-05` | 2 CPU, 16 GiB, GCP us-east, 3.71 GHz, AVX-512 (the machine type of `perf-04`) | **final** speed, first byte and memory | 15 min |

- Correctness per file: `xbench extract` extracts every AAC track and runs
  the round trip (`src/lib.rs`: stream metadata including the media header
  duration and warnings, then every packet's size, decode time, duration,
  presentation time, keyframe flag and bytes; an edit list the output adds
  from `iTunSMPB` is checked against the tag). Then `scripts/extract_check.py`
  runs ffprobe/ffmpeg 7.0.2 and MediaInfo 26.10 (installed by the
  repository's pinned scripts) and applies the known divergences; failed
  checks also get diagnostics in the record.
- Speed per file: one warm-up of every engine, then 5 rounds with the engine
  order rotated each round, inputs on local disk (page cache warm after the
  warm-up), outputs deleted outside the timed region; the median of the 5 is
  the file's time, and the ratio `ours / lav_avio` per file is the unit.
  CIs are distribution-free order-statistic intervals of the median.
- The CPU frequency cannot be pinned on a cloud host (`frequency_pinned: false`);
  `/proc/cpuinfo` is recorded at start and end of each run (constant within
  each run).
- Memory: `xbench-mem` (counting allocator copied from the parsing-guard
  spike, confined test-only `unsafe`). Phase `parser`:
  probe plus two listings of the track, the second reading every sample
  through the same buffered source with the extractor's 64 KiB read window.
  Phase `extract`: `extract_audio` into
  `io::sink()`. Growth = extract peak − parser peak; bound = the output's
  `stts`, `stsc`, `stsz`, `stco`/`co64` and `stss` box bytes + 64 KiB.

## Correctness (point 9, `check-05`)

All 544 files ran; no harness error.

| Outcome | Files |
|---|---:|
| No allowed AAC track: encrypted `enca` entries dropped by the allowlist (10); the parser rejects the file (3: truncated, missing movie header, second movie box) | 13 |
| AAC track present but every one rejected as unsupported (expected): edit list outside the model (9), non-zero first decode time without an edit list (2), composition offsets in an audio track (1), a gap or overlap between decode times (1) | 13 |
| **Files with an extractable AAC track** (520 tracks) | **518** |
| … every check agrees directly | 274 (52.9%) |
| … every difference covered by a known divergence | 244 (47.1%) |
| … uncovered differences | 0 |

Of the 518, 152 are held out (69 direct, 83 through known divergences).

| Known divergence | Tracks | What differs | Why the output is right |
|---|---:|---|---|
| `KD-X-TKHD` | 218 | MediaInfo duration and frame count, which follow the track header | The output's track header is the sum of its edits (ISO/IEC 14496-12 8.3.2) or its media duration rounded to movie ticks; the source's differs (it leaves out an empty edit, holds the movie duration, or in fragmented files describes no samples) |
| `KD-X-EDIT-DISCARD` | 31 | `nb_frames` against the listed packets | ffprobe hides samples outside the edit on both sides; the output keeps every sample |
| `KD-X-GAPLESS-EDIT` | 9 | start time, durations, packet and frame times, all by exactly the encoder delay | The output's edit list from `iTunSMPB` maps the delay to time 0 (amendment point 2); the decoded audio is identical |
| `KD-X-GAPLESS-CONFLICT` | 7 | the first and last decoded frames | The source's edit list and tag disagree; the output keeps the edit list (point 4) |
| `KD-X-PROFILE-UNNAMED` | 4 | stream profile | ffprobe names `LC` only in the one-stream output; the configuration bytes are identical |

A track can be covered by several entries. Each entry's predicate rechecks
its conditions from the files' box headers and the oracles' output, and its
near misses are tested (`tools/oracle/tests/test_extract_kd.py`).
Per-file outcomes are in `evidence/summary.json` (`correctness.per_file`)
and the raw records in `evidence/check-05.json`. `check-04` (before the read
window) gave the same outcome for every file.

## Speed (point 10, `perf-05`)

Per-file ratio `ours / lav_avio`, 152 held-out files with an extractable
track (5 held-out files have none: 4 encrypted, 1 rejected), lower is faster.

| Class | Files | Median | 95% CI | p90 | Max |
|---|---:|---:|---|---:|---:|
| All held out | 152 | **0.384** | [0.368, 0.398] | 0.573 | 3.006 |
| Non-fragmented | 148 | 0.386 | [0.369, 0.398] | 0.573 | 3.006 |
| Fragmented | 4 | 0.339 | [0.315, 0.439] | 0.411 | 0.439 |
| Longer than one hour | 10 | 0.407 | [0.283, 0.672] | 0.641 | 0.672 |
| Equal work only (libavformat copies every sample) | 142 | 0.383 | [0.368, 0.397] | 0.573 | 3.006 |
| Sensitivity: against `lav_native` | 152 | 0.361 | [0.348, 0.374] | 0.491 | 3.222 |
| Sensitivity: against `lav_faststart` | 152 | 0.327 | [0.316, 0.342] | 0.473 | 2.532 |
| Reference: against the `ffmpeg` command line | 152 | 0.036 | [0.033, 0.042] | 0.082 | 0.374 |
| Generated 2.2 GB fragmented file (not held out) | 1 | 0.445 | | | |

- **Timing noise.** `perf-05` had noisy rounds: 30 of its 304 engine-file
  timings vary by more than 3× across the 5 rounds, against none in
  `perf-04` on the same machine type, and libavformat's included. The two
  files above 1× are such cases, both small: `d4847adafd…` (1 MB, 17 reads)
  timed 1.6 to 30 ms for the same binary (3.006×; 0.605× in `perf-04`), and
  `93e504108e…` (9 MB, 16 reads) 0.6 to 30 ms, with libavformat 1.6 to 38 ms
  (1.213×). The medians and CIs above are robust to them; the held-out run
  was not repeated.
- **Same machine type, before and after.** `perf-04` and `perf-05` ran on
  the same machine type (3.70 GHz, AVX-512): the median moved from 0.589×
  to 0.384× with the read window, `0388073c…` from 1.577× to 0.573× (13,985
  reads instead of 99,713). On the 3.09 GHz machine of `perf-03` the
  second-round code measured 0.536×.
- **Equal work.** On 10 files libavformat copies fewer packets than the
  track has samples (it drops samples outside the edit: 474 of 597 on
  `97b724ad…`); ours copies every sample. Without them the median is
  0.383.
- **Sanity.** `ours_probe` is faster than `ours` on every file; the `ffmpeg`
  command line is slower than in-process libavformat on every file.
  `lav_faststart`, which rewrites its output at the end, was faster than
  `lav_native` on 7 files: timing noise.
- **Fixed cost and throughput.** Open and probe: ours 0.106 ms median
  (p90 1.4 ms); libavformat's `avformat_open_input` 0.865 ms (p90 9.4 ms).
  After that, output bytes per second on the 38 files with at least 10 MB of
  audio: ours 749 MB/s median, libavformat 336 MB/s; without the open the
  per-file ratio is 0.533 at the median.
- **Bytes read.** With the read window ours reads 0.94× libavformat's bytes
  at the median (p90 1.00, max 1.24 on a fragmented file) in about as many
  reads (median ratio 1.0); before it, 0.60× the bytes in 4.7× the reads.
- **Output size.** Equal to within 0.1% at the median; the extremes are
  unequal-work files (ours larger, for example 224,709 against 178,810
  bytes on `97b724ad…`) and small files, where libavformat's header is
  larger (2,149 against 2,332 bytes on `f24f4b8e…`).

### Time to the first output byte

| | Median | 95% CI | p90 | Max |
|---|---:|---|---:|---:|
| Ours / `lav_avio` | 0.480 | [0.458, 0.497] | 0.797 | 4.99 |
| Ours, ms | 0.514 | [0.414, 0.878] | 6.14 | 76.1 |
| `lav_avio`, ms | 0.928 | [0.835, 1.434] | 9.48 | 49.7 |
| Ours: first byte as a share of the total time | 0.321 | [0.269, 0.423] | 0.695 | 0.953 |

The `moov`-first layout needs the table pre-pass before the first byte;
libavformat writes `ftyp` and `mdat` right after its (slower) open. The
pre-pass is cheaper than libavformat's open on most files.

### Where the advantage shrinks

- **First byte on fragmented and very long audio.** The pre-pass walks every
  fragment or every sample before the first byte: on three of the four
  fragmented held-out files the first byte comes 1.5× to 5.0× later than
  libavformat's, and 3.6× later on the 7.6-hour `6024f3b8…` (76 ms against
  21 ms). A streaming consumer waits that long; the total time is still
  lower (0.283× on `6024f3b8…`).
- **Long interleaved recordings.** They were the slow case before the read
  window and remain the highest of the timed classes: the longer-than-one-
  hour class has a median of 0.407× and a maximum of 0.672× (`b7d3b1ab…`);
  `0388073c…` is at 0.573×.
- **Sparse audio.** Where audio samples lie far apart, a window read takes
  in other tracks' bytes the extraction does not need: up to 5.2× the bytes
  of the second-round code on the development files of `exp-window-04`
  (128 MB instead of 25 MB on a 393 MB file), though in half the reads and
  no more time there. Where reads are cheap and bytes are not (a slow disk,
  a metered network), this costs more than it saves.
- **Large audio tracks.** The open-cost advantage fades as copying
  dominates: the median ratio is 0.36 on the 76 files with less than 1 MB of
  audio and 0.45 on the 38 with at least 10 MB.

## Memory (point 10, `perf-05`)

153 tracks (152 held-out files and the generated file), all `valid`
allocator counts, every one within the bound.

| File | Size | Audio | Samples | Growth | Output tables | Bound (tables + 64 KiB) | Growth / bound |
|---|---:|---:|---:|---:|---:|---:|---:|
| `6024f3b8…` (held out, longest) | 220 MB | 7.6 h | 1,178,682 | 4,821,647 | 4,821,984 | 4,887,520 | 0.987 |
| `3d8f6047…` (held out) | 987 MB | 2.2 h | 368,424 | 1,541,440 | 1,505,152 | 1,570,688 | 0.981 |
| `b2924496…` (held out) | 2.27 GB | 2.2 h | 364,299 | 1,524,924 | 1,488,312 | 1,553,848 | 0.981 |
| `b7d3b1ab…` (held out, largest) | 3.30 GB | 1.6 h | 274,920 | 2,169,416 | 2,124,268 | 2,189,804 | 0.991 |
| `f0efce40…` (held out) | 146 MB | 1.0 h | 155,047 | 686,652 | 634,384 | 699,920 | 0.981 |
| Generated fragmented | 2.20 GB | 38.5 min | 108,302 | 499,440 | 442,528 | 508,064 | 0.983 |
| All 153 | | | | | | | median 0.996, max 0.9993 |

- Growth follows the sample count, not the file size: the 2.2 GB fragmented
  file grows 0.50 MB, the 3.3 GB file 2.2 MB, the 220 MB 7.6-hour file
  4.8 MB.
- The margin is small by design: the copy buffer is allocated after the
  movie box is written and freed, so growth is the tables plus the copy
  buffer minus the `stsc` and chunk-offset bytes, plus a few hundred bytes
  of fixed structures (median 272 bytes under the bound, at least 44).
- First round, for comparison (`perf-01`): 0 of 153 within the bound,
  growth up to 2.09× the bound (9.13 MB on `6024f3b8…`), on power-of-two
  steps from `Vec` capacity doubling.

## Remote input (reported, not gated)

`remote.py` on the 152 held-out files with an extractable track, run
`remote-02` (GCP eu-west), with the first round's code. The read window
(third round) changes the range path: it requests 64 KiB windows instead of
one read per sample, which cuts the request count and raises the bytes; this
was not measured again, and open question 3 stays with the numbers below.

| | Files | Median | 95% CI | p90 | Max |
|---|---:|---:|---|---:|---:|
| Range path: bytes requested / object size | 152 | 0.66 | [0.61, 0.78] | 2.13 | 21.2 |
| Range path: requests | 152 | 357 | [210, 924] | 74,482 | 1,133,424 |
| Measured time, range / (download + local extraction) | 41 | 40.5 | [14.6, 63.5] | 102 | 113 |
| Modelled time, range / (download + local extraction) | 152 | 98.7 | [79.7, 176] | 5,408 | 79,846 |

- **Range is never faster.** None of the 41 measured files and none of the
  152 modelled ones; the closest measured file is 3.5× slower (smallest
  modelled ratio 1.6×). By size, the modelled median is 60× below 10 MB,
  427× at 10 to 100 MB, 4,279× at 100 MB to 1 GB and 1,943× at 1 GB or more.
- **Why.** `RangeReader` with the default `RangePolicy` (sized
  for probing: windows of at most 4 KiB) sends about one ranged GET per
  sample read during extraction, and each R2 request took 0.346 s at the
  median from the container. Files with long audio-only tracks need about
  one request per sample (1.13 million for the 7.6-hour `6024f3b8…`) and
  request up to 21× the object's bytes, because every read fetches at least
  a 4 KiB window; 28 files request more bytes than the object has.
- **Method.** `range_local` serves the range path's reads from the verified
  local copy: the exact requests and bytes for every file, without network
  time. The R2 range path (`range`) ran only on files of at most 300
  requests (41 files, 1,504 s in total, after the download of the same
  object); the modelled time is the local range time plus requests × the
  median measured request time, and it is 0.97× the measured time at the
  median on those 41. The download path (8 MiB ranged GETs, 16 in flight,
  then local extraction) ran on every file: median 7.0 MB/s, p90 54 MB/s
  (small files are latency-bound).
- **remote-01**, the first attempt (eu-south), served every read from R2 on
  every file with a 300 s limit. It finished 2 files in 7 minutes and was
  stopped: on `016ab71f…` (90 MB) the range path made 971 requests in 300 s
  and hit the limit, which kills the driver; the next write to its input
  then fails with `BrokenPipeError`. `remote.py` now records the driver's
  exit code and standard error.

This answers the measurement half of the open design question on remote
extraction (whether to route between range reads and a full download, and
the bar for it) for the first round's read pattern: with the probe-sized reader, extraction should
download. A range path for extraction
would need large coalesced reads of the audio sample runs, which is a design
question for that open question, not a bar here.

## Sources with several edits

`docs/design/edit-list-writers.md`, "M4A extraction": the output carries the
source's edit list entry by entry. Measured on the writer
against its pass criteria, fixed in that note before these runs.

**Which inputs this reaches.** Of the 544 files of `evidence/inputs.json`,
one has an AAC track whose edit list has several media edits:
a166d09989f9 (6,069 entries, 189,662 samples, 48 kHz). Four other `real`
files carry the `elst-multi-edit` tag, but on a video track, so extraction
reads them as before. There is no held-out input with such a track, so the
speed bar below is a development measurement.

**Correctness.**

| Bar | Result | Status |
|---|---|---|
| Fixtures, 100% | 44 of 44 AAC fixture tracks pass every oracle (41 directly, 3 through known divergences), `edit_multi_audio` and `edit_multi_av` included. The Rust round trip compares every entry of the output's edit list with the source's | **Met** |
| Corpus, 100% | a166d09989f9's audio track is extracted and passes every oracle, through `KD-X-EDIT-MULTI` and `KD-X-TKHD`. Its 189,662 stored samples and the frames its 6,069 media edits present equal the source's | **Met** |
| No other file changes | **533 of the 534 AAC tracks of all 544 corpus inputs are byte for byte what `main` writes**, and so are 42 of 42 fixture tracks. The one difference is a166d09989f9's audio, which `main` refuses. No stream `main` extracts is refused here, and the two files CoreAV's probe rejects are rejected by both. Run below | **Met** |
| Refusals explicit | Windows that go backwards, a media time below −1 and a media edit of duration 0 all fail with `edit list outside the supported model` and write nothing (a test); the same list in order is extracted | **Met** |
| Mutation audit | `writer_kd_audit.py --writer extract` on all 44 outputs: 472 caught, 176 no effect, 760 not applicable; nothing COVERED or ABSORBED. Every edit-list mutation of the two several-edit outputs is caught. The 176 without effect are the oracle blind spots of an earlier change | **Met** |

**Speed (development measurement).** One machine, `coreav extract-audio`
against `ffmpeg -i SRC -map 0:N -vn -c:a copy -f ipod OUT`, engines
interleaved, median of 7 or 9 runs after one warm-up each, `main` built
from the commit this branch starts at and timed in the same run:

**The whole corpus, byte for byte** (`bytes_check.py`, evidence
`evidence/bytes-several-edits.json`). Every one of the 544 inputs was
downloaded, SHA-256 verified, and every AAC track `coreav probe` reports
was extracted twice — with this branch and with `main` built from the
commit it starts at — on one machine:

| | |
|---|---|
| Files | 544 (63.1 GB), of which 157 held out; 0 harness errors |
| Files CoreAV's probe rejects | 2, both builds alike (012168a196be, 9120d3c689a1) |
| AAC tracks compared | 534, of which 154 on held-out files |
| Same exit status and same output bytes | **533** |
| Differing | **1**: a166d09989f9 stream 0, which `main` refuses with `edit list with several edits (multi-edit), not supported here yet` and this branch extracts |
| Streams `main` extracts that this branch refuses | **0** |

So the change reaches exactly the one track it is for, and leaves the other
533 byte for byte as they were.

Three of the four files below have a simple edit on their AAC track;
a166d09989f9 has the several-edit one.

| Input | ours ÷ ffmpeg | ours | `main` | ffmpeg |
|---|---|---|---|---|
| a166d09989f9, several edits | **0.0725** | 0.1150 s | — (refused) | 1.5863 s |
| 6e67814d336c, simple edit | 0.1171 | 0.0841 s | 0.0842 s | 0.7180 s |
| 42fce01d091e, simple edit | 0.1455 | 0.0084 s | 0.0080 s | 0.0576 s |
| 88ba79019cd6, simple edit | 0.0969 | 0.0117 s | 0.0115 s | 0.1203 s |

Share faster than FFmpeg: 4 of 4. Not worse than `main`: the three
inputs both accept are within 0.4 ms of it, and their outputs are byte
identical, so the difference is the machine's. The per-sample passes are
unchanged; the entries are written once, in the header.

The local runs downloaded 63.1 GB once for the whole-corpus run; R2 egress is free.

## Evidence

- `evidence/inputs.json`: the point 9 selection (sha256 and R2 keys only).
- `evidence/<run>.json` for every run above: every JSON file the run wrote,
  bundled (relative path to content): per-file records, status and attempt
  files with binary hashes, tool versions and machine data.
- `evidence/summary.json`: the final tables and per-file classification
  (`summarize.py check-05 perf-05 remote-02`).
- `evidence/only/`: the file lists of `perf-02` (11 held-out files) and of
  the `exp-window-*` runs (45 development files).

## Reproduce

    cargo build --release                      # xbench, xbench-mem
    python3 -I make_inputs.py                  # evidence/inputs.json
    # the cloud runs used a launcher and a poll script (not published)
    python3 -I summarize.py check-05 perf-05 remote-02

`perf` and `remote` runs were launched the same way
(`remote` takes `--discovery-folder`). The `exp-window-*` runs time variant
driver builds placed in `variants/` (not committed; their SHA-256 values are
in the runs' records) with `perf.py --development --only LIST --variants NAMES`. The local smoke runs used development
files only: `check.py`, `perf.py` and `remote.py` take `--inputs`, `--out`,
`--work`, `--bin` and `--deadline`, with R2 credentials in the environment
(never printed) and the pinned oracles in `--bin`.
