# Rebuild copied media durations without changing native remux

## Problem and scope

An ordinary `coreav ffmpeg -i INPUT -c copy OUTPUT.mp4` with a zero input origin still preserves a conflicting declared `mdhd` duration. On an independently generated edited H.264 source with all 78 samples and `mdhd.duration` changed to 10000, CoreAV succeeds with `duration_ts=10000` while pinned FFmpeg 9.0.2 reconstructs 39936. Generated AV sources reproduce the same discrepancy when either the video or audio media header is shortened. Their packet bytes, timestamps and durations already agree; this unit changes the copied output header policy, not source packet selection.

Scope: single-input ISO ordinary copy, faststart and explicit copy/mapping through the existing full-file remux. Native remux, sanitization and replacement defaults continue preserving their own declared-header policy. Multi-input mapping, seek, concat and probe output remain separate; no frozen input, generator, result or judging rule changes.

## Ownership and interface

Add opt-in `RemuxOptions::rebuild_durations: bool`, default false. When set, the writer derives track durations from the retained edit mapping or copied media, and media durations from complete copied sample durations. The compatibility `Job::Remux` sets it; native CLI, Python, WASM and replacement consumers leave it false. The existing explicit video-origin policy also implies rebuilt durations, so its admission guard and duration reconstruction remain one policy path rather than independent overrides.

The existing bounded table pass already owns the sum of sample durations and all source edits. Under the opt-in policy it does not retain declared track/media overrides, including the later movie-timescale rescaling override. Only absent or simple source edits are admitted; multiple or unmodeled edits and a later video sample composed before the first decoded sample refuse before writing, rather than pretending that rebuilding headers establishes compatible packet selection. A table-pass timing refusal fails the whole request even under the native drop policy, rather than silently publishing only the remaining audio. Native preservation keeps the old path. Samples, edit entries, composition offsets, metadata and source timescales otherwise remain unchanged; positive-origin normalization retains its existing exact-common-clock policy. No extra scan, payload read or input-dependent allocation is introduced. If a reproduced case cannot match the unchanged pinned comparison under this policy, refuse it before output rather than rounding away the mismatch.

An unconditional change to remux would violate the native preservation contract. A probe-only change would leave the emitted file incorrect. Keeping the opt-in boundary lets compatibility output recompute its clock while native remux continues exposing the stored declarations.

## Verification before delivery

- Add generated playable pinned-oracle regressions for zero-origin edited and unedited video/AV sources with short, long and unspecified media durations, including audio-header mutations, faststart and explicit copy. Compare complete decoded output and timing using the existing exact comparator, and inspect source/output headers and packet clocks independently.
- Keep source sample/payload preservation and native declared-duration behavior in deterministic writer regressions. Test low table/work/retained budgets and unchanged error classes.
- Exercise `video_origin=Some(...)` and duration-rebuild options in the bounded remux fuzz target while keeping all current byte/sample invariants. Smoke-check harness construction locally; campaigns remain on CI.
- Recheck the queued origin-policy regressions, ordinary-copy controls, native remux, seek and probe behavior, relevant feature builds, all-target Clippy, formatting, pin and publication checks. No new performance or measured coverage claim is made.
