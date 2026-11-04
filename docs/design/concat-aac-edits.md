# Refuse concat inputs with positive AAC edits

## Plan

`coreav ffmpeg -f concat -i list.txt -c copy out.mp4` currently uses the native join writer. That writer preserves each file's presentation edits, including removal of AAC encoder priming. The pinned FFmpeg 9.0.2 concat command can instead retain those audio frames and shift the video's start. A successful native join is not an exact implementation of that command.

An independent generated input reproduces the difference: two copies of a 3.25-second H.264/AAC file produce 280 decoded audio frames natively and 282 through FFmpeg. Video starts at zero natively and at 285/12288 seconds through FFmpeg. Ordinary stream copy and concatenation of a video-only source agree exactly. This is development evidence, not a rerun or revision of the frozen GitHub shapes measurement.

The first work unit refuses concat inputs whose parsed AAC track has a positive media edit offset. It examines every input's AAC tracks, including unselected audio: omitting audio from the output does not remove its effect on the concat input's time origin. Use the existing bounded ISO parser and edit entries, not another box reader. Return the existing unsupported-command classification so strict-safe mode exits 4 without publishing an output and regular mode forwards the original arguments to FFmpeg.

Keep native `coreav join`, ordinary stream copy, video-only source concatenation, existing multi-edit refusals and zero-offset AAC controls unchanged. Do not add a packet-copy mode, change shared timing, alter the oracle or modify frozen measurement artifacts. Inputs without this edit pattern are not newly certified exact; other concat timing differences remain separate work.

## Acceptance

- Generated ISO tests cover a positive AAC edit in either list position, audio omitted or selected, a later AAC description, strict-safe refusal, output preservation, temporary-file cleanup and original-argument fallback.
- Zero-offset AAC and video-only source controls remain native; ordinary copy and native join retain their existing behavior.
- On independently generated playable media, the strict-safe candidate refuses the reproduced mismatch. Regular fallback produces the pinned reference's exact stream fields, frame timing, decoded pixels and full-precision decoded audio, using the unchanged current-fixture comparator. Positive controls use the same predicate.
- Run the affected CLI and fallback fixtures, feature-enabled library tests, all-target Clippy, formatting, public-text and reference checks. Recheck frozen artifact identities. No coverage score is refreshed by these diagnostics.
