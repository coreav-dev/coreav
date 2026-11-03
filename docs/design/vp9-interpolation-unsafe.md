# Scoped exception for one VP9 AVX2 interpolation kernel

- Status: Proposed. Independent review of this record and the pixel-round plan precedes any unsafe implementation commit.
- Date: 2026-11-03.
- Parent policy: 0001, sections 2 through 6, with the site inventory required by policy 0041.
- Scope: `coreav-kernels-vp9` only.

## Decision

This record permits exactly one x86-64 AVX2 arithmetic kernel, `horizontal8_avx2`, in the private module `crates/coreav-kernels-vp9/src/simd.rs`. It computes eight adjacent horizontal eight-tap interpolation sums for an unscaled eight-bit prediction. All four specification filter tables use the same function. It does not implement a second vertical, compound, transform, intra or loop-filter SIMD kernel.

The public kernel entry remains safe and takes the existing checked views. A private hook on the sealed sample trait selects the concrete `u8` implementation without reinterpreting a generic sample slice. `u16` storage, non-eight-bit requests, scaled grids and unsupported targets keep the safe scalar path. A declined fast path leaves the destination unchanged. Scalar tails complete an admitted block without an overread.

## Fixed memory and arithmetic contract

The arithmetic kernel takes `&[u8; 15]` and `&[i16; 8]` and returns `[i32; 8]`. For output lane `c` from 0 through 7, it sums `samples[c + t] * weights[t]` for taps `t` from 0 through 7. The union of the eight windows is exactly fifteen bytes. The function writes no destination memory and retains no reference.

Safe Rust constructs the fixed input reference from a checked visible-row slice. The source view does not promise padding after its final row. The wrapper checks the whole admitted block's actual source support in signed 64-bit arithmetic before converting offsets. It never treats stored row padding as visible pixels. Transfer tap windows through safe fixed-size byte-array reads and register values, without raw-pointer loads, raw-pointer stores, pointer arithmetic or a register-to-array transmute.

The specification table rows sum to 128. Their maximum absolute coefficient sum is 236. Byte inputs give partial and final sums between -13,770 and 46,410, which fit signed 32-bit lanes. Signed 16-bit accumulation is insufficient. The scalar reference and tests check the bounds against every table row. Even arbitrary `i16` coefficients fit the private function's signed 32-bit accumulation bound of `8 * 255 * 32768`.

The function returns sums before rounding. Shared safe code retains horizontal `Round2(sum, 7)` and clipping before vertical convolution, vertical rounding and clipping before compound averaging, and the specified rounded compound average. No pass is fused. The scalar fallback preserves the existing behavior for mismatched sample types and depths and for `u16` samples above the nominal bit-depth range.

## Feature detection and confinement

A private detected capability is constructible only after `is_x86_feature_detected!("avx2")` succeeds. Its safe wrapper owns the sole call to the `#[target_feature(enable = "avx2")]` arithmetic function. Detection occurs once per prediction block, not once per tap or output sample. No safe caller can manufacture the capability or invoke the target-feature function directly.

Only this SIMD module may allow unsafe code. Its permitted operations are the target-feature function and call and the register intrinsics needed for the one kernel. The crate copies the complete workspace lint table with only `unsafe_code` changed from `forbid` to `deny`. Every other module forbids unsafe code. `coreav-vp9` and its backend retain their workspace `forbid` and gain no exception.

The kernel crate's `safe` Cargo feature excludes the SIMD module at compile time and restores the crate's `forbid`. Other architectures also compile no AVX2 module. This is a guarantee about this VP9 kernel crate, not a whole-program guarantee covering unrelated dependencies or backends. Default x86-64 builds detect AVX2 at runtime and otherwise use scalar. Builds used for detection tests do not globally enable AVX2.

## Required evidence

The same pull request adds the manifest, module-declaration and SIMD-file rows to `docs/unsafe-sites.md`. Each row states its operations and preconditions. No row may imply permission for another kernel.

Tests compare the private arithmetic kernel with its safe twin and compare complete automatic predictions and optimized scalar predictions with the specification-based scalar reference. They cover all filters and phase pairs, lane order, signed sums, clipping, compound output, exact-size backing slices, row gaps, tails, edges, coordinate extremes, scale steps and full `u16` sample values. AVX2 hosts must prove that the direct kernel test actually executed the kernel. Refused admission must preserve the destination before fallback.

The safe entry has a differential fuzz target. Existing C1 conformance, public-boundary and oracle checks remain unchanged and run with automatic and safe kernels. The existing C3 differential-fuzz duration and architecture requirements are not reduced. An independent reviewer clears this record and the plan before any unsafe code is committed and reviews the implemented unsafe boundary before merge.

## Limits of this exception

This is not the H.264 parsing exception of policy 0038. No bitstream parser, reference management, row packing or wide loop filter may contain unsafe code under this record. Compiler autovectorization of safe Rust does not add an explicit SIMD kernel.

Every additional kernel needs its own new, separately reviewed scoped exception record and a new maintainer decision. This record remains limited to `horizontal8_avx2` and cannot be amended to authorize another kernel. Another target architecture, explicit vectorized vertical or compound stage, raw-pointer memory operation, parsing exception or wider unsafe module is also outside this permission and needs a new decision and a separate record. A design note or an inventory row alone cannot widen this permission.
