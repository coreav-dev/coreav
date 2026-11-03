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

## Predicate and consumer ordering

`opus_mapping0` requires actual parsed Opus configuration with mapping
family 0, one or two channels and zero signed output gain. The decoded
rate must be 48 kHz. The informational input-rate field grants nothing.
`supported_mp4_timeline` remains separately required. Both must be checked
before native output starts, while all existing packet-time refusals stay
in force. Missing, unknown or unproved requirements deny admission.

The PCM adapter in coreav-dev/coreav#346 rejects an unknown requirement
anywhere in the record, including in another codec's row. Add the consumer
predicate and activate the Opus row atomically after that adapter lands.
Keep the checked-in Opus presentation false until then. Retain a regression
that MP3 admission survives the added Opus row, plus mapping, gain, unknown
predicate and conversion-refusal regressions. This is decoder admission,
not permission to bypass an unregistered rate or channel conversion.

## Generated qualification

`scripts/opus_pcm_eligibility.py` checks 36 source-bound MP4 presentations:
SILK, Hybrid and CELT, mono and stereo, one-sample, 4,800-sample and complete
packet-boundary edits, each with original versus 3,840 informative PreSkip.
For each presentation, complete S16 and F32 output at one and eight threads
must equal the independently constructed native packet control with exactly
the source-specified interval retained. No length repair or alignment occurs.
The report binds the complete container bytes and packet payloads by SHA-256.
The command removes an old report before validation, so failure cannot leave
an earlier success as its result.

The packet control has zero delay and no discard padding. Complete S16
native output and the last 2,880 decoded samples per channel are compared
with explicit pinned libopus decoding through the existing comparison tool.
Both need quality strictly above 90, in addition to the unchanged RFC-vector
gate. All six generated cells passed locally with minimum quality 99.8 for
both windows. These are development checks, not a replacement conformance
suite or a claimed universal numerical bound.

The earlier two-second stereo low-delay CELT discrepancy reproduces against
the pinned default FFmpeg decoder: maximum F32 difference is 0.00249358 in
the Matroska terminal extension, while the first 96,000 samples per channel
differ by at most 1.19209e-7. Explicit libopus on the same bytes keeps the
complete 96,840-sample count, with maximum difference 1.53184e-5 overall and
1.52422e-5 in the extension. Both decoders remain black boxes. This isolates
the large difference to the default decoder's waveform, not lost or added
native packets. It does not establish that decoder's internal cause or waive
any conformance requirement. Matroska admission remains disabled.
