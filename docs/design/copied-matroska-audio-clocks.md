# Copied Matroska audio presentation

## Plan and scope

Lock presentation preservation when `coreav ffmpeg` encodes video and copies AAC or Opus from Matroska into MP4. Decode the complete input and output audio through the same checksum-verified FFmpeg 9.0.2 decoder and compare every F32 PCM byte, including the leading and terminal samples. Exercise both mono and stereo tracks and retain the existing complete-file controls across bitrate options. Refs coreav-dev/coreav#25.

Production behavior does not change. MP4-sourced copies, native transcode and remux, and Matroska output remain unchanged. No command admission, decoder qualification, measurement registry, roster, judging rule, baseline or coverage count changes.

## Specification

[RFC 9559 section 5.1.4.1.25](https://www.rfc-editor.org/rfc/rfc9559.html#section-5.1.4.1.25) defines CodecDelay in nanoseconds as the codec samples discarded during playback. [Section 11.2](https://www.rfc-editor.org/rfc/rfc9559.html#section-11.2) requires subtracting that delay from the Block timestamp. [Section 5.1.3.5.7](https://www.rfc-editor.org/rfc/rfc9559.html#section-5.1.3.5.7) separately defines terminal DiscardPadding. Preserve the presented sample sequence rather than equating a transport-clock tick or a sum of MP4 sample durations with a decoded sample count.

## Known oracle divergence

Generated 0.25-second, 48 kHz mono controls expose a difference in FFmpeg 9.0.2's Matroska-to-MP4 stream copy. The AAC source declares CodecDelay of 21,333,333 ns, corresponding to 1,024 samples; the copied oracle MP4 skips 1,008. The Opus source declares 6,500,000 ns, or 312 samples; the oracle MP4 skips 336. These match the delays rounded first to the one-millisecond transport clock, rather than preserving their sample precision.

| Codec | Input PCM samples | CoreAV copied-output PCM samples | FFmpeg copied-output PCM samples | Oracle presentation difference |
|---|---:|---:|---:|---|
| AAC | 12,000 | 12,000 | 12,000 | The sequence is displaced by 16 samples; an 11,984-sample overlap is byte-identical |
| Opus | 12,000 | 12,000 | 12,144 | The first 24 samples are lost; an 11,976-sample overlap is byte-identical |

For each codec, CoreAV's complete decoded output is byte-identical to the input's decoded audio. The AAC oracle output retains 16 preceding samples and loses 16 terminal samples. The Opus oracle output loses 24 leading samples and retains 168 samples beyond the input presentation. Equal AAC sample counts alone cannot detect the displacement.

The AAC MP4 sample-duration sum is 13,312 ticks in CoreAV and 13,000 in the oracle. That aggregate difference is not evidence of incorrect CoreAV presentation: adopting the oracle clock and trims changes the decoded sequence. These observations do not assert that every MP4 sample delta must equal the codec frame length.

The regression extends `matroska_copy_bitrate_keeps_complete_native_files` in `crates/coreav-cli/tests/ffmpeg_audio_options.rs`. Four generated two-track inputs cover both direct Matroska encoding and an MP4-to-Matroska copy that retains terminal trims. All eight mono/stereo stream checks require complete input/output F32 byte identity. The trimmed controls require 12,000 samples per channel; directly encoded AAC retains its untrimmed final frame and presents 12,288. Twenty-four bitrate-option outputs must also retain the complete native file bytes.

The trimmed controls reproduce the oracle displacement and length differences above as negative controls, so an equal AAC sample count cannot substitute for equal sample values. The same decoder is used on both containers, so this establishes presentation preservation, not bit-exactness between independent lossy decoders. The oracle's differing stream-copy output is documented, not adopted as an acceptance target or registered as a measurement exception.
