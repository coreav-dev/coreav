# ISO AV1 aspect ratios

`coreav ffprobe` reports square pixels for AV1 when the codec configuration
contains readable sequence dimensions and no other aspect-ratio signal
applies. The [AV1 ISO binding section 2.2.4](https://aomediacodec.github.io/av1-isobmff/#av1sampleentry)
requires a pixel-aspect box when render dimensions differ from the
sequence's maximum dimensions. A missing box does not mean an unknown
ratio for a configured square-pixel stream.

The existing precedence remains: the sample entry's `pasp`, the codec's
ratio, then the track's presentation geometry and matrix. The AV1 1:1
fallback comes last; display aspect uses the existing bounded reduction
of width times horizontal spacing over height times vertical spacing.
An AV1 tag without readable sequence dimensions gets no new default.
Other codecs and containers keep their existing behavior.

Generated-byte regressions cover missing `pasp`, an explicit non-square
ratio, track geometry, quarter-turn rotation and unreadable configuration.
Generated files are also compared with the pinned FFmpeg 9.0.2 binary.
The change adds no parser, payload read, comparison exception or threshold.
