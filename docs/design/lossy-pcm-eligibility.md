# Lossy decoder eligibility for PCM output

## Plan

The native PCM-output command may use lossy decoders that pass their
standard's conformance criteria. This does not promise byte identity with
FFmpeg 9.0.2. The command and its conversion adapters are separate work.

1. Audit the existing MP3, Opus and AAC-LC conformance tests and CI runs.
   Preserve every existing acceptance threshold. Distinguish the tested
   MPEG-1 Layer III scope from MPEG-2 Layer III conformance.
2. Agree a machine-readable, deny-by-default decoder record with the
   command owner before implementing it. Register only scopes supported
   by passing conformance evidence and correct presentation lengths.
3. Document measured FFmpeg differences separately from conformance.
   Numerical differences do not excuse incorrect pre-skip, end trimming,
   sample counts or drain behavior. Investigate Opus presentation before
   enabling it.
4. Keep the existing audio-conformance jobs as the acceptance owner. Add
   missing coverage and focused regressions where the audit finds gaps.
   Validate registry admission, missing evidence and out-of-scope inputs.

Only public conformance vectors and generated development inputs are used.
No other decoder implementation's source is read. Unit and short fixture
checks run locally; complete conformance runs run in CI. No held-out or
judged measurement inputs are opened by this work.

## Registration contract

[`audio-decoder-eligibility.json`](../compat/audio-decoder-eligibility.json)
is a decoder admission record, not an output-equality claim. Its schema is
`coreav.audio-decoder-eligibility/1`. The command owner reads it at build
time. Missing or unknown schema, codec, kind, container or requirement
denies admission. Both `decoder_eligible` and the selected presentation's
`eligible` must be true. Every requirement must be recognized and checked.
Duplicate decoder or container rows are invalid, not first-match wins.
`evidence` is a repository-relative documentation link. `limitations` is
informational and cannot grant permission.

`kind` is either `exact` or `conformance`. These lossy rows use only
`conformance`. Mixing, rate conversion and final serialization require
their own registrations and cannot inherit decoder eligibility. The
record alone changes no command: until the consuming adapter lands, lossy
PCM output remains on fallback.

The initial requirements are deliberately conservative:

- `mpeg1_layer3`: parse and validate Layer III frame headers, requiring
  MPEG-1 throughout decoding. Container rate metadata alone is not proof.
  A later version change is an error, never permission to retry fallback
  after writing output.
- `aac_lc`: an AAC-enabled build successfully preflights the selected
  track's actual AudioSpecificConfig as supported AAC-LC (AOT 2). Codec
  names alone do not prove the profile. Existing configuration and packet
  refusals remain in force.
- `supported_mp4_timeline`: the native MP4 presentation planner accepts
  the selected track without ignored edits or other warnings, and its
  selected packets have equal PTS and DTS. Unsupported edits, nonzero
  composition offsets, damaged or concealed packets are not admitted.
  The adapter must preserve that planner's complete sample sequence and
  exact count, including empty edits and terminal trimming.

Unknown predicates fail closed. A consumer may refuse a narrower scope.
Neither Matroska presentation nor Opus presentation is enabled initially.
This does not remove support from the separate `decode-audio` command.

## Conformance gates

All three decoder runs already exist in
[`audio-conformance.yml`](../../.github/workflows/audio-conformance.yml).
Changing this record starts those jobs. A registration is mergeable only
when its conformance job passes at the proposed head; a stored boolean is
not a substitute for that run. No existing threshold is relaxed.

### MP3

ISO/IEC 11172-4 Layer III full accuracy is RMS error strictly below
2^-15 / sqrt(12), with maximum absolute error at most 2^-14 of full scale.
The `mp3` job runs `compliance_bitstreams` against the checksum-pinned
Fraunhofer IIS v2.2 package: all eleven streams decode without error and
`compl.bit` is compared with `compl.hex`. Its deliberately incomplete last
frame is handled by the existing test's explicit reference-length rule.
The comparison uses decoded floats, before S16 quantization.

This is MPEG-1 evidence, not MPEG-2 conformance. ISO/IEC 13818-4 extends
testing to MPEG-2 audio. Its
[additional audio test sequences](https://www.iso.org/standard/40094.html)
include Layer III low-rate sweeps and stereo tools. Those sequences are
not run by the current job. MPEG-2 and the non-ISO MPEG-2.5 extension
remain ineligible until separate evidence is registered. Passing a
generated low-rate encode would not establish that conformance.

### Opus

[RFC 6716 section 6.1](https://www.rfc-editor.org/rfc/rfc6716#section-6.1)
defines decoder testing, updated by
[RFC 8251 section 11](https://www.rfc-editor.org/rfc/rfc8251#section-11).
The `opus` job runs all twelve updated vectors at 48 kHz, in mono and
stereo, against the same `.dec` reference set. Packet durations must match
reference lengths, nontrivial packets must end with the recorded range
state, and every output must pass the black-box `opus_compare` with
quality strictly above 90. The existing extra SILK stereo sample-equality
assertion remains. The archive and comparison-tool release are checksum
pinned. No reference implementation source is read.

Vector conformance does not test container pre-skip or end trimming. The
measured two-second MP4 outputs have exactly 96,000 samples per channel;
the oracle emits 648 more for SILK and Hybrid, and 840 more for the
low-delay CELT encode. A difference from the oracle alone does not prove
a CoreAV correctness bug: the source's finite edit owns presentation.
The earlier diagnostic did not retain a source-bound presentation proof
for each of these inputs. The existing
`opus_priming_samples_are_never_presented` fixture checks pre-skip and
finite-edit counts independently of the oracle. `Stream::finish` closes
the output without inventing an extra Opus packet.

The Matroska CELT terminal waveform discrepancy is not explained by that
MP4 edit. Neither it nor other presentation shapes are waived here.
Opus's decoder is conformance-qualified, but both presentation rows stay
false pending a complete mode-specific presentation and terminal-output
check. Any mismatch with the stream's specified sample count is a
correctness defect, not a known-divergence allowance.

### AAC-LC

ISO/IEC 14496-4 defines MPEG-4 conformance testing; its audio conformance
material is published separately as ISO/IEC 14496-26. The `aac` job uses
the public 2010 package, with each file SHA-256 pinned in
`crates/coreav-aac/tests/conformance.sha256`. It runs
`conformance_streams`: all 336 LC streams, both frame lengths and every
provided rate, against their reference waveforms, with 105 HE-AAC streams
refused by name.

The repository's existing full-accuracy gate is per-channel RMS at most
2^-15 / sqrt(12) and maximum absolute error at most 2^-14. The existing
reference alignment (within 4,096 samples), channel matching and clipping
to reference range remain unchanged. Noise and all-MS intensity blocks
retain the separately documented aggregate-energy gate of at most 0.1 dB
when there are at least 100 blocks. That energy threshold is a project
gate, not a claim that the standard mandates this particular number or
the oracle's random waveform. Fewer blocks are reported, not silently
treated as waveform agreement. See the complete
[AAC-LC bars](audio-decode.md#aac-lc-bars) for the comparator's scope.

The existing additional FFmpeg agreement, exact presentation-length and
thread-invariance gates remain required. The reference-alignment search
does not grant the PCM command permission to shift or truncate samples.

## Existing passing evidence

The public [conformance run](https://github.com/coreav-dev/coreav/actions/runs/41602813739)
at `3316fa3895075ebe61d0d0204d0d45eeaa3840e5` passed all three jobs:
MP3 RMS 5.365e-8 and maximum 4.172e-7; Opus all 24 comparisons passed
(minimum printed quality 96.2); AAC 336 LC decodes, 105 named HE-AAC
refusals and zero failures. This is prior evidence, not a claim that a
future source revision has passed. The PR's own CI must run again.
