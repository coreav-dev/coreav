# Known divergences: audio output versus FFmpeg 9.0.2

These entries describe existing audio-output checks, including a default encoder refusal. They do not add a machine-readable acceptance rule, change a tolerance, or reinterpret a frozen measurement. The linked tests own each exception's scope. Commands below use generated inputs and the pinned FFmpeg 9.0.2 binaries, never an arbitrary installed version.

## Opus MP4 PCM decode honors the edit window

For generated 100 ms mono inputs at 48 kHz, `coreav decode-audio --format f32 INPUT OUTPUT.wav` presents the finite edit's exact 4,800 samples. Default pinned FFmpeg 9.0.2 decoding to `-f f32le` writes 5,448 samples for SILK and Hybrid encodes and 5,640 for low-delay CELT. Their six packets each decode to 5,760 samples; the rate-1 edit starts at media time 312 or 120 and ends at 5,112 or 4,920. The oracle retains the rest of that final packet. This is not permission for CoreAV to present the tail or to accept an incorrect count.

[Opus in ISOBMFF sections 4.3.2 and 4.4](https://opus-codec.org/docs/opus_in_isobmff.html#4.4) assign whole-playback startup and terminal trimming to the edit list and make dOps PreSkip informative for that purpose. Changing only PreSkip from its original value to 3,840 must not change the finite edit's presented samples. Before the correction, CoreAV shortened the 4,800-sample SILK edit to 1,272 samples. That was a correctness bug, not a valid oracle divergence.

[`opus_mp4_presentation.py`](../scripts/opus_mp4_presentation.py) verifies the source's movie and media clocks, rate-1 edit, mono configuration, packet payload locations, mode and decoded packet durations before comparing counts. It requires complete native S16 and F32 output at one and eight threads, unchanged by informational PreSkip, and exact prefix identity when the edit end is shortened. Its packet-boundary controls set edit duration to 5,448 or 5,640: both native and default oracle counts must then agree. The finite-edit cases require the pinned oracle's full packet tail; any other count fails. These generated controls do not change the existing decoder waveform thresholds or any frozen comparison.

## Opus MP4 re-encoding preserves the source edit

The generated two-second mono Opus MP4 presents 96,000 samples at 48 kHz. Its movie and media timescales are both 48,000. Its single rate-1 edit starts at media time 312 and lasts 96,000 ticks; `dOps` pre-skip is 312. FFprobe reports `start_pts=0`, `duration_ts=96000`, and `initial_padding=312` for this source.

The duration exception covers exactly these two source and output-codec pairs in the [transcode fixture test](../crates/coreav-cli/tests/transcode.rs):

| CoreAV command | FFmpeg command | Presented output samples per channel |
|---|---|---|
| `coreav transcode opus.mp4 coreav-aac.mp4 --audio aac --encoder x264 --x264 X264 --ffmpeg FFMPEG` | `ffmpeg -threads:a 1 -i opus.mp4 -c:v libx264 -c:a aac reference-aac.mp4` | CoreAV 96,000; FFmpeg 96,648 |
| `coreav transcode opus.mp4 coreav-opus.mp4 --audio opus --encoder x264 --x264 X264 --ffmpeg FFMPEG` | `ffmpeg -threads:a 1 -i opus.mp4 -c:v libx264 -c:a libopus reference-opus.mp4` | CoreAV 96,000; FFmpeg 96,648 |

`X264` and `FFMPEG` name the encoder executables. The test wraps the AAC encoder process to compare its complete F32 input with CoreAV's direct audio decode. The corresponding audio-only `coreav ffmpeg -i opus.mp4 -vn -c:a aac output.mp4` and `-c:a libopus` paths must produce the same audio fields as the respective CoreAV transcode outputs. These are not additional oracle exceptions.

FFmpeg reports the source duration correctly but decodes 96,648 samples and retains those samples in the re-encode. CoreAV feeds only the source's 96,000 presented samples to the encoder, then removes the new encoder's delay and terminal padding from its output presentation.

ISO/IEC 14496-12 section 8.6.6 defines the edit's presentation duration. [Opus in ISOBMFF 0.6.8, section 4.4](https://opus-codec.org/docs/opus_in_isobmff.html#4.4) specifies sample-accurate startup and terminal trimming through that edit. Section 4.3.2 leaves whole-playback priming removal to the edit list. Keeping the padded tail would extend the source presentation; applying pre-skip again would trim it twice.

`audio_encodes_preserve_the_decoders_presented_samples` checks the source edit and configuration before both comparisons. `opus_mp4_padding_pair` admits only Opus in MP4, AAC or Opus output, and the exact 96,000/96,648 counts. It changes only the compared output `duration_ts`; all other selected audio fields remain equal. The assertion fails if FFmpeg starts producing 96,000 or either count changes. `the_opus_mp4_padding_exception_is_exact_and_cannot_become_stale` also rejects changed codecs, containers, output modes, counts, and stale agreement.

### The composed AAC baseline does not own presentation

The active [composed Opus-to-AAC check, version 2](design/composed-opus-aac-v2.md), applies the same source-presentation rule to implicit AAC selection by `coreav ffmpeg`, including commands such as `coreav ffmpeg -i opus.mp4 -c:v libx264 output.mp4`. Its ordinary FFmpeg source-to-AAC baseline supplies codec, profile, rate, channel count, and start properties, but not the presented sample count or raw priming offset. The retained version 1 control has 97,672 packet-duration samples minus 1,024 encoder priming samples, giving 96,648 against 96,000 captured samples. That historical failed control is not relabeled as agreement.

Version 2 derives the presentation independently from source bytes and complete decoded packet work. The captured PCM and candidate AAC must cover that exact presentation. An independent AAC replay of the exact captured PCM must match ordered packet payloads, codec configuration, and the complete presented F32 decode. This is not a general 648-sample allowance or permission to truncate, align, or repair a candidate. It also does not claim that the two-pair fixture exception covers arbitrary inputs.

[`scripts/tests/test_composed_audio.py`](../scripts/tests/test_composed_audio.py) checks these boundaries in `test_source_window_uses_declared_edits_and_packet_work_not_candidate_length`, `test_source_count_rate_channels_and_origin_cannot_be_chosen_by_the_capture`, and `test_interior_durations_cannot_hide_behind_unchanged_endpoints`. `test_normal_aac_baseline_does_not_own_duration_or_priming` preserves the baseline's limited role. Unlike the fixed two-pair exception above, this source-derived rule does not require FFmpeg to keep disagreeing. A future agreeing baseline still has to pass the independent source and replay checks.

## Opus MP4 to Matroska copy preserves the edit end

For the generated 0.24-second H.264/Opus MP4, compare `coreav ffmpeg -i input.mp4 -c copy output.mkv` with `ffmpeg -i input.mp4 -c copy reference.mkv`. The source edit presents 11,520 mono samples at 48 kHz. CoreAV's Matroska output decodes to 11,520 samples. FFmpeg's copy decodes to 12,168 because it does not carry the final 648 padded samples as `DiscardPadding`.

CoreAV's decoded audio is byte-identical to the corresponding prefix of the oracle copy. Selected stream fields, packet payload hashes, and decoded video remain equal. This is a presentation trim, not a packet-payload or decoder-quality exception.

[Opus in ISOBMFF sections 4.3.4 and 4.4](https://opus-codec.org/docs/opus_in_isobmff.html#4.4) distinguish sample duration from the actual presentation duration. [RFC 9559 section 5.1.3.5.7](https://www.rfc-editor.org/rfc/rfc9559.html#section-5.1.3.5.7) defines Matroska `DiscardPadding`. CoreAV carries the source's finite presentation end into that field instead of exposing the terminal padding.

`iso_opus_keeps_the_declared_edit_end_instead_of_decoding_the_padded_tail` in [`ffmpeg_matroska.rs`](../crates/coreav-cli/tests/ffmpeg_matroska.rs) binds the source's parsed presentation end to 11,520 samples, requires the exact decoded counts and prefix equality, and checks the final packet's `discard_padding=648`. Oracle agreement or a different extra count fails the assertion. The [Matroska writer design](design/mkv-write.md) describes the mapping; this fixture assertion is not a blanket exception for all Matroska copies.

## MP3 output keeps the original boundary length

For a supported lossless source containing `N` samples per channel, compare `coreav ffmpeg -i input.mp4 output.mp3` with `ffmpeg -i input.mp4 reference.mp3`. Explicit `-c:a libmp3lame` selects the same encoder path. Let `F` be the codec frame length: 1,152 samples at 32, 44.1, and 48 kHz, and 576 at the lower supported rates. When `r = N mod F` is from 1 through 46, the pinned FFmpeg clamps final discard padding and presents `N + 47 - r` samples. CoreAV presents exactly `N`.

CoreAV adds an explicit EOF zero guard before the external encode, validates the complete frames and timing, and accounts for that guard in its own gapless tag. FFmpeg and ffprobe read the resulting seekable MP3 at its original length. This preserves the source sample count without claiming that either encoder's compressed bytes must match. The [MP3 design](design/transcode.md#mp3-audio) defines this contract; no new interpretation of an audio quantizer or general duration tolerance is involved.

`ffmpeg_mp3_eof_guard_preserves_exact_boundary_lengths` in [`transcode.rs`](../crates/coreav-cli/tests/transcode.rs) generates `N = F + r` for residues 1, 46, and 47 at 48, 22.05, and 8 kHz. It requires CoreAV's decoded length and ffprobe duration to equal `N`, the oracle length to equal the formula above, and endpoint quality to remain within the existing registered comparison. The residue-47 control requires direct length agreement. Agreement at residues 1 or 46, or any changed extra count, fails the oracle-length assertion. This rule covers no other timing difference and does not apply the MP3 file's gapless trim to AVI packets.

## The default FLAC encoder refuses fewer than 16 samples

For supported 16-bit or 24-bit lossless sources, `coreav ffmpeg -i input.mka output.flac` accepts positive sample counts from 1 through 15. The same default `ffmpeg -i input.mka reference.flac` refuses them with exit status 234 and a diagnostic ending in `invalid block size: N`, where `N` is the exact count. Empty output and counts of 16 or more are outside this exception.

[RFC 9639 sections 4.1 and 8.2](https://www.rfc-editor.org/rfc/rfc9639.html#section-8.2) permit a final block smaller than 16 samples, including a one-frame stream. STREAMINFO's declared block-size bounds must still be from 16 through 65,535. CoreAV declares both bounds as 4,096 and writes the actual short final size in the frame header under section 9.1.1. Its output is independently decoded and checked against the source samples, total sample count, MD5, and legal header fields before the reference encode runs.

[`scripts/flac_output_check.py`](../scripts/flac_output_check.py) records the default refusal, then uses `ffmpeg -i input.mka -frame_size 4096 reference.flac` only for those tiny positive counts. The explicit reference must decode exactly and match the selected probe fields. This is a correctness-only reference setting, not default-command agreement or size or speed evidence. The [FLAC output design](design/flac-output.md) retains the complete source and header checks.

`encode_reference` fails if the default oracle succeeds, reports another error, times out, or changes the exact diagnostic/count binding. `test_only_the_specific_default_refusal_allows_a_tiny_reference` and `test_tiny_reference_rule_excludes_empty_and_normal_outputs` in [`test_flac_output_check.py`](../scripts/tests/test_flac_output_check.py) exercise those boundaries. `tiny_final_blocks_keep_legal_streaminfo_bounds_and_their_actual_size` in [`flac_encode.rs`](../crates/coreav-audio/tests/flac_encode.rs) checks the native final-block contract.
