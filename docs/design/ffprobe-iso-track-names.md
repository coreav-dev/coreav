# ISO user-data names in ffprobe

## Goal and scope

Report a track's user-data `name` atom as `stream.tags.name`, using the
existing bounded metadata snapshot. A movie-level `name` remains a format
tag and must not leak into a track or absorb a track's name. Other metadata
atom mappings, chapter names and timecode reel names are unchanged.

The user-data container is ISO/IEC 14496-12 section 8.10.1. The existing
reader already retains each child atom's type, payload and track position;
no new box parser, source read or larger budget is needed.

## Mapping

Use the atom's track position, not its track ID or the selected stream's
index. Collect names in one pass over retained items, then attach them to
the existing per-track metadata. The last `name` in a scope wins, including
an empty value. Select decoding from the original payload length before
stopping text at its first NUL. Pinned FFmpeg 9.0.2 treats payloads of at
most four bytes as Mac OS Roman, including valid UTF-8 byte sequences.
Longer payloads use UTF-8. Keep decoding in one fallible owner shared by
track and movie names.

Admit a short name only when its converted UTF-8 text fits twice the
original payload length. Generated short controls beyond that boundary
show truncated oracle text; do not imitate that truncation. For longer
payloads, admit only valid UTF-8 before NUL. Invalid sequences, including
UTF-16 BOMs, have oracle replacement behavior that differs from Rust's
lossy decoder and are unsupported rather than silently approximated.

The byte control `c3 a9 00 ff` therefore reports `√©` in either scope,
while `c3 a9 00 00 00` reports `é`. The control `ff fe 41 00 42 00`
would report two replacement characters followed by `A` in the oracle;
CoreAV refuses a requested name instead. These are pinned-binary
observations, not a claim that ISO mandates either legacy text encoding.

Do not add `name` to the generic unscoped atom mapping: that would copy
track names into format metadata. Reuse the existing tag ordering and
dictionary overwrite behavior, retaining all unrelated metadata.

The existing walk also records the item position of the movie header and
each track's media box. These two scalar positions retain names that occur
before creation/language tags without new reads or a second parser. Emit
an early name before those tags and a late name after the existing track
tags but before derived timecode.

Carry one name-support fact in the report. If a bound, malformed box or
unsupported retained name leaves names uncertain, requesting a stream or
format name refuses before writer or packet output, using the existing
fallback. This conservatively applies even if selection or a later
duplicate would hide that unsupported name.
Tag selection is case-insensitive like the writer. Requests that do not
select names keep their existing native behavior. No bound is raised.

## Verification

Commit the plan before implementation. Check scoped, duplicate, empty,
NUL-terminated, Unicode and invalid-UTF-8 controls against the pinned
binary. Add a generated CLI regression that fails before the mapping,
including multiple tracks and stream selection. Keep unrelated name-like
atoms out of the new mapping.

Verify a small existing non-holdout development reproduction only after
checking its full size and SHA-256. Compare every unrelated output field
against the main baseline. This is a limited diagnostic, not a refreshed
scoreboard or judged run. Keep the comparator, command registry, oracle
pin, caps, tolerances and baselines unchanged.

Run the affected metadata/CLI tests, targeted Clippy, formatting and public
text checks, then the canonical exact-head similarity guard. Unsupported
behavior must refuse and use the existing fallback rather than claim an
incompatible success.
