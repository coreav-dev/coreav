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
