# Scoped Opus PCM decoder eligibility

## Plan

This follow-up qualifies presentation separately from RFC decoder
conformance. The existing decoder gate remains all twelve RFC 8251
vectors, mono and stereo at 48 kHz, recorded packet durations and range
states, and `opus_compare` quality strictly above 90. No threshold changes.
AAC stays ineligible. Conversion and serialization admission remain owned
by their existing exact-stage records.

The candidate scope is Opus mapping family 0, mono or stereo at the native
48 kHz rate, in supported MP4 timelines. Matroska remains ineligible in
this PR: its earlier CELT terminal waveform discrepancy must be explained,
not accepted as a numerical divergence. Mapping families 1, 2 and 255,
gain-bearing configurations, concealed packets, unsupported edits and
timestamp warnings are outside this admission.

1. Reproduce the earlier terminal waveform discrepancy with the pinned
   FFmpeg 9.0.2 binaries as black boxes. Distinguish the default decoder,
   explicit libopus decoder and container presentation from packet decode.
   Read no other decoder source. Record the cause before granting scope.
2. Generate short mono and stereo SILK, Hybrid and CELT development inputs.
   Parse packet bytes, durations, channel mapping, gain and edit clocks
   independently. Bind every result to the complete input SHA-256.
3. Require the complete MP4 output to equal the same native packet decode
   with precisely the source-specified beginning and end removed. Compare
   every F32 and S16 sample at one and eight threads. Check finite edits,
   packet-boundary ends, zero output and the existing PreSkip invariance.
   Check terminal samples explicitly; a correct total alone is insufficient.
4. Check the untrimmed complete packet sequence against the independent
   pinned libopus decode, with the existing RFC comparison tool and strict
   quality-above-90 gate. This generated-input check supplements rather
   than replaces the unchanged standard-vector gate. Never align, crop,
   add a packet or relax a count to improve a waveform result.
5. Add a deny-by-default mapping-family-0, zero-gain predicate beside the
   existing supported-MP4-timeline predicate. Unknown predicates continue
   to deny admission in consumers. Coordinate the predicate contract with
   the PCM adapter owner; this PR does not silently extend that adapter.
6. Change only the qualified MP4 Opus presentation row if all checks pass
   and review accepts this plan and its evidence. Otherwise leave the row
   false and report the exact blocker. Keep AAC and Matroska false. Add
   mutations for missing predicates, wrong mapping or gain, stale reports,
   source/count drift and terminal sample corruption.

Only generated development inputs and the existing public conformance
vectors are used. No held-out inputs, speed claims, benchmark campaign or
new performance acceptance rule is involved. Plan is committed before
implementation or qualification runs. Short local fixtures check tooling;
complete vector and qualification checks run in GitHub Actions. The PR
must be green and independently reviewed before it can grant admission.

## Failure handling

A count, order, terminal waveform or decoder-reference failure blocks the
affected scope. Do not describe it as FFmpeg numerical nonidentity until
the source presentation and independent packet decode explain it. Keep
the finite-edit FFmpeg tail divergence already documented by the MP4 fix;
the source edit, not the default oracle count, owns that presentation.

If the discrepancy requires a decoder correction beyond this scoped
qualification, retain disabled admission and report the correction as a
separate task. No source-byte repair or comparison exception is permitted.
