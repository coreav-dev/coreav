# HEVC intra prediction kernels

This family implements ITU-T H.265 V11 (01/2026), clause 8.4.4.2, for the opt-in Main and Main10 decoder. Its public input, rejection behaviour, reference substitution, smoothing and decoded samples stay unchanged. Interpolation, inter prediction combining, inverse transforms, filters and decoder scheduling are outside this change.

## Safe specialization first

The previous predictor selects a mode, derives its angle, resolves negative references and checks the output address for each sample. It also calculates the DC sum for every mode. The replacement selects planar, DC or angular prediction once per block. Planar prediction hoists its two endpoints and row terms; DC alone computes its sum. Active output rows are borrowed without touching padding.

Angular prediction prepares a bounded main-reference array once. For a side length `N`, logical indices `-N` through `2*N` fit a fixed 97-element array. Positive references follow the selected top or left edge; negative references use the same inverse-angle projection and rounding as the literal equations. Each projected row or column derives its integer offset and fractional weight once, then interpolates consecutive prepared references. Horizontal prediction writes columns without allocating a transposed block.

Reference samples lie in `0..=1023`. Planar and DC outputs and angular interpolation are rounded convex combinations, so their results remain in the sample domain without clipping. Only the luma boundary adjustment for horizontal and vertical modes can leave that domain; it retains signed arithmetic and clipping. Products and sums fit `i32`. The prepared angular array is live after the smoothing copy's scope and fits within the existing scratch bound; resource accounting does not change.

No unsafe code or new CPU requirement is introduced by this first step. Further vectorization requires a separately measured benefit over this safe implementation, the scalar-forced path, differential fuzzing and the private SIMD boundary defined in [the kernel policy](hevc-simd.md).

## Verification

Independent literal prediction equations cover every mode, size and depth. Additional randomized references cover missing samples, smoothing choices, both components, unaligned output starts and exact final-row extents, with padding left unchanged. The existing malformed-input checks retain rejection before output writes. Complete conformance and census checks continue to compare decoded bytes with the qualified oracle. Reference timings compare explicit main and candidate commits on the same runner with identical verified inputs; performance results remain separate from this design.
