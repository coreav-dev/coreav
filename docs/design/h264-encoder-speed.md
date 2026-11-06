# Recovering H.264 encoder throughput after the rate-weight change

## Scope and starting point

The quantizer-derived P-picture rate weight in [h264-encoder-rate-weight.md](h264-encoder-rate-weight.md) improved compression on A and C but lowered QP28 driver throughput from 54.5, 53.3 and 47.9 pictures per second to 30.5, 21.9 and 37.6 on A, B and C. Its byte-preserving counters attribute the cost to more complete P-inter evaluations and more P-intra proposals, because a smaller syntax floor rejects fewer competitors of a non-exact skip. This change recovers that throughput from the merged rate-weight commit. It keeps the objective, normative admission, reconstruction, quantization, SIMD arithmetic, tie ordering, the IDR decision path, default dependencies, licensing and product commands unchanged. The [search design](h264-encoder-search.md) and its reviewed notes remain the only implementation inputs.

The target is QP28 driver throughput on A, B and C close to the values before the rate-weight change, with no BD-rate loss on A or C against the rate-weight curves beyond noise. Noise here means changes of a few hundredths of a percent, the size produced by reordering equal-cost work. BD-rate is deterministic, so any larger loss is a real tradeoff and must be disclosed rather than averaged away.

## Order of work

1. Commit this plan before changing code.
2. Remove work without changing output. Candidate sites come from profiles of the rate-weight build: per-sample clamped reads for motion-compensation windows and edge SAD rows, repeated tile copies in the 4x4 intra proposal and exhaustive loops, bit-at-a-time writes, divisions and inverse checks for coefficients that are already zero, and deblocking edges whose boundary strengths are all zero. Every such change must leave the stream and reconstruction byte-identical to the rate-weight build at QP22, 28, 34 and 40 on A and C and at QP28 on B, in both the dispatched and scalar builds.
3. Only then add early decisions that may change output, each justified by the arithmetic it relies on and accepted only on complete curves:
   - Quiet skip. If every 4x4 luma block, every chroma AC block and both chroma DC blocks of the skip prediction's residual quantize to zero, a coded P16x16 at the skip vector reconstructs the same samples with more syntax, so it cannot beat skip. The macroblock then takes skip without the remaining inter search and the intra proposal. Other vectors and intra could still win in principle, so this is a policy decision measured on curves, not a proof of optimality. The zero test applies the quantizer's own rounding rule without division, and a test proves it equals quantizing and checking for zero at every QP and rounding boundary.
   - Narrower intra proposal. If a complete coded P16x16 at the skip vector does not beat skip, the inter search still runs in full, but the P-intra proposal considers only the 16x16 modes and skips the 4x4 proposal. The complete objective still decides between every evaluated candidate. The duplicate P16x16 plan at the skip vector is evaluated once.
4. Check that the IDR path and the original all-IDR byte fixture are unchanged, and that scalar and dispatched builds stay byte-identical.

## Measurement

Compression uses the same prepared A and C inputs, sixty pictures at 30 fps, GOP32, enabled deblocking and QP22, 28, 34 and 40, with B at QP28 as a sanity check. BD-rate is PCHIP on log bitrate over the common sample-weighted YUV PSNR interval, with no extrapolation. PSNR is recomputed independently from raw squared error, and every new stream decodes with pinned FFmpeg 9.x to the encoder's reconstruction. These curves are deterministic and can be computed on any host.

Throughput is measured only on a GitHub-hosted runner. One job builds three arms with the same toolchain: the commit before the rate-weight change, the rate-weight merge commit and this change. It fetches the existing lossy A, B and C sources, prepares the same raw inputs with pinned FFmpeg and checks each prepared input's recorded SHA-256 before any timing. Each input then runs alternating QP28 calls of the three arms on one pinned CPU, rotating the order each repeat, with no build or other work overlapping the calls. Every call must reproduce its arm's expected stream and reconstruction, and its decoder check must pass. Logs, artifacts and job names use only the labels A, B and C. Driver throughput includes CoreAV decoder verification and file IO, the same scope as the earlier figures. It is a reference figure on a shared runner class, not a judged speed bar or a matched-quality encoder ratio. Local instruction counts guide the work but are not reported as results.

## Correctness and delivery

Run both encoder build modes and their complete-stream identity comparison, encoder Clippy, formatting, the Python receipt tests, the existing decoder CI and the current-main similarity guard. No new unsafe code, public tuning API or dependency is needed. The plan, implementation, review fixes and reference outcome ship in one signed-off pull request.

## Reference outcome

The [reference record](h264-encoder-speed-reference.json) keeps every compression cell, the identity summaries of two case sets and every timed call. The byte-identical commit reproduces the rate-weight stream and reconstruction in every case of both sets: 57 cases covering A and C at four QPs, B at QP28 and 48 small synthetic cases, and the 48 cases of `scripts/h264_g1_simd_identity.py`. Pinned FFmpeg 9.0.2 decodes every stream of the first set to the encoder reconstruction.

The early decisions change output, so identity to the rate-weight build is not expected after them. In the first set they change the nine real-input cells and none of the 48 small synthetic cases. In the second set they change one case: 80x48 edge pattern, QP51, deblocking enabled, IDR interval 3, four pictures, whose stream goes from 868 to 867 bytes. The narrower intra proposal causes it: a P macroblock whose coded P16x16 at the skip vector does not beat skip no longer gets a 4x4 intra proposal, while quiet skip alone leaves this case unchanged. Dispatched and scalar builds remain byte-identical in every case of both sets, and the native decoder matches every reconstruction.

Against the rate-weight curves, BD-rate is +0.032% on A and −0.242% on C. At QP28, B moves from 127.448 to 127.360 kbit/s and from 44.448141 to 44.434092 dB.

Throughput comes from one GitHub-hosted ubuntu-24.04 job on an AMD EPYC 7763 with four visible CPUs, timing calls pinned to one CPU. The job built all three arms with rustc 1.99.1, checked every prepared input's SHA-256 and checked every call's stream and reconstruction. Each input runs one warm-up call per arm and then seven repeats of all three arms in rotated order. Ratios are paired within each repeat. The runner class differs from the host of the earlier figures, so only same-job comparisons are meaningful.

| Input | Before fps, median (range) | Rate weight fps | This change fps | This change / before | This change / rate weight |
|---|---:|---:|---:|---:|---:|
| A | 49.162 (48.123–49.405) | 26.229 (26.078–26.376) | 46.215 (45.299–46.290) | 0.938 (0.919–0.961) | 1.762 (1.724–1.769) |
| B | 44.707 (44.366–45.151) | 18.252 (18.027–18.319) | 40.921 (40.500–41.321) | 0.913 (0.909–0.928) | 2.254 (2.229–2.275) |
| C | 40.821 (40.553–41.298) | 32.333 (32.075–32.615) | 41.436 (41.103–41.557) | 1.013 (1.003–1.023) | 1.284 (1.264–1.290) |

This recovers about 87% of the throughput lost on A, 86% on B and all of it on C. A and B remain 6% and 9% below the build before the rate-weight change. In the local profile, the remaining cost is spread across motion scoring, quantization, deblocking, syntax counting and copies. Closing that gap is left to a later change.

## Second byte-identical round

After this change, A and B remained 6% and 9% below the build before the rate-weight change on the same runner. The follow-up is limited to byte-identical work and starts from a profile of the merged encoder. A site enters the round only if removing its cost would recover at least 3% of QP28 driver time on A or B; otherwise the round stops and reports the profile instead.

The profile's largest encoder entry is the bit writer, at about 5% of samples on A and B. Almost all of it is the counting mode used during mode decisions: each counted syntax element pays a function call and the write path's setup, because one non-inlined function serves both counting and writing. The round keeps the counting path inline and moves byte writing into its own function, without changing any syntax check, limit or written bit. Other entries are each below that threshold: copies are spread over several callers at about 1% each, the 16x16 tile fill in fractional motion scoring is about 1.6%, and deblocking as a whole is about 5.5%.

Verification is the same as for the first byte-identical commit: every case of both identity sets must match the merged encoder's stream and reconstruction in dispatched and scalar builds. Timing again uses one GitHub Actions job with the merged encoder and the candidate, plus the build before the rate-weight change for reference. That reference build also predates later decoder changes in the driver's verification path, so its ratio is context, not a pure encoder comparison.
