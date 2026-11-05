# Admit only exact input-seek copies

## Problem

`-ss 0.2 -i in.mp4 -c copy -t 1 out.mp4` uses the same native clip options as output seeking over that interval. Native clipping snaps the start and makes its result gapless, while pinned FFmpeg 9.0.2 preserves input-seek pre-roll and applies different packet and timestamp bounds. A generated reordered H.264/AAC source produces 29 native video frames instead of 31, different video/audio durations, and different decoded-output timing. Changing headers alone cannot repair the selected samples.

Independent generated controls also disagree for audio-only input seeks, reordered video at seek zero, and video-only starts between cut points. Video-only, zero-composition-offset controls at exact cut points and whole-sample ends agree. These development observations are not refreshed measured coverage.

## Decision

Keep native `clip`, its existing public options and summary, split, output-only seeking and no-input-seek trimming unchanged. Add a compatibility-only input-seek entry point over the existing clip implementation; the CLI chooses it only when an input `-ss` is present, including zero. It rejects selected audio, several edits, composition offsets in the retained video, a start that snaps elsewhere, and an end that does not equal the complete retained sample duration. A finite requested end must equal the planned end; without one the plan must retain the source tail. The existing parser refusal for negative seek values stays unchanged. Negative input timeline origins, combined input/output seeking and additional input duration/until bounds refuse rather than collapsing their meanings. Audio-only input-seek copies take the existing refusal path. Source chapters or non-tool tags the clip writer does not preserve also refuse. Existing multi-input input-option refusal remains unchanged.

The writer validates its own already-built tables and exact rational plan before writing any bytes. It does not trust a codec's displayed reorder count, parse boxes again, add a source pass, decode samples, increase a read/work/table limit, or change native clip semantics. The old clip API delegates to the unchanged native mode; a new compatibility entry point has a current CLI consumer. Existing table storage already knows whether any retained composition offset is nonzero, so no new per-sample storage is needed.

This chooses reversible refusal over a new FFmpeg packet-copy reconstruction algorithm. Strict-safe mode exits 4 without publishing output or replacing an existing destination. Regular mode forwards the original arguments to the pin. The admitted subset retains existing clip output and must match complete stream fields, decoded-frame timing and decoded values exactly. Native input-seek audio, reordered pre-roll and fractional bounds remain follow-ups; output-seek differences are a separate unit.

Clock admission also refuses source video timescales below 10,000 ticks per second, which the pin rescales, and CLI time bounds not representable in whole microseconds. Independent generated sources at 1,000 and 8,000 ticks per second retained their native clocks while the pin wrote 16,000; controls at 10,000 and 24,000 retained equal clocks. An all-intra billion-tick source sought at `0.541666667` produced 24 native frames and 25 pinned frames. These are generated development diagnostics, not measured coverage. Incomplete source metadata snapshots refuse through the existing metadata-completeness owner; video side data and non-default dispositions also refuse rather than assuming they survive clipping.

## Verification

The maintainer approved current-CI contract changes for only the `basic.mp4` C18 benchmark command (`-ss 0.5 -i INPUT -to 1.5 -c copy`), the `aac.m4a` C19 benchmark command (`-ss 0.5 -i INPUT -t 1 -c:a copy`), and the synthetic format-span input-seek test. Each requires strict-safe exit 4, destination preservation and exact real pinned fallback. The existing format-span checksum remains attached to unchanged output seeking. Required refusals receive no native credit or instruction count. The separate R3 command-family native floor and every other fixture, baseline, cap and tolerance remain unchanged.

- Add a pinned red regression before implementation for the reproduced reordered AV input seek and neighboring inter/all-intra, video-only/audio-only, zero/nonzero seek and aligned/unaligned endpoints.
- Require strict-safe refusal with absent/existing destination and staging preservation wherever admission cannot prove equality; verify byte-identical real pinned fallback and exact original argument forwarding.
- Compare admitted aligned video-only controls with the pin's complete packets, stream fields and all decoded pixels, and preserve the existing input-origin regression.
- Add library tests for the new entry point's pre-writer refusal and admitted controls, including composition offsets, several edits and rational/sample endpoints. Preserve native clip bytes, existing defaults, errors and bounds; do not weaken tests.
- Run affected CLI/library tests and Clippy, formatting, pin/reference/text/shape checks, an exact-public-head canonical guard and independent review. No frozen input, generator, fixture, record, population, runner or judging condition changes or reruns are part of this work.
