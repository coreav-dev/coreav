# ISO clean-aperture compatibility refusal

## Goal and scope

Do not report native ffprobe success while silently omitting clean-aperture
side data. Pinned FFmpeg 9.0.2 reports `Frame Cropping` and four crop fields
for applicable ISO visual sample entries with `clap`. The current adapter
does not establish those values.

Use the existing bounded sample-entry reader's retained `clap` presence
to refuse compatibility output before writing anything, then use the
existing fallback. ISO/IEC 14496-12 section 12.1.4 defines clean aperture.
This change does not implement its geometry or a new parser.

## Design

Record one report boolean when a retained visual sample-entry box has
type `clap`. At the ffprobe emission boundary, refuse any request for that
report before writer creation, packet/frame traversal or partial output.
Strict-safe mode returns the existing unsupported status; ordinary mode
passes the original arguments to the existing pinned ffprobe fallback.

Refuse the whole compatibility request, including count-only and
unrelated-field selections. A selection-specific geometry predicate would
duplicate an unimplemented mapping and could let an incomplete native
response through. Raw library probing and native commands remain
unchanged; commands without a retained clean-aperture box remain native.

The reader retains only the first box of each known visual-entry type.
Presence is sufficient for this conservative refusal even with duplicate,
fractional or malformed aperture payloads that were retained. Do not infer
safe geometry from payload length, equality to the coded size or zero
offsets. A larger native aperture implementation belongs in another task.

Add no source read, codec parsing, packet scan, larger budget or cap.
Keep comparator keys, known divergences, command rows, oracle pin and
baselines unchanged. Do not alter ffmpeg planning or native probe output.

## Verification

Before choosing this group, verify the main base with the feature-gated
`transcode,input-capture` library suite and all-target Clippy. Include all
explicit report fixtures when adding the boolean.

Add generated regressions that fail on main: strict-safe refusal with
empty stdout for JSON/default, name-independent selections, packets and
counts; original argument and exit-status preservation through a fallback
stand-in; raw probing and no-clap native controls.

Use short generated valid-picture controls and only real non-holdout
development input after complete size/SHA-256 validation. Compare fallback
stdout and status with the pinned binary and preserve no-clap output
against the same main binary. This is diagnostic evidence, not a full
scoreboard or held-out/judged run. Run the affected tests, feature-gated
fixtures, Clippy, formatting, public-text checks and canonical exact-head
similarity guard before readiness. Independent review and final approval
remain required.
