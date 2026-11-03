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

## Implemented boundary

The supported edit planner is now the sole owner of the playback window.
The additional `max(media_start, PreSkip)` clamp is removed. Absent edits
still use the previous PreSkip behavior; unsupported edits still emit
their existing warning. This change does not broaden the planner's edit
support or alter any packet's decoder state.

The former `opus_priming_samples_are_never_presented` test encoded that
extra clamp. Its two expected lengths for edits starting at media time 0
and 100 are intentionally changed to the full 9,120 and 8,640 samples
specified by their edits. Its replacement name states the corrected
contract; the no-edit expectation and the media-time-312 control are
unchanged. This is a specification correction, not an oracle tolerance.

`informative_opus_preskip_cannot_shorten_a_finite_edit` preserves the
4,800-sample edit at media time 312 while varying PreSkip across 0, 312
and 3,840. It requires complete S16 and F32 bytes, counts, warning-free
output and thread invariance. Before the fix, PreSkip 3,840 produces only
1,272 samples.

`scripts/opus_mp4_presentation.py` builds three short mode-isolated
sources with the pinned oracle and reads the MP4 edit and packet payload
tables independently. It checks twelve presentations: SILK, Hybrid and
CELT, two finite edit ends, and original versus 3,840 PreSkip. All S16
and F32 native outputs, at one and eight threads, must preserve the edit
exactly. Finite output must be the exact prefix of the same decoder's
full-window output, without shifting or adding samples.

For an edit ending at the decoded packet boundary, default FFmpeg 9.0.2
agrees with the specified count: 5,448 samples for the SILK and Hybrid
sources, and 5,640 for low-delay CELT. Both PreSkip values give the same
oracle output length. For the 4,800-sample finite edit, FFmpeg keeps that
packet tail instead of honoring the edit end. The
[known divergence](../known-divergences-audio-output.md#opus-mp4-pcm-decode-honors-the-edit-window)
records this separately. The test compares presentation counts with the
oracle; existing lossy decoder waveform-conformance bars remain unchanged.
