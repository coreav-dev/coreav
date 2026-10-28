# Spike A, H7 sidx census

Throwaway measurement driver, outside the Cargo workspace. It runs
the hardened `coreav-isobmff` remote index path (`RangeReader`, `plan_index`,
packet listing) over real range reads against the corpus v1 objects in R2 and
records the route, requests and requested bytes of every fragmented file. It is
a byte-accounting census. The H7 time measurement of this route, against a full download, is in the section "H7 time measurement" below.

## Hypothesis and criteria

Fixed before the run:

- **Byte bar (pass).** Every file routed to the segment index or to a short
  sequential walk requests at most 5% of its bytes, probe plus index, with
  speculative windows counted.
- **Typed routes (pass).** Every other fragmented file gets a typed
  `DownloadThenIndex` reason before any fragment walk, never a slow walk.
- **Correctness (pass).** Packets listed over range reads equal packets
  listed from a local copy.
- **Kill.** Any routed file over 5%, a routed walk stopped by the byte cap,
  or a packet difference.
- The time bar against a full download is **not** evaluated here.

## Method

- `census.py` selects the corpus v1 files tagged `fragmented` (218
  development, 20 held out). For each file, it starts the Rust driver
  (`src/main.rs`) and serves its reads with signed R2 range GETs
  (`../r2_io.py`). Plan windows arrive as one batch and are fetched with at
  most 32 concurrent requests, then inserted into the
  reader. No retries; a transport failure is recorded as an error.
- Both sides count requests and bytes; `accounting_matches_sender` checks that
  the reader's counters equal what the parent actually sent.
- `src/main.rs --local FILE` lists the same packets from a local file through
  `FileSource`. The digest is FNV-1a over track, offset, size, decode time,
  composition time and sync flag of every packet.
- `summarize.py` produces the summaries in `evidence/`.

Reproduce (credentials as `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
`R2_ENDPOINT`; never printed):

    cargo build --release
    python3 -I census.py FILES.json OUT.jsonl [--holdout]
    python3 -I summarize.py OUT.jsonl

## Results

Final code (after the review fixes on an earlier pull request). Development split,
`evidence/dev-run5.jsonl`, 218 files:

| Route | Files | Max share of bytes | Median share | Median / max requests | Median wall ms |
|---|---:|---:|---:|---:|---:|
| Segment index | 81 | 4.735% | 2.562% | 77 / 385 | 2,199 |
| Sequential walk | 11 | 1.229% | 0.594% | 2 / 4 | 654 |
| Download then index | 103 | (probe only) | | 1 / 3 | 380 |
| Probe error | 22 | | | | |
| R2 transport error | 1 | | | | |

- Download reasons: byte bar 77 (small objects: one 4 KiB probe read is
  already more than 5% of them), no segment index with more than 8 estimated
  fragments 20, several fragments per subsegment 4, per-segment indexes
  (incomplete leading index) 2.
- Probe errors are the crate's existing outcomes on these files (media
  segments without a movie box, truncated files); a local probe gives the
  same errors.
- The transport error is one failed range GET on an 11 MB segment-index file
  (not retried); in run 4 the same file was indexed and matched its local
  listing.
- No routed file was stopped by the byte cap, and the counters matched the
  sender on every file.
- Correctness: the 84 routed development files up to 100 MB were downloaded
  and listed locally. In run 5, 83 agree (82 identical digests and packet
  counts, one truncated file failing identically) and the 84th is the
  transport error above. In run 4, all 84 agree.
  `evidence/local-digests-run5.json`, `evidence/local-digests-run4.json`.

Held-out split, `evidence/holdout-run2.jsonl`, 20 files: 18 on the
segment-index route, max **4.214%** of bytes (median 1.426%, median 124
requests); 2 small files routed to download by the byte bar; no errors. The
held-out split was never used to change anything. It was measured once with
the planner of run 4 (`holdout-run1`: max 4.214%, the same routes) and once
more with the final code after the review fixes, which were made for
reasons unrelated to it.

**Verdict for this census: pass** on the byte bar, typed routes and
correctness. It says nothing about the time bar.

### Development history

The planner changed twice during the development runs, each time on
development files only; every run is kept:

- Run 1 (`dev-run1`): 4 files stopped by the byte cap mid-walk. Cause: a
  `sidx` whose subsegments hold about 10 fragments each, so one window per
  subsegment missed most `moof` boxes; and a root index over leaf indexes
  before the first fragment was rejected as hierarchical.
- Run 2 (`dev-run2`): the first fix routes multi-fragment subsegments with a
  typed status and resolves leading index trees. 2 files still stopped
  mid-walk: their single `moof` was larger than any window.
- Run 3 (`dev-run3`): the planner predicts that extra read; no cap stop. One
  R2 transport error (not retried).
- Run 4 (`dev-run4`): the same planner, with packet digests; no errors other
  than probe errors.
- Run 5 (`dev-run5`): after review fixes (a read with a held prefix requests
  only the rest; no kept window contains another; a plan must fit the
  reader's room). Same routes as run 4; one R2 transport error.

## H7 time measurement

The method was fixed and committed (b35ccfd) before
any run: the engines, the client, the statistics, the sanity checks and the
groups. The held-out files were not used to
change anything; the route's defaults are unchanged.

- **Harness.** `h7_time.py` runs every engine with a fresh keep-alive pool
  (`r2pool.py`, SigV4, at most 32 connections), so TCP and TLS setup is always
  timed. The engines per file and trial, in shuffled order, are: I (the
  driver over range reads, plan windows fetched with 32 in flight, every
  track's packets listed), D1 (one GET), D8 (8 MiB parts, 32 in flight) and L
  (local listing of each downloaded file). I1 (one request in flight) runs in
  the first trial only. 5 trials per file. `remote_time.py` runs one worker
  per region (2 cores, 8 GiB, `retries=0`, 3,000 s timeout);
  `summarize_time.py` computes the statistics (10,000 bootstrap resamples,
  seed 20261015).
- **Inputs** (`evidence/time/inputs.json`): the 20 held-out fragmented files.
  11 are gated (segment-index route, index lists packets). 7 are encrypted:
  segment-index route, but every stream is `encv`/`enca` and dropped by the
  allowlist, so the index lists nothing; they are not gated. 2 are
  download-routed, outside H7.
- **Runs.** `time-us-west-20261015T044716Z` and `time-ap-20261015T050944Z`.
  Raw records are in `evidence/time/<run>/raw.json`, statistics in
  `evidence/time/summary.json`.
- **Conditions.** Both workers ran in the cloud host's gVisor sandbox with 2 vCPUs.
  The sandbox hides the CPU model and reports a load average of 0.0, so CPU
  load could not be measured beyond that. Downloads were written to
  `/dev/shm` (8 GiB, memory-backed). The `ap` request landed in
  `ap-southeast` on AWS (the earlier H7 run had landed in `ap-northeast` on
  GCP); `us-west` was on Azure. Each run made 13,512 R2 requests and moved
  18.5 GB, almost all of it the download baselines.

### Verdict: H7 fails on the segment-index route

| Region (provider) | Warm 1-byte GET median (p10–p90) | TCP+TLS connect median | Single GET MB/s median (p10–p90) | Gated passes | p10 of per-file median R [95% CI] | Median [95% CI] | p90 [95% CI] |
|---|---|---|---|---:|---|---|---|
| us-west (AZURE) | 225 ms (206–252) | 28 ms | 20.2 (2.5–51.1) | 1 / 11 | 0.89 [0.23, 2.31] | 2.31 [1.15, 3.03] | 3.17 [2.92, 4.83] |
| ap-southeast (AWS) | 159 ms (142–196) | 31 ms | 27.5 (2.5–59.4) | 1 / 11 | 0.93 [0.18, 2.66] | 2.85 [1.43, 3.66] | 3.70 [2.89, 5.62] |

`R_full` (index time over the best download plus local indexing), per-file medians across the 11 gated files:

| Region | p10 [95% CI] | Median [95% CI] | p90 [95% CI] |
|---|---|---|---|
| us-west | 0.89 [0.23, 2.28] | 2.28 [1.14, 2.97] | 3.12 [2.88, 4.71] |
| ap-southeast | 0.92 [0.18, 2.62] | 2.82 [1.42, 3.60] | 3.63 [2.85, 5.36] |

Per gated file (5 trials each; I = index time, D = best download, both medians over trials; R = mean of per-trial `I / D` with its bootstrap 95% CI):

| File | Size MB | Subsegments | Requests | Max bytes share | us-west I / D s | us-west mean R [95% CI] | ap I / D s | ap mean R [95% CI] | Verdict |
|---|---:|---:|---:|---:|---|---|---|---|---|
| `c86f3d1f2aff` | 1.0 | 30 | 30 | 3.48% | 0.75 / 0.30 | 2.80 [1.99, 4.20] | 0.73 / 0.22 | 4.84 [2.63, 7.33] | fail |
| `aa7931b18bb0` | 3.0 | 45 | 45 | 3.10% | 0.86 / 0.37 | 2.45 [2.22, 2.77] | 0.65 / 0.26 | 3.36 [2.40, 4.90] | fail |
| `d4e8cc76b778` | 5.4 | 20 | 22 | 1.41% | 0.92 / 0.40 | 2.22 [1.82, 2.52] | 1.08 / 0.35 | 3.83 [2.48, 5.31] | fail |
| `e4f4b288040c` | 7.9 | 23 | 25 | 0.95% | 1.09 / 0.37 | 3.77 [2.33, 5.49] | 0.74 / 0.27 | 3.68 [2.50, 5.64] | fail |
| `97bdf1aaa8ff` | 12.6 | 184 | 185 | 3.03% | 1.93 / 0.61 | 3.71 [2.80, 5.01] | 1.50 / 0.43 | 3.69 [2.95, 4.42] | fail |
| `cf502122eb67` | 14.7 | 222 | 224 | 3.15% | 2.36 / 0.49 | 4.90 [4.64, 5.19] | 2.55 / 0.41 | 6.18 [4.45, 8.49] | fail |
| `c81111de114e` | 31.7 | 30 | 30 | 0.11% | 0.74 / 0.69 | 1.07 [0.94, 1.20] | 0.74 / 0.49 | 1.35 [1.10, 1.58] | fail |
| `a2d42b189482` | 36.1 | 123 | 124 | 1.42% | 2.01 / 0.79 | 2.97 [2.40, 3.56] | 1.33 / 0.56 | 2.38 [1.79, 3.08] | fail |
| `de7354bf340b` | 101.5 | 384 | 385 | 0.59% | 3.53 / 1.19 | 3.10 [2.32, 3.79] | 2.68 / 1.04 | 2.67 [2.44, 2.89] | fail |
| `4642ef7a47b3` | 110.7 | 77 | 77 | 0.11% | 1.12 / 1.43 | 1.40 [0.80, 2.39] | 0.91 / 1.07 | 1.06 [0.82, 1.44] | fail |
| `ac56f681d198` | 1386.5 | 208 | 210 | 0.05% | 2.37 / 12.66 | 0.32 [0.19, 0.48] | 1.77 / 10.07 | 0.17 [0.15, 0.19] | pass |

Encrypted files (segment-index route, every stream dropped, not gated):

| Region | Files | Passes | Median of per-file median R | Verdict over all 18 | All-18 median R [95% CI] |
|---|---:|---:|---:|---|---|
| us-west | 7 | 0 | 2.51 | fail | 2.42 [2.18, 3.03] |
| ap-southeast | 7 | 0 | 2.67 | fail | 2.76 [2.31, 3.13] |

Download-routed files (outside H7):

| File | Size bytes | Region | Time to the route decision (median) | Best download + local index (median) | Route + download + local over download + local |
|---|---:|---|---:|---:|---:|
| `895d1f32f29c` | 986,360 | us-west | 460 ms | 307 ms | 2.63 |
| `cc6023189f53` | 40,609 | us-west | 250 ms | 282 ms | 2.00 |
| `895d1f32f29c` | 986,360 | ap-southeast | 311 ms | 285 ms | 2.18 |
| `cc6023189f53` | 40,609 | ap-southeast | 197 ms | 183 ms | 2.13 |

Shrink groups (median of per-file median R; passes):

| Group | us-west files | us-west median R | us-west passes | ap files | ap median R | ap passes |
|---|---:|---:|---:|---:|---:|---:|
| small_under_10MB | 4 | 2.31 | 0 | 4 | 2.97 | 0 |
| size_10MB_to_100MB | 4 | 3.10 | 0 | 4 | 3.03 | 0 |
| large_100MB_plus | 3 | 0.89 | 1 | 3 | 0.93 | 1 |
| many_subsegments_over_200 | 3 | 3.03 | 1 | 3 | 2.66 | 1 |
| subsegments_up_to_200 | 8 | 2.31 | 0 | 8 | 2.87 | 0 |
| latency_above_region_median | 5 | 2.31 | 1 | 5 | 2.89 | 1 |
| latency_at_or_below_region_median | 6 | 2.62 | 0 | 6 | 2.61 | 0 |


- **The H7 bar.** H7 passes only if every gated file passes in every
  region. Only 1 of 11 passes, in both regions: the 1.39 GB file (mean `R`
  0.32 [0.19, 0.48] in us-west, 0.17 [0.15, 0.19] in ap-southeast). The
  110.7 MB file's interval crosses 1 in both regions. The other 9 files take
  1.1 to 6.2 times as long as downloading the whole object. **H7 fails.**
- **Bytes.** The byte bar holds for every gated file in every trial: at
  most 3.48% of the object.
- **Download plus local indexing** (a comparison fixed in the method)
  changes nothing: local indexing of a downloaded file takes milliseconds
  (median 3–4 ms, at most 26 ms), so the per-file median `R_full` is within
  5% of `R`.
- **Sanity checks.** All pass in both regions:
    - the remote index was never faster than the local index of the same
      file in the same trial (0 of 200 comparisons per region);
    - the driver's counters matched what the client sent in every trial;
    - packet digests over range reads equalled the local digests in every
      trial;
    - every download returned exactly the object size with the same ETag;
    - no route changed between trials.
- **Concurrency helps here.** The one-request-in-flight diagnostic I1 took
  2.5 to 29 times as long as I (median per file). That is the opposite of the
  containers of the earlier H7 run, where concurrency made things slower.

### Where the time goes, and where the advantage shrinks

The index is bound by round trips, not bytes. Median over gated trials:

| Region | Probe done | Route decided | Window batch | Index complete |
|---|---:|---:|---:|---:|
| us-west | 277 ms | 446 ms | 1,144 ms | 1,788 ms |
| ap-southeast | 222 ms | 334 ms | 827 ms | 1,438 ms |

A one-byte GET costs 160–225 ms, even on a warm connection. The index
needs at least the probe round trip, about one round trip of route reads,
and then `ceil(subsegments / 32)` rounds of window requests on 32 new TLS
connections. A whole-object GET needs one round trip plus the transfer.

- **Small files lose.** Under 10 MB, the median `R` is 2.3 (us-west) and
  3.0 (ap). The download is a single round trip, and the index cannot finish
  in fewer than two or three. Files routed to download-then-index pay the
  route decision on top of that: probe plus plan before the download made
  them 2.0–2.6 times slower than a plain download.
- **Many subsegments lose.** Files with more than 200 subsegments need
  7 to 12 window rounds. The 14.7 MB file with 222 subsegments is the worst
  at `R` 4.9–6.2. Even the 101.5 MB file with 384 subsegments takes 2.7–3.1
  times as long as its download.
- **Latency does not decide it alone.** ap-southeast had about 30% lower
  latency than us-west, and its median `R` was not lower (2.85 against
  2.31), because its downloads were faster too. Within a region, the
  one-byte latency of different files varied too little to separate them.
- **Only large objects win.** The index wins only when the transfer takes
  longer than the index's 1.4–1.8 s of round trips. At the large-object
  throughputs measured here (roughly 85–140 MB/s for the 100 MB and 1.4 GB
  files), that means objects of very roughly 150–250 MB or more. This is an
  inference from two points (110.7 MB fails with `R` near 1, 1.39 GB passes),
  not a measured threshold.

### Evidence for the route's review-requested defaults

Both are kept, and the measurement shows no problem caused by either.

- **Windows of at most 4 KiB.** Every gated trial stayed under 3.5% of
  bytes, and no trial was stopped by a cap. On the development split, 77 of
  81 segment-index files needed at most one read beyond probe and windows,
  with a median window of 2,560 bytes. The time is latency-bound: a 512-byte
  and a 4 KiB request cost the same round trip, so larger windows would not
  have changed `R`, and smaller ones would not help either.
- **Routing thresholds (8 unindexed fragments, the byte bar).** No held-out
  file took the sequential route. Both download-routed held-out files were
  sent there by the byte bar (40.6 kB and 986 kB objects), and downloading
  them is indeed the fast path (183–307 ms). On the development split, the
  sequential walks had at most 7 estimated fragments, and the download-routed
  estimates start at 10. The measurement says one thing about the bound: a
  walk of up to 8 sequential round trips (about 1.3–1.8 s at these
  latencies) would also lose to a download for objects under about 100 MB.
  Changing it needs a new decision; the bound is an open question of the
  route.

## Limitations

- The census wall times are from one development host. The time bar is
  measured separately above, with one worker in each of two cloud regions
  and 11 gated files; other regions, clouds, and S3 or GCS are unmeasured.
- Packet equality on real files was checked only for files up to 100 MB.
- Every R2 transfer: about 25 MB and 9,600 requests per development run (five
  runs), about 5.2 MB and 2,060 requests per held-out run (two runs), plus
  1.05 GB downloaded for the local comparison.
