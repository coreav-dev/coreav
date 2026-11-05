# ISO ancillary stream bit rates

## Goal and scope

Match pinned FFmpeg 9.0.2 for the bit rate of non-fragmented ISO data and
subtitle streams in full JSON and selected stream reports. Keep reported
stream start and duration, format timing, native probing and stream-copy
behavior unchanged. Preserve codec-derived rates and fragmented sampling.

An edited data track can currently use its presentation duration as the
estimated rate's denominator. Audio and video instead use the encoded media
span, independent of edits. The same payload bytes should not be paired with
a different presentation window without evidence that the oracle does so.

Pinned data/timecode and timed-text controls establish the encoded span:
require a positive media-header duration, bounded by the summed sample duration
when that sum is positive. Zero sample duration falls back to the header;
zero header duration does not fall back to either samples or edits. Short,
long, delayed, nonzero-origin and multiple edits leave the estimate unchanged.
Whole-file estimates truncate to an integer. Missing bytes and a zero selected
duration remain unknown. Existing audio/video fallbacks and codec-derived or
fragmented rates keep their current behavior.

## Design boundary

Use the existing retained sample bytes, summed sample durations and media
header duration. Establish the data and subtitle participation rule with
generated pinned-binary controls before choosing a denominator. Vary edit
length and origin independently of the sample and header durations, including
missing or zero durations. Check whether the shorter positive sample/header
span used for audio/video also applies here, rather than assuming it does.

Keep selection in the existing compatibility rate owner. No additional reads,
sample scans, metadata parsers, budgets or codec admission. Do not change
native track rates, stream-duration selection, known divergences, comparison
keys, command rows, oracle pin, baselines or thresholds. This change does not
include the separate ISO format-output span correction.

## Verification

Commit this plan before implementation and generated-control comparison.
Before selecting the group, check current main's feature-gated CLI fixtures
and all-target Clippy. Confirm a current mismatch on an already acquired,
size/SHA-256-verified non-holdout development reproduction.

Add focused regressions that fail before the correction. Cover data and
subtitle estimates, short and long edits, header/sample disagreement, zero
and unknown timing, existing audio/video estimates, codec-rate precedence and
fragmented sampling. Compare full JSON and selected stream-rate output with
the pinned binary. Check that other stream fields, raw probing and native-copy
outputs remain unchanged, including a shared ffmpeg planning consumer.

Use generated inputs and only verified non-holdout development reproductions.
Limited diagnostics are not a refreshed scoreboard or a held-out or judged
run. Run affected CLI/ISO suites, feature fixtures, Clippy, formatting and
public-text/shape/reference checks. Require the canonical exact-head guard,
independent plan-first review and the established merge gates.
