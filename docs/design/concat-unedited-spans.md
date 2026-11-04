# Preserve unedited video spans in joins

## Plan

An unedited video's first composition time need not be zero. The media header's duration is the sum of decode durations (ISO/IEC 14496-12 8.4.2.3), not an absolute composition-time endpoint. Without an edit list, media time is presentation time; composition offsets establish when each sample is presented (8.6.1 and 8.6.6).

Before this change, the join writer capped an unedited video's presentation end at the media duration measured from zero. This could cut off its final presented frames. It also prevented the previous input's last sample from spanning the gap before the next input, producing extra edit boundaries instead of the concat muxer's extended decode duration.

An independent H.264/AAC development source with no edit lists has video starting at 1024/12288 seconds, video duration 3.25 seconds and audio starting at zero. Two-file concat previously reported video duration 6.5 seconds instead of pinned FFmpeg 9.0.2's 6.583333 seconds. The format duration used to place the second file already agrees; changing ffprobe's shared timing or its output-only override would not fix the writer's end bound.

Use the existing sample-derived presentation span when no edit supplies an end. Keep explicit edit windows, native join placement rules and all parser/read/work budgets unchanged. Do not add another packet reader or propagate ffprobe output timing into planning. This corrects both native join preservation and its FFmpeg concat caller at the source of the lost tail.

There is a separate first-input-origin constraint at the compatibility boundary. A video-only source with a positive format start is normalized to zero by FFmpeg concat, while native join preserves that start. Keep such a command on explicit refusal/fallback until that normalization is implemented exactly; do not shift native join globally. Existing positive-AAC-edit and multi-edit refusals remain.

## Acceptance

- Add a failing writer regression that independently checks every input sample's presentation after an unedited composition delay, both native and explicit concat placement, without weakening existing edit-list tests.
- Compare independent playable zero-origin audiovisual concat with pinned FFmpeg 9.0.2 using unchanged exact stream/frame timing, decoded pixels and full-precision audio checks. Include one and multiple inputs and reordered video; do not certify other source classes from these controls.
- Check positive first-origin refusal, original-argument fallback, strict-safe output preservation and zero-origin native controls. Native join keeps its own first origin. Ordinary copy, seek planning, raw probe and ffprobe output remain unchanged.
- Run writer and CLI fixtures, feature-enabled CLI library tests, all-target Clippy, formatting, public-text/reference checks and an exact-head similarity guard. Keep the frozen GitHub measurement, roster, inputs, runner and judging conditions byte-identical; these development checks produce no refreshed coverage score.
