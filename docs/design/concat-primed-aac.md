# Primed AAC presentation in native AVC concat

## Goal

Make the twelve primed-AAC development concat commands run natively with `avc1` and SPS/PPS only in `avcC`. Preserve the existing input placement and each input's presentation edits. This is compatible output, not byte-exact FFmpeg concat output. Refs coreav-dev/coreav#25 and coreav-dev/coreav#435.

## Presentation contract

Keep `Placement::At(concat_offsets(...))`. Its offsets accumulate each input's reported duration minus start time in whole microseconds, then `place()` rounds them to video ticks and holds the previous last picture where necessary. Keep that placement, the first-input origin rule and the existing per-input edit construction. Do not switch to `AfterVideo`, shift the timeline by AAC priming, or present the priming samples as FFmpeg concat does.

The stored samples remain the ordered concatenation of the inputs, with unchanged payloads. Only the existing, independently calculated extension of a non-final input's last video sample may change its duration. Each audio edit excludes its own input's priming. A newly admitted primed input must have one exactly representable skipped prefix of at least its parsed AAC frame length.

PNS decoder state does not reset at a container edit. A fresh decoder for each input is therefore not a sample-exact oracle for the continuous coded stream. The presentation oracle instead combines independently verified stored-packet identity, input-derived edit arithmetic and a complete continuous raw decode. It does not claim to reset codec state at an input boundary.

## Implementation boundaries

1. Change only the compatibility admission and decode-start scheduling in `join.rs`. Reuse the parsed, single-description AAC-LC configuration. Explicit SBR/PS and unqualified configurations remain refused. Configuration flags cannot disprove implicit SBR; do not treat a sample-rate heuristic as proof or introduce a new payload parser. Record that residual scope explicitly.
2. Admit multiple output edits only for the qualified primed class. Keep unprimed audio-gap, AC-3/E-AC-3, per-input multi-edit, differing-configuration and explicit-filter refusals. A join-local helper keeps simple-edit ordering unchanged and uses media decode order for admitted multi-edit tracks. Shared mux ordering, `place()` and the visual sample-entry/scan owner are unchanged.
3. Reproduce the direct-native FLAC non-integral-clock lead and check the mapping specification. Preserve native packet-copy admission with exact statistics where available and specification-defined unknown values otherwise. Block bounds have no unknown encoding, so any retained declared bounds and internal-short-block limitation must remain explicit. Keep the accurate combined statistics of the already qualified cases. Do not rewrite FLAC frame headers or checksums, or hide source, integrity or resource errors.
4. Put the new CLI qualification in `ffmpeg_concat_primed.rs` and `support/concat_presentation.rs`. Adapt the existing primed-refusal development cases without adding an unregistered module to the frozen harness graph. Qualify the four naturally admitted rate, mixed-video-only, later-IDR and AUD controls separately from the requested twelve. Keep the existing comparator and video-metadata targets untouched.

## Independent verification

- Require strict-safe native success, `avc1`, identical input/output `avcC`, and an independent NAL walk with no in-sample parameter sets. Check every stored packet, hash, size, flags, duration and ordering, including the exact permitted video-tail extension.
- Independently read input/output `elst`, `mdhd`, `stts` and `ctts`. Derive expected placement, media bases, presentation windows and empty gaps from the input records rather than the writer's summary. Require exact movie-clock representation and exclusion of at least one full AAC frame per admitted audio input.
- Decode complete output with the checksum-verified FFmpeg 9.0.2 oracle and `-ignore_editlist 1`. Establish that no implicit leading skip changes the raw sample origin. Apply the independently derived edit windows to the complete decoded media, with exact presented sample counts and offsets. Compare all video pixels and presentation times with the independently decoded inputs. Observe the default edit-honoring reader separately without substituting FFmpeg concat's clocks for the chosen contract.
- Compare complete stream and format records with a closed list of calculated metadata/layout differences. Keep mutations for changed payloads, priming, edit windows, gaps, durations and metadata. No arbitrary field dropping or new numerical tolerance is allowed. An exactness failure stops qualification and is reported.
- Retain the existing native controls and atomic refusal/fallback regressions. Run focused writer and CLI tests, the canonical workspace suite, formatting, Clippy, source-provenance checks and final-head similarity verification.

## Gates and sequencing

This plan is the first commit of one implementation PR and precedes media runs. Do not open that PR until coreav-dev/coreav#436 has merged. Integrate current main and rerun the complete comparator and C02 on that integrated result first.

C02's command, inputs, generator, representative and `bench/guard/baseline.json` remain unchanged. Its instruction count must stay within the existing 5% tolerance of 1,121,797; stop and report if it does not. No timing claim or held-out measurement is planned. Compatible-category registration waits until coreav-dev/coreav#433 merges and is separate work.
