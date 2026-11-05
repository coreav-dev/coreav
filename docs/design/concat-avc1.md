# Native AVC concat with configuration-only parameter sets

## Goal

Restore native `coreav ffmpeg -f concat -i LIST -c copy OUTPUT.mp4` for the H.264 cases that coreav-dev/coreav#426 moved to fallback. The output uses `avc1` with SPS and PPS NAL units only in `avcC`. It is compatible output, not byte-exact FFmpeg 9.0.2 output.

ISO/IEC 14496-15:2017 Table 2 prohibits SPS and PPS NAL units in `avc1` and `avc2` samples. Table 1 of the seventh-edition FDIS, w23225, retains that rule. Byte-identical duplicate parameter sets are still parameter sets. The pinned FFmpeg concat output repeats them before IDR pictures. CoreAV must not reproduce that carriage or substitute `avc3`.

## Implementation plan

1. Keep the existing join planner, configuration comparison, edit admission and bounded copy owner. Remove the blanket H.264 compatibility refusal only after the writer can establish the configuration-only invariant before publication. Different input configurations, unsupported sample descriptions and unrepresentable timelines continue to refuse atomically.
2. Preserve the selected source's visual encoder metadata and the first input's FLAC STREAMINFO or ALAC cookie instead of zeroing fields during a full-input join. Keep excerpt handling for clipping and splitting separate. Correct stored video media duration without changing the admitted packet clocks or presentation edits.
3. Extract the existing compatibility encoder identifier into one shared CLI owner used by WAV and MP4. Pass container-neutral metadata to the writer rather than making the native library depend on a pinned external encoder version. Preserve the source's compressor name rather than inventing an encoder identity for copied video.
4. Restore the eleven generated controls and the unchanged C02 instruction-guard command to native execution. Add full-output comparisons for decoded video and audio, every aligned packet and complete stream records. Explicit assertions must account for parameter-set bytes, bitrate, physical layout and any remaining metadata difference. The malformed-AAC preservation control must retain its matching decode failure rather than claim successful audio decoding.

## Verification boundaries

The eleven generated controls are the unedited audiovisual case, four video-only audio-gap cases, two FLAC and ALAC copy cases, and four unfiltered AAC-filter controls. The twelve primed-AAC cases remain refusals and are a follow-up. Explicit AAC filters, positive AAC edits and multiple audio edits keep their existing admission rules. Other native command coverage must not decrease.

C02's command, inputs, representative and `bench/guard/baseline.json` stay unchanged. Restore only its current guard routing. Compare its native instruction count against the existing five-percent tolerance. If it exceeds that tolerance or needs a new baseline, stop and report the result instead of modifying the frozen material.

Run focused writer and CLI regressions with the checksum-verified FFmpeg 9.0.2 oracle, relevant Python checker tests, formatting and Clippy. PR CI runs the instruction guard and complete affected fixture checks. Run the clean-room similarity guard at the public head before reporting readiness. No implementation source from other media libraries is used.

## Scope

This plan and its implementation share one PR. The plan is the first commit. Top-200 compatible-category registration and the twelve never-native primed-AAC cases are separate work. Refs coreav-dev/coreav#25.
