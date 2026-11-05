# H.264 encoder deblocking and P pictures

Status: Deblocking and one-reference P pictures are implemented. The bounded motion-search amendment below changes mode selection, not normative reconstruction or the fixed-QP API. This extends the opt-in scalar [intra encoder](h264-encoder.md), not its evaluation protocol. There are no quality or speed claims. The existing all-IDR, fixed-QP, CAVLC path keeps deblocking disabled unless the caller explicitly enables it. No command-line product or default-build dependency changes.

## Sources and delivery order

The normative source is Rec. ITU-T H.264, version 15, 08/2024. Clauses 6.4, 7, 8.2, 8.3, 8.4, 8.5, 8.7, and 9.2 define neighbours, syntax, reference pictures, prediction, reconstruction, filtering, and CAVLC. Tables and equations come from the specification. Existing CoreAV scalar kernels supply normative sample arithmetic. No other encoder or decoder implementation source is opened during implementation. The later search-efficiency work also uses the reviewed technique note listed under Prior art. FFmpeg and JM are black-box test executables only.

The implementation is a dependent stack with a plan commit before code in each layer:

1. Add opt-in deblocking of complete reconstructed IDR pictures. This document is that layer's first commit.
2. Add a fixed GOP with one short-term reference. Implement P_L0_16x16 and P_Skip first, verify complete sequences, then add 16x8, 8x16, and 8x8 partitions with 8x8, 8x4, 4x8, and 4x4 subpartitions. Begin the layer with a focused implementation and verification checklist in this document.
3. Add a design-only note for a subsequent rate-control slice. Do not implement rate control or CABAC.

Each layer remains open until its conformance checks and independent review are complete. These changes do not authorize a stage-gate verdict or a merge.

## Settings and ownership

`Settings` has an explicit `Deblocking` choice. `Disabled` preserves current slice syntax and reconstructed bytes. `Enabled` writes `disable_deblocking_filter_idc = 0` with both slice offsets zero. Nonzero offsets and multiple slices are out of scope. Existing callers select `Disabled` explicitly. The later P-picture layer adds `Gop::AllIntra` and a checked fixed IDR interval. Neither feature is inferred from input content.

The encoder remains sans-IO, safe scalar Rust. `Scalar` is selected directly, never runtime dispatch. Production code uses no unsafe, SIMD, unchecked indexing, `unwrap`, or `expect`. Geometry, QP, GOP interval, frame count, output budgets, and allocation sizes are validated before use. Fixed-size candidate tiles and fallible picture buffers retain the current allocation discipline.

The picture encoder returns a completed coded reconstruction and slice bytes privately. Visible reconstruction is cropped only after filtering. The sequence owner commits its counters, next picture identity, reference, and final macroblock motion-vector count only after every NAL and byte-limit check succeeds. Failure preserves the previous reference and all sequence state, including that count. The previous reference and one tentative current picture are the only full coded pictures retained during encoding. Candidate search does not allocate full pictures or a picture per candidate.

## Deblocking after reconstruction

Finish all macroblock reconstruction before filtering. Intra prediction must read unfiltered samples, even when an earlier macroblock will later be filtered. A picture-level pass avoids a second unfiltered reference picture and prevents filter output from entering same-picture intra prediction.

Process macroblocks in raster address order. For each component of each macroblock, process its vertical edges from left to right, then horizontal edges from top to bottom, as clause 8.7 specifies. Skip picture-boundary edges. Filter the padded coded picture before exposing its visible crop or retaining it as an inter reference.

For the initial all-intra layer, macroblock-boundary strengths are 4 and internal-edge strengths are 3. Luma has edges at offsets 0, 4, 8, and 12. Each 4:2:0 chroma component has edges at offsets 0 and 4. Fixed QP makes the average QP equal to the component QP. Map chroma QP before selecting thresholds. Use Tables 8-16 and 8-17 for alpha, beta, and tC0, and the checked `EdgeMut` views with `Scalar` filtering for clauses 8.7.2.3 and 8.7.2.4. Do not expose decoder-private state to the encoder.

The inter layer retains one compact record per macroblock containing its intra/inter identity, luma nonzero-block mask, and quarter-luma motion vectors on the 4x4 grid. Boundary strength is derived from the actual selected blocks. An intra boundary takes precedence, then nonzero luma coefficients give strength 2, then a motion-component difference of at least four quarter samples gives strength 1, otherwise 0. All inter blocks use the same reference picture. Chroma uses the corresponding luma strengths rather than chroma coefficient counts. The record buffer is bounded by validated coded geometry.

## One-reference picture sequence

Keep progressive 8-bit 4:2:0, Constrained Baseline, CAVLC, one slice, fixed QP, and the existing level envelope. The configured interval starts with IDR, followed by P reference pictures until the next IDR. An interval of one is all-IDR. No B pictures, list modification, weighted prediction, long-term references, scene-cut detection, or open GOP is added.

Keep `pic_order_cnt_type = 2`, four-bit `frame_num`, and `max_num_ref_frames = 1`. IDR resets `frame_num` to zero. Each P picture advances it modulo 16, including wrap within a GOP. The one-entry short-term reference list selects the preceding successfully encoded picture. Non-IDR reference marking uses sliding-window operation with `adaptive_ref_pic_marking_mode_flag = 0`. IDR discards the previous reference. Alternate `idr_pic_id` at IDR boundaries, not on P pictures.

Repeat SPS and PPS before IDR pictures. Keep the existing fixed three-NAL access-unit shape initially by repeating them before P pictures too. A P slice uses a type-1 reference NAL, `slice_type = 5`, one active list-0 reference, and no reference-list modification. Parameter sets remain identical for the sequence. The access-unit and cumulative byte budgets count all repeated headers. Frame count and presentation order must agree in every decoder, including the final flush.

## Motion prediction and search

Represent motion as signed quarter-luma-sample components. Derive the motion predictor and reference availability from clause 8.4.1.3, including top-right substitution, intra neighbours, partition-specific 16x8 and 8x16 choices, and already selected subpartitions. Emit signed `mvd_l0` relative to that predictor. Validate its syntax range under clause 7.4.5.1 separately from the derived-vector limits in clauses A.3.1(k) and A.3.1(l). At level 5.1, derived horizontal components must be in [-8192, 8191] and vertical components in [-2048, 2047], in quarter-luma-sample units. Apply these bounds to every integer candidate, fractional refinement, predictor candidate, and derived skip vector. Reject an out-of-range candidate without clipping or changing its predictor. Reference index zero is implicit because only one reference is active.

Derive P_Skip independently by clause 8.4.1.1. A missing left or top macroblock, or either neighbour with reference zero and zero motion, forces zero motion. Otherwise use the specified predictor. Skip is its prediction with no residual, never a searched vector with residual syntax removed. Commit skip motion and zero coefficient counts into subsequent neighbour state.

The search is our own deterministic bounded search, not an H.264 requirement. At the maintainer's direction, replace exhaustive full-RDO motion trials with cheap prediction-cost search and bounded full-RDO finalists. The former 33-by-33 search performed complete macroblock transforms, reconstruction and syntax counting for every motion trial and repeatedly recomputed unchanged regions. The amendment below deliberately permits different selected vectors and encoded bytes; conformance and transactional admission remain unchanged.

For each partition, score luma prediction SAD plus `2^(floor(QP/6))` times the signed Exp-Golomb bit count of its motion-vector difference. This uses SAD-scale rather than squared-error-scale motion cost. Admit the vector and its difference before scoring. Seed zero, the exact normative predictor and its nearest integer-sample position, including valid predictor seeds outside the ordinary search square. Resolve equal cheap scores by fewer motion bits and then lexicographic vector order.

Use an eight-neighbour square pattern at integer luma steps 16, 8, 4, 2 and 1, with at most two moves per step. Each pass tests neighbours around a fixed best integer centre; stop that step when the centre does not move. Ordinary integer probes stay within [-16, 16] in each component. This covers diagonal as well as axial motion with at most 80 integer pattern probes, rather than exhaustively testing 1,089 positions. Around the best integer result, score eight half-sample neighbours, then eight quarter-sample neighbours around the best result of the half-sample stage. Retain centres and reject out-of-range vectors rather than clipping them.

Keep the best four distinct cheap candidates across all stages. Run complete macroblock RDO on those four and on protected zero and exact-predictor seeds, with duplicates removed: at most six full trials per searched partition. Four ranked candidates preserve alternatives to the cheapest SAD result without restoring the expensive broad RDO sweep; the two protected seeds retain deterministic stationary and predictor fallbacks. This is a runtime/quality choice, not a claim that SAD order matches full RDO order. Report actual quality and bitrate changes against the former search.

Prepare the unchanged part of a trial once per searched partition. Reuse its prediction pixels, luma quantized levels and reconstructed luma blocks, and unaffected chroma transforms and AC levels. For a finalist, predict and transform only the changed partition and overlapping chroma blocks. Chroma DC combines all four blocks, so recompute that coupled DC decision and its required chroma reconstruction. Complete candidate syntax, normative limits and distortion are still evaluated before selection. Fixed-size caches are local to one macroblock search, allocate no full pictures and cannot alter committed state. A cached finalist must match the uncached complete evaluation exactly.

Use edge-replicated windows from the complete coded reference. Negative coordinates use Euclidean division and remainder, not truncation toward zero. Call `Scalar::luma_mc` for the six-tap half-sample and quarter-sample process in clause 8.4.2.2.1. Call `Scalar::chroma_mc` for clause 8.4.2.2.2 using the same vector in eighth-chroma-sample units. Keep interpolation windows and destination tiles fixed-size. No original source picture becomes a reference.

Evaluate each full-RDO finalist through forward transform, quantisation, normative inverse reconstruction, and exact candidate syntax counting, reusing independent unchanged work as described above. Ordinary inter luma blocks retain their DC coefficient in the 4x4 transform. Chroma retains its separate 2x2 DC process. Select by coded-picture Y, Cb, and Cr squared error plus `2^(floor(QP/3))` times syntax bits, matching the existing integer penalty. Ties prefer fewer bits, then skip, then larger partitions, then lexicographic motion and specification mode order. These are reproducibility choices, not a quality claim.

Partition search is greedy in syntax order. Each partition searches with the selected earlier partitions committed into a candidate-local motion map. Compare each complete macroblock candidate, including intra candidates, only after all its luma and chroma residuals and exact syntax have been counted. Rejected candidates cannot change committed motion, CAVLC counts, or reconstructed samples. Intra prediction in P pictures obeys `constrained_intra_pred_flag = 1`, so inter neighbours are unavailable to intra modes.

Before comparing candidate costs, admit partitions under clause A.3.1(m) and Table A-1's level-5.1 `MaxMvsPer2Mb = 16`. Count the normative `MvCnt` after prediction, not distinct vector values or nonzero vectors. Intra macroblocks contribute zero, P_Skip and P_L0_16x16 contribute one, 16x8 and 8x16 contribute two, and P_8x8 contributes the sum of its subpartition counts. Equal vectors in separate partitions still count separately. Admit a macroblock only when its count plus the preceding selected macroblock's count is at most 16. Prune a partial partition layout once its accumulated count exceeds the remaining budget. An inadmissible partition layout cannot win even if its distortion or syntax cost is lower. Intra candidates remain available with count zero. If no representable candidate survives, fail the frame rather than emit a level violation.

The picture's tentative previous-count state starts from the last macroblock of the preceding successful picture, not zero. Every selected macroblock updates that tentative state. This enforces the bound for every consecutive pair in decoding order, including row transitions and the last-P/first-P picture boundary. Keep the preceding count until a successful IDR's intra macroblocks replace it with zero, which also conservatively covers an IDR boundary. Return the final count with the tentative picture and commit it only on complete access-unit success. Rejected candidates and failed frames cannot change the sequence's carried count.

Also admit candidates under clause A.3.1(n)'s limit of 3,200 bits of `macroblock_layer` data. The syntax traversal shared by counting and emission owns this bound for intra and inter candidates. Count the complete macroblock syntax and residuals, but not the preceding `mb_skip_run`. Reject an over-budget candidate before cost comparison and retain representable alternatives. If none survive and at least one exceeded this bound, return `Error::MacroblockLimit` without publishing output or advancing reference, GOP, byte-accounting or motion-count state. The independent conformance reader checks the same ceiling from emitted bits, not encoder counters. Fixed QP can refuse dense content; I_PCM or per-macroblock QP fallback remains later work, as recorded in the [bit-limit design](h264-encoder-macroblock-limit.md).

## Slice data and skip runs

Reuse one residual syntax traversal for counting and emission. Add the inter coded-block-pattern mapping from Table 9-4, the P macroblock-type offset for intra macroblocks, partition prediction syntax, and CAVLC `mb_skip_run`. Uncoded and skipped blocks reset all affected coefficient-count neighbours.

Keep a pending skip-run count in the slice writer. Before a non-skipped macroblock, emit that run, including zero, followed by its macroblock syntax. At the end of the slice, emit a nonzero trailing run before RBSP trailing bits. For local decisions, compare the exact length of the currently terminated candidate prefix, including the current run. The common prior-prefix length cancels between candidates. This is a greedy prefix cost, not a claim of optimal whole-slice bit allocation.

## Verification and completion predicates

Correctness remains independent of timing and quality references. All conformance matrices run in GitHub Actions. The maintainer separately authorizes small local reference comparisons on already available media, including a longer sequence at 640x360 or larger; they do not establish a judged speed or quality verdict. Use the pinned FFmpeg in `scripts/oracle-ffmpeg.env` and pinned black-box JM in `scripts/jm-decoder.env`.

The deblocking layer requires all of the following:

- The disabled setting preserves existing synthetic output bytes and passes the unmodified intra conformance suite.
- Enabled-filter syntax has zero offsets. Hand-derived edge fixtures exercise strong and normal filtering, threshold equality, both directions, all QPs, chroma QP transitions, and picture boundaries.
- Complete filtered intra sequences, including cropped coded edges and independent chroma patterns, must match the encoder's filtered reconstruction in every visible Y, Cb, and Cr sample in CoreAV, pinned FFmpeg, and pinned JM separately. A fixture must demonstrate that enabling the filter actually changes reconstructed samples.
- Default features still link no encoder kernels. Formatting, full-feature Clippy, unit tests, and repository policy checks pass.

The P-picture layer additionally requires all of the following:

- The syntax reader verifies reference marking, active reference count, P macroblock forms, skip-run placement, GOP boundaries, frame-number wrap, and parameter-set consistency.
- Synthetic still, integer translation, fractional translation, opposing partition motion, nonuniform chroma, and abrupt picture changes exercise selected inter, skip, partition, and intra paths. Assertions establish that each intended syntax path is actually selected or explicitly constructed by a syntax fixture.
- Every supported luma and chroma interpolation phase, negative displacement, edge extension, constrained-intra neighbour case, motion-predictor fallback, and partition shape has a deterministic test.
- Cheap-search tests bound probe and full-trial counts, retain distinct finalists and protected seeds, preserve deterministic ties and motion boundaries, and compare cached finalist levels, syntax cost and complete reconstruction against uncached evaluation. Existing all-intra snapshots, refusal/rollback tests and conformance checks stay unchanged.
- Candidate-admission tests accept a consecutive `MvCnt` sum of 16 and reject 17, including a 16-vector macroblock followed by P_Skip. A forced-cost fixture makes the forbidden partition layout cheaper and verifies that mode decision still chooses an admissible layout or intra. Cover row transitions, last-P/first-P picture boundaries, and failed-frame retry without resetting the carried count. Independently derive `MvCnt` from emitted macroblock and sub-macroblock syntax in the conformance reader and require every consecutive pair's sum to be at most 16 across picture boundaries. A synthetic stream with a sum of 17 must fail that reader even if all decoder samples match.
- Motion-boundary fixtures cover both derived-vector endpoints, one quarter sample beyond each endpoint, and independently invalid `mvd_l0` syntax. Check derived skip vectors and fractional refinements as well as integer candidates.
- Forced partition fixtures emit every supported macroblock and sub-macroblock layout at QPs 10, 16, 28, 40, and 51, with filtering both enabled and disabled. Test-only export records the actual stream and encoder reconstruction. Actions independently parses all 80 streams and compares their complete visible planes with CoreAV, pinned FFmpeg, and pinned JM. A macroblock-layer ceiling regression rejects an oversized cheaper candidate without counting its skip-run prefix.
- Every output at every tested QP decodes completely. All three decoders match the encoder's complete visible Y, Cb, and Cr samples for every frame in order, with exact frame count, crop, profile, level, and metadata. One mismatch, warning, missing frame, oracle failure, or incomplete output fails conformance.
- A rejected frame, allocation failure, or output-budget failure cannot advance GOP state or replace the reference. Retrying a valid frame matches a fresh encoder at the same successful prefix.
- The similarity guard passes on the exact public head against the main allowlist. Its result records the checked commit without exposing comparison source. Every commit has a DCO sign-off.

## P-picture implementation checklist

This is the first commit of the P-picture layer. The normative rules and search choices above remain its contract. Implement and verify these units in order:

1. Add `Gop::AllIntra` and a checked fixed IDR interval to settings. Keep all existing callers explicitly all-intra. Move the completed coded picture into the encoder's one-reference state only after every output check passes. Keep IDR identity, modulo-16 frame number, and the preceding macroblock's `MvCnt` transactional.
2. Add P reference-slice headers, P_L0_16x16, derived P_Skip, skip-run emission, and the inter coded-block-pattern mapping. Keep normative motion prediction separate from the deterministic integer, half-sample, and quarter-sample search. Reconstruct every winning candidate through the scalar kernels.
3. Extend candidate-local motion maps to 16x8, 8x16, and P_8x8 with every specified subpartition shape. Enforce the consecutive-count budget before cost comparison and retain the zero-count intra fallback. Include constrained-intra neighbour availability and inter deblocking strengths derived from the selected motion and nonzero luma blocks.
4. Add a separate test-only sequence driver and untimed synthetic Actions conformance. Independently derive `MvCnt` from emitted syntax, including the last-P/first-P boundary, rather than trusting encoder instrumentation. Verify frame-number wrap, IDR boundaries, skipped tails, integer and fractional motion, partition coverage, crop, reference rollback, complete three-decoder sample equality, and the unchanged all-intra byte snapshot.

No rate-control or CABAC implementation belongs in this layer. Its completion requires the exact-head guard against the main allowlist, not only a proposed allowlist preview. The layer remains unmerged until its independent review and the external stage-gate decision are complete.

## Pre-study search-efficiency design

This is a proposal, not implemented behavior. Our bounded-build profile identifies partition search, prediction access, cheap scoring, finalist evaluation, and transform arithmetic as targets. Their inclusive costs overlap. The design below comes from that profile, the specification, and first principles, before studying other encoders.

Keep fixed QP, the existing coding tools, safe scalar Rust, and all normative admission and transactional failure rules. Areas 1 and 3 may change selected modes and bytes. Areas 2, 4, and 5 must preserve complete candidate costs, refusals, and reconstruction for identical inputs. Keep the current bounded search as the comparison arm.

### 1. Partition pruning and early skip

Study question: Which inexpensive evidence justifies omitting partition searches without losing useful motion boundaries or residual coding choices?

Our design tests normative P_Skip and whole-macroblock motion first. Start with an early-skip experiment restricted to admissible skip with zero Y, Cb, and Cr distortion, rather than a content-independent error threshold. Otherwise rank split layouts by prediction error plus motion cost before full RDO. Retain the whole layout and the best ranked split family, and refine a subgroup only when its cheap score improves on its unsplit parent. Keep an admissible intra fallback. These are heuristic selection policies, not proofs of optimal RDO cost. Compare their bitrate and quality against searching every admitted layout. Clauses 8.4.1 and A.3.1 still determine legal prediction and motion counts.

### 2. Padded reference access and interpolation reuse

Study question: When does reusing edge extension and fractional samples save more work than the extra storage and cache traffic cost?

Our design copies an edge-replicated block window once per local search region and validates its geometry before row access. Share that window across probes and finalists. Derive horizontal, vertical, and diagonal half-sample results lazily within the window, then form quarter-sample predictions with clause 8.4.2.2.1's rounding and clipping order. Keep intermediate precision where the specification requires it. Key reuse by reference identity, region, component, and phase. Keep caches bounded and fallible, discard them on reference replacement, and use the existing sampled path for valid vectors outside the cached region. Begin with local windows rather than extra full-picture planes. Chroma retains clause 8.4.2.2.2's separate interpolation.

### 3. Cheaper motion scoring and early termination

Study question: Which predictor ordering and prediction-cost metric retain good RDO finalists with fewer probes and pixel operations?

Our design orders the existing zero, predictor, and rounded-predictor seeds before the bounded spatial pattern. Compute signed Exp-Golomb motion length arithmetically, with equality tests against the existing bit counter. Accumulate SAD by rows and stop an ordinary probe only when its partial SAD plus motion cost strictly exceeds the current retained-candidate cutoff. Equality must still respect deterministic ties. Protected zero and predictor candidates remain eligible for full RDO. Compare the current SAD ranking with a small SATD reranking stage before fractional refinement, without treating either metric as a lower bound on final RDO. Keep the current probe and finalist caps until measurements justify changing them.

### 4. Incremental finalist evaluation

Study question: Which candidate dependencies permit exact reuse beyond the existing per-partition unchanged-region cache?

Our design retains selected block predictions, transforms, levels, and distortion in macroblock-local state. A trial replaces only affected blocks and invalidates dependent motion differences, coefficient-neighbour counts, and syntax costs. Recompute coupled chroma DC and every reconstruction it changes. Initially keep the complete shared syntax traversal for final counting and MacroblockLimit admission. Reuse syntax fragments only after identifying all their neighbour dependencies and proving equality with complete counting. Keep trial changes private until selection, and preserve uncached evaluation as a test oracle for every partition shape and refusal path. Clauses 8.4, 8.5, 9.2, and A.3.1 remain the contract.

### 5. Transform and quantization arithmetic

Study question: How much generic arithmetic can be removed while retaining exact levels, inverse samples, and coefficient-range refusals?

Our design factors each fixed integer basis into row and column operations rather than evaluating every two-dimensional basis product. Precompute quantization denominators and inverse scales for the picture's luma and mapped chroma QPs. Preserve the current signed rounding, tie behavior, separate DC handling, intermediate precision, and range checks. A zero-block shortcut must establish that every relevant level, including coupled DC, is zero. Compare specialized scalar results with the generic transform and quantizer across all QPs, boundary coefficients, and refusal cases. Clause 8.5 governs normative inverse reconstruction. Changing the quantizer's quality policy is separate from this arithmetic rewrite.

### Comparison and isolation

Measure each area separately before combining it. Report actual probes and full trials beside full encode time, complete reconstruction agreement, bitrate, PSNR, and SSIM on identical inputs and settings. A heuristic need not preserve the previous bitstream, but every output must pass the unchanged conformance and rollback checks. Exact rewrites must also match their complete-evaluation reference. Adopt a studied technique only after comparing it with this design in the same run. No SIMD, unsafe code, rate control, or default-build change belongs here.

### Prior art

The reviewed [search-efficiency technique note](../technique-notes/h264-encoder-speed.md) compares this design with x264 core 165 at `0480cb05fa188d37ae87e8f4fd8f1aea3711f7ee` (GPL-2.0-or-later) and OpenH264 v2.7.0 at `1bca62311e447fd9c39fc93de2c4b96cd5ce971c` (BSD-2-Clause). It supplies prose about avoided work, not code or measured optimization results. The implementation session uses that note without opening either source. The independently derived choices above remain the starting arm; competing note-derived choices require same-run measurements before adoption.

The implementation context has read no other encoder source and accepts no unreviewed study report. A separate study compares this committed design with pinned implementations. Its independently reviewed, committed prose note is the only source-derived input a fresh implementation context may receive.

## Search-efficiency implementation

The first implementation takes three bounded changes from the design above. It stops further inter search after an admitted normative P_Skip has zero distortion in all three components. It still compares that skip with the already evaluated intra candidates. This is a search policy, not a proof that no omitted candidate could have a lower syntax cost. Any nonzero luma or chroma error retains the existing bounded partition search.

Reference prediction uses a checked read-only region directly when the complete interpolation footprint lies inside the coded reference. Integer SAD similarly reads checked rows. At edges and for outlying predictor seeds, the existing sampled edge-extension path remains the fallback. Unlike a persistent padded-window or half-sample cache, this alternative adds no preparation, reference lifetime, or storage. It preserves every prediction and cheap score. Fractional interpolation itself is unchanged.

The forward 4x4 transform and luma DC Hadamard use separable row and column arithmetic derived from their existing bases. Quantization, signed ties, coefficient admission, inverse reconstruction, and chroma DC remain unchanged. Basis-position tests include signed extremes; generic-matrix comparisons cover DC levels and refusals at every admitted QP.

Each change is compared separately with the unchanged bounded encoder, then combined in the same reference run. The implementation does not add heuristic split pruning, SATD, score cutoffs, reciprocal quantization, interpolation caches, or further incremental finalist state. Their storage and quality tradeoffs remain open rather than being selected from the technique note without measurement. The existing full three-decoder conformance matrix and exact-head encoder similarity guard still apply.

## Partition-search cost design

This is the design for the next speed round, committed before its separate study. It does not change encoder behavior. The starting implementation already has bounded motion probes, unchanged-region preparation, exact zero-distortion skip termination, checked interior reference access, and separable transforms. The first comparison arm retains its complete search order and encoded bytes. No unsafe code, SIMD, rate control, new coding tools, or default-build change belongs in this round.

### Local profile

One reference-only run profiles the first four prepared pictures of an existing clip at 640x480, QP 28, fixed GOP 32, and enabled deblocking with zero offsets. This is one IDR picture and three P pictures, not the earlier 60-picture throughput workload. The prepared 60-picture YUV420 input has SHA-256 `756fa818691a60fa95f987bc606b00c95776a81142e7500cb39fac263cae9583`. Its preparation differs from the earlier reference bytes, so this profile does not establish a throughput change against that reference.

The source is main at `a1b5c6b6b6dc32ba071913993404f849a827770a`. The encoder and its H.264 kernel and decoder sources are unchanged from `fddcb94a6f7e38dfa058cd6601ca6f45b20cb83e`. The host exposes two AMD EPYC logical CPUs. The driver runs on CPU 0, with Rust 1.99.1 and an optimized release build with debug information. FFmpeg 9.0.2, build `n9.0.2-17-g2a571b6068`, prepares the input and independently decodes the output as a black box.

Build with `CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release -p coreav-h264-encoder --features full --example p`. Prepare 60 pictures using `ffmpeg -i input.webm -vf fps=30 -frames:v 60 -pix_fmt yuv420p -f rawvideo input.yuv`, then copy the first 1,843,200 bytes into `profile.yuv`. Profile with `perf record -F 499 -e cycles:u --call-graph dwarf,16384 -- taskset -c 0 target/release/examples/p 640 480 28 4 profile.yuv out.h264 recon.yuv decoded.yuv true 32`. Use `perf script -F period,ip,sym,dso` to group each sample once by its search-stage ancestor, including inline frames, and sum event periods rather than counting samples equally.

The run records 2,285 samples with no lost samples. The driver exits successfully and emits four pictures. Both its CoreAV decoder and pinned FFmpeg reproduce all 1,843,200 reconstruction bytes. The measured wrapper takes 4.97 seconds, including profiling, decoder verification, and file writes. That duration is not an uninstrumented encode-speed figure.

| Disjoint stage | Share of sampled process cycles | Share within partition search |
|---|---:|---:|
| Cheap motion search | 39.04% | 44.03% |
| Full-RDO finalists | 37.46% | 42.25% |
| Unchanged-region preparation | 10.49% | 11.83% |
| Remaining partition bookkeeping | 1.68% | 1.89% |
| Outside partition search | 11.33% | Not applicable |

Partition search accounts for 88.67% of sampled process cycles. Within the cheap-search row, fractional prediction takes 14.48 percentage points of the process total, visited-candidate lookup takes 8.53, motion-difference bit counting takes 2.88, and SAD plus remaining search work takes 13.15. Within the finalist row, coupled chroma reconstruction takes 8.99 points, changed-region prediction takes 4.32, complete syntax and cost evaluation takes 8.65, and residual transforms plus other finalist work takes 15.50. These subdivisions are disjoint within their parent row. This short sample identifies work to test, not a measured speedup ceiling or evidence that a particular rewrite wins.

### 1. Can candidate lookup stay exact without scanning unused entries?

Yes. The current visited list has 99 optional slots, but each lookup traverses the complete array. Keep one fixed-capacity dense prefix of scored vectors and search only its initialized entries. Preserve the existing four-entry ranking, protected seeds, returned cached score, and tuple tie order. This changes storage and traversal, not the candidates visited. Compare it with the current array before adding a coordinate-indexed lookup. A coordinate table needs initialization and an outlying-predictor path, which may cost more than a short dense scan. H.264 clauses 8.4.1 and 7.4.5.1 still govern vector and difference admission.

### 2. Can a probe stop before computing every pixel while preserving search decisions?

Yes, when a lower bound proves that the probe cannot affect any consumer of its score. Clause 9.1 permits deriving the signed Exp-Golomb length directly instead of passing motion differences through the generic bit counter. Add that exact motion cost before accumulating nonnegative row SAD. Stop only on strict excess, preserving equal-cost ties.

The cutoff must protect more than the fourth ranked candidate. Integer probes can move the best integer search centre even when fractional seeds already occupy the ranking. Fractional probes can move their stage centre. Use a bound that cannot improve the full ranking or the active centre, and do not cut off while the required ranking is incomplete. Fully score protected seeds. A visited entry must distinguish an exact score from an abandoned probe, never return a partial sum as a cached score, and recompute if a later consumer cannot safely reuse its rejection. Compare the complete probe trace, ranking, centres, finalists, and output against the uncut evaluator. SAD is not a lower bound on full RDO, so this cannot reject a full candidate by its final distortion-and-rate objective.

### 3. Can preparation survive selection of the preceding partition?

Yes, if reuse follows the actual predicted blocks rather than the layout name alone. The current preparation reconstructs the unchanged region again for every searched partition. Retain macroblock-local predictions, luma levels, reconstructed luma, and chroma transforms and AC levels for the current base motion map. Each finalist replaces the blocks its partition overlaps. Selection advances that local base, while rejected trials cannot alter it. A layout transition revalidates the retained blocks against the new motion map, including the zero vectors of unsearched regions.

Keep the complete shared syntax traversal and candidate admission initially. Clauses 8.4.1, 8.5, and 9.2 make motion differences, coefficient-neighbour counts, and coded-block patterns dependent on more than one retained block. Recompute coupled chroma DC and every reconstruction it affects, even where AC levels remain unchanged. Reuse a finalist's retained fractional prediction only when its reference, position, extent, component, and vector match. Keep the uncached evaluator as the test oracle for complete candidate keys, coefficients, reconstruction, refusals, and rollback. This design removes repeated preparation without claiming independent chroma-block costs.

### 4. Is interpolation reuse worth extra reference storage?

The profile makes it worth testing, but does not select a cache. Compare the existing checked reference access with a bounded macroblock-local window shared by probes and finalists. Prepare edge extension once and derive needed half-sample values lazily. Retain the precision, rounding, clipping, and negative-coordinate rules of clause 8.4.2.2.1. Chroma follows its separate clause 8.4.2.2.2 process. Valid predictors outside the ordinary search window use the existing sampled path.

Prefer bounded local reuse over three additional full-picture half-sample planes until a same-input comparison shows that their preparation and memory traffic pay for themselves. Retaining predictions for ranked finalists is another comparison arm that avoids populating phases nobody uses. Reference replacement invalidates all retained data. Test every luma and chroma phase, partition extent, and picture edge against the existing predictor before timing it.

### 5. Should cheap scores decide which partition layouts never receive RDO?

Not in the byte-identical arm. Clauses 7.3.5 and 8.4 permit the supported layouts, but do not require an encoder to search all of them. Omitting a legal layout is a quality policy, not an exact consequence of a low whole-block SAD. The same applies to accepting a nonzero-distortion skip before searching alternatives. Keep the current exact-skip rule, layout order, greedy subgroup choices, motion budget, and finalist caps while testing the exact changes above.

If those changes leave layout count as the dominant cost, compare the earlier design's whole-block-plus-best-split policy separately. Record the omitted searches, complete output bytes, bitrate, PSNR, and SSIM beside encode time on identical prepared pictures. Do not mix its output changes into an exact optimization or infer a quality win from equal numeric QP.

### Comparison and study boundary

Test each exact change separately against main before combining accepted changes. An A/B uses identical input bytes, settings, build flags, host, and affinity, and checks complete bitstream and reconstruction equality. Preserve normative coefficient and vector limits, the consecutive `MaxMvsPer2Mb` budget, the macroblock bit ceiling, and transactional failures. A technique from a later reviewed note must beat this independently derived design or another comparison arm in the same run before adoption. The existing three-decoder conformance and encoder similarity checks remain required.

The prior-art input remains the already reviewed [search-efficiency technique note](../technique-notes/h264-encoder-speed.md), listed above. No new implementation source or unreviewed study output is an input to this design. A separate study can now compare its techniques with these five answers. Only its reviewed note, after commitment, may inform implementation.

## Partition-search implementation

The implementation uses the design above and the reviewed [partition-search reuse note](../technique-notes/h264-partition-search.md), without opening the studied sources. It adopts two exact changes. Visited-score lookup and finalist admission inspect only initialized entries. Motion probes count their signalling cost before SAD and stop only when accumulated cost strictly exceeds both the fourth ranked cost and the best integer-centre cost. Without four ranked entries and an integer centre, every probe completes. The initial protected seeds therefore complete. Fractional stage centres start at the best integer result and can only improve, so the integer bound also protects those decisions.

An abandoned probe is not inserted into the score cache. A revisit can repeat its calculation, but can never retrieve a partial sum as an exact score. Complete scores retain the existing tuple order. No layout, seed, search pattern, finalist cap, complete RDO objective, normative admission, or committed sequence state changes. The new regressions compare every ranking and integer-centre update against complete scoring across partition extents, QP boundaries, edge extension, outlying predictors, and reverse-order revisits. Explicit fixtures protect an integer-centre improvement outside the top four and an equal-cost tie.

### Reference comparisons

Each isolated comparison runs five alternating before/after pairs on the four-picture input above. Both accepted changes reduce elapsed time in all five matched pairs and preserve complete stream and reconstruction bytes. These are local reference figures, not judged speed claims. The input is lossy. Builds, host, settings, and CPU affinity match the local profile. The timed command is the existing `p` driver, including its CoreAV decoder verification, input reads, and output writes, without a profiler. All comparisons use optimized release builds with `CARGO_PROFILE_RELEASE_DEBUG=1`.

| Arm | Before wall seconds, in call order | After wall seconds, in call order | Median before | Median after |
|---|---|---|---:|---:|
| Initialized-prefix lookup against main | 3.816699, 4.590795, 3.815880, 4.081827, 4.120345 | 3.584350, 3.769618, 3.781091, 3.855414, 3.833405 | 4.081827 | 3.781091 |
| Row cutoff against initialized-prefix lookup | 4.076100, 4.119784, 3.929944, 3.909300, 3.935649 | 3.791440, 3.711077, 3.706743, 3.549293, 3.656575 | 3.935649 | 3.706743 |
| Direct motion-length arithmetic against initialized-prefix lookup, rejected | 3.763301, 3.778297, 3.533032, 3.551166, 3.970318 | 3.729738, 3.874050, 4.281814, 3.908159, 4.380935 | 3.763301 | 3.908159 |

The direct signed Exp-Golomb length experiment preserved bytes but did not win, so the implementation retains the existing syntax counter. Further base-state retention, interpolation caches, and heuristic layout omission are not implemented in this round. They require their own comparisons rather than assuming that the profile or technique note establishes a win.

A single full 60-picture pair compares main with both accepted changes. Wall time is 70.592323 seconds before and 64.620203 seconds after, or 0.850 and 0.929 pictures per second, a 1.092x throughput ratio. Process CPU time is 69.043010 and 61.692367 seconds respectively. This one-pair reference includes the driver's verification and IO and has no measured repeatability interval. It does not establish the same gain on other inputs or hosts.

Both complete streams have SHA-256 `26e139c6f54b809565934ab1048c47527973b6589fa14d75b1efa9775ddc8a59`. Both complete 60-picture reconstructions have SHA-256 `d670fa1bdbc5e24308f390f831855de08fbd5b9153b6cb2e1445579749481ab4`, with all 27,648,000 bytes equal. Pinned FFmpeg independently reproduces those bytes. Rate and reconstruction quality are unchanged because the complete output bytes are unchanged, not because a rounded quality metric happens to agree.

## Next design-only slice

The [picture-budget proposal](h264-encoder-rate-control.md) defines the next design-only slice after inter conformance. It separates a target-rate policy from normative QP and buffer constraints, defines failure and flush behavior, and proposes synthetic acceptance checks before any implementation. CABAC, adaptive quantisation, lookahead, and any quality or speed acceptance method remain separate work.
