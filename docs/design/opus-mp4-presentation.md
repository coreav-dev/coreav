# Opus MP4 presentation trimming

## Plan

Whole-playback presentation follows the supported MP4 edit list.
[Opus in ISOBMFF sections 4.3.2 and 4.4](https://opus-codec.org/docs/opus_in_isobmff.html#4.4)
make dOps PreSkip informative for this purpose and assign startup and
terminal trimming to the edit. Do not clamp an edit's media window to
that informational value. Keep existing behavior for absent or
unsupported edits outside this correction's scope.

1. Remove the additional PreSkip clamp from supported edit playback.
2. Update the fixture expectations that deliberately encoded the old
   clamp, with an explicit specification-based reason. Add exact S16 and
   F32 count and output-invariance controls for a 4,800-sample edit at
   media time 312 with PreSkip 312 versus 3,840.
3. Compare generated SILK, Hybrid and CELT MP4 inputs with pinned FFmpeg
   9.0.2. Independently parse edit metadata and packet work before judging
   native output. Require exact counts from the edit; compare oracle
   output where its presentation agrees and document default-oracle
   disagreements rather than treating them as permission to change the
   presentation.
4. Run fixture and conformance CI, preserving all decoder waveform bars,
   and the clean-room similarity guard. No new decoder or PCM-output
   eligibility is granted by this change.

Only generated development inputs and public specifications are used.
No other decoder source is read. Matroska signed padding is a separate
change. No held-out measurement or performance run is part of this work.
