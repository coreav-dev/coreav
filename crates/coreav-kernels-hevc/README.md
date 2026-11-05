# coreav-kernels-hevc

Safe, allocation-free scalar HEVC intra prediction and inverse reconstruction kernels for Main 8-bit and Main10 8/10-bit 4:2:0 samples.

This crate is unpublished and has no default features. Enable `native-hevc` explicitly to expose its kernels. HEVC may be covered by patents; this feature is not a patent licence. It must not enter a default consumer dependency graph.

## Interface

`BitDepth`, `BlockSize` and `IntraMode` have checked constructors. `Block` and `BlockMut` borrow caller-owned slices and check the stride, slice extent and address arithmetic. All blocks use row-major samples; the last row needs only its active width. Padding is untouched.

- `predict_intra` substitutes unavailable references, applies normative luma reference filtering and strong smoothing when eligible, and predicts planar, DC or any of the 33 angular modes. References are `Option<u16>` in bottom-left-to-corner-to-top-right order: `p[-1][2*N-1]` through `p[-1][-1]` through `p[2*N-1][-1]`. The first `4*N+1` entries are consumed. Chroma uses 4:2:0 rules, without luma filtering.
- `inverse_residual` takes signed-16 coefficient levels, effective component QP and a fully expanded, nonzero `u8` scaling matrix in raster order. For a disabled scaling list, supply all 16s. It performs inverse scaling, exact integer inverse DCT or intra-luma 4x4 DST, prescribed intermediate clipping, and final rounded shifts. Transform skip is a distinct 4x4 scaling/shift path. Transquant bypass copies coefficients without using QP or scaling factors.
- `add_residual` adds signed residuals to validated `u16` prediction samples and clips to the declared depth.

The caller derives neighbour availability, resolved chroma modes, QP and depth offsets, scaling-list inheritance/expansion and transform selection from syntax. Intra-luma 4x4 uses `IntraLumaDst4`; chroma 4x4 uses `Dct`. PCM is copied by the caller and does not pass through inverse scaling. Range extensions, residual DPCM, cross-component prediction, coefficient rotation, intra-smoothing disable controls, inter prediction and in-loop filters are outside this crate's Main/Main10 scope. No bitstream reader, allocation policy, SIMD dispatch or decoder is provided.

`SCRATCH_BYTES` bounds the fixed internal numerical arrays for one call, separately from caller-owned input/output buffers. The decoder charges it to its total reconstruction-memory budget.

## Derivation and verification

The only algorithm source is ITU-T H.265 V11 (01/2026), clauses 8.4.4 and 8.6.2–8.6.7, including equations 8-316, 8-319 and 8-321 for the integer transform tables. Table storage uses frequency-major rows; the specification's two-dimensional arrays use the first index for the horizontal coordinate. No floating-point transform approximation is used.

The pinned PDF has SHA-256 `78b093480e178a2b99d8cb7499934240f359c50c6d09568f8be651bc1151d4d2`. Unit tests derive expectations from equations and separately transcribed basis rows; they do not use media or another decoder. They cover both depths, every supported size and prediction mode, non-DC transform impulses, signed extremes, intermediate clipping, effective-QP extremes, custom scaling factors, missing neighbours and checked buffer boundaries. They are kernel checks, not decoder-conformance evidence.
