# ISO track-reference group counts

## Scope

Report the number of timecode track-reference stream groups in the ISO base
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
reference types and target IDs. Compute one optional count in `IsoMeta::read`
from that snapshot and pass it to the ISO format builder. Do not
infer groups from a timecode tag or decoded sample: a relationship can
exist without readable timecode sample data, and unsupported tracks
still appear in the probe's metadata.

Retaining a raw relationship graph in the report or rescanning the
source in the format builder would expose more state, duplicate work
and move metadata interpretation out of its existing owner. Reject
those alternatives in favor of the single derived scalar.

## Relationship policy

Generated controls distinguish reference kinds, repeated references,
several target IDs, sharing, missing IDs, ordering and unsupported track
descriptions. Collect the IDs whose first sample-entry kind is `tmcd`,
then remove each ID when a retained `tmcd` reference names it. Count only
successful removals: shared targets form one group, several resolved
targets form several groups, and unrelated references form none.
The target handler and timecode timing fields do not determine grouping.
Do not filter through selected or supported streams or infer IDs from
stream indexes. The lookup is O(tracks + reference IDs), and its size
is bounded by the retained target tracks.

An incomplete snapshot cannot establish a count, including known zero.
Retain `None` internally, but refuse before any output when that count
is requested. The pinned binary reports 1 for the over-bound fixture,
so printing `N/A` would be an incompatible success. The existing fallback
runs ffprobe with the original arguments; strict-safe mode instead
refuses with exit status 4 and empty stdout. A request that does not
select the count retains its existing native behavior.
The reader already skips a `tref` payload over 4096 bytes; correct its
completeness flag on that existing skip rather than rereading it or
raising the limit. Other partial-data and I/O-error behavior remains
unchanged. Add no parser, source read, new budget or decoder operation.

## Verification

First demonstrate the hard-coded-zero failure through the CLI with a
generated linked-target case. Exercise sharing and duplication so an
implementation that counts links cannot pass. Check noncontiguous
track IDs, target-before-source ordering, missing and unrelated targets,
unlinked timecode tracks and stream selection. Check both sides of the
existing reference-size bound so skipped links never become known zero.
Test strict-safe refusal and fallback before output, including mixed
packet/count requests, and keep other selected fields native.
Confirm that the
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
