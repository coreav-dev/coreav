# Copied MP3 bitrate metadata

MP4-sourced MP3 copied beside encoded video retains source bitrate fields today. A generated 48 kHz control carries 67,511 bits/s, while pinned FFmpeg 9.0.2 writes 64,000. The difference comes from a shortened final stored sample: the complete compressed frames represent more samples than the container table clock.

## Clock and ownership

The writer reuses `coreav_audio::mp3::Header` rather than adding a second MPEG audio header parser. A metadata-only packet pass requires one complete Layer III frame per MP4 sample, consistent MPEG version and sample rate, and the same sample count as the completed output tables. It sums 1,152 samples per MPEG-1 frame or 576 per MPEG-2 frame. The existing parser also supports the non-ISO MPEG-2.5 extension. The average is `packet_bytes * 8 * sample_rate / decoded_frame_samples`, rounded down. It uses actual frame bytes, including padding: a generated 44.1 kHz constant-rate control averages 63,978 rather than its nominal header rate of 64,000.

ISO/IEC 11172-3 and 13818-3 establish the frame sample counts. The choice between complete-frame and shortened stored clocks for bitrate hints is an oracle compatibility policy, not a claim that the specifications forbid the stored durations. The maximum is at least the average. An explicit request supplies its other hint; otherwise the source's stored-clock average supplies it. Buffer size is zero. Neither packets nor sample tables change.

The extra packet pass uses only work left after the existing listing and copy passes. Insufficient work, malformed headers, partial or compound samples, free format, inconsistent clocks and source errors leave the complete entry unchanged. They never add a refusal. The writer depends on the existing audio crate, which the CLI and both bindings already use; no decoder implementation changes.

## Default and excluded paths

The ffmpeg adapter's already-native video-encode route distinguishes no request from an explicit zero. The shared `CopiedAudioBitRate` type has `FromSource` and `Requested(u32)` states; outer `None` preserves native metadata. Missing, malformed and out-of-range `-b:a` values use `FromSource`. A generated MP3 control without `-b:a` writes maximum 67,511 and average 64,000; an explicit zero writes 64,000 for both. This also normalizes copied AAC and Opus metadata when `-b:a` is absent, as the pinned oracle does for stale or missing fields. Native transcode and ordinary remux retain `None` and their existing bytes.

Ignored values are not an oracle-parity claim. In a generated AAC control, FFmpeg wraps a request of 4,294,967,296 into maximum zero, while CoreAV retains its absent-request policy. Previously ignored values still introduce no refusal.

Matroska input and output remain unchanged. MPEG-2 AAC object types and QuickTime version-1/version-2 sound entries remain explicit, tested skips rather than new admission checks. Exact Matroska copy-clock parity is separate work.

## Verification

Generated CBR and VBR MP3 controls at 48, 44.1 and 22.05 kHz compare `esds` and `btrt` with the pinned oracle, with and without `-b:a`. Full-file witnesses preserve every non-metadata byte, including the shortened stored sample duration. AAC and Opus controls include absent requests with stale or missing metadata. Synthetic MP3 controls prove byte-identical fallback for unproved clocks and an exhausted optional-work allowance. Native transcode and Matroska before/after files remain byte-identical. New dependency notices are checked without changing distribution licenses.
