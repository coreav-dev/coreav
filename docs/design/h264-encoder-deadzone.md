# H.264 dead-zone quantization

The encoder uses a wider zero interval, with offsets of 5/12 for intra prediction and 1/6 for inter prediction, without changing inverse scaling or reconstruction. It replaces nearest rounding with ties toward zero and implements compression item 1 of the [pipeline roadmap](h264-encoder-pipeline.md).

## Arithmetic contract

For signed numerator `n`, positive divisor `d` and an offset `f = k / 12`, the signed level is `sign(n) * floor((abs(n) + floor(d * k / 12)) / d)`. Ordinary 4x4 coefficients use `n = 64 * coefficient` and the existing position-dependent divisor. Luma DC and chroma DC keep their distinct scale formulas and apply the same prediction-specific offset. The quantizer's zero test is derived from this exact integer formula, including its boundary equality.

The ordinary scalar kernel retains its precomputed reciprocal, integer multiply and shift. Its stored bias is `floor(2 * d * k / 12)`; tests will prove equality to direct division for every coefficient in the bounded 8-bit residual domain at every QP 0–51. A later vector kernel must reproduce these integer levels, sign restoration, separate-DC policy and zero decisions exactly. Inverse scaling and candidate admission remain unchanged.

Malvar, Hallapuro, Karczewicz and Kerofsky, [Low-Complexity Transform and Quantization in H.264/AVC](https://www.microsoft.com/en-us/research/publication/low-complexity-transform-and-quantization-in-h-264avc/), IEEE TCSVT, July 2003, describes a dead-zone control parameter in the range zero to one half and division-free quantization. The offset pairs below are fixed experiments inside that range, not code or constants copied from another encoder.

## Selection

Compare the six pairs formed by intra offsets `1/3`, `5/12` and inter offsets `1/6`, `1/3`, `5/12` against current main. Use existing reference inputs at QP 22, 28, 34 and 40, one thread and the current GOP, deblocking and search settings.

Select the pair with the lowest arithmetic-mean Y-PSNR BD-rate across all five inputs, provided no input is worse than +0.5%. Report every pair tried, not only the selected pair. Compute BD-rate with PCHIP interpolation of log rate over the common PSNR interval, without extrapolation. If none qualifies, retain the current quantizer rather than silently relaxing the bound.

The selected pair is intra 5/12 and inter 1/6. It has the lowest mean BD-rate of the six pairs and improves every input. The pull request records the complete comparison. Offsets depend on the prediction type, not on the slice type: an intra macroblock in a P picture uses the intra offset. Separate chroma DC follows its macroblock's prediction type, and the quiet-skip DC test uses the inter offset. QP admission remains 10–51; QP 0 is an arithmetic-test case only.

Measure encode-only speed in the same Actions jobs, alternating release-build arms with separate target directories. Every output must decode with the pinned FFmpeg to its reconstruction. Regenerate output fixtures only after selection. No motion-search, motion-compensation, SIMD, threading, profile or public-settings change is included.
