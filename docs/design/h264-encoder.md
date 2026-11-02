# H.264 encoder: G1 intra-only design

Status: Pre-implementation design and acceptance protocol. This note adds no encoder or command-line option. G1 asks whether a safe scalar intra encoder can produce conforming pictures within the objective rate-distortion bar below. It does not establish perceptual quality.

The planned crate is `crates/coreav-h264-encoder`. It is opt-in in the full build only. The default build links no still-patented codec. G1 does not change `coreav ffmpeg`, `coreav ffprobe`, transcode selection, containers, or release packaging. See the [backend design](encoders.md) for those separate contracts.

## Specification and provenance

The sole codec-design source for this initial design is [Rec. ITU-T H.264, version 15, 08/2024](https://www.itu.int/rec/T-REC-H.264-202408-I/en). The relevant clauses are 6.4 for block order and neighbours, 7 for syntax and semantics, 8.3 for intra prediction, 8.5 for reconstruction, 9.1 and 9.2 for entropy coding, and Annex A for profile and level limits.

H.264 specifies decoding, not an optimal encoder search, forward quantiser, or rate controller. The search and rounding choices below are our own initial hypotheses, derived from the specified reconstruction process. They are not normative H.264 algorithms or techniques obtained from another encoder. Commit this design before any source-study session begins, as policy record 0037 point 4 requires.

## Stream subset

G1 accepts planar 8-bit 4:2:0 progressive SDR pictures with even visible dimensions from 16 through 1920 pixels wide and 16 through 1080 pixels high. A sequence has fixed geometry and declared signalling. Unsupported formats or changes fail before producing an access unit. The encoder performs no resampling, scaling, rotation, or colour conversion. A separate, pinned preparation step may produce those input planes from eligible masters as specified below.

Every picture is a complete IDR access unit with one I slice and raster-ordered macroblocks. Both I_16x16 and I_4x4 are available. There are no inter predictions, P or B slices, I_PCM escapes, CABAC, 8x8 transforms, transform bypass, custom scaling lists, FMO, ASO, redundant slices, fields, MBAFF, or data partitioning.

The SPS signals `profile_idc = 66` and `constraint_set0_flag = constraint_set1_flag = 1`. Other constraint flags and reserved bits are zero. This is the Constrained Baseline subset in Annex A.2.1.1, not the separate High-profile intra family. Use level 5.1 for the initial common comparison envelope, and validate its Annex A limits, including macroblocks per picture, macroblocks per second, decoded-picture buffering, and applicable bitstream bounds. Never treat a level tag alone as proof. Refuse an output that exceeds the envelope.

Pad the coded frame to whole 16x16 macroblocks by repeating the last visible row and column. Signal right and bottom cropping in the SPS in 4:2:0 crop units. References and CAVLC neighbours operate on the coded picture. Metrics and picture equality use the visible crop. Include padded-edge cases in conformance tests.

Use `frame_mbs_only_flag = 1`, `pic_order_cnt_type = 2`, `log2_max_frame_num_minus4 = 0`, and `max_num_ref_frames = 1`. Each IDR resets `frame_num` to zero. Alternate `idr_pic_id` between zero and one to distinguish consecutive pictures. Use short-term reference marking with both IDR marking flags zero. No reconstructed picture is used for temporal prediction.

The PPS selects one slice group, CAVLC, no weighted prediction, `pic_init_qp_minus26 = 0`, zero chroma QP offset, and `constrained_intra_pred_flag = 1`. Enable the deblocking-control syntax but set each slice's `disable_deblocking_filter_idc = 1`. Disabling deblocking keeps the first reconstruction loop small and is matched in the baseline. An enabled filter belongs to a later gate.

VUI records the declared preparation convention for range, matrix, primaries, transfer function, and chroma location. Record original source declarations separately, including absent or unknown fields. Do not infer historical colour from resolution. Both comparison arms use nominal 30/1 timing for signalling and measurement, with `timing_info_present_flag = 1`, `num_units_in_tick = 1`, `time_scale = 60`, and `fixed_frame_rate_flag = 1`. These fixed values fit the unsigned 32-bit timing fields. Source frame rate is recorded but is not an eligibility rule or a configurable G1 timing value. Every picture is independently coded, with no temporal resampling. No HRD, VBV, SEI, AUD, or filler is required in G1. The comparison counts parameter sets, not optional identifying metadata.

## Crate boundary and bounded state

The encoder is a sans-IO library. It accepts checked plane views and sequence settings, then returns complete SPS, PPS, and IDR NAL units and a reconstruction for testing. It reads no file, launches no process, and starts no thread. A test-only driver handles raw-frame input and Annex B output. Container muxing, decoding source files, and CLI integration stay outside the crate.

Sequence validation checks dimensions, strides, plane extents, frame counts, QP, arithmetic overflow, and caller limits before allocating. Limits bound coded pixels, total frames, bytes per access unit, and total output bytes. Failure returns an error, never partial successful output. Macroblock scratch holds predictions, coefficient levels, reconstructed candidates, and neighbour state. Reuse that bounded scratch rather than allocate for every mode.

`coreav-kernels-h264::Scalar` supplies the already specified intra prediction and inverse-transform arithmetic through checked views. G1 explicitly selects `Scalar`, never runtime SIMD dispatch. New forward transforms, quantisation, mode selection, and bit writing belong to the encoder. A forward kernel moves to the kernel crate only when there is an actual shared consumer or a later measured kernel change.

`coreav-h264` remains the independent NAL parser and decoder, not an encoder state machine. Its decoder is a development dependency and a conformance oracle. G1 does not expose its private SPS, CAVLC, or reconstruction internals just to make encoding convenient. Specification tables may have a shared generated owner if the implementation demonstrates a need, but the encoder never calls the decoder to reconstruct its own output. Sharing arithmetic does not replace independent FFmpeg and JM checks.

G1 production code is safe scalar Rust with no `unsafe`, intrinsics, FFI, or encoder-specific unsafe-policy exception. The first SIMD kernel in a later gate must bring its encoder unsafe policy record, checked safe interface, scalar equivalence evidence, and `docs/unsafe-sites.md` rows. The decoder's existing policy is not an implicit encoder exception.

## Prediction and mode decision

Derive availability and reference samples from reconstructed neighbours, never original pixels. At picture boundaries, apply the exact unavailable-neighbour rules in clause 8.3. Preserve unfiltered reconstruction for subsequent intra prediction. Deblocking is disabled for this gate, so the reconstruction exposed for comparison is also the final picture.

For I_16x16, test every available vertical, horizontal, DC, and plane prediction. Each candidate transforms and quantises its residual, reconstructs it, and measures distortion on that reconstruction. For I_4x4, visit the sixteen luma blocks in the specification's block order. Test every legal one of the nine modes, including top-right substitution and the DC fallbacks. Commit the selected block reconstruction before searching the next block. This is a greedy block search, not a global search over all mode combinations.

For each luma candidate, test every available chroma DC, horizontal, vertical, and plane mode on both chroma planes. Use the I_4x4 most-probable-mode derivation from clause 8.3.1.1 and its `prev_intra4x4_pred_mode_flag` and `rem_intra4x4_pred_mode` syntax. Keep candidate neighbour-mode and coefficient-count state separate until a candidate wins.

Select the candidate with the lowest `D + lambda * B`. `D` is the sum of squared errors over the coded luma and chroma samples, with unit weight per sample. `B` is the exact macroblock syntax and CAVLC bit count from a counting writer, with the candidate's real starting neighbour state. Use the integer `lambda = 2^(floor(QP / 3))` as the initial, independently chosen rate penalty. This value is a design hypothesis, not a quality claim. Ties prefer fewer bits, then I_16x16, then the lowest specification mode number. For a 4x4 block decision, compare the block's reconstruction error and its incremental syntax and residual bits. Final macroblock comparison includes all luma and chroma bits.

This exhaustive local candidate evaluation is intentionally simple. No borrowed fast-mode thresholds, psychovisual costs, lookahead, adaptive quantisation, or source-dependent presets enter G1. Any training-only revision is committed with its rationale before the held-out run.

## Forward transform, quantisation, and reconstruction

Subtract prediction from source into signed residuals. Use a separable 4x4 integer forward transform derived algebraically from the linear part of clause 8.5.12.2. Its row basis is `(1,1,1,1)`, `(2,1,-1,-2)`, `(1,-1,-1,1)`, and `(1,-2,2,-1)`. Derive its scale factors from the inverse equations, rather than copying another encoder's multiplier table. Use wide intermediates and explicitly defined signed rounding.

I_16x16 separates the sixteen luma DC terms into the 4x4 Hadamard transform paired with clause 8.5.10. Each chroma plane separates its four DC terms into the 2x2 transform paired with clause 8.5.11. AC residual blocks exclude those separately coded DC terms. I_4x4 luma includes DC in each ordinary sixteen-coefficient block. Use the frame scan of clause 8.5.6, not a row-major coefficient serialization.

The initial quantiser chooses the nearest signed coefficient level to the ideal inverse-scaled transform value, ties toward zero. Derive the ideal scale from the flat scaling list, `LevelScale4x4`, QP quotient and remainder, and the appropriate ordinary, luma-DC, or chroma-DC inverse process. Keep these three scale paths distinct. There are no tuned dead zones or trellis search. Reconstruct with the normative rounded integer inverse process, not with the ideal forward estimate. Enforce all coefficient, intermediate, and CAVLC `level_prefix <= 15` constraints before accepting a candidate. Never wrap, silently clip levels to legal syntax, or fall back to I_PCM. If no legal candidate exists, the encode fails and conformance fails.

G1 uses a fixed luma QP for the sequence, restricted to 10 through 51. The restriction avoids the low-QP CAVLC DC representability issue called out in clauses 8.5.10 and 8.5.11. Map chroma QP through the normative nonlinear table with zero offset. Signal `slice_qp_delta = QP - 26` and zero `mb_qp_delta` where that syntax is present. There is no CRF, ABR, per-block QP change, VBV, or bitrate target.

For every winning block, inverse-scan levels, apply the appropriate inverse scaling and DC transform, apply the integer inverse 4x4 transform, add prediction, and clip samples exactly as clause 8.5 requires. Store this result for the next prediction. Preserve the complete encoder reconstruction until all three decoders have compared it. Transform tests cover basis impulses, signed extrema, DC-only blocks, all six QP remainders, chroma-QP transitions, and padded boundaries.

## NAL and entropy writing

Write SPS, PPS, slice headers, macroblock types, coded block patterns, prediction modes, and residuals from clauses 7 and 9. Use SPS and PPS identifiers zero. Repeat both parameter sets before every IDR in both comparison arms. Each access unit contains one type-5 NAL with nonzero `nal_ref_idc`. A byte writer adds `rbsp_trailing_bits` and inserts emulation-prevention bytes after RBSP construction. Annex B start codes are a driver concern, not part of the entropy writer.

The bounded bit writer supports fixed-width fields and unsigned, signed, and mapped Exp-Golomb codes. Encode `ue(v)` from `codeNum + 1`, map signed values as clause 9.1 specifies, and use the intra coded-block-pattern mapping rather than writing that pattern as an unsigned integer directly. A counting sink and a byte sink share the same syntax traversal so candidate bit costs cannot drift from emitted bits.

CAVLC reverses the parsing process of clause 9.2. Derive `TotalCoeff`, at most three trailing ones, their sign bits, remaining levels in reverse scan order, `total_zeros`, and `run_before`. Derive `nC` from available left and top blocks' nonzero counts, with the separate chroma-DC VLC. Handle the first non-trailing level adjustment and evolving suffix length exactly. Distinguish blocks with 16, 15, and 4 possible coefficients. Uncoded blocks still update neighbour counts to zero.

All VLC mappings come from specification tables. Tests exhaust the finite coefficient-token, total-zero, and run-before mappings and exercise level suffix transitions, zero blocks, sparse blocks, dense blocks, and negative levels. Parse every emitted syntax form back with an independent reader. No x264, JM, or OpenH264 table is an implementation input.

## Pre-registered G1 acceptance

These are acceptance bars, not results. G0 gave the quality judge no vote authority. SSIMULACRA2 and XPSNR are objective measurements here, not validated substitutes for viewers. Neither an agent judgement, a visual inspection, nor a metric average establishes perceptual parity.

| Check | Required result |
|---|---|
| Conformance | 100% of outputs at every tested QP decode completely. CoreAV, pinned FFmpeg, and pinned JM reconstruct every visible Y, Cb, and Cr sample exactly as the encoder did. Frame count, order, crop, profile, level, and metadata agree. One mismatch fails G1 |
| Objective rate-distortion | On the complete held-out roster, the median per-clip BD-rate relative to matched x264 intra-only is at most +30% separately for SSIMULACRA2 and XPSNR. Both must pass |
| Reconsideration | A median above +60% in either metric requires reconsidering the approach. A median above +30% through +60% is still a miss, not a provisional pass |
| Speed | Record diagnostics only. There is no G1 speed pass or fail threshold |
| Safety and scope | Safe scalar implementation, bounded failures, full-build opt-in only, and no compatibility-command changes |

### Oracle and baseline pins

FFmpeg is the exact binary verified by `scripts/oracle-ffmpeg.env` and `scripts/oracle_pin.py`, currently 9.0.2, build `n9.0.2-17-g2a571b6068`. Do not replace it with a system installation. Its software H.264 decoder runs with one thread, no hardware acceleration, and strict error reporting, producing native `yuv420p` without filters or format conversion.

x264 is core 165 at `X264_COMMIT` in that same pin file, currently `0480cb05fa188d37ae87e8f4fd8f1aea3711f7ee`. Build the test-only CLI with `scripts/install-x264.sh` without reading source. Reusing the repository's oracle pin avoids an unrelated baseline and ties the CLI to the pinned FFmpeg's libx264. Record the executable hash, build configuration, and effective version. A pin change requires a pre-run protocol amendment, not an unrecorded upgrade.

Use JM release 19.0, the `jm19.0.zip` release archive from `https://iphome.hhi.de/suehring/tml/download/`. Build `ldecod` outside the repository as a black box, never inspect its source, and delete the source tree after building. Freeze the archive SHA-256, compiler, build command, executable SHA-256, and complete decoder command in the run manifest before encoding any roster input. A release name alone is not a completed integrity pin. JM `lencod` may provide a separately labelled diagnostic RD anchor, but is not the x264 acceptance denominator and cannot rescue a miss.

Use the following x264 controls at each QP in `16, 22, 28, 34, 40`. Our driver uses the same picture envelope, metadata, fixed QP, disabled deblocking, one slice, and one thread. The baseline is `medium` to provide a stable nontrivial intra search, not a claim that our search matches its complexity. The freeze records the CLI's complete effective configuration and the exact supported spelling of these controls, including Annex B and repeated-header defaults.

```text
--preset medium --profile baseline --level 5.1
--keyint 1 --min-keyint 1 --scenecut 0 --bframes 0 --ref 1
--no-cabac --partitions i4x4 --no-8x8dct --trellis 0
--no-psy --aq-mode 0 --no-mbtree --ipratio 1 --pbratio 1
--chroma-qp-offset 0 --no-deblock
--threads 1 --lookahead-threads 1 --sync-lookahead 0 --rc-lookahead 0
--slices 1 --qp Q
```

Both arms receive the identical prepared planar frame bytes, dimensions, frame count, nominal 30/1 timing, and preparation colour and siting convention from the manifest. No tune, CRF, hidden resampling, or per-clip option search is allowed. Verify the effective baseline settings and parse the output for IDR-only slices, QP, CAVLC, cropping, and disabled filtering before accepting a cell. Parameter-setting failure is not permission to substitute a preset. No identifying SEI is counted or emitted in either arm.

### Roster freeze and held-out split

Every G1 measurement run is blocked until a separate freeze PR merges. This plan fixes the selection method, not an already acquired byte set. The freeze supplies a machine-readable roster and byte manifest with full SHA-256 per original source and per prepared reference, byte counts, selected stream, first frame, frame count, original and prepared geometry, original timing and colour declarations, preparation commands and conventions, parent identity, neutral label, and split. Hash Y then Cb then Cr in presentation order without headers or padding. Hash the ordered manifest as well. Source identities, acquisition chains, licence evidence, and the neutral-label mapping remain private. Public records use neutral labels and hashes only. The G0 validation sets are not eligible for encoder evaluation.

Select 24 independent references delivered uncompressed as raw planes or Y4M, or as production-grade masters. Exclude sources documented as derived from a consumer or low-bitrate lossy delivery. Production acquisition need not have been lossless. Record the documented chain privately, including acquisition compression or production mezzanine stages, rather than claim that uncompressed delivery proves lossless history. Use no two excerpts from one parent sequence and no duplicates across the training and held-out sets.

Eligible rights are public domain, CC0, CC BY, or terms that explicitly allow internal R&D. Record whether redistribution is allowed or the source must remain private. Non-commercial, research-only, standards-only, or unclear terms remain excluded unless the maintainer decides otherwise before inclusion. A readable download URL is not permission.

Use exactly the first 60 presented frames of each source. Sixty independently coded pictures provide repeated spatial samples while bounding source and metric work. Equal frame counts prevent frame-rate or duration differences from changing a clip's weight. There is no minimum or maximum source frame rate and no duration requirement. Do not interpolate, duplicate, or drop frames to meet a temporal target. A source with fewer than 60 frames cannot fill a slot.

Preparation uses the pinned FFmpeg 9.0.2 and an explicitly frozen crop, scale, and conversion filter chain to produce progressive 8-bit `yuv420p` within the encoder's geometry bounds. Declare crop coordinates, target dimensions, scaling kernel, input interpretation, output range and colour convention, chroma siting, precision, and rounding. Where source metadata is absent, record both the absence and the preparation assumption, not an invented source declaration. Freeze every command and prepared-plane hash before any encode. No automatic format conversion, denoising, or result-dependent preparation is permitted. Both encoders receive those same bytes without further conversion.

The four prepared-resolution groups are R1, height at most 360; R2, height 361 through 720; R3, height 721 through 1080 and width at most 1440; and R4, height 721 through 1080 and width above 1440. The following content classes describe pixel characteristics, not source origins. Each class supplies exactly one independent clip in each of its three listed resolution groups, so each resolution group has six clips. If sourcing cannot fill a group after the permitted preparation, report its missing slot count and propose the smallest group amendment before freezing. Do not guess a replacement rule or shrink a quota silently.

| Class | Required picture characteristic | Resolution slots |
|---|---|---|
| C1 | Smooth gradients with dark or flat regions | R1, R2, R3 |
| C2 | Large flat regions with sharp boundaries | R2, R3, R4 |
| C3 | Dense thin edges or small high-contrast detail | R3, R4, R1 |
| C4 | Fine irregular texture without dominant grain | R4, R1, R2 |
| C5 | Persistent fine noise or grain | R1, R2, R3 |
| C6 | Rapid motion with fine spatial detail | R2, R3, R4 |
| C7 | A cut, fade, or large illumination transition in the selected frames | R3, R4, R1 |
| C8 | Low-motion pictures with detailed stable regions | R4, R1, R2 |

Before any encode or quality score, the sourcing task commits the eligible inventory, licence decisions, parent grouping, class assignments, and exclusions. Class assignment is based only on reference pictures. If a sequence fits multiple classes, assign the lowest numbered class with a required slot, before hash selection. No assignment can use encoded output or an RD result.

For each class and resolution slot, sort candidates by SHA-256 of the UTF-8 string `coreav-h264-g1-v1:` followed by the lowercase prepared-reference hash and take the first eligible candidate. Process slots in class order and the table's listed resolution order. Exclude a parent already selected in an earlier slot. If any slot is empty, sourcing remains incomplete. No quota or denominator shrinks. Assign labels `G1-01` through `G1-24` in that order. Within each class, the selected clip with the smallest selection hash is training and the other two are held out. This fixes eight training and sixteen held-out clips, with no family overlap.

Training may guide correctness and mode-cost changes only after the freeze merges. The sixteen held-out clips may not guide quantisation, mode choice, or baseline settings. Commit the candidate revision and binary hashes before exposing held-out results. Run every held-out clip at all five QPs once as the judged arm. Report the eight training clips separately without pooling them into the acceptance median. Generated edge fixtures supplement conformance but do not replace this roster.

### Metrics and BD-rate method

Decode every output first and require conformance before scoring. Compare decoded visible pictures with the common raw reference in presentation order at native resolution. No searched alignment, fitted gain, border removal, or skipped frames is permitted. Each clip contributes one RD curve per encoder and metric.

SSIMULACRA2 uses the `ssimulacra2` executable from libjxl, built with CMake in Release mode with tools and developer tools enabled, tests and benchmarks disabled, and target `ssimulacra2`. The freeze selects the highest stable libjxl release published by 2026-11-02, and pins its archive digest, dependencies, build command, compiler, and executable digest. This is a measurement tool only, never an encoder dependency. No alternative SSIMULACRA implementation is permitted.

Convert both prepared reference and reconstruction identically to full-resolution 16-bit sRGB PNG with FFmpeg's `zscale` and PNG encoder. For this metric, interpret both sets of planes as BT.709 limited-range input with BT.709 primaries and transfer and left chroma siting. Use bilinear chroma upsampling, sRGB output transfer, and full-range RGB. This fixed measurement convention is not an eligibility rule or a claim about a source's historical colour. It applies even when original declarations differ or are absent. The freeze pins zimg and the exact conversion command, precision, and rounding with fixture output hashes. Compute every frame's SSIMULACRA2 score, then take the arithmetic mean of all 60 scores. Do not clamp negative scores or replace the mean with a percentile after seeing results.

XPSNR uses FFmpeg's `xpsnr` filter at the shared oracle's source revision in a measurement-only build. Build outside the repository without source inspection, using the shared revision, Release optimisation, `--disable-doc`, `--disable-debug`, `--disable-network`, `--enable-libzimg`, and the `xpsnr`, `zscale`, and `format` filters. Enable raw-video input, PNG output, and null output. Remove installed examples and the source tree after building. The freeze records the complete configure command, source archive digest, compiler, zimg dependency, and executable hash. The pinned distributed oracle remains the conformance decoder.

Use the identical prepared Y, Cb, and Cr samples for XPSNR. Feed nominal 30/1 timing to every metric input, independent of source timing, so the filter's temporal weighting has a fixed convention. The primary curve uses clip XPSNR-Y from the filter's sequence aggregation, not an average of frame dB values. Report chroma scores separately. Do not silently switch to PSNR or pool a different plane weighting.

Before any encode, commit the metric executable hashes, conversion command, XPSNR aggregation command, and a fixture demonstrating frame count and finite output. These are mandatory run inputs, not choices made by a runner after it sees RD results. Their absence means no judged run may start.

Rate is eight times the complete elementary-stream byte count divided by 60 pictures, in bits per picture. Nominal bitrate at 30/1 is exactly thirty times this rate and yields the same per-clip BD-rate. Actual source timing is not a rate denominator. Count SPS, PPS, NAL headers, emulation prevention, slice data, and Annex B delimiters, with repeated headers before every IDR in both arms. Exclude only optional identifying SEI, which both drivers must omit or remove identically before size accounting. Do not compare MP4 overhead or residual bits alone.

For each clip and metric, order the five points by increasing quality. Rates must be positive and finite. Quality must be finite and strictly increase with rate. Duplicate qualities, nonmonotone curves, missing points, or insufficient overlap make the cell invalid and fail completeness, not become discarded clips. There is no isotonic repair, extrapolation, extra QP search, or post-hoc ladder change.

Interpolate natural log rate as a function of quality with monotone piecewise cubic Hermite interpolation, using the shape-preserving PCHIP slopes and analytic piecewise-polynomial integration. Integrate only over the common quality interval. Require its width to be at least half the smaller arm's quality span. BD-rate is `100 * (exp(mean(log(rate_ours) - log(rate_x264))) - 1)`. Positive values mean more bits for our encoder. Do not fit a single cubic polynomial. Freeze and test the interpolation implementation on identical curves and constant-rate-ratio curves before encoding.

Compute each metric's median over all sixteen held-out per-clip percentages, averaging the middle two sorted values. Publish every per-clip result, group medians, maximum, and completeness counts beside the overall median. A strong median cannot hide a conformance failure or missing cell.

### Failure rules and execution

Conformance failures are defects to fix during development. A judged run with a mismatch, oracle error, missing metric, timeout, invalid curve, or missing clip is a failed or incomplete arm, never a pass. Retain all evidence. Do not replace a clip, drop a QP, widen overlap, change a tool, or weaken a threshold to recover it. A measured miss is final for that candidate and method. A new approach needs a new committed protocol and explicit stage-gate decision.

Rerun the same arm only for a recorded infrastructure fault, such as preemption, a failed input-integrity gate, or runner loss. Keep the failed attempt and the reason for invalidation. Checkpoint by clip, encoder, and QP with hashes so a resumed untimed job does not silently mix settings or recompute successful cells to seek a better result.

Builds, conformance, and objective scoring run in GitHub Actions. GCP spot is an alternative for untimed metric computation with checkpoint and resume, subject to separate run authorization. This plan provisions nothing. Media, timing, profiling, corpus jobs, and metric sweeps do not run on a developer laptop.

Speed diagnostics, if collected in Actions, report the host, CPU model, compiler, thread count, encoded workload, elapsed and CPU time, and peak memory. Shared-runner timings are not acceptance evidence. After the first implementation is profiled on an authorized fixed host, turn the maintainer's target of 1.0x x264 speed at matched quality into pre-registered G2 and G3 bars. Match quality through the RD curves, not equal numeric QP. No G1 result establishes that target or imports the research estimates as speed bars.

## Clean-room policy and prerequisites

Policy record 0037 governs encoder work under the boundaries below. It does not authorize copying, general source reading in implementation contexts, a default-build encoder, or an unsafe G1 implementation.

- Source study of x264, JM, and OpenH264 is allowed only in a separate study session from pinned releases outside the repository. It starts with a specific measured question and compares at least two implementations before recommending a technique. This design must already be committed. A study context, its forks, and anyone receiving an unreviewed study report cannot implement the encoder.
- Study produces prose in `docs/technique-notes/h264-encoder.md`, not encoder code or pseudocode. Record the technique, specification process, source version and licence, file or function provenance, measured rationale, source-specific constraints, and alternatives to our committed design. An independent reviewer checks the note for copying and mirrored structure before an implementation session receives it.
- Implementation starts in a fresh context using the specification, this design, and only committed, reviewed notes. It never opens the studied source. Do not copy code, comments, non-specification identifiers, distinctive constants, non-specification tables, function decomposition, or control flow. No GPL or LGPL code enters CoreAV. A learned technique requires a same-run comparison with our own alternative before adoption.
- JM `ldecod` is allowed as a black-box conformance oracle, and `lencod` as a diagnostic RD anchor. Build pinned releases without source inspection. Neither binary is linked, shipped, or treated as the normative specification.
- x264 developers' papers and prose are permitted design inputs in later work. Patches, diffs, and code are source study and must follow the separation rule. x264 output analysis is also allowed as an input, including mode or QP statistics. Record observations and independently test resulting hypotheses. Never present matching another encoder's mode map as conformance or as a quality bar.
- Encoder unsafe kernels are permitted only after an encoder policy record and unsafe-site rows cover them. That work belongs to a later gate. The full-build-only packaging boundary remains in force.
- Before the first encoder code merges, the similarity guard must cover `crates/coreav-h264-encoder` against x264, JM, and OpenH264. It must use the main allowlist and the pre-registered default of no shared span of 50 normalized tokens. The guard change is a separate prerequisite, not part of this documentation PR. A match blocks merging until rewritten from the specification or reviewed under the guard policy. Cosmetic token edits are not a remedy.

### Prior art

No other encoder implementation was studied for this design. No source-derived technique note was used. x264, JM, and OpenH264 are named comparison or future-study targets only. FFmpeg and JM are black-box decoder oracles. Later work must update this section with the implementations actually studied and the reviewed notes actually used.

### Next gate prerequisites

Before implementation merges, merge both this committed design and the encoder similarity-guard coverage. The separate sourcing and freeze PR must then merge the exact roster, byte hashes, complete JM, metric and x264 pins, preparation and conversion commands, and a run-entry check. That check refuses every G1 measurement run if the merged manifest or an artifact digest is absent or inconsistent, if a slot or split is incomplete, or if a required licence decision is missing. It must reject an unmerged manifest, not merely accept any file on a candidate branch. Test both missing-manifest and digest-mismatch refusals before enabling a run. No G1 encode or quality measurement may precede that merged freeze.

The freeze is a mechanical application of the rules above. Any change to content quotas, eligibility, baseline, metrics, ladder, split, or failure handling requires an explicit protocol amendment before encoder results are exposed. Before held-out acceptance, also commit the driver and candidate revision. This note does not claim those run artifacts, an encoder, or an acceptance result already exist.
