# Stage 2 W1: safe probe benchmark (H4)

Throwaway spike code, separate from the hardened crates. It measures the hardened `coreav-isobmff` crate
against FFmpeg 7.0.2 used as a black box (isolated libavformat
benchmarks). The W1 criteria W1-1 to W1-6 are listed below. The
current method and headline rules (W1 parser speed on a public corpus with a
held-out split; the corpus used is corpus v1 of publicly available files, kept
private, not redistributed) were fixed and amended before the run.

Hypothesis **H4**: in-process safe probing with an allowlist beats a minimal
FFmpeg build with format and codec whitelists in a sandbox, on cost and safety.

## Headline (run-05: 241 held-out files of corpus v1, private, one cloud-host CPU, unpinned)

**Parser speed is not near parity, but the gap is much smaller than the
earlier 3.42x suggested and it closes on large files.** Equal work: the
format is given, probing is minimal, both sides ignore edit lists, and both
read the same memory-mapped file through 4 KiB of buffering.
libavformat (B-lean) takes a **median of 2.33x [2.26, 2.40]** of CoreAV's time
(A-index) per file. p10 is 1.36 and p90 2.75. CoreAV is slower on 9 of 241
files.

- Most of that gap is a fixed cost per open: about 276 µs for libavformat
  against 99 µs for CoreAV. Per sample, libavformat costs 63 ns and CoreAV
  42 ns (1.5x). On files with at least 10,000 samples the median is 1.4x to
  1.5x, and CoreAV is sometimes slower.
- CoreAV's probe still does less: it does not extract tags, chapters, codec
  configuration, profile and level, or bit rate, which libavformat parses at
  open. Part of the fixed cost is that work.
- The on-demand design is a real advantage: a probe that builds no index
  costs a median of 4.19x less than libavformat's open (B-mov), rising to
  14.5x on files with more than 100,000 samples.
- Operational cost, same shape and lean on both sides: one sandboxed process
  per request costs a median of **1.26x [1.20, 1.51]** less with CoreAV than
  with the minimal `ffprobe` told the format and not decoding (D-lean).
  Process start and library loading dominate this figure.

Every speed figure in this file comes with the metadata caveat above.

## Engines

| Engine | What runs | Shape |
|---|---|---|
| A | CoreAV probe, lazy (no per-sample work), memory-mapped file | in-process |
| A-index | A plus the full index (one entry per sample; PCM per 1,024 samples; per subsegment with a segment index), raw decode times | in-process |
| A-index-file | A-index through CoreAV's buffered file reader (4 KiB first fill) | in-process |
| B-lean | `avformat_open_input`, format `mov`, `probesize` 32, `analyzeduration` 0, `ignore_editlist` 1, `ignore_chapters` 1, custom I/O copying from the memory-mapped file into a 4 KiB buffer | in-process |
| B-lean-file | B-lean with custom I/O doing positioned reads into a 4 KiB buffer | in-process |
| B-mov | format given, default options and file I/O (32 KiB buffer); run-04's engine | in-process |
| B | format auto-detected, defaults (criterion W1-1) | in-process |
| B' | B plus `avformat_find_stream_info` (sanity check only) | in-process |
| A-sbx | A, one sandboxed process per request | process |
| D-lean | minimal `ffprobe` (LGPL, `mov` demuxer and allowlisted codecs only) in the same sandbox, `-f mov -probesize 32 -analyzeduration 0 -nofind_stream_info`, with format and codec whitelists | process |
| D | the same `ffprobe` with its defaults (reference, criterion W1-5) | process |

Sandbox (A-sbx, D, D-lean): user and network namespaces, resource limits,
no-new-privileges and a seccomp filter traced from the exact binary on the
benign files (`baseline/sandbox_launch.c`, `baseline/trace_syscalls.c`). No
CoreAV crate links FFmpeg, and no FFmpeg source was read.

## Method

- **Files**: the 303 held-out files of corpus v1 (publicly available files, kept private, not redistributed) in an ISO base media or QuickTime container, fetched from R2 and
  checked by SHA-256 (pinned to the manifest hash).
  CoreAV was never tuned on them, and the crate was frozen at `cafa814`
  before the split. The other 104 held-out files are WebM, Matroska, Ogg or
  MP3, outside an MP4/MOV parser's scope.
- **Headline set** (241 files): both parser engines open the file, CoreAV
  drops no stream, and CoreAV's index entry count equals the packet count of
  the black-box `ffprobe` with the same options, or the file has a segment
  index. Excluded and listed in `summary.json`: 56 files where CoreAV drops a
  stream (AC-3 or E-AC-3 audio, some subtitle, telemetry and QuickTime sound
  formats), 3 with a count difference, and 3 that CoreAV rejects but
  libavformat reads (two without a movie header, one truncated). Over all 300
  opened files the median is 2.36x [2.30, 2.45].
- **Timing**: every engine is its own long-lived process on its own pinned
  core, with the same fixed glibc allocator settings. Each process makes one
  warm-up pass, then 15 rounds follow. File order is shuffled every round, and
  engine order is shuffled for every file. Per-file ratio = median time of the
  FFmpeg-side engine over rounds divided by the CoreAV engine's. Headlines are
  medians over files with 95% bootstrap intervals (resampling files), never
  sums.
- **Sanity checks** (all passed, so the run is valid): an engine doing more
  work was never faster. B'/B 4.34x [3.93, 5.14], B/B-mov 1.41x, A-index/A
  1.41x, D/D-lean 1.22x [1.06, 1.46].
- **CPU**: the cloud host reports the model as "unknown" (gVisor) at 3,100 MHz, with
  no governor to read and no way to pin the frequency. All figures are from
  an unpinned CPU.

### Headline set: all 407 held-out files accounted for

Source: `corpus/holdout-v1.json` and `evidence/run-05/summary.json`
(`excluded_from_headline`; the raw run files are not published); ratios below are B-lean / A-index from the
existing run-05 warm timings, no new runs.

| Bucket | Files | Reason | Examples (sha256 prefix, category) | Median ratio |
|---|---|---|---|---|
| Headline | 241 | both engines open it, no stream dropped, index count equals ffprobe packet count | (see `summary.json`) | 2.33x |
| Out of scope: container | 104 | not ISO BMFF or QuickTime: WebM 59, MP3 21, Ogg 14, Matroska 10 | n/a, never fed to either engine | not timed |
| CoreAV dropped a stream | 56 | codec outside CoreAV's allowlist (AC-3/E-AC-3, some subtitle, telemetry and QuickTime sound tracks); A does less work, so the pair is unequal. spread over all seven groups | 0119957558a27611, 13cf128006da7d55, 145cd00359874eec | 2.79x |
| Index count differs | 3 | CoreAV entries differ from ffprobe packets: 1473 vs 4419, 802 vs 803, 155047 vs 155048 | 8da9593e9329d63f (2.36x), be60172deef2e6ae (3.08x), f0efce40f5b20794 (0.70x) | 2.36x |
| Rejected by CoreAV, read by libavformat | 3 | two fail CoreAV's movie-header checks, one is truncated | 00de70872ff1a518 (SVQ1, no ftyp), 2b42d40aa3287b9a (QuickTime), 665dddf09f221cc6 (truncated HEVC) | not timed |
| **Total** | **407** | 241 + 104 + 56 + 3 + 3 | | |

**Bias check.** The 59 opened-but-excluded files are larger (median 24 MB and
about 5,600 entries, against 6 MB and 1,100 in the headline) but are not
slow for CoreAV: their median ratio is 2.77x (p10 1.40, p90 3.94), higher
than the headline in every group except group-e. Over all 300 opened files
the median is 2.36x (p10 1.37, p90 2.97). So the exclusions do not flatter
the headline; if anything it is slightly conservative, and the dropped-stream
ratios are inflated by CoreAV skipping a stream. One exclusion does remove a
reverse case: f0efce40f5b20794 (audio, 155k entries, 0.70x), which matches
the known shrink on long audio files. The 3 rejected files are untimed
correctness gaps, not speed cases, and are listed as parser divergences.

## Results

### Parser speed (B-lean / A-index, held-out, per-file ratio)

| Category | Median [95%]; p10, p90; files |
|---|---|
| **All** | **2.33x [2.26, 2.40]; p10 1.36, p90 2.75; 241** |
| group-e | 2.57x [2.27, 2.67]; 2.12, 2.89; 19 |
| group-b | 2.54x [2.36, 2.72]; 2.28, 2.80; 18 |
| group-d (dataset videos) | 2.40x [2.29, 2.49]; 1.60, 2.72; 106 |
| group-f | 2.32x [2.17, 2.49]; 1.92, 2.72; 34 |
| group-g | 2.18x [1.64, 2.35]; 1.07, 2.75; 16 |
| group-c | 1.63x [1.46, 2.36]; 1.17, 2.76; 35 |
| group-a | 1.63x [1.30, 2.06]; 0.91, 2.30; 13 |

| Index entries per file | Parser speed | Probe only (B-mov / A) |
|---|---|---|
| under 1,000 | 2.55x [2.52, 2.61]; 114 files | 3.71x |
| 1,000 to 10,000 | 2.24x [2.15, 2.31]; 72 | 4.37x |
| 10,000 to 100,000 | 1.54x [1.24, 1.71]; p10 0.89; 43 | 8.97x |
| 100,000 or more | 1.40x [1.25, 1.63]; 12 | 14.5x |

Other cuts: AV1 1.39x [1.03, 1.96]; 30 minutes or longer 1.40x [0.93,
1.62]; 2 GB or larger 1.59x [1.18, 1.72]; H.264 2.40x; HEVC 2.08x;
fragmented with a segment index 2.32x. File-system pair (B-lean-file /
A-index-file): 2.97x [2.82, 3.09]. libavformat with its defaults (B-mov /
A-index-file): 3.74x [3.58, 3.91].

**Files where CoreAV is slower** (B-lean / A-index below 1): two audio-only
AAC files (0.68x with 77,088 samples; 0.81x), a 2.5 GB group-e recording
(0.78x), four AV1 files (0.87x to 0.98x), and two long group-c files (0.93x;
one with 343,516 samples). All are files where per-sample work dominates.

**Fixed cost and per-sample cost** (Theil–Sen fit of time against index
entries, headline files):

| Engine | Per open | Per entry |
|---|---|---|
| A-index | 99 µs [96, 103] | 41.8 ns [40.6, 43.7] |
| B-lean | 276 µs [263, 287] | 63.3 ns [57.7, 70.0] |
| A-index-file | 60 µs [58, 62] | 41.0 ns [39.7, 42.3] |
| B-lean-file | 251 µs [239, 259] | 65.7 ns [59.2, 72.5] |

**Bytes read** per file (median): CoreAV 12.8 KB from the mapping and
16.6 KB through its file reader; B-lean 16.6 KB; B-mov 45.5 KB with its
default 32 KiB buffer.

### Operational cost (D-lean / A-sbx, both one sandboxed process per request)

| Category | Median [95%]; p10, p90; files |
|---|---|
| **All** | **1.26x [1.20, 1.51]; p10 1.07, p90 1.76; 300** |
| group-e | 1.56x [1.20, 1.69]; 1.10, 1.81; 23 |
| group-d | 1.55x [1.50, 1.57]; 1.09, 1.72; 113 |
| group-c | 1.53x [1.28, 1.58]; 1.06, 2.97; 57 |
| group-a | 1.15x [1.11, 1.21]; 1.05, 1.51; 23 |
| group-b | 1.13x [1.10, 1.24]; 1.07, 1.66; 27 |
| group-g | 1.13x [1.10, 1.19]; 1.08, 1.54; 21 |
| group-f | 1.12x [1.09, 1.23]; 1.04, 1.76; 36 |

p99 latency 27.9 ms (A-sbx) against 91.6 ms (D-lean); peak memory per request
4.1 MiB against 16.5 MiB (median). The default `ffprobe` (D), which runs
stream-information decoding, is 1.80x [1.72, 1.85]. CoreAV is slower on 2
of 300 files (0.98x, 1.00x).

### Cold page cache

Not measured on a cloud host. Eviction with `posix_fadvise` had no effect in
the cloud host's sandbox: 0 of 5,130 cold requests found the file evicted, so the
run-05 "cold" phase is a warm measurement and is not reported as cold.
Locally, where eviction was verified (no page left resident) on the tuning
files: B-lean-file / A-index-file 1.75x (p10 1.12, p90 2.53, 55 files), and
1.10x on the self-made 2.2 GB fragmented file (69 against 76 ms).

### Where the libavformat time goes (local, black box, tuning and self-made files only)

`gap_breakdown.py`, `evidence/local/gap-breakdown.json`. Medians of per-file
ratios against A-index on 55 tuning files. Local machine, unpinned.

| Change from B-mov | Median ratio | What it shows |
|---|---|---|
| B-mov (format given, defaults) | 2.85x | starting point |
| + `probesize` 32, `analyzeduration` 0, `ignore_chapters` | 2.87x | probing options cost nothing measurable once the format is given |
| + `ignore_editlist` | 2.70x | edit-list work; on the one-hour file 5.8 to 3.9 ms |
| + positioned reads into 4 KiB | 2.38x | read-ahead: 64 KB to 21 KB per file; on the 2.2 GB file 77.5 to 9.7 MB and 24.1 to 10.7 ms |
| B-lean (memory-mapped instead) | 2.54x | mapping costs both sides; CoreAV's file path is also faster than its mapped path |

- **Process start and library loading**: 15.7 ms per process for libavformat
  (load and version check) against 1.4 ms for CoreAV's process. It is
  excluded from every in-process figure. It dominates the sandboxed
  process comparison.
- **Open against stream information**: counted at the libavcodec boundary,
  `avformat_open_input` decodes nothing. `avformat_find_stream_info` opens a
  median of 2 decoders and decodes about 1 frame per stream (up to 16 parser
  calls). B' costs a median of 4.34x B on the held-out files.
- **Allocator** (the B' anomaly of run-04): with glibc's default settings,
  every libavformat close trimmed the heap, and the next open took about
  2,000 minor page faults on the one-hour file. Under B', the extra
  allocations kept the heap from trimming (about 270 faults), so B' ran
  faster than B. With fixed allocator settings both take about 100 faults,
  and B' is slower, as it must be. CoreAV is unaffected (30 to 60 faults).
- **Per-call cost**: the fixed cost per open (above) is the largest
  remaining part of the gap on small files.

**Conclusion.** Harness effects inflated run-04: summed times, a 32 KiB
read-ahead, edit-list work, allocator trimming, and fixed open cost on tiny
self-made files. With those removed, the remaining gap is real work done by
libavformat: about 180 µs more per open, partly metadata that CoreAV skips,
and about 1.5x more per sample. It is not mostly harness overhead, but it is
mostly fixed cost. On large files it narrows to 1.4x to 1.5x.

### When the advantage shrinks

- **Many samples per file**: 2.55x under 1,000 samples, 1.4x to 1.5x above
  10,000. CoreAV is slower on 9 held-out files: two audio-only AAC files, four
  AV1 files, a 2.5 GB group-e recording and two long group-c files.
- **Audio-only and group-c files**: 1.63x medians, with p10 0.91 for group-a.
- **Metadata**: CoreAV extracts less (tags, chapters, codec configuration,
  profile and level, bit rate). Parity on metadata would add work on
  CoreAV's side.
- **Sandboxed processes**: process start and library loading dominate, so
  the gap is 1.26x with both sides lean (1.12x to 1.15x for group-f, group-g,
  group-b and group-a files), against about 2.3x in-process.
- **Fair I/O**: with libavformat's default 32 KiB read-ahead the gap looks
  like 3.74x. With matched 4 KiB reads it is 2.97x on the file system and
  2.33x from memory.
- **Probe only**: the 4x to 14x advantage holds only when the caller does not
  need the full index.

### Safety (hostile suite, 111 files)

Engines A, A', A-index and A-sbx: zero crashes, timeouts or out-of-memory;
every structural and PixelSmash-class file rejected or, for MagicYUV in MOV,
dropped and reported (strict mode rejects it); worst request 0.033 s and
4.9 MiB; zero criterion failures. Engine D rejected 9 files and read 102.

### W1 criteria

| Criterion | Run-05 (held-out) | Note |
|---|---|---|
| W1-1 at least as fast as libavformat in-process | pass: A/B summed 10.3x [10.0, 10.7] | As written, a lazy probe against an open that builds the index, summed over files. Not a parser-speed claim; under equal work see the headline. |
| W1-2 zero crashes on the hostile suite | pass | |
| W1-3 bounded (< 1 s, < 512 MiB) | pass: 0.033 s, 4.9 MiB | |
| W1-4 PixelSmash-class rejected at probe time | pass | |
| W1-5 beats the minimal sandboxed build on cost | pass: A/D summed 218x; p99 2.6 against 376 ms; memory 4.4 against 20.3 MiB | As written, in-process against one process per request. Same shape and lean: 1.26x [1.20, 1.51]. |
| W1-6 beats it on safety | pass | |

H4 holds as written. The honest reading: CoreAV is safe on hostile input with
no C code on the probe path. It is cheaper as a lazy, on-demand probe. Under
equal work it is about 2.3x faster per file in-process (mostly fixed cost),
1.4x to 1.5x on large files, and 1.26x as a sandboxed process.

### Self-made, biased

Generated fixtures, the one-hour MP4 and the 2.2 GB fragmented MOV (37
files). They are not in any headline. CoreAV's 4 KiB first read fill was chosen
on the 2.2 GB file.

| Pair | Median [95%]; p10, p90 |
|---|---|
| parser speed (memory) | 2.37x [2.22, 2.52]; 1.75, 2.69 |
| parser speed (file system) | 3.44x [3.09, 3.77]; 2.37, 4.03 |
| B-mov reference | 4.56x [4.03, 4.81]; 3.32, 5.28 |
| operational cost (lean) | 1.19x [1.10, 1.46]; 1.04, 1.70 |

2.2 GB fragmented MOV: 1.60x (memory) and 2.02x (file system). One-hour MP4:
1.60x (file system).

### Superseded runs

Run-03 and run-04 reported summed-time
ratios on self-made fixtures and two corpus files (run-04: 3.42x). They are
kept as history and superseded by run-05, among other reasons because summed
times let two synthetic files dominate, the two engines did unequal work, and
the tuning files were also the evaluation files.

## Evidence

The raw files of runs 01 to 05 (cloud-host runs) are not published; the tables
in this note are their summaries, with every distribution, cut and excluded file.
Run-05 ran on one cloud CPU (a gVisor sandbox, unpinned) for 24 minutes and read 304
objects (49.9 GB) from the corpus bucket, with no egress charge. It kept raw
per-request records (warm and cold), the index check, memory, the hostile suite,
the sandbox check and the machine record. `evidence/run-05/` holds its `summary.md` and `summary.json` (every distribution, cut and excluded file); `evidence/local/gap-breakdown.json` is the local breakdown. One launch was refused by the cloud
host before any container started (disk request), and a disk check followed.

## Reproduce

From the repository root, with `S` naming the spike's directory:

```sh
"$S/check.sh"                                                # spike gates
cargo build --release --manifest-path "$S/Cargo.toml"
```
