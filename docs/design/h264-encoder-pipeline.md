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

The maintainer set the target as x264 medium at the same thread count, on both throughput and PSNR BD-rate, at the settings the [efficiency reference](h264-encoder-efficiency-reference.json) already matches: Constrained Baseline, CAVLC, one reference picture, no B pictures, fixed QP and PSNR tuning. Steps 1 to 9 do not close either gap. The candidates below are ranked by the stage attribution behind this plan and the measured results of steps 1 to 4. An expected gain names the stage it removes work from or the loss it targets; gains on the same stage do not add. Each item is measured like the steps, and output-changing items keep the same BD-rate bound.

On low-motion inputs, IDR coding, the quiet-skip test and the P intra proposal hold most encoder time. On real-motion inputs motion search dominates, and fractional refinement is the larger part of it. Leaf arithmetic (quantization, inverse scaling, transforms, residual extraction and sample copies) is spread across every stage. For compression, the encoder quantizes to the nearest level, eliminates no sparse coefficients and makes no rate-distortion choice between levels. These are standard techniques in the published literature and the largest remaining differences inside the matched settings.

*Obvious* items follow decisions already made and start when their dependencies merge. *Maintainer* items need a decision first.

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
| 9 | Row wavefront, after the one-time removal of raster-order cost state | Changes once, then identical for every thread count | Steps 6 and 8, so per-picture planes exist before rows start | Scales with cores; needed for parity above one thread and neutral at one thread | Obvious: the threading model is decided |
| 10 | New NEON arithmetic for items 1, 4, 5 and 7 | Identical | Those items | Arm hosts keep scalar SAD and SATD until then | Maintainer: the kernel policy names AVX2 detection and reuse of existing NEON kernels, not new NEON arithmetic |

Step 10 completes the target pipeline for P pictures the way step 1 did for IDR. **P finalist shortlist (output change).** Rank skip, 16x16 and, when step 2 searched them, 8x8 layouts by SATD plus known syntax lengths, and build and count exactly only the best two. When neither passes macroblock admission, the complete evaluation runs as now, so the shortlist never refuses a macroblock the complete evaluation accepts.

### Compression efficiency

| Rank | Item | Output | Depends on | Expected gain | Start |
|---|---|---|---|---|---|
| 1 | Dead-zone quantization: a rounding offset below one half, separate for intra and inter blocks, fixed before it is measured | Changes | Step 5, which owns the same quantizer | Largest inside the matched settings; more zero blocks also helps speed | Obvious |
| 2 | Sparse-coefficient elimination: zero a 4x4 block, or a macroblock's luma, whose only levels are isolated ones scored by a fixed position cost | Changes | Item 1 | Medium on low motion; speed-neutral or better | Obvious |
| 3 | Mode statistics against x264 output on the input with the largest gap: skip, intra and partition shares and vector lengths | None, diagnosis only | None | Locates decision losses for items 4 and 5 | Obvious: the encoder design allows output analysis |
| 4 | More motion predictors (co-located and above-right vectors) and a search range that follows their spread | Changes | Item 3 | Medium on real motion | Obvious |
| 5 | Rate-distortion levels for finalists: choose each level among the nearest value, one step smaller and zero by exact CAVLC bits and squared error | Changes | Item 1, steps 5 and 10 | Large, but slower; with no presets its cost counts against the same speed target | Maintainer: trades speed for rate |
| 6 | Tools outside the matched settings: CABAC, B pictures, several reference pictures, the 8x8 transform, weighted prediction and adaptive quantization | Changes the profile | Rate control for some | Large; x264 medium uses them by default | Maintainer: changes the comparison basis and the configuration surface |

Compression items come from the specification and published literature, as the steps do. They are accepted when they lower BD-rate against current main without any input exceeding the bound.

### Order

Steps 5, 6 and 8 are in progress. Once they merge, speed items 1, 5 and 7 and compression item 1 can proceed in parallel, because they touch separate code. Step 10 and the revised step 3 follow step 6. The wavefront comes last among speed items, so it parallelizes the cheaper per-row work rather than the current one.

## Decisions outside this plan

Decided by the maintainer: the target is x264 medium at the same thread count, at the matched settings. Rows are encoded in a wavefront after decisions stop depending on raster-order state (the pending skip-run length inside costs and the consecutive motion-vector count), a small one-time output change; after it, output is identical for every thread count. There are no presets.

Still open:

- Whether parity is judged only at the matched settings, or also against x264 medium's defaults with the tools in compression item 6.
- Whether rate-distortion levels (compression item 5) may cost speed.
- Whether new NEON arithmetic needs an amendment to the encoder kernel policy.

## Correctness and delivery

Each pull request runs both encoder build modes, the identity scripts, encoder Clippy and tests, formatting, the unsafe-site inventory and the current-main similarity guard. No new unsafe code, dependency or default-build change is planned for steps 1 to 9. Vector kernels in the roadmap stay inside the encoder kernel policy: a private module, a scalar twin, differential tests and fuzzing, and output identical to the scalar build.
