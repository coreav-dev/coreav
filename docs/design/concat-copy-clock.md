# Copied AAC concat packet clocks

## Goal and boundary

Implement an exact compatibility concat clock for supported ISO video/AAC inputs instead of routing their packet stream through native gapless presentation. Keep native `join_mp4`, `Placement::AfterVideo`, `Placement::At`, ordinary copy, parser budgets and output ownership unchanged. Unproved compositions retain strict-safe refusal or fallback with the original command. This is stream copy, not AAC decoder eligibility or a tolerance contract.

The existing [positive AAC edit](concat-aac-edits.md) and [audio gap](concat-audio-gaps.md) notes record why native presentation is insufficient. An edit can trim priming or express silence but cannot make one packet span a gap or assign successive one-tick decode times at an overlapping boundary. A compatibility packet clock belongs beside the join writer's bounded sample listings, not in a second CLI packet reader. Existing native placements remain available; a distinct concat policy carries each file's slot and reported start rather than changing their meaning.

## Sources and hypotheses

ISO/IEC 14496-12 Sections 8.6.1.2, 8.6.1.3 and 8.6.6 define decode deltas, composition offsets and edit mappings; Sections 8.4.2.3, 8.3.2.3 and 8.2.2.3 define the duration fields. ISO/IEC 14496-1 Section 7.2.6 defines decoder configuration descriptors, and ISO/IEC 14496-3 defines AAC configuration and frame length. No decoder is added. The public [FFmpeg concat demuxer documentation](https://ffmpeg.org/ffmpeg-formats.html#concat) says timestamps are adjusted globally, each file starts after the preceding file's duration, and unequal stream lengths can leave gaps. MP4 edit-list and negative-timestamp options are documented on the same page and in [format options](https://ffmpeg.org/ffmpeg-formats.html#Format-Options).

The following mapping is a hypothesis to test with checksum-verified FFmpeg 9.0.2 on independently generated inputs after this plan commit, before implementing it. The container specification defines the resulting representation; it does not establish the external program's demuxer or muxer decisions.

- A simple source edit exposes a packet at its media timestamp minus the edit's media time plus its initial empty edit. Copied AAC retains every encoded packet, including priming, rather than trimming each input independently.
- Each file's slot is the sum of preceding reported file durations in microseconds. Its effective start may include negative AAC priming from unselected as well as selected audio. Test whether this is a per-file demuxer origin or a global output shift, including mixed edited/unedited inputs and video-only selection; do not assume either from an all-identical list.
- Convert a proved offset to each track's clock with the pinned program's observed rounding, without accumulating rounded per-packet time. Test half-tick boundaries, two and three inputs and several clock rates before admitting a rounding rule.
- The output audio clock may clamp a candidate DTS to at least the previous DTS plus one. Test equality and overlap cascades. Each preceding packet's stored delta must then be the difference between consecutive final DTS values; the final packet's delta requires its own observation. PTS must agree with DTS for the initial admitted audio.
- Derive the output's leading edits, media/track/movie durations and trailing presentation from this same clock. Test priming retention, final padding and video start/duration instead of assuming that changing audio tables alone establishes exactness.

## Implementation shape

### First proved model

Post-plan generated observations distinguish the origin policies: the shift is global, from the first file's raw AAC priming, including when audio is unselected. Each offset is its file slot minus its reported start plus that one shift, rounded from microseconds independently into each output track's clock. Rounding through the video's clock loses an audio tick and is not equivalent. Mixed edited/unedited orders establish this distinction even though those source shapes remain outside the initial native admission.

The generated edited 48 kHz cases retain every audio packet, produce one-tick boundary deltas followed by recovery to the source cadence, and extend the preceding packet across a short-audio gap. Both tracks' media duration headers equal the output table's decode-duration sum, independently of the video edit's endpoint. Keep the existing video edit construction and hold/extension policy; preserve one full audio media edit instead of applying native join's redundant-edit removal. Movie and track durations still derive from their edit endpoints.

Initially admit two-track, nonfragmented H.264/AAC-LC sources with explicit no-SBR mono/stereo 48 kHz configuration and a simple 1024-tick AAC priming edit. Require zero reported starts, equal priming, complete source presentation endpoints, contiguous zero-composition audio, full 1024-tick audio deltas except a positive tail no longer than a frame, and no unproved half-tick rounding. This admits a concrete positive family without changing the existing 44.1 kHz, synthetic partial-edit, multi-description or unedited-gap refusal contracts. Additional sample rates, mixed priming and bare-source leading deltas require separate evidence rather than guessed admission.

Add one explicit compatibility concat placement carrying file slots and reported starts. Retain existing placement variants and route only the FFmpeg concat caller through the new policy after its command admission. The writer owns source edit admission, the effective origin and the final sample clock. Reuse typed AAC configuration, timelines, sample iteration, tables, header construction and chunk-copy scheduling; do not add a media parser or decoded buffering.

The first listing collects and validates bounded clock facts. The second table listing applies the proved boundary deltas and rejects a changed source. Copying still visits each retained sample once, with the same bytes, sizes, sync state and composition order. Charge retained clock state and every listing to the existing allowances before allocation or writing. Keep native and compatibility policies structurally separate so a source edit cannot silently select the wrong meaning.

Initial admission may be limited to positively proved AAC-LC configurations, 1024-sample frames, mono/stereo, supported equal input track clocks, simple unit-rate edits, contiguous audio timestamps and constant full-frame deltas except the tail. Reject unsupported configurations, fragments, multiple edits, unknowable origins, unproved rounding or endpoints, or a clamp consuming a whole input. Non-AAC copied audio retains its existing admission and refusal contract. Do not widen a condition merely to make a development case pass. Complete bounded listings before classifying a timing refusal so later source I/O, malformed data, work/table limits, arithmetic failure and source changes retain their terminal classification. Unsupported timing may use the original-command fallback; terminal failures and execution failures may not.

Move only the positive-AAC-edit or multi-edit-output refusal that the new writer actually replaces. Keep unrelated refusals, native join behavior and existing negative fixtures intact. If black-box evidence invalidates the clock model, revise this note before implementing another model rather than layering exceptions onto it.

## Verification

Use short independent playable video/AAC sources, different pitches and durations, repeated and mixed input orders, two and three files, equal/short/long audio, edited/unedited input mixtures, reordered video, fractional rates, default and explicit mappings, audio omission and faststart. Every reference input consumes the shared audio-input arguments. These are development fixtures, not a coverage or timing round.

Compare complete ordered encoded packet hashes, sizes, flags, PTS, DTS and duration; stream/frame timing, priming, raw edit lists and cross-multiplied movie/media/track durations; all decoded video and full-precision audio under the unchanged comparator. Exact output means the complete existing judging contract, not whole MP4 file identity or a new tolerance. Exercise comparator mutations for a one-tick boundary, removed clamp, shifted origin and restored per-input priming trim; each must fail.

Unit cases cover abutment, gap, single and cascading overlaps, whole-input clamp refusal, checked integer boundaries and late terminal faults. Unsupported source mutations must preserve absent/existing destinations, stdout and staging, and real fallback must match complete pinned output and status. Compare native default outputs against the preimplementation build and retain all existing assertions. Run focused writer/CLI tests, applicable feature builds, Clippy, formatting, oracle/public-text checks and the exact-head similarity guard. State any extra bounded work cost.

Do not change or run the frozen roster, representatives, command set, generators, results, judging, thresholds or limits as development evidence. Any later full-command coverage measurement remains a separate authorized run after this PR merges; no gain is claimed here. Refs coreav-dev/coreav#25.
