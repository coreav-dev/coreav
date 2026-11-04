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
an empty value. Text stops at its first NUL. Pinned FFmpeg 9.0.2 preserves
valid UTF-8 and uses Mac OS Roman when the bytes are not valid UTF-8.
Keep this decoding in one owner shared by track and movie names.

Do not add `name` to the generic unscoped atom mapping: that would copy
track names into format metadata. Reuse the existing tag ordering and
dictionary overwrite behavior, retaining all unrelated metadata.

The existing walk also records the item position of the movie header and
each track's media box. These two scalar positions retain names that occur
before creation/language tags without new reads or a second parser. Emit
an early name before those tags and a late name after the existing track
tags but before derived timecode.

Retain the existing metadata-completeness fact in the report. If a bound
or malformed box leaves names uncertain, requesting a stream or format
name refuses before writer or packet output, using the existing fallback.
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
