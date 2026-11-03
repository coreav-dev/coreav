# ISO track-reference group counts

## Scope

Report the number of track-reference stream groups in the ISO base
media adapter's existing `format.nb_stream_groups` field. It is currently
always zero. Generated linked, independently linked, shared, unlinked
and missing-target timecode relationships produce 1, 2, 1, 0 and 0 with
pinned FFmpeg 9.0.2.

This is a count-only feature. Keep `-show_stream_groups` refused, do not
add group documents or change command classification, and leave other
containers, stream selection, tags, timing and packet output alone.
Other group families requiring new parsers are outside this change.

## Inputs and ownership

ISO/IEC 14496-12 section 8.3.3 and Apple's
[track references](https://developer.apple.com/documentation/quicktime-file-format/track_references)
and [track reference type atom](https://developer.apple.com/documentation/quicktime-file-format/track_reference_type_atom)
define links by reference type and target track ID, including many-to-many
relationships. The stream-group count itself is a pinned-probe output
convention, not another box field.

The existing bounded metadata reader already retains the track IDs,
reference types and target IDs. Compute one scalar in `IsoMeta::read`
from that snapshot and pass it to the ISO format builder. Do not
infer groups from a timecode tag or decoded sample: a relationship can
exist without readable timecode sample data, and unsupported tracks
still appear in the probe's metadata.

Retaining a raw relationship graph in the report or rescanning the
source in the format builder would expose more state, duplicate work
and move metadata interpretation out of its existing owner. Reject
those alternatives in favor of the single derived scalar.

## Relationship policy

Before implementation, vary reference types, repeated references,
several target IDs, shared targets, missing IDs, target ordering and
unsupported track descriptions with generated inputs. Establish which
reference kinds the pinned binary groups and whether identity depends
on the target alone or also on the reference kind. Count resolved
groups, not references, source tracks or standalone timecode entries.

Use a bounded lookup over the already retained IDs rather than a
quadratic cross-product of tracks and references. Preserve the metadata
reader's current partial-data, limit and I/O-error behavior. Add no
parser, source read, new budget or decoder operation.

## Verification

First demonstrate the hard-coded-zero failure through the CLI with a
generated linked-target case. Exercise sharing and duplication so an
implementation that counts links cannot pass. Check noncontiguous
track IDs, target-before-source ordering, missing and unrelated targets,
unlinked timecode tracks and stream selection. Confirm that the
unsupported group-listing flag is still refused before file access.

Check the selected development reproduction only after revalidating
its explicit non-holdout flag, full SHA-256 and size. Compare the final
binary with pinned 9.0.2, and confirm that no unrelated output field
changes. Do not claim a new full scoreboard or a judged result.

Run the focused ISO ffprobe and neighboring metadata/limit tests,
targeted all-target Clippy, formatting and public-text checks. Run the
canonical similarity guard at the exact PR head. Keep the comparator,
known-divergence registry, oracle pin, command rows and thresholds
unchanged, and do not access held-out or judged inputs.
