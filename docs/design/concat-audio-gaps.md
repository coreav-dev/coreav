# Refuse concat audio timelines needing several edits

## Problem

An unedited audio track can end before its video's presentation ends. The native join preserves the gap before the next input with separate media edits and an empty edit. That is a valid native presentation, but it is not the packet timeline written by pinned FFmpeg 9.0.2's concat copy. Its final audio packet before the boundary spans the gap instead. Treating native join output as an exact compatibility copy can therefore return a wrong success even when every source has no edit list.

Independent generated H.264/AAC examples reproduce this with three seconds of audio beside 3.25 seconds of video, and with a fractional video frame rate. In the repeated short-audio example, CoreAV reports audio duration 271950 ticks at 44100 Hz instead of the pinned output's 282975. The native output's edit boundaries also cause repeated packets when the external demuxer presents it. Video-only copies of these sources remain exact. These are development diagnostics, not a changed measured population or coverage result.

## Decision and boundary

Keep native join unchanged. At the existing compatibility join admission boundary, refuse a staged join whose output needs more than one audio edit. The writer's existing `JoinSummary::audio_edits` is authoritative; do not infer the condition from rounded probe durations or add a second packet reader. The compatibility output transaction already keeps the result in a temporary file until this boundary accepts it. Refusal must preserve an existing destination and publish no new file, and regular mode must forward the original arguments to the existing pinned fallback.

This conservatively refuses any multi-edit audio output, including a potentially exact case, rather than guessing that its demuxed timeline agrees. An absent audio track or one audio edit retains the existing path. Source positive-AAC-edit, multiple-input-edit and first-origin guards remain. Native `coreav join`, its placement policy, packet durations, edit lists and reports do not change. Ordinary copy, seek and probe behavior are outside this unit.

Exact native concat audio-gap reconstruction would need an explicit compatibility packet-clock policy, including the last packet duration at every boundary. That is a separate follow-up, not a change to native gapless presentation or an oracle tolerance.

Generated AC-3 and E-AC-3 examples with simple source edits also need several output audio edits and differ from pinned FFmpeg 9.0.2. On a repeated two-second source, native join's copied output starts video at zero instead of 68 ticks at 12800 Hz and decodes 125 audio frames instead of 126. ISO/IEC 14496-12 edit mappings can preserve a valid native presentation without reproducing that external packet timeline; the fixed AC-3 frame size is specified by ETSI TS 102 366 F.2. The audio-copy fixture checker previously ignored edits while comparing packet bytes, which proves native preservation but not compatibility equality. Its two compatibility cases now require explicit refusal, destination preservation and byte-identical pinned fallback. Native join, every source recipe and all native packet/duration/decode checks remain unchanged. Refusals are reported separately from native passes.

## Verification

- First add a pinned regression that fails with the current wrong native success. Generate unedited equal-span, shorter-audio and fractional-rate sources; cover repeated and mixed inputs, full AV, explicit mapping and video-only selection.
- For refused cases, compare fallback files byte for byte with FFmpeg 9.0.2 and check strict-safe refusal with both absent and existing destinations. Keep the unchanged exact comparator for retained native controls, including complete video and full-precision audio.
- Check native join output hashes against the preceding implementation and verify no new source reads, parser budget changes or extra packet pass are introduced by the admission check.
- Run affected CLI tests, feature-enabled checks, lint, formatting, public-text/reference checks and the canonical exact-public-head similarity guard. Do not modify frozen inputs, generators, records, fixtures, runners or judging conditions; no refreshed coverage or speed claim is made.
