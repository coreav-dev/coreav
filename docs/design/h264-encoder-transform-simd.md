# Bit-identical H.264 transform SIMD

## Scope

Accelerate the opt-in encoder's existing 4x4 forward transform, zero test, quantization and inverse path under [policy 0044](encoder-policy.md#0044-simd-kernels-for-the-opt-in-h264-encoder). Mode decisions, DC-specific luma and chroma transforms, quantizer constants, coefficient admission, syntax and threading remain unchanged. There is no 8x8 transform in this encoder.

The current scalar path combines residual extraction with its forward transform, uses a precomputed exact reciprocal for quantization, and combines inverse scaling, range validation and reconstruction. Vector arithmetic must preserve those contracts rather than reintroduce an older decomposition. Initial function-level attribution compares unchanged main with an output-identical attribution build on the existing five reference inputs, including controls for attribution overhead. Only a path with useful end-to-end headroom proceeds.

## Arithmetic and interfaces

The existing private `Avx2` capability and shared cached detection select new arithmetic in a private child of the SIMD module. Every non-SIMD module keeps `forbid(unsafe_code)`. The `scalar` feature excludes these paths and retains the scalar twins. A new public kernel crate or a caller-owned function table would add an API without a consumer, so neither is introduced.

The safe transform owner continues to establish complete borrowed rows before any raw load. Forward transformation reads exactly four samples in each of four source and prediction rows. Signed residuals are bounded by 255 and forward coefficients by 36 times 255. The private `Forward` type carries that bound into quantization; construction stays inside the transform owner. Vector quantization borrows the same immutable QP table as scalar arithmetic, including its bias and reciprocal, rather than recomputing or duplicating constants.

Quantization preserves the exact unsigned product and right shift: `(128 * abs(coefficient) + bias) * reciprocal >> 40`, followed by the original sign restoration and optional DC removal. The current table's multiplicands fit unsigned 32 bits, and their product fits unsigned 64 bits. The zero test preserves its inclusive threshold and DC exclusion. Tests cover every coefficient magnitude and QP, including exact ties, rather than using approximate reciprocal arithmetic.

Inverse scaling retains wrapping 32-bit multiplication and the same signed-16-bit admission checks before the DC override. The inverse transform uses signed 32-bit lanes and checks every intermediate that the scalar butterfly checks, plus the final upper bound of 32735 before rounding. The DC-only shortcut, zero-block behavior, pixel rounding and clipping, and error ordering remain authoritative in the shared safe path. No saturated intermediate may turn a scalar refusal into success.

## Verification and delivery

Keep callable scalar twins for differential tests and fuzzing. Compare basis, full-swing and random residuals; every QP and quantizer boundary; signed scaling and inverse-range boundaries; DC overrides; and malformed, unaligned and exact-final-byte views. On an AVX2 host the tests explicitly require detection to produce a capability, preventing a scalar-versus-scalar pass. Run bounded AddressSanitizer differential targets on x86 Actions.

Complete streams and reconstructions must match main and a scalar-forced candidate on all five inputs and every checked QP. The encode-only comparison uses explicit commits, distinct executables and separate target directories, alternating calls on one pinned CPU, with no builds or decoder checks in the timed region. Retain only a gain beyond per-cell variation; report a miss without a performance PR. Run both build modes, strict encoder and workspace Clippy, formatting, unsafe-site and workspace checks, the private-term check and exact-head similarity guard. The same PR corrects the preceding SIMD change's fuzz inventory wording and its AVX2-detection assertion.

## Reference outcome

Attribution on the five existing inputs at QP22 and QP28 puts the four selected paths at 10.6–13.2% of encoder time. The output-identical attribution build's median time is 0.977–1.006 times main in its alternating controls, so the stage shares are approximate, not an independent speed result. The largest selected path is forward transformation on A and B, and reconstruction on C, D and E.

The [per-call reference](h264-encoder-transform-simd-reference.json) compares the vector arithmetic with main in five x86 Actions jobs, using the same toolchain and separate target directories for main, vector and scalar-forced arms. Inputs are the existing lossy prepared pictures. Timing uses sixty pictures, GOP32, enabled deblocking, one pinned CPU, a warm-up and seven alternating repeats at each QP22/28/34/40 cell. It includes encoding and identical stream/reconstruction accumulation, and excludes input loading, output writes and decoder verification. CPU models and every observation are retained with the reference; only paired arms within the same job are comparable.

Across all twenty cells, the ratio of median main time to median vector time is 1.0158–1.0474. A separate noise check bootstraps each cell's median paired time ratio, and every such interval lies above 1.0. These short shared-runner observations support retaining this arithmetic change, not a representative speed claim or a claim about significance across all cells. All thirty input/QP identity cells preserve the complete stream and reconstruction against main and the scalar-forced build, and the qualified pinned decoder matches every warm-up reconstruction. The generated scalar/vector comparison also passes all 48 cases and 192 pictures.

Both build modes pass 64 release tests and strict encoder Clippy. Workspace Clippy, formatting and the unsafe inventory pass on Actions. A bounded AddressSanitizer campaign completes 6,146,188 transform differential inputs without a finding; the unchanged SAD and SATD targets also pass. Quantization tests cover every signed coefficient magnitude through 9,180 at every QP, and inverse tests cover signed intermediate boundaries and every admitted DC value. Final-head similarity and review evidence belong to the pull request.
