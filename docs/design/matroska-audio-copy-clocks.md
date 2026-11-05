# Matroska audio-copy clock qualification

Plan committed before new generated comparisons or implementation. Refs coreav-dev/coreav#25.

## Scope

Qualify compatibility copying of one selected AAC audio stream from Matroska to MP4 or M4A without decoding or re-encoding it. A retained development example copied identical packet payloads but changed priming and duration against pinned FFmpeg 9.0.2. Inspect the existing demuxer and writer clock owners; do not infer packet times from report strings or change native remux's duration-reconstruction policy.

Prefer a compatibility-specific exact clock path only if existing typed source facts establish its full presentation. Otherwise refuse the unqualified composition before staging and preserve original-argument fallback. Unknown or unsupported timing must not become native success. I/O, work, retained-memory and arithmetic failures retain their terminal classifications. Do not widen limits or add an AAC decoder qualification.

## Verification

Use independent generated short mono/stereo AAC examples at 44.1 and 48 kHz, with source origins and final packet boundaries varied. Compare complete packet payloads, PTS/DTS/durations and declared priming, edits and duration against the checksum-verified pin. Distinguish selected audio from unselected video or other tracks, and keep already-qualified ISO copy behavior unchanged.

For every refused composition, require strict-safe exit 4, zero stdout, unchanged absent/existing destinations and no staging residue. Normal mode must preserve the original command and match the pin's complete output and exit status. Retain native remux controls, source I/O/limit tests and relevant compatibility regressions. No frozen measurements, commands, thresholds, judging rules or coverage counts change. A refusal fix is not new native coverage.
