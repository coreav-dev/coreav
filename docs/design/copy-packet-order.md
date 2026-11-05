# Copied packet order

## Goal and scope

Make ordinary ISO video stream copy agree with pinned FFmpeg 9.0.2 at the
packet level, including cross-stream packet order, payload hashes and exact
rational PTS, DTS and durations. The existing writer keeps per-stream samples
but groups them into approximately one-second chunks. That storage policy
can change the order in which a demuxer returns packets from several streams.

Investigate ordinary MP4 copy first, including faststart, explicit audio/video
copy codecs and input-zero stream maps. Use independently generated controls
with distinct video and audio, reordered maps, B-frames, nonzero edit origins
and unequal sample clocks. Determine the pinned binary's ordering and tie
behavior from its outputs, not another implementation's source.

## Boundary

Keep native remux, concat, seeking, audio-only copying and unrelated container
paths unchanged. Do not change copied payloads, per-stream clocks, stream
selection, output metadata or the existing comparison predicate to make a
packet-order difference disappear. If exact behavior cannot be established,
refuse before publication and retain the original-argument fallback.

Reuse the existing two-pass writer and its authoritative output tables.
Any compatibility-only scheduling must drive both chunk offsets in the
header and payload copying from the same policy, include edit-derived decode
origins, use exact rational comparisons, and remain under the existing table
and sample-work budgets. Add no media-sized packet buffer or second parser.
Settle the smallest representation after the pinned controls establish the
root and update this note to describe that decision.

## Verification

Commit this plan before implementation and oracle controls. Demonstrate the
current mismatch, then add a focused regression that observes the complete
ordered packet sequence rather than sorting or grouping it by stream. Assert
payload hashes, counts and exact rational timing independently of container
sizes. Check decoded output and adjacent stream/format metadata as well.

Exercise default and explicit maps, audio-first maps, unequal clocks, ties,
B-frames, single-stream controls, faststart and atomic failure/fallback.
Preserve native remux bytes and existing unrelated tests. Run the affected
CLI and writer suites, formatting, Clippy, public-text and shape checks, and
the canonical similarity guard at the final public head. This is a generated
development reproduction, not a rerun or update of frozen coverage results.

Refs coreav-dev/coreav#25.
