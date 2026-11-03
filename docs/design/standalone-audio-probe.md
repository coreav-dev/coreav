# Native standalone audio probing

## Scope and sources

`coreav ffprobe` will inspect standalone RIFF/WAVE PCM and IEEE floating-point audio, including WAVE_FORMAT_EXTENSIBLE. RF64 and BW64 add the mandatory `ds64` size substitution, so they are included when their sample format is otherwise supported. Compressed WAVE codecs remain on the existing fallback. RIFX also remains on fallback. The pinned oracle labels a generated RIFX stream as little-endian PCM despite its big-endian format fields; the sample-byte-order question needs a primary specification clause before native support can claim correctness.

The format sources are Microsoft's [RIFF reference](https://learn.microsoft.com/en-us/windows/win32/xaudio2/resource-interchange-file-format--riff-), [WAVEFORMATEX](https://learn.microsoft.com/en-us/windows/win32/api/mmreg/ns-mmreg-waveformatex), and [WAVEFORMATEXTENSIBLE](https://learn.microsoft.com/en-us/windows/win32/api/mmreg/ns-mmreg-waveformatextensible), EBU Tech 3306 for RF64, and ITU-R BS.2088 for BW64. These define chunk lengths, word padding, sample representations, channel masks and 64-bit size replacement.

Native FLAC uses [RFC 9639](https://www.rfc-editor.org/rfc/rfc9639.html), particularly Sections 6 through 8 for the marker, metadata headers, STREAMINFO, seek tables, padding and Vorbis comments. ID3v2-prefixed FLAC is included if the pinned binary accepts it. The ID3v2.3/2.4 header specification defines the bounded synchsafe prefix skip; ID3 content beyond this routing prefix remains unsupported.

Implementation is clean-room Rust from these specifications. FFmpeg 9.0.2 executables are black-box output oracles, never source references.

## Native output contract

The default and JSON writers emit `-show_format` and `-show_streams`, with existing entry selection and audio stream selection. The existing generic writers also render the report in their other supported forms. Empty chapter, program and stream-group sections remain empty.

The format fields are `filename`, `nb_streams`, `nb_programs`, `nb_stream_groups`, `format_name`, `format_long_name`, `start_time`, `duration`, `size`, `bit_rate` and `probe_score`. Unknown values retain each writer's existing omission or `N/A` semantics.

The stream fields are `index`, `codec_name`, `codec_long_name`, `profile`, `codec_type`, `codec_tag_string`, `codec_tag`, `mime_codec_string` for FLAC, `sample_fmt`, `sample_rate`, `channels`, `channel_layout`, `bits_per_sample`, `initial_padding`, `id`, `r_frame_rate`, `avg_frame_rate`, `time_base`, `start_pts`, `start_time`, `duration_ts`, `duration`, `bit_rate`, `max_bit_rate`, `bits_per_raw_sample`, `nb_frames`, `nb_read_frames`, `nb_read_packets` and `extradata_size` where the format supplies it. Dispositions use the shared all-zero audio disposition list. LIST/INFO WAVE tags and FLAC Vorbis comments become format tags in stored order, with the pinned binary's standard key aliases and duplicate handling. FLAC STREAMINFO supplies extradata size. Codec-specific fields that do not apply are absent or unknown, not fabricated.

MP3, Ogg, unsupported WAVE codecs, FLAC pictures/cuesheets requiring extra streams or chapters, nonempty ID3 metadata, and packet/frame listings or counts remain on fallback. Empty ID3v2.3/v2.4 prefixes are supported, including a matching v2.4 footer. A partial interleaved WAVE sample frame stays on fallback because the format does not define duration salvage for it. No decode, encode, raw frame traversal, frozen command-set record or held-out input changes are included.

The pinned oracle leaves FLAC starts unavailable for one audio frame and reports zero for multiple frames. STREAMINFO and the remaining payload extent establish one frame when the payload equals the declared maximum frame size, is shorter than twice the nonzero minimum frame size, or has a known sample total no greater than the minimum block size. A payload larger than the maximum frame size or sample total larger than the maximum block size establishes multiple frames. Ambiguous headers stay on fallback rather than inspecting audio frames or inventing timestamps. Both one-frame and multi-frame generated files are compared in the parity check.

## Parser boundaries and limits

Keep the standalone audio reader beside the existing ffprobe report builders. It returns a validated audio header and the shared report, not decoded samples. Traverse positioned reads; skip sample payloads, padding and seek-table entries without retaining them. Never load the file or declared data chunk into memory.

Apply caller `Limits` before work and allocation. `max_tracks` admits the single audio stream; `max_total_boxes` and `max_top_level_boxes` bound metadata/chunk traversal; `max_box_bytes` bounds retained metadata blocks; `max_retained_bytes` and `max_table_bytes` bound aggregate retained metadata and tags; `max_sample_work` bounds metadata entry work; `max_depth` bounds nested LIST traversal. All offsets, lengths, padded advances and duration arithmetic are checked. Variable storage uses checked sizes and fallible reservation. A huge skipped sample extent is not a metadata allocation.

Recognized malformed files fail as malformed rather than falling back. Source I/O and limit errors always fail the probe. Only explicitly unsupported formats or requested operations fall back. A truncated WAVE data payload is reported from the available complete samples, subject to the specification and the oracle disagreement rule. Malformed optional metadata may be omitted only where the specification permits it; omission must never swallow source I/O or limit errors.

## Verification before opening

Generate every fixture from code, without copying media. Compare stdout byte-for-byte with the verified pinned macOS arm64 FFmpeg 9.0.2 ffprobe for both default and JSON writers. Cover unsigned 8-bit and signed 16/24/32-bit PCM, 32/64-bit float, extensible valid bits and channel masks, RIFX, RF64/BW64, odd chunk padding, truncated data, LIST/INFO tags, FLAC STREAMINFO with and without seek tables and padding, Vorbis comments, and an ID3 prefix. Add native strict-safe regressions so equality cannot be satisfied by invoking fallback.

Exercise small caller limits and source failures through the public limited-probe entry point, including zero streams, too many chunks/comments, excessive metadata bytes and truncated metadata. Unsupported signatures, codecs and packet/frame requests must retain fallback and strict-safe refusal. Run formatting, affected Rust tests and Clippy, public-text and PR-shape checks, and the similarity guard. Heavy suites run in CI.

The specification and file bytes decide oracle disagreements. Record a demonstrable oracle defect with the exact clause; escalate ambiguous semantics rather than choosing an undocumented interpretation. No performance claim is made.

## Extensible PCM precision disagreement

Microsoft's WAVEFORMATEXTENSIBLE Members and Remarks distinguish `wBitsPerSample` (the storage container width) from `wValidBitsPerSample` (the meaningful precision), and identify PCM integer samples by the complete PCM subtype GUID. For a generated stereo 48 kHz PCM input with a 32-bit container, 24 valid bits, block alignment 8 and 3,840 data bytes, the stored frame count is 480. The pinned oracle reports `pcm_f24le`, floating-point sample format and 640 samples instead. CoreAV reports signed 32-bit storage and 480 frames; valid precision does not reinterpret the storage type. This exact generated case is asserted separately from byte-identical fixtures and documented in [the public divergence reference](../known-divergences-ffprobe9.md#extensible-wave-pcm-storage-width). It grants no general output tolerance and changes no frozen records.
