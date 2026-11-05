# Copied AAC concat packet clocks

## Outcome

The proposed primed-AAC concat clock remains deferred. Its generated prototype matched packet clocks and decoded output only after inserting AVC parameter sets and changing the video sample-entry label to `avc3`; pinned FFmpeg 9.0.2 writes `avc1`. That prototype is not part of the native writer.

The [configuration-only AVC concat design](concat-avc1.md) restores the twelve formerly native command cases with `avc1`, SPS/PPS only in `avcC`, corrected metadata and unchanged packet clocks. ISO/IEC 14496-15:2017 Table 2 and the seventh-edition FDIS Table 1 prohibit in-sample SPS/PPS under `avc1`. Compatible output therefore omits the parameter sets FFmpeg 9.0.2 inserts and is not counted as exact FFmpeg output. Unsupported framing, changing configurations and the existing timing/filter cases still refuse atomically and forward the original command in normal mode.

## Generated observations

The public [concat demuxer documentation](https://ffmpeg.org/ffmpeg-formats.html#concat) says each file starts after the preceding file's duration and that unequal stream lengths can leave gaps. Short checksum-verified FFmpeg 9.0.2 observations found:

- Primed 48 kHz AAC copy retains encoded priming packets and applies a global first-input origin shift, including when audio is unselected.
- Boundary decode times can clamp to the preceding packet's decode time plus one; stored preceding deltas then reflect those final decode times rather than each input's original cadence.
- The reference inserts SPS/PPS before IDR slices from ordinary H.264 inputs. The older native concat path omitted those bytes: a generated IDR was 1959 bytes natively and 1997 bytes in the reference.
- Matching each track independently is insufficient: the unchanged copied-output comparison retains global packet order.
- The discarded prototype's `avc3` versus reference `avc1` label difference remained visible in `codec_tag_string` and `codec_tag`, even when packet payloads and clocks matched.

These are development observations, not a coverage or timing run. A future clock implementation needs an exact admitted output representation before it can produce native output.

## Regression contract

The unedited audiovisual regression and the other formerly native generated controls require strict-safe native success. Their shared comparator checks complete packet records in global order, complete stream and format records, decoded samples and pixels, frame timing and stored media duration. It removes only oracle SPS/PPS copies that match `avcC` byte for byte at the expected IDRs. Payload-derived bitrates, physical positions, and the FLAC/ALAC metadata differences remain explicit assertions.

The twelve primed development combinations likewise require strict-safe absent/existing-destination preservation and complete real fallback bytes. The comparisons retain the entire `-show_streams -show_data` result, including `codec_tag_string`, `codec_tag`, profile, codec configuration, colour, channel and sample-entry-derived tags, instead of choosing a stream-field allowlist. Packet comparisons retain bytes, hashes, sizes, flags and clocks in global order; only the container byte position is excluded where native container layout is not part of the contract.

Twelve previously native cases return from fallback to native output: the unedited audiovisual output, four video-only audio-gap controls, two FLAC/ALAC audio-copy cases, four unfiltered AAC-filter controls, and C02. The malformed AAC control preserves the reference's identical strict decode failure and emitted prefix. AC-3/E-AC-3 retain their multi-edit timing refusal. The twelve provisional primed-AAC clock cases remain refused. The instruction guard's C02 command, fixture recipe and baseline stay unchanged and receive a native instruction count again. Other codec families are not generally certified by this change.

All structural selection and placement assertions remain, using VP9 where H.264 is now intentionally unsupported. Existing positive-AAC, multi-edit, gap, malformed-input and original-argument fallback controls remain. The independent generated non-AVC control is not evidence for untested metadata combinations.

## Boundaries

No frozen roster, representative, command, generator, judging field, threshold, limit or baseline is changed. C02's existing guard is executed, but no registered coverage measurement runs. Top-200 compatible-category registration and the twelve primed-AAC cases remain separate work. Refs coreav-dev/coreav#25.
