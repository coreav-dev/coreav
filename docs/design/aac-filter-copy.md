# Admit identity AAC filtering for MP4 concatenation

## Problem

An explicit output `-bsf:a aac_adtstoasc` currently refuses during option parsing, including MP4 concat inputs whose AAC access units already use AudioSpecificConfig rather than ADTS. The documented filter removes ADTS transport headers and supplies AudioSpecificConfig; silently ignoring it is unsafe when transport headers or a different selected audio codec are present.

## Decision

Accept only the exact output option `-bsf:a aac_adtstoasc` for the existing concat-to-MP4 copy path. Keep other filters, selectors, filter arguments/chains and non-concat uses refused. Existing selection, input validation, timing admission, edit refusals and limits remain authoritative. No seek behavior or registered fixture, command, judging or benchmark material changes.

The new admission is an identity proof, not an ADTS converter. After the native writer finishes its staged output, inspect that same owned file handle before publication. Require every emitted audio track to be configured AAC and every retained audio packet to have a non-ADTS prefix; short or transport-like packets refuse. No audio makes the audio-only filter inert. Use the existing ISO reader, packet iterator and default limits, with a fixed prefix buffer and no per-sample storage. Inspect the emitted artifact rather than reopening source paths, so the proof concerns exactly the bytes being published.

Add a checked variant of the existing atomic file writer with one current consumer. Existing entry points keep their no-readback path; only checked output is opened for readback. Validation runs after buffering/header patches are flushed and before placement, and every failure removes staging without changing an existing destination. The CLI retains the existing regular-mode fallback, including the shared banner prefix and the complete original argument sequence.

## Verification

- Commit this plan before implementation or generated diagnostics; use only independent generated inputs, never rerun frozen judging.
- Require a red regression for the explicit filter on an otherwise admitted concat; compare native output with the same unfiltered native command and pinned FFmpeg 9.0.2, including packet identity, complete decoded timing and full-precision decoded values.
- Cover AAC, video-only, non-AAC audio, ADTS-like packet prefixes, short samples, filter syntax/position and later concat inputs. Unsupported cases require strict-safe exit 4, absent/existing destination and staging preservation, and real pinned fallback bytes/status plus exact original argument replay under the existing banner policy.
- Test the atomic checked writer's success, rejection, patched header and default no-readback behavior. Test packet/header admission and existing limits with small synthetic inputs.
- Keep native clip, seek, concat timing and frozen Rust tests unchanged. Run affected tests, workspace/feature Clippy, formatting, pin/reference/public-text/shape checks and an exact-head canonical guard; request independent review after the PR opens.
