# Apple encoder output readers

The Apple backend is under development. The standalone crate has checked AAC and H.264 metadata readers and a macOS AudioToolbox adapter. It is not integrated into the CLI and provides no acceptance-bar or performance result. The [encoder contract](../encoders.md) defines the remaining work and its acceptance bars. Platform encoding is not a finding that an operator's use is patent-covered. Paid services may need their own patent licence.

## AAC configuration and packet boundaries

`coreav-backend-apple` has no production Rust dependencies. Its standalone manifest and lockfile permit isolated checks until workspace integration. macOS fixture tests alone depend on CoreAV's independent AAC decoder. This does not add a decoder to the production library graph.

`aac::Config::parse` reads a two-byte ISO/IEC 14496-3 `AudioSpecificConfig`. It accepts AAC-LC with 1024-sample frames, an indexed sample rate, and mono or left-right stereo. The parsed rate and channel configuration must equal the requested PCM format. Explicit frequencies, program configurations, 960-sample frames, core-coder dependencies, and extensions are refused. `from_cookie` additionally reads bounded MPEG-4 ES, DecoderConfig, DecoderSpecificInfo, and SLConfig descriptors. It accepts the optional ASC suffix that explicitly signals absent SBR, normalizing that representation to the same two-byte configuration. Unknown descriptors, ES flags, unexplained suffixes, and present SBR are refused.

AudioToolbox's private cookie may clear the DecoderConfigDescriptor's reserved low bit: hosted output uses `0x14` where the standard descriptor uses `0x15`. Cookie parsing accepts both audio-stream forms, with the upstream flag still clear and all descriptor extents checked. It extracts only the validated ASC; the private descriptor wrapper is not copied into container configuration. The captured-cookie regression refuses every other stream-type/flag byte and any requested rate or channel mismatch.

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

VideoToolbox ownership and callback cancellation, the hosted H.264 independent-decode capability smoke, and the reference-host driver remain separate implementation steps. No M1 through M5 or AAC acceptance result is claimed. The development machine is not a measurement host, and no physical measurement host is provisioned or promised.

## H.264 configuration and access-unit metadata

The safe `h264` module adds no native calls or production dependencies. `Dimensions` represents nonempty even visible geometry. `Config::from_parameter_sets` checks one complete SPS and PPS against that geometry before constructing an owned avcC record, including the High-profile chroma/depth extension fields and four-byte NAL lengths. It retains the exact checked parameter-set NALs; in-band replacements must match them byte for byte. Configuration size and uncropped coded geometry have caller limits. Cropping uses progressive 4:2:0 crop units, not raw pixel offsets.

The initial subset is High profile, 8-bit 4:2:0 progressive output with POC type 0 or 2. The readers check bounded Exp-Golomb fields, parameter-set references, emulation-prevention bytes, complete parameter-set trailing bits, and the optional VUI/HRD syntax with its bounded CPB table. Custom scaling matrices, intra-profile constraints, interlacing, POC type 1, frame-number gaps, slice groups, redundant pictures and a nonzero VUI reordering allowance are explicitly unsupported. This is not a general H.264 parser or a decoder, and it does not certify every level constraint or compressed slice.

`AccessUnit::copy_from` validates complete four-byte length-prefixed NAL extents, NAL headers and caller byte/NAL-count limits before copying the access unit. It requires an I/P coded-slice header starting at macroblock zero. Additional slices must have increasing start addresses and matching frame number, parameter-set reference, IDR identity, reference/non-reference classification and POC fields. B/SP/SI slices and other NAL forms outside the subset are refused. Declared sync must agree with IDR NAL headers; a non-IDR I slice is not relabeled as a random-access point. Identical parameter sets before the slices, an initial access-unit delimiter, bounded SEI payload extents and filler are supported.

The owned bytes preserve their original order and framing. Slice payload syntax, macroblock coverage, decoded pixels and reference availability are not validated by these metadata checks. Source-frame identity, timestamp order, durations and final submitted-frame accounting belong to the later session owner. A metadata-reader success does not establish successful decoding or complete encoder drain.

Configuration parsing uses bounded temporary RBSP buffers and retains parameter-set bytes plus avcC; peak storage is a fixed multiple of the caller's configuration bound. Access-unit parsing holds at most one bounded temporary RBSP plus its final owned byte copy. Processing is linear in the bounded input sizes, with no recursive parsing or untrusted-count allocation. Synthetic fixtures build header metadata only, not decodable video. The `h264_output` fuzz target checks hostile parameter sets and access units on hosted CI; no local hostile-fixture or fuzz execution is required.

## AudioToolbox ownership and streaming

`audiotoolbox::Encoder` uses the direct `AudioCodec` push/pull API instead of `AudioConverter` input callbacks. The component copies accepted input into its own buffer during `append`. No Rust frame reference or callback state survives a call, and there is no callback queue. One private native module owns one successful non-null component instance and disposes it on every later failure or drop. Failure outputs from component creation confer no ownership. The owner is neither Send nor Sync.

Settings select the Apple software AAC-LC component, CBR, unchanged indexed rate, mono or left-right stereo, and quality 0 through 127. Format properties are set before codec controls. A control is written only when the property API reports it writable; the initialized value must still equal the required value. The codec owns its priming method and delay mode. Neither is an input to the adapter, and their optional properties are not written. Final trim comes from checked codec metadata and must pass the independent decode fixture. The actual instance is resolved back to its component. Identity records its type, subtype, manufacturer, and documented component version value. The selected Apple software manufacturer identifies a software codec, not a requested hardware preference. No framework semantic version or OS build is invented.

The native fixtures request quality 96, the SDK's `kAudioCodecQuality_High`. In the [hosted diagnostic at `46b2013a`](https://github.com/coreav-dev/coreav/actions/runs/41403823335/job/125141266719), AudioToolbox reported the quality property writable but changed requested 127 to 96 immediately, before initialization. Such a request is refused with `Error::Setting`, not silently accepted as an equivalent quality. The settings constructor checks the numeric range; it does not promise native availability of every value. The adapter has no implicit quality default.

`append` accepts finite interleaved float PCM and a contiguous sample-frame identity. It reports partial consumption explicitly. Non-frame-aligned consumption is refused. Empty chunks do not signal EOF, and timestamp gaps are refused rather than filled with silence or closed. `start_drain` freezes a bounded tail plan from the direct codec's priming counts and the accepted source count. Each `pull` appends at most 1024 stack-backed zero frames toward flushing the reported delay and completing the last AAC frame, without counting that silence as source PCM. Once that tail is consumed, the API's zero-byte EOF marker is supplied once. Input exhaustion after that marker ends the native drain; input exhaustion before it is not EOF. `pull` returns at most one owned, checked AAC packet and a progress state. Failure, unknown output status, early EOF, a stalled output claim, excess output, and incomplete drain cannot become successful completion. A failed session accepts no further native work.

The caller supplies per-chunk, total sample, packet, output-byte, cookie, and trim limits. Output storage is fallibly allocated to a bounded size, and returned counts are checked against initialized buffers before the safe reader sees them. Cancellation and the session's single deadline are checked before and after synchronous framework calls. Neither can interrupt such a call or enforce a cap on the framework's own internal allocations. This adapter does not make strict-safe mode eligible.

Synthetic tail work has a separate bound, not a source or caller-chunk allowance: at most the leading and trailing trim limits combined, fed in chunks of at most 1024 frames through an initialized 2048-float stack buffer. Packet alignment must fit the trailing limit. Tail frames never increase the submitted-source count or permit more source input; all resulting packets still consume the cumulative packet and byte limits and the same deadline. A zero-consumption append does not advance the tail or authorize EOF.

Finalization reads the final cookie and codec-specific priming and padding values, checks their arithmetic against the actual accepted PCM and emitted packet totals, and returns validated container configuration only after drain. The final priming values must equal those that planned the tail; the codec must report no additional padded zeros after explicitly aligned input. Leading trim is the codec's reported delay, and trailing trim is the planned tail minus that delay. These are `AudioCodec` properties, not `AudioConverterPrimeInfo` input lookahead. The native fixtures independently decode synthetic AAC with the Rust decoder and check declared trim, channel impulse placement, and all four acceptance formats. They are correctness fixtures, not quality or speed bars against the pinned FFmpeg oracle.

## Sources

- ITU-T H.264 (08/2024), clauses 7.3.1, 7.3.2, 7.3.3, 7.4 and Annex E: NAL/RBSP syntax, parameter sets, picture-header identity and VUI/HRD.
- ISO/IEC 14496-15:2004/Amd.1:2006, clause 5.2.4.1: AVC decoder configuration and High-profile extension fields.

- ISO/IEC 14496-3:2009, subpart 1 `AudioSpecificConfig` and subpart 4 `GASpecificConfig`.
- Apple's public `CoreAudioBaseTypes.h`, `AudioStreamPacketDescription`, and `AudioConverter.h`, `AudioConverterPrimeInfo`.
- Apple's [AudioStreamPacketDescription](https://developer.apple.com/documentation/coreaudiotypes/audiostreampacketdescription) and [AudioConverterPrimeInfo](https://developer.apple.com/documentation/audiotoolbox/audioconverterprimeinfo) references.

The only native unsafe file is `crates/coreav-backend-apple/src/audiotoolbox/sys.rs`, with its crate lint change registered alongside it in the global unsafe-site inventory. Every other source file retains the crate's `deny` level without an opt-out.
