# Bit-identical H.264 encoder SIMD

## Scope and profile

The maintainer chose SIMD after the search-efficiency recovery in coreav-dev/coreav#452. This change accelerates arithmetic, not search policy. It implements the narrow [encoder kernel policy](encoder-policy.md#0044-simd-kernels-for-the-opt-in-h264-encoder). Source isolation and the two reviewed technique notes used by the search design remain unchanged; no new implementation source is an input.

The scalar encoder at `22505edba2a909ef658071160b1c65748a8ed3de` is unchanged in the starting tree `09626af9abc319de81f7fab72dc5c529c6711a91`. Release builds retain debug information. On the existing two-core AMD EPYC reference host, `perf record -F 999 -e cycles:u --call-graph dwarf,16384` samples the unchanged 60-picture QP28 driver on inputs A and C. There are no lost samples. Decoder verification and IO are included, so these percentages describe the driver, not isolated encoding.

| Exclusive sampled family | A | C |
|---|---:|---:|
| Inter motion scoring | 8.26% | 13.54% |
| Scalar luma interpolation, including its separate closure | 9.24% | 17.74% |
| Scalar chroma interpolation | 2.35% | 1.61% |
| Quantization | 4.64% | 2.86% |
| Inverse scaling and range validation | 3.46% | 2.93% |
| Scalar inverse 4x4 transform and add | 1.09% | 1.21% |
| SATD | 0.85% | 1.59% |

The first family is prediction: reuse the existing independently tested AVX2 luma and chroma interpolation rather than write another normative filter. Next comes exact row SAD in motion scoring. Existing intra prediction, inverse-transform/add and deblocking kernels are eligible for checked reuse where separate reference measurements show a benefit. Division-heavy quantization and inverse scaling keep their current scalar arithmetic; SATD and forward transforms are not leading hotspots in this profile and are deferred rather than adding unmeasured unsafe paths.

## Dispatch and kernel shape

One immutable, process-local kernel selection is initialized with `OnceLock`. The normal opt-in encoder uses detected AVX2 support and the existing pixel dispatcher with AVX-512 disabled. The `scalar` Cargo feature overrides both selections, without reading an environment variable or changing sequence settings. Unsupported architectures retain scalar SAD; existing NEON pixel kernels can be reused where available. No new NEON arithmetic is needed for this round.

The new SAD interface takes two equal-length rows of at most sixteen samples. Safe checks establish those lengths; unsupported row shapes fall back to the scalar zip reduction. The vector function computes unsigned byte differences and sums in wide lanes. Four-, eight-, and sixteen-byte loads read exactly their admitted extents. Row boundaries and the existing strict partial-SAD cutoff remain in the caller, so candidate order, early rejection, ties, and the probe count cannot change.

Prediction continues to use checked `Block` and `BlockMut` views, including the existing edge-replicated scratch window. Reused inverse kernels receive the same already-validated coefficients and preserve their normative shifts and clipping. The scalar policy remains authoritative; dispatch cannot turn a rejected candidate into an accepted one.

Alternatives were a new public kernel crate or threading a function table through every search structure. Both introduce ownership and API surface without a current consumer. A small private capability and the existing checked pixel trait keep geometry and arithmetic in their existing owners.

## Verification and reference method

- Compare every new kernel against safe scalar over all extreme byte combinations, lengths including exact final-byte boundaries, random samples and unaligned subslices. Compare error results at malformed length boundaries too.
- Add a differential fuzz project for the new private kernel. Existing H.264 kernel fuzz targets continue covering reused interpolation, inverse arithmetic and deblocking. Complete encoder identity tests compare scalar-forced and dispatched streams and reconstructions, with forced prediction fixtures and admission failures unchanged.
- Run `cargo test --locked --release -p coreav-h264-encoder --features full` and again with `--features full,scalar`, encoder all-target Clippy, formatting, `scripts/unsafe_sites_check.py`, affected fuzz checks, and the existing three-decoder encoder conformance workflow. The current-main similarity guard checks the final public head.
- Retain an unchanged scalar baseline binary. For A, B and C, run five alternating QP28/GOP32 pairs over the same sixty prepared pictures, CPU 0, with no overlapping builds or tests. Record wall and CPU time, median and range, and byte identity of the stream and reconstruction. Include scalar-forced final runs to distinguish SIMD dispatch from other changes. These are local reference figures, not matched-quality or representative speed claims.

Clip C compression-efficiency diagnosis is separate and changes no policy or code in this work.
