# Known divergences: joins versus the oracles

Every intentional difference between a join and its inputs, as the
black-box oracles see them, is documented with a machine-readable twin. The twin is
[`known-divergences-join.json`](known-divergences-join.json);
`scripts/join_check.py` applies it with one predicate per entry, and
`tools/oracle/tests/test_join_kd.py` keeps this page, the twin and the
predicates in sync. A join counts as agreement only if every difference is
covered; covered joins are reported apart from direct agreement. Which
packets the output holds, and in which order, has no known divergence; a
join of a source's segments must give back the source with none, but for
the clip's `KD-C-AAC-PROFILE` ([`known-divergences-clip.md`](known-divergences-clip.md)),
which applies to joins as it is: `ffprobe` names the AAC profile from what
it decodes at the stream's start, so the same configuration bytes can be
named differently in two files whose layouts differ.

The output of a join holds one edit per media edit of each input and track
([`docs/design/join.md`](design/join.md)). ISO/IEC 14496-12 defines what an
edit list of several edits presents. `ffmpeg` 7.0.2 applies the first edit
exactly and edits after it approximately; every entry below but
`KD-J-DTS-SHIFT` is one of those approximations, recorded from its observed
behaviour (the binaries are black boxes).

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10.
Predicates use the files' own box headers and the oracles' output, never
file names.

## KD-J-VIDEO-LATER-EDIT

- Fields: the video packets' times and discard flags, and the decoded video.
- When: the output's video has more than one media edit, and the inputs
  have composition delays (an edit's media time after the input's first
  decode time, as with B-frames).
- What each reports: the edits present each input's frames at its own times
  plus its offset. `ffmpeg` presents the frames of an input after the first
  by one constant off: later by up to that input's composition delay, or
  earlier by up to the first input's; a frame the shift would put before the
  previous frame is shown at that frame's time. It lists their packets with
  other times, some marked discarded or listed twice. Examples: two clips of
  `vfr.mp4`, the second's frames 1,024 ticks (80 ms) late; two clips of a
  recording whose first clip's delay is 7,245 ticks and second's
  6,000, the second's frames 1,245 ticks early, its first frame shown at the
  first clip's last frame's time.
- Why: ISO/IEC 14496-12 (each edit maps its media time to its start in the
  presentation); `docs/design/join.md` (one edit per input).
- Coverage requires: no packet missing or extra, every stored sample
  listed once (copies at the same byte position dropped), in the inputs'
  order, with their data, size and keyframe flag; the first input's times
  exact; at least one later input's times differing, and for each that
  does, every presentation time off by one constant other than 0, within
  the same bounds as its frames, and every decode time off by that constant
  plus the input's `KD-J-DTS-SHIFT` difference (0 unless the output's
  smallest composition offset, which must be the smallest of the inputs'
  own, is negative and below the input's); the output's edit for each
  input whose times or frames move the input's own: starting at its offset
  plus the time before its own first media edit, from its own first media
  time moved to where its first sample is stored in the output (read from
  the input's file and from both files' decode times as `ffprobe` lists
  them with edit lists ignored, its `KD-J-DTS-SHIFT` move added back); and
  the decoded video the
  inputs' frames in order with their checksums, the first input's at its
  times, every later input's shifted by one constant, at least minus the
  first input's composition delay and at most its own, each frame the shift
  would put before the previous one at the previous one's time.

## KD-J-VIDEO-EDIT-END

- Field: the decoded video.
- When: an input before another ends its video edit inside its last
  presented frame (a clip cut between two frames).
- What each reports: the edit cuts that frame short, and the next input's
  first frame is presented at the edit's end. `ffmpeg` shows the cut frame
  for its whole duration (as it decodes the input alone, or until the
  input's stored frames end) and the next input's first frame after it, and
  may list that frame twice. Examples: a clip of `av1.mp4` ending at 0.7 s,
  half way through a frame, then another clip; a clip of a variable frame
  rate recording whose frames run 10 ms past its edit, the next clip's first
  frame shown twice from there.
- Why: ISO/IEC 14496-12 (an edit presents its media for its duration);
  `docs/design/join.md` (each input keeps its own edit).
- Coverage requires: every packet agreeing; the decoded video the inputs'
  frames, but for each such next input's first frame, presented at the end
  of the cut frame's full duration (its duration as `ffmpeg` decodes the
  input, or the end of the input's stored frames) instead of the input's
  start, and possibly listed twice.

## KD-J-AUDIO-EDIT-START

- Fields: the audio packets' times, and the decoded audio.
- When: the output's audio has more than one media edit, and an edit after
  the first starts at a media time inside a frame (a clip's one frame of
  pre-roll plus the part of a frame before its start, say).
- What each reports: the edit presents its media from its media time.
  `ffmpeg` drops the frame that holds that media time and presents the next
  frame from the edit's start, so that input's audio comes earlier by the
  distance to that frame boundary, less than a frame. Example: two clips of
  a 44.1 kHz file; the second's edit starts 68 samples into a frame, and
  `ffmpeg` presents its audio 956 samples (22 ms) early.
- Why: ISO/IEC 14496-12 (an edit presents its media from its media time);
  `docs/design/join.md` (one edit per input, the input's own priming and
  pre-roll left unpresented).
- Coverage requires: no packet missing or extra, and every packet equal in
  data, size and keyframe flag; the first input's times within a tick; at
  least one later input's times differing, and for each that does, every
  `pts` and every `dts` off by one constant above 0 and below a frame (the
  inputs' most common packet duration: 1,024 samples, 2,048 where SBR
  doubles the rate), equal within less than a tick (an offset that falls
  between ticks) to the distance from the media time of the input's own
  edit, inside a packet, to the input's next packet start; the
  output's edit for each such input the input's own, as for
  `KD-J-VIDEO-LATER-EDIT`; and every decoded frame of every edit equal to the inputs' packets
  decoded in sequence, each edit after the first read from the first frame
  boundary at or after its media time.

## KD-J-AUDIO-NOISE

- Field: the decoded audio.
- When: the output has an audio edit after the first, and the inputs' AAC
  decodes differently with another decoder history (perceptual noise
  substitution, whose noise depends on it).
- What each reports: before an edit after the first, `ffmpeg`'s demuxer
  delivers some packets again (packets from before the edit's media time,
  flagged discard, and at times the stream's first packet), decodes them
  and drops their output. Its decoder's history at the edit then differs
  from the reference's (the inputs' packets decoded in sequence), and so
  does the noise it decodes from there on. The output's samples are those
  packets decoded in the order `ffmpeg` delivers them, bit for bit.
  Examples: the fixtures' AAC, encoded with perceptual noise substitution,
  differ from the reference by at most 7.0e-4 near the end of their test
  tone; a recording's second clip differs by up to 0.18 at a peak of
  0.9 in two thirds of its frames, and equals the delivered packets decoded
  in that order in every frame.
- Why: ISO/IEC 14496-3 (perceptual noise substitution fills bands with
  noise the bitstream does not define, so the decoded samples depend on
  the decoder's history); ISO/IEC 14496-12 (an edit's media is the samples
  from its media time; packets delivered again are not part of the
  output); the same cause as `KD-C-AUDIO-RESTART` and
  `KD-S-AUDIO-RESTART`.
- Coverage requires: every decoded frame of every edit equal, sample for
  sample, to the output's packets decoded in the order `ffprobe` lists
  them under the output's edits (each packet placed at its first delivery
  not flagged discard), at the media time the edit gives it, every edit
  after the first read from the first frame boundary at or after its media
  time (as for `KD-J-AUDIO-EDIT-START`), within one sample.

## KD-J-AUDIO-HELD-FRAME

- Field: the decoded audio.
- When: the output's AC-3 or E-AC-3 audio has more than one media edit, and
  an edit after the first starts at a media time inside a frame.
- What each reports: the edit presents its media from its media time.
  `ffmpeg` presents the whole frame that holds that media time inside the
  edit before it, ahead of the edit's start; the next frame is
  presented from the edit's start, as for `KD-J-AUDIO-EDIT-START`. That
  frame, and those after it, decode as `ffmpeg` delivers the packets
  (`KD-J-AUDIO-NOISE`): AC-3's dither for zero-bit mantissas depends on the
  decoder's history. Example: two clips of `av_ac3.mp4`; the second's edit
  starts 1,348 samples into a frame, and `ffmpeg` presents that frame from
  1,536 samples before the edit's start; `av_ac3.mp4` concatenated with
  itself, 904 samples before.
- Why: ISO/IEC 14496-12 (an edit presents its media from its media time);
  `docs/design/join.md` (one edit per input, the input's own pre-roll left
  unpresented).
- Coverage requires: for each edit after the first, exactly one decoded
  frame presented in the edit before it, ahead of the edit's start, that
  equals, sample for sample, the whole frame of the packet that holds the edit's media
  time, decoded in the order `ffprobe` lists the packets under the
  output's edits (as for `KD-J-AUDIO-NOISE`); and every other decoded frame
  of every edit equal to the packets decoded in that order, each edit
  after the first read from the first frame boundary at or after its media
  time (as for `KD-J-AUDIO-EDIT-START`), within one sample.

## KD-J-DTS-SHIFT

- Field: the video packets' `dts`.
- When: the output's video has composition offsets that a reader taking
  them as signed sees as negative, and an input's smallest offset is
  larger than the output's (another input brings the most negative one).
- What each reports: CoreAV keeps every decode time as stored. `ffprobe`
  moves a track's decode times back by its smallest composition offset
  when that is negative: by the output's for the output, by the input's
  own for each input. Every decode time of such an input then differs in
  the output by one constant, the difference of the two, while
  presentation times agree. Example: the fixture `chapters.mov` (offsets of
  0 or more) followed by `negative_cts.mp4` (down to −1024 ticks at 12,800
  per second): every decode time of the first input is 1,024 ticks lower in
  the output.
- Why: composition offsets are copied as stored; the
  same cause as `KD-C-DTS-SHIFT` and `KD-S-DTS-SHIFT`.
- Coverage requires: no packet missing or extra and none differing in
  data, size or keyframe flag; presentation times equal; the output's
  smallest offset equal to the smallest of the inputs' own, read from the
  input files; and for every input whose decode times differ, every decode
  time off by one constant, equal to the output's smallest offset minus the
  input's (each counted only when negative), above 0.

## KD-J-PTS-SHIFT

- Fields: a join of a source's segments, against the source: the video's
  start time, its packets' `pts` and `dts`, and its decoded frames' times.
- When: the source's video has no edit list and composition offsets whose
  smallest, read as signed, is negative (version 1 `ctts` or `trun`).
- What each reports: CoreAV, the segments and the joined file present each
  sample at its stored composition time. For the source, without an edit
  list, `ffmpeg` moves every time later by the size of the smallest offset;
  the joined file has an edit list, and keeps them. Every source time then
  exceeds the joined file's by that size. Example: a fragmented recording at
  6,000 ticks a second with offsets down to −100 ticks, whose join of its
  segments starts 1/60 s before the source in `ffprobe`.
- Why: composition offsets are reported as stored, never shifted; ISO/IEC 14496-12 (composition time is decode time
  plus the offset); the same cause as `KD-C-PTS-SHIFT` and `KD-S-PTS-SHIFT`.
- Coverage requires: no edit list on the source's video; the joined video's
  smallest composition offset, read as signed, −D with D above 0 (it holds
  every sample, so its smallest offset is the source's); the start times D
  apart; every packet equal in data, size and keyframe flag with `pts` and
  `dts` exactly D apart in a shared time base; and the same frames once D
  is applied.

## KD-J-SYNC-VERIFIED

- Field: a join of a source's segments, against the source: the video
  packets' keyframe flags.
- When: the source's container marks samples as sync that the coded data
  does not start from, and `ffprobe` reports the container's flag: HEVC
  without a sync sample table, or with one that lists every sample.
- What each reports: the segments, and so their join, mark as sync only
  the samples the cut rule allows (`docs/design/cut-points.md`, "The
  output's sync flags"): for HEVC, IRAP pictures. `ffprobe` flags every
  source packet, as the container states. Example: `clip_hevc_stss_all.mp4`,
  an IRAP picture every 25 frames and an `stss` that lists every sample,
  whose joined segments flag 4 of its 100 packets.
- Why: ISO/IEC 14496-12 8.6.2 (without a sync sample table every sample is
  a sync sample); ISO/IEC 14496-15 and ITU-T H.265 7.4.2.2 (an HEVC sync
  sample holds an IRAP picture, `nal_unit_type` 16 to 23); the same cause
  as `KD-C-SYNC-VERIFIED` and `KD-S-SYNC-VERIFIED`.
- Coverage requires: every source packet in the joined file, one for one,
  in order, equal in data, size and times; the joined file starting with a
  keyframe; differences only in flags the source's packets have and the
  joined file's do not, at least one; and, with edit lists ignored, the
  joined file's flagged packets exactly the ones `ffmpeg` decodes as key
  frames.

## KD-J-AUDIO-LAST-CUT

- Field: the decoded audio of a join of a source's segments.
- When: the source's last AAC sample is stored shorter than its frame (the
  end of the encoded audio), and `ffmpeg` 9.0.2 reads padding to discard
  on that frame in one of the two files only: in the joined file's sample
  table and not in a fragmented source's track run (`fragmented.mp4`),
  or at the joined file's edit end and not at the end of a source whose
  edit list starts with an empty edit (`edit_list.mp4`).
- What each reports: every packet and every frame but the last is the
  same; the last frame starts at the same time, and `ffmpeg` decodes it
  cut to the end the side data gives in one file (136 or 133 samples) and
  whole (1,024) in the other. `ffmpeg` 7.0.2 cut it in neither.
- Why: the join copies the samples and their stored durations, and the
  last segment's edit ends where the source's media does; which of the
  files `ffmpeg` reads Skip Samples side data from is its own. The same
  cause as KD-R-FRAG-LAST-TRIM and KD-X-FRAG-LAST-FRAME.
- Coverage requires: no packet difference; every decoded frame but the
  last equal at the same time, the last ones starting at the same time;
  and Skip Samples on the last audio packet of exactly one of the files,
  skipping nothing and discarding exactly what that file's last frame
  lacks of the other's.
