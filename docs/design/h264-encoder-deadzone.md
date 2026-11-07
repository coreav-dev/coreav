# H.264 dead-zone quantization

The encoder uses a wider zero interval, with luma offsets of 5/12 for intra prediction and 1/6 for inter prediction, and a chroma offset of 5/12 for both. Inverse scaling and reconstruction are unchanged. This replaces nearest rounding with ties toward zero and implements compression item 1 of the [pipeline roadmap](h264-encoder-pipeline.md).

## Arithmetic contract

For signed numerator `n`, positive divisor `d` and an offset `f = k / 12`, the signed level is `sign(n) * floor((abs(n) + floor(d * k / 12)) / d)`. Ordinary 4x4 coefficients use `n = 64 * coefficient` and the existing position-dependent divisor. Luma DC and chroma DC keep their distinct scale formulas and apply the same prediction-specific offset. The quantizer's zero test is derived from this exact integer formula, including its boundary equality.

The ordinary scalar kernel retains its precomputed reciprocal, integer multiply and shift. Its stored bias is `floor(2 * d * k / 12)`; tests prove equality to direct division for every coefficient in the bounded 8-bit residual domain at every QP 0–51. A later vector kernel must reproduce these integer levels, sign restoration, separate-DC policy and zero decisions exactly. Inverse scaling and candidate admission remain unchanged.

Malvar, Hallapuro, Karczewicz and Kerofsky, [Low-Complexity Transform and Quantization in H.264/AVC](https://www.microsoft.com/en-us/research/publication/low-complexity-transform-and-quantization-in-h-264avc/), IEEE TCSVT, July 2003, describes a dead-zone control parameter in the range zero to one half and division-free quantization. The offset pairs below are fixed experiments inside that range, not code or constants copied from another encoder.

## Selection

Compare the six pairs formed by intra offsets `1/3`, `5/12` and inter offsets `1/6`, `1/3`, `5/12` against current main. Use existing reference inputs at QP 22, 28, 34 and 40, one thread and the current GOP, deblocking and search settings.

Select the pair with the lowest arithmetic-mean Y-PSNR BD-rate across all five inputs, provided no input is worse than +0.5%. Report every pair tried, not only the selected pair. Compute BD-rate with PCHIP interpolation of log rate over the common PSNR interval, without extrapolation. If none qualifies, retain the current quantizer rather than silently relaxing the bound.

The original luma-selected pair is intra 5/12 and inter 1/6. It has the lowest mean Y-PSNR BD-rate of the six pairs and improves luma on every input. The pull request records the complete comparison. Luma retains these offsets. The colour-plane follow-up selects 5/12 for both intra and inter chroma, for ordinary chroma coefficients and chroma DC. Offsets depend on the prediction type, not on the slice type: an intra macroblock in a P picture uses the intra luma offset. The quiet-skip chroma DC test uses the chroma inter offset. QP admission remains 10–51; QP 0 is an arithmetic-test case only.

### Colour-plane validation

The original selection used Y PSNR only. Quantizer comparisons now report separate Y, U and V BD-rates and a weighted curve whose quality is `(6 * PSNR_Y + PSNR_U + PSNR_V) / 8` for each encoded point. The weighted result is a BD-rate integrated over that quality curve, not an average of three BD-rate percentages. `scripts/h264_quantizer_report.py` owns this calculation and rejects missing planes, nonfinite scores, incomplete QP sets, duplicate quality points and failed oracle comparisons. Integration uses only each pair's common quality interval. Points are sorted by quality without smoothing or replacing the observations; the report flags nonmonotone rate curves, which can occur when a chroma plane's quality does not decrease monotonically with QP.

Chroma offset changes keep the luma offsets fixed. They must keep each input's Y, U and V BD-rate no worse than +0.5% against nearest rounding, while the five-input mean weighted BD-rate is not worse than the original dead-zone setting.

The follow-up compares chroma intra offsets 5/12 and 1/2 with chroma inter offsets 1/4, 1/3, 5/12 and 1/2, while retaining luma intra 5/12 and luma inter 1/6. These eight pairs are fixed before their comparison. The half-step endpoint uses the same integer-floor contract as the other offsets. The initial per-plane check found chroma regressions beyond the gate, so retaining the original shared offsets is not an accepted outcome.

Chroma intra 5/12 and inter 5/12 is the best pair that passes every input's plane gate and improves the mean weighted figure. It narrows the inter chroma zero interval compared with the original dead zone; luma decisions still use their unchanged quantizer, while the chroma portion of quiet-skip admission uses the corrected chroma threshold.

Measure encode-only speed in the same Actions jobs, alternating release-build arms with separate target directories. Every output must decode with the pinned FFmpeg to its reconstruction. Regenerate output fixtures only after selection. No motion-search, motion-compensation, SIMD, threading, profile or public-settings change is included.
