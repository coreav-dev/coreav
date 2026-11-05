# HEVC scalar in-loop filters

Status: milestone 3 implementation plan. The plan is the first commit of the same work unit as its implementation. Milestone 2 is merged in coreav-dev/coreav#393; its final G2 result checked 17 eligible pictures and all 17 defined picture hashes. This note extends the [decoder plan](hevc.md) and [scalar intra owner](hevc-intra.md), not a production decode backend.

## Outcome and limits

Reconstruct original independently decodable Main and Main10 intra pictures with their signalled deblocking and sample adaptive offset (SAO), for matching 8-bit or 10-bit 4:2:0 components. Complete vertical deblocking, horizontal deblocking and SAO before verifying decoded-picture-hash SEI or publishing samples. Preserve serial tile, WPP and dependent-segment semantics and all G1/G2 evidence. Never disable an official filter or compare intermediate samples with a filtered reference.

Use ITU-T H.265 V11 (January 2026), especially clauses 7.3.8.3, 7.4.9.3, 8.7 and 9.3, and original H.265.1 conformance data. The pinned PDF SHA-256 remains `78b093480e178a2b99d8cb7499934240f359c50c6d09568f8be651bc1151d4d2`. No other HEVC implementation source, source-derived notes or HM is read. FFmpeg/ffprobe 9.0.2 remain black-box binaries. No encoder recipe or generated test bitstream is added.

Inter prediction and its motion-dependent boundary strengths remain milestone 4. Production backend/CLI wiring, default-build changes, SIMD, unsafe code, threading, timing and encoder work are outside this milestone. The kernel dependency stays optional under `native-hevc`; default consumers remain unchanged.

## One picture owner

Keep `StreamCore` as the sole activation, inheritance, POC and header-DPB owner, and retain the single tile-layout owner. Extend retained SPS/PPS/slice syntax with PCM loop-filter suppression, tile and slice loop-filter permissions, effective beta/tC offsets and picture-level chroma QP offsets. Dependent segments inherit the effective independent header, not newly invented filter defaults.

`Frame` continues to own planes, cell metadata, contexts and publication. Add a private in-loop module, not a second picture decoder. Retain CU/PU/TU boundaries, effective QP and PCM/bypass protection in the existing cell map; retain per-CTU slice identity, filter permissions and SAO parameters. All selected CUs are intra, so eligible intra block boundaries have boundary strength two; do not invent inter strengths.

Parse each CTU's SAO before its coding tree. Add SAO merge/type contexts to the existing context set so independent slices, tile resets, dependent restoration and WPP snapshots use the same owner. Resolve left/up merge availability by the normative tile and slice rules. Retain all component offsets, band positions and edge classes with the required chroma sharing, offset signs and bit-depth scaling.

On completed reconstruction, perform one idempotent finalization transition. It runs before the first applicable suffix hash and also before publication when no hash is present. An incomplete picture cannot filter or publish. Filtering twice is forbidden by state, not by counting a second pass as harmless. A filter, hash, budget or callback failure keeps the existing terminal-session behavior and discards unpublished samples.

## Scalar kernels and sample availability

Add safe, allocation-free numerical kernels to `coreav-kernels-hevc`, with checked bit depth, QP/threshold inputs and complete fixed edge neighbourhoods. The bitstream crate derives normative beta/tC indices and chroma QP, determines availability and protected destinations, gathers samples and owns writes. The kernel crate does not read headers, environment variables or operating-system state.

Deblock only eligible coding/prediction/transform boundaries on the normative eight-luma-sample grid, in four-sample edge segments. Use reconstructed, unfiltered samples for vertical filtering and vertically filtered samples for horizontal filtering. Respect independent-slice and tile crossings, effective offsets, coded-picture edges, PCM loop-filter suppression and transquant bypass. Derive chroma filtering QP from the averaged luma QP and picture-level chroma offsets, not slice residual chroma offsets (8.7.2.5.5). Strong/normal luma and chroma equations, clipping, side protection and both depths receive equation-derived unit expectations.

SAO reads the completed deblocked picture, never a neighbour already changed by SAO. Use one fallibly allocated component source copy at a time, charged before reservation, and a separate destination plane already owned by `Frame`. Band classification uses the original component depth; edge classification requires both prescribed neighbours and honours slice/tile/picture availability. Preserve protected PCM/bypass destinations without treating them as missing source neighbours. Cropping is not a filter boundary; coded samples are filtered before visible-output packing.

The component copy was chosen over a rolling-row cache because it keeps edge-neighbour reads and arbitrary CTU boundaries explicit with one bounded buffer. A second frame owner or per-tile decoder was rejected because it would duplicate activation and boundary semantics. A later measured optimization may replace the copy without changing these contracts.

## Bounds and caller tools

Charge planes, the enlarged cell map, CTU metadata, context state, source-copy capacity and allocation overlap to the existing reconstruction-memory budget. Retain the picture-wide CABAC-bin limit. Filter loops have finite bounds derived from checked coded dimensions, CTU count and component count; no caller-controlled recursion or unchecked growth is added. Return the existing static `Limit`, `InvalidData` and `Unsupported` errors. No pixel buffer is allocated by header-only inspection.

Keep the existing selected-coded-index callback contract, full-width u16 planes and no-clobber/error cleanup. Extend the conformance example to bounded streaming NAL input rather than rejecting whole files above its former 256 MiB file cap. Enforce the existing per-NAL limit and total reconstruction budget independently. Compare packed native output row by row rather than making another whole-image Python allocation. These are conformance-tool changes, not production consumer wiring.

## Evidence rules

The completing driver derives every independently decodable supported intra target from the original full census, including filtered and unfiltered pictures. Keep original archive identities, duplicate accounting, coded indices, output mappings, all four existing synthetic recipes and the two exact SAODBLK exclusions. Missing references, incomplete native/oracle work, budget exhaustion, wrong order/packing or a mismatch fail; do not reduce a denominator to obtain a pass.

For output pictures, retain published references and the existing whole-output-MD5-anchored oracle rule. An oracle-derived per-picture reference is usable only after complete output matches the original published MD5 exactly. Keep source, coded/output index, layout, attachment, anchor and oracle identity visible. Verify every applicable defined coded-component SEI hash independently of visible-output hashes; reserved hashes never receive checked credit.

**Non-output references, decision of 2026-11-05.** OPFLAG_B coded intra pictures 39 and 73 remain in the denominator despite having no output index. Use their original decoded-picture-hash SEI, labelled exactly `coded-picture reference (SEI), not output`. Do not invent an output mapping or claim an oracle output-frame comparison for them. Independently hash the emitted coded components against their original SEI references, with original stream/coded-index provenance. Missing or mismatching defined SEI remains a failure.

**Serial coverage gap, decision of 2026-11-05.** Official metadata has no admitted 10-bit intra candidate for tiles or dependent segments. Record `unproved at 10-bit: tiles/dependent segments, no official candidate` in the plan, result and tracking issue. This labelled gap does not block milestone 3. Do not generate replacement bitstreams or add fixture-generation machinery. Add a later official or already-held effective case if one becomes available. This changes the earlier both-depth serial pixel completion requirement only for these absent cases, not their implementation or available-case correctness requirements.

Metadata signals remain candidate evidence, not pixel conformance. Report effective filter work and actual sample changes separately from enabled flags. Exercise all available original tile/WPP/dependent targets at their original depths; keep a missing effective case labelled rather than treating header inspection as a pass. No performance bar, held-out set, freeze, seal or extra registration process is introduced.

## Implementation and verification

1. Retain effective syntax and filtering metadata in the shared owners. Add specification-derived SAO context/bin and merge tests, inherited-header checks and bounded metadata-allocation failures.
2. Add both-depth deblocking and SAO numerical kernels with independent equation-derived edge/band expectations. Check QP and offset extremes, clipping, strong/normal decisions, unchanged protected sides, edge classes and coded-picture boundaries.
3. Integrate ordered picture filtering, finalization and terminal errors. Exercise PCM/bypass suppression, tile/slice permissions, dependent inheritance, WPP transitions and publication/hash ordering with small self-written unit inputs. Preserve unfiltered G2 behavior.
4. Extend the existing conformance path to every original G3 target and each of the 26 development files' first independent IRAP, using the pinned binary. Retain complete original G1/G2 accounting and negative controls. Run all media acquisition/decoding on Actions, not the personal device. Record actual bounded acquisition requests; no additional draw or VM is needed.
5. Require formatting, strict Clippy, affected unit/script tests, feature-off/default dependency isolation, affected fuzz-target builds, exact-head similarity against the reviewed main allowlist, one independent review and green final-head CI. Review fixes remain in this PR. Code is ready only when original sample/hash evidence agrees, not merely because unit tests pass.

Negative controls cover changed samples/components, wrong references/indices, missing non-output SEI, bad SAO/merge syntax, unavailable edge neighbours, truncated entropy, incomplete CTUs, duplicate filtering, exhausted budgets and failed output/callback cleanup. Linux child resource limits and credential-free media subprocesses remain mandatory. Preserve complete failed results instead of silently accepting fewer targets. Record any remaining effective-coverage gaps beside the final correctness result.
