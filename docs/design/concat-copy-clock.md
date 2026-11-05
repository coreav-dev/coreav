# Copied AAC concat packet clocks

## Outcome

The proposed native AAC concat clock is deferred. The generated prototype matched packet clocks and decoded output only after inserting AVC parameter sets and changing the video sample-entry label to `avc3`; pinned FFmpeg 9.0.2 writes `avc1`. That is not exact output. The specification text available for this work does not settle the disputed parameter-set/sample-entry interpretation, so this change neither writes that combination nor registers a known divergence.

All H.264 compatibility concat now refuses through the existing unsupported-command path after the existing timing checks. Strict-safe mode exits 4 without publishing, while normal mode forwards the complete original command to FFmpeg. Native `coreav join`, ordinary copy and the existing parser/writer owners stay unchanged. The abandoned clock, AVC scanner, sample-entry rewrite and special chunk scheduler are removed rather than retained as unreachable code. No native coverage gain is claimed.

## Generated observations

The public [concat demuxer documentation](https://ffmpeg.org/ffmpeg-formats.html#concat) says each file starts after the preceding file's duration and that unequal stream lengths can leave gaps. Short checksum-verified FFmpeg 9.0.2 observations found:

- Primed 48 kHz AAC copy retains encoded priming packets and applies a global first-input origin shift, including when audio is unselected.
- Boundary decode times can clamp to the preceding packet's decode time plus one; stored preceding deltas then reflect those final decode times rather than each input's original cadence.
- The reference inserts SPS/PPS before IDR slices from ordinary H.264 inputs. The older native concat path omitted those bytes: a generated IDR was 1959 bytes natively and 1997 bytes in the reference.
- Matching each track independently is insufficient: the unchanged copied-output comparison retains global packet order.
- The discarded prototype's `avc3` versus reference `avc1` label difference remained visible in `codec_tag_string` and `codec_tag`, even when packet payloads and clocks matched.

These are development observations, not a coverage or timing run. A future clock implementation needs an exact admitted output representation before it can produce native output.

## Regression contract

The existing unedited audiovisual concat regression now compares complete packet bytes and global packet order. Adding this assertion failed on the old native output before the refusal fix. Its strict-safe case now requires refusal and no output; its normal-mode case compares the complete fallback file, stream fields, packets and decoded presentation with the pin.

The twelve primed development combinations likewise require strict-safe absent/existing-destination preservation and complete real fallback bytes. The comparisons retain the entire `-show_streams -show_data` result, including `codec_tag_string`, `codec_tag`, profile, codec configuration, colour, channel and sample-entry-derived tags, instead of choosing a stream-field allowlist. Packet comparisons retain bytes, hashes, sizes, flags and clocks in global order; only the container byte position is excluded where native container layout is not part of the contract.

Five previously native generated outputs now require fallback: the unedited audiovisual output and four video-only audio-gap controls. The twelve provisional clock outputs also refuse, retaining main's earlier behavior for primed AAC. Other codecs are not generally certified by this change. An AV1 video-only control with absent optional field-order and encoder metadata compares every exposed stream field and packet byte without changing the comparison.

All structural selection and placement assertions remain, using VP9 where H.264 is now intentionally unsupported. Existing positive-AAC, multi-edit, gap, malformed-input and original-argument fallback controls remain. The independent generated non-AVC control is not evidence for untested metadata combinations.

## Boundaries

No frozen roster, representative, command, generator, judging field, threshold, limit or result is changed or run. Any later registered coverage measurement remains separate. The sample-entry mismatch is a refusal, not a known-divergence exception. Refs coreav-dev/coreav#25.
