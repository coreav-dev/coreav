# Speed at parity with FFmpeg: Matroska, subtitles, chapters; Python binding overhead

Throwaway measurement code, outside the Cargo workspace. It measures CoreAV against libavformat under the default speed
bars against FFmpeg, and the Python module against the Rust library under its own
overhead bar. Every bar was fixed in commit `0a2f6c8` before the runs; the
second set of binding overhead bars in commit `ab9e832`, before their run.

## Hypothesis and bars

CoreAV probes Matroska and WebM files, remuxes them to MP4, extracts
`mov_text` subtitles as WebVTT and reads chapters at least as fast as
libavformat doing the same work in-process; the Python module adds little
to the Rust library. The bars, from the design notes:

- **Matroska probe** ([`docs/design/mkv.md`](../../design/mkv.md),
  "Speed bar"): `coreav_mkv::probe` against `avformat_open_input` with the
  format given; the median of the per-file ratios at most 1.00 overall and
  per class (WebM, Matroska), on the 69 held-out `real` Matroska and WebM
  files of corpus v1.
- **Matroska and WebM remux**
  ([`docs/design/mkv-remux.md`](../../design/mkv-remux.md), "Speed
  bar"): `remux_matroska` against libavformat remuxing the same streams to
  MP4 with `+faststart` and 64 KiB custom I/O (`lav_avio`); the same bar on
  the held-out files the remux accepts, refusals counted apart.
- **Subtitles and chapters**
  ([`docs/design/text.md`](../../design/text.md), "Speed bars"):
  `extract_subtitles` against libavformat and libavcodec demuxing,
  decoding (`mov_text`), encoding (`webvtt`) and writing the stream;
  `read_chapters` against `avformat_open_input`. The median ratio at most
  1.00 on the held-out inputs and on all `real` inputs; both must pass.
- **Python binding overhead**
  ([`docs/design/python.md`](../../design/python.md), "Binding
  overhead bar" and "Second bars"): `probe` adds at most 50 µs per call;
  `extract_audio`, `remux` and `clip` at most 1.05 times the Rust library
  doing the same work.

A "faster" claim needs the upper bound of the 95% confidence interval
below 1.00.

## Results at a glance

| Bar | Result | Status |
|---|---|---|
| Matroska probe, median ours / `lav_open` at most 1.00 | **0.227** (95% CI 0.218 to 0.233), 69 held-out files; by DocType WebM 0.230 (60), Matroska 0.227 (9) | **Met**, faster |
| Matroska and WebM remux, median ours / `lav_avio` at most 1.00 | **0.448** (95% CI 0.412 to 0.515), 43 held-out files; by DocType WebM 0.455 (41), Matroska 0.346 (2) | **Met**, faster |
| Subtitles, median ours / libavformat at most 1.00 | **0.030** on 4 held-out streams; **0.040** (95% CI 0.034 to 0.073) on all 12 `real` streams | **Met**, faster |
| Chapters, median ours / libavformat at most 1.00 | **0.006** on 4 held-out files; **0.005** (95% CI 0.003 to 0.016) on all 14 `real` files | **Met**, faster |
| Python `probe`, at most 50 µs added | +12.6 µs (fixtures), +12.7 µs (224 held-out files) | **Met** |
| Python writing functions, at most 1.05 on the fixtures (first bar) | `extract_audio` 1.242, `remux` 1.411, `clip` 1.500; after the fix 1.215, 1.347, 1.356 | **Missed** |
| Python writing functions to a path, at most 1.05 (second bars) | `extract_audio` **1.043**, `remux` **1.015**, `clip` 1.148 | **Met** for two, **missed** for `clip` |
| Python writing functions to memory without the copy, at most 1.05 (second bars) | `extract_audio` 1.087, `remux` 1.104, `clip` 1.191 | **Missed** |

The confidence intervals of the subtitle and chapter sets are wide: the
held-out sets are 3 and 4 files, which is why the bars also hold on every
`real` file.

## Method

- Machine: the development machine, 8 vCPU Intel Xeon at 2.90 GHz, otherwise
  idle during every run reported here; sources in the page cache, outputs
  on a 6 GB tmpfs.
- `perf.py`: one file at a time; per file one warm-up run of every engine,
  then 5 rounds with the engines' order rotated; a file's time per engine is
  the median of its 5 rounds; ratios are per file. CoreAV (`pbench`, the
  library calls of the bar) and the baselines time the operation inside
  their processes. The command lines (`ffmpeg`, `ffprobe`, the pinned
  7.0.2 static build) are references timed from outside, process start-up
  included.
- Baselines: libavformat and libavcodec 7.0.2 in-process, from the pinned
  LGPL package of Spike C,
  written from the public API documentation: `baseline/remux.c`, copied
  from the remux benchmark spike, and `baseline/lav_text.c` (probe, chapters and
  subtitles), both with 64 KiB custom I/O like CoreAV's buffered source.
- `summarize.py`: the median of the per-file ratios, overall and per
  class, with a bootstrap 95% confidence interval (10,000 resamples, seed
  89).
- `overhead.py` with the `overhead` binary: per input and side, the median
  of 30 calls after 5 warm-up calls, in-process on both sides; Python times
  with `time.perf_counter_ns` around the call, Rust with `Instant` around
  the same `coreav_cli` calls. For in-memory outputs it also times copying
  the output once into a new `bytes` (`copy` in the results).
- Results carry the corpus manifest's SHA-256, extension and set, no file
  names; the fixture runs carry the generated
  fixtures' names.

## Matroska probe (`probe-02`)

| Class | Files | Median | 95% CI | Max |
|---|---:|---:|---|---:|
| all | 69 | 0.227 | 0.218 to 0.233 | 0.450 |
| WebM (DocType) | 60 | 0.230 | 0.217 to 0.234 | 0.286 |
| Matroska (DocType) | 9 | 0.227 | 0.204 to 0.240 | 0.450 |
| Reference: ours / `lav_info` (plus `avformat_find_stream_info`) | 69 | 0.022 | | |
| Reference: ours / `ffprobe` command line | 69 | 0.008 | | |

Median times per file: ours 0.035 ms, `lav_open` 0.153 ms, `lav_info`
1.818 ms, `ffprobe` 4.416 ms. No file failed on either side.

The bars' classes are by DocType; one `.mkv` file (SHA-256 `584a16b9…`)
declares `webm`.
`summarize.py --classes` groups by extension, which gives WebM 0.230 (59)
and Matroska 0.224 (10); the DocType figures above were computed from the
same results with that one file moved.

## Matroska and WebM remux (`remux-03`)

| Class | Files | Median | 95% CI | Max |
|---|---:|---:|---|---:|
| all | 43 | 0.448 | 0.412 to 0.515 | 0.693 |
| WebM (DocType) | 41 | 0.455 | 0.412 to 0.538 | 0.693 |
| Matroska (DocType) | 2 | 0.346 | 0.244 to 0.448 | 0.448 |
| Sensitivity: ours / `lav_native` (libavformat's own file I/O) | 43 | 0.444 | | |
| Reference: ours / `ffmpeg` command line | 43 | 0.185 | | |

Median times per file: ours 10.9 ms, `lav_avio` 29.7 ms, `lav_native`
31.1 ms, `ffmpeg` 52.9 ms.

- Of the 69 held-out files, 20 were refused, counted apart as the bar says:
  19 whose video is VP8 (the remux refuses VP8 video by default: VP8 in MP4
  does not play in Safari, and dropping the video silently would turn the
  file into audio only), 1 with no stream MP4 can carry.
- On 6 more files, all Matroska with H.264, the baseline program stops
  with `Invalid argument` when it writes the first packets (stage `write`,
  after 3 to 5 packets): libavformat's MP4 muxer refuses them as the
  minimal program copies them, where the `ffmpeg` command line does more
  timestamp handling. So they have no gating ratio. The bar did not say
  how to count baseline failures; they are reported apart rather than
  dropped: against the `ffmpeg` command line, by the same method
  (`remux-03-baseline-failed.json`), ours is 0.252 at the median and 0.533
  at most. The Matroska class (by DocType) therefore rests on 2 of its 8
  remuxed files. By extension the classes are WebM 0.452 (40) and
  Matroska 0.448 (3).

## Subtitles (`subs-02`) and chapters (`chapters-02`)

| Set | Inputs | Median | 95% CI | Max | Reference: command line |
|---|---:|---:|---|---:|---:|
| Subtitles, held-out | 4 streams (3 files) | 0.030 | 0.022 to 0.041 | 0.041 | `ffmpeg` 0.019 |
| Subtitles, all `real` | 12 streams (10 files) | 0.040 | 0.034 to 0.073 | 0.103 | `ffmpeg` 0.021 |
| Chapters, held-out | 4 files | 0.006 | 0.002 to 0.038 | 0.038 | `ffprobe` 0.004 |
| Chapters, all `real` | 14 files | 0.005 | 0.003 to 0.016 | 0.039 | `ffprobe` 0.003 |

Median times: subtitles (all `real`) ours 1.30 ms, libavformat 21.3 ms;
chapters ours 0.046 ms, libavformat 12.1 ms. libavformat reads the whole
movie box, sample tables included, before it lists chapters, and demuxes
every interleaved packet to reach one subtitle stream; CoreAV reads only
the chapter boxes, and only the subtitle track's samples.

The text corpus has 34 `real` files with subtitles or chapters. The 10 with
`mov_text` streams are all measured. Of the 20 with chapters in the
manifest, the bar covers the 14 where CoreAV reads chapters; of the other
6, 5 are QuickTime files whose chapter reference names no track (neither
CoreAV nor `ffprobe` lists chapters) and 1 is a GoPro file whose HiLight
tag `ffprobe` lists as a chapter (KD-T-HMMT). The remaining files carry
other subtitle formats or text tracks only. Their outcomes are in
`docs/measurements/text-corpus-v1.md`.

## Python binding overhead

### First bar, on the fixtures (`overhead-02`, `overhead-03`)

| Function | Inputs | `overhead-02` | `overhead-03` (fix) | Bar |
|---|---:|---|---|---|
| `probe` | 64 | +12.6 µs | +13.9 µs | 50 µs: **met** |
| `extract_audio` | 26 | 1.242 | 1.215 | 1.05: **missed** |
| `remux` | 47 | 1.411 | 1.347 | 1.05: **missed** |
| `clip` | 39 | 1.500 | 1.356 | 1.05: **missed** |

- **Why it missed.** The output buffer grew from empty by doubling; in the
  interpreter's heap every step moved it and faulted its pages in again,
  up to 40% of a short call (`remux` of one fixture: 111 µs in Python
  against 60 µs in Rust; 70 µs with the buffer reserved, the same as
  reusing one buffer across calls). Valgrind counted only about 8% more
  instructions in Python, which pointed at memory rather than code.
  The fix (commit `ab9e832`) reserves the source's size plus an eighth and
  64 KiB, at most 64 MiB (`docs/design/python.md`, choice 4).
- **What remains.** The fixture calls take 25 to 80 µs; handing the result
  to Python (the `bytes` copy, the summary `dict`, the arguments) costs a
  median 6.4 µs for `extract_audio` and 14 µs for `remux` and `clip`, more
  than 5% of such calls. Building the `dict` with `json.loads` instead
  was slower (20 µs for a 2.3 KB probe document).
- The bar stays missed on these inputs.

### Second bars, on held-out real files (`overhead-held-out-*`)

224 held-out `real` MP4, MOV and M4A files of corpus v1 up to 64 MiB, each
function on the files both sides process.

| Function | Inputs | To a path | In memory | In memory without the copy | Bar |
|---|---:|---|---|---|---|
| `probe` | 221 | +12.5 µs | +12.7 µs | | 50 µs: **met** |
| `extract_audio` | 113 | **1.043** | 1.167 | 1.087 | 1.05: met to a path, **missed** in memory |
| `remux` | 202 | **1.015** | 1.390 | 1.104 | 1.05: met to a path, **missed** in memory |
| `clip` | 172 | 1.148 | 1.286 | 1.191 | 1.05: **missed** |

- **To a path**, the added time per call is 3 to 20 µs at the median of
  each quarter of the inputs by duration (the longest quarter of `remux`
  calls, 5 to 32 ms, shows 109 µs, under 1%); it falls under 5% once a
  call takes about 0.4 ms. `clip` cuts one second, so its calls are short
  (median about 120 µs) and the fixed cost shows.
- **In memory**, the copy into `bytes` is a large part: a remux is mostly a
  copy itself, so a second copy adds up to half again. The rest, beyond the
  copy measured on its own, comes from the interpreter's allocator state
  with earlier results still alive; in a loop that drops each result, a
  24 MB remux took the Rust time plus the copy, with no page faults.
- Callers that want the library's speed write to a path. The copy into
  `bytes` is kept: a zero-copy result needs a buffer object built with
  `unsafe` that is not a `bytes` (choice 4).

### Third bars (fixed before their run)

[`docs/design/python.md`](../../design/python.md), "What the misses
were" and "Third bars": all eight functions; both sides built with `dist`
(the wheel's profile); an in-memory output copied once on the Rust side as
returning `bytes` does; calls under 1 ms judged by the median added time
(at most 50 µs), longer calls by the median ratio (at most 1.05), a class
of fewer than 10 inputs reported only. Inputs: the 33 held-out `real` MP4,
MOV and M4A files of corpus v1 over 64 MiB and up to 256 MiB, never
measured by these bars (`select_third.py held-out`); for
`extract_subtitles`, the 30 corpus v1 files up to 256 MiB with a text track
(`select_third.py text`), not a held-out set. Development on 80 files that
are not held out (the 80 lowest SHA-256 of the `real` MP4, MOV and M4A
files up to 64 MiB).

Results (`overhead-third-*`; the verdicts in `*-summary.json`): every bar
met except `check` on calls of 1 ms or more (1.161 to a path, 1.128 in
memory), where the wheel's build of the frame scan takes about 10% more
instructions than the CLI's. The table is in the design note.
Disclosure: the summaries the run printed (the `.log` files) used 25 µs, a
draft value left in `overhead.py`, instead of the committed 50 µs, and
judged classes of any size; they were redone from the same records
(`overhead.py --summarize`), with no measurement repeated. That moved
`check`'s short calls from missed to met (+49.2 µs to a path, 0.8 µs
under the bar) and `clip`'s and `extract_subtitles`' long calls (4 and 2
inputs) from missed to reported only.

### `check` again (fixed before its run)

Bar and set in [`docs/design/python.md`](../../design/python.md),
"`check` again": the third bars' `check` bars, on the same 33 held-out
files, which those bars measured before, so this is a parity check rather
than a fresh held-out result. One run of `overhead.py --only check`. The
wheel and `overhead` were built `dist` from `main` with an earlier change.

Result (`overhead-check-wheel.json` and its summary): calls of 1 ms or
more **1.042** (19
inputs), shorter calls **+16.4 µs** (14). Both are met. The third bars had
1.161 (21 inputs) and +49.2 µs (12); an earlier change moved two calls under 1 ms.
Spend: 33 downloads (4.2 GiB), R2 read requests only (under 0.01 USD).

## Runs not reported

The first runs (`probe-01`, `subs-01`, `chapters-01`, `remux-01`,
`remux-02` and `overhead-01`) ran while builds and downloads were using the
machine, and `remux-01` filled the 64 MB `/dev/shm`. They were discarded,
and every operation was run again, one after the other, on the otherwise
idle machine; only those runs are in `evidence/`. All of the discarded
runs came before the second binding bars were fixed; each of the second
bars' two runs (`overhead-held-out-memory`, `overhead-held-out-path`)
ran once.

## Reproduce

    # inputs: corpus v1 held-out real files, downloaded by SHA-256 into
    # DIR with a selection.json of their manifest entries
    cargo build --release        # pbench and overhead
    cc -O2 -o work/lav_text baseline/lav_text.c -I$LAV/include -L$LAV/lib \
       -Wl,-rpath,$LAV/lib -lavformat -lavcodec -lavutil   # likewise remux.c
    python3 -I perf.py remux --media DIR --out evidence/remux.json \
       --pbench target/release/pbench --lav-text work/lav_text --remux work/remux \
       --bin /opt/coreav-oracle-bin --outputs /mnt/tmpfs/pb
    python3 summarize.py evidence/remux.json --baseline lav_avio --holdout-only --classes
    # binding overhead, with the coreav wheel installed (built `dist`)
    cargo build --profile dist --bin overhead
    python overhead.py --overhead target/dist/overhead --media DIR \
       --out evidence/overhead.json [--to /mnt/tmpfs/oh] \
       [--policy ../../docs/schema/policy-examples/web-upload.json]
    # third bars: inputs fetched from R2 one at a time
    python3 select_third.py held-out DIR
    python overhead.py --overhead target/dist/overhead --media DIR --fetch \
       --policy ../../docs/schema/policy-examples/web-upload.json \
       --out evidence/overhead-third-path.json --to /mnt/tmpfs/oh
    # check again: the same selection, check alone
    python overhead.py --overhead target/dist/overhead --media DIR --fetch \
       --only check --policy ../../docs/schema/policy-examples/web-upload.json \
       --out evidence/overhead-check-wheel.json

`$LAV` is the prefix of the pinned LGPL package; `perf.py` takes `probe`,
`remux`, `subs` and `chapters`.

## Spend

Under 0.01 USD: the machine is the development machine; corpus downloads from R2
have no egress fee, only read requests.
