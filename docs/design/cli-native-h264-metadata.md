# Native H.264 constant frame rates and sample aspects

## Scope

Extend the explicit MP4 encoder preference from coreav-dev/coreav#509 using the checked timing and sample-aspect types from coreav-dev/coreav#511. Admit supported positive constant rational frame rates and declared non-square sample aspects. Keep the default build, release feature sets, unflagged commands, original fallback arguments, command deadline, geometry, pixel-format and color exclusions unchanged. Matroska output remains separate work.

## Exact presentation clock

Previously eligible 30 fps square-pixel commands retain their former timing predicate over the shared presentation ledger, including its existing terminal-duration inference and single-picture one-tick default. The selected source-rate hint must be absent or exactly 30 fps, and normalized intervals and the ledger's final duration must still match 30 fps exactly. This preserves old native commands without applying their defaults to new rates or non-square aspects.

For widened admission, the native path derives its rate from the presented packet clock, not from the external encoder path's snapped average or a nominal metadata rate. Require positive identical presentation intervals and the same final duration, then reduce the exact rational clock with checked arithmetic and validate it with `FrameRate::new`. A one-picture source needs an explicit positive duration. Missing final duration on a longer stream may use the preceding constant interval, but an explicitly zero or different final duration is not admitted by this new branch.

This admits every supported rational cadence represented exactly by the source clock, including NTSC ratios. Nonuniform intervals, including rounded alternating timestamp steps, retain the existing path. A tolerance cannot distinguish those observable timestamps from genuine VFR. No floating-point admission or timestamp rewrite is added. The decoded-frame boundary rechecks the admitted exact interval.

Only the native branch uses the derived rate. Other encoders retain their existing rate selection and snapping. MP4 sample timestamps and the final duration continue to come from the existing presentation ledger.

The exact source-clock interval must also fit the positive 32-bit MP4 sample-duration field. A reducible rate does not make an oversized stored interval representable. Such inputs retain the existing path rather than rewriting the source clock or replacing the final duration with a guessed tick.

## Declared sample aspect

Validate the original source ratio before the shared aspect helper can narrow its terms. Reduce positive ratios exactly and use `SampleAspectRatio::new` to enforce the library's represented bounds. Absent, zero-term and square ratios retain the default, preserving previously eligible native output. Shared normalization already treats declarations such as `(0, 1)` or `(1, 0)` as unstated. Unsupported ratios or an unrepresentable MP4 display size cause native ineligibility and the unchanged fallback.

The reduced declared ratio feeds the encoder settings and the existing MP4 `pasp` and display-size paths, so container and VUI signalling agree. The encoder library and muxer APIs do not change.

## Verification

- Unit cases cover integer, NTSC and unusual rational clocks, nonzero origins, one-picture durations, nonuniform or duplicate timestamps, incorrect final durations and overflow.
- Actual-base native comparisons retain single-picture zero duration and multi-picture zero terminal duration at 30 fps. Non-30 and non-square controls cannot inherit those defaults; positive-duration non-square single-picture input exercises widened admission.
- Generated MP4 cases use pinned FFmpeg 9.0.2 to require exact reported `r_frame_rate`, `avg_frame_rate`, SAR and DAR, with independent SPS and `pasp` checks.
- Fresh same-head reconstruction verifies native pictures. Complete output bytes remain identical across thread choices at fractional rates and non-square aspects.
- VFR, rounded timestamp cadence and unsupported aspects match the flag-off fallback. Previously eligible absent, unspecified and square aspects retain their native bytes. Default feature graphs and unflagged output comparisons remain unchanged.
