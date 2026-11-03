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
