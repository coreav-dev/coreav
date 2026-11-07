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

Prediction continues to use checked `Block` and `BlockMut` views, including the existing edge-replicated scratch window. Inverse arithmetic, intra prediction and deblocking retain the scalar implementations. The existing single-block inverse 4x4 dispatcher itself uses scalar arithmetic. A separate trial enabling existing intra and deblocking dispatch produced no clear benefit and was removed. The scalar policy remains authoritative; dispatch cannot turn a rejected candidate into an accepted one.

Alternatives were a new public kernel crate or threading a function table through every search structure. Both introduce ownership and API surface without a current consumer. A small private capability and the existing checked pixel trait keep geometry and arithmetic in their existing owners.

## Verification and reference method

### On-demand encoder interpolation

The encoder retains its checked, edge-replicated source windows rather than preparing whole half-sample pictures. A private AVX2 chroma entry accepts the existing `Block` and `BlockMut` views, checks the supported shapes before forming pointers, and falls back to `Scalar` for other shapes. It shares the private CPU capability with SAD. The `scalar` feature bypasses this entry entirely. Luma six-tap filtering and quarter-sample averaging retain the existing runtime pixel dispatcher: a replacement and a four-wide specialization did not improve encode-only reference throughput.

Chroma uses the four bilinear weights and rounds by 32 before shifting six bits. Loads and stores use only each active row's exact width, including two-wide chroma, without padded copies or picture-level scratch. A single bilinear loop is retained for every phase; separate integral and one-axis loops did not improve the reference comparison.

Differential tests and fuzzing compare every phase and admitted shape against the existing scalar kernels, including unaligned windows ending at the last borrowed byte. Complete scalar/dispatched identity and an alternating encode-only comparison against an explicit main commit decide whether this specialization is retained. Existing wider pixel dispatch is the baseline, not a scalar-only comparison.

The retained chroma implementation at `7c9d8765c7cdb27206fd801efe3279ed998f6c23` was compared with main `c2ed850b3c61a5fd4c08add6f96af551269f9312` on one pinned CPU of a GitHub-hosted Ubuntu 24.04 AMD EPYC 9V74 runner. Each of five existing inputs used sixty pictures, GOP32, enabled deblocking and QP22/27/32/37/40. Five alternating calls per arm and cell used separate build directories and explicitly bound commits. Timing included encoding and identical stream/reconstruction accumulation, but excluded input loading, output writes and independent decoding. All twenty-five cells retained complete stream and reconstruction digests; independent decoding matched every reconstruction.

The median of each input's five quantizer-cell speed ratios was 1.0046, 1.0071, 1.0011, 1.0039 and 1.0058 respectively. This is a small shared-runner reference gain, not a representative speed claim. The path is retained because its fixed-width loop removes generic row-access overhead without an allocation, picture cache or additional phase dispatch, while preserving the scalar twin and bounded differential coverage. New luma paths and additional chroma phase specializations were removed when their paired comparisons did not improve on the existing arithmetic.

- Compare every new kernel against safe scalar over all extreme byte combinations, lengths including exact final-byte boundaries, random samples and unaligned subslices. Compare error results at malformed length boundaries too.
- Add a differential fuzz project for the new private kernel. Existing H.264 kernel fuzz targets continue covering reused interpolation, inverse arithmetic and deblocking. Complete encoder identity tests compare scalar-forced and dispatched streams and reconstructions, with forced prediction fixtures and admission failures unchanged.
- Run `cargo test --locked --release -p coreav-h264-encoder --features full` and again with `--features full,scalar`, encoder all-target Clippy, formatting, `scripts/unsafe_sites_check.py`, affected fuzz checks, and the existing three-decoder encoder conformance workflow. The current-main similarity guard checks the final public head.
- Retain an unchanged scalar baseline binary. For A, B and C, run five alternating QP28/GOP32 pairs over the same sixty prepared pictures, CPU 0, with no overlapping builds or tests. Record wall and CPU time, median and range, and byte identity of the stream and reconstruction. Include scalar-forced final runs to distinguish SIMD dispatch from other changes. These are local reference figures, not matched-quality or representative speed claims.

Clip C compression-efficiency diagnosis is separate and changes no policy or code in this work.

## Implementation and reference results

The implementation is `5c189df8f71c619573ea35824e2a22fc2dc3ea43`. Only interpolation dispatch and row SAD were retained. The AVX2 SAD tier uses natural-width 128-bit unsigned-byte reductions for four-, eight-, and sixteen-sample rows; it does not widen a short row by reading padding. Existing NEON interpolation is reused where the pixel dispatcher supports it; no new NEON kernel is added or claimed locally tested.

Three alternating development pairs per input first compared interpolation alone with the baseline, then added SAD. Their observed medians were A 41.874 → 51.664 and C 35.546 → 42.236 pictures/s for interpolation, then A 43.265 → 51.331 and C 39.226 → 44.308 for SAD. These are separate noisy comparisons, not factors to multiply. The additional intra and deblocking trial was A 46.354 → 46.260 and C 44.065 → 44.511, so it was removed rather than treating the small difference as a win. [The raw record](h264-encoder-simd-reference.json) retains every development call.

The final reference uses sixty unchanged prepared pictures per input, QP28, GOP32 and enabled deblocking. Each cell below is the median of five calls, with minimum and maximum in parentheses. Before/SIMD are one alternating paired comparison. Scalar control/SIMD control are a separate alternating comparison of the final implementation, compiled with and without `scalar`; do not compare cells across those two series.

| Input | Before, pictures/s | SIMD, pictures/s | Scalar control, pictures/s | SIMD control, pictures/s |
|---|---:|---:|---:|---:|
| A | 43.131 (37.939–43.772) | 50.969 (47.905–53.636) | 42.351 (41.920–45.186) | 46.987 (42.520–49.986) |
| B | 39.889 (38.144–40.864) | 45.986 (43.674–50.371) | 38.910 (37.531–39.182) | 46.022 (45.274–49.450) |
| C | 25.676 (24.677–32.232) | 32.509 (31.617–38.801) | 31.624 (27.063–34.298) | 44.047 (37.645–45.390) |

The paired median changes against the baseline are +18.17%, +15.29%, and +26.62%. Shared-host variation is visible, particularly between C's two series, and these figures establish no representative or statistically qualified speed verdict. Native decoder verification and IO are included in every timed call. The first scalar control was built before the rejected intra/deblocking dispatch calls were removed; its times are excluded and replaced with a control built from the exact final source. The original baseline/SIMD pairs remain valid.

All sixty retained final timing calls preserve the prior main stream and reconstruction hashes. The separately repeated pinned FFmpeg 9.0.2 decode matches all sixty pictures on A, B and C. Their bitrate, PSNR and SSIM are consequently unchanged from the [efficiency reference](h264-encoder-efficiency-reference.json), not new quality estimates. Both comparison series, executable hashes, compiler identities from each executable's ELF notes, input hashes, and complete wall/CPU observations are in the raw record. The compiler is Rust 1.99.1 and the release profile retains debug information; no native target-CPU override is used.

Local verification passes 51 encoder tests in each build mode, 21 existing pixel-kernel equivalence tests, 31 Python harness tests, and 48 generated complete-stream identity cases containing 192 pictures. The private SAD differential target passes 10,000 coverage-guided AddressSanitizer inputs. Encoder Clippy in both modes, formatting, unsafe-site inventory and workflow checks pass. The existing three-decoder workflow now also runs the generated scalar/dispatched identity comparison; public CI and the final-head similarity result belong to the pull request evidence.
