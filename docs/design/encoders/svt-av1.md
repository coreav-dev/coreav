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
