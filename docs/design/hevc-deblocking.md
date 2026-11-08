# Eight-bit HEVC deblocking

The checked luma and chroma interfaces retain their fixed four-line arrays, error order and return values. The driver still owns boundary strength, QP derivation, protection and vertical-before-horizontal traversal. Main10 remains scalar. The implementation follows ITU-T H.265 V11 clause 8.7.2 and the existing specification-written scalar equations, not another decoder.

## Dispatch and ownership

Use the existing cached `Avx2` capability to dispatch only eight-bit calls into the private `simd/deblock.rs` family. Its safe methods accept the same fixed-size arrays, not pointers. Luma validates samples before QP parameters; chroma retains its QP-before-sample error order. Shared safe threshold and four-line decision functions remain in `filter.rs`. Unsupported CPUs and the `scalar` feature keep scalar arithmetic; the latter compiles no SIMD module.

The alternative was dispatch after scalar sample validation. That leaves repeated sample checks and array conversion on the hot path. Dispatching the whole checked kernel lets the bounded vector loads also validate samples, without changing the public API or moving driver control into unsafe code. Scalar references remain callable through the existing test-support feature for differential fuzzing.

After input and parameter validation, zero clipping thresholds or two protected sides can return their unchanged edge before arithmetic. Luma's curvature, strong-filter selection and secondary-side decisions remain common scalar calculations across the four lines. They must not become independent per-lane decisions.

## Lanes and numerical bounds

Transpose the four rows into eight signed 16-bit lanes: four P-side samples followed by the corresponding four Q-side samples. Strong luma arithmetic computes both sides with the same weighted equations. Normal luma and chroma compute the rounded P-side delta once, then negate that rounded result for Q. Rounding an independently negated numerator would differ at ties and is not equivalent.

All validated samples are in 0..=255. The largest strong weighted sum is 2,044, the normal luma delta numerator has magnitude at most 3,068, and the chroma numerator has magnitude at most 1,279. The clipping threshold is at most 24, so doubled thresholds, signed differences, absolute values and secondary corrections fit signed 16-bit lanes. Final sample clipping and protected-side selection preserve the scalar results.

Luma reads and writes exactly four 16-byte rows; chroma exactly four 8-byte rows. Fixed-array types prove those extents before a pointer exists. Unaligned intrinsics impose no alignment requirement, no padding is read, no allocation is introduced and no pointer escapes. Each target-feature entry and its load/store operations gets an unsafe-inventory row under record 0045.

## Verification and retention

Compare automatic and scalar results across QPs, threshold offsets, boundary strengths, protection combinations, strong and normal decisions, clipping extremes and invalid samples, including error precedence. A differential fuzz target calls the checked interfaces on both valid and malformed fixed arrays. Native x86 Actions must prove runtime dispatch, and forced-scalar checks must exclude the unsafe module. Complete conformance and census checks run in both modes, along with exact-head similarity and independent review.

Keep the vector family only after an explicit-main-versus-candidate reference comparison shows useful end-to-end benefit. An incremental dispatch control disables only this family and retains existing interpolation and SAO SIMD. These comparisons use the qualified pinned binary oracle, matched picture work, alternating repeated calls and retained no-media results. Instruction removal is only an optimistic selection tool, not a speed claim. Feature defaults, decoder isolation, source-copy accounting and other kernel families remain unchanged.
