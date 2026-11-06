# coreav-kernels-hevc

Allocation-free HEVC intra/inter prediction, inverse reconstruction and in-loop filter kernels for Main 8-bit and Main10 8/10-bit 4:2:0 samples, with safe scalar implementations and runtime-selected AVX2 interpolation.

This crate is unpublished and has no default features. Enable `native-hevc` explicitly to expose its kernels. HEVC may be covered by patents; this feature is not a patent licence. It must not enter a default consumer dependency graph.

## Interface

`BitDepth`, `BlockSize` and `IntraMode` have checked constructors. `Block` and `BlockMut` borrow caller-owned slices and check the stride, slice extent and address arithmetic. All blocks use row-major samples; the last row needs only its active width. Padding is untouched.

- `predict_intra` substitutes unavailable references, applies normative luma reference filtering and strong smoothing when eligible, and predicts planar, DC or any of the 33 angular modes. References are `Option<u16>` in bottom-left-to-corner-to-top-right order: `p[-1][2*N-1]` through `p[-1][-1]` through `p[2*N-1][-1]`. The first `4*N+1` entries are consumed. Chroma uses 4:2:0 rules, without luma filtering.
- `inverse_residual` takes signed-16 coefficient levels, effective component QP and a fully expanded, nonzero `u8` scaling matrix in raster order. For a disabled scaling list, supply all 16s. It performs inverse scaling, exact integer inverse DCT or intra-luma 4x4 DST, prescribed intermediate clipping, and final rounded shifts. Transform skip is a distinct 4x4 scaling/shift path. Transquant bypass copies coefficients without using QP or scaling factors.
- `add_residual` adds signed residuals to validated `u16` prediction samples and clips to the declared depth.
- `deblock_luma` and `deblock_chroma` filter four complete edge lines with checked depth/QP inputs and independently protected sides. The caller selects eligible boundaries and effective offsets. `chroma_qp_index` supplies the shared Table 8-10 mapping used by inverse quantization and chroma filtering.
- `sao_sample` applies decoded band or edge offsets to an unprotected sample. The caller supplies available neighbours from the unmodified deblocked component, never an already offset destination.
- `interpolate_inter` borrows a full coded `RefPlane` and a rectangular `RectMut<i32>`, producing a reusable signed internal-precision predictor without final clipping. `InterPrediction` gives the coded destination origin in component samples and a resolved signed `MotionVector` in quarter-luma or eighth-chroma units. For 4:2:0 the two motion vectors have the same numerical components. Every reference tap replicates the coded picture edge. Luma uses eight taps and chroma four taps; two-axis filtering preserves the prescribed first-stage shift before the second stage. The complete clipped filter-support rectangle is depth-checked before output writes; reference padding is ignored.
- `predict_uni` and `predict_bi` round and clip one or two internal predictors. `weight_uni` and `weight_bi` accept checked `PredictionWeight` values: the effective denominator and weight, and the offset before depth scaling. The caller derives effective luma/chroma weights and offsets from syntax. Two-list denominators must match. The enclosing predictor ranges are −16830..=33150 at eight bits and −16880..=33247 at ten bits, so both depths require `i32`; all active predictor values are checked before writes. Weighted arithmetic uses `i64`.

`Rect` and `RectMut` check rectangular dimensions in 1..=64, independent row strides, address arithmetic and the last active sample. Interpolation further limits chroma to 32x32. No extra row padding is required or modified. `RefPlane` checks the full coded reference geometry independently of block bounds. Interpolation caches up to eight horizontal rows in fixed stack scratch. The caller owns and charges each rectangular `i32` predictor buffer, as well as the exported `SCRATCH_BYTES` envelope.

The caller derives neighbour availability, resolved chroma modes, QP and depth offsets, scaling-list inheritance/expansion, transform selection, reference lists and motion candidates from syntax. Intra-luma 4x4 uses `IntraLumaDst4`; chroma 4x4 uses `Dct`. PCM is copied by the caller and does not pass through inverse scaling. Range extensions, residual DPCM, cross-component prediction, coefficient rotation and intra-smoothing disable controls are outside this crate's Main/Main10 scope. No bitstream reader, allocation policy or decoder is provided.

## Dispatch and safety

With `native-hevc`, x86-64 builds select AVX2 interpolation only after cached CPU and OS capability detection. Unsupported CPUs and other architectures use the scalar twin. The `scalar` feature forces scalar kernels and compiles no SIMD unsafe code. The feature-disabled crate remains `no_std`; automatic x86-64 detection uses the standard library only in native-enabled, non-scalar builds.

Hand-written unsafe intrinsics are confined to the private `src/simd.rs` module. All other production modules forbid unsafe code. Vector accesses stay within active borrowed regions, with scalar tails and no padding over-reads. [The SIMD design](../../docs/design/hevc-simd.md) defines the exception, numerical invariants, scalar equality tests and differential fuzz targets. [The unsafe inventory](../../docs/unsafe-sites.md) lists each operation and its preconditions.

`SCRATCH_BYTES` bounds the fixed internal numerical arrays for one call, separately from caller-owned input/output buffers. The decoder charges it to its total reconstruction-memory budget.

## Derivation and verification

The only algorithm source is ITU-T H.265 V11 (01/2026), clauses 8.4.4, 8.5.3.3.3–8.5.3.3.4, 8.6.2–8.6.7 and 8.7, including equations 8-214–8-277 and Tables 8-8/8-9 for inter prediction, and equations 8-316, 8-319 and 8-321 for the integer transform tables. Table storage uses frequency-major rows; the specification's two-dimensional arrays use the first index for the horizontal coordinate. No floating-point transform approximation is used.

The half-sample luma filter has positive coefficient sum 88 and negative magnitude sum 24, enclosing the other phases and chroma filters. For maximum sample M and first-stage divisor D=2^(depth−8), first-stage extrema are L=floor(−24*M/D) and U=floor(88*M/D). The enclosing two-axis extrema are floor((88*L−24*U)/64) and floor((88*U−24*L)/64). The internal predictors are uncentred, exactly as specified; no additional internal offset is introduced.

The pinned PDF has SHA-256 `78b093480e178a2b99d8cb7499934240f359c50c6d09568f8be651bc1151d4d2`. Unit tests derive expectations from equations and separately transcribed basis rows; they do not use media or another decoder. They cover both depths, every supported size and prediction mode, non-DC transform impulses, signed extremes, intermediate clipping, effective-QP extremes, custom scaling factors, missing neighbours and checked buffer boundaries. They are kernel checks, not decoder-conformance evidence.
