# Native input summaries

## Goal

Make `coreav ffprobe INPUT`, `coreav ffprobe -i INPUT` and `coreav ffmpeg -i INPUT` without an output file print the native input summary on standard error. Standard output stays empty. The reference is the pinned FFmpeg 9.0.2, used only as a black-box executable.

The comparison covers every byte from `Input #0` through the final stream's metadata and side data, including format metadata, duration, start time, bitrate, chapters and programs. Containers are ISO BMFF, Matroska/WebM, MPEG-TS and the standalone WAV/FLAC probes when available. A field that cannot be represented faithfully makes the whole summary unsupported before anything is written. Unsupported inputs use the existing fallback, or exit 4 under strict-safe mode.

## Deliberate divergence

CoreAV never prints FFmpeg's version, configuration or library banner. These commands behave as though `-hide_banner` were always present, including the fallback invocation. CoreAV does not identify itself as an FFmpeg version. This is an identity rule, not a relaxed summary comparison.

## Design and ordering

1. Commit this plan before implementation. Review this commit first.
2. Build a complete summary in memory from native probe information. Keep the printer shared between the two compatibility commands. Preserve metadata ordering, stream identifiers, codec descriptions, dispositions, chapter times, program membership and side-data descriptions. Add native information only where the existing report loses a fact needed by the summary.
3. Keep summary support separate from structured section support. Validate the complete summary before writing, so an unsupported field cannot leak a partial native summary before fallback. Respect the command's log level without changing structured output.
4. Replace the no-section fallback hint after the command-line compatibility change lands. Start from main and rebase onto main after that change merges; do not edit its branch.
5. For `ffmpeg -i INPUT` without an output, print the same summary, then the pinned reference's missing-output message and exit status. Keep malformed commands and unrelated output jobs on their existing paths.

## Evidence

Generate small fixtures covering each native container, video and audio codec descriptions, unknown values, nonzero and negative starts, metadata escaping and multiline values, chapters, multiple streams and programs, dispositions and supported side data. Compare both bare ffprobe spellings and the no-output ffmpeg command against the pinned oracle with its banner hidden. Assert exact standard error, empty standard output, exit status, strict-safe native execution and fallback without partial output.

Use only the existing development partition for real-file diagnostics, with the parity tooling's explicit development identities. Never read held-out inputs or change command sets, judging rosters, divergence records, round records, bars or tolerances. Heavy or real-file runs belong on GitHub Actions; local checks use generated media only. No paid infrastructure is required.

Unit tests cover formatting and refusal boundaries. Integration tests drive the built CLI. Existing structured-output regressions remain unchanged. Run formatting, affected tests, Clippy, public-text and pull-request shape checks, and the similarity guard before handoff. An oracle disagreement is resolved from the public specification and file bytes; an undecidable case is escalated without inventing an exception.
