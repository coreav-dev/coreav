# H.264 encoder deblocking and P pictures

Status: Implementation plan. This extends the opt-in scalar [intra encoder](h264-encoder.md), not its evaluation protocol. There are no quality or speed claims. The existing all-IDR, fixed-QP, CAVLC path keeps deblocking disabled unless the caller explicitly enables it. No command-line product, default-build dependency, or evaluation input changes.

## Sources and delivery order

The sole algorithm source is Rec. ITU-T H.264, version 15, 08/2024. Clauses 6.4, 7, 8.2, 8.3, 8.4, 8.5, 8.7, and 9.2 define neighbours, syntax, reference pictures, prediction, reconstruction, filtering, and CAVLC. Tables and equations come from the specification. Existing CoreAV scalar kernels supply normative sample arithmetic. No other encoder or decoder implementation source is an input. FFmpeg and JM are black-box test executables only.

The implementation is a dependent stack with a plan commit before code in each layer:

1. Add opt-in deblocking of complete reconstructed IDR pictures. This document is that layer's first commit.
2. Add a fixed GOP with one short-term reference. Implement P_L0_16x16 and P_Skip first, verify complete sequences, then add 16x8, 8x16, and 8x8 partitions with 8x8, 8x4, 4x8, and 4x4 subpartitions. Begin the layer with a focused implementation and verification checklist in this document.
3. Add a design-only note for a subsequent rate-control slice. Do not implement rate control or CABAC.

Each layer remains open until its conformance checks and independent review are complete. These changes do not authorize a stage-gate verdict or a merge.

## Settings and ownership

`Settings` gains an explicit `Deblocking` choice. `Disabled` preserves current slice syntax and reconstructed bytes. `Enabled` writes `disable_deblocking_filter_idc = 0` with both slice offsets zero. Nonzero offsets and multiple slices are out of scope. Existing callers select `Disabled` explicitly. The later P-picture layer adds `Gop::AllIntra` and a checked fixed IDR interval. Neither feature is inferred from input content.

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

The initial search is our own deterministic bounded search, not an H.264 requirement. Exhaust integer displacements from -16 through +16 luma samples in each direction. Also evaluate zero and the normative predictor even when outside that square, provided they fit the specification's motion range. Around the best integer candidate, evaluate the eight half-sample neighbours, then the eight quarter-sample neighbours of the best half-sample result. Retain the centre at each step. Duplicate candidates may be skipped without changing ordering.

Use edge-replicated windows from the complete coded reference. Negative coordinates use Euclidean division and remainder, not truncation toward zero. Call `Scalar::luma_mc` for the six-tap half-sample and quarter-sample process in clause 8.4.2.2.1. Call `Scalar::chroma_mc` for clause 8.4.2.2.2 using the same vector in eighth-chroma-sample units. Keep interpolation windows and destination tiles fixed-size. No original source picture becomes a reference.

Evaluate each searched motion candidate through forward transform, quantisation, normative inverse reconstruction, and exact candidate syntax counting. Ordinary inter luma blocks retain their DC coefficient in the 4x4 transform. Chroma retains its separate 2x2 DC process. Select by coded-picture Y, Cb, and Cr squared error plus `2^(floor(QP/3))` times syntax bits, matching the existing integer penalty. Ties prefer fewer bits, then skip, then larger partitions, then lexicographic motion and specification mode order. These are reproducibility choices, not a quality claim.

Partition search is greedy in syntax order. Each partition searches with the selected earlier partitions committed into a candidate-local motion map. Compare each complete macroblock candidate, including intra candidates, only after all its luma and chroma residuals and exact syntax have been counted. Rejected candidates cannot change committed motion, CAVLC counts, or reconstructed samples. Intra prediction in P pictures obeys `constrained_intra_pred_flag = 1`, so inter neighbours are unavailable to intra modes.

Before comparing candidate costs, admit partitions under clause A.3.1(m) and Table A-1's level-5.1 `MaxMvsPer2Mb = 16`. Count the normative `MvCnt` after prediction, not distinct vector values or nonzero vectors. Intra macroblocks contribute zero, P_Skip and P_L0_16x16 contribute one, 16x8 and 8x16 contribute two, and P_8x8 contributes the sum of its subpartition counts. Equal vectors in separate partitions still count separately. Admit a macroblock only when its count plus the preceding selected macroblock's count is at most 16. Prune a partial partition layout once its accumulated count exceeds the remaining budget. An inadmissible partition layout cannot win even if its distortion or syntax cost is lower. Intra candidates remain available with count zero. If no representable candidate survives, fail the frame rather than emit a level violation.

The picture's tentative previous-count state starts from the last macroblock of the preceding successful picture, not zero. Every selected macroblock updates that tentative state. This enforces the bound for every consecutive pair in decoding order, including row transitions and the last-P/first-P picture boundary. Keep the preceding count until a successful IDR's intra macroblocks replace it with zero, which also conservatively covers an IDR boundary. Return the final count with the tentative picture and commit it only on complete access-unit success. Rejected candidates and failed frames cannot change the sequence's carried count.

## Slice data and skip runs

Reuse one residual syntax traversal for counting and emission. Add the inter coded-block-pattern mapping from Table 9-4, the P macroblock-type offset for intra macroblocks, partition prediction syntax, and CAVLC `mb_skip_run`. Uncoded and skipped blocks reset all affected coefficient-count neighbours.

Keep a pending skip-run count in the slice writer. Before a non-skipped macroblock, emit that run, including zero, followed by its macroblock syntax. At the end of the slice, emit a nonzero trailing run before RBSP trailing bits. For local decisions, compare the exact length of the currently terminated candidate prefix, including the current run. The common prior-prefix length cancels between candidates. This is a greedy prefix cost, not a claim of optimal whole-slice bit allocation.

## Verification and completion predicates

No evaluation inputs, quality scores, speed measurements, or timing diagnostics are used. All large matrices run in GitHub Actions. Local work is limited to builds, unit tests, and small synthetic reproductions. Use the pinned FFmpeg in `scripts/oracle-ffmpeg.env` and pinned black-box JM in `scripts/jm-decoder.env`.

The deblocking layer requires all of the following:

- The disabled setting preserves existing synthetic output bytes and passes the unmodified intra conformance suite.
- Enabled-filter syntax has zero offsets. Hand-derived edge fixtures exercise strong and normal filtering, threshold equality, both directions, all QPs, chroma QP transitions, and picture boundaries.
- Complete filtered intra sequences, including cropped coded edges and independent chroma patterns, must match the encoder's filtered reconstruction in every visible Y, Cb, and Cr sample in CoreAV, pinned FFmpeg, and pinned JM separately. A fixture must demonstrate that enabling the filter actually changes reconstructed samples.
- Default features still link no encoder kernels. Formatting, full-feature Clippy, unit tests, and repository policy checks pass.

The P-picture layer additionally requires all of the following:

- The syntax reader verifies reference marking, active reference count, P macroblock forms, skip-run placement, GOP boundaries, frame-number wrap, and parameter-set consistency.
- Synthetic still, integer translation, fractional translation, opposing partition motion, nonuniform chroma, and abrupt picture changes exercise selected inter, skip, partition, and intra paths. Assertions establish that each intended syntax path is actually selected or explicitly constructed by a syntax fixture.
- Every supported luma and chroma interpolation phase, negative displacement, edge extension, constrained-intra neighbour case, motion-predictor fallback, and partition shape has a deterministic test.
- Candidate-admission tests accept a consecutive `MvCnt` sum of 16 and reject 17, including a 16-vector macroblock followed by P_Skip. A forced-cost fixture makes the forbidden partition layout cheaper and verifies that mode decision still chooses an admissible layout or intra. Cover row transitions, last-P/first-P picture boundaries, and failed-frame retry without resetting the carried count. Independently derive `MvCnt` from emitted macroblock and sub-macroblock syntax in the conformance reader and require every consecutive pair's sum to be at most 16 across picture boundaries. A synthetic stream with a sum of 17 must fail that reader even if all decoder samples match.
- Motion-boundary fixtures cover both derived-vector endpoints, one quarter sample beyond each endpoint, and independently invalid `mvd_l0` syntax. Check derived skip vectors and fractional refinements as well as integer candidates.
- Every output at every tested QP decodes completely. All three decoders match the encoder's complete visible Y, Cb, and Cr samples for every frame in order, with exact frame count, crop, profile, level, and metadata. One mismatch, warning, missing frame, oracle failure, or incomplete output fails conformance.
- A rejected frame, allocation failure, or output-budget failure cannot advance GOP state or replace the reference. Retrying a valid frame matches a fresh encoder at the same successful prefix.
- The similarity guard passes on the exact public head against the main allowlist. Its result records the checked commit without exposing comparison source. Every commit has a DCO sign-off.

## Next design-only slice

Plan a bounded fixed-GOP rate controller after inter conformance. Its note must separate a target-rate policy from normative QP and buffer constraints, define failure and flush behavior, and propose synthetic acceptance checks before any implementation. CABAC, adaptive quantisation, lookahead, and any quality or speed acceptance method remain separate work.
