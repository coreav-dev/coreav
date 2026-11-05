# Reuse ordinary copy when every mapped stream is from the first input

## Problem and scope

A command with two inputs but only `-map 0:v:0 -map 0:a:0` still uses the multi-source native remux job. Merely naming an unused input therefore bypasses ordinary compatibility copy's sample-derived duration reconstruction and edit admission. Independent generated H.264/AAC sources reproduce a wrong success when either media header declares 10000 ticks: the single-input copy agrees with pinned FFmpeg 9.0.2, while the equivalent first-input-only mapped command differs.

This unit covers MP4 compatibility copy whose final selected streams all come from input zero. Mapping streams from another input, mixing inputs, metadata from another file, nonzero input origins and general multi-input parity remain outside scope. No frozen input, generator, fixture, result or judging condition changes.

## Decision

After the existing multi-input validation and final positive/negative map selection, canonicalize a selection entirely from input zero to the existing `Job::Remux`. Preserve the selected stream order and the first input's left-out cover handling. The validated input origins are already zero, so the job does not request video-origin normalization. Default metadata also belongs to the first input, which is why a selection entirely from a later input is not equivalent and stays on the existing multi-source path.

Reuse the ordinary writer policy rather than duplicating its options in the multi-input writer or changing native `remux_mp4_inputs`. This applies the same bounded duration reconstruction and supported-edit/composition admission to equivalent commands. All input validation, codec selection, disposition guards, alias protection, source limits and output transactions remain. Unsupported timing retains whole-request strict-safe refusal or original-argument fallback. No parser, packet reader, resource cap or oracle changes are needed.

## Verification

- Add a generated pinned regression that fails before the implementation for both shortened video and audio media headers, with a distinct unused second input and first-input metadata. Cover direct mapping, removal of a later-input selection, faststart and single-stream selections.
- Compare complete output timing, all decoded video bytes and full-precision audio with unchanged FFmpeg 9.0.2 checks. Include long and unspecified declarations and equivalent single-input output controls.
- Check unsupported simple edits and ambiguous first-video timing with absent and existing destinations and byte-identical fallback, preserving the original unused input arguments.
- Preserve genuinely cross-input mapped output hashes and native remux defaults, plus ordinary copy, seek and probe controls. Run affected CLI tests, feature-enabled checks, lint, formatting, public-text/reference checks and the canonical exact-public-head similarity guard. These are development diagnostics, not refreshed coverage or performance measurements.
