# Refuse a negative map before a positive map

## Problem

`-i a.mp4 -i b.mp4 -c copy -map -1:a -map 0 out.mp4` succeeds natively, while pinned FFmpeg 9.0.2 rejects it without creating an output. The selector resolves maps in reverse order to bound retained output selections, but it does not validate the forward-order precondition for a negative map. The same wrong success occurs with a leading negative map of input zero and with a negative selector that matches no source stream.

A later negative map is valid after a positive map, even if earlier removals emptied the current selection. A check of whether the current selected vector is empty would reject valid remove-and-readd commands. Optional maps already take the existing refusal/fallback path. The pin accepts an optional leading negative map followed by a positive map, so that behavior must remain available through fallback.

## Decision

At the shared stream-selection boundary, refuse a parsed map list whose first map is negative. Keep the bounded reverse-order resolver and its work/output limits unchanged. Strict-safe mode exits 4 and publishes no file. Regular mode forwards the original arguments to FFmpeg, including its rejecting exit behavior and unchanged existing destination. This avoids imitating FFmpeg's error codes or diagnostics inside CoreAV's usage-error convention.

The rule applies to single and multiple inputs. It does not reject a negative map merely because previous positive selections were later removed. Positive-first mappings, remove-and-readd orderings, all-removed selection errors and optional-map fallback retain their existing behavior. No input parser, writer, resource limit, native output oracle or frozen judging condition changes.

## Verification

- Add a pinned regression that fails on the current wrong success and checks leading negative maps of the same and another input, several leading removals, and a removal without a later positive map.
- Verify absent and existing destinations, exact fallback exit behavior, and no published file for rejected pin commands.
- Keep positive-first remove/readd commands native and compare selected stream fields and complete per-stream packet clocks and payload hashes with the pin. Check that all-removed selections still fail.
- Check optional leading and optional unmatched-positive maps through their unchanged refusal/fallback route, including exact output bytes when the pin succeeds.
- Run affected CLI and feature tests, lint, formatting, public-text/reference checks and a canonical exact-public-head similarity guard. Record the pre-existing defect on the copy tracking issue. These checks are independent development diagnostics, not refreshed measured coverage.
