# Preserve packet order across mapped inputs

## Problem

Copying video from one MP4 input and audio from another can succeed with identical per-stream packet hashes and rational clocks but a different complete packet order from pinned FFmpeg 9.0.2. The multiple-input entry always selects native writer defaults, so it cannot use the compatibility scheduler described in [ordinary-copy packet order](copy-packet-order.md).

## Scope

Reuse that scheduler for compatibility copies containing video and streams from at least two contributing inputs. Preserve input-zero-only routing and native library defaults. Later-input-only selections, audio-only copying, seeking, concat, replacements and other containers remain outside this change. The implementation depends on the existing ordinary-copy scheduler and is a separate layer above it.

Keep existing admission for unfragmented sources with equal movie timescales, zero CLI presentation origins, default metadata, bounded track counts and explicit copy mappings. Do not add per-input rebasing, larger clocks, a parser or another payload buffer. Movie metadata still comes from input zero; track metadata and output order follow the maps.

## Ownership

Add a narrow `RemuxInputsOptions` with `interleave_packets` and `rebuild_durations`. Its defaults preserve native output. Keep mapped streams and omitted cover offsets in their existing arguments, avoiding another metadata copy in the native wrapper. Add an options-bearing multiple-input entry; retain the existing public entry as a delegating default-policy wrapper rather than break its callers or expose ambiguous single-input stream selectors.

The compatibility adapter enables packet interleaving and duration rebuilding together after final map resolution. Permit these policies for pure multiple-input copies, retaining replacement refusals and the existing multiple-input video-origin refusal. Each selected track uses its source movie's existing sample/edit validation and duration reconstruction. Equal source movie clocks keep final edit construction exact. Do not bypass edit endpoints, composition-order checks, exact delay representability or table/work/retained-byte budgets.

Header offsets and copied payloads must share the existing signed-rational DTS scheduler and output-track-order tie rule. One sample per chunk consumes the existing table allowance; exceeding it remains a refusal. The old public entry and native remux bytes must stay unchanged.

## Verification

- Use independent generated, content-distinct H.264/AAC sources with B-frames, 44.1 and 48 kHz audio, and equal movie clocks. Compare the complete ordered packet vector, including hashes and exact rational PTS/DTS/durations, without grouping by stream.
- Cover reversed and repeated maps, three contributing inputs, faststart, metadata/dispositions, timing, decoded video and audio, plus unchanged input-zero-only, later-input-only and audio-only controls.
- Check selected-track duration declarations and unsupported edits in either input, unequal clocks, fragmentation, invalid mappings and existing budgets. Writer admission refusals leave an empty sink; CLI refusals preserve absent/existing destinations and staging, and replay original arguments through exact pinned fallback.
- Run affected writer and CLI tests, standalone writer fuzz builds, Clippy, formatting, public-text and shape checks, then the exact-head canonical similarity guard. Do not change frozen commands, judging, coverage records, baselines, caps or tolerances, and make no timing claim.
