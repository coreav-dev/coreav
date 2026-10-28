# Spike A, H7 adaptive routing

Throwaway measurement driver, outside the Cargo workspace. It
measures the adaptive router: per object, the router starts a
whole-object GET, reads the leading boxes from the first bytes of that
stream, predicts the time of the range-read index (I) and of the download
plus local listing (D + L), and either abandons the download and finishes on
range reads, or lets the download finish and lists the file locally. No
crate change: the router lives in this driver (`router.py`,
`h7_adaptive.py`) on top of the `coreav-isobmff` segment-index planner.

## Hypothesis and criteria

Fixed before any data collection:

- **Claim.** A router A, choosing per object between I and the best full
  download plus local listing (D + L), is (a) never meaningfully slower than
  the faster of the two, and (b) faster than a download on large objects.
- **Bar (a).** For every gated held-out file in both regions, the upper bound
  of the bootstrap 95% interval (10,000 resamples, fixed seed) of the mean
  per-trial `A / min(I, D + L)` is at most **1.10**.
- **Bar (b).** For every gated held-out file of at least **250 MB**, the
  upper bound of mean `A / (D + L)` is below **1** in both regions, and the
  median across those files of the per-file median is at most **0.70**.
- **Byte bar.** Whenever A takes the index path, at most 5% of the object's
  bytes are requested or received, in every trial, counting the bytes the
  abandoned stream delivered.
- **Correctness.** Packet digests on either path equal the local digest.
- **Kill.** Any file failing (a), (b) or the byte bar, or a digest
  difference. H7 passes on this route only if all hold in both regions. The
  original bar for I against a full download stays failed and is not re-run.

## Method

The method below, the router code and the held-out list are committed before
any run; `m` and the cold defaults are committed before the held-out run.

### The router

- **Stream first.** A opens a fresh keep-alive pool (the sidx route's client:
  HTTP/1.1, SigV4, at most 32 connections) and starts one whole-object GET on
  its own connection, read on a thread into memory-backed scratch. The driver
  (`src/main.rs --router`) probes through a `RangeReader` whose reads inside
  the **first read** (the first 1 MiB of the stream, the crate's largest
  single read) wait for the stream's bytes. A probe read beyond the first
  read means the leading boxes (and any `sidx`) are not in it: A downloads.
  After probe, reads outside the first read (route reads of `plan_index`) go
  to range GETs while the stream keeps running.
- **Prediction (point 2).** After `plan_index` the driver reports the route,
  the window count, the predicted extra reads (windows shorter than the first
  `moof` plus 16 bytes, the prediction `plan_index` budgets) and the route
  requests. Then

      T_index = RTT × (1 + r_route + ceil(S / 32)) + c_parse
      T_dl    = RTT + size / B + c_local

  with `S` = windows + predicted extra reads and `r_route` = the requests
  `plan_index` made. A takes the index path iff the route is `SegmentIndex`
  and `T_index × m < T_dl`.
- **Index path.** A shuts the stream's socket down, records the bytes the
  stream delivered so far, fetches the plan windows with 32 in flight and
  lists every track's packets. The timer stops when the listing is complete.
- **Download path.** The stream simply continues (one GET); then L lists the
  downloaded file. The timer stops when L is complete.
- **Running estimates** (`router.py`), one router per run, never reset
  between objects: RTT is the median of the last 16 request latencies of A
  (time from sending a request on an open connection to its response
  headers, so connection setup is excluded); B is the median of the last 16
  single-GET throughputs of A's completed downloads of at least 32 MB
  (bytes over the time after the first byte); `c_parse` is the median of the
  driver's CPU time on A's index path; `c_local` the median of A's L times.
  Until a sample exists each uses its cold default.
- **Cold defaults.** Tuning run: the region medians of the last run: RTT 0.2245 s and 0.1594 s (warm one-byte GET), B 43.2 and
  52.9 MB/s (D1 over objects of at least 32 MB), `c_local` 4.3 and 3.3 ms
  (L), and `c_parse` equal to `c_local` (no earlier sample), for us-west and
  ap. Held-out run: the medians of the tuning run of each region
  (`fit_margin.py`); a region without a tuning run keeps the values of
  that last run.

### Runs and engines

- **Inputs** (`make_inputs.py`, `evidence/inputs-tuning.json`,
  `evidence/inputs-holdout.json`).
  - Tuning: the corpus v1 development fragmented files that took the segment
    index route and listed packets in the sidx census, plus the
    development share of the top-up.
  - Held out: the 20 held-out fragmented files of the sidx census with their
    groups (11 gated, 7 encrypted, 2 routed to download, the "no leading
    `sidx`" group of point 10), plus the top-up holdout. Top-up files are
    gated unless tagged encrypted. A gated file whose route is not the
    segment index is still gated; its bar is then against D + L.
  - Counts: tuning 68 files, 10.65 GB (53 corpus v1, 15 top-up); held out
    26 files, 3.89 GB: 17 gated (11 corpus v1, 6 top-up; 6 of them at or
    above 250 MB), 7 encrypted, 2 download.
- **Engines per file and trial** (5 trials, shuffled order): held-out run A, I, D1, D8, with L after each download; tuning run
  A forced to the index path (A_I) and A forced to the download path (A_D),
  each recording the router's prediction, plus I, D1, D8 and L. I, D1, D8
  and L are the sidx time measurement's engines, unchanged. A latency probe per file and
  trial records the TCP+TLS connect time and the median of 5 warm one-byte
  GETs, as in the sidx time measurement.
- **Fit (point 3, `fit_margin.py`).** For every candidate `m` from 1.00 to
  20.00 in steps of 0.01, A(m) per tuning trial is A_I's time if the router
  (with A_I's recorded prediction) would take the index path, else A_D's;
  `m` minimises the worst per-file mean of `A(m) / min(I, D + L)` over the
  tuning files of every tuned region, ties to the smallest `m`.
- **Regions (point 5).** One cloud worker in `us-west` and one in `ap`, 2
  cores, 8 GiB, explicit timeouts, `retries=0` (`remote_adaptive.py`). The
  tuning run is in both regions if the plan allows, else in one; the held-out run is in both regions, once.
- **Launch and preemption** (changed after the two lost tuning runs below,
  before any complete run). A detached launch only spawns the function
  and records its call id; `poll_adaptive.py` waits with 30 s polls,
  retries only client transport errors, treats any other error as the run's
  outcome, falls back to the worker's files on the evidence volume, and
  stops the app if no result arrives before the timeouts. The worker commits
  the volume every 5 minutes. The platform restarts an input whose container is
  preempted (independently of `retries=0`); the restarted container resumes
  from the last checkpoint: it keeps every checkpointed record, restores the
  router's running estimates and the request counters, replays the shuffles
  so the order is unchanged, and measures only what is missing. Records lost
  between the checkpoint and the preemption are measured again. Every
  container is listed in `attempts.json`; at most 3 per run, and the
  measurement window counts from the first, so a restart never extends the
  run beyond its planned time.
- **Tuning region.** After the two lost us-west tuning runs, the
  tuning run is in `ap` only (fixed parameter 5: the cheaper region), so that the
  held-out run in both regions stays within the plan. `us-west` keeps the cold defaults from the sidx time
  measurement.

### Runs

Every run is kept.

| Run | Outcome |
|---|---|
| `tuning-us-west-20261015T085504Z` | lost: the client's single long wait failed after 1,485 s (`ConnectionError: Deadline exceeded`); the detached container ran on until the app was stopped by hand; 3 of 5 trials in the volume, not used |
| `tuning-us-west-20261015T094655Z` | lost: the container was preempted after 117 records; the cloud host restarted the input, which failed on the existing run folder; 1.7 trials in the volume, not used |
| `tuning-ap-20261015T105110Z` | completed, one container, 4,271 s (ap-southeast, Azure); used for the fit |
| `holdout-us-west-20261015T120350Z` | completed, one container, 1,068 s (us-west, Azure) |
| `holdout-ap-20261015T122205Z` | completed, one container, 1,890 s (ap-south, AWS) |

### Statistics and checks (`summarize_adaptive.py`)

- D + L is the faster of D1 + L and D8 + L in the same trial. I counts when
  it listed packets without error; otherwise `min(I, D + L)` is D + L.
- Point 8: per region, for `A / min(I, D + L)`, `A / (D + L)` and `A / I`,
  the per-file mean with its interval and, across gated files, the median,
  p10 and p90 of per-file medians, each with an interval that resamples
  files. The misroute rate (A's path differs from the faster of I and D + L
  in that trial) and the time lost per misroute; the prediction error as
  `T_index / I` and `T_dl / (D1 + L)`; the latency and throughput figures.
- Point 9 sanity checks (a failure invalidates the run, not the bar):
  - I never faster than L in the same trial;
  - A's download path never faster than D1 + L (the same single GET plus
    the same listing) by more than the noise, the per-file spread (max − min)
    of D1 + L over trials;
  - A's index path: point 2 serves A's probe reads from the stream, so A
    does not repeat I's probe range reads and is expected to be faster than
    I in total by about one round trip. The work A shares with I is
    everything after the route decision (window batch and listing), so the
    check compares A after its decision with I after its route decision,
    with the per-file spread of the latter as noise. A against I in total is
    reported, not checked;
  - counted requests and bytes equal what the client sent (A: first-read
    reads plus range GETs; I: range GETs); downloads return exactly the
    object size with the same ETag (checked by the client, a failure is an
    engine error); routes and router decisions per trial, changes counted.
- Point 10 groups: near the crossover (median predicted times within 25%),
  under 10 MB, over 200 subsegments, trials far from the prior (measured D1
  MB/s outside 0.5–2 × the prior B, or warm GET outside 1/1.5–1.5 × the
  prior RTT), no leading `sidx` (the download group), encrypted files, and
  the cold start (the first object of each run).

### Known risk, recorded before any run

Point 2 makes A's download path one GET ("the download simply continues"),
and the cost model uses single-GET throughput, while the bars compare A
with the faster of D1 + L and D8 + L. In the sidx time measurement, D8 beat D1 by
1.3–1.9× on the 30–110 MB files and by about 2× on the 1.39 GB file. A file
that A downloads while D8 is much faster than D1 can therefore fail bar (a)
whatever `m` is. This is the design as accepted; the run reports it, it is
not changed.

A second edge, found in the local smoke test on development files: when
probe has already read every subsegment start (objects of a few hundred
kilobytes), `S` and `r_route` are 0, so `T_index = RTT + c_parse` and the
router takes the index path. The time is then the same as a download, but
the stream may have delivered much of the object, which counts against the
byte bar. No gated held-out file is that small (the smallest is 0.97 MB
with 30 subsegments); the run reports the byte share of every index-path
trial.

## Results (2026-10-15)

### Verdict: H7 fails on the adaptive route

| Bar | us-west (Azure) | ap-south (AWS) | Verdict |
|---|---|---|---|
| (a) upper CI of mean `A / min(I, D + L)` ≤ 1.10, every gated file | 1 of 17 files pass | 0 of 17 files pass | **fail** |
| (b) upper CI of mean `A / (D + L)` < 1, every gated file ≥ 250 MB | 2 of 6 pass | 2 of 6 pass | **fail** |
| (b) median of per-file median `A / (D + L)` ≤ 0.70 (files ≥ 250 MB) | 1.23 [0.42, 2.71] | 0.94 [0.30, 2.04] | **fail** |
| Byte bar (index path, every trial) | max 0.06% (15 index-path trials) | max 0.07% (13) | pass |
| Correctness (packet digests on either path) | all equal | all equal | pass |

The method, router code and held-out list were committed before any run
(a3bbbc3); `m = 5.51` and the cold defaults were committed (c822f10)
before the held-out run, which ran once per region. Nothing was changed
after it. The original bar for I against a full download stays failed
and was not re-run.

### Why

- **The download path is one GET; the best download is not.** Point 2 lets
  the started GET simply continue. On the download path A costs the same as
  D1 + L (median `A / (D1 + L)` 1.01 in us-west, 1.06 in ap), but D8, the
  parallel ranged download, was faster than D1 by a median factor of 1.50
  and 1.34 on those trials, so the median download-path `A / (D + L)` is
  1.64 and 1.47. Bar (a) cannot pass on any file A downloads while D8 beats
  D1, whatever `m` is. This was the known risk recorded before the run.
- **The model misses sequential extra reads, so `m` came out large.** The
  planner predicts extra reads from the first `moof` only. In the tuning
  split, files whose later `moof` boxes outgrow their window (the 60 fps Big
  Buck Bunny representations) needed about 60 sequential extra reads, and I
  and A's index path took 13–14 s against a predicted 1.3 s. The fit then
  chose `m = 5.51` (worst tuning per-file mean 4.86, against 8.43 at
  `m = 1`), which keeps A on the download path for most objects: A took the
  index path in 15 (us-west) and 13 (ap) of 85 gated trials, only on the
  three largest files with few subsegments (300 MB, 408 MB, 1.39 GB).
- **Where A takes the index, it wins.** On index-path trials A was faster
  than I (median `A / I` 0.97 and 0.95: A reads the probe bytes from the
  stream), and the 1.39 GB file passes bar (b) in both regions (mean
  `A / (D + L)` 0.36 [0.27, 0.43] and 0.34 [0.20, 0.58]); the 300 MB file
  passes (b) in us-west, the 408 MB file in ap.
- **Every misroute was a download where the index was faster** (6 of 85
  trials in us-west, 8 in ap), mostly the 523 MB Sintel representation
  (222 subsegments), losing up to 9.1 s.

### Numbers per region (point 8)

| Region (provider) | Warm 1-byte GET median (p10–p90) | Connect median | Single GET MB/s (≥ 32 MB) | Bar (a) passes | Bar (b) passes | (b) median of medians [95% CI] | Max index-path share | Digests |
|---|---|---:|---:|---:|---:|---|---:|---|
| us-west (Azure) | 200 ms (191–223) | 25 ms | 59.1 | 1 / 17 | 2 / 6 | 1.23 [0.42, 2.71] | 0.06% | equal |
| ap-south (AWS) | 221 ms (208–239) | 11 ms | 49.9 | 0 / 17 | 2 / 6 | 0.94 [0.30, 2.04] | 0.07% | equal |

Distributions across gated files (per-file medians; bootstrap 95% CI resampling files):

| Region | Ratio | p10 [CI] | Median [CI] | p90 [CI] |
|---|---|---|---|---|
| us-west | A / min(I, D + L) | 0.99 [0.95, 1.22] | 1.40 [1.10, 1.96] | 2.32 [1.77, 3.36] |
| us-west | A / (D + L) | 0.44 [0.41, 1.17] | 1.40 [1.05, 1.86] | 2.32 [1.78, 3.36] |
| us-west | A / I | 0.35 [0.21, 0.57] | 0.82 [0.53, 1.25] | 1.81 [1.10, 3.15] |
| ap-south | A / min(I, D + L) | 0.97 [0.91, 1.06] | 1.46 [1.02, 1.74] | 2.03 [1.59, 2.77] |
| ap-south | A / (D + L) | 0.39 [0.25, 1.07] | 1.46 [1.02, 1.55] | 1.83 [1.51, 2.26] |
| ap-south | A / I | 0.33 [0.18, 0.48] | 0.76 [0.44, 1.15] | 1.60 [1.01, 2.77] |

Per gated file (5 trials; A, I, D + L are medians over trials in seconds; ratios are the mean per-trial ratio with its 95% CI; paths are A's path per trial, i = index, d = download):

| File | Set | MB | Subseg. | us-west A / I / D+L s | us-west A/min mean [CI] | us-west A/(D+L) mean [CI] | us-west paths | ap-south A / I / D+L s | ap-south A/min mean [CI] | ap-south A/(D+L) mean [CI] | ap-south paths | Verdict |
|---|---|---:|---:|---|---|---|---|---|---|---|---|---|
| `c86f3d1f2aff` | v1-holdout | 1.0 | 30 | 0.32 / 0.56 / 0.31 | 1.16 [0.94, 1.55] | 1.16 [0.94, 1.55] | ddddd | 0.31 / 0.75 / 0.28 | 1.06 [0.91, 1.23] | 1.06 [0.91, 1.23] | ddddd | fail |
| `aa7931b18bb0` | v1-holdout | 3.0 | 45 | 0.32 / 0.78 / 0.30 | 1.17 [0.99, 1.47] | 1.17 [0.99, 1.47] | ddddd | 0.36 / 0.87 / 0.33 | 1.18 [0.95, 1.52] | 1.18 [0.95, 1.52] | ddddd | fail |
| `d4e8cc76b778` | v1-holdout | 5.4 | 20 | 0.50 / 0.79 / 0.33 | 1.48 [1.38, 1.58] | 1.48 [1.37, 1.58] | ddddd | 0.56 / 1.24 / 0.37 | 1.69 [1.44, 2.03] | 1.69 [1.44, 2.03] | ddddd | fail |
| `e4f4b288040c` | v1-holdout | 7.9 | 23 | 0.51 / 0.77 / 0.37 | 1.46 [1.30, 1.68] | 1.46 [1.30, 1.67] | ddddd | 0.67 / 1.18 / 0.42 | 1.45 [1.17, 1.72] | 1.45 [1.17, 1.72] | ddddd | fail |
| `97bdf1aaa8ff` | v1-holdout | 12.6 | 184 | 0.46 / 1.68 / 0.37 | 1.30 [1.04, 1.55] | 1.30 [1.04, 1.55] | ddddd | 0.71 / 2.27 / 0.46 | 1.45 [0.90, 2.14] | 1.45 [0.90, 2.14] | ddddd | fail |
| `cf502122eb67` | v1-holdout | 14.7 | 222 | 0.50 / 2.15 / 0.41 | 1.25 [1.09, 1.45] | 1.25 [1.08, 1.45] | ddddd | 0.46 / 2.56 / 0.44 | 1.09 [1.01, 1.15] | 1.09 [1.01, 1.16] | ddddd | fail |
| `c81111de114e` | v1-holdout | 31.7 | 30 | 0.88 / 0.56 / 0.55 | 1.61 [1.18, 2.04] | 1.57 [1.16, 1.98] | ddddd | 1.16 / 1.12 / 0.56 | 2.09 [1.56, 2.87] | 2.09 [1.56, 2.87] | ddddd | fail |
| `a2d42b189482` | v1-holdout | 36.1 | 123 | 0.87 / 1.53 / 0.63 | 1.75 [1.16, 2.57] | 1.75 [1.16, 2.57] | ddddd | 1.38 / 2.66 / 0.75 | 1.99 [1.48, 2.50] | 1.99 [1.48, 2.50] | ddddd | fail |
| `de7354bf340b` | v1-holdout | 101.5 | 384 | 1.42 / 3.28 / 0.80 | 2.30 [1.49, 3.60] | 2.30 [1.49, 3.58] | ddddd | 2.11 / 4.71 / 1.36 | 1.37 [0.98, 1.71] | 1.37 [0.98, 1.71] | ddddd | fail |
| `209b0656e14f` | topup-holdout | 102.2 | 94 | 1.46 / 1.07 / 0.72 | 2.43 [1.35, 3.50] | 2.39 [1.27, 3.50] | ddddd | 2.61 / 1.57 / 1.12 | 2.25 [1.31, 3.55] | 2.25 [1.31, 3.55] | ddddd | fail |
| `4642ef7a47b3` | v1-holdout | 110.7 | 77 | 1.92 / 0.97 / 0.69 | 3.37 [2.27, 4.67] | 3.37 [2.27, 4.67] | ddddd | 2.63 / 1.40 / 1.78 | 1.94 [1.54, 2.33] | 1.65 [1.23, 2.07] | ddddd | fail |
| `98e9d0b932ab` | topup-holdout | 299.9 | 31 | 0.65 / 0.59 / 1.48 | 1.00 [0.84, 1.14] | 0.42 [0.37, 0.47] | iiiii | 1.10 / 0.96 / 2.88 | 2.41 [0.95, 4.48] | 0.94 [0.32, 1.62] | ddiii | fail |
| `520043d30f5f` | topup-holdout | 341.0 | – | 4.30 / nan / 2.09 | 1.86 [1.33, 2.23] | 1.86 [1.36, 2.23] | ddddd | 5.05 / nan / 3.49 | 1.67 [1.44, 2.03] | 1.67 [1.44, 2.02] | ddddd | fail |
| `9858f3d1cb2d` | topup-holdout | 372.6 | 372 | 3.65 / 3.30 / 1.75 | 2.25 [1.93, 2.64] | 2.25 [1.93, 2.64] | ddddd | 8.36 / 5.43 / 3.26 | 2.37 [1.78, 2.97] | 2.37 [1.78, 2.97] | ddddd | fail |
| `2e9582e47a1f` | topup-holdout | 407.7 | 41 | 0.75 / 0.76 / 1.94 | 2.40 [0.94, 5.23] | 0.67 [0.27, 1.33] | iiiii | 1.21 / 1.34 / 3.58 | 0.94 [0.73, 1.16] | 0.35 [0.29, 0.40] | iiiii | fail |
| `1ddd1a40d405` | topup-holdout | 523.5 | 222 | 7.67 / 2.15 / 2.53 | 3.96 [2.98, 4.94] | 3.50 [2.80, 4.21] | ddddd | 9.62 / 3.54 / 4.50 | 2.44 [1.88, 2.88] | 1.83 [1.53, 2.14] | ddddd | fail |
| `ac56f681d198` | v1-holdout | 1386.5 | 208 | 1.99 / 2.18 / 5.12 | 0.92 [0.86, 0.96] | 0.36 [0.27, 0.43] | iiiii | 3.00 / 2.78 / 11.24 | 1.11 [0.75, 1.55] | 0.34 [0.20, 0.58] | iiiii | fail |

| Region | Misroutes / trials | Index chosen, download faster | Download chosen, index faster | Time lost per misroute, median (max) | T_index / I median (p10–p90) | T_dl / (D1 + L) median (p10–p90) |
|---|---|---:|---:|---|---|---|
| us-west | 6 / 85 (7%) | 0 | 6 | 2.51 s (9.13 s) | 0.84 (0.66–0.93) | 0.84 (0.58–1.24) |
| ap-south | 8 / 85 (9%) | 0 | 8 | 3.89 s (6.72 s) | 0.77 (0.51–1.12) | 0.95 (0.55–1.32) |

Shrink groups (gated files unless named; medians of per-file medians):

| Group | us-west files | us-west A/min | us-west A/(D+L) | us-west bar (a) passes | ap-south files | ap-south A/min | ap-south A/(D+L) | ap-south bar (a) passes |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| near_crossover_25pct | 0 | – | – | 0 | 0 | – | – | 0 |
| small_under_10MB | 4 | 1.22 | 1.22 | 0 | 4 | 1.27 | 1.27 | 0 |
| many_subsegments_over_200 | 5 | 1.73 | 1.73 | 1 | 5 | 1.45 | 1.45 | 0 |
| large_250MB_plus | 6 | 1.57 | 1.23 | 1 | 6 | 1.25 | 0.94 | 0 |
| no_leading_sidx_download | 2 | 1.08 | 1.08 | 0 | 2 | 1.23 | 1.23 | 0 |
| encrypted | 7 | 1.12 | 1.12 | 1 | 7 | 1.19 | 1.19 | 0 |

- us-west: trials far from the prior 36 of 85, median A/min 1.26 against 1.92 for the others, 1 misroutes. Cold start: download path, A/min 0.96.

- ap-south: trials far from the prior 57 of 85, median A/min 1.44 against 1.43 for the others, 6 misroutes. Cold start: download path, A/min 0.85.

- us-west sanity: {"I_faster_than_L": 0, "A_download_beats_D1L_beyond_noise": 0, "A_index_after_decision_beats_I_after_route_beyond_noise": 0, "A_index_total_faster_than_I_trials": 10, "A_index_trials_with_I": 15, "A_accounting_mismatch": 0, "I_accounting_mismatch": 0, "engine_errors": 0, "I_route_changes": 0, "A_route_changes": 0, "decision_changes": 0, "digest_mismatch_files": 0}
- ap-south sanity: {"I_faster_than_L": 0, "A_download_beats_D1L_beyond_noise": 0, "A_index_after_decision_beats_I_after_route_beyond_noise": 0, "A_index_total_faster_than_I_trials": 7, "A_index_trials_with_I": 13, "A_accounting_mismatch": 0, "I_accounting_mismatch": 0, "engine_errors": 1, "I_route_changes": 0, "A_route_changes": 0, "decision_changes": 1, "digest_mismatch_files": 0}


Per-file means of `A / I` with their intervals, and every per-file
statistic, are in `evidence/summary.json`.

Notes:

- `520043d30f5f` (341 MB) has no I time: its index packs several fragments
  per subsegment, so the planner routes it to download-then-index and A
  downloads it every time (gated against D + L, as fixed before the run).
- One I range GET timed out in ap (`9858f3d1cb2d`, trial 2); recorded, not
  retried. One router decision changed between trials (ap, `98e9d0b932ab`:
  download twice while the running throughput estimate settled, then index).
- Regions: us-west landed on Azure in both the tuning and the held-out
  phase; the `ap` request landed on ap-southeast (Azure) for tuning and on
  ap-south (AWS) for the held-out run (the sidx time measurement's ap run
  was ap-southeast, AWS). Reported, not rerun (point 5).

### Corpus top-up (point 4)

The top-up corpus has 21 `real` files, 10.25 GB, 16 over 250 MB; 6 held
out with the new salt (102, 299, 340, 372, 407 and 523 MB). The 40-file
target was not reached: the title, signature and packager-key caps bind
first, several candidates have no leading `sidx` and some could not be fetched. With the 11 gated corpus v1 files,
the held-out set had 17 gated files, 6 of them at or above 250 MB. The
details and every skip reason are in the top-up corpus's own README.

### Sanity checks (point 9)

All pass in both regions, so the run is valid: I was never faster than L
(0 trials); A's download path never beat D1 + L beyond the noise; A after
its decision never beat I after its route decision beyond the noise; A's
and I's counted requests and bytes equalled what the client sent in every
trial; every download returned the object size with the same ETag (no
download error); no route changed between trials. In total time, A's index
path was faster than I in 10 of 15 (us-west) and 7 of 13 (ap) trials, as
expected from the stream-served probe.

## Reproduce

Credentials as `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `R2_ENDPOINT`
(never printed):

    cargo build --release --target x86_64-unknown-linux-musl
    python3 -I make_inputs.py V1_MANIFEST.json <top-up manifest.json> evidence
    python3 -I run_adaptive.py launch
    python3 -I fit_margin.py evidence/params-tuning.json evidence/<run>/raw.json ... > evidence/params-holdout.json
    python3 -I summarize_adaptive.py --inputs evidence/inputs-holdout.json evidence/<us-west run>/raw.json evidence/<ap run>/raw.json > evidence/summary.json
