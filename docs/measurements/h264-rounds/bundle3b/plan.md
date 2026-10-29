# Bundle 3b: bundle 3 with 4-wide luma blocks in vector registers, composition plan

Written before any timing of the bundle. Bundle 3 (`bundle3/plan.md`, `docs/design/h264.md`, "Bundle 3: one thread's
inter prediction at 1080p and 4k") was rejected only by its sub-class bar B3-2 (the 16 streams at 1080p and 4k,
0.9887 against 0.985); its whole-stream readings were one thread 0.9888 [0.9820, 0.9908] over the 32 training
streams, 0.9884 over the 64 held-out ones, and eight threads 0.9982. Those arms are final. Bundle 3b is a new round:
bundle 3's three items rebased on today's `main`, and one new item, N, that no earlier round measured. The bars are
whole-stream gain bars with per-class guards, as bundle 3's result taught: an instruction cut at one resolution
shows in time spread over the classes, so a gain bar on a subset of classes decides on noise.

## The items

| | item | code | crates touched | its own readings |
|---|---|---|---|---|
| P | the inter placement checked once a band, by macroblock and part | bundle 3's commits, cherry-picked unchanged | `coreav-h264`: `inter.rs`, `mb.rs`, `tier.rs`, `work.rs`, `fast/mod.rs`; `coreav-kernels-h264`: `lib.rs`, `predict.rs`, `simd/mod.rs`, `simd/x86/predict.rs`, `tests/equivalence.rs`, the `predict` fuzz target | `bundle3/plan.md` |
| R | prefetch hints straight from the first row once the last row is checked | as P | `coreav-kernels-h264`: `prefetch.rs`, `simd/mod.rs` | `bundle3/plan.md` |
| C | chroma deblocking on bytes, both edges and both planes of a pass in one 32-lane run | as P | `coreav-kernels-h264`: `lib.rs`, `simd/mod.rs`, `simd/tests.rs`, `simd/x86/chroma_mb.rs`, the `filter_chroma_mb` fuzz target | `bundle3/plan.md` |
| N | 4-wide luma blocks through the 8-wide AVX2 kernel on copies | new, this branch | `coreav-kernels-h264`: `simd/x86.rs`, `simd/mod.rs`, `simd/neon.rs`, `simd/portable.rs`, `simd/tests.rs` | below |

**Why N.** A callgrind profile of P, R and C on today's `main` (one thread, discard, the class frame counts, the 32
training streams) found the reference kernel's luma interpolation, `Scalar::luma_mc`, taking 1.9% to 4.4% of the
instructions of four streams (`a81de82106be` 4.43%, `36501d448eaa` 3.53%, `e624cbe0d2be` 3.39%, `c7ecda16e4a7`
1.88%; none on the other 28): 4-wide blocks at a fractional position, which `Simd::prefers_luma_mc` sent to the
reference, at about 3,800 instructions a call. The x86 kernels had no 4-wide path. N copies the block's 9-wide window
into rows of 16 bytes and the block into rows of 8, runs the 8-wide interpolation and its finish (written, averaged or
weighted), and copies the 4 columns back; output column `c` reads only window columns `c` to `c + 5`, so the
columns past the window change nothing kept. Instructions of N over P, R and C on those four streams: 0.9671,
0.9719, 0.9748, 0.9846. A directional timing of N alone over P, R and C on those four streams on the implementer's
VM (ten rounds, a null copy of the base binary as control, before these bars): 0.9977, 0.9747, 0.9901, 0.9878, the
null 0.996 to 1.005. That reading does not enter any bar.

## Overlaps, and how the bundle resolves them

Applied in the order P, R, C, N to `main` at `1e7dd54b`. P, R and C cherry-pick without a conflict in code. Their
descriptions of `unsafe` sites are taken onto `docs/unsafe-sites.md`: the paragraphs on loads and prefetch
hints and the rows of `simd/mod.rs`, `simd/x86/chroma_mb.rs` and `simd/x86/predict.rs` as bundle
3 wrote them. N adds no `unsafe` site of a new kind: one call of `luma_mc_ptrs` on two local arrays, inside
`simd/x86.rs`, whose row covers the luma interpolation; N and P meet only in that P's 4-wide blocks reach N through
`Auto::luma_mc`, as they reached the reference before.

Two open changes touched the same files: the CAVLC slice loop (`cavlc.rs`, `mb.rs`, `inter.rs`,
`decoder.rs`) and bundle 4 (`inter.rs`, `mb.rs`, `prefetch.rs`, `simd/mod.rs`). Neither is in this bundle;
whichever merges first, the other rebases, and a rebase that changes code here repeats the output checks.

## Checks before any timing

- Output: the bundle byte-identical to `main` `1e7dd54b` on the 32 training streams at 1 and 8 threads, with and
  without `fast` (`identity-fast.txt`, `identity-safe.txt`).
- Conformance: 97 of 97 at 1, 2 and 8 threads with detected and scalar kernels, fast and safe (`conformance.txt`).
- `cargo fmt`, `clippy -D warnings` and the tests of both crates, `coreav-h264`'s also without `fast`; N's test
  `a_4_wide_luma_block_matches_the_reference` (every height 1 to 16 and position, written, averaged and weighted,
  windows whose last row ends the slice); the fuzz targets `luma_mc`, `weighted_mc`, `predict` and
  `filter_chroma_mb`, 300 s each without a finding (`fuzz-*.txt`).
- Callgrind instructions of the bundle over `main`, per stream (information, `instructions-bundle-1t.txt`).

## Bars (final; committed with the composition, before the local timing)

Three arms, `base` (`main` at `1e7dd54b`), `bundle` and the null arm (a second copy of `base`), built alike (`cargo
build --locked --profile dist --example decode -p coreav-h264`, one toolchain, `RUSTFLAGS` unset), FFmpeg 9.0.2 as
the reference command, seeded order, calls of at least 3 s (`--min-seconds 3`). Each bar is a median of per-stream
ratios, `bundle` over `base`, with its 95% bootstrap interval over streams.

- one thread: `scripts/h264_parity.py` on the helper VM class (Intel Xeon @ 2.90 GHz, family 6, model 106), pinned
  (`--pin 2`), the host-health gate before and after each call, the harness's thresholds for that host;
- eight threads: `scripts/h264_parity_gcp.py` on `c4-standard-16`, one thread a core, its void rules, ten rounds.

| | measure | host | bar |
|---|---|---|---|
| B3b-1 | one thread, discard, all 32, twenty rounds | helper VM | at most **0.990**, upper end below **1.000** |
| B3b-2 | one thread, discard, the 64 streams of the held-out draw (`docs/measurements/h264-n2-heldout/list.json`), five rounds | helper VM | at most **0.995**, upper end below **1.000** |
| B3b-3 | one thread, write, all 32 | helper VM | at most **1.000** |
| B3b-4 | one thread, each class (sd, 720p, 1080p, 4k), discard and write | helper VM | at most **1.005** (guard) |
| B3b-5 | eight threads, discard and write, all 32 | `c4-standard-16` | at most **1.005** each (guard) |
| B3b-6 | eight threads, each class, discard and write | `c4-standard-16` | at most **1.010** (guard) |
| B3b-7 | output | | as "Checks before any timing" |

Sizing:

- B3b-1 and B3b-2 are the gain bars, both over whole sets of streams. B3b-1 is bundle 3's B3-1. B3b-2 is set
  tighter than B3-7's 1.000: a bundle that gains on the training set should gain on streams it was not shaped on,
  and 0.995 still leaves room for the held-out set's different mix of classes.
- B3b-3 to B3b-6 are guards: no class may lose more than about one host noise (one thread about 0.5%, eight threads
  1% by class, where eight streams make an interval wide). They are not gain bars.
- N's own gain is concentrated on the streams with 4-wide partitions (4 of the 32 training streams), so it adds
  little to a median over 32; it is in the bundle as new code that removes a measured cost, not to carry a bar.

The bundle ships if it meets B3b-1 to B3b-7. Otherwise nothing from it merges. One run decides, and a re-run of the
same arms is allowed only for a host fault as the host's void rules define it.

The gating run is requested only if a local interleaved timing meets both of these. It runs on the implementer's VM
(Intel Xeon @ 2.50 GHz, family 6, model 85, 8 vCPUs), not the bar class: it screens, and the bars above decide.
- one thread pinned, six rounds over the 32 streams, discard: median at most 0.990 with the upper end of its
  interval below 1.000;
- eight threads, `--min-seconds 3`, five rounds, discard: median at most 1.005.

Otherwise the round ends there with its local result recorded.
