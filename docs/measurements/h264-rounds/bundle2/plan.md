# Bundle 2: ten small changes together, composition plan

Written before any build or timing of the bundle. Each change below was shown correct (identical output on the
training set, conformance, its tests) and was left out alone because its gain was below what a run resolves, or, for
round 23's CAVLC level reading, because one run missed on a stream its code does not touch. Round 25 ("Round 25: five
small changes together") is the precedent. A bundle draws only on parts with a measured effect of their own: each
item's own reading is listed with it.

## The items

| | item | code | crates touched | its own reading |
|---|---|---|---|---|
| A | CAVLC level reading (round 23's design) | written anew on `main`'s `cavlc::block_body` | `coreav-h264`: `cavlc.rs` | six CAVLC streams at one thread 0.9797 (round 23 gating), 0.9795 (round 25 gating), 0.9865 (round 23 local) |
| B | chroma intra prediction from one view of both planes | `rejected/h264-intra-chroma` `b9824c9` | `coreav-kernels-h264`: `lib.rs`, `simd/mod.rs`, `simd/tests.rs`, `simd/x86.rs`, `views.rs`, a fuzz target | local 1T 0.9986 [0.9969, 1.0030]; 720p 0.9961; the kernel's instructions 322 to 204 a call |
| C | coefficient scalings from per-QP tables | `rejected/h264-coeff-tables` `d6e962f7` | `coreav-h264`: `recon.rs`, `mb.rs`, `work.rs` | local 1T 0.9981 [0.9909, 1.0019]; instructions 0.9911, 32 of 32 fewer |
| D | band views kept for the slice (8T) | `rejected/h264-band-views` `bc97fa55` | `coreav-h264`: `inter.rs`, `mb.rs`, `work.rs`, `tests/threads.rs` | gating 8T 0.9915 [0.9823, 1.0038], 1T 1.0012 |
| E | slack rows at the picture's edges in every layout (8T) | `rejected/h264-band-slack` `4b512a85` | `coreav-h264`: `picture.rs`, `decoder.rs`, `inter.rs`, `mb.rs`, `work.rs`, `deblock.rs` | local 8T 0.9863 [0.9724, 0.9994] (`c27c8be5`), 1T 1.0032 before `4b512a85` halved its one-thread cost |
| F | deblocking thresholds from a table by average QP | `58bb8b0` | `coreav-h264`: `deblock.rs`, `mb.rs`, `work.rs` | local 1T 0.9984 [0.9930, 1.0055] |
| G | bands of 4 rows for pictures of 32 to 63 macroblock rows (8T) | `perf/h264-bundle2-g` `e4b2ff8d` | `coreav-h264`: `picture.rs` | band-height sweep, the 7 training streams of that height: 0.968 of the 8T wall time (median; exploratory, read after the run, `band-height/plan-sweep.md`) |
| H | RBSP emulation-prevention removal by runs | `rejected/h264-rbsp-runs` `58ef0ccb` | `coreav-h264`: `nal.rs` | instructions 0.9929 (32 of 32 fewer); local 1T 0.9945 [0.9902, 0.9990], 23 of 32 faster (a 2.50 GHz VM, not the bar class; `rbsp-runs/`) |
| I | a chroma apron two rows taller (8T) | `perf/h264-bundle2-i` `4776091e` | `coreav-h264`: `picture.rs`, `decoder.rs` (a test) | callgrind at 8T over `main` `70a4cec4`: 0.9907, 0.9655, 0.9896 on the three streams with the most declined chroma windows, 1.0072 on an sd stream with none (`chroma-apron/`; its probe was void) |
| J | the CAVLC block parse from compile-time code tables | `rejected/h264-cavlc-block` `97514aa6` (built on A) | `coreav-h264`: `cavlc.rs` | local 1T over A, the six CAVLC streams 0.9844 [0.9734, 1.0008], 5 of 6 faster; all 32 0.9971 [0.9937, 1.0007] (`cavlc-block-probe/`) |

A is written fresh from round 23's design text and clause 9.2.2.1, not taken from `rejected/h264-cavlc-levels` or
`rejected/h264-small-bundle`. Below a prefix of 14, level_prefix, its terminating one and the suffixLength bits of
level_suffix are read as one field and consumed by one skip. levelCode becomes a level without a branch, and a
prefix of 14 or more goes out of line to the clause's three cases. Its tests compare against the clause-by-clause
decoder on random blocks and on blocks whose prefixes run from 0 to 40 zeros, with fixed vectors at prefixes 14, 15,
16 and 32.

G was added after the band-height sweep (`docs/measurements/h264-rounds/band-height/`), before the composition is
final. Its rule was read from the sweep's own data, so the bundle's eight-thread bars include the held-out draw,
which the sweep did not see (B2-7). G changes only `Geometry::band_mbs_for`; no other item touches it, and the
samples do not depend on the band height.

H (`rbsp()` in `nal.rs`, the bytes between emulation-prevention bytes copied as runs) touches a file no other item
touches; it was offered to the bundle by the kernels workstream before the composition is final.

I (`TOP_CHROMA` from 9 to 11) was added after its probe ended void (`chroma-apron/plan-probe.md`). It and G change
different items of `picture.rs`; with E, a band's chroma origin and slack follow `chroma_first(k)`, which I's
constant moves for every band but the first, so E's identity checks are run again on the pair.

J (the CAVLC block parse, `docs/design/h264.md`) was added before the composition is final. It is built on A
(`perf/h264-bundle2-a`) and changes only `cavlc.rs`'s block parse; it applies on A, B and C without a conflict. Its
gain is on the six CAVLC streams, so it shares B2-3 with A.

## Overlaps, and how the bundle resolves them

Applied one after another to `main` at `1534db3`, B, C and D apply cleanly. E conflicts with D in
`reference_band`, and F conflicts with C in `work.rs`'s `Spare`; both conflicts are textual. The meaning of each:

- **A and C, the CAVLC level loop.** A changes how a level is read, and C how the level is scaled as it is stored
  (`CoeffScale::apply`, now a constant round and shift). Both are kept; the stored value is `apply` of A's level.
- **D and E, `reference_band`.** D keeps a band's `Planes` view for the slice, and E makes a band's origin depend on
  the band (`luma_origin(k)`). A kept view is built once with its own band's origin. That is correct, because the
  view is keyed by reference and band. D's test `many_references_give_the_pictures_one_thread_gives` and E's
  identity checks cover the pair.
- **E and an earlier change.** E's one-thread cost came from the deblocking filter computing a band-dependent origin per
  macroblock. An earlier change (deblocking setup) adds one base offset per macroblock row. E is applied on top of it, taking
  `luma_origin(k)` and `chroma_origin(k)` once per row, which removes that cost.
- **F and changes A and B.** Change A changed only the kernels crate. Change B changes the form of `Thresholds` (alpha and beta
  as bytes, the t'C0 row by bS) but still derives them per edge with `Thresholds::new(qp_p, qp_q, offsets)`. F
  replaces that derivation with a look-up by average QP, so neither covers it. F is rewritten on an earlier change's `Thresholds`
  form, and C and F each keep their own box in `Spare`. F is dropped if, after an earlier change merges, `main`'s driver no longer
  derives thresholds per edge, or if F on that `main` does not lower the median callgrind instruction count over the
  32 streams (one thread, the callgrind class counts).
- **B and change A, `views.rs`.** B builds one checked view of both chroma planes for intra prediction, and change A builds
  both chroma windows of the deblocking filter from one range computation (`ChromaMbMut::pair`). They are separate
  types for separate kernels. B is applied on change A's `views.rs`, and shares change A's range computation if that is
  exported. B changes `unsafe` code in `simd/x86.rs`; the bundle lists the changed sites in
  `docs/unsafe-sites.md`.
- **C, D and E with change A.** Change A (the MB layer's stored state) rewrites much of `mb.rs`, `inter.rs`, `picture.rs`
  and `deblock.rs`. C, D and E change other fields and functions of those files: C the scalings and their use in
  `parse_residual_blocks`, D `reference_band` and a field of `SliceDecoder`, E `Geometry` and the origins. They are
  applied on an earlier change's code with no change of meaning, and identity is checked again after the rebase.

## Order and base

The bundle is built only after earlier changes are decided, on `main` as it then is (`base`). The items are applied in
the order A, B, C, D, E, F, G, H, I, each as its change to `crates/` and `docs/unsafe-sites.md`. An item that fails its own
tests or identity after the rebase is dropped before any timing, and the drop is recorded. The composition is final
when the bars below are committed. Nothing is added or removed after a measurement, and the bundle is measured once.

## The composition (final)

`base` is `main` at `bb7255cd` (earlier changes merged). The items were applied in the order A to J on
branch `perf/h264-bundle2`, each as its change to `crates/` and `docs/unsafe-sites.md`; none was dropped.

- A, D, G, H and J applied without a conflict. B, C, E, F and I had textual conflicts with an earlier change or with each other,
  resolved as "Overlaps" says: B's view (`ChromaIntraMut`, checked through `IntraRows::pair`, one length for both
  planes) is kept beside an earlier change's `ChromaMbMut::pair`, a separate type for a separate kernel; C's `coeff_tables` and
  `main`'s ring both stay in `Spare`; E's `luma_origin(k)` and `chroma_origin(k)` are taken once a row in an earlier change's
  per-row offsets; I's test takes both E's slack and the taller apron.
- F was rewritten on an earlier change's `Thresholds` (alpha and beta as bytes, t'C0 by bS): `Thresholds::new` stays for the tests
  and the table is built by `Thresholds::of_average`; the luma edges, the cold per-edge path and the chroma edges take
  `ThresholdTable::get`. `main`'s driver still derived the thresholds per edge, and F on A to E lowers the callgrind
  instructions on all 32 streams (median 0.9945, 0.9901 to 0.9989; `instructions-item-f.txt`), so F stays.
- B changes the `unsafe` of `simd/x86.rs` (the paired chroma intra view, `column8`); its row of
  `docs/unsafe-sites.md` names them.

## Checks before any timing

- Output: the bundle byte-identical to `base` on the 32 training streams at 1 and 8 threads, with and without
  `fast`.
- Conformance: 97 of 97 at 1, 2 and 8 threads, with scalar kernels and in safe mode.
- The tests of every item, A's new ones included; B's fuzz target (`chroma_mb`) builds.
- Callgrind instructions of the bundle over `base` per stream (information).

## Bars (final; committed with the composition, before the local timing)

Three arms, `base`, `bundle` and the null arm (a second copy of `base`), built alike (`cargo build --locked
--profile dist --example decode -p coreav-h264`, one toolchain, `RUSTFLAGS` unset), with FFmpeg 9.0.2 as the
reference command, seeded order (`--shuffle-seed`), calls of at least 3 s (`--min-seconds 3`). Each bar is a median
of per-stream ratios, `bundle` over `base`, with its 95% bootstrap interval. Two hosts, as the design note's
"Where the eight-thread arms run" assigns them:

- one thread: `scripts/h264_parity.py` on the helper VM class (Intel Xeon @ 2.90 GHz, family 6, model 106), pinned
  (`--pin 2`), the host-health gate before and after each call, the harness's thresholds for that host;
- eight threads: `scripts/h264_parity_gcp.py` on `c4-standard-16` with one thread a core (Intel Xeon Platinum 8581C,
  family 6, model 207), its bars (null deviation 2%, round spread 1.25) and void rules, `--gate-threads 8`, a cap of
  at most 10 USD over the bundle's runs together.

| | measure | host | bar |
|---|---|---|---|
| B2-1 | one thread, discard, all 32 | helper VM | at most **0.995**, upper end below **1.000** |
| B2-2 | eight threads, discard, all 32 | `c4-standard-16` | at most **0.990**, upper end below **1.000** |
| B2-3 | one thread, discard, the six CAVLC streams | helper VM | at most **0.985** |
| B2-4 | write, all 32, one thread and eight threads | each its own | at most **1.000** each |
| B2-5 | each class (sd, 720p, 1080p, 4k), discard and write | each its own | at most **1.005** |
| B2-6 | output | | as "Checks before any timing" |
| B2-7 | eight threads, discard, the 64 streams of the held-out draw (`docs/measurements/h264-n2-heldout/list.json`), five rounds | `c4-standard-16` | at most **1.000** |

Sizing against the noise of each host (the design note's calibration: the all-32 median of a binary against
itself; method section of an earlier change for one thread):

- One thread, about 0.6 to 0.7% either side on the helper VM: B2-1's 0.995 with its upper end below 1.000 is a gain
  bar the expected gain (C 0.998, B 0.9986, F 0.9984, H 0.9945, A on the six CAVLC streams 0.98) can meet only
  together; B2-3 is at the six streams' own reading.
- Eight threads, about 0.1% either side on `c4-standard-16` ([0.9997, 1.0013] discard, [0.9989, 1.0013] write):
  B2-2 stays a gain bar, because the expected gain (D 0.9915 at gating, E 0.986 local, G 0.968 on the streams it
  touches, I 0.966 to 0.991 in instructions on the streams it touches) is several times that noise. The eight-thread
  readings of D, E, G and I were taken on the helper VM, whose SMT and noise differ; the bars, not those readings,
  decide on the new host. B2-5 at eight threads moves from the draft's 1.010 to 1.005, the one-thread value: the
  draft's margin was for the helper VM's 1.5%.
- B2-7 runs five rounds rather than ten to keep both eight-thread runs under the cap (the calibration's 32 streams,
  four commands, ten rounds took 2.2 hours, 2.66 USD by the cap's bound).

The bundle ships if it meets B2-1 to B2-7. Otherwise nothing from it merges. One run decides, and a re-run of the
same arms is allowed only for a host fault as the host's void rules define it.

The gating run is requested only if a local interleaved timing on the helper VM meets both of these:
- one thread pinned, six rounds over the 32 streams, CPU time: median at most 0.995 with the upper end of its
  interval below 1.000;
- eight threads, `--min-seconds 3`, five rounds, discard: median at most 1.000. This is a guard against a loss, not
  a gain bar: the helper VM's eight-thread noise (about 1.5%) is larger than the gain the bar would test.

Otherwise the round ends there with its local result recorded.

## Result: passed

B2-1 to B2-7 met in one gating run (`gating/README.md`, `gating/analysis-final.txt`; `base` `bb7255cd`, `k`
`c5557ada`, no call void): one thread discard 0.9818 [0.9770, 0.9853] (32 of 32 faster), eight threads 0.9690
[0.9632, 0.9747], six CAVLC streams 0.9484, held-out 64 at eight threads 0.9663. The bundle merges as one change;
`docs/design/h264.md`, "Bundle 2: ten small changes together", records it and the checks after the rebase on `main`.
