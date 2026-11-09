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
2. **P partition gating (output change).** Search the four 8x8 partitions only when the 16x16 result leaves a SATD above a QP-scaled bound. When coding the residual at the skip vector did not beat skip, the macroblock is already well predicted, so the bound doubles. Raising the bound for every macroblock instead cost one real-motion input up to +1.7% PSNR BD-rate on a plane even at 1.25 times the bound, while the doubled bound for these macroblocks alone kept every input and plane within +0.5% of main.
3. **P intra gating (output change, margin gate).** Comparing the best I16x16 SATD with the inter SATD cost directly proved unsafe: the few P macroblocks that intra wins are mostly I4x4 decisions, which an I16x16-only estimate cannot see, so the gate dropped them and cost rate. The shipped gate keeps the cheap I16x16 proposal and skips only the cheap I4x4 proposal, and only when four times the best inter plan's SATD rank (16x16 or 8x8, motion cost included) is at most the best I16x16 SATD. With a factor of one or two, an I16x16-based gate still lost I4x4 decisions on one input; with four, every input and plane stayed within +0.5% PSNR BD-rate of main. The gate applies only once an inter candidate has been admitted; without one, the I4x4 proposal runs as before, and a rejected cheap proposal still falls back to the complete search, so admission never depends on SATD.
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
| 3 | An I4x4-aware P intra gate that also covers the macroblocks step 3's margin gate leaves open | Changes | Step 3 | Medium on low motion, where the remaining P intra proposals cost about as much as motion search | Obvious; measured before it is built |
| 4 | Vector SATD for 4x4 and 8x8 Hadamard sums | Identical | None | Medium, and growing as steps 1, 2, 3 and 10 move decisions to SATD ranking | Obvious, under the kernel policy |
| 5 | Vector forward transform, quantization, inverse scaling and inverse transform on the 32-bit path | Identical | Step 5 | Medium on every input: these leaves run in every exact evaluation | Obvious, under the kernel policy |
| 6 | Step 7, exact early quiet test | Identical | Step 5 | Medium on low motion, small on real motion | Obvious |
| 7 | Multi-candidate SAD: one source row against several candidate rows per call | Identical | Step 6 | Small to medium; integer probes are already cheap | Obvious, under the kernel policy |
| 8 | Step 9, deblocking fast path; a vector filter only if the fast path leaves a measurable share | Identical | None | Small; an earlier trial of the decoder's dispatched filter showed no clear benefit | Obvious |
| 9 | Row wavefront, after the one-time removal of raster-order cost state; admission keeps the normative motion-vector limit for consecutive macroblocks (below) | Changes once, then identical for every thread count | None; the current reference planes are immutable | Scales with cores; needed for parity above one thread, with a measured one-thread ownership cost | Obvious: the threading model is decided |
| 10 | New NEON arithmetic for items 1, 4, 5 and 7 | Identical | Those items | Arm hosts keep scalar SAD and SATD until then | Permitted by the encoder kernel policy, with x86-64 first; the first NEON implementation must add passing encoder verification on an arm64 CI runner |

Step 10 completes the target pipeline for P pictures the way step 1 did for IDR. **P finalist shortlist (output change).** Skip is still built and counted exactly, because the quiet test reads it and it can be the best complete candidate. The other inter options (coded 16x16 at the skip vector, the 16x16 plan and, when step 2 searched them, the 8x8 plans) are deduplicated first, so identical complete representations occupy one slot. The remaining options are ranked by SATD plus known syntax lengths, and only the best two are built and counted exactly. When neither passes macroblock admission, the remaining options are evaluated as now, so the shortlist never refuses a macroblock the complete evaluation accepts.

The wavefront removes the consecutive motion-vector count from costs, not from admission. Admission still enforces the limit on motion vectors in two consecutive macroblocks, and the count carried from the previous picture is kept as now. Within a row the predecessor is the left neighbour. Across a row boundary the predecessor belongs to another row, so the last macroblock of each row and the first macroblock of the next are each held to half the pair limit. The pair is then legal without waiting for the other row, and output does not depend on the thread count.

The [row-wavefront implementation](h264-encoder-wavefront.md) separates immutable macroblock decisions from serial entropy coding and deblocking. Its explicit worker-count constructor and automatic available-core selection produce the same stream. Padding and half-sample planes are not prerequisites: workers share the existing immutable reference picture and retain the existing per-call interpolation.

**Intra 4x4 neighbours once per block (identical output).** The Intra_4x4 ranking predicts up to nine modes of each 4x4 block from the same neighbours, and each prediction used to build a checked view, read those neighbours again and copy its rows through a call per row. `Intra4x4Neighbours` in the shared kernel crate now reads them once per block, and each mode's prediction is a 16-byte array that the encoder stores with fixed four-byte row copies. The ranking, the shortlist trials and the cheap path all predict from that one read; the neighbours lie outside the block, so the trials that overwrite the block cannot change them. The decoder's reference `pred4x4_view` predicts through the same type, and its aarch64 code makes the same four calls in one instruction fewer (551 against 552); the x86-64 decoder uses its own vector intra kernels and does not reach it.

One GitHub Actions x86-64 job (AMD EPYC 7763, two physical cores and four logical CPUs) compares main at `68cbc80372fb15e9876e6c062f187b1677bb398a` with this change on the five existing lossy inputs at QP22 and QP28, sixty pictures, GOP32 and enabled deblocking. Callgrind counts the encode calls only; timing alternates the builds and keeps five calls per arm and cell after one warm-up.

| Input and QP | Instructions removed | 1T time saved | 4T time saved |
|---|---:|---:|---:|
| A-22 | 3.79% | 3.43% | 2.92% |
| A-28 | 3.88% | 3.09% | 4.52% |
| B-22 | 2.63% | 3.56% | 2.01% |
| B-28 | 3.07% | 4.10% | 2.02% |
| C-22 | 0.98% | 1.61% | 1.97% |
| C-28 | 1.43% | 1.67% | 2.86% |
| D-22 | 1.85% | 2.56% | 1.92% |
| D-28 | 2.64% | 3.08% | 2.55% |
| E-22 | 1.24% | 1.07% | 1.33% |
| E-28 | 1.83% | 2.44% | 2.37% |
| Mean | 2.33% | 2.66% | 2.44% |

Callgrind on C-22 shows predictions through a checked view (`pred4x4_view`) falling from 346,430 to 74,346 calls, now reached only outside the ranking, while 272,084 predictions from the shared read run at about 68 instructions each instead of about 211; `memcpy` calls fall by about 0.8 million. Every one of the 270 encodes in this run produces the same stream and reconstruction as main, and the pinned FFmpeg 9.0.2 decoder reproduces the reconstruction in all 30 decodes. An earlier job on an AMD EPYC 9V74 runner, with the prediction arithmetic in a different function, measured 2.37%, 2.76% and 2.22%. These are shared-runner reference figures, not a fixed-host speed verdict.

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

After the existing search selects an admitted complete macroblock, retain its modes, motion and prediction type. Skip stays skip. If all starting luma and chroma levels, including separate DC, are zero, return without refinement. This shortcut also skips DC trials whose nearest-rounded target could be nonzero. Otherwise refine the selected non-skip representation before it is committed, covering intra16 luma DC and both chroma DC groups. In each ordinary luma or chroma block, retain at most two current coefficients of magnitude one or two, chosen from reverse scan order; other ordinary levels stay unchanged. The luma intra 5/12 and inter 1/6 offsets and the separate chroma 5/12 offsets remain the starting point and the fallback. Cheap proposals and the quiet-skip test retain their current arithmetic and budgets. This bounded search does not re-rank other modes or claim a joint trellis optimum.

For each retained coordinate, compare its current signed level with nearest rounding, the nearest magnitude reduced by one, and zero. Nearest rounding uses the same position-dependent divisor and the integer one-half bias. Compute only targets that are visited. Keep the current result on equal cost. Visit luma, U and V in order, each separate DC group before its AC blocks and AC blocks in coding order. Perform one pass, not an unbounded convergence loop. Recompute an intra4 block's nearest target from its actual prediction after the already-refined preceding blocks, rather than retaining targets derived from stale reconstructed borders.

Use exact reconstructed sample SSE on the affected plane and the complete macroblock's exact CAVLC and header bit count, including current neighbouring counts, coded-block-pattern changes and the pending skip-run prefix. The level objective is `512 * plane_scale * plane_SSE + weight * bits`, with `weight = (norm(plane_QP, 0) << (plane_QP / 6))^2 / 4`, the existing quantizer-derived P-candidate rate weight. Chroma uses its mapped QP. `plane_scale` is one for luma and four for each chroma plane: a 4:2:0 macroblock has 256 luma samples but only 64 samples of each chroma plane, so this compares normalized plane distortion rather than underweighting each chroma plane by sample count. Final mode selection retains its current objective, including the legacy IDR mode weight; the level objective does not introduce a fitted rate constant. Other planes' fixed distortion cancels during a coordinate trial. A trial is retained only on strict objective improvement.

The first implementation used unnormalized SSE for every plane. Its reference comparison improved mean luma BD-rate but failed the per-plane chroma limit. That arm is rejected, not accepted through a weighted average. The next comparison fixes the distortion normalization from the 4:2:0 sample counts before measuring it; the +0.5% per-plane limit is unchanged.

Every trial uses the existing fused inverse scaling, inverse transform, rounding and clipping. Intra4 replay follows coding order so changed reconstructed borders affect every dependent prediction. Separate DC changes replay their whole luma or chroma plane because their inverse Hadamard couples blocks. Count syntax through the existing macroblock writer in count mode, not a duplicate bit estimator. Recheck coefficient and macroblock admission for every trial, discard a rejected trial and preserve the admitted starting candidate. Retain transactional picture failure and the existing scalar/dispatched equality requirement. No new unsafe code or dependency is needed.

Compare the implementation with its explicit merged-main base on existing A to E inputs, sixty pictures, GOP32, enabled deblocking and QP22/28/34/40. Each arm uses a separate release build directory and records its full source and executable hashes. Decode every output with the qualified pinned oracle and compare complete reconstructed bytes. Compute visible Y, U and V SSE independently, then per-plane PSNR and PCHIP BD-rate on log bitrate over common quality support without extrapolation. Report each per-plane value, their arithmetic means and the 6:1:1 weighted plane BD-rate `(6 * BD_Y + BD_U + BD_V) / 8`. Also report BD-rate from the explicitly labelled weighted-PSNR curve `(6 * PSNR_Y + PSNR_U + PSNR_V) / 8`, so the two aggregates cannot be confused.

Acceptance requires no input above +0.5% BD-rate on any plane and a negative arithmetic mean of the weighted plane BD-rate across the five inputs. Undefined overlap is not a pass. Report encode-only speed cost per input from alternating arms on the same Actions runner, excluding input loading, file writes and decoding. The maintainer accepted the initial one-trailing-coefficient plus DC point as the default, including its measured 0.849 to 0.916 times baseline throughput. That point passed all fifteen plane limits and improved mean weighted plane BD-rate by 2.633% against merged main after the separate chroma-offset correction. It is the incremental reference for the two-trailing-coefficient plus DC default, not the earlier pre-refinement quantizer. Keep all attempted variants and failures in the private report. Run measurements on throwaway Actions branches and delete those runs and branches afterward. Verify both build modes, exact scalar/dispatched output, admission and rollback tests, strict Clippy, repository lints, public-text checks and exact-head similarity before independent review.

First remove redundant work without changing the selected levels: retain unchanged CAVLC residual costs, deriving their context-dependent token length from the existing tables. When coded patterns stay fixed, update the edited residual and its right and below token contexts; when a pattern changes, recount through the same macroblock syntax owner. Reconstruct only a changed independent 4 by 4 block, or the affected intra4 suffix, skipping later blocks whose available prediction borders are unchanged. Update only the affected region's distortion. Separate DC still couples its entire plane. Check every incremental count against the uncached writer in test builds, and compare partial replay with full replay. Then measure bounded subsets of the same final-winner pass, including low-magnitude trailing coefficients and DC groups, rather than refining other mode candidates. Retain the full quality and speed curve alongside the accepted tradeoff.

### Stronger default coefficient budget

The follow-up increases the ordinary-block budget from one to two trailing current coefficients of magnitude one or two. Separate DC, fixed mode/motion, strict tie retention, admission fallback and the existing rate/distortion objective stay unchanged. There remains one default, not a public preset or a run-time switch.

The maintainer requires the complete cost to be compared directly with pre-refinement main `382daaa8f9d14ef2966782013838468e32cfda4c`. The one-coordinate implementation at `33f983421e58e34a4175ad4aa8780a2d0a36e12c` is only an incremental reference; ratios from separate comparisons cannot establish the total-cost limit. Keep the same existing inputs, preparation hashes, sixty pictures, GOP32, deblocking, QP22/28/34/40 and both labelled weighted aggregates. Against the pre-refinement baseline, require every input's Y, U and V BD-rate to be at most +0.5%, a negative five-input mean weighted-plane BD-rate, and at least 0.85 times baseline throughput on every input at both one and four requested encoder threads. Use separate release builds at explicit source commits, paired alternating QP28 observations on the same Actions runner, and retain every call, median and range. The comparison after the speed changes uses nine observations per arm and a same-binary control. If any input or thread count misses, retain the draft and report the complete table, without rounding a miss into a pass.

Measured ordinary-block budget: `2`. Measured candidate: `980d571c90a509db998cb778514312992448ede3`; direct baseline: `382daaa8f9d14ef2966782013838468e32cfda4c`.

| Input | Y BD-rate | U BD-rate | V BD-rate | Throughput, 1 worker | Throughput, 4 workers |
|---|---:|---:|---:|---:|---:|
| A | −1.004551% | −1.915512% | −9.036384% | 1.014123× | 1.042835× |
| B | −0.056678% | −2.030309% | −3.410406% | 1.085911× | 1.090179× |
| C | −1.002728% | −5.645026% | −8.601897% | 0.919562× | 0.993361× |
| D | −5.089595% | −7.943629% | −10.429743% | 0.885950× | 0.932764× |
| E | −3.828439% | −3.571050% | −6.896190% | 0.886955× | 0.918975× |

All fifteen plane limits and all ten speed floors pass. The five-input mean weighted-plane BD-rate is −3.134302%; the separate weighted-PSNR-curve mean is −2.800490%. The worst speed cell is D at one worker, `0.8859499181424747×`. The nine paired QP28 observations per arm and thread count retain every call. The shared-host reference uses Ubuntu 24.04, AMD EPYC 9V45, four logical CPUs on two SMT-enabled cores and rustc 1.99.1. The same-binary control for E at one worker is 0.984111×, with a 95% interval of 0.970353–0.998779×; no observations were excluded or retimed. The lowest candidate paired-bootstrap 95% bound is 0.865361×. These are shared-host reference observations, not fixed-host production guarantees. Qualified FFmpeg 9.0.2 matches all forty complete curve reconstructions, and native/scalar one/four-worker identity covers all 160 curve encodes. The full method and timing ranges are in [coreav-dev/coreav#490](https://github.com/coreav-dev/coreav/pull/490).

Count the ceiling before implementing arithmetic changes. A throwaway release profile makes the complete refinement entry and sample-error function visible, then counts their inclusive instructions on short prefixes of existing inputs. These counts bound a hypothetical zero-cost replacement in the instrumented workload, not a wall-clock gain. Selected levels and future reference pictures stay unchanged; an output-changing erasure would change the later search workload. Record complete encoded/reconstruction digests and independent decoding to show that useful encoding occurred. Try the already implemented two-coordinate policy without new kernels first. A proposed AVX2 kernel needs its own removable-cost ceiling and useful end-to-end reference benefit, scalar equivalence, bounded memory views, differential tests/fuzzing, inventory and similarity checks under the existing encoder exception.

### Order

The row-wavefront implementation proceeds before steps 6 and 8 because it can share the existing immutable reference planes. The reference-plane changes remain separate speed candidates, not concurrency prerequisites. Speed items 1, 5 and 7 and compression item 1 can proceed when their listed arithmetic or storage dependencies are ready. Vector quantization and dead-zone quantization still share one arithmetic contract: the vector kernel must reproduce the scalar quantizer exactly, so whichever lands second re-runs the scalar and vector identity tests against the other's rounding. Step 10 and the revised step 3 remain independent decision candidates.

## Decisions outside this plan

Decided by the maintainer: the target is x264 medium at the same thread count, at the matched settings. Rows are encoded in a wavefront after decisions stop depending on raster-order state (the pending skip-run length inside costs and the consecutive motion-vector count), a small one-time output change; after it, output is identical for every thread count. There are no presets.

Also decided for this roadmap:

- Parity is judged first at the matched settings. A later phase widens the comparison to x264 medium's defaults, with the tools in compression item 6.
- Rate-distortion levels (compression item 5) are enabled by default, even though they cost speed.
- The encoder kernel policy permits new NEON kernels under the same safety and verification rules as the existing vector kernels. x86-64 kernels come first, and the first NEON implementation must add passing encoder verification on an arm64 CI runner.

## Correctness and delivery

Each pull request runs both encoder build modes, the identity scripts, encoder Clippy and tests, formatting, the unsafe-site inventory and the current-main similarity guard. No new unsafe code, dependency or default-build change is planned for steps 1 to 9. Vector kernels in the roadmap stay inside the encoder kernel policy: a private module, a scalar twin, differential tests and fuzzing, and output identical to the scalar build.
