# Matroska audio-copy clock qualification

Plan committed before new generated comparisons or implementation. Refs coreav-dev/coreav#25.

## Scope

Qualify compatibility copying of one selected AAC audio stream from Matroska to MP4 or M4A without decoding or re-encoding it. A retained development example copied identical packet payloads but changed priming and duration against pinned FFmpeg 9.0.2. Inspect the existing demuxer and writer clock owners; do not infer packet times from report strings or change native remux's duration-reconstruction policy.

Prefer a compatibility-specific exact clock path only if existing typed source facts establish its full presentation. Otherwise refuse the unqualified composition before staging and preserve original-argument fallback. Unknown or unsupported timing must not become native success. I/O, work, retained-memory and arithmetic failures retain their terminal classifications. Do not widen limits or add an AAC decoder qualification.

## Verification

Before admitting any subset, compare a generated 32 kHz AAC-LC control with zero block origin, a full final frame and no discard padding. Its 1,024 samples occupy exactly 32 milliseconds, so both transport and sample clocks can express every endpoint without rounding. A source-side alignment argument alone does not establish compatibility.

Keep any initial admitted subset audio-only and require a supported explicit AAC configuration, an integral frame duration in both source clocks, exactly aligned delay, contiguous full-frame timestamps, matching declared durations and an exact presentation endpoint in the movie clock. Unknown timestamps, lacing, codec-state changes, invisible packets, block additions and any nonzero discard padding remain unqualified. Accumulate these facts in the existing bounded packet walk; complete it before classifying a timing refusal so a later I/O or resource error cannot become fallback. Keep qualification and writing tied to the same preparation, or repeat the same owner's guard in the writing pass, without changing native remux timing.

Use independent generated short mono/stereo AAC examples at 44.1 and 48 kHz, with source origins and final packet boundaries varied. Compare complete packet payloads, PTS/DTS/durations and declared priming, edits and duration against the checksum-verified pin. Distinguish selected audio from unselected video or other tracks, and keep already-qualified ISO copy behavior unchanged.

For every refused composition, require strict-safe exit 4, zero stdout, unchanged absent/existing destinations and no staging residue. Normal mode must preserve the original command and match the pin's complete output and exit status. Retain native remux controls, source I/O/limit tests and relevant compatibility regressions. No frozen measurements, commands, thresholds, judging rules or coverage counts change. A refusal fix is not new native coverage.
