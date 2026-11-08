# Eight-bit HEVC residual reconstruction

This change stays inside the opt-in HEVC kernel crate. The first target is prediction-plus-residual clipping, defined by ITU-T H.265 V11 (01/2026), equation 8-325. It leaves dequantization, inverse DCT/DST, transform skip, parsing, filtering and ten-bit reconstruction unchanged.

The checked `Block` types already prove every active row's extent, including a short final row without trailing padding. The eight-bit path borrows each row as a fixed array for sizes 4, 8, 16 and 32 instead of repeating checked coordinate arithmetic for every sample. A complete prediction-domain validation pass still precedes every output write. Invalid sizes and prediction samples retain the same errors, and padding is neither read nor written.

For prediction `p` in 0 through 255 and any signed 32-bit residual `r`, `clip(p + r, 0, 255)` equals `clip(saturating_add(p, r), 0, 255)`. Negative overflow cannot occur because `p` is nonnegative. Positive overflow already implies an output of 255, so saturation cannot change the clipped result. This allows safe signed 32-bit arithmetic without narrowing the residual domain or adding an unsafe exception.

Fixed-size rows and safe arithmetic come before explicit SIMD. The existing ten-bit implementation remains the reference for a separate follow-up; no ten-bit vector path is added here. Scratch allocation and its accounting do not change.

Verification compares every prediction value with extreme and boundary residuals using independent 64-bit addition, covers independent strides and a minimally sized last row, and retains the existing all-size reconstruction and no-partial-write tests. Conformance outputs must remain bit-exact. Reference comparisons build explicit baseline and candidate commits in one Actions job, verify pixels before timing, rotate repeated calls, and retain complete function-level instruction profiles. These are reference figures without a judged speed threshold.
