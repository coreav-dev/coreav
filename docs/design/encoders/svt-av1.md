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

This unit has no FFI sites or native dependencies and needs no licence exception. Native integration must register the actual FFI files and their ownership preconditions before the wrapper's production PR opens. SVT-AV1 4.2.0 requires the backend-scoped BSD-3-Clause-Clear exception and retention of the upstream copyright, licence, and AOM Patent License 1.0 notices. These requirements do not change the licence of CoreAV's own glue code.

The worker protocol, sandboxed worker entry, static build recipe, raw-frame driver, conformance and quality drivers, and measurement-only in-process arm are not implemented by this unit. The in-process arm must never become a release feature or the default path.

## Specification

The readers are written from the [approved AV1 specification](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), sections 4.10, 5.2, 5.3, 5.5, 6.2.2, and 6.4, and the published HTML specification's [OBU header semantics](https://aomediacodec.github.io/av1-spec/#obu-header-semantics). The configuration record follows the [AV1 ISO Media File Format binding](https://aomediacodec.github.io/av1-isobmff/), section 2.3. Other encoders' wrappers are not implementation references.
