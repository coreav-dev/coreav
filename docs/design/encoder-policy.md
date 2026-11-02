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
