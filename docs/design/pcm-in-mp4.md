# Native S16 PCM in MP4

## Scope

Add native audio-only S16 PCM output in MP4 using the existing registered decoder and conversion plans. Initially admit regular-file output, one selected audio stream, mono 48 kHz input and output, unchanged channel count and rate, and effective `pcm_s16le`. Keep decoder feature gates and numerical contracts unchanged. AAC remains opt-in and decode-tolerance output, never exact-output evidence. Default builds and licenses do not change.

One pull request contains this plan as its first commit, the implementation and regressions. No frozen input, generator, command, result, judging predicate, baseline, category registration or public count changes. Only independently generated development inputs are executed. A default video encode is not replaced with video copying: commands that retain video keep their existing refusal.

## Before implementation

Capture the unchanged baseline binary, build features, input hashes and complete native outcomes before changing runtime code. Every baseline-native raw/WAV command must remain native in normal and strict modes with unavailable fallback, preserving its entire output. New PCM-in-MP4 support must not move an existing native command to fallback. Stop before pushing if this gate cannot be met.

After committing this plan, generate short mono 48 kHz WAVE and FLAC controls with checksum-verified FFmpeg 9.0.2. Observe the reference MP4 sample-entry fourcc/version, `pcmC` payload, channel-layout children, file-type brands, metadata, edits, sample granularity and terminal packet. Record those observations here before implementation. Use ISO/IEC 23003-5 and the MP4 registration authority for the PCM binding, and ISO/IEC 14496-12 for the container. Binaries are black boxes; do not read other implementations' source or history. Do not guess the layout from a filename or codec label.

## Ownership

### Observed reference layout

After the first plan commit, independent mono 48 kHz S16 WAVE, FLAC-in-MP4 and AAC controls produced `ipcm` version-0 audio entries with 16-bit samples. Their `pcmC` payload is `00 00 00 00 01 10` (version/flags zero, little-endian flag, 16-bit width); no `chnl` child is present. `btrt` carries zero buffer size and equal 768,000-bit/s maximum/average rates. File brands are `isom`, minor version 512, then `isom`, `iso2`, `mp41`. Movie and media clocks are 48,000 Hz; an identity edit covers the complete frame count. Movie metadata carries the existing pinned muxer encoder tag.

Each PCM frame is one two-byte MP4 sample with duration one tick. The reference stores one contiguous audio chunk, while its probe groups samples into 1,024-frame packets plus the complete short terminal packet. A 1.1-second control crosses the one-second boundary without splitting the packet clock at that boundary. The implementation must verify this behavior through the unchanged mux API rather than assume its chunk policy is invisible. These are layout observations, not a compatibility or measured-coverage result.

Use WAVE and containerized FLAC for exact controls through the common two-pass decoder. Standalone FLAC has a separate prepared-stream output owner; it remains outside this new MP4 route rather than bypassing that owner or accidentally emitting its raw bytes into an MP4 filename.

Route only MP4 with exactly one selected audio stream and effective S16 PCM through the current raw-audio plan. Keep the existing outside-WAV/raw guard for commands with video and every unsupported layout. Add a private `raw_audio/mp4.rs` output adapter; reuse the public MP4 muxer's sample-entry, header, table and write APIs without changing its public interface or the transcode writer.

The existing raw-audio owner retains decoder selection, exact conversion composition, the preflight and execution decodes, their equality check, output staging and terminal error classification. Explicit muxer dispatch prevents an MP4 request from falling through to headerless raw bytes. For MP4 only, refuse absent qualified frame counts, stdout, unsupported source metadata, chapters or dispositions before publication. Reuse the existing metadata predicate rather than establish a second list of supported tags; any extraction from the audio-encode owner must preserve its behavior.

Use the preflight's complete presented frame count to prepare bounded PCM sample tables and checked byte counts before writing a header. Stream the execution decode through the existing S16 packing owner. Require the complete bytes written to equal the muxer's declared data extent and `frames * channels * 2`. Missing/extra samples, source changes, truncation, I/O, decoder and resource failures remain terminal; they must not become fallback after native execution starts.

Changes are limited to this note, `coreav-cli/src/ffmpeg/mod.rs`, `raw_audio/mod.rs`, the new `raw_audio/mp4.rs`, `ffmpeg/audio.rs` only for behavior-preserving metadata-predicate extraction, a new `coreav-cli/tests/ffmpeg_pcm_mp4.rs`, affected module unit tests and `docs/agents/ffmpeg-cli.md`. Do not change `join.rs`, the existing WAV header owner, decoder/encoder APIs or Cargo features.

## Verification

Compare complete generated lossless-input results with the pin, including stream and format metadata, sample-entry fields, packet clocks and payload extent. Independently require the stored PCM to equal the same CoreAV raw-S16 command's entire output, so the container cannot hide a conversion or trimming error. For AAC development controls, reuse the registered full presentation, numerical and packing contract without widening it; validate all non-PCM fields and clocks separately and make no exactness or category claim. Short or prefix comparisons cannot qualify output.

Exercise absent/existing destinations, publication failure, unsupported stdout and metadata, frame/count/table limits and terminal late I/O/source failures. Verify that the video-bearing PCM command retains its refusal and complete original-command fallback. Re-run baseline-native raw/WAV forms in both modes with unavailable fallback and require zero native-to-fallback moves and unchanged complete bytes.

Run affected default and AAC-enabled CLI targets with the required pinned oracle, all-target Clippy, formatting and the canonical required-oracle workspace with complete fixture prerequisites. Validate public text, references, signed commit shape and unchanged frozen identities before the batched push. Obtain final-head similarity verification, CI and independent review before readiness. Refs coreav-dev/coreav#25.
