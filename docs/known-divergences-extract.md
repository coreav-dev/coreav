# Known divergences: audio extraction versus the oracles

Every intentional difference between an extracted M4A file and its source,
as the black-box oracles see them, is documented with a machine-readable
twin. The twin is
[`known-divergences-extract.json`](known-divergences-extract.json);
`scripts/extract_check.py` applies it with one predicate per entry, and
`tools/oracle/tests/test_extract_kd.py` keeps this page, the twin and the
predicates in sync. Each entry covers only the fields it names; an output
counts as agreement only if every difference is covered, and covered
outputs are reported apart from direct agreement.

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10. These
entries were registered after the release measurements of audio extraction
(`docs/spikes/audio-extract-bench/`).
Predicates use the files' own box headers and the oracles' output, never
file names. Probe divergences are listed in
[`known-divergences.md`](known-divergences.md).

## KD-X-TKHD

- Fields: MediaInfo `Duration` and `FrameCount`.
- When: the source's track header duration is not the sum of its edits (for
  example it leaves out a leading empty edit, or holds the movie duration),
  or describes no samples (a fragmented file) while the output's rounds the
  media duration to a coarse movie timescale.
- What each reports: MediaInfo reports each file's track header duration.
  The output's is the sum of its edit durations, or without an edit list its
  media duration rounded to the nearest movie tick. Example: a source track
  header of 3,426,871 ms beside a 20 ms empty edit and a 3,426,871 ms media
  edit; the output's is 3,426,891 ms.
- Why: ISO/IEC 14496-12 8.3.2 (the track duration is the sum of the
  durations of all of the track's edits), and the source's edit list is
  copied.
- Coverage requires: the output's track header equals that sum (or that
  rounding), the source's differs, MediaInfo's output duration equals the
  output's track header duration, and its source duration equals the
  source's track header duration or the media duration (the same in both
  files), each to the millisecond.

## KD-X-EDIT-DISCARD

- Field: the output's `nb_frames` against the source packet count.
- When: the edit list leaves some samples outside the presentation.
- What each reports: `ffprobe` omits those samples from its packet list, for
  the source and the output alike; `nb_frames` counts the sample table, and
  the output keeps every sample with the same edit list. Example: 495
  samples, 493 listed packets.
- Why: one packet per sample, never dropped, and the edit list is copied; the same behaviour as
  `KD-EDIT-DISCARD`.
- Coverage requires: equal `nb_frames` on both sides, equal packet lists on
  both sides (the packet check passes), and fewer listed packets than
  `nb_frames`.

## KD-X-EDIT-MULTI

- Field: the output's `nb_frames` against the source packet count.
- When: the source's edit list has several media edits.
- What each reports: `ffprobe` applies its own model to such a list
  (`docs/design/edit-lists.md`, "What `ffprobe` lists", rules 7 to 9): it
  lists a preroll sample before each media edit and repeats samples at the
  edits' boundaries, so it lists more packets than the track has samples —
  for the source and the output alike, since the output carries the same
  entries. `nb_frames` counts the sample table, and the output keeps every
  sample. Example: 432 samples, 499 listed packets.
- Why: one packet per sample, never dropped; every entry of
  the edit list is copied, with its media time moved by the first decode
  time (`docs/design/edit-list-writers.md`, "M4A extraction").
- Coverage requires: the source's list has several media edits; the output's
  movie and media timescales are the source's and its entries are the
  source's with every media time moved by the source's first stored decode
  time, read from both files; equal `nb_frames` on both sides; equal packet
  lists on both sides (the packet check passes); and more listed packets
  than `nb_frames`.

## KD-X-GAPLESS-EDIT

- Fields: stream `start_pts`, `start_time`, `duration_ts`, `duration`, the
  packet times and the framemd5 times.
- When: the source has no edit list and gives its encoder delay and padding
  in an `iTunSMPB` tag that adds up exactly to its samples, so the output
  carries an edit list made from the tag.
- What each reports: `ffmpeg` applies the source's tag by discarding the
  delay while keeping the source's timeline, so the first content sample is
  presented at the delay (2,112 ticks, 48 ms). The output's edit list maps
  the delay to presentation time 0. The decoded audio is the same, frame for
  frame; every time differs by exactly the delay, and the duration is the
  content length on the output and the whole track on the source.
- Why: a tag that adds up exactly stands for an edit list, which the output
  carries; ISO/IEC 14496-12 8.6.6 (an edit
  maps a media time to the start of the presentation).
- Coverage requires: the tag's delay, padding and length add up to the
  source's media header duration; the output's single edit starts at the
  delay; source times equal output times plus the delay, with equal data
  hashes, sizes, frame durations and frame hashes; the source duration is
  the whole track and the output's the tag's length, in a time base of one
  over the sample rate.

## KD-X-GAPLESS-CONFLICT

- Field: framemd5.
- When: the source has an edit list and an `iTunSMPB` tag that disagree: the
  tag does not add up to the samples (iTunes previews whose tag describes
  the whole original recording).
- What each reports: the output keeps the edit list; `ffmpeg` applies the
  tag on the source, so it decodes the first and last frames differently
  (two frames more on the output in the corpus).
- Why: the edit list is copied, and a tag that does not add up is ignored.
- Coverage requires: the tag does not add up to the media header duration,
  the output has the source's edit list, and every source frame except the
  first and the last appears, in order and unchanged, in the output.

## KD-X-FRAG-LAST-FRAME

- Field: framemd5.
- When: the source is fragmented and its last sample's stored duration is
  shorter than the frame it holds (the end of the encoded audio, as
  `ffmpeg` 9 writes it in a fragment's track run).
- What each reports: the output stores the same duration in its sample
  table, and both files list it as the last packet's duration in `ffprobe`
  9.0.2. `ffmpeg` 9.0.2 cuts the last decoded frame to that duration on the
  output but not on the fragmented source, so only that frame differs
  (1,024 samples on the source, 136 on the output in the fixtures).
  `ffmpeg` 7.0.2 cut neither.
- Why: the sample durations are copied (ISO/IEC 14496-12 8.6.1.2 and
  8.8.8); which of the two readers of the same black box applies a
  duration to the decoded audio is not the writer's choice.
- Coverage requires: the source declares fragments (`mvex`); every frame
  but the last is equal, the last at the same times; the last packet's
  duration is the same in both files, the output's last frame lasts exactly
  that, and the source's last frame lasts longer.

## KD-X-PROFILE-UNNAMED

- Field: stream `profile`.
- When: `ffprobe` names no profile for the source's AAC stream, and names
  `LC` for the output.
- What each reports: the output's configuration is the source's, byte for
  byte; `ffprobe` names the profile only in the one-stream output.
- Why: the `esds` is copied as stored; the same cause
  as `KD-AAC-LC-UNNAMED`.
- Coverage requires: `aac` on both sides, no source profile, output `LC`.
  The coreav round trip compares the configuration bytes separately.

## KD-X-SBR-TIMESCALE

- Fields: stream `time_base`, `start_pts` and `duration_ts`, and the
  packet times.
- When: an HE-AAC source whose media timescale is its core's rate (22,050
  for a stream decoded at 44,100, say): the output is written at the
  decoded rate (`docs/design/edit-list-writers.md`, "HE-AAC timescale").
- What each reports: `ffprobe` gives each file's media timescale as its
  time base, so every time of the output is the source's times a whole
  number (2 for SBR's doubled rate); the times in seconds, the packets and
  the decoded audio are the same.
- Why: ISO/IEC 14496-12 12.2.3.3 (an audio timescale matches the sampling
  rate or is an integer multiple of it); ISO/IEC 14496-3 1.6.5.2 (an HE-AAC
  decoder outputs SBR's rate). The box checks verify the multiplied
  durations themselves (`writer_boxes.sbr_scale`).
- Coverage requires: `ffprobe`'s sample rate for the source (the decoded
  rate) equal to the output's media timescale and sample rate, and a whole
  multiple above 1 of the source's media timescale; time bases of one over
  each file's media timescale; the output's start, duration and every
  packet's times equal to the source's times that multiple, with the same
  packet hashes and sizes.
