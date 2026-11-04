# ISO ffprobe format spans

## Goal and scope

Match pinned FFmpeg 9.0.2 for the format duration printed by full JSON
format/stream reports and duration-only default or CSV requests. The format
bit rate follows from the same duration. Keep stream timing, native probe
durations, packet timestamps and native copy behavior unchanged.

The compatibility adapter currently excludes data streams from its earliest
start and latest end when audio or video exists. Existing diagnostic captures
include data/timecode tracks whose earlier start or slightly later end is
reflected in the reference format duration but not the adapter's span.
Metadata participation is a compatibility rule, not an ISO duration definition.

Pinned controls establish independent start and end rules. Every known stream
start contributes, including an ancillary stream whose end is excluded. Take
the maximum audio/video end and the maximum ancillary end separately; use the
latter only when it is less than one second after the former. At exactly one
second it is excluded. This is an aggregate decision: a far ancillary end also
prevents a nearer ancillary end from extending the audio/video bound, regardless
of track order or whether the ancillary streams are data or subtitles. Without
a known audio/video end, use the ancillary bound. Round each reported start and
duration to microseconds before adding them, preserving the current rounding.

A valid-picture control with audio/video ending at 2 seconds and timecode
ending at 2.5 seconds reports 2.5 seconds even with a stale 20-second movie
header. A 3-second timecode end instead reports 2 seconds. With audio/video
starting at 0.5 seconds, a timecode start at zero still gives a zero format
start even when its far end is excluded. Mixed subtitle/data controls and
reversed track order confirm that the cutoff is applied to the combined maximum.

## Design boundary

Use the already reported stream kinds, time bases, starts and durations in the
existing format-span owner. Establish the exact participation rule with the
pinned binary before changing it; do not assume that every ancillary stream
end contributes. Distinguish stream-derived timing from a stale or coarsely
rounded movie header with independent generated controls.

Keep the existing report format bounds for stream-copy planning and attached
picture timing. Store only the three typed format-timing values for the
ffprobe output boundary, using the same timing-value builder. Only the format
writer consumes this override, so seek origins, concat placement and stream records do not
inherit the output correction. A data stream starting before video otherwise
changes the shared report's seek origin and the bytes of a one-second copy.
The copy regression pins those bytes to the unchanged implementation.

Non-timing format fields have one owner: the completed report. In particular,
cover pictures are added after the initial format record is built and update
its stream count. A duplicate format snapshot would miss that update. The
timing override cannot carry a stream count, and emission overlays only its
three named values on the completed record. Cover-art regressions check the
emitted count against the listed streams and pinned JSON/default/CSV output.

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
