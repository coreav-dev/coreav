# One band for small pictures at eight threads

Status: planned. These rules and bars are committed before any judged timing run. Refs coreav-dev/coreav#22.

## Candidate

`base` is `main` at `c981b760f88057d281e98df183f53f49091645dc`, including bundle 6. `k` differs only in the automatic picture layout. When the effective thread count is exactly eight, no explicit band height is set, and the coded picture height is below 32 macroblock rows, the picture uses one band. Non-reference pictures already use one band. Explicit band heights, other thread counts, pictures of 32 or more rows, and invented frame-number-gap references keep their existing rules.

The condition belongs in `Decoder::start_picture`, beside the existing non-reference condition. `Geometry::band_mbs_for` and both memory-budget paths remain unchanged. Their banded estimate is conservative for one band and stays valid if `limit_threads` subsequently lowers eight threads to seven. The alternative of changing the shared geometry default would lower that estimate and would need a separate treatment of thread-count transitions. No fields, types, signatures, public options or unsafe sites change.

The threshold keeps the current one-row-band bucket. It covers every SD stream in both registered sets, but excludes 576-line pictures and 720p. Widening it after a result is a different candidate. A short, very wide picture also qualifies because the rule uses coded height, not display area.

## Ceiling before implementation

On three training streams, the baseline's existing `--band-height 10000` override removes band lookup and apron-copy work without adding candidate code. Callgrind at eight threads, 90 output frames, the `dist` build with default features, on a Compute Engine c4-standard-8, gives the following totals. The override is only a ceiling probe and is not a timed arm. Valgrind exposes AVX2 rather than the host's AVX-512 tier. In-flight frames can add work beyond the output limit in both arms.

| stream | class | base instructions | one-band instructions | ratio |
|---|---|---|---|---|
| 41e324ebb708.0.sd.h264 | sd | 246681158 | 239850970 | 0.9723 |
| 5185721dda11.0.sd.h264 | sd | 127952435 | 122720683 | 0.9591 |
| 1d4b1775d78a.0.720p.h264 | 720p | 1883367761 | 1849749471 | 0.9821 |

The SD ceiling is 2.8% to 4.1% fewer instructions. This is enough to try a 1.5% timed SD bar, but says nothing about waiting. Publishing a reference only after its last row is filtered can serialize dependent pictures. The 720p probe is information only; the candidate leaves that class unchanged.

## Calls and host gates

Both decoders use `cargo build --locked --profile dist -p coreav-h264 --example decode`, with `RUSTFLAGS` unset. `base` is also the byte-identical null arm. FFmpeg is the assembly-enabled static 9.0.2 build pinned once in `scripts/oracle-ffmpeg.env`; verify it with `scripts/oracle_pin.py --verify-bin` before upload. The historical 7.0.2 extractor is used only to reproduce the registered elementary-stream hashes, never as a timing or decode oracle.

- `t8` uses the 32 training streams in `docs/measurements/h264-training/es.json`, ten rounds, through `scripts/h264_parity_gcp.py` on c4-standard-16, SMT off, eight physical cores, unrestricted turbo, Xeon Platinum 8581C.
- `h8` uses the 64 validation streams in `docs/measurements/h264-n2-heldout/es.json`, five rounds on the same host class. They are held out from this candidate's tuning, but are the previously reported set, not a fresh draw. Its reports use the runner's `t8` prefix in a separate directory.
- `g` uses the 32 training streams, twenty rounds, on the calibrated fixed-frequency one-thread class, c4-standard-8 with SMT off, `--one-thread-class --turbo all-core-max --pin 2`. The two reference streams and its 3.1 GHz checks follow bundle 6.

Frame limits stay sd 1800, 720p 900, 1080p 300 and 4k 150. Seed is 20261101. The existing runner owns randomized interleaving, the null arm, retries of unresolved measurements, reference drift, frequency gates and completeness. Eight-thread calls use `--min-seconds 3 --null-bar 0.02 --spread-bar 1.25`; one-thread calls use the class's existing 0.04 and 1.50 thresholds. No host gate is relaxed. Preserve every per-call report, including failures, and check exit status and nonempty output in identity runs.

## Bars fixed before timing

Ratios below are medians over per-stream ratios, not ratios of aggregate times. Relative bars use `scripts/h264_parity_bars.py`, 2000 bootstrap resamples with the registered seed, and complete reports. The FFmpeg bars read that analyzer's `vs_ffmpeg.k` medians from the same reports. Each FFmpeg interval is the same bootstrap applied to the report's per-stream `k:discard` or `k:write` ratios. Report the share below 1.000 beside each median and interval.

| bar | measure | threshold |
|---|---|---|
| SB-1 | `t8`, discard, sd, `k/base` | median at most 0.985 and 95% interval upper bound below 1.000 |
| SB-2 | `t8`, discard and write, overall and every class, `k/base` | median at most 1.003 |
| SB-3 | `t8` and `h8`, discard and write, overall and every class, `k/FFmpeg` | median at most 1.000 |
| SB-4 | `h8`, discard, sd, `k/base` | median at most 0.990 and 95% interval upper bound below 1.000 |
| SB-5 | `h8`, discard and write, overall and every class, `k/base` | median at most 1.003 |
| SB-6 | `g`, discard and write, overall; every class, `k/base` | overall median at most 1.003 and interval upper bound below 1.005; each class median at most 1.005 |
| SB-7 | output and checks | every identity call identical to `base`; conformance, bench-guard and similarity guard pass |

The targeted gain is on SD, so the unaffected classes are guards, not required gains. The relative 0.3% and 0.5% tolerances account for the sub-percent placement and A/A spread measured in the earlier placement-control round. They do not permit a slowdown against FFmpeg at eight threads. One-thread code does not take the new branch; SB-6 is a no-regression guard at those tolerances. One-thread FFmpeg ratios are reported, not gated.

Identity covers all 32 training streams at one and eight threads with and without `fast`, plus all 64 validation streams at one and eight threads with `fast`, 256 calls. Use the existing bundle 6 identity job without changing the decoder example. A logged decode or flush error is also a failure, even if its process exits successfully. Conformance and bench-guard run in CI. Similarity evidence names the checked public commit and uses the current main allowlist.

Run the training call first. If SB-1, SB-2 or its SB-3 checks miss, stop without starting validation or the one-thread guard. Otherwise run validation and the guard. A missed bar is final for these arms. Repeat a call only after a recorded host fault, never because a result is close. A failed candidate stays out of production; retain its evidence and exact diff. No faster-than-FFmpeg claim is made without a held-out interval upper bound below 1.000.
