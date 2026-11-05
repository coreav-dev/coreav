# Keep copied-video AAC encoding on exact fallback

## Scope

Commands that copy video while encoding AAC already refuse the combined stream set. The current AAC encoder runs an external FFmpeg program, which strict-safe mode forbids. Exposing encoded-audio replacement data would not supply a native encoder or establish exact decoding, encoder delay, packet timing, or container output.

Keep that admission boundary. This change adds regression coverage, not new native coverage. Do not add a replacement API, invoke an oracle inside strict-safe mode, or treat copied video as proof that the encoded audio matches.

## Contract

For selected video beside AAC encoding, strict-safe mode must exit 4 with empty stdout. It must preserve an absent or existing destination and leave neighboring files unchanged. Normal mode must retain the original arguments, including input order, stream maps, codec selectors and the legacy `-strict -2` option, through the existing fallback.

The fallback reference is the pinned FFmpeg 9.0.2 program. Compare its successful exit, stdout and complete output bytes. Do not discard packet hashes, timing, encoder delay, metadata or container layout from the comparison. Exact argument replay also retains the existing `-hide_banner` prefix and propagates the child exit status.

Build paired reference and fallback arguments from `scripts/oracle-audio-args.txt` before every encoded input. Also retain strict-safe controls without those oracle-only options, so an unsupported thread option cannot hide accidental admission of the copied-video AAC command itself.

## Verification

- Generate independent H.264/AAC inputs with distinct dimensions, audio rates and tones. Include B-frames, AAC priming and a delayed audio edit.
- Cover one input, two automatically selected inputs, explicit maps across inputs, input-zero maps, codec overrides, indexed codec selectors and faststart.
- Check both absent and existing destinations, directory entries and a neighboring sentinel. On Unix, record the complete fallback argument vector and prove strict-safe mode never invokes that recorder.
- Run the regression with the default CLI and the AAC-enabled transcode build, plus the affected CLI target, Clippy, formatting, pin, public-text and shape checks. Obtain an exact-head canonical similarity result before readiness.

This work changes no frozen commands, judging rules, baselines, limits, tolerances or coverage records. Seeking, concat, audio-only encoding, native AAC encoding and MPEG-TS output remain outside this change.
