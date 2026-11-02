# Encoder quality judge validation

Status: completed without quality-vote authority. Development calibration failed; held-out and vision-model judging were not run. The registration below is preserved. This study writes no encoder and does not authorise an encoder implementation.

The question is whether a repeatable quality judge can replace routine visual comparisons during encoder development. The eventual encoder target is x264 medium-class quality at matched-quality speed parity. This study measures the judge's cost, not encoder speed. A favourable quality result alone does not establish that the judge is affordable on every candidate encode.

## Result dated 2026-11-02

G0 ended with no quality-vote authority under the registered failure rule. All 276 required development cases completed with finite readings on all 120 frames and the required in-scope support. The byte identities were frozen before calibration, and all five calibration dispositions were fixed before any held-out judging.

| Detector | Clean controls | Severe positives | Development calibration |
| --- | --- | --- | --- |
| Banding | 20 | 32 | Failed: zero separation, null threshold |
| Blocking | 20 | 32 | Calibrated |
| Flicker | 20 | 80 | Calibrated |
| Colour shift | 20 | 80 | Calibrated |
| Pumping | 8 | 32 | Calibrated; 12 controls out of scope |

Banding's maximum clean-control CAMBI increase and minimum severe-positive increase were both zero. Every severe banding candidate had different bytes from its reference, but all 32 measured CAMBI increases were zero. The gap did not exceed `1e-9`, so no threshold was substituted. No detector, fixture, scope, weight or bar was changed after observing this result.

The other four detectors calibrated on development data only. This does not establish their held-out recall or false-positive rates. Human held-out scoring, planted held-out judging and vision-model judging were never opened. No human-agreement, vision-gain or full-judge affordability claim follows from this study. The 0.90 proxy bar remains unchanged.

## Claims and current data limit

The original headline requires at least 0.90 pairwise accuracy on statistically significant human preferences. The released compression labels inspected here do not contain the observer uncertainty needed to identify those preferences. That headline is **not estimable** from this release. A point-score-gap proxy is registered separately and must never be reported as statistical significance.

The frozen proxy leaves **341 pairs in 19 source-family clusters**, including **216 x264-involving pairs in 19 clusters**. The requirement, fixed before screening held-out labels, is at least 300 pairs in 20 clusters, including 100 x264-involving pairs in ten clusters. It fails the cluster requirement. Do not lower the requirement, change the split, relax the gap, or add pairs to rescue this roster. No headline proxy pass or cross-content generalisation claim is available from it.

The registration defines a second, independently frozen MOS-gap proxy in set D, plus interval-separated auxiliary evidence in set E. Neither is represented as statistically significant human preference. Keep each set's result and sufficiency verdict separate. Do not pool an easy stratum or another set into set A to rescue its failed requirement. Any later uncertainty-bearing study needs a new registration and a fresh held-out roster, not another judgement of these observations.

## Data and permission boundaries

| Neutral set | Use | Boundary |
| --- | --- | --- |
| Subjective set A | Same-source, same-preset compression comparisons, including x264-labelled conditions. | Internal use only. The exact source, distributed licence tag and authorisation are retained privately. Point scores do not establish statistical significance. |
| Subjective set B | First-JND observations on 24 released 1080p contents. | JND detection, not arbitrary better/worse preference labels. Retain the applicable notices and source mapping privately. |
| Subjective set C | Supplemental banding classification against human-marked image regions. | Region labels are not annoyance MOS. Automatically inherited patch labels are not independent human judgements. |
| Subjective set D | Compression comparisons with point MOS, bitrate and resolution strata. | Use a development-calibrated MOS-gap proxy and report nontrivial pairs separately. Easy ordering must not carry the accuracy bar. |
| Subjective set E | A smaller compression set with MOS and confidence intervals. | Use only authorised source families. Correlated excerpts are one family, not independent recordings. |
| Original mathematical clips | Controlled injections and clean controls. | No downloaded imagery, fonts or audio. Injection identity is known; human visibility is not established by construction. |

The current set A label table has 1,962 rows and 60 sequence identifiers. Its pinned public tree contains 1,566 media files in 45 sequence directories, including 43 reference files. Literal mapping by the publisher's filename rule finds 1,523 labelled media files and leaves 439 rows unmatched. Missing media is not silently inferred or downloaded from another release. Only files needed by a frozen roster may be fetched.

The table provides BT, MOS and ELO point scores, but no per-pair votes, observer counts, standard deviations, standard errors or confidence intervals. BT and MOS agreement is not independent corroboration if they derive from the same responses. Exact encoder builds and the independence of original recording sessions are not established by sequence identifiers. References are the dataset's source videos, not assumed pristine recordings.

Do not use a paper's licence as a licence for its external dataset. Use only internally authorised material. Do not infer an additional permission from a public download link. A dataset-search result incidentally included encoder-source snippets; no source page was opened and the snippets were not used. Participants with that exposure do not write subsequent encoder code.

## Frozen split and proxy

The private source manifest assigns all 60 set A sequence identifiers before objective scoring or gap calibration. A family removes one terminal `-19xx` or `-20xx` suffix from a sequence identifier. This groups repeated names conservatively; it does not prove capture independence. It produces 55 clusters. Interpret SHA-256 of `coreav-quality-g0-source-split-v1|family` as an unsigned integer. Residue zero modulo three is development; other residues are held out. There are 20 development and 35 held-out clusters before availability and proxy screening. Never reassign an unavailable family.

Calibrate gaps using development labels only. Within each `(comparison, sequence, preset)` group, enumerate unordered pairs with finite BT and MOS and strictly agreeing nonzero directions. Within each `(comparison, preset)` bucket, set the BT and MOS thresholds to their respective linearly interpolated 75th percentiles of absolute agreeing pair gaps. Require at least two development families in the bucket. A missing bucket has no held-out fallback. No objective metric, vision-model answer or held-out gap enters calibration.

Freeze those thresholds before screening held-out labels. A held-out pair must share comparison, sequence, preset, resolution and frame rate, have both media and its reference in the pinned tree, and exceed or equal both gap thresholds in the same direction. ELO has no eligibility vote. Missing or nonfinite scores are ineligible with recorded counts. JND observations cannot calibrate these BT/MOS units without shared stimuli; set B therefore remains a separate test.

For deterministic selection, hash the comparison, sequence, preset and lexically sorted media paths, joined by `|`. Keep at most 20 pairs per family by ascending SHA-256, then at most 600 pairs overall by that hash. A/B presentation is randomised independently of the label. Source pairs, not presentation orders, are the observations.

Immutable release identities, exact fitted thresholds, source mappings and roster hashes are frozen in the private registration bundle. No corpus pin, publisher, media identity or label is published here.

The split was committed first, thresholds second, and held-out eligibility was then screened without objective predictions. Counts are preflight evidence, not a judged quality run. The insufficient roster remains frozen. It receives one non-certifying diagnostic evaluation, without a headline pass or a revised denominator.

### Set D and ordering controls

Preserve the published source-level training/validation split. If any source appears in both, reserve the whole source for development. There are 559 development and 141 held-out source identifiers, with no mixed-source partition found in the release metadata. Keep portrait and landscape presentations separate while sharing a source cluster if their source identifier is identical.

Within each orientation, calibrate the MOS gap as the linearly interpolated 75th percentile of strictly positive absolute within-source transcode-pair gaps in development data. Exclude the original-reference row from candidate pairs. Freeze the two thresholds before held-out screening. The frozen rule qualifies 5,729 candidate pairs across 137 held-out source identifiers. Nominal resolution/tier ordering explains 4,220 of them; 1,509 across 133 identifiers are not explained by that nominal ordering. These are metadata counts, not measured-bitrate strata or judge results.

After fetching only the frozen candidate pool's media, measure original video payload bitrate as eight times the sum of video packet sizes divided by the complete decoded duration. Do not use the size of a losslessly prepared metric input. Resolution is stored pixel count. A dominates B if it has at least as many pixels and at least as high a measured video bitrate, with one strict inequality. An eligible pair is easy when its proxy-preferred side dominates. Tradeoffs, exact ordering ties and preferences opposing dominance are the hard stratum.

Choose hard and easy rosters independently. Hash orientation, source identifier and lexically sorted candidate names, joined by `|`. Keep at most 20 pairs per family in each stratum by ascending hash, then at most 600 hard and 200 easy pairs overall. Freeze their media identities and membership before computing objective predictions. Report both the higher-bitrate-alone and higher-resolution-alone heuristic's agreement, plus dominance coverage and correctness, on the entire qualified pool. Report judge accuracy separately on hard and easy rosters. **Only the hard roster can clear set D's 0.90 proxy-accuracy bar.** It also needs at least 300 pairs and 20 source clusters. A failure of those counts is reported, never repaired by easy pairs or a lower gap.

The published human viewport, resizer and encoder build are not established by the available metadata. The registered evaluation assumption is to use the released original's geometry as the display grid and bicubic-scale each candidate to that grid, with matching frame count and clock. Reject contradictory aspect ratios or clocks rather than silently retime. Apply an explicit limited-range SDR BT.709 interpretation when tags are absent, flagging that assumption; reject contradictory tags. This is an assumed presentation, not a recovered description of the human experiment. It limits the interpretation of agreement.

### Set E interval proxy

Reserve all seven authorised source-family clusters for held-out auxiliary evidence; fit no parameter from their quality labels. Require both candidate files and a distributed reference. Compare only within one test, source, encoded resolution and encoded frame rate. Qualify a pair when one reported `MOS-CI` exceeds the other's `MOS+CI`. Do not assume the interval's confidence level or observer covariance. Label this as a reported-interval proxy, not verified significance.

Hash test, source and lexically sorted candidate names joined by `|`, and keep at most six pairs per family by ascending hash. This leaves 29 pairs in six families, including 18 H.264-labelled pairs. A permitted family without an eligible pair contributes no invented observation. This small set is auxiliary and cannot carry a cross-content accuracy claim.

Follow its documented presentation transform explicitly: bicubic scale to 3840 by 2160, source frame rate rounded to two decimal places, square pixels and planar 10-bit 4:2:2. Apply the same reference/display contract to each candidate. Preserve the 10-bit samples and native chroma rows for metrics. Any unavailable mapping or unverified presentation detail is reported, not silently replaced with the cheaper 8-bit 4:2:0 path.

## Measurement layers

| Layer | Registered work | Failure meaning |
| --- | --- | --- |
| L0, input validity | Check immutable file identities, complete decoding, pixel format, geometry, colour interpretation, frame count and presentation timestamps. Match the stimulus and reference without implicit rescaling, retiming or repeated final frames. | A failed or incomplete decode cannot become a quality tie. Encoder-internal reconstruction agreement is inapplicable because no encoder is being written. |
| L1, rate conditions | Record bytes, duration, bitrate, codec labels and supplied rate/preset conditions. Compare only declared compatible conditions. | Do not infer VBV, HRD compliance or CRF monotonicity from a published point-score table. Controlled black-box checks may test the instruments, not validate a new encoder. |
| L2, full-reference quality | VMAF v0.6.1 and NEG, XPSNR, SSIMULACRA2, Butteraugli 3-norm and PSNR-YCbCr. Compute every selected frame. | PSNR is diagnostic only. Missing metrics or shortened work are failures, not omitted votes. |
| L3, defects | Source-relative CAMBI, grid residuals, source-edge error, temporal residual change, chroma RMSE and source-gradient energy loss. Preserve support counts and temporal tails. | Zero support is not evidence of absence. These measures need validation before they become detectors. |
| L4, vision model | Blind, order-swapped two-alternative forced choice on reference/candidate crops and contiguous strips, with a fixed prompt. | It is explanation-only unless its pre-registered held-out contribution clears every retention bar. |
| L5, human evidence | Preserve published human labels, their interpretation and the proxy distinction. | An optional later human audit cannot overwrite a failed held-out result or turn a point-score proxy into significance. |

Every FFmpeg-based operation reads `scripts/oracle-ffmpeg.env` and verifies the installed binary before use, including cache hits. Record binary hashes and model identifiers, hashing external model files when supplied. Embedded models are bound to the binary hash rather than given an invented standalone hash. Constant-rate timestamp checks allow half a declared time-base tick per stream, and half the sum of ticks between streams, while requiring exact frame counts, increasing timestamps and equal first-frame origins. The image metrics are black-box tools from one recorded build. Registered SDR planar formats use an explicit limited-range BT.709 to full-range sRGB conversion with 16-bit RGB images for image metrics. Preserve the registered sample depth and subsampling before that conversion. Do not silently substitute an SDR tone map for an HDR human experiment.

## Exact defect measurements and pooling

Let `R` be reference luma, `D` decoded luma and `E = D - R`, in 8-bit-equivalent code values. For 10-bit inputs divide values by four without rounding or quantization. Use signed arithmetic. Chroma is measured at native subsampling.

- On grids of 4, 8 and 16 pixels from the frame origin, blocking excess is `max(0, |gradient(E)|grid - (|gradient(E)|before + |gradient(E)|after)/2)`. Average both axes. Flat support requires all three source gradients to be at most four code values. Dark support additionally requires the four source pixels to be at most 64. Record the number of supported bonds.
- Source edges have absolute gradient at least 16. Mark both endpoints and expand by a clipped radius of two pixels in both axes. Average `|E|` in that union for edge error. This responds to blur and displacement as well as ringing.
- Temporal residual change is `mean(|E[t] - E[t-1]|)`, with frame zero set to zero. It subtracts shared source changes but is not motion compensated.
- Chroma errors are independent Cb and Cr RMSE. Texture loss averages `max(0, gradient(R)^2 - gradient(D)^2)` on source gradients of at least four, pooling both axes. Added noise can conceal lost energy; this is not a texture classifier.
- Banding uses full-reference CAMBI or separately measured source and candidate CAMBI. Record both source and candidate values and their nonnegative increase. An unavailable reference reading is not zero.

For each nonnegative defect sequence, report the all-frame mean, mean of the largest `ceil(N/10)` readings, and greatest contiguous `min(N, ceil(fps))`-frame mean. Include the final possible window. Clips shorter than one second use the whole clip. Use equivalent lower tails for higher-is-better quality metrics. Never sample away frames for the primary measurements.

## Pairwise rule

This rule is frozen for the diagnostic evaluation. A result above the numerical bars does not override the current roster's failed source-cluster requirement.

For a quality metric, pool half its frame mean and half its worst-decile mean. Higher is better for VMAF NEG, XPSNR and SSIMULACRA2; lower is better for Butteraugli 3-norm. Cap XPSNR at 100 dB, explicitly recording infinite readings. For candidates A and B, compute `tanh((A-B)/scale)` with scales 10 VMAF points, 3 dB and 10 SSIMULACRA2 points; Butteraugli contributes `tanh((B-A)/1)`. Their unweighted mean is `Q`. Classic VMAF and PSNR never vote.

For each candidate, take the maximum `tanh(worst_one_second/scale)` defect risk. Scales are five for CAMBI increase, two for each blocking measure, three for edge error, two for temporal error, four for each chroma RMSE and 64 for squared-gradient energy loss. All values are source-relative. For blocking, edge error and texture loss, source-support counts must agree between candidates. A disagreement fails preflight. If any frame has zero support for a measure, omit that measure from both candidates' risks for the whole pair. Preserve the unsupported frames and omitted measure names. Never replace an unsupported value with zero. CAMBI increase, temporal error and both chroma errors remain mandatory, so the maximum is defined. Do not change weights or scales for partial coverage. Report accuracy separately for complete and partial source-gated coverage, without claiming that an omitted defect is absent.

Let the risks be `rA` and `rB`. The no-vision preference score is `d = 0.8Q + 0.2(rB-rA)`. Positive prefers A, negative prefers B, and exact zero is a tie. A tie on an eligible preference pair counts as an incorrect answer.

The vision-assisted arm uses `0.85d + 0.15v`, where `v` is +1 or -1 only when the two order-swapped answers agree after mapping back to content identity. Two valid tie answers give `v = 0` and count as order-consistent, completed work. A vision tie is incorrect for vision-only accuracy on a non-tie preference pair. An inconsistent, missing or invalid answer gives `v = 0`, counts as incorrect for vision-only accuracy and fails order consistency. Missing or invalid answers also fail work completeness. All remain in the registered denominators. Score the assisted arm from its resulting score, with an exact zero again incorrect. L0 failures cannot be overridden by either arm.

## Vision-model input and budget

Use Capy agents with recorded model identifier `openai/gpt-6.1-sol`, not an external model API. The vision-vote experiment uses the independently selected set D hard roster only. It does not give a retained vote to the insufficient set A roster or the small set E auxiliary set. Record the prompt hash, model identifier, usage and every answer. The identifier does not prove immutable provider weights or absence of public-dataset training exposure.

Select inputs without human labels. For frame `t`, compute `d[t]` by the pairwise rule using individual frame readings instead of pooled readings, with the same directions, scales and whole-pair support eligibility. Rank every contiguous `min(N, ceil(fps))`-frame window by `mean(abs(d[t]))`. Select the largest value, then the earliest start. Within that window, the peak is the frame with largest `abs(d[t])`, with the earliest tie. For `N >= 4`, show four consecutive frames starting at `max(0, min(peak-1, N-4))`. Otherwise show every existing frame without padding.

For crop selection, define each candidate's residual energy at each luma pixel as the sum of squared source-relative Y, Cb and Cr errors in 8-bit-equivalent units. Replicate native chroma samples to their corresponding luma cells without interpolation. A crop's score is the mean absolute difference between the two residual-energy fields across its pixels and selected frames. Crop width and height are each at most 256 native pixels. Enumerate origins at multiples of 256 and the final edge-aligned origin on each axis, without duplicates. Sort by descending score, then ascending row and column. Select the first crop and the first remaining nonoverlapping crop. If none exists, show one crop rather than inventing a second.

Use three image files for reference, A and B strips, with crops as rows and selected frames as columns. A fourth file shows both candidates' residual-magnitude strips, using `min(255, floor(4*sqrt(energy)+0.5))` as grayscale values. Label only crop positions, relative frame indices and blind candidate letters. Preserve native crop pixels without resizing. File names and prompts reveal neither codec, bitrate, scores, human labels nor project ownership.

The fixed question asks which candidate has higher visual fidelity to the reference, allowing a tie, then requests one defect category and a short explanation. Swap A/B in a separate blinded presentation. Do not reveal the first answer. Evaluate at most 600 pairs, 1,200 answers and 4,800 image files. The total ceiling is 10 million Capy-reported tokens, including repeated instructions, cached-input accounting and reasoning. Reserve a budget for each batch, stop before the remaining budget is insufficient, and obtain authorisation before increasing the ceiling. An incomplete arm has no vote.

## Planted defects

Use five original scene classes: smooth gradients, dark gradients, moving slanted edges, moving deterministic texture, and fades with a fixed frame-60 cut. Frames are 1920 by 1080, 30 fps, 120 frames, limited-range 8-bit BT.709 YUV420. Source seed varies the source, not the defect identity. Procedural variants are not independently recorded natural scenes.

Development seeds are 0 through 3 per class. Held-out seeds are 20 through 27 per class. Freeze generated byte identities before detector calibration. A local injection affects the central half-width and half-height; a global injection affects the full frame. Refuse clipping instead of weakening an injection silently.

| Defect | Strength ladder | Positive source classes |
| --- | --- | --- |
| Banding | Luma quantization steps 2, 4, 8, 16 | Smooth and dark gradients |
| Blocking | Alternating signed offsets of 1, 2, 4, 8 on 16-pixel blocks | Smooth and dark gradients |
| Flicker | Alternating signed luma offsets of 1, 2, 4, 8 each frame | All five |
| Colour shift | Opposing Cb/Cr offsets of 2, 4, 8, 16 | All five |
| Pumping | Blend fractions 1/8, 1/4, 1/2, 1 with a 5-pixel box blur for the first six frames of each 30-frame cycle | Edges and texture |

The two strongest levels are severe positives by construction, not human-verified visibility thresholds. Judge recall at those levels, separately by defect and local/global scope. Report the two lower levels diagnostically. Clean controls include source motion, grain-like texture, fades and cuts shared by source and candidate. Identity and lossless round-trip controls cannot establish false-positive rates on perceptually equivalent lossy encodes.

Each detector uses its named measure and worst-one-second pooling. Blocking takes the maximum of its supported grid measures; colour takes the greater chroma RMSE; pumping uses texture loss; flicker uses temporal residual change; banding uses CAMBI increase. Calibrate a threshold once on development clips as the midpoint between the maximum clean-control value and minimum severe-positive value. If those ranges overlap or the gap is no larger than `1e-9`, that detector fails development calibration. Record a nonpassing defect verdict with no substitute threshold. The remaining evaluation is diagnostic-only and cannot grant quality-vote authority, even if pair accuracy clears its bars. Keep every fixed raw measure, fusion weight and scale unchanged. Do not drop the failed detector or fit again after a miss. Commit all five calibration dispositions and fitted thresholds before opening synthetic held-out results. An unsupported held-out severe positive counts as a miss in its registered denominator, not a removed example.

Calibration requires every registered development clean control and severe positive, with complete finite frame readings and source support. A blocking reading needs at least one grid measure supported on every frame; pumping needs texture support on every frame. If any required clip or reading is missing, nonfinite or unsupported, record failed calibration for that detector with a null threshold and the affected clip identities and reasons. Do not compute a midpoint from the remaining clips, substitute zero or use a fallback measure. The same nonpassing, diagnostic-only disposition applies. Neither the proxy gaps nor the 0.90 bar changes.

### Amendment dated 2026-11-02: pumping calibration scope

This amendment corrects detector applicability before any held-out metric, human-label judgement or vision-model exposure. The preceding all-clean-control rule made pumping calibration fail on textureless references, even though pumping's registered positive source classes are edges and texture. The original rule and its unsupported-control finding remain recorded above. This amendment supersedes only pumping's clean-control applicability, not an observed held-out failure.

Pumping calibration uses all eight development clean controls from edges and texture, with seeds 0 through 3, and all 32 registered severe positives from those same classes. Smooth gradients, dark gradients and fades with a cut are outside pumping's calibration and planted-detector scope. Record those 12 development controls as out of scope, never as passes or zero-valued readings. Compute pumping's clean-control false-positive rate only on its in-scope controls and report the out-of-scope count separately. Every in-scope control and positive must still have complete finite readings and texture support on every frame. An unsupported in-scope case still fails calibration with a null threshold. Do not choose the scope from the measured defect response or fitted separation.

Development-only structural preflight covers all 20 clean controls across all five detectors and every registered severe positive. The table counts controls with structural source support on every frame, out of four controls per class. Banding, flicker and colour have no source-support predicate; their entries establish only that the predicate is inapplicable, not that a metric returned finite values.

| Detector | Smooth gradients | Dark gradients | Moving edges | Moving texture | Fades with a cut | Severe positives with source support, where applicable |
| --- | --- | --- | --- | --- | --- | --- |
| Banding | n/a | n/a | n/a | n/a | n/a | n/a |
| Blocking | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 32/32 |
| Flicker | n/a | n/a | n/a | n/a | n/a | n/a |
| Colour shift | n/a | n/a | n/a | n/a | n/a | n/a |
| Pumping | 0/4, out of scope | 0/4, out of scope | 4/4 | 4/4 | 0/4, out of scope | 32/32 |

The preflight evaluates the generator's shared scalar at native 1920 by 1080 coordinates for every one of the 120 frame indices. Each supported blocking frame has a grid-aligned flat-bond witness, and each supported pumping frame has a source-gradient witness of at least four. Texture absence on the other three classes follows their row-invariant ramp and adjacent-step bound of at most one. Severe positives share those exact references, so their source support is unchanged by injection strength or local/global scope. This is an existence check, not a sampled metric score, support-count measurement or threshold fit. All 256 severe-positive cases are represented without encoding or decoding full-size clips. Twenty complete 32 by 32 development clips remain byte-identical after extracting the shared scalar from the generator.

No other detector has a structural class-scope mismatch, so the other four retain all 20 development clean controls. All other rules, bars, proxy gaps, cluster floors and frozen corpus rosters remain unchanged, including the 0.90 accuracy bar. No fitted threshold or calibration pass follows from this preflight. Held-out media, human labels and vision-model work stay closed until this amendment merges. Missing or unsupported in-scope work still fails closed.

## Bars and reporting

| Test | Bar or disposition |
| --- | --- |
| Significant human preference | Accuracy at least 0.90 and better/worse AUC at least 0.95; no dataset below 0.85 accuracy. **Not estimable with the current point-score release.** |
| Point-score proxy | Same numerical accuracy/AUC bars, distinctly labelled proxy, separately for each registered cohort and only after its minimum pair/cluster requirements. **Set A fails preflight.** Set D's hard stratum must clear its own 0.90 bar; easy pairs cannot contribute to that denominator. |
| Compression score correlation | Per-dataset SROCC at least 0.85 and no worse than classic VMAF by more than 0.02, with within-source/preset grouping. It cannot rescue an unassessable headline. |
| set B first-JND test | AUC at least 0.80. Use each viewer's first threshold from its QP-1 anchor. A QP is positive when at least 75% of viewers have crossed that threshold and negative when at most 25% have; omit the middle band by this fixed rule. Do not relabel detection as preference. |
| Human banding annoyance | SROCC at least 0.80. **Not estimable from set C region labels.** Report set C classification separately, grouped by parent source where provenance permits, with no annoyance claim. |
| Each planted defect | Severe-positive recall at least 0.90 in each registered scope; clean-control false-positive rate at most 0.05 per detector. Report unsupported support and every injection failure. |
| Vision retention | On the sufficient set D hard roster, order consistency at least 0.90, pair accuracy at least 0.80, assisted accuracy gain at least 0.02, and the paired 95% source-cluster bootstrap interval for gain strictly above zero. All are required; otherwise explanation-only. No vote is inferred for an insufficient or unevaluated cohort. |

Report every denominator and source-cluster count, class/preset strata, ties, incomplete work and confidence intervals. Randomise presentation independently with a fixed hash. A cluster is the frozen source identifier shared by every rendition and orientation of that source. In set D, use the original-source identifier in the frozen manifest, never its rendition or pair identifier. Sequence-derived clusters provide only approximate recording independence. Do not count reversed presentations or correlated rate points as independent subjects.

For presentation order, use each pair's frozen 64-character lowercase hexadecimal pair identifier and its neutral set letter `A`, `D` or `E`. Hash UTF-8 `coreav-quality-g0-presentation-v1|set|pair_id` with SHA-256, replacing `set` and `pair_id` with those exact strings. Add no whitespace, newline, byte-order mark or Unicode normalization. Canonical candidate order is ascending UTF-8 bytes of the two exact candidate identities stored in the frozen pair record. If the digest interpreted as an unsigned big-endian integer is even, present that order as A then B; if odd, reverse it. The second blinded presentation always swaps the first. Use the same first-order mapping for both judge arms. Labels, quality readings and the bootstrap stream never enter this hash. Reject malformed pair identifiers or duplicate candidate identities rather than inventing an order.

For each sufficient preference roster, sort the `K` cluster keys by UTF-8 bytes. Each of 10,000 bootstrap replicates draws `K` clusters uniformly with replacement. Include every registered pair in each drawn cluster with multiplicity. Use the same draws for both arms. Accuracy is pair-weighted within each replicate, and gain is `(assisted_correct - base_correct) / total_pairs`. Ties, invalid answers and missing answers stay in that total and count as incorrect.

The deterministic random stream hashes UTF-8 `coreav-quality-g0-bootstrap-v1|20261102|counter` with SHA-256. Start the decimal counter at zero and increment it after every digest. Interpret the digest as an unsigned big-endian integer `x`. For each draw, accept `x < floor(2^256/K)*K` and use index `x mod K`; otherwise consume another digest. Continue one stream across replicates. Sort the 10,000 gains. The 95% interval uses the 0.025 and 0.975 quantiles, with linear interpolation at zero-based index `(10000-1)*p`. Do not substitute a bias-corrected or studentised interval. Retention requires the observed paired gain to be at least 0.02 and the lower endpoint to be strictly above zero, in addition to every other registered bar.

set B's entire fixed released collection and the set C HD deposit are reserved for held-out auxiliary evaluation, not fitting. Their immutable release manifests are frozen separately before metric tuning. Verify archive contents, label mapping and parent identities before decoding. A missing identity, ambiguous anchor, missing mapping or unavailable label contract is a failed data preflight, not a permission to replace a sample.

## Execution and cost

Keep media, labels, crops and bulky readings outside the public repository. Treat downloaded media as untrusted. Run full-size generation, metric sweeps and timing on an isolated remote host with a hard deadline and deletion on failure. Local work is limited to unit fixtures and interface checks.

Before any judged phase, verify this note is merged and freeze tool identities, the applicable roster and all fitted development parameters. A missed quality or sufficiency bar is final for that arm. Re-run only after a documented infrastructure fault, never after a narrow miss or a changed method. Cap exhaustion produces an incomplete, nonpassing phase, not a reduced denominator.

Measure cost first on synthetic development clips, not human held-out media. Include input verification, complete decoding, colour conversion, every frame metric, defect analysis and pooling in end-to-end cost. Separate setup/build time and cached-reference variants. Record per-stage wall and child CPU time, processed frames, bytes, thread count, host class and at least five repetitions with median and range. Vision-token and latency costs are separate; do not report metric-only cost as the full judge cost. No encoder speed ratio or per-candidate affordability claim follows from this diagnostic.

Public results contain aggregate numbers, method, tool identities and neutral set labels only. Source mapping, licences, attribution, pins, media and labels remain private. This study uses only existing authorised material. No further data request or publisher material is planned. A replacement protocol or a larger resource budget requires an explicit decision before another registration.
