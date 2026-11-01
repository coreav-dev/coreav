# Known divergences: splits versus the oracles

Every intentional difference between a split and its source, as the
black-box oracles see them, is documented with a machine-readable twin. The twin is
[`known-divergences-split.json`](known-divergences-split.json);
`scripts/split_check.py` applies it with one predicate per entry, and
`tools/oracle/tests/test_split_kd.py` keeps this page, the twin and the
predicates in sync. A split counts as agreement only if every difference is
covered; covered splits are reported apart from direct agreement.

Each segment is written by the clip writer's code (its tables, file head
and copy pass; [`docs/design/split.md`](design/split.md)), so the entries
of [`known-divergences-clip.md`](known-divergences-clip.md) also apply to
each segment, rechecked by their predicates on the segment as if it were a
clip. The entries below are the split's own. Which packets each segment
holds, and in which order, is the split's promise and has no known
divergence; the entries below cover only times and keyframe flags that the
oracles report differently.

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10.
Predicates use the files' own box headers and the oracles' output, never
file names.

## KD-S-AUDIO-RESTART

- Field: the decoded audio of a segment.
- When: the segment's audio, decoded from the segment's first audio packet
  (a split adds no pre-roll), does not give the same samples from its
  second decoded frame on as decoding the source from its first frame.
- What each reports: the segment's samples equal, sample for sample, the
  source's own packets of the segment decoded from the segment's first
  packet, from the segment's audio media time on (its edit's, or 0 without
  an edit list); they differ from the source decoded from its start.
  Example: the fixtures' AAC is encoded with perceptual noise substitution,
  whose noise depends on the decoder's state: the second segment of
  `split_hevc.mp4` cut every second differs by at most 2.4e-5 from its
  second frame on.
- Why: ISO/IEC 14496-3 (a decoder's output depends on its state; the noise
  of perceptual noise substitution is not defined by the bitstream); ATSC
  A/52 7.3.4 (AC-3 and E-AC-3 dither mantissas coded with no bits
  from a generator whose state the bitstream does not define);
  `docs/design/split.md` (every packet is copied once, so no segment but
  the first has the frame before its first). The cause is that of
  `KD-C-AUDIO-RESTART`, which needs an edit list; a segment's audio often
  has none.
- Coverage requires: at most one media edit on the segment's audio, and its
  decoded samples equal to the source's packets of the segment, extracted
  by `ffmpeg` in their raw format (one frame per sample, as for
  `KD-C-AUDIO-RESTART`; edit lists ignored) and
  decoded from the segment's first audio packet, at exactly the media time
  of its edit, or from the first sample without an edit list; and the
  segment's decode durations exactly those the source's `stts` stores for
  the kept samples, read from both files (the decode follows the segment's
  own times, so wrong ones would agree with themselves).

## KD-S-EDIT-MIDFRAME

- Fields: the video packets' `pts` and `dts`, and the decoded video.
- When: the source's video edit starts at a media time inside a frame, not
  on a frame boundary.
- What each reports: CoreAV applies the edit exactly, and the first
  segment keeps the source's edit, so both tools agree on it. The other
  segments' edits start on a frame (at their cut), and `ffmpeg` reports
  them exactly; but it starts the source's timeline at a frame boundary,
  so every source time it reports after the first segment is off by the
  same fraction of a frame. Example: a recording whose edit starts at
  media time 64,679 with frames of 1,001 ticks; from the second segment on,
  every packet's times in `ffprobe` are 386 ticks from the source's.
- Why: ISO/IEC 14496-12 (an edit maps its media time to the start of the
  presentation); the simple edit list is applied exactly;
  the same cause as `KD-C-EDIT-MIDFRAME`.
- Coverage requires: one media edit in the source whose media time is not
  a multiple of the frame duration; every packet of every segment equal in
  data, size and keyframe flag; the first segment's times equal; and every
  other differing time off by one constant, not 0 and smaller than a frame.
  For the decoded video: the segments' frames equal the source's once that
  constant is applied to every segment but the first.

## KD-S-DTS-SHIFT

- Field: the video packets' `dts`.
- When: a segment's video has composition offsets that a reader taking them
  as signed sees as negative, and the smallest of them differs from the
  source's (the segment holds only part of the samples).
- What each reports: CoreAV keeps decode times as stored. `ffprobe` shifts a
  track's decode times by its smallest composition offset, which is the
  source's for the source and the segment's for the segment, so every
  decode time in such a segment differs by one constant while presentation
  times agree.
- Why: composition offsets are copied as stored; the
  same cause as `KD-C-DTS-SHIFT`.
- Coverage requires: every packet of every segment equal in data, size,
  keyframe flag and presentation time, and in each segment whose decode
  times differ, such an offset in its table and one constant difference,
  not 0: the source track's smallest composition offset less the
  segment's, each counted where negative, read from both files' `ctts`.
  With `KD-S-EDIT-MIDFRAME` at once (the source's edit starts inside a
  frame), the presentation times of every segment but the first may all
  differ by one constant smaller than a frame instead, and each segment's
  decode times then by that constant plus its own. Example: a screen
  recording with offsets down to −335 ticks and an edit at media time 169
  with frames of 10 ticks, whose second segment's smallest offset is −310:
  the source's times for its packets are 1 tick (presentation) and 26
  ticks (decode) before the segment's, shifted by its start.

## KD-S-PTS-SHIFT

- Fields: the video packets' `pts` and `dts`, and the decoded frames' times.
- When: the source's video has no edit list and signed composition offsets
  whose smallest is negative (version 1 `ctts` or `trun`).
- What each reports: CoreAV and the segments present each sample at its
  stored composition time. For the source, without an edit list, `ffmpeg`
  moves every presentation time later by the size of the smallest offset;
  for a segment, whose edit starts at its first presented sample, it keeps
  them. Every source time then exceeds the segments' by that size. Example:
  a fragmented 10-bit HEVC recording with offsets down to −100 ticks (one
  frame at 60 frames per second), whose segments are all listed and decoded
  100 ticks earlier than the source.
- Why: composition offsets are reported as stored, never shifted; ISO/IEC 14496-12 (composition time is decode time
  plus the offset); the same cause as `KD-C-PTS-SHIFT`.
- Coverage requires: no edit list on the source's video; the smallest
  composition offset over the segments' tables, read as signed, −D with D
  above 0; every packet of every segment equal in data, size and keyframe
  flag, with the source's `pts` and `dts` above the segment's (shifted by its
  start) by exactly D; and, for the frames, the same frames once D is
  applied.

## KD-S-SYNC-VERIFIED

- Field: the video packets' keyframe flags.
- When: the source's container marks samples as sync that the coded data
  does not start from, and `ffprobe` reports the container's flag: HEVC
  without a sync sample table, or with one that lists every sample.
- What each reports: each segment marks as sync only the samples the cut
  rule allows (`docs/design/cut-points.md`, "The output's sync flags"):
  for HEVC, IRAP pictures. `ffprobe` flags every source packet, as the
  container states. Example: `clip_hevc_stss_all.mp4`, an IRAP picture
  every 25 frames and an `stss` that lists every sample, split every
  second into 4 segments that flag 4 of its 100 packets.
- Why: ISO/IEC 14496-12 8.6.2 (without a sync sample table every sample is
  a sync sample); ISO/IEC 14496-15 and ITU-T H.265 7.4.2.2 (an HEVC sync
  sample holds an IRAP picture, `nal_unit_type` 16 to 23); the same cause
  as `KD-C-SYNC-VERIFIED`.
- Coverage requires: every source packet in exactly one segment, in order,
  equal in data, size and times; each segment starting with a keyframe;
  differences only in flags the source's packets have and the segments' do
  not, at least one; and, in each segment with edit lists ignored, the
  flagged packets exactly the ones `ffmpeg` decodes as key frames.
