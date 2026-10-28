# Bundle 3: one thread's inter prediction at 1080p and 4k, composition plan

Written before any timing of the bundle. The PMU profile at `ae609547` (`docs/measurements/h264-rounds/profile-ae609547-pmu/`)
puts the largest one-thread excess over FFmpeg at 1080p and 4k in inter prediction: the placement of a block and the
reference hints. Three changes were each shown correct (identical output on the training set, conformance, their
tests and fuzz targets) and each left out of `main` because its gain alone did not show in a run: the inter
placement was rejected at eight threads with no one-thread gating reading, and the other two had local intervals
spanning 1.000. Bundle 2 (`bundle2/plan.md`) is the precedent. Each item's own readings are listed with it.

## The items

| | item | code | crates touched | its own readings |
|---|---|---|---|---|
| P | the inter placement checked once a band, by macroblock and part | `rejected/h264-inter-place` (`b6b5de86`, `208c0a24`, `98e886ac`) | `coreav-h264`: `inter.rs`, `mb.rs`, `tier.rs`, `work.rs`, `fast/mod.rs`; `coreav-kernels-h264`: `lib.rs`, `predict.rs`, `simd/mod.rs`, `simd/x86/predict.rs`, `tests/equivalence.rs`, the `predict` fuzz target | gating at eight threads on `c4-standard-16` 0.9988 [0.9958, 1.0026] (rejected by IP-2); local one thread 0.9899, 4k 0.9779 (`inter-place/local-ab/`); instructions 0.9922, 32 of 32 fewer, 4k 0.9790 |
| R | prefetch hints straight from the first row once the last row is checked | `rejected/h264-prefetch-straight` (`2968bd3a`) | `coreav-kernels-h264`: `prefetch.rs`, `simd/mod.rs` | local 4k 0.9938 [0.9799, 1.0064], all 32 0.9989 (`prefetch-stalls/`); instructions 0.9930, 21 of 32 fewer, 4k 0.9743, sd 1.0002 |
| C | chroma deblocking on bytes, both edges and both planes of a pass in one 32-lane run | `rejected/h264-chroma-deblock-bytes` (`e4cabc07`, docs `12ae4073`) | `coreav-kernels-h264`: `lib.rs`, `simd/mod.rs`, `simd/tests.rs`, `simd/x86/chroma_mb.rs`, the `filter_chroma_mb` fuzz target | local one thread 0.9996 [0.9853, 1.0052], 720p 0.9888 (`chroma-bytes/README.md`); instructions 0.9962, 31 of 32 fewer |

The instruction readings are callgrind totals, one thread, discard, the class frame counts, each item alone over
`main` at `df389085` (`instructions-items-1t.txt`, `python3 -I ic_ratio.py instructions-items-1t.txt m-df38 b3-place
b3-pref b3-chroma`). Two candidates were left out before the composition: a band published with no reader waiting
(`instructions-wake-8t.txt`: 1.0000 at eight threads, no instruction cut, so nothing for a bundle to add), and the
apron kept (an eight-thread item, conflicting with bundle 2 and an earlier change, carried by the SIMD workstream).

## Overlaps, and how the bundle resolves them

Applied in the order P, R, C to `main` at `974f46c1`, all five code commits apply without a conflict.

- **P and R, `simd/mod.rs`.** P adds `Simd::predict_part_inlined`'s call into `x86::predict`; R changes `hint_read`'s
  argument to an address. Different items of the file; R's hints are issued by P's callers exactly as by `main`'s
  (`request` in `inter.rs` is not touched by P).
- **P and C, `lib.rs`.** P adds `Kernels::predict_part` and its types; C makes `EdgeParams` `repr(C)` with a
  compile-time assertion of its size. Different items.
- **The struct layouts.** P adds a field to `SliceDecoder` (the band's `Macroblocks`), and C changes `EdgeParams`'s
  representation. A struct-layout change has moved one-thread time by about 0.3% on its own in earlier rounds, which
  is why the one-thread bars below decide and are read on the bundle as built, not item by item.
- **Documents.** C's design section and evidence (`12ae4073`) are applied with it; its paragraph and row of the
  `unsafe` site list are taken onto `main`'s (which bundle 2 and an earlier change changed since); P's rows of that list (from its
  docs commit `8bc7aaae`) are applied to `main`'s rows.

## Checks before any timing

- Output: the bundle byte-identical to `main` `974f46c1` on the 32 training streams at 1 and 8 threads, with and
  without `fast` (`identity-fast.txt`, `identity-safe.txt`).
- Conformance: 97 of 97 at 1, 2 and 8 threads with detected and scalar kernels, fast and safe (`conformance.txt`).
- `cargo fmt`, `clippy -D warnings` and the tests of both crates; the fuzz targets `predict` and `filter_chroma_mb`,
  300 s each without a finding (`fuzz-*.txt`).
- Callgrind instructions of the bundle over `main`, per stream (information, `instructions-bundle-1t.txt`).

## Bars (final; committed with the composition, before the local timing)

Three arms, `base` (`main` at `974f46c1`), `bundle` and the null arm (a second copy of `base`), built alike (`cargo
build --locked --profile dist --example decode -p coreav-h264`, one toolchain, `RUSTFLAGS` unset), FFmpeg 9.0.2 as the
reference command, seeded order (`--shuffle-seed`), calls of at least 3 s (`--min-seconds 3`). Each bar is a median of
per-stream ratios, `bundle` over `base`, with its 95% bootstrap interval.

- one thread: `scripts/h264_parity.py` on the helper VM class (Intel Xeon @ 2.90 GHz, family 6, model 106), pinned
  (`--pin 2`), the host-health gate before and after each call, the harness's thresholds for that host;
- eight threads: `scripts/h264_parity_gcp.py` on `c4-standard-16`, one thread a core (Intel Xeon Platinum 8581C,
  family 6, model 207), its void rules (null deviation 2%, round spread 1.25), `--gate-threads 8`, ten rounds, a cap
  of at most 10 USD over the bundle's runs together.

| | measure | host | bar |
|---|---|---|---|
| B3-1 | one thread, discard, all 32 | helper VM | at most **0.990**, upper end below **1.000** |
| B3-2 | one thread, discard, the 16 streams at 1080p and 4k | helper VM | at most **0.985** |
| B3-3 | one thread, write, all 32 | helper VM | at most **1.000** |
| B3-4 | one thread, each class (sd, 720p, 1080p, 4k), discard and write | helper VM | at most **1.005** |
| B3-5 | eight threads, discard and write, all 32 | `c4-standard-16` | at most **1.005** each |
| B3-6 | eight threads, each class, discard and write | `c4-standard-16` | at most **1.010** |
| B3-7 | one thread, discard, the 64 streams of the held-out draw (`docs/measurements/h264-n2-heldout/list.json`), five rounds | helper VM | at most **1.000** |
| B3-8 | output | | as "Checks before any timing" |

Sizing:

- The target is one thread at 1080p and 4k, where all three items cut most (instructions at 4k: P 0.979, R 0.974,
  C 0.998). One thread's noise on the helper VM is about 0.6 to 0.7% either side, so B3-1 at 0.990 with its upper end
  below 1.000 is a gain bar that the items can meet only together (local readings P 0.9899, R about 0.999, C about
  1.000 over all 32), and B3-2 holds the bundle to the classes it is for.
- Eight threads are a guard, not a gain bar: P's own gating there read 0.9988 with the 4k class at 1.0062, and the
  bundle is not expected to gain at eight threads. B3-5 and B3-6 keep it from costing there, with margins of a few
  times that host's noise (about 0.1% for all 32, more by class).
- B3-7 checks the one-thread result on streams the items were not shaped on.

The bundle ships if it meets B3-1 to B3-8. Otherwise nothing from it merges. One run decides, and a re-run of the same
arms is allowed only for a host fault as the host's void rules define it.

The gating run is requested only if a local interleaved timing meets both. It runs on the implementer's VM (Intel
Xeon @ 2.50 GHz, family 6, model 85, 8 vCPUs), not the bar class: it screens, and the bars above decide.
- one thread pinned, six rounds over the 32 streams, discard: median at most 0.990 with the upper end of its interval
  below 1.000;
- eight threads, `--min-seconds 3`, five rounds, discard: median at most 1.005.

Otherwise the round ends there with its local result recorded.

## Result: rejected, by B3-2

One gating run (`gating/README.md`, `gating/analysis.txt`; `base` `974f46c1`, the bundle's `crates/` `72cf01fe`, null
arm `base`; no call void). One thread on the 2.90 GHz class, pinned, twenty rounds (held-out five); eight threads on
`c4-standard-16`, ten rounds.

| | measure | result | bar | |
|---|---|---|---|---|
| B3-1 | one thread, discard, all 32 | 0.9888 [0.9820, 0.9908], 29 of 32 faster | at most 0.990, upper end below 1.000 | met |
| B3-2 | one thread, discard, the 16 streams at 1080p and 4k | **0.9887** [0.9787, 0.9911], 15 of 16 faster | at most 0.985 | **missed** |
| B3-3 | one thread, write, all 32 | 0.9873 [0.9827, 0.9915] | at most 1.000 | met |
| B3-4 | one thread, each class, discard / write | sd 0.9921 / 0.9899, 720p 0.9853 / 0.9870, 1080p 0.9895 / 0.9896, 4k 0.9853 / 0.9775 | at most 1.005 | met |
| B3-5 | eight threads, all 32, discard / write | 0.9982 [0.9961, 1.0005] / 0.9988 [0.9977, 1.0012] | at most 1.005 each | met |
| B3-6 | eight threads, each class, discard / write | at most 1.0033 / 1.0056 (4k) | at most 1.010 | met |
| B3-7 | one thread, discard, 64 held-out | 0.9884 [0.9855, 0.9896], 60 of 64 faster | at most 1.000 | met |
| B3-8 | output | identical on 96 of 96 streams at 1 and 8 threads (gating), `checks.md` | | met |

The bundle ships only if it meets all eight, so nothing from it merges. The gain is real and general (all 32 0.9888,
held-out 0.9884, every class below 1.000) but was not concentrated at 1080p and 4k as the instruction counts
suggested (4k 0.9510 in instructions, 0.9853 in time); the 720p class gained as much as 4k. The code is kept on
`rejected/h264-bundle3`; the items stay on
their own `rejected/` branches.

Recorded in the gating evidence: the one-thread script's 10 h timeout ended the held-out run during its 1080p call
(no report, nothing read); the two remaining held-out calls ran with the same binaries and options after the host
gate's reference was moved by its level-shift rule, written down before the restart (`gating/1t/REBASE.md`). Neither
touches B3-2's calls. Against FFmpeg 9.0.2 (information, discard): one thread, training, `base` 1.031 and the bundle
1.009; held-out 1.015 and 1.003; eight threads 0.872 and 0.870. Spend: 3.17 USD on Compute Engine (bound).
