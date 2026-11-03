# Transport MP2 sample formats

## Scope

Match the sample format reported by pinned FFmpeg 9.0.2 for Layer II
MPEG audio in transport streams. This is an output-format observation:
ISO/IEC 11172-3 and 13818-3 describe the coded samples, not whether a
decoder exposes integer or floating-point planes.

The current adapter chooses the format from the resolved codec name
alone. Generated inputs with identical complete MP2 frames distinguish
initialization through the first Program Map Table from a later program,
and distinguish MPEG-1 Audio and MPEG-2 Audio declarations
(ISO/IEC 13818-1 table 2-34). A blanket change for every MP2 stream would
break the existing floating-point cases.

## Plan

Use only the existing stream declaration and first-table provenance,
with the resolved Layer II codec as the guard. The pinned binary reports
`s16p` for a type 0x04 declaration or a stream outside the first completed
PMT after the PAT, and retains `fltp` for type 0x03 in that first table.
Generated controls preserve this distinction with several streams in the
first table and a shared PID in a later program. Actual Layer III stays
`fltp` under both declarations and both program positions.
The MP2 codec-name fallback alone is not a parsed layer: require the
audio header's Layer II detail before overriding the default format.
With no PES payload or no readable audio header, both declarations and
both program positions retain `fltp`. The existing fallback codec name
is unchanged.
Keep the choice in the transport ffprobe adapter rather than changing
the shared codec-name defaults. The parser already exposes these facts;
add no parser, read, decode operation, configuration field or retained
state for this output choice.

Keep codec identity, sample rate, channels, bit rate, packet listing,
program membership and timing unchanged. In particular, do not imitate
an audio codec guessed for non-audio VBI data, alter downmix metadata or
expand any comparison predicate to make neighboring keys pass.

## Verification

The regression constructs complete silent MPEG-1 frames at 48 kHz with
zero bit allocations: Layer II at 192 kbit/s, and Layer III at 128 kbit/s
with empty side information. The pinned binary confirms the expected
flat and nested fields and decodes every audio stream without errors.
The 32 cases combine both layers, both declarations for each of two
streams in the first table, both declarations for a later stream, and
a PES before or after the later PMT. A PID shared by both programs keeps
its original format. Declaration changes preserve the elementary frames
so that payload corruption cannot explain the format difference.
Eight additional cases cover absent PES data and unreadable payloads
under both declarations in initial and later programs. These require
`fltp` in flat and nested output, matching the pinned binary.

Check the identified development multiplex only after verifying its
non-holdout flag and full SHA-256. Compare every actual MP2 stream's flat
and nested format, and verify that unrelated fields are unchanged.
Keep the existing comparator, known-divergence registry, command rows
and thresholds untouched. These checks are development diagnostics,
not a held-out or judged result.

Run neighboring ffprobe tests, transport tests, targeted all-target
Clippy, formatting and public-text checks. Run the canonical similarity
guard at the exact PR head before reporting readiness.
