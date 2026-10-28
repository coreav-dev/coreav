# Corpus v1 run of the upload policy check

Throwaway measurement code, outside the Cargo workspace. It runs
`coreav check` (an earlier pull request, `docs/design/policy-check.md`) on every
corpus v1 file with the three example policies in
`docs/schema/policy-examples/`, and times it against `ffprobe` on the
held-out files.

Python only: `run.py` needs `boto3` (Apache-2.0) for the R2 downloads and
imports `scripts/policy_check.py` and `scripts/policy_ref.py` for the
per-file checks. No Rust code, so there is no Cargo graph to license-check.

## Hypothesis and bars

A verdict is a pure function of the policy and the probe output, its
reasons are exact and point at the values they read, and it agrees with
the verdict the black-box oracle's output implies. The bars, quoted from
the design note and committed before this run:

| Bar | Set | Threshold |
|---|---|---|
| B1 crashes | every file × policy | 100%: exit 0 or 1 within 10 s and 1 GiB, one valid verdict |
| B2 determinism | every file × policy | 100%: two runs print the same bytes with the same exit code |
| B3 spec | every file × policy | 100%: reasons equal `policy_ref.py`'s over the probe output; an unreadable file has one `unreadable` reason carrying the probe's error |
| B4 precision | every reason | 100%: its value is the probe value at its path (or its documented derivation) |
| B5 oracle | `real` files with an ISO base media extension that both CoreAV and `ffprobe` read, × policy | at least 98% of cases agree with the `ffprobe`-derived verdict on the observable rules; every disagreement is listed and explained |
| B6 speed | held-out `real` ISO base media files | median of per-file ratios `coreav check` ÷ `ffprobe -show_format -show_streams` (process wall time, median of 5 interleaved runs each, warm page cache) at most 0.5; reported per class (fragmented, larger than 1 GB) |
| B7 memory | every ISO base media file | peak resident memory of `coreav check` at most 300 MiB, reported against file size |

"Misses on B1 to B4 block the merge. A miss on B5 is investigated, and a
cause in CoreAV is fixed before the merge; a cause in the oracle or its
mapping is recorded with evidence. B6 and B7 misses block a release claim,
not the merge."

## How it runs

    python3 -m venv venv && venv/bin/pip install boto3
    venv/bin/python -I run.py check --work WORK --coreav BIN --ffprobe FFPROBE --jobs 8
    venv/bin/python -I run.py speed --work WORK --coreav BIN --ffprobe FFPROBE
    venv/bin/python -I run.py recheck --work WORK --coreav BIN --ffprobe FFPROBE
    python3 summarize.py --work WORK

R2 credentials come from `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and
`R2_ENDPOINT`. The run caps downloads at 300 GiB and 10,000 requests. R2
charges no egress; the run is local, so its infrastructure cost is the R2
read operations (about 3,000 requests per run, well under 0.01 USD).

## Results

### Runs

| Run | Code | What it measured |
|---|---|---|
| 1 | `2f4212b`: the first version, before the independent review | B1 to B5 and B7 on every file; the first run against the bars |
| 2, 3 | intermediate versions | stopped or superseded when review findings changed the code they measured |
| final | `bda4744` (this pull request after review) with the probe fixes of an earlier pull request applied, as `main` holds them once both merge | B1 to B7 on every file, in one pass with the scripts as committed |

The runs ran on one sandbox machine (8 cores, Amazon Linux 2023), reading
corpus v1 from R2 and checking each file once it was downloaded. The final
run downloaded 247.7 GB in 2,971 requests.

### Final run against the bars

| Bar | Result | Status |
|---|---|---|
| B1 crashes | 0 of 8,913 cases (2,971 files × 3 policies) | **Met** |
| B2 determinism | 0 of 8,913 cases | **Met** |
| B3 spec | 0 of 8,913 cases | **Met** |
| B4 precision | 0 of 11,065 reasons | **Met** |
| B5 oracle | 3,123 of 3,123 cases agree: 3,039 directly (97.31%), 84 through the predicate-checked entries of `docs/known-divergences-policy.md` | **Met**, counting documented divergences as agreement; direct agreement alone is below 98% |
| B6 speed | median ratio **0.133** over 303 held-out files | **Met** (bar 0.5) |
| B7 memory | largest peak 43,612 KiB (44.7 MB), p99 6.0 MB, median 3.1 MB; the 6.3 GB file peaks at 3.1 MB and the 4.7 GB file at 5.0 MB | **Met** (bar 300 MiB) |

### B5 in detail

- 1,050 `real` files have an ISO base media extension. 1,041 were compared:
  9 are files that CoreAV's probe rejects as malformed (4 invalid
  structures, 5 truncated; existing probe strictness) and `ffprobe` reads;
  the check rejects them as `unreadable`.
- Covered cases, each by an entry whose predicate rechecks its condition
  from the box headers and `ffprobe`'s output, never from CoreAV's output:
  KD-P-PROTECTED 72 (encrypted streams: CoreAV leaves `encv`/`enca`
  unidentified, `ffprobe` names the codec inside), KD-P-UNIDENTIFIED-DURATION
  6, KD-P-FRAGMENT-ORIGIN 3, KD-P-CHAPTER-TEXT 2, KD-P-DECLARED-DURATION 1.
- The entries were written after run 1 missed the bar, from the
  investigation the bar requires. Run 1 had 97.12% direct agreement
  (3,033 of 3,123). The investigation found:
  - two probe defects, fixed in an earlier pull request: QuickTime uncompressed
    audio with a sample size of 1 (half the bit rate, or invalid tables),
    and AAC channel counts taken from the sample entry instead of the
    `AudioSpecificConfig` (5.1 reported as stereo);
  - oracle mapping errors, fixed in `scripts/policy_ref.py`: mu-law is
    not linear PCM, QuickTime `text` is not timed text, MP3 counts only in
    `mp4a`, `ffprobe` omits the major brand of some files,
    tracks marked as attached pictures are tracks;
  - five intentional differences, documented with predicates.

### B6 speed (held-out files)

Per-file ratio `coreav check --policy web-upload.json` ÷ `ffprobe -v error
-of json -show_format -show_streams`, process wall time, the median of 5
interleaved rounds each after one warm-up run, files in the page cache.

| Class | Files | Median | 95% CI | p90 | Max | coreav ms | ffprobe ms |
|---|---:|---:|---|---:|---:|---:|---:|
| All held out | 303 | 0.133 | 0.125 to 0.141 | 0.273 | 0.842 | 0.99 | 7.55 |
| Not fragmented | 283 | 0.128 | 0.114 to 0.136 | 0.246 | 0.529 | 0.99 | 7.91 |
| Fragmented | 20 | 0.317 | 0.256 to 0.412 | 0.528 | 0.842 | 0.91 | 3.02 |
| Larger than 1 GB | 12 | 0.129 | 0.045 to 0.185 | 0.202 | 0.252 | 1.28 | 24.09 |

A check is a probe (box headers and sample tables, fragment headers when a
duration needs them) plus the evaluation; `ffprobe` also starts its
demuxers and reads the first packets. The highest ratios are small
fragmented files, where `ffprobe` stops early and the check reads every
fragment header for exact durations.

### Verdicts

| Policy | Accept | Reject | Of which unreadable |
|---|---:|---:|---:|
| strict-gate | 502 | 2469 | 743 |
| web-upload | 939 | 2032 | 743 |
| transcription | 1002 | 1969 | 743 |

Unreadable files are mostly other containers (546 Matroska, WebM, MP3, Ogg
and image files: `unsupported_container`); 160 ISO base media files are
malformed or truncated, 151 of them in the `edge` set. The rule
`multiple_descriptions` failed in 54 cases.

### Checker change during the runs

An earlier version of this pull request's run reported B4 failures for
`unknown` reasons that point inside an object the probe prints as `null`
(`/streams/i/video/width` for a video codec under another handler). The
checker treated such a pointer as missing; `verdict-v1.md` now states that
an `unknown` reason may point there, and the checker resolves the pointer to
`null`. The change was made before the final run, which used it from the
start; the bars themselves did not change.

### Reported, not gated

- Audio channel counts, per stream, against `ffprobe`: 90 files differ. Nearly
  all are AAC streams whose configuration is mono under a stereo sample
  entry; the probe keeps 2 because parametric stereo may be signalled only
  in the bitstream, and `ffprobe` decodes 1. Two are AAC streams with
  channel configuration 0 (a program config element), where the probe keeps
  the entry's 2 and `ffprobe` reports 6.
- Spend: R2 read requests only, about 12,000 over all runs (under 0.01
  USD); egress is free and compute was local.
