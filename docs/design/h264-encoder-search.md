# Bounded H.264 encoder search

Status: the initial bounded policy and the compression-efficiency amendment below are implemented after their respective design-first commits. They change normal P-picture search, not H.264 reconstruction, the all-IDR path, or default-build admission. Encoded bytes may change.

Complete P-picture decisions now use the independently derived [quantizer-aware rate weight](h264-encoder-rate-weight.md). Complete scores and exact syntax floors share 512 fixed-point units per sample SSE. IDR ordering and the cheap SAD/SATD proposal weights remain unchanged.

## Inputs and constraints

The specification is Rec. ITU-T H.264, version 15, August 2024. Clauses 7 and 9 define syntax and bit counting, 8.3 and 8.4 define prediction, 8.5 defines reconstruction, and Annex A defines admission. The reviewed [encoder speed note](../technique-notes/h264-encoder-speed.md) and [partition-search note](../technique-notes/h264-partition-search.md) are allowed prose inputs. No other encoder source, unreviewed study report, or new source study is needed for this design.

### Prior art

The two linked technique notes study x264 core 165 at `0480cb05fa188d37ae87e8f4fd8f1aea3711f7ee`, under GPL-2.0-or-later, and OpenH264 v2.7.0 at `1bca62311e447fd9c39fc93de2c4b96cd5ce971c`, under BSD-2-Clause. This implementation uses those notes as prose, without opening either source. The bounded proposal and its constants come from the committed independent design, not either implementation's control flow.

The previous [profile and exact-search round](h264-encoder-p.md#partition-search-cost-design) assigns about 89% of sampled process cycles to partition search. Cheap probes, full-RDO finalists, and preparation all scale with the number of searched partitions. Removing only arithmetic cannot remove those multiplicative factors. The remaining intra work also prevents a tenfold improvement even if inter search disappeared, so this design addresses both normal P-picture paths.

Keep the opt-in feature, safe scalar Rust, fixed QP, GOP, colour signalling, filtering, shared syntax traversal, and transactional sequence state. A lower-cost search decision never bypasses vector, coefficient, consecutive motion-count, macroblock-bit, or output-budget admission. Preserve all existing legal prediction and syntax support for conformance fixtures.

## Step 1: propose complete inter layouts cheaply

Search only P_L0_16x16 and P_8x8 with four 8x8 partitions in normal mode decision. Do not search 16x8, 8x16, or smaller subpartitions. Those tools remain representable and independently tested, but no longer multiply the ordinary search tree.

For each partition, score zero, the normative predictor, and its rounded integer position. Starting at the best integer seed, test four axial neighbours at integer steps of four, two, and one luma samples, with one fixed-centre pass per step. Then test four axial half-sample neighbours and four axial quarter-sample neighbours. Keep the best result after each stage. This is at most 23 probe calls per partition, or 115 across both layouts, including seeds and duplicates. Predictors outside the local range remain legal seeds. Reject, never clip, invalid vectors or differences. Retain deterministic cost, motion-length, and vector ties.

Build each layout greedily in syntax order with a private motion map. Rank complete layouts by luma SAD plus the existing SAD-scale penalty times complete inter-prediction syntax length. Count that syntax after deriving all motion differences. Do not double-count motion lengths already present in partition scores. This is a cheap proxy, not full RDO or a lower bound on it.

Run complete reconstruction and exact syntax counting only for the best cheap layout. If normative coefficient or macroblock admission rejects it, try the other admitted layout. Always evaluate the normative skip candidate independently. An exact zero-distortion skip can terminate normal inter search. Otherwise compare the completed regular inter candidate with skip using the existing full candidate objective. At most two regular complete inter evaluations are needed, not several macroblock reconstructions for every partition of every layout.

The alternative of ranking every old layout cheaply still performs many redundant searches. Retaining only whole-block motion would be cheaper, but gives opposing local motion no representation. Whole-block plus four 8x8 regions is the initial compromise to measure rather than a claim of optimal quality.

## Step 2: postpone and bound P-picture intra work

Evaluate inter before intra in P pictures. An admitted exact skip can finish the macroblock without constructing discarded intra candidates. This is a deliberate mode policy, not a proof that intra cannot use fewer bits.

Otherwise rank legal I_16x16 luma modes and legal chroma modes by prediction SAD, without residual transforms or reconstruction. Complete only the best luma/chroma pair and compare it with the admitted inter candidate using exact full RDO. Normal P-picture selection omits I_4x4 analysis. IDR pictures keep the current complete intra search, including I_4x4, so the unchanged all-intra byte snapshot remains valid.

If the selected cheap intra pair is unrepresentable, use the existing complete intra procedure as a bounded admission fallback. This preserves the zero-motion-count escape when an inter layout is forbidden. The ordinary path has one skip evaluation, at most one admitted regular inter evaluation, and one complete intra pair. Refusal paths can additionally try the second inter layout and the existing finite intra fallback. No partial candidate may change committed neighbours, reconstruction, or sequence state.

The competing design is a content threshold that omits intra whenever inter error looks small. It avoids more work but introduces a poorly justified threshold. Cheaply choosing one intra pair keeps a complete intra comparison without that threshold.

## Reference comparisons and verification

Commit this design first on a fresh branch from main. Measure step 1 against main, then step 2 against step 1 and main. Keep only steps whose speed benefit repays their observed quality and rate cost. These are reference comparisons without acceptance bars, not claims at matched quality.

Use the existing 640x480 clip and at least one additional existing R2 clip. Prepare 60 YUV420 pictures at 30 pictures per second with the pinned FFmpeg. Preserve native geometry where admitted and record any conversion, prepared hashes, and lossy input status. Use QP 28, GOP 32, one reference, enabled zero-offset deblocking, and one pinned CPU. Run x264 medium with the same previous constrained-baseline, single-thread, fixed-GOP, no-B, no-lookahead, unweighted settings and fixed QP offsets. Report each arm's fps, stream bitrate, PSNR, and SSIM against the identical prepared source. Keep timing and metric work separate. Record complete run failures rather than averaging incomplete output.

The existing raw-picture driver verifies CoreAV reconstruction while encoding. Independently decode every measured stream with pinned FFmpeg and compare every visible Y, Cb, and Cr byte to encoder reconstruction, with exact frame counts and no decoder warnings. Rate and quality may differ from main. Encoder reconstruction and the normative decoder must not differ.

Add tests for probe and full-evaluation bounds, small-range and outlying seeds, deterministic ties, layout admission, cheap mode legality, refusal fallback, and transactional retry. Keep existing tests intact. The existing forced-layout fixtures and three-decoder CI still cover every supported syntax form, including forms omitted by normal search. Run the exact-head encoder similarity guard using the current main allowlist before review.

## Implementation and reference results

This section records the initial implementation in coreav-dev/coreav#446; the recovery amendment below supersedes its normal search policy. That round adopted both steps: two cheap inter-layout proposals and one successful full evaluation, with the other layout retained only for admission failure. Broad search and incremental-trial helpers remained test-only. The P-picture intra path completed one cheap-selected pair, with the complete original procedure retained for IDR pictures and admission fallback. Its 43 encoder tests passed, including the unchanged all-intra byte snapshot.

The following are single-run reference figures from the same two-CPU AMD EPYC machine, pinned to CPU 0, using optimized release builds with debug information. Both inputs are already lossy. A retains 640x480 geometry. B is resized from 414x900 to 368x800 for the level-3.0 comparison envelope. Each uses the first 60 prepared pictures after conversion to 30 pictures per second. A has 48 distinct prepared pictures and B has five. B is a low-motion sanity check only, not evidence for broad-motion performance. Prepared hashes, binary hashes, the compiler identified by the measured executables' ELF notes, source commits, and complete per-call wall and CPU times are in the [raw reference record](h264-encoder-search-reference.json).

| Clip | Arm | Pictures/s | kbit/s | PSNR, dB | SSIM |
|---|---|---:|---:|---:|---:|
| A | Main | 1.120 | 346.960 | 45.637606 | 0.998474 |
| A | Step 1, bounded inter | 10.377 | 361.972 | 44.987874 | 0.998169 |
| A | Step 2, inter-first bounded intra | 17.479 | 399.544 | 44.976752 | 0.998128 |
| A | x264 medium | 313.106 | 353.124 | 47.729296 | 0.998890 |
| B | Main | 0.748 | 126.776 | 44.152652 | 0.992678 |
| B | Step 1, bounded inter | 7.893 | 126.776 | 44.152652 | 0.992678 |
| B | Step 2, inter-first bounded intra | 14.128 | 126.776 | 44.152652 | 0.992678 |
| B | x264 medium | 340.392 | 114.048 | 43.909035 | 0.991632 |

Step 1 is 9.267x main on A and 10.548x on B. Its cost on A is 4.327% more rate, 0.649732 dB less PSNR, and 0.000305 less SSIM. B's complete stream is unchanged. Step 2 is another 1.684x on A and 1.790x on B. Compared with main, the final A result is 15.609x throughput at 15.156% more rate, 0.660854 dB less PSNR, and 0.000346 less SSIM. Final B is 18.882x main with unchanged complete output. These fixed-QP observations alone do not establish the rate-distortion cost. x264 remains much faster, and equal numeric QP does not establish equivalent quantization or quality.

Timing uses the existing `p WIDTH HEIGHT 28 60 INPUT STREAM RECON DECODED true 32` driver under `taskset -c 0`. Native timing includes its CoreAV decoder verification and file IO. The x264 arm times pinned FFmpeg with raw YUV420 input, 30 pictures per second, `-preset medium -tune psnr -profile:v baseline -level:v 3.0 -qp 28 -threads 1 -g 32 -keyint_min 32 -bf 0 -refs 1 -sc_threshold 0 -rc-lookahead 0 -weightp 0`, and `-x264-params ipratio=1:pbratio=1:chroma-qp-offset=0:sync-lookahead=0`. Both input and output options declare `-color_primaries bt709 -color_trc bt709 -colorspace bt709 -color_range tv -chroma_sample_location left`; input options precede `-i`. A raw-output passthrough with those same options must preserve every prepared byte before encoding. Its timing includes startup and stream IO but not decoder verification. The figures are end-to-end driver references, not isolated encoder-kernel costs, and have no repeatability interval.

Pinned FFmpeg 9.0.2 independently decodes every measured native output and matches every encoder reconstruction byte, with exactly 60 pictures and no decoder warnings. Metric work is outside timing. FFmpeg's `psnr` and `ssim` filters compare the complete decoded YUV420 output with the identical prepared source. PSNR is the complete-sequence, sample-weighted average across Y, Cb, and Cr, not the arithmetic mean of frame PSNR values. It is independently recomputed as `10 * log10(255² * sample_count / sum_squared_error)` from the raw bytes. SSIM is the filter's complete-sequence All result. Each metric retains all 60 per-picture records. Bitrate counts the complete elementary stream, including repeated headers, over two seconds.

### Corrected x264 input path

The original x264 rows are withdrawn. Output-only colour options caused FFmpeg to insert a colour-matrix conversion through RGB, so x264 did not receive the native arm's prepared samples. For A, passthrough without output colour options has SHA-256 `756fa818691a60fa95f987bc606b00c95776a81142e7500cb39fac263cae9583`; output-only tagging changes it to `3a1e2f9932c42a7f56b3b5b5e2f4b9f4904f5e175a1f082fa774904b775f74f7`. Comparing the old decoded stream against those two references reproduces 42.052419 dB and 47.830702 dB respectively. That is a reference-sample mismatch, not a codec quality change. The corrected rows above re-encode both inputs with input metadata declared before `-i` and digest-matched passthrough. Their stream bytes, bitrate, timing, PSNR, and SSIM replace the original observations. The receipt test checks this command ordering and sample binding, not just the reported score.

### Three-QP rate-distortion reference

The maintainer requested a small QP 22, 28, 34 sweep on A to estimate the compression cost instead of inferring it from one fixed QP. The same main and final executables, prepared input, GOP, filtering, and CPU affinity are used. QP 28 reuses the original native timing observations; the other native cells and all corrected x264 cells are new calls. There is one observation per cell, no timing interval, and no new media. B remains a low-motion sanity check only.

| QP | Arm | Pictures/s | kbit/s | PSNR, dB | SSIM |
|---|---|---:|---:|---:|---:|
| 22 | Main | 1.097 | 435.616 | 49.404697 | 0.999310 |
| 22 | Final | 19.349 | 507.480 | 49.002062 | 0.999193 |
| 22 | x264 medium | 317.149 | 442.784 | 52.873790 | 0.999524 |
| 28 | Main | 1.120 | 346.960 | 45.637606 | 0.998474 |
| 28 | Final | 17.479 | 399.544 | 44.976752 | 0.998128 |
| 28 | x264 medium | 313.106 | 353.124 | 47.729296 | 0.998890 |
| 34 | Main | 0.896 | 264.880 | 40.272336 | 0.995438 |
| 34 | Final | 17.310 | 299.176 | 39.162975 | 0.995469 |
| 34 | x264 medium | 351.804 | 259.744 | 42.362430 | 0.997307 |

The three-point PSNR BD-rate estimate is **+19.34% final versus main**, over the common 40.272336–49.002062 dB interval, and **+32.28% final versus x264 medium**, over 42.362430–49.002062 dB. Positive values mean that final uses more bits at the same PSNR. These are small-curve interpolation estimates for one lossy input, not representative quality verdicts. They do not establish equal perceptual quality or a matched-quality speed ratio.

The calculation sorts each arm's three `(PSNR, kbit/s)` points, interpolates natural-log bitrate against PSNR with SciPy 1.18.1 `PchipInterpolator(..., extrapolate=False)`, and integrates each interpolant analytically with `.integrate(low, high)` over that pair's common quality interval. BD-rate is `100 * expm1((integral_final - integral_anchor) / (high - low))`; no point is extrapolated. Identical curves return 0%, and multiplying every rate by 1.3 returns +30%. The raw record includes both intervals, all nine calls, independent squared-error scores, and metric frame counts. Every new native reconstruction again matches both CoreAV and pinned FFmpeg exactly.

## Compression-efficiency recovery design

This amendment is committed before its implementation. The maintainer chose compression-efficiency recovery before SIMD after the bounded-search result in coreav-dev/coreav#446. The target is a single-digit PSNR BD-rate increase against the pre-change main, ideally below +5%, while retaining at least 80% of the bounded encoder's throughput. On A, the concrete reference target is at least 14 pictures per second. Report the remaining BD-rate against x264 as well. These are local reference targets, not a representative-cohort claim.

The complete candidate objective, quantization, entropy coding, reference state, deblocking, IDR analysis, legal motion vectors, and admission remain unchanged. Only proposal quality and the allocation of cheap search work change. The specification and the two reviewed notes above are the only implementation inputs. No new source study is required by this design; an unanswered implementation question would stop work for a separate study rather than invite source reading here. No SIMD, unsafe code, external encoder dependency, or default-build change belongs to this amendment.

### Hypotheses and experiment order

The prior step measurements separate two plausible losses. Bounding inter search changed A's QP-28 quality by about 0.65 dB and increased rate about 4%; bounding intra then added about 10% rate with little further PSNR change. This suggests improving transform-aware intra proposals before spending more complete inter evaluations. It does not prove which prediction or syntax decisions caused either loss.

1. Replace sample-domain SAD with a transform-domain ranking for the few intra predictions. For each 4x4 residual, use the separable four-point Walsh-Hadamard transform and half the sum of absolute coefficients, summed over the prediction extent. This SATD is a cheap encoder heuristic, not the normative reconstruction transform or an exact encoded-rate estimate. Include the known prediction-mode signalling length with the existing cheap QP-scaled penalty. Keep motion probing unchanged initially so the cost and benefit of this ranking are visible on their own.
2. Restore a bounded I4 proposal without restoring nine reconstructed trials per block. Visit the sixteen 4x4 blocks in normative order. Rank legal predictions by SATD plus their exact prediction-mode signalling length, reconstruct the one chosen prediction, and use those reconstructed borders for the next block. Compare the aggregate cheap I4 score, including its mode syntax, against the best cheap I16 score. Complete only the selected luma/chroma pair through the existing candidate evaluator. Keep the original exhaustive intra procedure for IDR and admission fallback. This retains the normal one-complete-intra-pair bound rather than adding a second full macroblock trial everywhere.
3. Improve motion and layout proposals independently if the intra steps do not reach the target. First compare SATD on a small retained set of already-probed vectors, including the normative predictor, rather than running the entire motion search in a more expensive metric. Alternative seeds are the admitted spatial neighbours and the whole-block proposal for split regions; they are search seeds, not changes to normative vector prediction. Exact motion-difference and layout syntax lengths remain part of each cheap score. Record the increased probe or prediction cap explicitly before measuring an adopted variant.
4. Test selective complete refinement only if cheap ranking leaves a material gap. The alternative is one second full layout evaluation for ambiguous cheap scores or large residuals, not a return to exhaustive layouts. An initial ambiguity condition is a score gap within one eighth of the best score; a residual condition is mean absolute residual above the cheap QP scale `1 << (QP / 6)`. These independently chosen heuristics are alternatives to measure, not imported constants or established correctness bounds. Record which condition actually triggers and its evaluation count. Retain skip's independent evaluation and the hard maximum of two regular complete inter evaluations.

Measure each implemented step separately against its current parent and the unchanged bounded-search arm. Keep only steps that improve the measured speed and BD-rate frontier; revert a slower, lower-quality or dominated alternative instead of hiding it inside a bundle. A later step may be combined with an earlier retained step, but its incremental effect must remain visible. The target does not justify changing the normative reconstruction or weakening a test. Stop with the smallest combination that reaches the reference goal rather than implementing every listed alternative.

### Exact syntax floors before construction

The complete objective is nonnegative sample squared error plus the fixed QP weight times complete syntax length. This permits a conservative lower bound before reconstructing a candidate. Every P-picture intra macroblock has a type code at least 5, a nonnegative chroma-mode code, and the current skip-run prefix. Counting `ue(5)`, `ue(0)`, and `ue(pending_skip)` therefore underestimates every legal intra candidate's rate; omitted mode, coefficient, pattern and QP fields have nonnegative length. If an admitted inter result already costs strictly less than that weighted floor, omit intra construction. Equality still evaluates candidates to preserve the full tie order. IDR and admission fallback remain unchanged.

For an inter proposal, its derived layout and motion syntax plus the regular macroblock's current skip-run prefix give the corresponding floor. Again, use strict excess against an admitted result, never a predicted distortion or an estimated coefficient rate. First verify complete stream identity with the same candidate policy and measure the saved work separately. This exact reduction may fund the optional second full layout evaluation for close cheap scores without spending the speed budget on candidates that cannot win.

The refinement alternative is to make both existing proposals eligible for complete comparison, while applying their exact syntax floors before each trial. This substitutes an objective lower bound for the heuristic score-gap trigger and still permits at most two regular full evaluations. It changes the eligible candidate set relative to the first-success policy; it is not claimed to preserve that policy's stream. Intra-only floor pruning is measured separately for identity before this refinement. A floor-rejected first proposal must not accidentally suppress the second proposal's admission fallback.

### Coalescing identical motion regions

A four-region proposal may assign identical vectors to adjacent regions. Before complete evaluation, compare its exact motion and layout syntax with admitted whole-block, horizontal-half, and vertical-half representations of that same 4x4 motion map. Only merge regions whose vectors are exactly equal; never average vectors or add motion probes. Re-derive each representation's normative motion differences and retain the cheapest syntax, with partition count and layout code resolving ties. Prediction samples, residual transforms, reconstructed samples and neighbour motion maps are unchanged for that proposal, so coefficient rate is unchanged. Fewer signalled partitions can also reduce the carried motion-count charge. This restores a representation already covered by forced conformance fixtures without restoring a new search tree or increasing the two-proposal full-evaluation cap. Measure it separately from complete refinement.

The absolute floor for any regular inter representation is the current regular skip-run prefix, `ue(0)` for the smallest type, and two zero motion differences. On proposal entry, an admitted result strictly below that weighted floor makes motion probes unnecessary, including probes for a split that could later coalesce. Count proposal entries separately from actual probes. A nonzero-distortion skip retains its actual distortion; this is an objective bound, not a near-zero-error shortcut.

Once both proposals are eligible for complete comparison, their cheap distortion rank affects evaluation order only. Compare ordering by exact syntax length against the transform-domain ordering, removing extra prediction and SATD work if stream identity and timing support that simplification. Deduplicate identical complete inter representations before evaluation. These changes must preserve the two-proposal refinement's chosen stream, apart from coalescing's separately measured effect.

For the motion-ranking alternative, retain the two best completed SAD probes and the normative predictor when it was admitted. Deduplicate that set and compare its prediction SATD plus the already-counted exact motion-difference length. This adds at most three transform-domain prediction comparisons per partition, or fifteen across the two search layouts, without changing the 23-probe SAD budget. The universal syntax floor can eliminate the entire stage when no regular result can win. Compare this alternative separately on both inputs rather than assuming that a richer proxy improves compression.

### Evidence and checks

Reuse A and its exact prepared bytes, the corrected x264 input declaration, QP 22, 28, 34, GOP 32, single-thread affinity, complete stream bitrate, sample-weighted YUV PSNR, and SSIM All. Use the same PCHIP log-rate method and common quality intervals, with no extrapolation. Retain the pre-change main executable, bounded-search executable, source and executable digests, per-call wall and CPU time, and complete metric frame inventories. Reuse recorded deterministic rate and quality where byte identities establish the same cell; label reused timing rather than presenting it as a new observation.

Add one real-motion input from media already stored in R2, separate from A and from low-motion B. Prepare sixty pictures at 30 pictures per second with admitted geometry, record any scaling and the prepared digest, and establish its distinct-picture count and temporal sample differences before drawing performance conclusions. Measure pre-change main, the bounded-search arm, each adopted step, and corrected x264 on that same input. B remains a low-motion sanity check only. No new media sourcing or GCP is needed.

If the second input's native and x264 three-point curves have no common PSNR interval, report that result as undefined rather than extrapolating. One additional x264 QP-40 anchor may provide a separately labelled four-point diagnostic comparison while retaining the required QP 22, 28, 34 cells. This adds quality coverage, not a substituted native sweep or a new acceptance rule.

Verify exact pinned-FFmpeg reconstruction for every newly encoded native cell, including complete picture counts. Prove raw x264 input passthrough before encoding and independently recompute PSNR from the identical reference samples. Keep metric work outside timing and retain the native driver's verification-and-IO timing qualification. Check SATD against a direct matrix definition on zero, constant, impulse, signed-extreme and deterministic residuals. Check legal intra modes, reconstructed I4 border dependencies, deterministic ties, proposal budgets, fallback, admission, and transactional retry. Keep the unchanged all-intra snapshot, existing forced syntax fixtures, three-decoder conformance, signed-off commits, and exact-head main-allowlist similarity guard.

## Recovery implementation and reference results

The final policy keeps the bounded I4 proposal, exact intra and inter syntax floors, complete comparison of the two eligible inter proposals, equal-vector region coalescing, and retained-vector SATD ranking. Normal search performs at most 115 SAD probe calls and fifteen retained-vector SATD comparisons, followed by at most two regular complete inter evaluations and the independent skip evaluation. Intra either loses the syntax-floor comparison without construction or completes one cheap-selected luma/chroma pair. IDR and admission fallback retain the complete original intra procedure. Coalescing can emit already-supported rectangular forms without searching new vectors. Equivalent complete representations are deduplicated.

The final inter proposals are ordered by exact syntax length. Once both are eligible for complete comparison, extra layout prediction and SATD ranking changed only evaluation order; removing that work preserved all six measured streams. Universal pre-probe bounds also preserved those streams. The separate intra-floor step preserved all six of its parent's streams. Strict equality still evaluates candidates, and tests cover skip-run code-length discontinuities. The committed implementation has 49 passing Rust tests, including the unchanged all-intra snapshot, prediction and coefficient equality after coalescing, reconstructed I4 borders, and the probe and evaluation bounds.

### Measured frontier

Each development entry below is one QP-28 timing call and a QP 22, 28, 34 PSNR BD-rate curve. Timings include native decoder verification and IO, have no interval, and are not the final paired timing comparison. The [raw recovery record](h264-encoder-efficiency-reference.json) contains every cell's bitrate, PSNR, SSIM, wall and CPU time, input and executable digests, complete metric counts, and common quality intervals.

| Policy | A pictures/s | A BD-rate versus old main | C pictures/s | C BD-rate versus old main |
|---|---:|---:|---:|---:|
| Initial bounded search | 22.375 | +19.34% | 27.545 | +14.61% |
| Bounded I4 with transform-domain intra ranking | 17.696 | +9.45% | 28.784 | +16.13% |
| Transform-domain layout ranking | 19.107 | +7.58% | 31.931 | +14.95% |
| Exact intra floor | 22.188 | +7.58% | 33.908 | +14.95% |
| Complete inter comparison with syntax floors | 25.072 | +7.42% | 33.589 | +15.58% |
| Equal-vector coalescing | 20.453 | +7.46% | 30.704 | +14.86% |
| Universal pre-probe floor | 38.393 | +7.46% | 34.017 | +14.86% |
| Syntax-only proposal ordering | 43.765 | +7.46% | 34.043 | +14.86% |
| Retained-vector SATD ranking | 40.295 | +7.27% | 33.136 | +14.41% |

Transform-domain intra ranking alone barely improved A's BD-rate, to +19.05%, so it was not retained as a standalone answer. Replacing the split's zero seed with the whole-block vector gave +9.46% on A and +15.63% on C after I4, and was rejected. Coalescing traded about 0.04 BD-rate points on A for about 0.72 points on C before the later bounds removed much of its cost; its complete motion map and per-proposal reconstruction are unchanged. The final retained-vector comparison improves both curves at a small timing cost. These are measured tradeoffs across both inputs, not a claim that every isolated stage improves every cell.

A local build overlapped the first universal-floor timing series. Those times were discarded and the same binary was rerun without a concurrent build; the table uses that clean series. Its streams and quality metrics were identical. No rerun was made to rescue a quality result.

### Final paired timing and rate-distortion estimates

Five alternating QP-28 pairs compare the bounded and final executables on CPU 0, after builds finished. Every repeat preserves its curve cell's complete stream and reconstruction. Median pictures/s and observed minimum–maximum are:

| Input | Initial bounded search | Final | Throughput change |
|---|---:|---:|---:|
| A | 24.654 [22.865–25.555] | 44.432 [42.183–49.017] | +80.22% |
| C | 35.830 [34.827–38.239] | 33.513 [31.122–34.094] | −6.47% |

These are local reference figures from the same shared two-CPU AMD EPYC host and optimized release build with debug information. They are not isolated encoder-kernel timings or statistical performance claims. Both inputs remain within the allowed 20% throughput loss, and A exceeds the 14-pictures/s reference floor. Main's A cells are reused from the prior reference; C's old-main cells are new calls. Final curve validation uses the committed implementation and is byte-identical to the last development candidate.

| Input | Initial bounded BD-rate versus old main | Final BD-rate versus old main | Final BD-rate versus x264 medium |
|---|---:|---:|---:|
| A | +19.34% | +7.27% | +18.77% |
| C | +14.61% | +14.41% | Undefined for the primary three-point anchor; +271.23% with the extra anchor |

A reaches the single-digit target, but not the below-5% aspiration. C shows little compression recovery, so the A improvement must not be generalized to it. C's bounded and final curves do not overlap x264's primary QP 22, 28, 34 quality range. Its separately labelled comparison adds x264 QP 40, giving the final curve a common 35.465954–38.715978 dB interval. A uses the primary three-point anchor over 42.362430–49.013435 dB. Final versus old main uses 40.272336–49.013435 dB on A and 33.419766–38.715978 dB on C. Pair-specific intervals differ; these percentages are not transitive. No extrapolation or matched-quality speed claim is made.

The final operating points are:

| Input | QP | kbit/s | PSNR, dB | SSIM |
|---|---:|---:|---:|---:|
| A | 22 | 458.888 | 49.013435 | 0.999254 |
| A | 28 | 365.804 | 45.371226 | 0.998398 |
| A | 34 | 273.932 | 39.285981 | 0.995533 |
| C | 22 | 340.768 | 38.715978 | 0.974656 |
| C | 28 | 162.852 | 35.683142 | 0.966172 |
| C | 34 | 77.512 | 32.791933 | 0.953264 |

All inputs are already lossy. A's preparation is unchanged. C is resized from 960x812 to 480x406, padded at the bottom to 480x416, and converted from 1440/49 to 30 pictures per second. Its sixty prepared pictures contain 59 distinct images; mean adjacent luma absolute difference is 0.724 and the maximum is 4.530. The prepared digest and exact conversion dimensions are in the raw record. B remains a low-motion sanity check only: at QP 28, final preserves the bounded encoder's complete stream, 126.776 kbit/s, 44.152652 dB PSNR and 0.992678 SSIM. Its single-call timing is not part of the paired comparison.

Every newly encoded native curve and sanity cell matches pinned FFmpeg reconstruction exactly across all sixty pictures, and the repeated timing outputs match their verified cells. x264 uses the corrected input-before-output colour declaration, with digest-matched raw passthrough. Independent raw squared error confirms every PSNR; both metrics retain sixty per-picture records. The raw record names the implementation commit and executable digests; x264 has no encoder-side reconstruction check, so its `oracle_exact` field is null rather than a failed comparison. No new source study, SIMD, unsafe code, or default-build dependency was introduced.
