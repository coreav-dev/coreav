# H.264 held-out speed measurement of main

Status: Accepted. The salt, the list and the bars below were fixed before any
timing run; the results are below the line at the end of this file, with
the bars unmoved.

N2's 139-stream held-out set is spent (`docs/design/h264.md`, "The held-out
draw"). This is the fresh draw that note calls for, measured against the
decoder on `main`.

## Draw

`scripts/h264_heldout_draw.py` (run at `ca5e4bc8`) writes `pool.json`,
`excluded.json` and `list.json` next to this file.

- **Pool.** Corpus v1 `real` streams that N1's corpus run
  (`docs/spikes/h264-n1/evidence/corpus.jsonl.gz`) decoded and that agreed with
  FFmpeg, with N1's FFmpeg time at least 0.5 s: N2's pool. Scope is N1's:
  profile 66 with constraint_set1, 77 or 100, 8-bit 4:2:0, progressive, no
  MBAFF. Minus: N2's 139 held-out streams (137 of them were in this pool,
  which is what `list.json` counts as `n2_heldout`); every stream whose
  12-hex prefix appears in the tracked text of `docs`, `crates`, `bench`,
  `scripts`, N1's and N2's READMEs and N2's `sets/` at `ca5e4bc8` (tuning,
  profiling and A/B sets and any other mention: 63 streams,
  `excluded.json`); and streams whose estimated FFmpeg time for the frames
  measured is under 0.4 s (8 streams). Pool left: 58 sd, 71 720p, 64 1080p,
  49 4k.
- **Salt.** `coreav-h264-n2-heldout-1:20261021`, SHA-256
  `3cf2c0debda603a17da34015533e8f1f958c25468b715a8e514156b6599023b2`.
- **Rule.** Per resolution class, order the pool by
  HMAC-SHA256(salt, `<file sha256>.<track>`) ascending and take the first 16:
  16 sd, 16 720p, 16 1080p, 16 4k, 64 in all. Classes by luma samples as in
  N2: sd at most 640x480, 720p at most 1280x720, 1080p at most 1920x1088,
  4k above.
- **List.** `list.json`, SHA-256
  `7b4ce4119cb61acf7f2e9825dd882b0accdfe90e5d3022e37182c69ff27c11fd` as fixed (the public copy, whose `key` fields hold the file name only and whose `category` fields are removed, hashes to `4113b5a262d04bd2e401e5feecf3138f9f403db4eec8efb5d858bb5a9087e2e8`). It mixes profiles
  as the corpus does (44 High, 9 Main, 11 constrained Baseline); the profile
  is not a stratum.
- The draw is spent once a number is reported from it. It is not used for
  tuning, profiling or any later A/B.

## What is measured

- **Binary.** `main` at the SHA recorded in the results, built with
  `cargo build --profile dist --example decode -p coreav-h264`, SIMD kernels
  on (`Auto`).
- **Input.** Each stream's video track is stream-copied (no decode) to an
  Annex B elementary stream: the first `F` access units, `F` by class:
  sd 1,800, 720p 900, 1080p 300, 4k 150, or the whole stream where it has
  fewer. These are chosen so FFmpeg spends about half a second or more on a
  stream, where process start-up no longer dominates a ratio; N2 timed whole
  streams up to 9,000 frames, which is not affordable for 64 streams times two
  FFmpeg builds, and a ratio over the first `F` units is what the rounds of
  step 6 report. Both sides read the same bytes (SHA-256 of every
  elementary stream in the report).
- **Harness.** `scripts/h264_parity.py` from `main`, one call per class and
  FFmpeg build, `--pin 2 --rounds 5 --frames F`, every command of a stream
  timed in the same interleaved rounds after one warm-up. FFmpeg runs
  `-threads 1`. Symmetric pairings only: `discard` (ours `--discard`
  against `ffmpeg -f null`) and `write` (ours to stdout against `ffmpeg -f
  rawvideo`); `mixed` is not used.
- **FFmpeg builds.** 7.0.2, the repository pin (the bar's arm), and
  9.0.2 (BtbN `n9.0.2-17-g2a571b6068`), as information. The two arms are
  separate calls, each timing this decoder again beside its FFmpeg; the
  decoder's times in the two arms are reported next to each other.
- **One run.** Each stream is timed once per arm. A re-run is allowed only for
  a host failure (the VM or a process killed, not a slow stream or an
  unwelcome number), and the report says so.

## Bars

The rules for speed bars against FFmpeg apply; the bar is the one in `docs/design/h264.md` ("Speed
(step 6)"), P1, unchanged:

- **P1**: the median per-stream ratio (ours over FFmpeg 7.0.2, per-stream
  medians over the 5 rounds, above 1 is slower) is at most **1.15**, overall
  and in each resolution class, in **both** symmetric pairings.

Reported with each, overall and per class: the median with a 95% bootstrap
interval (2,000 resamples, `random.Random(20261021)`, percentiles
`[int(.025*N)]` and `[int(.975*N)-1]`), the share of streams where this
decoder is faster than FFmpeg (ratio below 1), and the per-stream table.
A claim that this decoder is faster than FFmpeg needs the upper bound of the
interval below 1.00; a median above 1.00 is reported as such
whatever the 1.15 bar says.

- **Output identity**: for every stream, this decoder's frames equal FFmpeg's
  (`ffmpeg -threads 1 -i es -frames:v F -fps_mode passthrough -f framemd5 -`):
  same frame count and the same MD5 per frame. Measured outside the timing
  rounds; each stream that differs is named with its first differing frame.
  The count of streams that match is reported.
- **Failures.** A stream on which any command exits non-zero is kept in the
  table as a failure and left out of the medians; the count and the names are
  reported. Nothing is dropped after the fact otherwise.

---

Results. Bars above unchanged. Binary: `main` at `ca5e4bc8`
(`cargo build --profile dist --example decode -p coreav-h264`, rustc 1.99.1).
Harness: `scripts/h264_parity.py` from `ca5e4bc8`, `--pin 2 --rounds 5`.
Machine: 8 vCPU Intel Xeon (the VM reports 2.50 GHz; AVX-512 present), 16 GB, Linux 6.18.49, a cloud
VM idle during the run; this is one machine, and ratios on another machine
differ. Ratio = ours over FFmpeg, above 1 is slower.

All 64 streams ran in both arms: no failure, no host failure, no re-run. The
extraction (`tools/extract.py`) wrote the elementary streams whose SHA-256
are in `es.json`; the elementary stream of 18 streams has fewer than `F`
access units (the whole stream, as the rules say).

**Bar P1 (median at most 1.15, overall and per class, both pairings): not
met, in every cell.** This decoder is slower than FFmpeg 7.0.2 on all 64
streams in both pairings; no stream is faster.

### Against FFmpeg 7.0.2 (the bar)

| Class | n | discard: median [95% interval] | faster | write: median [95% interval] | faster |
|---|---|---|---|---|---|
| overall | 64 | 3.359 [3.125, 3.506] | 0/64 | 3.095 [2.870, 3.284] | 0/64 |
| sd | 16 | 3.266 [3.155, 3.479] | 0/16 | 3.262 [3.057, 3.408] | 0/16 |
| 720p | 16 | 3.481 [2.976, 3.703] | 0/16 | 3.247 [2.861, 3.530] | 0/16 |
| 1080p | 16 | 3.537 [3.114, 3.760] | 0/16 | 3.246 [2.818, 3.451] | 0/16 |
| 4k | 16 | 2.704 [2.254, 3.567] | 0/16 | 2.370 [2.168, 2.870] | 0/16 |

### Against FFmpeg 9.0.2 (information)

| Class | n | discard: median [95% interval] | faster | write: median [95% interval] | faster |
|---|---|---|---|---|---|
| overall | 64 | 3.336 [3.075, 3.448] | 0/64 | 3.157 [2.898, 3.268] | 0/64 |
| sd | 16 | 3.336 [3.132, 3.472] | 0/16 | 3.252 [2.956, 3.522] | 0/16 |
| 720p | 16 | 3.434 [2.988, 3.703] | 0/16 | 3.262 [2.832, 3.560] | 0/16 |
| 1080p | 16 | 3.394 [3.017, 3.976] | 0/16 | 3.181 [2.862, 3.423] | 0/16 |
| 4k | 16 | 2.571 [2.238, 3.379] | 0/16 | 2.394 [1.973, 3.002] | 0/16 |

Per-stream ratios are in `reports/summary.json` (`per_stream`) and the raw
parity reports in `reports/<class>.<arm>.json`.

**Output identity: 64 of 64 streams** have the same frame count and the same
MD5 on every frame as FFmpeg 7.0.2 (`identity.json`, `tools/identity.py`),
measured before the timing rounds with the binary that was timed.

Consistency of the two arms: FFmpeg 9.0.2 over 7.0.2, median over 64
streams, 1.000 [0.990, 1.008] on `-f null` and 0.993 [0.982, 1.005] on `-f rawvideo`; this
decoder's time in the second arm over the first, 0.997 [0.987, 1.001] on discard and 0.992 [0.981, 1.004] on write. The two
FFmpeg builds are indistinguishable here, and the two arms measure this
decoder alike.

The draw is spent.
