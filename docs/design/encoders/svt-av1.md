# SVT-AV1 output validation

The [encoder contract](../encoders.md) defines the backend's scope and acceptance bars. This crate implements bounded AV1 low-overhead OBU framing and sequence-header configuration reads for 8-bit 4:2:0 SDR output. It does not link SVT-AV1, encode frames, or enable an encoder in the CLI.

## Framing is a separate boundary

An encoder packet is untrusted even when it comes from a linked library. The framing reader borrows the supplied bytes and checks their extents without copying payloads. Caller-provided limits bound the packet size and number of OBUs, including zero-length OBUs. AV1 size fields occupy at most eight bytes and cannot exceed the specification's 32-bit value range. Nonminimal size encodings remain valid.

The reader rejects forbidden or reserved header bits, reserved OBU types, missing size fields, truncated extensions or payloads, invalid size fields, and temporal delimiters with payloads. It also rejects extension headers on sequence headers, temporal delimiters, and tile lists, following the published HTML specification's layer-specific table. Reserved-type rejection is a strict backend-output subset: a general AV1 decoder must ignore reserved types. The reader exposes OBU kinds, layer identifiers, and borrowed payloads only through its checked views.

Framing validity does not establish that a sequence header or frame payload is valid. It also does not establish frame count, order, timestamps, color metadata, or agreement with submitted frames. Callers must not derive an MP4 sample description or mux packets from framing checks alone.

## Sequence configuration

The sequence reader takes a checked OBU and rejects any kind other than a sequence header. Caller limits bound payload bytes and maximum frame dimensions before the reader returns configuration. The reader consumes the sequence syntax, including optional timing, operating points, decoder-model fields, and trailing bits. Both normal and reduced still-picture headers are supported.

The accepted output scope is profile 0, 8-bit, non-monochrome 4:2:0 without film grain or PQ or HLG transfer characteristics. Valid AV1 formats outside that scope are unsupported rather than silently converted. Reserved fields, invalid clock values, malformed conditional syntax, and missing or nonzero trailing padding are errors. The configuration retains the signaled color description and range without changing them. Unspecified color metadata remains accepted, but does not prove that the source is SDR. The caller must establish the SDR source requirement separately.

Sequence maximum dimensions are upper bounds, not the dimensions of every decoded frame. Timing describes the sequence clock, not submitted-frame timestamps. The reader does not establish frame syntax, actual dimensions, frame count, order, decoder-level conformance, or agreement with the request. Those checks remain necessary before output can be accepted for muxing.

The four-byte `av1C` header comes from checked profile, level, tier, bit-depth, and chroma fields. It is not a complete MP4 sample description. Its optional initial presentation delay is absent: the sequence's initial display delay counts frames, while the container's initial presentation delay counts samples and cannot be derived without sample grouping.

## Standalone verification

Until workspace registration, the crate owns its manifest and lockfile. Its local lint configuration retains the workspace's production restrictions. Verification uses:

```sh
cargo fmt --manifest-path crates/coreav-backend-svt-av1/Cargo.toml --check
cargo clippy --manifest-path crates/coreav-backend-svt-av1/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/coreav-backend-svt-av1/Cargo.toml --locked
cargo deny --all-features --manifest-path crates/coreav-backend-svt-av1/fuzz/Cargo.toml check licenses
```

Construction checks the packet byte limit. Iteration checks each OBU and its count limit. Callers must consume the reader through its final `None` and propagate every error before accepting a packet. An error ends iteration permanently. An empty packet is an empty sequence, not a valid encoded frame.

The hostile-output fixtures check truncated input, invalid headers and lengths, and exact limit boundaries. The `fuzz/` project exposes the `obu` and `sequence` targets for affected-target CI. The framing target checks finite iteration, caller bounds, layer-ID ranges, and terminal errors. The sequence target parses arbitrary payloads behind a valid sequence-OBU header and checks configuration bounds. These are correctness checks, not encoder acceptance results. None of bars A1 through A6 has been measured by this unit.

## Native integration remains separate

The Rust crate has no FFI sites or native dependencies and needs no licence exception yet. A production wrapper PR must carry its actual FFI files and unsafe-site rows together. The integration owner supplies and checks those rows before opening and signs off on the PR afterward. The separate backend-scoped BSD-3-Clause-Clear exception remains a prerequisite for that production PR. SVT-AV1 distributions retain the upstream copyright, licence, and AOM Patent License 1.0 notices. These requirements do not change the licence of CoreAV's own glue code.

The worker protocol, sandboxed worker entry, raw-frame driver, conformance and quality drivers, and measurement-only in-process arm are not implemented by this unit. The in-process arm must never become a release feature or the default path.

## Static library build

`scripts/encoders/svt-av1/build.py` builds the pinned SVT-AV1 release into a new installation prefix. It does not enable the Rust backend, link a codec into CoreAV, or authorize distribution. Release packaging, notice integration, and the scoped licence check remain the integration owner's work.

Each target builds on its native OS and architecture. The script refuses a different host rather than silently cross-compile or substitute a glibc library.

| Target | Build host | C compiler | macOS deployment minimum |
|---|---|---|---|
| `x86_64-unknown-linux-musl` | Linux x86-64 | `musl-gcc` | Not applicable |
| `aarch64-unknown-linux-musl` | Linux AArch64 | `musl-gcc` | Not applicable |
| `x86_64-apple-darwin` | macOS x86-64 | `clang` | 10.12 |
| `aarch64-apple-darwin` | macOS AArch64 | `clang` | 11.0 |

The host needs Python 3.9 or later, CMake 3.16 or later, Make, and native C and C++ compilers. CMake configures both languages even though the encoder library is C. x86-64 builds also need NASM 2.14 or later. Linux needs `readelf`, `gcc-ar`, and `gcc-ranlib`. The recipe explicitly supplies GCC's LTO archive tools because CMake's compiler-name inference can look for nonexistent musl-prefixed tools. macOS verification needs `lipo` and `otool`.

```sh
python3 scripts/encoders/svt-av1/build.py \
  --target aarch64-apple-darwin --prefix /absolute/path/to/svt-av1 --jobs 2
python3 -m unittest discover -s scripts/encoders/svt-av1 -p 'test_*.py'
```

The prefix's parent must exist and the prefix must not exist. No other process may create or change that destination during the build. `--archive` accepts a local copy of the same pinned tarball. The script verifies the full SHA-256 before extraction in both download and local modes. `--cmake` selects the CMake executable. `--jobs` accepts 1 through 16 and defaults to 2.

The recipe uses a Release static build with applications and upstream tests disabled, CPU-native tuning disabled, LTO enabled, and position-independent code. It ignores inherited build flags rather than let a caller's shell silently change the recipe. Temporary source and build directories are removed on success or failure. Installation is staged, verified, and published only after its link-and-version probe passes. Existing installations are never reused as evidence of a fresh build.

Each build-tool command has a one-hour timeout. Timeout or cancellation terminates and reaps the command's process group before removing its workspace. This is a build-tool bound, not an encoder deadline guarantee.

The probe retains the encoder API symbols and calls only the public version query. It checks the exact release version and target architecture. Linux's probe has no ELF interpreter or shared dependencies; macOS permits only system libraries. The probe performs no encoding, so success proves neither encoder conformance nor the release acceptance bars. The installed manifest records build settings and hashes of the archive, probe, headers, and notices.

The installation retains `LICENSE.md`, `LICENSE-BSD2.md`, and `PATENTS.md`, plus the `fastfeat` and `safestringlib` licence files. These are copied from the verified archive, not reconstructed from licence identifiers. The files alone do not establish complete release-package notice coverage.

## Specification

The readers are written from the [approved AV1 specification](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), sections 4.10, 5.2, 5.3, 5.5, 6.2.2, and 6.4, and the published HTML specification's [OBU header semantics](https://aomediacodec.github.io/av1-spec/#obu-header-semantics). The configuration record follows the [AV1 ISO Media File Format binding](https://aomediacodec.github.io/av1-isobmff/), section 2.3. Other encoders' wrappers are not implementation references.

## Phase 1 implementation plan

This plan precedes the runtime implementation. It covers a safe native wrapper, sandboxed encoding, MP4 output, and explicit `coreav transcode --video av1` selection in one change. Existing H.264 output remains the default. Windows, AV1 output from the compatibility command, ten-bit input, HDR, and quality or speed acceptance runs are outside this implementation.

### Establish the native contracts before FFI

Only the pinned recipe's installed public headers and the upstream `v4.2.0` documentation under `Docs/` supply API contracts. AV1 syntax and MP4 mapping come from the specifications linked above. Native implementation sources and other projects' integrations are not references. Hosted native smoke artifacts retain installed headers and the installation manifest for reproducible citations. Those headers are not committed to this repository.

Input-picture reuse remains a blocking API question at the plan checkpoint. Before writing runtime FFI, record the exact installed-header or API-documentation lines that establish how long the picture descriptor, planes, and application-private data must live. A successful submission or a packet with the same presentation timestamp is not evidence of release unless the API contract says so. If the permitted material supplies no release event or synchronous-copy guarantee, stop implementation at that boundary rather than infer one from another wrapper or an experiment.

SVT's level of parallelism is not a thread count. [`Docs/Parameters.md`, lines 375–403 at `v4.2.0`](https://gitlab.com/AOMediaCodec/SVT-AV1/-/blob/v4.2.0/Docs/Parameters.md#L375-403) states that higher levels create more threads and retain more pictures. Lines 384–387 explicitly say that the level neither targets a thread count nor constrains logical processors. The `--pin` behavior in lines 400–403 restricts eligible cores, not the number of threads. The backend must not translate CoreAV's `--threads N` into parallelism level N or report that value as a thread limit. Initially, an explicit thread-count request is refused for AV1. The effective summary records the chosen parallelism level separately from a null thread limit. Native configuration fields and their ranges are checked against the installed headers before this setting is finalized.

### Wrapper ownership

The optional native feature links only the exact verified static recipe installation. No system-library, dynamic-library, or runtime-download fallback is added. The Rust glue retains its existing licence. The wrapper accepts checked, owned eight-bit planar 4:2:0 SDR pictures and validated settings, never a media file or a compressed source packet.

A session owns its native handle, configuration, every retained input allocation, and each output buffer until its documented release point. Its state distinguishes accepting input, draining, completed, and failed. Initialization failure cleans up partial ownership. Failure is terminal. No submission or output follows it. Finalization signals EOF once, drains through the encoder's end marker, releases every native output exactly once, and destroys the session before releasing any input that may still be borrowed. The session has no concurrent native calls and does not expose raw pointers.

Every actual Rust FFI site and lint exception receives its safety precondition in `docs/unsafe-sites.md` in the same commit as its code. The native wrapper, build integration, licence inputs, and inventory receive the shared integration owner's exact-diff review before commit. The existing global safety and licence checks remain enabled.

### Worker and packet boundaries

The release path always reexecutes the CLI as a private encoder worker under the existing sandbox launcher. The worker receives settings, sequential picture identities, and checked raw planes. One bounded writer queue and a concurrent bounded output reader prevent pipe deadlock. The existing process keeper owns the deadline, process group, stderr tail, cancellation, and reaping. A worker cannot publish files or receive the source container. Strict-safe keeps its existing platform memory-enforcement requirements.

The worker protocol has a versioned header, bounded records, explicit picture identity and packet extents, and an unambiguous completed-drain record. Neither worker-supplied flags nor native timestamps alone certify a valid output sample. The parent checks every record, consumes the stream to its final boundary, and rejects unknown, duplicate, missing, extra, truncated, or out-of-range data. Count, packet, total-byte, retained-frame, and metadata limits are charged before allocation.

The safe AV1 reader validates OBU framing and sequence configuration, then establishes complete samples, presentation identity, decode order, and random access under the selected prediction structure. It checks the emitted geometry, bit depth, chroma format, range, and colour description against the request. Unsupported frame structures are refused, not guessed from packet flags. A configuration change or incomplete drain fails the entire job. Codec-neutral submission and packet ledgers continue to own work and timing accounting. Encoded sample payloads are spooled rather than accumulated in memory.

### MP4 and command integration

The MP4 writer builds an `av01` sample entry and `av1C` from validated output. It follows the binding's temporal-unit/sample rules and sync-sample requirements. Configuration OBUs and sample payloads obey that binding rather than the external H.264 transport's Annex B conventions. Existing replacement timing, metadata handling, copied audio, temporary-output publication, and destination-preservation rules remain shared.

Both `--encoder auto` with `--video av1` and explicit `--encoder svt-av1` select the built-in backend when compiled for a supported Unix target. An explicit incompatible encoder, unsupported setting, or unavailable build fails before media reads where the failure is parameter-known. Header-known incompatibilities fail before decoding. Decoded-only incompatibilities fail before the first submission. No fallback follows an accepted frame. Preset 8 and CRF 32 are AV1 defaults only. Explicit numeric presets accept 0 through 13 and CRF accepts 0 through 63. No bit-depth, range, frame-rate, or geometry conversion is implicit.

New encoded-stream metadata uses `coreav.transcode/2`, with its schema, examples, serializers, and tests updated together. It reports the backend that actually ran, native version, software kind, sandbox isolation, and effective settings. Existing summaries keep their version 1 contract. The compatibility command's vocabulary does not change. Release-owner coordination precedes opening the implementation pull request.

### Verification sequence

1. Obtain installed-header evidence, resolve input ownership, and finalize the native configuration in this note before runtime FFI.
2. Test wrapper initialization, repeated submission, drain, cleanup, invalid settings, and allocation limits. Exercise the real native library on all four Unix release targets.
3. Test malformed worker records and AV1 output, count and identity mismatches, configuration changes, incomplete EOF, worker crashes, timeouts, slow sinks, and destination preservation. Add affected parser fuzz targets without a long fuzz campaign.
4. Produce synthetic raw-picture fixtures with distinct frames. Decode every encoded output with the pinned oracle FFmpeg and an independent AV1 decoder. Compare the two reconstructions, presented frame count and order, geometry, colour and timestamps. This checks correctness, not lossy equality with source pixels or encoded-byte identity.
5. Check the actual MP4 sample entry, `av1C`, sample extents, decode and presentation times, sync samples, duration, and copied-audio packets. Exercise MP4 and Matroska sources, empty and one-frame input, delayed output, and unsupported input properties.
6. Build and run native encode/decode smoke checks for Linux musl and macOS on x86-64 and AArch64, covering default and full release integration. Keep unsupported builds and strict-safe refusals tested.
7. Run formatting, clippy, targeted tests, schema/help checks, native notice and unsafe inventories, the public-text checks, and the similarity guard. Merge current main before opening the single pull request.

These checks establish implementation correctness only. They do not satisfy or claim the separate A1–A6 quality, speed, size, or held-out acceptance results in the encoder contract.
