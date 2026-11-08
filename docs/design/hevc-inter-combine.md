# HEVC inter prediction combination

This family changes only the opt-in HEVC kernel crate's uni, bi and weighted prediction combination under H.265 V11 (01/2026), clause 8.5.3.3.4. Interpolation, reference-picture selection, parsing, default features and decoder isolation stay unchanged.

## Safe arithmetic first

Validate shapes, matching denominators and every active predictor before writing any output. Borrow active rows with their independent strides, so padding is neither validated nor modified. Select uni or bi once per block rather than inside every sample. Keep the original checked, signed `i64` equations as a test-support reference.

Every admitted internal predictor has magnitude at most 33,247. Effective weights have magnitude at most 255, scaled offsets at most 512, and the initial shift is in 4..=13. Even the conservative bi bound `2*33247*255 + (2*512+1)*8192 = 25352770` is below `2^25`, so products, offset sums and rounding fit signed `i32`. The bound deliberately combines maxima from different depths. Signed right shifts and final clipping remain in their normative order; in particular, bi offsets are not rounded separately.

## Dispatch boundary

Both depths use the safe row implementation. This family adds no runtime capability check, target-feature function or unsafe code. The `scalar` feature continues to exclude existing SIMD in other kernel families without changing these combination equations.

## Predictor-domain validation

Range validation changes only for blocks with at least eight active samples per row, in both eight-bit and Main10 decoding. A cached private AVX2 capability selects signed comparisons over complete groups of eight `i32` predictors in `simd/combine.rs`. Dispatch occurs once before the validation loops, and one target-feature call covers both predictors. The safe scalar predicate checks every tail. Loads stay within the active borrowed row, without reading padding or writing output. Both predictors still finish validation before combination starts; shape and denominator checks retain their precedence.

Combination products, shifts, clipping and stores remain in the existing safe implementation. Unsupported CPUs, narrow rows and scalar-forced builds retain the original scalar validation loops. Full-output tests and the existing combination differential target compare valid and invalid lanes with the literal reference. Reference comparisons report every cell and any regressions; instruction counts distinguish extra shared-path work from timing or layout effects before scope decisions.

## Verification

Compare automatic and scalar-forced results with the literal reference over both depths, all denominators, effective-weight and offset extremes, uni and bi, widths 1..=64, independent strides, unaligned starts and exact final extents. Preserve rejection-before-write checks. A bounded differential fuzz target exercises the same safe interfaces. Both full conformance modes and the census remain required before shipping.

Reference comparisons bind every arm to an exact commit, verify pixels with the qualified pinned oracle before timing, and check decoded work on every call. Baseline and safe rows run on the same pinned Actions CPU with alternating repeated calls. Instruction attribution is not a CPU-time share or a promised speedup; private reports retain the host, raw calls and CoreAV/FFmpeg ratios.
