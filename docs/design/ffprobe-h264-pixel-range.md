# H.264 pixel-format range in ISO ffprobe

## Goal and scope

Match pinned FFmpeg 9.0.2 pixel-format names for ISO H.264 streams whose
sample-entry colour box disagrees with their sequence parameter set.
Keep reported colour fields and their existing container precedence
unchanged. A full-range H.264 configuration with a limited-range box
reports `pix_fmt=yuvj420p` and `color_range=tv`, not `yuv420p`.

The two declarations are already retained separately. ITU-T H.264
E.1.1/E.2.1 defines the sequence parameter set's range flag and its
inference; ISO/IEC 14496-12 section 12.1.5 defines the colour box.
The spelling of compatibility pixel-format names is verified against the
pinned binary, not inferred from the reported colour field.

## Design

At the existing ISO H.264 video-info boundary, derive the pixel-format
range from the codec configuration's retained parameter-set colour.
Do not let the merged container-facing range override an explicit SPS
range in the H.264 configuration adapter. Keep `compat::reported_color` and its consumers
unchanged. An absent parameter-set range keeps the existing container
fallback; it does not become an explicit reported colour declaration.
An empty track cannot establish a pixel format from this configuration
alone, so leave that field unknown. External-reference colour reporting
keeps its existing behavior and is not part of this change.

Reuse the existing SPS parser, sample-entry facts and pixel-format mapper.
Add no source reads, packet scan, parser, allocation budget or new cap.
Do not change HEVC, AV1, VP9, MPEG-TS, Matroska or frame output. Do not
modify comparison keys, tolerances, command rows, oracle pin or baselines.
If a newly observed case cannot be derived to match, refuse before output
instead of claiming approximate native success.

## Verification

Before selecting this group, compile and run the feature-gated library
fixtures with `transcode,input-capture` and all-target Clippy at the main
base. For the change, add a generated CLI regression that fails on that
base and checks both pixel format and unchanged reported range.

Compare contrasting full/limited/absent SPS and container declarations
with the pin, including empty tracks, monochrome and higher bit depths
where the existing mapper supports them. Retain all unrelated metadata
values against the same main binary. Use only short generated controls
and admitted non-holdout development input after full size/hash checks.
This is diagnostic evidence, not a full scoreboard or judged run.

Run the relevant CLI tests, feature-gated library suite, all-target
Clippy, formatting and public-text checks. Run the canonical similarity
guard on the published exact head before readiness. Independent review
and final maintainer approval remain required.
