# Remote media against ffprobe and ffmpeg on the same URLs

Measurement code, outside the Cargo workspace.
It times `coreav` reading presigned R2 URLs against `ffprobe` and `ffmpeg`
reading the same URLs, under the bars of
[`docs/design/remote.md`](../../design/remote.md), "Speed bars", fixed
in commit `81bcf59` before any run.

## Hypothesis and bars

Reading by byte ranges with the shared read plan (`coreav-range`), the
command probes, remuxes, extracts audio from and clips remote objects at
least as fast as FFmpeg's own HTTP reader. The bars, from the design note:

- `coreav probe URL` against `ffprobe -show_format -show_streams URL`;
  `coreav remux` against `ffmpeg -c copy -movflags +faststart` with the same
  streams; `coreav extract-audio` against `ffmpeg -map <the stream> -c copy`;
  `coreav clip --start S --end S+10` against `ffmpeg -ss S -i URL -t 10 -c
  copy` with the same streams, S a quarter of the duration.
- Inputs: the 30 MP4/MOV/M4A and 30 Matroska/WebM held-out `real` corpus v1
  objects with the lowest SHA-256 among those of at most 256 MiB that the
  operation accepts (clip and extract-audio: MP4/MOV only). Classes: ISO
  base media and Matroska.
- Per input: one warm-up of each side, then three rounds in alternating
  order; a side's time is its median round; ratio ours / theirs.
- **Bar: the median ratio is at most 1.00 overall and in each class, for
  each operation.**

## Method

`bench.py OPERATION` (probe, remux, extract-audio, clip) presigns each
object for an hour, runs both command lines from the development machine (8
vCPU Intel Xeon at 2.90 GHz) with outputs on a tmpfs, and records the
medians with the manifest's SHA-256, extension and size (no names or URLs). An input our command refuses (`refused`) or the baseline
cannot handle (`baseline_failed`) is recorded and the next one taken.
`summarize.py RESULTS.json` gives the medians per class with a bootstrap
95% confidence interval (seed 115).

R2's time to the first byte of a range request was about 260 ms from the
workspace (TLS and connection about 11 ms); one stream downloaded a 35 MB
object in 0.78 s, four parallel ranges in 0.60 s. Every request the plan
makes in sequence therefore costs about a quarter second, which is what
the tuning below is about.

## Development, before the held-out run

Tuned on the files that are not held out (`bench.py --development`, 8 to 10
per class), with `COREAV_REMOTE_SETTINGS` to vary the plan's numbers:

| Change | Development result |
|---|---|
| Plan as ported (4 MiB first span) | probe: ISO 0.58, Matroska 1.59 |
| A jump past an unread response keeps it open | (fixed the Matroska probe's third request) |
| A 1 MiB tail requested with the first request | probe: ISO 0.44, Matroska 0.91 |
| A spool of every received byte | remux: Matroska 1.74 → 1.10 (a 35 MB file fetched once instead of 2.6 times) |
| A 32 MiB first span | remux: ISO 0.79, Matroska 0.90; probe: ISO 0.53, Matroska 0.94 |
| A 64 MiB first span, 16 MiB skip | probe: Matroska 1.00 (rejected) |

Disclosure: a first smoke run, before the tuning, ran `probe` on 4 of the
held-out inputs (the two lowest SHA-256 of each class); its numbers led to
the first two changes above. The bars and the selection rule are as
committed in `81bcf59`.

## Results

Held-out runs on code `53cd009` (`probe-01`, `remux-01`, `extract-audio-02`,
`clip-01` in `evidence/`). Ratio ours / FFmpeg's, median per input; lower
is faster.

| Operation | Class | Inputs | Median | 95% CI | Max | Median times (ours / FFmpeg) | Bar 1.00 |
|---|---|---:|---:|---|---:|---|---|
| probe | all | 60 | **0.833** | 0.762 to 0.875 | 1.697 | 312 / 372 ms | met, faster |
| probe | ISO | 30 | 0.542 | 0.321 to 0.741 | 1.090 | 303 / 673 ms | met, faster |
| probe | Matroska | 30 | 0.946 | 0.862 to 1.092 | 1.697 | 347 / 357 ms | met (not shown faster) |
| remux | all | 60 | **0.800** | 0.763 to 0.856 | 1.700 | 366 / 752 ms | met, faster |
| remux | ISO | 30 | 0.711 | 0.503 to 0.780 | 1.583 | 338 / 1,067 ms | met, faster |
| remux | Matroska | 30 | 0.909 | 0.805 to 0.987 | 1.700 | 404 / 488 ms | met, faster |
| extract-audio | ISO | 30 | **0.681** | 0.335 to 0.776 | 1.376 | 340 / 1,059 ms | met, faster |
| clip | ISO | 30 | **0.454** | 0.287 to 0.660 | 1.167 | 412 / 1,316 ms | met, faster |

- Every bar is met. The Matroska probe's interval reaches above 1.00, so
  it is at parity, not shown faster: both sides need one round trip for a
  small file, and ffprobe's start-up is the difference.
- Inputs the operation refused, counted apart and replaced by the next
  SHA-256 as the selection rule says:
  - probe: 1 (a movie box without a header);
  - remux: 18 (16 VP8 videos, refused by default; 1 movie box without a
    header; 1 PCM audio only);
  - extract-audio: 26 (18 without audio, 5 whose audio is not AAC, 2 movie
    boxes without a header, 1 encrypted track);
  - clip: 39 (25 shorter than 12 s, 8 without video, 2 that do not probe,
    3 with a codec clip does not take, 1 other).
- Two deviations from the method as written, both disclosed here:
  - **The first extract-audio run is invalid** (`extract-audio-01-invalid`):
    the harness read the stream to map from `streams`, which the
    `coreav.extract-audio/1` summary does not have (it has `stream`), so
    `ffmpeg` copied every stream, more work than ours, and failed on 5
    files whose video cannot go into M4A. Fixed (`f83b4b0`) and run again on
    the same selection: `extract-audio-02`, the result above.
  - **Clip skips files shorter than 12 s.** The bar cuts 10 s from a quarter
    of the duration, which needs at least about 13 s; the harness checks
    the duration first and treats a shorter file as one the operation
    does not accept.
- The maxima (1.1 to 1.7) are single files, not examined one by one here;
  at about 260 ms a round trip, one extra request decides a small file's
  ratio.

## Clip re-run after the cut point check

`clip` now reads the coded data of the frame it starts at, and of every
frame of a run that the container marks sync throughout
(`docs/design/cut-points.md`). The clip bar was run again with the same
selection rule, method and bar.

| Run | Code | Inputs | Median | 95% CI | Max | Median times (ours / FFmpeg) | Bar 1.00 |
|---|---|---:|---:|---|---:|---|---|
| `clip-02` | first version of the check | 30 | **0.419** | 0.306 to 0.817 | 1.642 | 478 / 1,131 ms | met, faster |
| `clip-03` | final version (`8062380`) | 30 | 0.402 | 0.310 to 0.686 | 1.763 | 417 / 1,088 ms | met, faster (not held out) |
| `clip-04` | start candidates bounded in bytes | 30 | 0.447 | 0.334 to 0.723 | 1.752 | 419 / 1,055 ms | met, faster (not held out) |

- **`clip-02` is the held-out result.** Its per-input request counts on
  four inputs, slower than in `clip-01` by 0.4 to 0.7, led to the one
  change before `clip-03`: past the start, a run that is not marked sync
  throughout believes its table for where the listing stops, so no frame
  away from the start is read before the copy. The selection is
  therefore no longer held out for `clip`, and `clip-03` is a re-measure
  on it, not a held-out claim. A later held-out claim for `clip` needs a
  new selection.
- The selection differs from `clip-01` by one input: a file that did not
  probe then (a compressed movie box, read since an earlier change) now takes the place
  of the next SHA-256. 38 inputs were refused (25 shorter than 12 s, 8
  without video, 4 with a codec clip does not take, 1 with audio other
  than AAC).
- The maximum, `1b450d90085d`, is an all-intra H.264 file without a sync
  sample table (all 367 frames are IDR pictures). Its run is marked sync
  throughout, so every frame of it is checked before the copy and the run
  is received twice: 1.14 in `clip-01`, 1.76 in `clip-03`.

## The other commands

Bars and method are fixed in `docs/design/remote.md` ("The other commands")
before the runs. Evidence: `evidence/<operation>-01.json`, with summaries
in `evidence/summary-<operation>-01.json`, and the run's log
`evidence/url-223-01.log`.
- **Build:** the `dist` profile of `main` at `ecee36b`.
- **Machine and inputs:** the same machine and URLs as above.
- **Round:** round 1 for each operation over URLs.

| Operation | Inputs | Ours / baseline, median (95% CI) | Classes | Bar |
|---|---:|---|---|---|
| `check` (against `ffprobe -count_packets`) | 60 | **0.822** (0.778 to 0.887) | ISO 0.548 (0.413 to 0.783); Matroska 0.907 (0.853 to 1.015) | met |
| `split --every 10` (against the segment muxer) | 30 | **0.782** (0.380 to 0.891) | ISO only | met |
| `sanitize` (against the copy with metadata stripped) | 30 | **0.813** (0.532 to 0.896) | ISO only | met |
| `extract-subtitles` (against `-f webvtt`) | 1 | 0.072 | ISO only | met |
| `chapters` (against `ffprobe -show_chapters`) | 3 | 0.582 (0.434 to 0.618) | ISO only | met |

Every bar is met.
- **Faster:** `check` overall and in ISO base media, `split` and `sanitize`
  are faster than FFmpeg reading the same URLs, their upper bounds below
  1.00.
- **On par:** `check` on Matroska, whose interval reaches 1.015.
- **Too few inputs for a claim:** subtitles and chapters.
- **Slower inputs:** a few single inputs are slower than FFmpeg, up to 1.69
  (`check`), 3.33 (`split`) and 1.54 (`sanitize`), the medians far below.
- **Refused or not run:**
  - `split` refused 10 inputs: disallowed codecs (SVQ1, SVQ3, MPEG-4 Part
    2), audio that is not AAC, and audio-only files.
  - `sanitize` refused 2: a compressed movie box and PCM audio.
  - Of the 4 held-out files tagged `subtitle-track`, 3 are the files tagged
    `chapters`: their text track is a chapter list, not subtitles, and
    `extract-subtitles` answers "no such track" (FFmpeg was not run on
    them, since the harness skips the baseline once ours refuses). The
    subtitle row rests on the fourth.
- **What `check`'s evidence does not record:** this run kept no exit code
  per input, so a `reject` verdict reached early (a codec outside the
  allowlist, before any sample is scanned) cannot be told apart from a
  full check. Leaving out the 8 ISO inputs with codecs outside the
  allowlist leaves the ISO median at about 0.558. `bench.py` now records
  each side's exit code, and prints the commit and the binary's SHA-256
  at the start of a run.

## Spend

R2 GET requests only (no egress fees): well under 1 USD, the two clip
re-runs included.
