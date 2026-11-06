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

Each P macroblock passes a cost ladder: a cheap exact test settles most skips without transforms; a predictor-seeded integer search with early termination proposes motion; fractional refinement reads precomputed half-sample planes; candidates are ranked by SAD, then SATD plus known syntax lengths; only the one or two finalists are reconstructed and counted exactly. IDR macroblocks use the same ladder for intra modes. Picture-level work (padded reference planes, half-sample planes, deblocking) is done once per picture in row order, which prepares row-parallel encoding.

## Steps

Each step is one pull request with one outcome, in this order. Before a step is built, its ceiling is counted in a throwaway build that deletes the work it removes; a step whose ceiling is negligible is dropped. Byte-identical steps must reproduce the current main stream and reconstruction in every case of both identity sets (`scripts/h264_g1_simd_identity.py` and the 57-case set), in dispatched and scalar builds. Output-changing steps report PSNR BD-rate against current main over QP 22, 28, 34 and 40 on A, B, C and D (PCHIP on log rate, common interval, no extrapolation), regenerate any byte fixture they change in the same pull request, and decode with the pinned FFmpeg to the encoder reconstruction. Throughput is an encoder-only reference figure from one GitHub-hosted job that runs current main and the candidate alternately, with instruction counts beside it.

1. **IDR cost ladder (output change).** For each I4x4 block, rank the legal modes by SATD plus mode bits and keep the exact trials of the best two that complete; a trial whose levels leave the coefficient range is not a finalist. Rank the I16x16 and chroma modes by SATD and complete only the best two of each, so an IDR macroblock normally builds at most six complete candidates. When none of them passes macroblock admission, the macroblock falls back to the complete search, which can build more, so the shortlist never refuses a macroblock the complete search accepts. P pictures keep the complete fallback.
2. **P partition gating (output change).** Search the four 8x8 partitions only when the 16x16 result leaves a SATD above a QP-scaled bound.
3. **P intra gating (output change).** Build the P intra proposal only when its SATD cost, with its known syntax, is below the SATD cost of the best inter prediction. The gate applies only once an inter candidate has been admitted; without one, the complete intra search runs as now, so admission never depends on SATD.
4. **Integer search early termination (output change).** Stop the integer refinement when the seed's SAD is below a QP-scaled bound.
5. **Narrow transform path (byte-identical).** Forward transform, quantization, scaling and inverse transform on 32-bit fixed arrays, with the residual taken in the same pass and range checks at the block boundary instead of per call.
6. **Edge-extended reference planes (byte-identical).** Pad each reference picture once so motion compensation and SAD read rows without clamping, and keep candidate samples in reusable per-macroblock scratch instead of moved structures.
7. **Exact early quiet test (byte-identical).** Every 4x4 forward-core coefficient is bounded by its row and column gain times the block's sum of absolute residuals, and the chroma DC terms come exactly from signed block sums. When that bound, passed through the quantizer's own zero test, cannot reach a nonzero level, the block is quiet without transforming it; otherwise the transform runs as now. A test proves the bound against the transform and zero test at every QP.
8. **Half-sample planes (byte-identical).** Compute the three half-sample planes of each reference picture once with the normative six-tap filter, and derive quarter samples by the normative averages, so fractional probes and inter reconstruction read the planes instead of interpolating per call. Its value is judged on D, because low-motion inputs search little.
9. **Deblocking fast path (byte-identical).** Filter the rows of edges with nonzero boundary strength through row slices checked once per edge instead of per-sample checked access.

## Decisions outside this plan

- The speed target and BD-rate bound for steps 1 to 9.
- Threading model. Rows can be encoded in a wavefront if decisions stop depending on raster-order state (the pending skip-run length inside costs and the consecutive motion-vector count), which is a small one-time output change; after it, output is identical for every thread count. Slices are simpler but cost rate and make output depend on the slice count.
- Presets, which change defaults and the configuration surface.

## Correctness and delivery

Each pull request runs both encoder build modes, the identity scripts, encoder Clippy and tests, formatting, the unsafe-site inventory and the current-main similarity guard. No new unsafe code, dependency or default-build change is planned for steps 1 to 9.
