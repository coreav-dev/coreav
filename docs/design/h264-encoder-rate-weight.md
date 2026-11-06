# Quantizer-aware H.264 mode decisions

## Scope and starting point

The maintainer approved calibrating the local rate weight after the byte-identical SIMD work in coreav-dev/coreav#454. This is one design-first change from `c6d0413ab1312efbdf9f3648588cc92da563ad47`. It changes encoder policy, not normative reconstruction, quantization, admission, SIMD arithmetic, default dependencies, licensing, or product commands. The [search design](h264-encoder-search.md) and its reviewed notes remain the implementation inputs. No other implementation source or unreviewed study output is an input.

The current complete candidate objective is sample SSE plus `2^(QP/3)` times exact local syntax bits. On the existing C reference, it selects long runs of non-exact skip and infrequent expensive corrections. At QP28, a represented inter update with SSE 1,805 and 53 charged bits loses to skip at SSE 7,484 and 9 bits because the weight is 512; the crossover is about 129. This establishes a local selection mechanism, not a promised sequence-level bitrate recovery. Reconstruction feedback changes later candidates and skip runs.

## Independent derivation

Use our quantizer and the specified inverse arithmetic, not another encoder's formula. Let `v` be the DC normalization entry already owned by `transform.rs`, and `k = floor(QP/6)`. A constant 4x4 residual of value `d` has forward DC coefficient `16d`. Its quantized DC level is nearest to `64d / (v * 2^k)`, and inverse DC reconstruction divides the scaled coefficient by 64. The step in each reconstructed sample is therefore `v * 2^k / 64`. The energy-normalized coefficient is `4d`, so its quantizer step is `q = v * 2^k / 16`.

For a scalar uniform quantizer, the elementary high-resolution model gives coefficient distortion `D = q^2 / 12` and entropy rate `R = constant - log2(q)`. Its stationary slope is `-dD/dR = q^2 * ln(2) / 6`. This model ignores prediction feedback, entropy discontinuities, coefficient coupling and content distributions; it supplies units and a starting scale, not an optimal encoder. Use `q^2 / 8` as a simple rational starting point near that slope. Compare the neighbouring scales `q^2 / 16` and `q^2 / 4` on complete sequences rather than trusting the model or offline rescoring.

At QP22, QP28 and QP34, the starting weights are 8, 32 and 128. Unlike the old power-of-two schedule, all six normalization remainders participate. Fixed-point cost uses 512 units per sample SSE so low-QP weights are not rounded prematurely to whole SSE units. A single cost owner supplies complete scores, exact syntax floors and zero-error checks; no SAD or SATD estimate becomes a lower bound on that score.

Initial calibration is confined to complete P-picture candidate decisions and their exact bounds. Keep IDR decision ordering and the current cheap proposal metrics unchanged to isolate the diagnosed temporal-refresh mechanism and preserve the original all-IDR byte fixture. P-picture intra candidates still compete under the same complete objective as inter and skip. The later regret analysis determines whether the proposal proxies also need calibration. This does not declare the old IDR or proposal weights optimal.

## Ordered implementation and experiments

1. Commit this plan before changing code or running candidates. Retain the merged SIMD encoder as the before arm. Implement one shared quantizer-derived cost owner and evaluate the starting scale and its two neighbours. Keep search layouts, probe caps, candidate construction, normative admission and tie ordering otherwise fixed. Select the useful compression/runtime tradeoff from complete curves and disclose disagreements between inputs; there is no judged bar or promised recovery.
2. Use the same prepared A and C sequences, sixty pictures at 30 fps, GOP32, enabled deblocking and the existing sample bindings. B remains a QP28 sanity check. Use QP22, 28, 34 and 40 for complete native and corrected x264 curves, retaining the historical three-point results separately. The extra low-quality point gives before/after comparisons a chance to share an actual quality interval instead of extrapolating. Reuse valid existing anchor cells where their exact samples and settings match. Record per-frame Y/U/V SSE and sample-weighted PSNR, complete stream bits, global P-skip and fixed active-region P-skip counts, and the error trajectory between IDRs. C's right-half macroblocks remain the same active-region diagnostic; quality always uses the complete visible planes.
3. Independently recompute PSNR from raw squared error and decode every newly encoded native stream with pinned FFmpeg 9.x. Preserve explicit input colour metadata before x264's raw input and prove byte-identical raw passthrough. Compute PCHIP BD-rate on log bitrate over common PSNR support without extrapolation. Report before/after on their shared interval as well as each arm versus x264; label undefined or narrow intervals and any expanded grid. Native and x264 timing scopes remain distinct. Final timing uses five alternating before/after pairs at QP28 with no overlapping builds, plus scalar-forced identity verification.
4. Fix the objective before inspecting bounded proposal regret. At QP28, select a deterministic, stratified sample of at most 144 macroblocks across A and C and at most eight P pictures per input. Compare the normal winner with complete evaluations of already-completed cheap probe vectors under its represented layouts: improvements here are ranking losses. Separately add a bounded set of candidates excluded by probe/finalist or layout budgets, including already-supported rectangular layouts: additional improvements are budget losses. Count only normatively admitted candidates and evaluate complete coupled macroblocks, not sums of independent block proxies. Cap diagnostic full evaluations at 128 per sampled macroblock. Preserve the production bitstream in logging-only diagnostics and report sample size, evaluated work and positive/zero regret separately. These are bounded counterfactuals, not a global optimum or predicted BD-rate gain.
5. Widen production proposals or tools only if step 4 identifies a concrete loss worth its runtime cost. Measure any such step separately on complete curves. Before generalizing beyond A and C, select another existing input with sustained visible motion using raw temporal differences and image coverage, before observing its encoder results. Do not source new media or treat C's distinct-picture count as broad-motion evidence. If the bounded evidence does not support another tool, retain the calibrated objective alone and disclose the remaining gap.

## Correctness and delivery

The initial implementation retains the `q^2 / 8` starting scale after the four-point sequence comparison. The lower neighbour gives only a small additional improvement on A and a worse curve on C; the higher neighbour loses compression on both inputs. Lowering the rate weight also increases runtime, so the result is a compression/runtime tradeoff rather than a speed improvement. Proposal regret is evaluated only after this objective is fixed; it does not justify widening search by itself.

Keep scalar and dispatched output byte-identical at the new policy. Existing decoder conformance, level and caller limits, transactional failure, exact syntax counting, equality boundaries and the original all-IDR byte test remain intact. Tests cover all admitted QPs, the quantizer normalization remainders, fixed-point score units, exact bounds and zero-error decisions. Existing cost-projection assertions follow the shared owner without dropping their predicates. No new unsafe code or public tuning API is needed.

Run both encoder build modes, their complete-stream identity comparison, encoder Clippy, formatting, Python receipt/metric tests, the existing three-decoder CI and the current-main similarity guard. Reference observations, input/executable identities, compiler notes and comparison intervals are sanitized and committed with the outcome. These are small local reference figures under the maintainer's speed-over-audit direction, not representative or perceptual quality claims. The plan, implementation, review fixes and evidence ship in one signed-off pull request.

## Reference outcome

The [reference record](h264-encoder-rate-weight-reference.json) retains all four QPs for every arm, complete frame-plane squared errors, stream and executable identities, exact calls, scalar/dispatched identity and bounded diagnostic rows. These observations use the existing lossy A/C inputs and B sanity input, not a new representative set. The older three-point references remain separate. Every new native curve stream decodes identically to its encoder reconstruction with pinned FFmpeg 9.0.2; raw colour-declared x264 passthrough preserves the prepared samples. PSNR is independently recomputed from raw SSE.

The following x264 comparisons use one shared interval per input across the before arm and all three candidate scales. Percentages are bitrate overhead at equivalent sample-weighted YUV PSNR, not fixed-QP bitrate changes. C's common interval is narrow.

| Input | Common PSNR interval, dB | Before versus x264 | q²/16 versus x264 | Selected q²/8 versus x264 | q²/4 versus x264 |
|---|---|---:|---:|---:|---:|
| A | 37.911877–49.013435 | +22.281% | +8.095% | +8.240% | +9.875% |
| C | 36.826487–38.715978 | +279.176% | +34.142% | +30.057% | +38.229% |

Using each before/selected pair's complete common support, selected versus before is −11.527% on A over 37.762452–49.013435 dB and −65.416% on C over 36.276198–38.715978 dB. On the selected/x264 pair's support, the remaining overhead is +7.909% on A and +26.080% on C. Those pair-specific intervals differ from the table, so these percentages are not transitive. No curve is extrapolated.

At QP28, C changes from 162.852 to 356.160 kbit/s and from 35.683142 to 42.631999 dB; x264 is 247.904 kbit/s and 42.094278 dB. The lower rate weight spends more bits on corrective updates rather than letting distortion accumulate. C's global P-skip share falls from 98.115% to 91.121%; its fixed right-half share falls from 96.229% to 82.241%, versus x264's 83.395% global and 66.790% right-half. Complete-plane frame PSNR at pictures 1, 16 and 31 is 42.485, 34.840 and 34.261 dB before, versus 42.860, 42.506 and 42.410 dB after. IDR reconstructions are unchanged.

### Runtime cost

Five alternating QP28 calls per arm run on CPU 0 of a shared two-vCPU AMD EPYC KVM host, with no overlapping build or test. All repeats preserve the expected complete stream and reconstruction. The measured ELFs identify rustc 1.99.1. Native timing includes CoreAV decoder verification and file IO; x264 timing includes FFmpeg startup and stream IO but no decoder verification. These are driver reference figures, not matched-quality encoder speed ratios.

| Input | Before fps, median (range) | Selected fps, median (range) | x264 fps, median (range) |
|---|---:|---:|---:|
| A | 54.513 (50.785–57.500) | 30.544 (30.141–30.781) | 433.257 (364.745–445.006) |
| B | 53.287 (50.491–53.993) | 21.890 (20.918–21.969) | 436.963 (418.901–526.775) |
| C | 47.922 (46.560–48.462) | 37.567 (33.025–37.726) | 437.831 (342.312–453.337) |

Byte-preserving diagnostic counters explain the extra work. Complete regular P-inter evaluations rise from 10,040 to 29,426 on A, 13,746 to 55,964 on B, and 23,731 to 31,529 on C. Complete P-intra pairs rise from 5,441 to 23,432, 6,380 to 41,072, and 11,810 to 22,620 respectively. A smaller syntax-only floor rejects fewer non-exact skip competitors. Skip-evaluation and IDR-pair counts remain unchanged. B's small quality change, 44.152652 to 44.448141 dB at 126.776 to 127.448 kbit/s, comes with a substantial runtime cost; repeated content is not a useful motion-quality result.

### Bounded proposal regret

After fixing q²/8, a logging-only diagnostic samples 72 macroblocks per input across P pictures 1, 4, 8, 16, 24, 33, 40 and 59. Each picture uses a deterministic 3×3 spatial grid at one sixth, one half and five sixths of the coded dimensions. The complete production streams and reconstructions remain unchanged. Trials use the same reconstructed reference, neighbours, pending skip run, admission and coupled complete macroblock objective as the production winner.

Ranking trials substitute up to ten distinct already-completed probe vectors per partition in the represented layouts, prioritizing the retained SATD finalists before the remaining SAD-ranked probes. Each trial completes the whole coupled macroblock and coalesces identical vector maps. Additional budget trials add supported 16×8 and 8×16 layouts, omitted ordinary layouts when a bound prevented their construction, and a fixed set of unprobed axial and diagonal vectors. Rectangular diagnostic SATD sums the same 4×4 transforms over the actual rectangle. Neither independent block scores nor diagnostic winners change production state.

| Input | Positive / zero ranking regret | Positive / zero additional budget regret | Admitted ranking / budget trials | Maximum total trials per macroblock |
|---|---:|---:|---:|---:|
| A | 0 / 72 | 0 / 72 | 1,698 / 4,663 | 119 |
| C | 5 / 67 | 0 / 72 | 1,934 / 4,800 | 116 |

Four C macroblocks improve using retained SATD vectors; remaining completed probes also improve three cases, including two already improved cases, for five positive macroblocks in total. The largest local objective reduction is 6.119%; the total is 1,931 SSE-equivalent objective units across the sample. Additional budget trials do not improve the ranking result. All 13,095 attempted trials are admitted and stay below the 128-trial cap. These are bounded one-part-at-a-time counterfactuals, not a global optimum, exhaustive finalist combinations or a predicted BD-rate gain.

No production tool or probe budget is widened: this sample gives no additional gain from the wider budget, and the calibration already spends substantially more runtime. The remaining ranking loss and repeated-content runtime cost are explicit follow-up candidates. C is low motion; no sustained-motion generalization is made, and such a claim needs another existing input selected before seeing encoder results.

### Verification

Both encoder modes pass 52 unit tests, including the original all-IDR byte fixture, every admitted QP's cost units, exact-bound equality and zero-error decisions. Both Clippy modes, formatting, 31 Python metric/receipt tests, unsafe inventory and issue-reference checks pass. Across nine complete curve/sanity cells plus 48 cropped and synthetic boundary cases, dispatched and scalar builds preserve identical streams and reconstructions for 732 pictures; pinned FFmpeg matches every reconstruction. The full three-decoder CI and independent review remain separate PR gates.
