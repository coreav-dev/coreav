# ISO ffprobe format spans

## Goal and scope

Match pinned FFmpeg 9.0.2 for the format duration printed by full JSON
format/stream reports and duration-only default or CSV requests. The format
bit rate follows from the same duration. Keep stream values, native probe
durations, packet timestamps and native copy behavior unchanged.

The compatibility adapter currently excludes data streams from its earliest
start and latest end when audio or video exists. Existing diagnostic captures
include data/timecode tracks whose earlier start or slightly later end is
reflected in the reference format duration but not the adapter's span.
Metadata participation is a compatibility rule, not an ISO duration definition.

## Design boundary

Use the already reported stream kinds, time bases, starts and durations in the
existing format-span owner. Establish the exact participation rule with the
pinned binary before changing it; do not assume that every ancillary stream
end contributes. Distinguish stream-derived timing from a stale or coarsely
rounded movie header with independent generated controls.

The change must not add source reads, sample scans, metadata parsing, budgets
or codec admission. Do not change stream duration selection, subtitle codec
classification, fragment-origin behavior or comparison policy. Keep the oracle
pin, command rows, known divergences, baselines and thresholds unchanged.

## Verification

Before choosing the group, check current main's feature-gated CLI library
fixtures and all-target Clippy. Commit this plan before implementation.

Generate valid-picture controls that vary ancillary data/timecode starts and
ends independently of audio/video and the movie header. Include near and far
ends, a one-second boundary, equal ends, missing timestamps, different time
bases, audio/video-only and data-only controls. Confirm the pinned rule rather
than applying a filename, codec-name or observed-value exception.

Add focused regressions that fail before the change. Compare complete selected
duration output and exit status for JSON, default and CSV writers, and check
the dependent format bit rate. Keep stream values and raw/native-copy results
identical on controls. Use only generated inputs and size/SHA-256-verified
non-holdout development reproductions; limited diagnostics are not a refreshed
scoreboard or a held-out or judged run.

Run affected CLI/ISO tests, feature-gated fixtures, Clippy, formatting, public
text checks and the canonical exact-head similarity guard. Independent review
and final approval remain required.
