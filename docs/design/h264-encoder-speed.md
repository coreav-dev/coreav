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
