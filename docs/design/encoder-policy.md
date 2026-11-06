# 0043. Platform encoders and the SVT-AV1 licence exception

- Status: Accepted upon merge.
- Date: 2026-11-02
- Kind: Policy record.
- Amends: Policy records 0001, 0041, and 0042 only as stated below.

## Context

Record 0042 permits SVT-AV1 through a sandboxed encoder backend, but does not cover platform encoders or the BSD-3-Clause-Clear licence. Platform encoding also needs an explicit distinction between calling an operating-system codec and shipping a codec implementation.

## Decision

1. VideoToolbox, AudioToolbox, and Media Foundation join the encoder backends eligible for FFI under record 0042. Each backend has a safe public API and confines FFI to named modules. Every CoreAV-authored `unsafe` site, including callbacks, ownership transfers, and framework object lifetimes, is listed with its preconditions in `docs/unsafe-sites.md` in the change that adds it. This does not grant an exception to other crates.
2. These three operating-system encoders run in-process. This is a narrow addition to record 0042 point 4, alongside its NVENC exception, not permission to run other software encoder libraries in-process by default. The backend receives only checked raw frames, PCM samples, and parameters. It never receives an input file, container, or compressed source packet.
3. Record 0042 point 3 applies unchanged. Encoded bytes, format descriptions, lengths, packet counts, codec configuration, and timestamps are untrusted. Safe Rust validates them against the supplied buffers, submitted work, and configured limits before muxing. Their readers have fuzz targets. The platform exception does not waive resource limits or make in-process calls interruptible. Strict-safe mode refuses these encoders until it can enforce its existing guarantees. It does not silently substitute an external encoder.
4. Platform H.264 and AAC encoder adapters are included in the default and full native builds on their supported operating systems. They call the installed OS implementation and do not bundle an H.264 or AAC codec library. This exception does not include CoreAV's H.264 or AAC decoders in the default build, authorize another patented codec dependency, or authorize Windows transcoding before its sandbox and decode prerequisites. The default-build dependency check must retain those distinctions.
5. Documentation that offers platform H.264 or AAC encoding states that paid services may need their own patent licence. The OS API's availability is not a finding that an operator's use is covered. No blanket royalty-free claim follows from this record.
6. BSD-3-Clause-Clear is allowed only for the SVT-AV1 encoder dependency and its dedicated backend, not in the workspace-wide licence allowlist. The implementation adds a dependency-scoped licence exception and retains the upstream copyright, licence, and AOM Patent License 1.0 notices in distributions. BSD-3-Clause-Clear itself grants no patent rights. The separate AOM patent grant has its own scope and conditions.
7. SVT-AV1 remains sandboxed by default, like libopus. An in-process measurement arm is explicitly selected, diagnostic only, and never the default or a release feature. It does not weaken sandbox or strict-safe policy.
8. Record 0042 point 5 remains unchanged. No GPL build, linked or bundled x264, bundled FFmpeg, or CoreAV-initiated OpenH264 download is added. The libvpx VP9 encoder remains deferred and gains no new FFI permission here. H.264 with copied audio in MP4 remains the default output. AV1 is an explicit output choice.

## Consequences

The SVT-AV1 and macOS backend work can proceed independently under the [encoder design](encoders.md). Linking, CLI integration, release checks, notices, and unsafe-site registration remain implementation work. This documentation change enables no encoder at runtime.

Platform crashes and unbounded framework allocations can affect the caller because the OS encoders share its process. Bounded input, bounded callback queues, checked output, and deadline checks reduce risk but do not provide process isolation. A helper process remains the alternative if these limits prove unacceptable.

## Sources

- [SVT-AV1 4.2.0 licence](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.2.0/LICENSE.md) and [patent grant](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.2.0/PATENTS.md).
- [Apple VideoToolbox](https://developer.apple.com/documentation/videotoolbox) and [Microsoft Media Foundation H.264 encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder).

## 0044. SIMD kernels for the opt-in H.264 encoder

- Status: Accepted upon merge.
- Date: 2026-11-06
- Kind: Policy record.
- Amends: Policy records 0001, 0038, and 0041 only as stated below.

### Context

The self-written encoder remains opt-in. Its bounded search still spends substantial time on pixel arithmetic. The maintainer chose SIMD acceleration without changing prediction decisions, encoded bytes, reconstruction, or admission.

### Decision

1. Only the H.264 encoder's arithmetic kernels gain this exception. New intrinsics live in its private `simd` module, whose safe interface accepts bounded slices or fixed arrays. The crate keeps the workspace lint levels with `unsafe_code = "deny"`; only that module permits unsafe code. All other encoder modules retain `forbid`. Parsing, syntax, mode selection, resource accounting, and allocation remain safe Rust. This grants no exception to another crate or codec.
2. A private CPU-capability value proves AVX2 support before any target-feature call. Detection happens once; unsupported CPUs use the safe scalar implementation. Existing checked H.264 pixel kernels may be reused through their runtime dispatcher, without its AVX-512 tier. The encoder's `scalar` feature forces the scalar implementation, including these reused kernels. No process-wide target-CPU compiler flag is required.
3. Each kernel preserves the scalar implementation's complete domain, arithmetic widths, rounding, clipping, errors, and tie behavior. Tests compare random and extreme inputs with the scalar twin. A differential fuzz target exercises each new kernel through its safe interface. Complete encoded NALs and reconstructions must be byte-identical to the scalar encoder; independent decoder conformance remains required.
4. Each new or changed unsafe file and each lint exception has a `docs/unsafe-sites.md` row naming its operations and safety preconditions. Safe code establishes complete memory extents before pointers are formed. No vector load may read a padding byte outside the borrowed region. The inventory check, differential tests, fuzzing, and existing similarity guard cover the implementation in the same change.
5. Kernel selection follows profiles and measured reference comparisons. A scalar twin remains available, and a vector path without a useful measured benefit is removed. This adds no judged speed threshold or permission to change quality. The feature graph, patent boundary, licences, source isolation, and no-copying rules are unchanged; no patented encoder enters the default build.

### Consequences

Opt-in encoder builds can contain confined unsafe SIMD arithmetic. Their safe public API and byte output stay unchanged. The [SIMD design](h264-encoder-simd.md) names the selected kernels, dispatch boundary, numerical invariants, and verification commands. A scalar-forced build retains the previous arithmetic path. The existing decoder hot-path exception is not expanded.
