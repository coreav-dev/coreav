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

## Development controls and retained limitations

### Legacy comparison boundary

The legacy encoder comparison retains its fixed reference command and exact equality for unaffected requests. An exception requires a valid source container declaration and successful unscaled encoding. It verifies only the new reduced `pasp`, the exact H.264 VUI `aspect_ratio_info` bit span, and the lengths and chunk offsets those bytes force. The audited SPS parser supplies that bit boundary; no second SPS parser is introduced. All remaining SPS bits, AVC configuration, sample-entry headers, other boxes and the complete `mdat` payload remain byte-identical. Both outputs must also decode to identical complete video bytes through the qualified oracle.

The summary may differ only in `video.sample_aspect` and `output.bytes`. Each byte count must equal its actual file length, and its delta must equal exactly the structurally verified SAR signalling growth. Every other summary byte, exit code and stderr remains exact. Unsupported layouts fail closed. Oracle-free mutation tests must reject non-SAR payload, header, parameter-set, offset, summary and diagnostic changes with a named field. This is a current development gate, not a change to any registered measurement roster, frozen reference, historical result or tolerance.

The integration control uses independent 96 by 64, seven-frame inputs at 10 fps, with declared square, non-square and absent ratios. It checks the pipeline's summary and `pasp` separately from the encoder's VUI. Declared-source, unscaled outputs use the unchanged exact decoded-output comparator; absence controls verify that CoreAV does not add a declaration.

Separate before/after controls at 12 fps retain their original output clocks and complete scaled file bytes. Their seven-frame default FFmpeg reference has duration 7164 ticks, while CoreAV preserves the input's 7168 ticks at time base 1/12288. That existing timing difference is not repaired or counted here. The timestamped FFmpeg encoder worker also retains its existing square VUI default, including when the pipeline does not state a ratio or supplies another ratio through the container. The native x264 encoder follows the requested VUI. Neither existing worker behavior is changed to make this SAR fix pass.
