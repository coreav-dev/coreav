# 0042. Encoder backends, and GPL encoders as separate programs

- Status: Accepted
- Date: 2026-10-29
- Kind: policy record, with the narrowly scoped operator-helper amendment below.
- Amends: ADR 0001 (points 2, 4 and 5).

## Context

ADR 0001 grants the FFI exception to decoder-backend crates (NVDEC, dav1d, FFmpeg) only. The transcoding order (proposal 8.8) needs encoders: libopus for audio, SVT-AV1 and `rav1e` for AV1, NVENC for H.264 and HEVC on GPU hosts, and x264 for H.264 on CPU hosts. The first four are permissively licensed libraries or drivers that a backend crate would call. x264 is GPL, which CoreAV's license rule excludes from every CoreAV build and artifact. The operator-only helper below is a separate executable, not a CoreAV build feature.

## Decision

1. **Eligibility.** Encoder-backend crates join the decoder-backend group of ADR 0001 point 2: `coreav-backend-*` crates may call libopus, SVT-AV1, `rav1e` (through its C API) or NVENC through FFI. An encoder used as a Rust dependency without CoreAV-authored `unsafe` (for example `rav1e` as a crate) needs no exception; its own `unsafe` is a dependency's (ADR 0001 point 7). Apart from the operator-only libx264 helper in point 8, any other encoder library needs a new record. That helper gains no eligibility as a CoreAV backend dependency.
2. **Same scoping as decoders.** Each encoder backend is a crate of its own with the lint-copy mechanism of ADR 0001 point 3. Its `unsafe` lives only in the named modules, its public API is safe, and each site is listed in `docs/unsafe-sites.md` with its precondition, in the PR that adds it (ADR 0041). Its design note names the FFI surface and the safe API around it. Only builds with the backend's feature link the library.
3. **Inputs and outputs.** An encoder receives only frames, samples and parameters that safe Rust built and checked: never the input file, its container or its bitstream. Its output (the encoded bitstream, packet sizes, timestamps) is untrusted. Safe Rust reads it back, checks every size, offset and count against the buffers and the configured limits, and that reader has a fuzz target, as the worker's frame descriptors do (ADR 0001 point 4).
4. **Sandbox by default.** Software encoder libraries (libopus, SVT-AV1, `rav1e` through FFI) run in sandboxed backend processes by default, as dav1d and FFmpeg do. NVENC runs in-process, as NVDEC does. An in-process mode for a software encoder is opt-in, and its design note states what it gives up.
5. **GPL encoders are never linked or bundled.** x264, `ffmpeg -c:v libx264`, and any other GPL or LGPL encoder are never linked, never vendored, never shipped in a CoreAV artifact (crates, wheels, npm packages, container images, release archives) and never downloaded by CoreAV. They run only as a separately installed program that the operator provides and names (a path, an environment variable, or `PATH`), as a separate process:
    - under the worker's resource limits (address space, CPU time, no file writes, no core files), in its own process group, killed at the deadline or on an error;
    - fed raw frames on a pipe and a command line CoreAV builds, with its output read back as in point 3.

   Launching a program uses `std` process control and safe system-call wrappers, so it needs no FFI exception. CI and measurements may build a pinned copy of such a program outside the repository as a test-only black box, never committed and never shipped. Reading its source stays forbidden (clean room).
6. **Tests.** Tests of an encoder backend pin the encoder version and judge its output by decoder conformance and a quality bar (PSNR or VMAF) committed before the run, never by a hash of the encoded output (proposal 8.8). Tests that need an operator-provided program skip cleanly when it is not installed; CI installs it.
7. **The records and text it amends.** ADR 0001 points 2, 4 and 5 read "decoder and encoder backends" where they say "decoder backends". `CLAUDE.md`, proposal section 4 (principle 1), the section 5.1 crate table and section 8.8 ("Encoders are a policy prerequisite", "GPL encoders are never bundled") point to this record, in the same change.
8. **Operator-only shared-memory helper.** An operator may explicitly select a separately built libx264 executable using CoreAV's versioned shared-memory protocol instead of the pipe-fed program. Its original source is under the repository's MIT OR Apache-2.0 licence, but a helper linked with GPL libx264 is a separately licensed GPL executable. The exception permits that helper's public-API glue only. It does not permit linking libx264 into any CoreAV binary or library, changing a default or release feature set, or adding a GPL dependency or licence exception to the workspace, `cargo deny`, default-link checks or `deny.yml`.

   The helper stays outside the main workspace and dependency graph. CoreAV never downloads, builds, publishes or bundles it. Release and packaging workflows must never build it, and no release archive, wheel, npm package, container image or other CoreAV artifact contains it. Only the operator installs it. A correctness job may build it outside the artifact tree solely as a disposable test fixture, like the pinned encoder programs in point 5; it is never uploaded as a product. Its README states the built executable's GPL status.

   Pipe-fed external programs remain the default. The helper runs only as a separate, resource-limited process under the same deadline and output-validation boundary. It receives checked parameters and immutable frame snapshots, never compressed input, destination-file access or writable decoder memory. Each snapshot is a fresh file sealed against writes, growth and shrinking before publication. The decoder has no mapping of it. Acknowledgement may release the parent's handle, but never makes that file writable or reuses it for another frame. A retained or forwarded reference therefore remains immutable after decoder acknowledgement. Regression tests cover this property and complete-file identity with the pipe path.

## Consequences

- The first encoder crate (libopus is planned first) can start once this record is merged. Its PR adds its sites to `docs/unsafe-sites.md`, and its license to `deny.toml` if that license is not already allowed.
- H.264 encoding on CPU hosts depends on an operator who installs x264 or an FFmpeg built with libx264. CoreAV's own builds stay free of GPL code.
- Each sandboxed encoder adds a process boundary and a frame copy. A speed bar above 1.00 for it carries the written reason of Decision 34.

## Alternatives considered

- **One record per encoder.** Rejected: the exception is the same kind as the decoder backends', and the site list (ADR 0041) already records each crate's surface.
- **Linking libx264 into CoreAV, or shipping x264 in an optional package.** Rejected: GPL, against the permanent license rule. The operator-only executable in point 8 is excluded from every CoreAV artifact.
- **Software encoders in-process by default.** Rejected: their input frames come from untrusted files, and a crash or a runaway allocation would take the host process with it.

## Open questions

1. NVENC: the license terms of the Video Codec SDK headers and the driver API need a check before the NVENC crate (a maintainer item).
2. Patents per output codec (H.264 and HEVC through NVENC): whether encoder-only patents change the section 12.2 review, which today covers decoding only.
