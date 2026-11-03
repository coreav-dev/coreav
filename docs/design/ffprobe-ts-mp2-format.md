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
with the resolved Layer II codec as the guard, if the independent
controls confirm that these facts distinguish the observed formats.
Keep the choice in the transport ffprobe adapter rather than changing
the shared codec-name defaults. The parser already exposes these facts;
add no parser, read, decode operation, configuration field or retained
state for this output choice.

Keep codec identity, sample rate, channels, bit rate, packet listing,
program membership and timing unchanged. In particular, do not imitate
an audio codec guessed for non-audio VBI data, alter downmix metadata or
expand any comparison predicate to make neighboring keys pass.

## Verification

Construct complete silent MP2 frames from their specified header and
zero bit allocations. Check the generated inputs with the pinned binary
before fixing the adapter, including a failing integer-format case and
an unchanged floating-point case. Cover several streams in the first
table, later tables, shared PIDs, both MPEG-audio declarations, and an
actual Layer III negative control. Vary declarations while preserving
the elementary frames so that payload corruption cannot explain the
format difference.

Check the identified development multiplex only after verifying its
non-holdout flag and full SHA-256. Compare every actual MP2 stream's flat
and nested format, and verify that unrelated fields are unchanged.
Keep the existing comparator, known-divergence registry, command rows
and thresholds untouched. These checks are development diagnostics,
not a held-out or judged result.

Run neighboring ffprobe tests, transport tests, targeted all-target
Clippy, formatting and public-text checks. Run the canonical similarity
guard at the exact PR head before reporting readiness.
