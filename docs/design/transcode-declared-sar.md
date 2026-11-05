# Preserve declared sample aspect ratios

## Scope and cause

An unscaled H.264 transcode drops an explicitly declared square sample aspect ratio. The source's `pasp` reaches the transcode pipeline as `Some((1, 1))`, but `sample_aspect` normalizes it to `None`. The encoder consequently receives no sample-aspect argument, and the output loses the declaration even when all decoded video frames agree with the reference.

Preserve a valid declared ratio during unscaled encoding, including square ratios reduced to `1:1`. An unstated or invalid source ratio stays unstated. Scaling keeps its current normalization and display-aspect calculation, including an explicit scale request that resolves to the source dimensions. No crop, filter, decoder, audio, encoder-discovery or compatibility-registration work is included.

## Design

The existing pipeline remains the sole owner of the ratio passed to `EncoderSettings`, the replacement sample entry and the summary. Distinguish an absent scale request from a present request before normalizing a square ratio; equality of input and output dimensions alone cannot make that distinction. Continue using the existing encoder arguments and sample-entry writer. Do not add an output option or change the JSON schema's types.

The ratio remains a width-to-height pixel ratio, not a display aspect ratio. H.264 VUI aspect-ratio signalling and ISO BMFF `pasp` describe that declaration. Source declaration presence must not be inferred from a computed default of square pixels.

## Verification fixed before implementation

1. Add a failing-before unit regression for an unscaled declared square ratio, including an equivalent non-reduced ratio. Retain absent, invalid and non-square controls. Explicit identity and changed-size scale requests must retain their previous results.
2. Use independent generated development inputs with declared square, non-square and absent ratios. Inspect both output VUI/sample-entry declarations and the complete transcode summary. Check both the standalone command and the audio-disabled compatibility spelling.
3. Compare complete selected stream records, rational frame times and every decoded video sample against the qualified FFmpeg 9.0.2 reference. Exercise the existing encoder paths without accepting a missing or unverified oracle as a successful skip. Existing frozen shapes and their generated inputs are not rerun or relabelled by this change.
4. Preserve every existing native admission. No prior native command may move to fallback. Existing scaled outputs must retain their complete bytes; unsupported inputs, strict-safe policy and atomic destination protection remain unchanged.
5. Run affected tests and Clippy, formatting, documentation/schema checks and the unchanged CI instruction guard. C02's command, generated input, committed baseline and 5% tolerance remain unchanged. A failure is reported rather than hidden by a new tolerance or baseline.

The historical rank-145 result motivates the fix but is not a new measurement. Its upper bound is one exact shape and three repository-shape pairs. No coverage increase is claimed without separate eligible evidence; the published result stays unchanged.

This plan is committed before code changes or new media verification. Implementation and the plan ship in one pull request. Refs: `coreav-dev/coreav#25` and [the transcode design](transcode.md).
