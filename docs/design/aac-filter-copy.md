# Keep AAC concat filtering on exact fallback

## Problem

An explicit output `-bsf:a aac_adtstoasc` refuses during option parsing, including MP4 concat inputs whose AAC access units already use AudioSpecificConfig rather than ADTS. The filter removes ADTS transport headers and supplies AudioSpecificConfig, as described in the [public FFmpeg bitstream-filter documentation](https://ffmpeg.org/ffmpeg-bitstream-filters.html#aac_005fadtstoasc).

AAC packet identity alone does not qualify this stream-copy command. Native and pinned output must match as complete files. Independent generated H.264/AAC examples show that pinned concat inserts SPS/PPS NAL units even without the explicit audio filter, while the existing native join keeps the original video packets. VP9 packet identity also leaves different MP4 brands, box order and metadata. These are existing plain-copy differences, not effects of the AAC filter.

## Decision

Keep the existing bitstream-filter refusal for every explicit AAC concat-filter command. No byte-exact native subset has been established, so this change admits none. Do not invoke an external oracle inside strict-safe mode or add an unused checked-output API. Native AAC conversion, H.264 normalization and MP4 byte-layout matching are outside this change.

Strict-safe mode exits 4 before publication, preserving absent and existing destinations. Regular mode retains the original arguments and existing banner prefix when running the real pin. The acceptance check compares the entire fallback file byte for byte, with no discarded video hashes, packet sizes or bitrate fields. Matching decoded values or filtered/unfiltered output within one program is not a native pass.

Unfiltered copy and concat behavior stays unchanged; their demonstrated byte differences require separate work. Input generators, frozen tests, command data, judging, baselines, limits and tolerances stay unchanged. This change provides regression coverage, not new native coverage.

## Verification

- Use independent generated inputs and the pinned FFmpeg 9.0.2 binaries, never another implementation's source or a frozen judging rerun.
- Cover configured AAC and no selected audio, plus non-AAC audio, later-input transport-like bytes and filter syntax, position, selectors and non-concat uses.
- Require strict-safe exit 4, empty stdout, absent/existing destination and staging preservation, then identical real-pin exit status and complete output bytes through fallback.
- Retain exact argument replay and the inherited banner prefix. Run the generated regression, affected CLI tests, Clippy, formatting, pin/reference/text/shape checks and an exact-head canonical guard before readiness.
