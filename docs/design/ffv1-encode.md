# Native FFV1 encoding into Matroska

Status: Accepted.

This design adds lossless video encoding without an external encoder. The first implementation preserves 8-bit planar YCbCr samples and writes FFV1 version 3 into Matroska. It makes no speed or compression-ratio claim.

The only FFV1 format sources are [RFC 9043](https://www.rfc-editor.org/rfc/rfc9043) and the [Matroska FFV1 codec mapping](https://www.matroska.org/technical/codec_specs.html#v_ffv1). Other implementations are not design inputs. The pinned FFmpeg and ffprobe binaries are black-box correctness oracles.

## Scope and refusals

The library accepts `PixelFormat::Yuv` with depth 8 and `Chroma::Cs420`, `Cs422`, or `Cs444`. Plane samples remain unchanged. No scaling, range adjustment, matrix conversion, or bit-depth conversion occurs. The encoder reads validated `Picture` rows, including pictures whose strides contain padding.

Only FFV1 version 3, micro-version 4, is emitted. Every frame is a keyframe. The range coder uses `coder_type = 1`, the default transition table, one quantization-table set, default initial states, `ec = 1`, and `intra = 1`. This gives independent frames and slice checksums without a second entropy coder or temporal context lifetime.

The first CLI command is:

```sh
coreav ffmpeg -i input.mp4 -map 0:v:0 -an -sn -dn -c:v ffv1 output.mkv
```

It requires a build with the existing `transcode` feature and a decoder backend that accepts the source. Source decoding is limited to H.264 through the native backend, AV1 through dav1d, and VP8 or VP9 through libvpx. The current frame model carries no per-frame scan flag, so FFmpeg fallback and hardware decoders are not admitted by this path. The caller preflights source headers for progressive scan. Encoding itself starts no external process. Strict-safe mode still applies independently to source decoding. The accepted input containers are the existing local MP4/MOV and Matroska/WebM video sources. The output contains exactly one selected video stream. An input without audio needs no `-an`; selected audio, subtitles, data, attachments, or additional video tracks require explicit exclusion rather than silent loss.

The native command requires a `.mkv` destination and the Matroska muxer. Explicit `-f matroska` may accompany that extension. MP4, AVI, WebM, `.mka`, arbitrary extensions, standard output, image output, and raw output are refused for this encoder. There is no implicit FFV1 default, new `coreav transcode` option, or FFV1 version option.

RGB, gray, alpha, semiplanar formats, hardware-only pixels, samples above 8 bits, interlaced or unknown scan structure, frame-size or pixel-format changes, and unsupported colour declarations are refused. A requested `-pix_fmt` must equal the decoded format. Filters, resizing, seeking, trimming, concatenation, frame duplication, output frame-rate changes, and encoder-specific rate, quality, preset, or tuning options are refused. The existing stream mapping, overwrite protection, metadata refusals, and effective last-matching codec-option rules remain authoritative.

Admission failures before encoding are native-support refusals under the existing compatibility command contract. Strict-safe mode returns the existing unsupported exit status and writes no output. Outside strict-safe mode, the existing announced FFmpeg fallback remains unchanged for those early refusals. Once the FFV1 encoder exists, a changed frame or decoder capability failure is a terminal invalid-data error, never a new native-support refusal or a fallback attempt. The temporary output is removed and an existing destination remains unchanged. This feature does not silently substitute an external FFV1 encoder for a successful native job. Tests disable fallback when proving native success or refusal.

## Codec and container boundaries

The proposed caller shape separates sample coding from container and process policy:

```rust
let mut encoder = coreav_ffv1::Encoder::new(settings, limits)?;
let private = encoder.configuration();
let packet = encoder.encode(&picture, deadline)?;
```

`coreav-ffv1` depends on `coreav-frame`, not on decoders, Matroska, process launchers, or the CLI. Its public types are `Settings`, `Limits`, `Encoder`, and an error type. `Settings` contains the fixed geometry, supported pixel format, and optional sample aspect ratio. The supported-format enum and validated settings own format and slice-grid admission. Version, entropy coder, quantization tables, and checksum policy are implementation choices, not public tuning fields. `configuration()` borrows immutable initialization bytes. `encode()` takes the command's absolute deadline, returns one complete frame packet, and retains no previous packet or picture.

`coreav-transcode::ffv1` owns source demuxing, permitted decoder selection, presentation order, deadline, the temporary packet spool, and the one-track Matroska description. It refuses any transformation of decoded samples. It does not reuse `EncoderProgram`, synthesize an H.264 sample entry, or send FFV1 packets through Annex B parsing. Reusable source timing and packet-feeding logic is shared with the existing pipeline only where it has the same contract. H.264 settings, scaling, external-process state, and H.264 summaries stay out of this path.

The adapter calls the existing `coreav_mkv::write::plan` and `Plan::write`. `coreav_mkv::write::CodecId` gains the Matroska-only `Ffv1` mapping with `V_FFV1` and required initialization bytes. The existing reader already identifies `V_FFV1`. Neither block serialization nor cluster and cue layout needs a second implementation. The general source-copy adapter and MP4 replacement representation need no FFV1 sample-entry extension for this video-only slice.

The native H.264 source backend supplies colour from each picture's activated SPS, captured before the picture is shared with decode workers. It does not repeat the initial container colour on later frames or turn an absent range flag into explicit output metadata. The adapter compares raw frame colour before container inheritance, so disappearing declarations are terminal too. A real fixed-geometry source that changes its H.264 colour matrix after three frames is checked in strict and non-strict modes; both must terminate without publishing output or attempting fallback. The oracle must first confirm that the source actually changes colour while retaining its geometry and native pixel format.

`CodecPrivate` contains only the version-3 Configuration Record from RFC 9043 section 4.3, including its CRC parity. Each block contains one complete FFV1 frame, is visible, and is a keyframe. The Matroska track records the fixed pixel dimensions, exactly representable display aspect, supported colour fields, and progressive scan flag. Unknown colour fields remain unspecified. Conflicting stream/frame colour declarations, changing colour or aspect, and display aspect that the existing writer cannot preserve exactly are refused. The writer cannot record non-default chroma siting, so only unspecified or left chroma locations are admitted; centered and other declared locations are refused before the first packet, or terminally if they appear after encoding starts.

Source presentation order is retained without dropping, duplicating, or retiming frames. The adapter rejects missing, duplicate, or decreasing presentation times and incomplete decoded-frame coverage. It uses the source's positive frame durations, or adjacent presentation deltas where sufficient; an unknown final duration is omitted rather than invented. Rational source times are converted through the existing writer's nanosecond and segment-tick rules. Exact checks compare the resulting integer timestamps, not decimal time strings. Timing that collapses distinct frames to the same stored tick is refused. All packet bytes are spooled before output publication, so a later format or allocation refusal cannot replace an existing destination.

## Sample coding and slice layout

The median predictor, slice-local borders, sign-dependent context, and signed 8-bit residual folding follow RFC 9043 sections 3.1 through 3.8. No lossy quantization is applied to samples. Quantization tables classify neighbouring differences only.

One fixed context table uses positive-side runs of lengths 1, 3, and 124 for each of the first three context inputs. Their scale factors are 1, 5, and 25. The other two tables contain one run of length 128 and contribute zero. The negative half follows section 4.1, including entry 128. The resulting context count is 63. Luma and the shared chroma context bank start at 128 at every independent slice. This is an explicitly chosen small context model, not an imported preset or a size-optimized claim.

The range encoder is derived from the interval equations in section 3.8.1. The Configuration Record uses closed termination. Each slice uses sentinel termination: a discarded zero symbol with state 129 lets the decoder locate the range stream's end before the footer, as section 3.8.1.1.1 specifies. The first slice's range stream includes the frame keyframe symbol; later slices initialize their own range stream. Parameter and slice-header contexts persist within their respective headers; every quantization table starts with fresh states at 128, independently of the preceding table. A black-box check of the pinned oracle's Configuration Record confirms that table boundary. Sample contexts retain their slice-local lifetime. A test-only decoder of the scalar range symbols checks carry propagation, refill, the sentinel's exact end position, and every signed 8-bit residual independently of the image predictor.

Frames with at most 101,376 pixels use one slice. Larger frames use a 2-by-2 raster with exactly four slices, each occupying one raster cell. This satisfies the mandatory version-3 slice-area restriction in RFC 9043 section 5. Large 4:2:0 frames require width and height divisible by four. Large 4:2:2 frames require width divisible by four. These restrictions align internal slice boundaries to chroma samples. Large 4:4:4 frames need no chroma alignment restriction. Small frames use ceiling-rounded chroma geometry, including odd dimensions. Every slice resets prediction borders, covers its region exactly once, and has a checked 24-bit `slice_size`, zero `error_status`, and CRC parity. Configuration and slice parity use section 4.9.3's IEEE polynomial with initial value zero and no pre-inversion or post-inversion.

## Allocation and work bounds

The encoder takes explicit limits. Default bounds are 16,384 pixels per dimension, 16,777,216 pixels per frame, 64 MiB per encoded frame packet, and 128 MiB of encoder-owned live allocation. Caller-owned input pixels and already returned packets are outside that encoder-local budget. The transcode adapter retains only one returned packet at a time, writes it to the spool, and drops it before the next encode. The decoder keeps its existing independent limits.

The live-allocation charge includes initialization, context storage, and actual packet capacity, including any retained scratch capacity and temporary growth overlap. Every input-dependent allocation is charged before a fallible reservation. Packet growth stops at the smaller of the configured packet limit and the remaining allocation budget. Each slice also obeys the 24-bit footer limit. A legal picture may be refused if its encoded packet reaches a bound. No raw-size compression assumption permits an allocation.

The adapter bounds the spool with the existing transcode spool limit. It charges packet metadata before retaining it, bounds frame count, and subtracts its retained index charge before passing the remaining index budget to the Matroska planner. This prevents producer metadata and the writer's 512-byte-per-packet layout allowance from each spending the full index budget. All derived dimensions, offsets, lengths, sample counts, timestamp products, and counters use checked arithmetic and fallible conversions. Errors distinguish unsupported settings, invalid frame input, resource exhaustion, timeout, and I/O failure.

There is no production `unsafe`, unchecked indexing, `unwrap`, `expect`, panic, or unbounded recursion. Sample and context loops are bounded by admitted dimensions and fixed table sizes. The adapter checks the command deadline before decoding, after each returned frame, within encoding at least once per row, during spool copying, and before publication. No queued frames or worker threads are added for this synchronous encoder.

## Exact correctness gate

The required oracle comes only from `scripts/oracle-ffmpeg.env`. The Python gate verifies both supplied executables with the existing pin checker before running either binary. An absent, wrong, or failed oracle is a failure in CI, never a skip or a passing row. FFmpeg runs with fatal decode-error handling. Unexpected decode diagnostics fail the case even if its exit status is zero.

A Rust fixture driver creates deterministic source planes and calls the production encoder and Matroska writer. It writes the original packed pixels and a machine-readable expected manifest alongside each `.mkv`. The Python gate decodes each output with the pinned FFmpeg into the declared native pixel format, with frame synchronization disabled. It compares every byte of every frame, the complete frame count, and the exact end of output against those original planes. It never converts the oracle output to conceal a native-format mismatch. Hashes can identify failures but do not replace the byte comparison. No tolerance, quality metric, known-divergence allowance, or FFmpeg re-encode is an acceptance rule.

The fixture matrix covers all three formats, zero and maximum samples, all byte values, horizontal and vertical ramps, alternating extrema, flat regions, slice-boundary edges, and deterministic pseudorandom planes. It includes one-pixel 4:4:4, odd small subsampled pictures, padded strides, one and multiple frames, the one-slice pixel threshold, four-slice 640-by-480 and 1920-by-1080 pictures, narrow or tall admitted geometry, and repeated frames after unrelated pictures. Slice-boundary content differs on both sides so a missing, overlapping, or misplaced slice cannot pass.

Probe expectations are explicit. Stream fields must match codec name `ffv1`, width, height, native pixel format, stored sample depth where reported, sample aspect ratio, display aspect ratio, progressive field order, colour range, matrix, transfer, primaries, and the written stream time base. Decoded-frame fields must match frame count, presentation timestamp, known duration, dimensions, format, and keyframe status. Packet sizes and FFV1 version/configuration fields are also checked by the spec-derived structural tests. Millisecond rounding is computed as exact rational-to-integer conversion before comparison. Both constant cadence and nonuniform presentation timestamps have fixtures. Unknown duration or colour fields have explicit absence expectations rather than being silently skipped.

End-to-end CLI fixtures exercise MP4 and Matroska inputs through an available native decoder, with source decoded pixels captured independently by the pinned oracle. They require exact equality after FFV1 decoding. They cover automatic and explicit video selection, optional matching `-pix_fmt`, non-square aspect and colour metadata, and a renamed failing external encoder proving that no external encoder is needed. Fallback is disabled and its log must record no execution. Refusal fixtures cover every unsupported format/container family, scan/geometry changes, selected extra streams, encoder options, strict-safe decode restrictions, allocation boundaries, timeout, failed writes, and destination preservation. Repeated and indexed codec options must use the existing effective stream selection rather than a raw argument-string match.

Python comparator mutation tests change one pixel, remove or add a frame, alter a timestamp, change a probe field, truncate decoded output, and replace the oracle executable. Every mutation must fail. A synthetic H.264 stream changes geometry after its first frame; the real non-strict CLI must fail with exit status 1, preserve the destination, and add no fallback log entry. Adapter unit tests also change pixel format and colour after encoding starts while preserving early unsupported-input classification. Rust codec tests cover range symbols, context folding, borders, quantization-table serialization, CRC zero remainders, slice coverage, padded-row exclusion, and exact/one-below resource limits. A bounded fuzz target exercises the encoder over synthetic pictures and limit combinations. Fuzzing and the full oracle matrix run on GitHub Actions.

## Implementation sequence and alternatives

The design is the first commit. The remaining changes stay in the same pull request, ordered as codec with unit tests, bounded Matroska adapter, CLI integration, and required oracle/fixture checks. The plan is reviewed before codec implementation. Local checks stay limited to formatting, focused builds, unit tests, and short script tests. Full workspace tests, platform checks, fuzzing, similarity checking, and the pinned Linux oracle run in Actions. The CLI help, command documentation, and snapshots are updated with the actual admitted subset before the pull request opens.

Two placements were considered. Extending the external H.264 pipeline with an encoder enum would reuse its loop but also couple FFV1 to process discovery, H.264 settings, Annex B output validation, and MP4 sample entries. A small independent codec crate and a video-only Matroska adapter avoid those invalid states. The latter is selected. Sharing the narrow frame feeder where its contract agrees is preferable to copying the entire H.264 pipeline.

Version 1 would avoid Configuration Records and slice footers, but version 3 is selected for self-description, per-slice checksums, and independent bounded slices. RGB requires reversible colour transformation and a different line interleave, so it is deferred instead of silently converting it to YCbCr. Golomb Rice, custom state transitions, multiple table sets, retained inter-frame contexts, audio copying or encoding, and a streaming Matroska writer are also deferred. None is needed to prove exact preservation of the admitted sample planes.

References: coreav-dev/coreav#26, coreav-dev/coreav#267, [Matroska writer design](mkv-write.md), and [compatibility command design](compat-cli.md).
