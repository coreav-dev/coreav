# HEVC inter interpolation kernels

This is the first milestone-5 family for the opt-in HEVC decoder. It changes reference sampling and separable interpolation only. Parsing, reconstruction decisions, output ordering, backend selection, default features and licences stay unchanged. Algorithms come only from ITU-T H.265 V11 (01/2026), clause 8.5.3.3.3 and Tables 8-8/8-9. Vendor ISA documentation defines intrinsic semantics.

## Scoped SIMD policy

Only the private `simd` module of `coreav-kernels-hevc` may contain unsafe SIMD arithmetic. The crate copies the workspace lint table with `unsafe_code = "deny"`; every other production module forbids unsafe code. No parser, decoder, backend or consumer gains an exception. Safe interfaces accept bounded slices, not raw pointers. Each unsafe file and lint exception has a row in `docs/unsafe-sites.md` describing every operation and its safety preconditions.

Runtime CPU detection must establish AVX2 support before a target-feature call. Detection is cached, and unsupported CPUs use safe scalar kernels. The `scalar` feature forces the scalar implementation and excludes the SIMD module. No process-wide target-CPU flag is required. HEVC remains disabled without `native-hevc`.

Each vector kernel preserves the scalar twin's full admitted domain, including arithmetic widths, signed shifts, rounding, clipping and error behaviour. Tests cover both depths, all widths and phases, independent strides, picture edges, extreme samples and malformed inputs. Each new kernel has a differential fuzz target through its safe interface. Loads and stores must stay within the borrowed active region, including the final vector and scalar tail. Padding over-reads are forbidden.

A vector path remains only when a reference comparison shows a useful benefit against the safe implementation. A path without that benefit is removed. At the maintainer's direction these are reference numbers, not judged thresholds or registered speed claims. Correctness gates remain unchanged.

## Reference sampling before SIMD

The previous two-axis implementation recomputes each horizontal dot product for every vertical output tap. It also clamps the same reference coordinates for every coefficient. Safe precomputation first resolves the clamped source columns and borrows one active reference row at a time. A bounded row cache reuses horizontally filtered rows across vertical outputs. The first-stage shift still precedes the vertical dot product. Complete filter-support sample validation precedes output writes, including samples whose coefficient is zero.

Horizontal SIMD computes eight independent output positions from bounded `u16` source rows with signed `i32` products and sums. Vertical SIMD combines eight independent signed internal predictors per lane. Scalar tails handle all remaining widths. Both bit depths fit `i32`: the maximum absolute filter coefficient sum is 112, and the conservative two-axis bound `112*112*1023/256` is below `i32::MAX`. No intermediate is narrowed to `i16`.

Scratch storage is fixed, allocation-free and included in `SCRATCH_BYTES`, which the decoder already charges before reconstruction. The public interpolation signature and rejection contract do not change.

## Verification and reference comparison

C3 requires scalar-versus-automatic property tests and differential fuzzing for at least ten minutes per family on each target architecture. Earlier C1 and C2 gates run with scalar-forced and automatic dispatch at the final head. Exact output hashes, counts, geometry and depth remain required; a speed result cannot excuse a mismatch.

Reference comparisons use the existing development identities, pinned FFmpeg 9.0.2, identical compressed prefixes and a single Actions job for scalar-baseline, safe-precomputed and automatic builds. Instruction counts and rotated repeated wall and CPU times distinguish safe reuse from the incremental vector benefit. Pixel verification precedes timing. Host and raw per-call values accompany any reported reference numbers. NEON, inverse transforms, threading and backend wiring are outside this family.
