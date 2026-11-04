# ISO clean-aperture compatibility refusal

## Goal and scope

Do not report native ffprobe success while silently omitting clean-aperture
side data. Pinned FFmpeg 9.0.2 reports `Frame Cropping` and four crop fields
for applicable ISO visual sample entries with `clap`. The current adapter
does not establish those values.

Use the existing bounded sample-entry reader to establish aperture absence
across every retained description. Refuse compatibility output before writing
anything when `clap` is present or the visual child layout is not known clear,
then use the existing fallback. ISO/IEC 14496-12 section 12.1.4 defines clean
aperture. This change does not implement its geometry or another box parser.

## Design

Record one report boolean when any first or later retained sample description
contains `clap` or has an incomplete visual child walk. At the ffprobe emission
boundary, refuse any request for that report before writer creation,
packet/frame traversal or partial output.
Strict-safe mode returns the existing unsupported status; ordinary mode
passes the original arguments to the existing ffprobe fallback.

Refuse the whole compatibility request, including count-only and
unrelated-field selections. A selection-specific geometry predicate would
duplicate an unimplemented mapping and could let an incomplete native
response through. Raw library probing and native commands remain
unchanged; descriptions whose visual children are known clear remain native.

The reader retains only the first box of each known visual-entry type.
Presence is sufficient for this conservative refusal even with duplicate,
fractional or malformed aperture payloads that were retained. Do not infer
safe geometry from payload length, equality to the coded size or zero
offsets. A larger native aperture implementation belongs in another task.

The existing optional child walk also records completeness without changing
which metadata it retains. Zero-size, extended-size, oversized or truncated
children, trailing bytes and an exhausted `wave` depth make it incomplete.
Unknown visual child types also make the layout unproved: the pinned binary
can apply `clap` inside containers this walk does not enter, including `sinf`,
`schi`, `udta` and `ilst`. Only the already classified configuration and entry
leaf boxes, parsed Dolby Vision and ambient records, the `fiel` field-information
leaf, and `free`/`skip` padding can establish a clear layout. QuickTime File Format,
Video Sample Description Extensions, defines `fiel` as two 8-bit integers, not
a child list. An unclassified leaf can therefore cause
conservative fallback even without aperture metadata. Non-visual descriptions
do not inherit this visual-layout restriction.

Metadata retention is not an exhaustive list of known layouts. An extension-free
`d263` record is also clear: 3GPP TS 26.244 section 6.8 defines its seven-byte
decoder-specific structure. Longer records can contain optional boxes, so they
remain unproved and refuse. Neither `d263` nor `fiel` is newly retained or mapped.
The opaque `glbl` global codec header is also not a child list (Xiph's
[non-Ogg embedding description](https://wiki.xiph.org/Oggless), MOV example).
Listing it does not change native codec admission or invoke a decoder.

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

Cover used and unused later descriptions, zero-size and extended-size final
apertures, incomplete lists without a retained aperture, uninspected containers
and unknown child types. Preserve native success for multiple clear descriptions.
Check that child-walk completeness retains the same optional metadata visits,
including malformed nested `wave` lists and depth exhaustion.

Use short generated valid-picture controls and only real non-holdout
development input after complete size/SHA-256 validation. Compare fallback
stdout and status with the pinned binary and preserve no-clap output
against the same main binary. This is diagnostic evidence, not a full
scoreboard or held-out/judged run. Run the affected tests, feature-gated
fixtures, Clippy, formatting, public-text checks and canonical exact-head
similarity guard before readiness. Independent review and final approval
remain required.
