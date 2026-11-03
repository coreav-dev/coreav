# Unidentified private streams in ffprobe

## Goal

Return pinned FFmpeg 9.0.2 output or refuse and use the existing fallback,
rather than report incompatible metadata for an unidentified private
MPEG-TS stream. Keep the parser, limits, comparison rules and baselines
unchanged.

ISO/IEC 13818-1 table 2-34 assigns stream type 0x06 to PES private data,
not an audio codec. A private stream's descriptors or payload may identify
its contents. The native reader retains unidentified streams as binary
data; the pinned probe can instead expose some as MPEG audio. Relabeling
every private stream as audio would misidentify the remaining streams.

## Support boundary

Before output, refuse ffprobe compatibility requests when the existing
report identifies MPEG-TS and contains a binary-data stream whose codec
tag is the unresolved stream type 0x0006. Recognized private codecs and
explicit registration tags do not satisfy that predicate.

Own this check in the compatibility command, after its existing probe
and before creating the output writer. Return the existing Unsupported
failure so ordinary mode forwards the original arguments to ffprobe,
while strict-safe mode exits 4 with empty stdout. This conservative
boundary applies to the whole ffprobe request, including count-only
requests. The native probe API and native probe command remain unchanged.

The predicate uses only existing report entries. It performs no new
source reads, codec parsing, stream reordering or clock correction.
Matching the format and stream entries is linear in the bounded report.
It does not select a program, executable, PID or input by name.

An alternative is broader private-codec inference and program/packet
timing reconstruction. That would touch the current transport adapter
and mix independent compatibility rules. Refusal is the smaller safe
boundary until those behaviors have independently verified support.

## Verification

Commit this plan before implementation. Add a generated transport-stream
regression that fails before the check and verifies strict-safe refusal
before any output, including packet requests. Verify ordinary fallback's
arguments, output and exit status with a stand-in, and compare actual
pinned fallback output on a hash-verified non-holdout development input.
Keep recognized private-codec, registered-data and ordinary audio/video
controls native, and check that the native probe command is unchanged.

Run focused CLI/fallback tests, targeted Clippy, formatting, public-text
and issue-reference checks. Check that no file changed by the in-flight
transport-format or ISO group-count work is touched. Run the canonical
similarity guard at the published head. No held-out or judged run, new
exclusion, tolerance adjustment or baseline refresh is part of this work.
