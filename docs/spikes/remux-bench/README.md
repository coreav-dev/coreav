# Full-file remux measurements: faststart and MOV-to-MP4 on corpus v1

Throwaway measurement code, outside the Cargo workspace. It
measures `remux_mp4` of `crates/coreav-isobmff-write` and the `coreav remux`
command against the bars of
[`docs/design/remux.md`](../../design/remux.md), fixed before these
runs in commit `15b93bc` (`89693d9` before the branch was rebased onto `main`; author dates are kept).

## Hypothesis and bars

The remux rewrites a whole MP4/MOV file as MP4 by stream copy, with the
movie box first, keeping every sample of every stream MP4 can carry, at
least as fast as libavformat with `+faststart`, with memory bounded by the
output tables. The bars, quoted from the design note:

- **Correctness (corpus v1).** "at least 99% of remuxed files agree with
  every check, directly or through a known divergence; at least 99.9% of
  packets agree: `framemd5` packet lines of the kept streams, a missing or
  extra packet counting as disagreeing; 100% of outputs have `ftyp`,
  `moov`, `mdat` in that order and pass the library round trip; no panic,
  crash or timeout (one hour per file)." Refusals ("nothing MP4 can
  carry") are counted apart.
- **Speed.** "Baseline: libavformat in-process (black-box boundary) remuxing the same streams to MP4 with `+faststart`, 64 KiB
  custom I/O on both sides. Bar: the median of the per-file ratios ours / baseline is at
  most 1.10, and the median of every class (unfragmented, fragmented,
  longer than one hour) at most 1.5."
- **Memory.** "Peak heap growth beyond the parser's own use (probe and the
  listings of the kept streams) at most the output sample tables plus the
  copied metadata plus 64 KiB plus 16 KiB per kept stream, on every
  held-out file."

## Results at a glance

| Bar | Result | Status |
|---|---|---|
| Files agree, at least 99% | **924 of 933** remuxed files (99.04%), 760 directly; `check-03` on commit `38f36df` | **Met** |
| Packets agree, at least 99.9% | **19,746,936 of 19,746,936** (100%) | **Met** |
| `ftyp`, `moov`, `mdat` and the library round trip, 100% | every output | **Met** |
| No panic, crash or timeout | none | **Met** |
| Speed: median ours / `lav_avio` at most 1.10 | **0.321** (95% CI 0.297 to 0.340) over 268 held-out files, `perf-01` | **Met** |
| Speed: every class median at most 1.5 | unfragmented 0.321, fragmented 0.351, longer than one hour 0.311 | **Met** |
| Memory within the bound on every held-out file | 269 of 270 in `perf-01`; the miss was found, fixed and re-measured within the bound (below) | **Missed once, fixed** |

### Correctness (`check-03`)

- 1,043 files: 933 remuxed, 109 refused (nothing MP4 can carry:
  73 no stream can be copied to MP4, 25 ProRes has no MP4 sample entry, 9 PCM audio is not copied to MP4, 1 a gap or an overlap between decode times, 1 an edit starts before the first sample),
  1 unavailable (its R2 object fails the SHA-256 check on every download).
- The 9 disagreeing files are not remux differences: `coreav-isobmff`
  rejects them (6 truncated, 2 without a movie header, 1 with two movie
  boxes) where `ffprobe` reads them; the bars count them as disagreement.
- Known divergences used: KD-R-BASE-TIME (108086), KD-R-FRAG-DURATION (35134), KD-R-TIMECODE-TAG (135), KD-R-FRAG-FRAMES (66), KD-R-FRAG-EDIT-END (36), KD-R-PROBE-WINDOW (4), KD-R-CHAN (4), KD-R-FRAG-STATED-EDIT (3), KD-R-LAST-DURATION (1), KD-R-VFR-RATE (1)
  (counts are covered differences, mostly packets).
- **The known divergences grew during the development runs.** When the bars
  were fixed (commit `15b93bc`) four existed: KD-R-FRAG-FRAMES,
  KD-R-FRAG-DURATION, KD-R-TIMECODE-TAG and KD-R-CHAN. The development run
  `check-01` (code before the corpus fixes) and `check-02` found oracle
  behaviours that the other six describe, each with a predicate from the
  files' boxes and the oracles' output, tightened after an independent
  review. Under the four known divergences of the bars alone, **894 of 933
  files (95.8%)** agree; every packet still agrees.
- Streams dropped, by reason: 205 codec_not_allowed, 89 unsupported: timecode tracks are not copied to MP4, 73 unsupported: PCM audio is not copied to MP4, 10 unsupported: timed text is not copied to MP4, 2 unsupported: more than one sample description.

### Speed (`perf-01`)

Per-file ratio of the medians of 5 rounds, held-out files; lower is faster.
Code `3c6b04e` (`112e663` before the rebase; later commits change no copy
path), on the development machine: 8 vCPU Intel Xeon at 2.90 GHz, sources in
the page cache, outputs on a tmpfs.

| Class | Files | Median | 95% CI | p90 | Max |
|---|---:|---:|---|---:|---:|
| all | 268 | 0.321 | 0.297 to 0.340 | 0.513 | 0.655 |
| unfragmented | 257 | 0.321 | 0.298 to 0.339 | 0.512 | 0.655 |
| fragmented | 11 | 0.351 | 0.266 to 0.432 | 0.531 | 0.568 |
| longer than one hour | 9 | 0.311 | 0.228 to 0.404 | 0.404 | 0.486 |
| Sensitivity: ours / `lav_native` | 268 | 0.313 | | | |
| Reference: ours / `ffmpeg` command line | 268 | 0.096 | | | |

Median times per file: ours 3.6 ms, `lav_avio` 9.8 ms,
`lav_native` 9.7 ms, `ffmpeg` 33.5 ms. libavformat's
faststart writes `mdat` first and then moves the data to put `moov` in
front; the remux writes once, front to back. 31 held-out files have nothing
the remux copies.

### Speed again on current code (`perf-02`)

Round 2 of this measurement: the same set as `perf-01`, which passed on its
first round and was never used for tuning. Same method and machine. Code
the crates of `1114aa6`. The commit recorded in the run's evidence, `dd60869`, is this branch before its rebase onto `main`; its crates are those of `1114aa6`, on `main`. The bar is the default speed bar against FFmpeg, fixed in
`docs/design/remux.md` before the run: at most 1.00 overall and per class.

| Class | Files | Median | 95% CI | p90 | Max |
|---|---:|---:|---|---:|---:|
| all | 268 | 0.328 | 0.305 to 0.350 | 0.523 | 0.639 |
| unfragmented | 257 | 0.327 | 0.304 to 0.346 | 0.523 | 0.639 |
| fragmented | 11 | 0.358 | 0.257 to 0.429 | 0.448 | 0.528 |
| longer than one hour | 9 | 0.314 | 0.230 to 0.407 | 0.407 | 0.487 |
| Sensitivity: ours / `lav_native` | 268 | 0.316 | | | |
| Reference: ours / `ffmpeg` command line | 268 | 0.095 | | | |

**The bar is met.** The remux is faster than libavformat in process in
every class, every upper bound below 1.00.
- 270 files were remuxed. Two of them, `6e67814d` and `ea690039`, have no
  `lav_avio` time because libavformat in process failed on them, so 268
  are compared, as in `perf-01`.
- The `met_1.00` and `faster` fields of `perf_summary.py` were added after
  the run, to report the bar of at most 1.00 fixed before it;
  they compute
  nothing else.
- The `npx` rounds 1 and 2 also timed the command line on most of this set
  (files of at most 128 MiB). Median times per file are 3.4 ms
for ours and 9.4 ms for `lav_avio`. The memory bound holds on all 270 files
that ran it. Evidence: `evidence/perf-02.tar.xz` (every file's record,
which `perf-01` did not commit) and `evidence/summary-perf-02.json`.

### Memory (`perf-01`)

269 of 270 held-out files within the bound. The miss (`a7ab6bba…`,
growth 174,851 bytes against 165,203) came from a 55 KB cover-art `udta`
read through `BufferedSource`, whose window then kept a second copy for the
whole remux. Commit `7cfb4e6` reads such boxes with a read the buffered
source hands straight to the caller; re-measured, 151,847 bytes against the
same bound. The bar is unchanged; the miss is recorded here.

### Spend

USD 0.

## Sanitization (`sanitize-01` to `sanitize-05`)

Bars: [`docs/design/sanitize.md`](../../design/sanitize.md), "Corpus
v1 bars", fixed before the first run. The corpus run's 1,043 files (1,042
available), sanitized with the default `Keep` through `rbench sanitize`
and checked by `check.py --sanitize` (`scripts/remux_check.py` in its
sanitize mode). These are not held-out results: the same files decided
what the default keeps.

| Bar | `sanitize-01` | `sanitize-02` | `sanitize-04` (final) |
|---|---|---|---|
| No metadata left, 100% of outputs, no known divergence allowed | 933 of 933 | 933 of 933 | **933 of 933** (**Met**) |
| Files agree, at least 99% | 907 of 933 (97.2%), **missed** | 924 of 933 (99.04%) | **924 of 933** (99.04%), 848 directly (**Met**) |
| Packets agree, at least 99.9% | 19,746,920 of 19,746,936 | 19,746,936 of 19,746,936 | **19,746,936 of 19,746,936** (**Met**) |
| No panic, crash or timeout | none | none | none (**Met**) |

- **Code.** `sanitize-01` ran the first design (`31448c3` before the
  rebase onto `main`, now `2ccda89`), `sanitize-02` the default that keeps
  gapless information and track kinds (`5c60807`, now `be9861e`).
  Independent reviews then found what the allowlist still let through:
  a printable `kind` carrying free text, removed items kept in memory
  without a bound, SEI with the encoder's name and settings in H.264 and
  HEVC configurations (also hidden in parameter set arrays), ICC `colr`
  profiles, spherical headers' sources and unknown children, text appended
  to fixed-layout boxes, and names repeating values; the checker missed
  most of these. `sanitize-03` ran an intermediate fix (its 25 HEVC files
  checked again after the harness was corrected). `sanitize-04` is the
  full corpus on the final code (the commit that adds this table), with
  the checker that checks the movie box tree against an allowlist and
  every kept box's exact form, in one pass. `sanitize-05` repeats it on
  that code merged with the later `main` (Matroska remux, picture size
  checks): every count is the same.
- **Why `sanitize-01` missed.** 17 of its 26 disagreeing files were
  sanitization differences, not remux ones: 16 M4A files lost their
  `iTunSMPB` item, so `ffmpeg` no longer trimmed the encoder delay (2,112
  priming samples, `start_pts` 2112 against 0) or the end padding (the
  first packet's skip-samples side data differs), and one file lost a
  track's `kind` box and with it `ffprobe`'s `forced` disposition. Both
  decide how a file plays and identify no one, so the default keeps them,
  written again from what they say. The thresholds are unchanged; the
  requirements changed with the default (the outputs may hold those items
  in their exact forms), as the design note records.
- The 9 files that still disagree are the remux's 9 parser rejections
  (`check-03`). Refusals are the remux's: 109.
- Known divergences used: nine of the remux's ten, as in `check-03`
  (KD-R-TIMECODE-TAG covers nothing here: sanitization leaves no tags).
  All ten existed when the sanitization bars were fixed. Under the four of
  the remux bars alone, 894 of 933 files (95.8%) agree.
- What was removed (`sanitize-04`, items by what the summary says they
  describe): location 56 items in 23 files, device 767 in 570 files, date
  74 in 45 files, other 2,036 in 771 files. Cleared fields: creation times
  1,075, compressor names 317, sample entry extras (mostly encoder SEI in
  H.264 and HEVC configurations) in 51 streams.
- Fixtures in CI: 33 of 33 sanitized fixtures pass every check. Fuzzing:
  `sanitize` 2 × 20 min on the first design, 2 × 10 min after each later
  change (seeded with the fixtures), no finding.
- Speed and memory are the remux's: the same copy path.

## Sanitization speed (`sanitize-perf-01`)

The bar is fixed in `docs/design/sanitize.md` ("Speed bar"). The script is
`sanitize_perf.py`, summarized by `sanitize_summary.py`. The run measured
the crates of `1114aa6`. The commit recorded in the run's evidence, `dd60869`, is this branch before its rebase onto `main`; its crates are those of `1114aa6`, on `main`.

- **Inputs:** the 301 held-out files of `evidence/inputs.json`.
- **Engines:** `coreav sanitize SRC OUT` against `ffmpeg -c copy
  -map_metadata -1 -map_chapters -1 -movflags +faststart` with the same
  streams, both command lines.
- **Rounds:** one warm-up each, then five rounds with the order alternated.
- **Outputs:** on a tmpfs.

| Class | Files | Ours / ffmpeg, median (95% CI) |
|---|---:|---|
| **All** | 270 | **0.181** (0.169 to 0.194) |
| Unfragmented | 259 | 0.181 (0.168 to 0.193) |
| Fragmented | 11 | 0.202 (0.099 to 0.349) |
| Longer than one hour | 8 | 0.166 (0.131 to 0.231) |

**The bar is met.** Sanitizing is faster than FFmpeg overall and in every
class, every upper bound below 1.00, and no file is slower (at most 0.68).

- **The class longer than one hour** is judged here on the video track's
  duration (8 files). The remux's classes, which the bar names, use the
  file's duration (`perf-02`'s `duration_s`). That gives 10 files, two of
  them without video, with a median of 0.142 (at most 0.344): the verdict
  is the same either way.
- **Round:** sanitize's first speed run on this set as `perf`-style
  measurements go. The `npx` rounds 1 and 2 (`docs/measurements/npx-coreav.md`)
  timed `coreav sanitize` against `ffmpeg` on held-out files of at most
  128 MiB, most of them in this set; round 2 passed at 0.165.
`coreav sanitize` refuses 31 files, as the remux does (codecs or sample
entries MP4 cannot carry).

A first attempt wrote its outputs to the machine's `/dev/shm`, a tmpfs of
64 MiB. Every output larger than that failed to write, so 9 of its first
28 files failed. It ran for 49 s, after the
library probe run had ended and before this run started.
It was stopped and run again from the start on an 8 GB tmpfs,
with no change to code. Evidence: `evidence/sanitize-perf-01/` and
`evidence/summary-sanitize-perf-01.json`.

## Method

### Inputs

- `evidence/inputs.json` (`make_inputs.py`): every `set: real` file of the
  corpus v1 manifest with extension `mp4`, `mov`, `m4v`, `3gp` or
  `m4a`: 1,043 files (174.9 GB), 301 held out (49.6 GB). Correctness uses
  every file; speed and memory the held-out files only.
- Media come from the private corpus bucket, every object verified by
  SHA-256, and are deleted after use. No media is committed.

### Programs

| Program | What it does |
|---|---|
| `rbench remux SRC OUT` | `remux_mp4` with the library defaults (`BufferedSource(FileSource)` in, 64 KiB `BufWriter` out), then the library round trip: `ftyp`, `moov`, `mdat` in order, and for every kept stream every source sample with its bytes, size, duration, sync flag, composition offset, decode time and presentation time (`src/bin/rbench.rs`) |
| `rbench time SRC OUT` | one timed remux |
| `rbench-mem SRC TMP` | the remux under the test-only counting allocator (`src/counting_allocator.rs`, copied unchanged from the clip benchmark, confined test-only `unsafe`), against the bound above |
| `baseline/remux.c` | libavformat 7.0.2 (the pinned LGPL package of Spike C) in-process: the same streams to MP4 with `movflags +faststart`; custom 64 KiB I/O on both sides (`lav_avio`, the headline) or its own file I/O (`lav_native`); written from the public API documentation only |
| `ffmpeg -i SRC -map 0:N... -c copy -movflags +faststart DST` | the command line, reference only (includes process start-up) |

- **Correctness** (`check.py`, on the development machine, 6 processes): per
  file, `rbench remux`, then `scripts/remux_check.py` (top-level layout;
  track header fields, edit lists and sample entry children from the
  boxes; ffprobe streams, tags and format tags; `ffmpeg -copyts -c copy -f
  framemd5` of every kept stream; MediaInfo) with the known divergences of
  `docs/known-divergences-remux.md`.
- **Speed and memory** (`perf.py`, one file at a time on the development
  machine): per held-out file, `rbench-mem`, then one warm-up run of every
  engine and 5 rounds with the engines' order rotated; the median of the 5
  is the file's time. Outputs go to a tmpfs and are deleted before each run
  (re-creating a large file on the machine's XFS volume cost up to tens of
  milliseconds, unevenly); sources are read from the page cache after the
  download.
- `summarize.py RUN` computes the bars of a correctness run.

### Spend

USD 0: every run is on the development machine; R2 egress is free.

## Reproduce

```sh
python3 -I make_inputs.py
cargo build --release                       # rbench, rbench-mem
python3 -I check.py --inputs evidence/inputs.json --out evidence/runs/check-02 \
  --work /tmp/rbwork --bin /opt/coreav-oracle-bin --rbench target/release/rbench --workers 6
python3 -I summarize.py evidence/runs/check-02 > evidence/summary-check-02.json
cc -O2 -I$LGPL/include -o remux baseline/remux.c -L$LGPL/lib -Wl,-rpath,$LGPL/lib -lavformat -lavcodec -lavutil
python3 -I perf.py --inputs evidence/inputs.json --out evidence/runs/perf-01 --work /tmp/pwork \
  --outputs /mnt/rbout --bin /opt/coreav-oracle-bin --rbench target/release --lav ./remux
```
