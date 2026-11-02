# Apple encoder output readers

The Apple backend is under development. The standalone crate has checked AAC metadata readers and a macOS AudioToolbox adapter. It is not integrated into the CLI and provides no acceptance-bar or performance result. The [encoder contract](../encoders.md) defines the remaining work and its acceptance bars. Platform encoding is not a finding that an operator's use is patent-covered. Paid services may need their own patent licence.

## AAC configuration and packet boundaries

`coreav-backend-apple` has no production Rust dependencies. Its standalone manifest and lockfile permit isolated checks until workspace integration. macOS fixture tests alone depend on CoreAV's independent AAC decoder. This does not add a decoder to the production library graph.

`aac::Config::parse` reads a two-byte ISO/IEC 14496-3 `AudioSpecificConfig`. It accepts AAC-LC with 1024-sample frames, an indexed sample rate, and mono or left-right stereo. The parsed rate and channel configuration must equal the requested PCM format. Explicit frequencies, program configurations, 960-sample frames, core-coder dependencies, and extensions are refused. `from_cookie` additionally reads bounded MPEG-4 ES, DecoderConfig, DecoderSpecificInfo, and SLConfig descriptors. It accepts the optional ASC suffix that explicitly signals absent SBR, normalizing that representation to the same two-byte configuration. Unknown descriptors, ES flags, unexplained suffixes, and present SBR are refused.

`aac::PacketBatch` checks copied `AudioStreamPacketDescription` values against the supplied buffer and caller-supplied batch limits before allocating packet storage. Signed offsets must be nonnegative. Packets must be nonempty, in buffer order, nonoverlapping, and wholly within the buffer. Apple permits gaps between packets. The reader copies only the packet bytes into owned storage. Both zero, meaning the format's fixed frame count, and an explicit count of 1024 are accepted. An empty batch must have an empty buffer and does not by itself prove completion.

The packet reader validates extents and declared frame counts, not compressed `raw_data_block()` syntax or successful decoding. Per-batch limits do not replace cumulative output, queue, or deadline limits. Those belong to the future session owner. No decoder is linked by this crate.

`aac::Trim` checks that the cumulative checked packet count, minus declared leading and trailing trim, accounts exactly for the submitted PCM sample frames. Sample frames count one sample in every channel, not interleaved scalar samples. Both trim counts are bounded by the caller. The counts describe the encoded stream, not input lookahead requested by `AudioConverterPrimeInfo`. Arithmetic agreement does not prove impulse placement, channel order, or the truth of the declared trim. Independent decoding must check those properties.

## Verification

The standalone fixture tests cover the four AAC acceptance formats, every two-byte configuration for a requested 48 kHz stereo format, packet ownership, permitted gaps, malformed extents, exact batch bounds, and drain underflow, overflow, missing packets, and excess packets. The packet fixtures contain synthetic bytes and test packetization only.

The crate-local `aac_output` fuzz target exercises configuration, packet descriptions, and trim accounting. Fuzz runs belong on hosted CI, not the development machine.

The hosted macOS runner runs these commands, with one Rust build at a time:

```sh
cargo test --locked --manifest-path crates/coreav-backend-apple/Cargo.toml
cargo clippy --locked --manifest-path crates/coreav-backend-apple/Cargo.toml --all-targets -- -D warnings
```

VideoToolbox ownership and callback cancellation, H.264 output, the hosted H.264 independent-decode capability smoke, and the reference-host driver remain separate implementation steps. No M1 through M5 or AAC acceptance result is claimed. The development machine is not a measurement host, and no physical measurement host is provisioned or promised.

## AudioToolbox ownership and streaming

`audiotoolbox::Encoder` uses the direct `AudioCodec` push/pull API instead of `AudioConverter` input callbacks. The component copies accepted input into its own buffer during `append`. No Rust frame reference or callback state survives a call, and there is no callback queue. One private native module owns one successful non-null component instance and disposes it on every later failure or drop. Failure outputs from component creation confer no ownership. The owner is neither Send nor Sync.

Settings select the Apple software AAC-LC component, CBR, unchanged indexed rate, mono or left-right stereo, and quality 0 through 127. Format properties are set before codec controls. A control is written only when the property API reports it writable; the initialized value must still equal the required value. The codec owns its priming method and delay mode. Neither is an input to the adapter, and their optional properties are not written. Final trim comes from checked codec metadata and must pass the independent decode fixture. The actual instance is resolved back to its component. Identity records its type, subtype, manufacturer, and documented component version value. The selected Apple software manufacturer identifies a software codec, not a requested hardware preference. No framework semantic version or OS build is invented.

The native fixtures request quality 96, the SDK's `kAudioCodecQuality_High`. In the [hosted diagnostic at `46b2013a`](https://github.com/coreav-dev/coreav/actions/runs/41403823335/job/125141266719), AudioToolbox reported the quality property writable but changed requested 127 to 96 immediately, before initialization. Such a request is refused with `Error::Setting`, not silently accepted as an equivalent quality. The settings constructor checks the numeric range; it does not promise native availability of every value. The adapter has no implicit quality default.

`append` accepts finite interleaved float PCM and a contiguous sample-frame identity. It reports partial consumption explicitly. Non-frame-aligned consumption is refused. Empty chunks do not signal EOF, and timestamp gaps are refused rather than filled with silence or closed. `start_drain` supplies the API's zero-byte EOF marker once. `pull` returns at most one owned, checked AAC packet and a progress state. Failure, unknown output status, early EOF, a stalled output claim, excess output, and incomplete drain cannot become successful completion. A failed session accepts no further native work.

The caller supplies per-chunk, total sample, packet, output-byte, cookie, and trim limits. Output storage is fallibly allocated to a bounded size, and returned counts are checked against initialized buffers before the safe reader sees them. Cancellation and the session's single deadline are checked before and after synchronous framework calls. Neither can interrupt such a call or enforce a cap on the framework's own internal allocations. This adapter does not make strict-safe mode eligible.

Finalization reads the final cookie and codec-specific priming and padding values, checks their arithmetic against the actual accepted PCM and emitted packet totals, and returns validated container configuration only after drain. These are `AudioCodec` properties, not `AudioConverterPrimeInfo` input lookahead. The native fixtures independently decode synthetic AAC with the Rust decoder and check declared trim, channel impulse placement, and all four acceptance formats. They are correctness fixtures, not quality or speed bars against the pinned FFmpeg oracle.

## Sources

- ISO/IEC 14496-3:2009, subpart 1 `AudioSpecificConfig` and subpart 4 `GASpecificConfig`.
- Apple's public `CoreAudioBaseTypes.h`, `AudioStreamPacketDescription`, and `AudioConverter.h`, `AudioConverterPrimeInfo`.
- Apple's [AudioStreamPacketDescription](https://developer.apple.com/documentation/coreaudiotypes/audiostreampacketdescription) and [AudioConverterPrimeInfo](https://developer.apple.com/documentation/audiotoolbox/audioconverterprimeinfo) references.

The only native unsafe file is `crates/coreav-backend-apple/src/audiotoolbox/sys.rs`, with its crate lint change registered alongside it in the global unsafe-site inventory. Every other source file retains the crate's `deny` level without an opt-out.
