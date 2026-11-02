# Apple encoder output readers

The Apple backend is under development. No framework encoder, CLI option, or performance result is provided by these readers. The [encoder contract](../encoders.md) defines the remaining work and its acceptance bars. Platform encoding is not a finding that an operator's use is patent-covered. Paid services may need their own patent licence.

## AAC configuration and packet boundaries

`coreav-backend-apple` currently has no dependencies and calls no Apple framework. Its standalone manifest and lockfile permit isolated checks until workspace integration.

`aac::Config` reads a two-byte ISO/IEC 14496-3 `AudioSpecificConfig`. It accepts AAC-LC with 1024-sample frames, an indexed sample rate, and mono or left-right stereo. The parsed rate and channel configuration must equal the requested PCM format. Explicit frequencies, program configurations, 960-sample frames, core-coder dependencies, and extensions are refused. The input is the extracted configuration, not the AudioToolbox magic cookie or an MPEG-4 descriptor. Extracting that configuration from framework output is not implemented yet.

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

Framework ownership, callback cancellation, H.264 output, independent-decode capability smoke, affected-target fuzz CI, and the reference-Mac driver remain separate implementation steps. No M1 through M5 or AAC acceptance result is claimed. The development machine is not the designated reference host.

## Sources

- ISO/IEC 14496-3:2009, subpart 1 `AudioSpecificConfig` and subpart 4 `GASpecificConfig`.
- Apple's public `CoreAudioBaseTypes.h`, `AudioStreamPacketDescription`, and `AudioConverter.h`, `AudioConverterPrimeInfo`.
- Apple's [AudioStreamPacketDescription](https://developer.apple.com/documentation/coreaudiotypes/audiostreampacketdescription) and [AudioConverterPrimeInfo](https://developer.apple.com/documentation/audiotoolbox/audioconverterprimeinfo) references.

There are no unsafe sites in this reader-only slice.
