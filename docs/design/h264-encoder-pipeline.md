# H.264 encoder pipeline restructuring

## Scope and starting point

This note plans a restructuring of the opt-in H.264 encoder for speed. It starts from the current main encoder, whose bounded search, quantizer-derived rate weight, quiet skip and SIMD kernels are described in [h264-encoder-search.md](h264-encoder-search.md), [h264-encoder-rate-weight.md](h264-encoder-rate-weight.md), [h264-encoder-simd.md](h264-encoder-simd.md) and [h264-encoder-speed.md](h264-encoder-speed.md). The plan was written from a stage-level profile of that encoder (instruction counts with stage boundaries kept out of line, sampled profiles and per-macroblock decision counters) on the existing inputs A, B and C at QP 28 and 22, one thread. Its inputs are the specification, published literature and that profile. No other implementation's source is used.

## Where the current pipeline repeats work

- Most P macroblocks of low-motion input end as skip, many through the quiet-skip test, yet each still pays a complete skip reconstruction, an exact syntax count with squared error, twenty-four forward transforms for the zero test, a tile fetch with clamped reads and several large candidate copies.
- Each IDR macroblock evaluates every I4x4 mode of every block completely and builds up to twenty complete macroblock candidates.
- Motion search is already predictor-seeded and bounded, but it always searches the 16x16 layout and all four 8x8 partitions on non-quiet macroblocks, and every fractional probe interpolates its window from scratch.
- Exact evaluations use a scalar transform, quantization and reconstruction path that uses 64-bit arithmetic, checked results per call and separate residual extraction.
- The P-picture intra proposal is built and counted exactly on every non-quiet macroblock, although intra is rarely selected.
- Deblocking already skips edges whose boundary strengths are all zero, and filters the remaining edges with per-sample checked access.
- The test driver also decodes its own output to verify it; encoder throughput figures exclude that decode.

These inputs are low-motion. Verification therefore adds one existing real-motion input, D, prepared the same way, so search and evaluation costs are also measured where they dominate.

## Target pipeline

Each P macroblock passes a cost ladder: a cheap exact test settles most skips without transforms; a predictor-seeded, bounded integer search proposes motion; fractional refinement reads precomputed half-sample planes; candidates are ranked by SAD, then SATD plus known syntax lengths; only the one or two finalists are reconstructed and counted exactly. IDR macroblocks use the same ladder for intra modes. Picture-level work (padded reference planes, half-sample planes, deblocking) is done once per picture in row order, which prepares row-parallel encoding.

## Steps

Each step is one pull request with one outcome, in this order. Before a step is built, its ceiling is counted in a throwaway build that deletes the work it removes; a step whose ceiling is negligible is dropped. Byte-identical steps must reproduce the current main stream and reconstruction in every case of both identity sets (`scripts/h264_g1_simd_identity.py` and the 57-case set), in dispatched and scalar builds. Output-changing steps report PSNR BD-rate against current main over QP 22, 28, 34 and 40 on A, B, C and D (PCHIP on log rate, common interval, no extrapolation), regenerate any byte fixture they change in the same pull request, and decode with the pinned FFmpeg to the encoder reconstruction. Throughput is an encoder-only reference figure from one GitHub-hosted job that runs current main and the candidate alternately, with instruction counts beside it.

1. **IDR cost ladder (output change).** For each I4x4 block, rank the legal modes by SATD plus mode bits and keep the exact trials of the best two that complete; a trial whose levels leave the coefficient range is not a finalist. Rank the I16x16 and chroma modes by SATD and complete only the best two of each, so an IDR macroblock normally builds at most six complete candidates. When none of them passes macroblock admission, the macroblock falls back to the complete search, which can build more, so the shortlist never refuses a macroblock the complete search accepts. P pictures keep the complete fallback.
2. **P partition gating (output change).** Search the four 8x8 partitions only when the 16x16 result leaves a SATD above a QP-scaled bound.
3. **P intra gating (output change, revised).** Comparing the best I16x16 SATD with the inter SATD cost proved unsafe: the few P macroblocks that intra wins are mostly I4x4 decisions, which an I16x16-only estimate cannot see, so the gate dropped them and cost rate. The revised step gates on an I4x4-aware estimate (for example SATD from neighbouring source samples, without reconstruction), or keeps the cheap proposal and skips only the exact build that follows it. The gate applies only once an inter candidate has been admitted; without one, the complete intra search runs as now, so admission never depends on SATD. It is measured again before it is built.
4. **Integer search early termination (dropped).** Stopping the integer refinement once the best integer cost falls below a QP-scaled bound saved little time, because integer probes are SAD-only and cheap next to fractional probes and exact evaluations, and bounds high enough to matter cost rate on real motion. Fractional-probe cost is addressed by step 8 instead.
5. **Narrow transform path (byte-identical).** Forward transform, quantization, scaling and inverse transform on 32-bit fixed arrays, with the residual taken in the same pass and range checks at the block boundary instead of per call.
6. **Edge-extended reference planes (byte-identical).** Pad each reference picture once so motion compensation and SAD read rows without clamping, and keep candidate samples in reusable per-macroblock scratch instead of moved structures.
7. **Exact early quiet test (byte-identical).** Every 4x4 forward-core coefficient is bounded by its row and column gain times the block's sum of absolute residuals, and the chroma DC terms come exactly from signed block sums. When that bound, passed through the quantizer's own zero test, cannot reach a nonzero level, the block is quiet without transforming it; otherwise the transform runs as now. A test proves the bound against the transform and zero test at every QP.
8. **Half-sample planes (byte-identical).** Compute the three half-sample planes of each reference picture once with the normative six-tap filter, and derive quarter samples by the normative averages, so fractional probes and inter reconstruction read the planes instead of interpolating per call. Its value is judged on D, because low-motion inputs search little.
9. **Deblocking fast path (byte-identical).** Filter the rows of edges with nonzero boundary strength through row slices checked once per edge instead of per-sample checked access.

## Roadmap to x264 parity

The maintainer set the target as x264 medium at the same thread count, on both throughput and PSNR BD-rate, judged first at the settings the [efficiency reference](h264-encoder-efficiency-reference.json) already matches, with x264 medium's defaults as a later phase. The matched settings are Constrained Baseline, CAVLC, one reference picture, no B pictures, fixed QP and PSNR tuning. Steps 1 to 9 do not close either gap. The candidates below are ranked by the stage attribution behind this plan and the measured results of steps 1 to 4. An expected gain names the stage it removes work from or the loss it targets; gains on the same stage do not add. Each item is measured like the steps. An output-changing speed item uses the lowest setting that keeps every measured input within +1% PSNR BD-rate of current main, the bound already applied to steps 1 to 4.

The stage attribution suggests, without establishing a majority for any one stage, that IDR coding, the quiet-skip test and the P intra proposal are among the largest costs on low-motion inputs, and that motion search, with fractional refinement a large part of it, leads on real-motion inputs. The ranking treats this as a working hypothesis that each item's ceiling count tests first. Leaf arithmetic (quantization, inverse scaling, transforms, residual extraction and sample copies) is spread across every stage. For compression, the encoder quantizes to the nearest level, eliminates no sparse coefficients and makes no rate-distortion choice between levels. These are standard techniques in the published literature and the largest remaining differences inside the matched settings.

*Obvious* items follow decisions already made and start when their dependencies merge. *Decided* items record a maintainer decision made for this roadmap.

### Speed

| Rank | Item | Output | Depends on | Expected gain | Start |
|---|---|---|---|---|---|
| 1 | Vector six-tap filter and quarter-sample averages over the half-sample planes | Identical | Step 8 | Largest on real motion: fractional refinement is the biggest part of motion search, and the plane pass becomes one streaming filter per picture | Obvious, under the [encoder kernel policy](encoder-policy.md#0044-simd-kernels-for-the-opt-in-h264-encoder) |
| 2 | Step 10, P finalist shortlist | Changes | Steps 2 and 6 | Large on every input: complete P inter evaluation is the second-largest P stage | Obvious |
| 3 | Revised step 3, I4x4-aware P intra gate | Changes | None | Large on low motion, where the P intra proposal costs about as much as motion search | Obvious; measured before it is built |
| 4 | Vector SATD for 4x4 and 8x8 Hadamard sums | Identical | None | Medium, and growing as steps 1, 2, 3 and 10 move decisions to SATD ranking | Obvious, under the kernel policy |
| 5 | Vector forward transform, quantization, inverse scaling and inverse transform on the 32-bit path | Identical | Step 5 | Medium on every input: these leaves run in every exact evaluation | Obvious, under the kernel policy |
| 6 | Step 7, exact early quiet test | Identical | Step 5 | Medium on low motion, small on real motion | Obvious |
| 7 | Multi-candidate SAD: one source row against several candidate rows per call | Identical | Step 6 | Small to medium; integer probes are already cheap | Obvious, under the kernel policy |
| 8 | Step 9, deblocking fast path; a vector filter only if the fast path leaves a measurable share | Identical | None | Small; an earlier trial of the decoder's dispatched filter showed no clear benefit | Obvious |
| 9 | Row wavefront, after the one-time removal of raster-order cost state; admission keeps the normative motion-vector limit for consecutive macroblocks (below) | Changes once, then identical for every thread count | None; the current reference planes are immutable | Scales with cores; needed for parity above one thread, with a measured one-thread ownership cost | Obvious: the threading model is decided |
| 10 | New NEON arithmetic for items 1, 4, 5 and 7 | Identical | Those items | Arm hosts keep scalar SAD and SATD until then | Decided: the encoder kernel policy will be amended (approved) to allow new NEON kernels under the same rules before any new NEON kernel lands; x86-64 kernels come first |

Step 10 completes the target pipeline for P pictures the way step 1 did for IDR. **P finalist shortlist (output change).** Skip is still built and counted exactly, because the quiet test reads it and it can be the best complete candidate. The other inter options (coded 16x16 at the skip vector, the 16x16 plan and, when step 2 searched them, the 8x8 plans) are deduplicated first, so identical complete representations occupy one slot. The remaining options are ranked by SATD plus known syntax lengths, and only the best two are built and counted exactly. When neither passes macroblock admission, the remaining options are evaluated as now, so the shortlist never refuses a macroblock the complete evaluation accepts.

The wavefront removes the consecutive motion-vector count from costs, not from admission. Admission still enforces the limit on motion vectors in two consecutive macroblocks, and the count carried from the previous picture is kept as now. Within a row the predecessor is the left neighbour. Across a row boundary the predecessor belongs to another row, so the last macroblock of each row and the first macroblock of the next are each held to half the pair limit. The pair is then legal without waiting for the other row, and output does not depend on the thread count.

The [row-wavefront implementation](h264-encoder-wavefront.md) separates immutable macroblock decisions from serial entropy coding and deblocking. Its explicit worker-count constructor and automatic available-core selection produce the same stream. Padding and half-sample planes are not prerequisites: workers share the existing immutable reference picture and retain the existing per-call interpolation.

### Compression efficiency

| Rank | Item | Output | Depends on | Expected gain | Start |
|---|---|---|---|---|---|
| 1 | Dead-zone quantization: a rounding offset below one half, separate for intra and inter blocks, fixed before it is measured | Changes | Step 5, which owns the same quantizer | Largest inside the matched settings; more zero blocks also helps speed | Obvious |
| 2 | Sparse-coefficient elimination: zero a 4x4 block, or a macroblock's luma, whose only levels are isolated ones scored by a fixed position cost | Changes | Item 1 | Medium on low motion; speed-neutral or better | Obvious |
| 3 | Mode statistics against x264 output on the input with the largest gap: skip, intra and partition shares and vector lengths | None, diagnosis only | None | Locates decision losses for items 4 and 5 | Obvious: the encoder design allows output analysis |
| 4 | More motion predictors (co-located and above-right vectors) and a search range that follows their spread | Changes | Item 3 | Medium on real motion | Obvious |
| 5 | Rate-distortion levels for finalists: choose each level among the nearest value, one step smaller and zero by exact CAVLC bits and squared error | Changes | Item 1, steps 5 and 10 | Large, but slower; with no presets its cost counts against the same speed target | Decided: enabled by default, and it may cost speed |
| 6 | Tools outside the matched settings: CABAC, B pictures, several reference pictures, the 8x8 transform, weighted prediction and adaptive quantization | Changes the profile | Rate control for some | Large; x264 medium uses them by default | Decided: a later phase, after parity at the matched settings |

Compression items come from the specification and published literature, as the steps do. They are accepted when they lower BD-rate against current main and no measured input rises by more than +1%.

### Default rate-distortion coefficient levels

This stage starts from the merged prediction-specific dead-zone quantizer and the fused residual, transform and reconstruction path. It enables bounded level search by default, accepting a measured speed cost. It changes coefficient policy, not prediction tools, mode shortlists, motion search, profiles, public settings or normative inverse arithmetic. The implementation uses the specification and our existing arithmetic and CAVLC tables, not another encoder's source.

After the existing search selects an admitted complete macroblock, retain its modes, motion and prediction type. Skip stays skip. Refine the selected non-skip representation before it is committed. Cover ordinary luma coefficients, intra16 luma DC, chroma AC and both chroma DC groups. The existing intra 5/12 and inter 1/6 dead-zone results remain the starting point and the fallback. Cheap proposals and the quiet-skip test retain their current arithmetic and budgets. This first bounded search does not re-rank other modes or claim a joint trellis optimum.

For each coefficient, compare its current signed level with nearest rounding, the nearest magnitude reduced by one, and zero. Nearest rounding uses the same position-dependent divisor and the integer one-half bias. Keep the current result on equal cost. Visit luma, U and V in order, each separate DC group before its AC blocks, AC blocks in coding order and coefficients in reverse zigzag order. Perform one pass, not an unbounded convergence loop. Recompute an intra4 block's nearest levels from its actual prediction after the already-refined preceding blocks, rather than retaining targets derived from stale reconstructed borders.

Use exact reconstructed sample SSE on the affected plane and the complete macroblock's exact CAVLC and header bit count, including current neighbouring counts, coded-block-pattern changes and the pending skip-run prefix. The level objective is `512 * plane_SSE + weight * bits`, with `weight = (norm(plane_QP, 0) << (plane_QP / 6))^2 / 4`, the existing quantizer-derived P-candidate rate weight. Chroma uses its mapped QP. Final mode selection retains its current objective, including the legacy IDR mode weight; the level objective does not introduce a fitted rate constant. Other planes' fixed distortion cancels during a coordinate trial. A trial is retained only on strict objective improvement.

Every trial uses the existing fused inverse scaling, inverse transform, rounding and clipping. Intra4 replay follows coding order so changed reconstructed borders affect every dependent prediction. Separate DC changes replay their whole luma or chroma plane because their inverse Hadamard couples blocks. Count syntax through the existing macroblock writer in count mode, not a duplicate bit estimator. Recheck coefficient and macroblock admission for every trial, discard a rejected trial and preserve the admitted starting candidate. Retain transactional picture failure and the existing scalar/dispatched equality requirement. No new unsafe code or dependency is needed.

Compare the implementation with its explicit merged-main base on existing A to E inputs, sixty pictures, GOP32, enabled deblocking and QP22/28/34/40. Each arm uses a separate release build directory and records its full source and executable hashes. Decode every output with the qualified pinned oracle and compare complete reconstructed bytes. Compute visible Y, U and V SSE independently, then per-plane PSNR and PCHIP BD-rate on log bitrate over common quality support without extrapolation. Report each per-plane value, their arithmetic means and the 6:1:1 weighted plane BD-rate `(6 * BD_Y + BD_U + BD_V) / 8`. Also report BD-rate from the explicitly labelled weighted-PSNR curve `(6 * PSNR_Y + PSNR_U + PSNR_V) / 8`, so the two aggregates cannot be confused.

Acceptance requires no input above +0.5% BD-rate on any plane and a negative arithmetic mean of the weighted plane BD-rate across the five inputs. Undefined overlap is not a pass. Report encode-only speed cost per input from alternating arms on the same Actions runner, excluding input loading, file writes and decoding; speed is reported rather than gated. Keep all attempted variants and failures in the private report. Run measurements on throwaway Actions branches and delete those runs and branches afterward. Verify both build modes, exact scalar/dispatched output, admission and rollback tests, strict Clippy, repository lints, public-text checks and exact-head similarity before independent review.

### Order

The row-wavefront implementation proceeds before steps 6 and 8 because it can share the existing immutable reference planes. The reference-plane changes remain separate speed candidates, not concurrency prerequisites. Speed items 1, 5 and 7 and compression item 1 can proceed when their listed arithmetic or storage dependencies are ready. Vector quantization and dead-zone quantization still share one arithmetic contract: the vector kernel must reproduce the scalar quantizer exactly, so whichever lands second re-runs the scalar and vector identity tests against the other's rounding. Step 10 and the revised step 3 remain independent decision candidates.

## Decisions outside this plan

Decided by the maintainer: the target is x264 medium at the same thread count, at the matched settings. Rows are encoded in a wavefront after decisions stop depending on raster-order state (the pending skip-run length inside costs and the consecutive motion-vector count), a small one-time output change; after it, output is identical for every thread count. There are no presets.

Also decided for this roadmap:

- Parity is judged first at the matched settings. A later phase widens the comparison to x264 medium's defaults, with the tools in compression item 6.
- Rate-distortion levels (compression item 5) are enabled by default, even though they cost speed.
- The encoder kernel policy will be amended (approved) to allow new NEON kernels under the same rules as the existing vector kernels, before any new NEON kernel lands. x86-64 kernels come first.

## Correctness and delivery

Each pull request runs both encoder build modes, the identity scripts, encoder Clippy and tests, formatting, the unsafe-site inventory and the current-main similarity guard. No new unsafe code, dependency or default-build change is planned for steps 1 to 9. Vector kernels in the roadmap stay inside the encoder kernel policy: a private module, a scalar twin, differential tests and fuzzing, and output identical to the scalar build.
