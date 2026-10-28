# Corpus v1 run of the picture size verification

Throwaway measurement code, outside the Cargo workspace. It runs
the probe's picture size readers and sample scan
(`docs/design/frame-sizes.md`) and the scanning `coreav check` on every
corpus v1 file with an ISO base media extension, and times the scanning
check against `ffprobe -count_packets` on the held-out files.

Python only: `run.py` reuses the policy-check spike's runner
(R2 downloads, memory measurement, `boto3`)
and `scripts/policy_check.py`. No Rust code,
so there is no Cargo graph to license-check.

## Hypothesis and bars

The sizes the probe derives from parameter sets and frame headers bound
the pictures a decoder produces, and equal them for ordinary files; the
scan stays within the probe's limits. The bars, quoted from the design
note and committed before this run:

| Bar | Set | Threshold |
|---|---|---|
| S1 crashes | every file | 100%: no crash or timeout (10 s, 1 GiB), the scan returns a size or an error |
| S2 bound | every video stream that `ffprobe` decodes | 100%: `scanned_size` is not smaller than `ffprobe`'s `width` × `height` in either dimension, except where an entry of `docs/known-divergences-sizes.md` covers it |
| S3 agreement | H.264, HEVC, VP9 and ProRes streams with a `scanned_size` | at least 99% equal `ffprobe`'s `width` and `height` exactly (AV1 is reported apart) |
| S4 configuration | streams with a `parameter_set_size` | at least 99% equal their `scanned_size` |
| S5 policy | the policy check's corpus bars (`docs/design/policy-check.md`, B1 to B5), rerun on the files whose verdict changes | as committed there |
| S6 speed | held-out ISO base media files | median ratio of `coreav check` with the default size source (scanning) to `ffprobe -v error -count_packets -show_streams` at most 1.0 |
| S7 memory | every file | peak resident memory of the scanning check at most 300 MiB |

## How it runs

    venv/bin/python -I run.py speed --work WORK --coreav BIN --ffprobe FFPROBE
    venv/bin/python -I run.py sizes --work WORK --coreav BIN --ffprobe FFPROBE --jobs 8

R2 credentials come from `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and
`R2_ENDPOINT`. Downloads are capped as in `docs/spikes/policy-check/`.

## Results

One pass over every corpus v1 file with an ISO base media extension (2,356
files, 248 GB read from R2), with the code of this pull request at
`d067b6a`. Fixes followed from the run and the review: zero NAL lengths and
short tails in zero-filled samples; no bound from a record whose parameter
set cannot be read or whose version is not 1; crops subtracted only when
they are padding and valid as a whole. Every file these can change was
checked again with the final binary: the 1,157 files with an H.264 or
HEVC stream, and every file with a stream lacking a scanned or parameter
set size. The evidence holds both passes (`sizes.jsonl.gz`,
`sizes-rerun.jsonl.gz`); no corpus file has a left, top, large or invalid
crop, so the results did not move.

| Bar | Result | Status |
|---|---|---|
| S1 crashes | 0 failures in 2,356 files; the scanning probe takes 27 ms at the median, 1.68 s at most | **Met** |
| S2 bound | 0 of 1,255 streams have a scanned size smaller than `ffprobe`'s; no known divergence was needed | **Met** |
| S3 agreement | 1,172 of 1,181 H.264, HEVC, VP9 and ProRes streams (99.24%) equal `ffprobe`'s size exactly; AV1 74 of 74 | **Met** (bar 99%) |
| S4 configuration | 1,199 of 1,199 streams with a parameter set size have the same scanned size | **Met** (bar 99%) |
| S5 policy | the 52 files whose verified size is missing or differs from the declared one, × 3 policies: 0 failures in 156 cases; 36 of 36 oracle cases agree | **Met** |
| S6 speed | median ratio **0.135** (95% CI 0.120 to 0.148) over 303 held-out files: the scanning `coreav check` 1.52 ms, `ffprobe -count_packets -show_streams` 17.39 ms at the median; at most 0.70 | **Met** (bar 1.0) |
| S7 memory | largest peak 43,528 KiB (44.6 MB), median 3,144 KiB | **Met** (bar 300 MiB) |

S2 compares with `ffprobe`'s stream size, which comes from the first
parameter sets, so it cannot see a size that changes later in a stream;
the fixtures `size_inband_sps.mp4` and `size_vp9_resize.mp4` cover that,
checked against every decoded frame.

The scan reads the headers of NAL units, OBUs and frames, not their coded
data, so its cost follows the number of NAL units: a 1.6 GB 5.5K HEVC file
of 2,428 samples scans in 10 ms.

The 9 S3 differences are all larger on CoreAV's side, which is the
safe direction for a bound:
- 8 H.264 streams whose SPS codes a picture larger than the sample entry by
  less than one macroblock, with no cropping in the SPS (1920×1088 against
  1080, 864 against 854). `ffmpeg`, a black box here, outputs the sample
  entry's size for these; the probe reports what the SPS codes.
- 1 layered HEVC conformance stream (L-HEVC) whose enhancement layer's SPS
  is 1920×1080; `ffmpeg` decodes its 960×540 base layer only.

31 streams have no scanned size (av1 3, h264 20, hevc 7, vp9 1), all in
the `edge` set: truncated or corrupt samples (NAL lengths past the end),
CMAF initialization segments without samples, and tables the probe rejects.
The policy rejects such streams under the default size source.

24 streams (12 in the `real` set) declare a size different from the one
their parameter sets give; `coreav probe` warns for each (`declared_differs_from_scanned` in
the summary).

## S6 again, per class (`speed-02`)

Round 2 of S6: the same 303 held-out files and the same method on current
code: the crates of `1114aa6`, a `release` build. The commit recorded in the run's evidence, `dd60869`, is this branch before its rebase onto `main`; its crates are those of `1114aa6`, on `main`. The first round passed
and the set was never used for tuning. The bar, fixed in
`docs/design/frame-sizes.md` before the run, is the default speed bar
against FFmpeg: at most 1.00 overall and in each class,
unfragmented and fragmented.

| Class | Files | `coreav check` / `ffprobe -count_packets`, median (95% CI) |
|---|---:|---|
| **All** | 303 | **0.150** (0.132 to 0.160) |
| Unfragmented | 283 | 0.150 (0.128 to 0.158) |
| Fragmented | 20 | 0.156 (0.076 to 0.176) |

**The bar is met.** The scanning check is faster than `ffprobe
-count_packets` in both classes, every upper bound below 1.00; the largest
ratio is 0.78. At the median the check takes 1.52 ms and `ffprobe`
16.59 ms. The overall median moved from 0.135 to 0.150 on code that
has grown since (the structure audit and more scanned formats), and stays
far below the bar. The `npx` rounds 1 and 2 also timed `coreav check` on
held-out files of at most 128 MiB, most of them in this set. Evidence: `evidence/speed-02.jsonl.gz` and
`evidence/summary-s6-02.json` (`summarize.py --speed`).
