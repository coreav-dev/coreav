# Known divergences: clips versus the oracles

Every intentional difference between a clip and its source, as the
black-box oracles see them, is documented with a machine-readable twin. The twin is
[`known-divergences-clip.json`](known-divergences-clip.json);
`scripts/clip_check.py` applies it with one predicate per entry, and
`tools/oracle/tests/test_clip_kd.py` keeps this page, the twin and the
predicates in sync. Each entry covers only the fields it names; a clip
counts as agreement only if every difference is covered, and covered clips
are reported apart from direct agreement.

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10.
Predicates use the files' own box headers and the oracles' output, never
file names. Probe divergences are listed in
[`known-divergences.md`](known-divergences.md), extraction divergences in
[`known-divergences-extract.md`](known-divergences-extract.md).

## KD-C-STTS-SHORT

- Field: the video stream's `duration_ts`.
- When: the clip's frames are presented later than their decode durations
  add up to, as in a variable-frame-rate source, so the video edit is longer
  than the sum of the decode durations (`stts`).
- What each reports: the output's media header declares at least the
  edit's length, so CoreAV reports the clip's length: the media edit, capped
  by the media header. `ffprobe` reports the smaller `stts`
  sum. Example: a clip of `vfr.mp4` from 0.6 s to 1.6 s; its edit is 1.6 s,
  `ffprobe` reports 1.52 s.
- Why: a stream's duration is the media edit, capped by the media header, and the clip's media duration is never shorter than the
  video edit; the same cause as `KD-STTS-SHORT`.
- Coverage requires: one media edit, `ffprobe`'s value equal to the
  output's `stts` sum, and that sum smaller than the edit (in media ticks),
  which is at most the media header duration.

## KD-C-EMPTY-EDIT-DURATION

- Field: a stream's `duration_ts`.
- When: the stream starts after the clip (its source stream starts later
  than the snapped start), so its edit list is an empty edit followed by a
  media edit.
- What each reports: CoreAV's stream duration is the media edit. `ffprobe` takes the smallest of the media header duration, the decode
  durations and the sum of every edit, the empty edit included, which here is
  the decode durations of the kept samples. Example: the head clip of
  `clip_audio_late.mp4`; the audio edit is 0.278 s empty and 0.722 s of
  media, `ffprobe` reports 0.7253 s, the 34 kept frames.
- Why: the stream duration is the media edit, and counting the empty edit,
  as `ffprobe`'s rule does, is rejected as an oracle quirk; a
  stream that starts after the clip gets an empty edit.
- Coverage requires: exactly an empty edit and one media edit, `ffprobe`'s
  value equal to that rule computed from the output's own box headers, and
  more than one tick away from the media edit alone; and those headers
  the source's: the output's media timescale its source track's (from the
  source's `mdhd`), and its decode durations summing to what the source's
  `stts` stores for the kept samples (not `ffprobe`'s packet durations,
  which can differ from the stored ones).

## KD-C-AUDIO-RESTART

- Field: the decoded audio.
- When: decoding the clip's audio, which starts one frame before the clip
  (the pre-roll), does not give the same samples as decoding the source from
  its first frame.
- What each reports: the output's samples equal, sample for sample, the
  source's own kept packets decoded from the clip's first kept packet, from
  the output edit's media time on; they differ from the source decoded from
  its start. Example: the fixtures' AAC is encoded with perceptual noise
  substitution, whose noise depends on the decoder's state: a clip of
  `basic.mp4` from 1 s differs by at most 5.7e-4 throughout, while the same
  clip of an AAC encoding without it is bit-exact.
- Why: ISO/IEC 14496-3 (a decoder's output depends on its state; the noise
  of perceptual noise substitution is not defined by the bitstream); the
  clip keeps one frame of pre-roll.
- Coverage requires: one media edit on the output's audio; the output's
  decoded samples equal to the source's kept packets, extracted by `ffmpeg`
  as ADTS frames (one per sample, edit lists ignored) and decoded from the
  first kept packet, at exactly the edit's media time; and the kept packets
  the ones the clip rules name, judged from the source's own packet list and
  times (within one tick): one packet of pre-roll before the first packet
  presented at the snapped start (a discarded priming packet included; none
  before the track's first), every packet presented inside the clip, and
  none past the one presented at its end. The decode compares
  the clip with its own kept packets, so it cannot see which packets were
  kept; this does. The edit's media time must also be the source's, as the
  decode starts there: where the source's edit list (ISO/IEC 14496-12
  8.6.6) puts the output's audio start (the snapped start plus the output's
  empty edit), less the first kept packet's stored decode time, rounded to
  the nearest tick. And the output's decode
  durations must be exactly those the source's `stts` stores for the kept
  samples. These three timing checks, read from the files themselves,
  apply to KD-C-AUDIO-JITTER and KD-C-AUDIO-EDIT-RATE as well: each decodes
  the clip's own packets at the clip's own times, which would agree with
  themselves when wrong.

## KD-C-AAC-PROFILE

- Field: the audio stream's `profile`.
- When: `ffprobe` names a different AAC profile for the source and the clip,
  or names one on one side only.
- What each reports: the clip's AAC configuration is the source's, byte for
  byte; `ffprobe` names the profile from what it decodes at the stream's
  start, which differs where SBR or parametric stereo is signalled only in
  the audio frames. Examples: an HEVC file whose AAC LC track it
  names `LC` and whose clip it names `unknown`; web files it names
  `HE-AACv2` and whose clips it names `HE-AAC`.
- Why: ISO/IEC 14496-3 (SBR and parametric stereo may be signalled
  implicitly in the bitstream); the `esds` is copied as stored; the same cause as `KD-X-PROFILE-UNNAMED` and
  `KD-AAC-IMPLICIT-SBR`.
- Coverage requires: `aac` on both sides, different profile names, and the
  same configuration bytes in both files as `ffprobe` dumps them.

## KD-C-CTTS-UNSIGNED

- Field: the decoded video (framemd5).
- When: the source's composition offset table is version 0 and holds
  offsets of at least 2^31, as writers that store negative offsets in an
  unsigned table do; the clip copies them as stored.
- What each reports: CoreAV reads a version 0 offset as unsigned, so such a
  sample is presented about 2^32 ticks later and lies outside the edit; the
  clip's run ends at the last sample it presents inside the clip. `ffmpeg`
  reads the offset as signed and presents the sample one or two frames
  earlier, inside the clip, in the source and the clip alike, so it decodes
  a frame near the clip's start or end that the clip does not hold.
  Example: an HEVC file whose B frames store -512 as 4,294,966,784;
  a 10-second clip has 327 such frames in the source and 326 in the clip.
- Why: ISO/IEC 14496-12 (version 0 offsets are unsigned); composition
  offsets are reported and copied as stored.
- Coverage requires: a version 0 table with an offset of at least 2^31 in
  both files, and the clip's frames equal to the source's frames inside the
  clip, in order and unchanged, short of at most four, each within eight
  frames of the clip's start or end.

## KD-C-EDIT-MIDFRAME

- Fields: the video packets' `pts` and `dts`, and the decoded video.
- When: the source's video edit starts at a media time inside a frame, not
  on a frame boundary.
- What each reports: CoreAV applies the edit exactly, so the source's
  frames are presented at their composition time minus the edit's media
  time, and the clip keeps that timeline. `ffmpeg` starts the source's
  timeline at a frame boundary instead, so every source time it reports is
  off by the same fraction of a frame. The clip's own edit starts on a frame
  and agrees in both tools. Example: a recording whose edit starts at
  media time 64,679 with frames of 1,001 ticks; `ffprobe` places the
  source's frames 386 ticks earlier than CoreAV, so a 10-second middle clip
  differs from the source by 386 ticks in every packet and by one frame at
  its start.
- Why: ISO/IEC 14496-12 (an edit maps its media time to the start of the
  presentation); the simple edit list is applied exactly, and the clip
  starts at the snapped sync sample.
- Coverage requires: one media edit in the source whose media time is not a
  multiple of the frame duration; every packet of the kept run equal in
  data, size and keyframe flag, with one constant time offset, not 0 and
  smaller than a frame; and the decoded frames equal once that offset is
  applied (where the clip's table also holds unsigned offsets of at least
  2^31, short of at most four frames near its ends, as for
  `KD-C-CTTS-UNSIGNED`). Where the clip's own edit starts inside a frame too
  (a clip that starts where the source does, at a constant frame
  duration), `ffmpeg` moves both timelines alike: the packets agree, and the
  decoded frames must equal the source's but for exactly one, the last,
  which on the edit's exact timeline (`ffmpeg`'s times plus the gap from the
  source edit's media time to the next frame boundary) lies past the clip's
  edit, within one frame of it.

## KD-C-DTS-SHIFT

- Field: the video packets' `dts`.
- When: the clip's video has composition offsets that a reader taking them
  as signed sees as negative, and the smallest of them differs from the
  source's (the clip keeps only part of the samples).
- What each reports: CoreAV keeps decode times as stored. `ffprobe` shifts a
  track's decode times by its smallest composition offset, which is the
  source's for the source and the clip's for the clip, so every decode time
  differs by one constant while presentation times agree. Example: a
  recording whose 10-second middle clip has decode times 200 ticks apart
  from the source's in `ffprobe`'s lists.
- Why: composition offsets are copied as stored, and timestamps are not
  shifted by negative composition offsets; the same cause as `KD-CTTS`.
- Coverage requires: such an offset in the clip's table, and every packet of
  the kept run equal in data, size, keyframe flag and presentation time,
  with decode times differing by one constant, not 0: the source track's
  smallest composition offset less the clip's, each counted where
  negative, read from both files' `ctts`.

## KD-C-AUDIO-JITTER

- Field: the decoded audio.
- When: the source's audio stores decode durations that differ from the
  length its frames decode to, as some phones write AAC with timestamps
  that jitter (25, 41, 957, 1,065 ticks for frames of 1,024 samples each).
- What each reports: the clip copies the samples and their stored durations
  unchanged, and its packets agree with the source's in data and time.
  Decoded, every frame is 1,024 samples, so a position on the stored
  timeline is not a position in the decoded audio, and the clip's decoded
  audio cannot be lined up with the source's by time.
- Why: ISO/IEC 14496-12 (decode durations are stored per sample), and the
  clip copies them as stored; ISO/IEC 14496-3 (an AAC
  frame decodes to a fixed number of samples); the clip keeps one frame of
  pre-roll.
- Coverage requires: kept packets whose stored durations differ from the
  decoded frames' length (all decoded frames the same length, the media
  timescale the sample rate), and the clip's decoded audio equal to a
  contiguous run of the source's kept packets decoded from the clip's first
  kept packet, starting within its first two frames; one media edit on the
  output's audio, and the clip's audio timing the source's, as for
  KD-C-AUDIO-RESTART (the kept packets, the edit's media time and the
  stored decode durations).

## KD-C-AUDIO-EDIT-RATE

- Field: the decoded audio.
- When: the clip's audio is decoded at a multiple of its media timescale
  (HE-AAC with SBR: a media timescale of 24,000, decoded at 48,000), and
  its edit's media time is not a whole number of frames, which happens
  whenever the clip's start is not on an audio frame.
- What each reports: the clip's edit starts at its media time, in media
  ticks (ISO/IEC 14496-12); `ffmpeg` starts elsewhere. Measured by
  rewriting the media time of one clip's edit (frames of 1,024 ticks,
  decoded at twice the timescale): 500 ticks start at decoded sample 500,
  not 1,000; 1,100 and 1,500 ticks at 2,048, not 2,200 and 3,000; 2,500
  ticks at 4,096, not 5,000; 0, 1,024 and 2,048 ticks where they should.
  So `ffmpeg` drops the whole frames before the media time and skips the
  rest only within the first frame, counting it as decoded samples. The
  clip's audio is then up to one frame early against the source decoded
  from its start, and the decoder restarts as for `KD-C-AUDIO-RESTART`.
- Why: ISO/IEC 14496-12 (an edit's media time is in the media's
  timescale); ISO/IEC 14496-3 (SBR doubles the output sample rate over the
  core's); the audio edit's media time is the snapped start in the output's
  audio media ticks.
- Coverage requires: one media edit on the output's audio whose media time
  is not a whole number of the kept packets' one decode duration; a decoded
  sample rate that is a multiple, above one, of the media timescale; and
  the output's decoded samples equal to the source's kept packets,
  extracted by `ffmpeg` as ADTS frames (one per sample, edit lists ignored)
  and decoded from the first kept packet, at exactly the position that rule
  gives; and the clip's audio timing the source's, as for
  KD-C-AUDIO-RESTART (the kept packets, the edit's media time and the
  stored decode durations).

## KD-C-PTS-SHIFT

- Fields: the video packets' `pts` and `dts`, and the decoded frames'
  times.
- When: the source's video has no edit list and signed composition offsets
  whose smallest is negative (version 1 `ctts` or `trun`), and the clip
  keeps a negative one.
- What each reports: CoreAV and the clip present each sample at its stored
  composition time. For the source, without an edit list, `ffmpeg` moves
  every presentation time later by the size of the smallest offset, so that
  none precedes its decode time; for the clip, whose edit starts at the
  first presented sample, it keeps the presentation times and moves the
  decode times earlier by the clip's smallest offset (as for `KD-CTTS`).
  Every source time then exceeds the clip's by that size. Example: a
  fragmented HEVC recording with offsets down to −100 ticks, whose
  clips are listed 100 ticks earlier than the source in both `pts` and
  `dts`.
- Why: composition offsets are reported as stored, never shifted; ISO/IEC 14496-12
  (composition time is decode time plus the offset); the same cause as
  `KD-CTTS`.
- Coverage requires: no edit list on the source's video; the clip's
  smallest composition offset, read as signed, −D with D above 0; every
  packet of the kept run equal in data, size and keyframe flag, with the
  source's `pts` and `dts` above the clip's (shifted by the clip's start)
  by exactly D; and, for the frames, the same frames once D is applied to
  the source's times.

## KD-C-SYNC-VERIFIED

- Field: the video packets' keyframe flags.
- When: the source's container marks samples as sync that the coded data
  does not start from, and `ffprobe` reports the container's flag: HEVC
  without a sync sample table, or with one that lists every sample.
- What each reports: the clip marks as sync only the samples its cut rule
  allows, the samples the coded data agrees with
  (`docs/design/cut-points.md`, "The output's sync flags"): for HEVC, IRAP
  pictures. `ffprobe` flags every source packet, as the container states.
  Copying the source's flags instead made `ffmpeg` leave a packet of the
  run out of the clip it lists. Example: `clip_hevc_stss_all.mp4`, an IRAP
  picture every 25 frames and an `stss` that lists every sample, whose
  clip from 0.6 s to 1.6 s flags 2 of its 42 packets.
- Why: ISO/IEC 14496-12 8.6.2 (without a sync sample table every sample is
  a sync sample); ISO/IEC 14496-15 and ITU-T H.265 7.4.2.2 (an HEVC sync
  sample holds an IRAP picture, `nal_unit_type` 16 to 23). For H.264 and
  AV1, `ffprobe` flags keyframes from the bitstream and agrees directly.
- Coverage requires: the clip holds the source's packets of its run one for
  one, equal in data, size and times, starting with a keyframe; they differ
  only in flags the source's packets have and the clip's do not, at least
  one; and, with edit lists ignored, the clip's flagged packets are exactly
  the ones `ffmpeg` decodes as key frames.

## KD-C-SYNC-FLAGS (retired)

Retired. `clip` now starts only where the coded data agrees
with the sync flag, and writes the flags it agrees with
(`docs/design/cut-points.md`), so the clip's first packet is a keyframe for
`ffprobe` and every frame of the clip decodes. The case this entry covered
(a source that marks every sample as sync, a clip starting on a P frame)
no longer occurs.
