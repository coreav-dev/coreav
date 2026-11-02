# SVT-AV1 output validation

The [encoder contract](../encoders.md) defines the backend's scope and acceptance bars. This crate currently implements only bounded AV1 low-overhead OBU framing. It does not link SVT-AV1, encode frames, or enable an encoder in the CLI.

## Framing is a separate boundary

An encoder packet is untrusted even when it comes from a linked library. The framing reader borrows the supplied bytes and checks their extents without copying payloads. Caller-provided limits bound the packet size and number of OBUs, including zero-length OBUs. AV1 size fields occupy at most eight bytes and cannot exceed the specification's 32-bit value range. Nonminimal size encodings remain valid.

The reader rejects forbidden or reserved header bits, reserved OBU types, missing size fields, truncated extensions or payloads, invalid size fields, and temporal delimiters with payloads. It also rejects extension headers on sequence headers, temporal delimiters, and tile lists, following the published HTML specification's layer-specific table. Reserved-type rejection is a strict backend-output subset: a general AV1 decoder must ignore reserved types. The reader exposes OBU kinds, layer identifiers, and borrowed payloads only through its checked views.

Framing validity does not establish that a sequence header or frame payload is valid. It also does not establish frame count, order, timestamps, color metadata, or agreement with submitted frames. Callers must not derive an MP4 sample description or mux packets from framing checks alone. The sequence-configuration reader and submitted-frame accounting remain separate work under the encoder contract.

## Standalone verification

Until workspace registration, the crate owns its manifest and lockfile. Its local lint configuration retains the workspace's production restrictions. Verification uses:

```sh
cargo fmt --manifest-path crates/coreav-backend-svt-av1/Cargo.toml --check
cargo clippy --manifest-path crates/coreav-backend-svt-av1/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/coreav-backend-svt-av1/Cargo.toml --locked
```

Construction checks the packet byte limit. Iteration checks each OBU and its count limit. Callers must consume the reader through its final `None` and propagate every error before accepting a packet. An error ends iteration permanently. An empty packet is an empty sequence, not a valid encoded frame.

The hostile-output fixtures check truncated input, invalid headers and lengths, and exact limit boundaries. The `fuzz/` project exposes the `obu` target for affected-target CI. It checks finite iteration, caller bounds, layer-ID ranges, and terminal errors. These are correctness checks, not encoder acceptance results. None of bars A1 through A6 has been measured by this unit.

## Native integration remains separate

This unit has no FFI sites or native dependencies and needs no licence exception. Native integration must register the actual FFI files and their ownership preconditions before the wrapper's production PR opens. SVT-AV1 4.2.0 requires the backend-scoped BSD-3-Clause-Clear exception and retention of the upstream copyright, licence, and AOM Patent License 1.0 notices. These requirements do not change the licence of CoreAV's own glue code.

The worker protocol, sandboxed worker entry, static build recipe, raw-frame driver, conformance and quality drivers, and measurement-only in-process arm are not implemented by this unit. The in-process arm must never become a release feature or the default path.

## Specification

The reader is written from the [approved AV1 specification](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), sections 4.10.5, 5.2, 5.3, and 6.2.2, and the published HTML specification's [OBU header semantics](https://aomediacodec.github.io/av1-spec/#obu-header-semantics). Other encoders' wrappers are not implementation references.
