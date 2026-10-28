# Known divergences: full-file remuxes versus the oracles

Every intentional difference between a remuxed file and its source, as the
black-box oracles see them, is documented with a machine-readable twin. The twin is
[`known-divergences-remux.json`](known-divergences-remux.json);
`scripts/remux_check.py` applies it with one predicate per entry, and
`tools/oracle/tests/test_remux_kd.py` keeps this page, the twin and the
predicates in sync. Each entry covers only the fields it names, and none
covers the checker's box-level comparisons (`box:` fields); an output
counts as agreement only if every difference is covered, and covered outputs
are reported apart from direct agreement. The remux itself is described in
[`design/remux.md`](design/remux.md).

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10.
Predicates use the files' own box headers and the oracles' output, never
file names.

## KD-R-FRAG-FRAMES

- Field: a stream's `nb_frames`.
- When: the source is fragmented (it has movie fragments, `moof`).
- What each reports: `ffprobe` counts the samples of the movie box's sample
  tables only: none for a source whose movie box is empty, or those before
  the first fragment. The output is not fragmented, so its count is every
  copied sample. Example: `fragmented_moov.mp4`, 25 video samples in the
  movie box and 50 in all.
- Why: the remux writes one sample table with every sample of the stream
  (`design/remux.md`, "Fragmented sources").
- Coverage requires: a fragmented source, the output's `nb_frames` equal to
  the number of packets `ffmpeg` copies from the output stream, and the
  source's `nb_frames` absent or smaller.

## KD-R-FRAG-DURATION

- Field: a packet's duration in the `framemd5` checksums.
- When: the source is fragmented and a sample's stored duration (`trun` or
  the fragment defaults) is not what `ffmpeg` reports for it. Example: the
  first AAC sample of `fragmented.mp4` is stored with 3,528 ticks, which
  reach the next sample's decode time; `ffmpeg` reports 1,024 for the source
  and 3,528 for the output.
- What each reports: the output's sample table stores the duration the
  source stores, and `ffmpeg` reports that duration for the output.
- Why: the remux copies each sample's stored duration (ISO/IEC 14496-12
  8.8.8, 8.6.1.2), so decode times stay those of the source.
- Coverage requires: a fragmented source, packets equal in every other
  field (stream, decode and presentation times, size, checksum, side data),
  and the output's duration equal to the gap to the output's next packet.

## KD-R-FRAG-EDIT-END

- Fields: a stream's `duration_ts`; MediaInfo's `Duration` and
  `FrameCount` of a track.
- When: the source is fragmented and the track's edit list ends with a
  media edit of duration 0, which in a fragmented file lasts to the end of
  the media (ISO/IEC 14496-12 8.6.6), with a media time above 0 (encoder
  priming, for example).
- What each reports: `ffprobe` and MediaInfo take the source's duration as
  the whole media; the output, no longer fragmented, states the edit's
  length, the media from the edit's media time to its end (rounded to the
  movie timescale), and both report that. Examples: an AAC track with a
  media time of 1,024 ticks at 44.1 kHz, which MediaInfo reports as
  160.078 s and 6,894 frames for the source and 160.055 s and 6,893 for the
  output; an H.264 track with a media time of 8,008 ticks at 96 kHz, which
  `ffprobe` reports as 143,006,864 ticks for the source and 142,998,880 for
  the output.
- Why: `design/remux.md`, "Fragmented sources": the remux writes such an
  edit's length explicitly.
- Coverage requires: a fragmented source whose stream ends its edit list
  with such an edit (read from the source's box headers), and either
  `ffprobe`'s `duration_ts` values apart by the media time within one movie
  tick, or MediaInfo's durations apart by the media time (within 1.5 ms)
  and its frame counts apart by that gap times the output's frame rate
  (within one frame, MediaInfo rounding each count, or two where the output's last sample is a partial frame).

## KD-R-FRAG-STATED-EDIT

- Fields: MediaInfo's `Duration` and `FrameCount` of a track.
- When: the source is fragmented and the track's edit list states its
  length (no edit of duration 0).
- What each reports: MediaInfo reports the source's duration from its
  samples and ignores the edit list; the output, no longer fragmented,
  carries the same edit list and a track duration that sums it, which
  MediaInfo reports. Example: an HEVC track whose samples last 122.4224 s
  and whose edit states 122.423 s; MediaInfo reports 122.422 s for the
  source and 122.423 s for the output.
- Why: the edit list is copied as stored and the track duration is the sum
  of the edits (ISO/IEC 14496-12 8.6.6, 8.3.2; `design/remux.md`,
  "Fragmented sources").
- Coverage requires: a fragmented source whose stream's edits all state a
  duration; MediaInfo's source duration equal to the samples' (the output's
  `stts` sum) and its output duration equal to the output's edits, each
  within 0.5 ms; frame counts apart by the gap times the frame rate, within
  one frame (two where the last sample is a partial frame).

## KD-R-LAST-DURATION

- Field: the duration of a stream's last packet in the `framemd5`
  checksums.
- When: the source is not fragmented and declares a media duration
  (`mdhd`) shorter than its sample durations (`stts`) add up to; the remux
  copies both as stored, so the output declares the same.
- What each reports: `ffmpeg` reports the stored duration of the last
  sample in one file and, in the other, the duration up to the media
  header's end, depending on where that sample lies in the file. Example:
  an AAC track whose `stts` sums to 388,096 ticks and whose media header
  says 388,095; the last packet is 1,024 ticks in the source and 1,023 in
  the output.
- Why: the declared durations are copied, also where they disagree with
  the samples (`design/remux.md`, "Samples and
  timing").
- Coverage requires: an unfragmented source, the packet the last of its
  stream and equal in every other field, both files with the same `stts`
  and media header duration, that duration shorter than the `stts` sum, and
  the two reported durations exactly the stored last duration and the
  media header's end minus the other samples.

## KD-R-BASE-TIME

- Fields: packets' decode and presentation times in the `framemd5`
  checksums, a stream's `start_pts` and `duration_ts`; MediaInfo's
  `Duration` and `FrameCount`.
- When: the source is fragmented, the stream has no edit list, and its first
  decode time `d0` (from the first fragment's base decode time) is above 0.
  The output's decode times start at 0, so the remux writes an empty edit
  `E` of that length, rounded to the movie timescale, followed by the media
  from time 0: each sample keeps its presentation time (ISO/IEC 14496-12
  8.6.6). The movie timescale of a fragmented source's output is a common
  multiple of the movie and media timescales, so `E` is exact.
- What each reports: `ffmpeg` presents the output's first packet at the
  end of the empty edit, the source's at `d0` plus the packet's
  composition offset, so every time differs by `E` (in media ticks) minus
  `d0` minus that offset: one frame's composition offset, plus the
  rounding of `E`. It counts the source's duration from time 0 and the
  output's from the start of the media. MediaInfo counts the output's empty
  edit in its duration and its frame count. Example: a 10 s base decode time on a 6,000 Hz
  video track with a composition offset of 100 ticks; `ffmpeg` reports the
  first packet at 60,100 in the source and 60,000 in the output, and a
  duration of 3,660,000 and 3,600,000 ticks; MediaInfo 600 s and 610 s.
  `ffprobe` reports the same start time (10 s) for both files.
- Why: `design/remux.md`, "Fragmented sources": the remux keeps every
  sample's presentation time; dropping the offset would move the whole
  stream.
- Coverage requires: a fragmented source whose stream has no edit list, an
  output edit list of exactly an empty edit and a media edit from time 0,
  and: for packets, the times apart by exactly that amount (the same for
  decode and presentation times, every other field equal); for
  `start_pts`, the output's equal to `E` and the source's within its first
  sample (its composition offset plus its duration) after it; for
  `duration_ts`, the source's larger by `d0` plus at most that same span;
  for MediaInfo, the durations apart by `E` (within 1.5 ms) and the frame
  counts by `E` times the frame rate (within one frame, MediaInfo rounding each count, or two where the output's last sample is a partial frame).

## KD-R-PROBE-WINDOW

- Fields: a stream's `profile`, `codec_name`, `sample_fmt`,
  `channel_layout` or `bits_per_sample`, and the `#codec_id` line of the
  `framemd5` header.
- When: the source stores a stream's first packets beyond the data
  `ffprobe` reads to identify streams (5 MB or 5 s by default): behind a
  large key frame, or after a long run of another stream's samples. The
  remux stores chunks that start together smallest first, so a stream's
  first packets come early; some sources do not.
- What each reports: for the source, `ffprobe` leaves the parameters it
  takes from those packets unset or less precise (an AAC profile
  `unknown`, MPEG audio `mp3` for Layer II); for the output it reports
  them. Example: a QuickTime file whose AAC profile `ffprobe`
  reports only for the output.
- Why: `design/remux.md`, "Output layout": the layout decides where a
  stream's first packets lie; the packets themselves are the same.
- Coverage requires: `ffprobe` with `-probesize 2000M -analyzeduration
  2000M` (a window over the whole file) giving both files the same value,
  and that value being the one `ffprobe` gives the output by default (the
  output is never the file that misses it); for the `framemd5` header,
  only `#codec_id` lines differing, the output's naming the whole-file
  probe's codec.

## KD-R-VFR-RATE

- Field: a stream's `r_frame_rate`.
- When: the stream has more than one decode duration (variable frame
  rate).
- What each reports: `ffprobe`'s `r_frame_rate` is an estimate from the
  frames it analyzes before it stops probing, which depends on how the
  streams are interleaved; it differs although the stored durations are the
  same. Example: 60/1 for the source and 60000/1001 for the output of a
  recording with 2,681 distinct runs of decode durations.
- Why: the decode durations are copied (ISO/IEC 14496-12 8.6.1.2);
  `r_frame_rate` is not stored in the file.
- Coverage requires: the same `stts` entries in both files, with more than
  one decode duration, and the same `avg_frame_rate` (which `ffprobe`
  computes from the frame count and the duration).

## KD-R-TIMECODE-TAG

- Field: the `timecode` tag of a stream or of the file.
- When: the source has a QuickTime timecode track, which the remux drops
  and reports.
- What each reports: `ffprobe` shows the timecode track's start as a
  `timecode` tag of the video stream it refers to, and of the file; without
  the timecode track the output has no such tag.
- Why: MP4 has no timecode track; the remux drops streams it cannot carry
  and lists them (`design/remux.md`, "Streams").
- Coverage requires: the tag in the source and not in the output, and a
  source stream with the `tmcd` sample entry among the reported dropped
  streams.

## KD-R-CHAN

- Field: MediaInfo's `ChannelLayout` and `ChannelLayout_Original` of an
  audio track.
- When: the source's sound sample entry has a QuickTime channel layout box
  (`chan`).
- What each reports: MediaInfo takes the source's layout from `chan` (for
  example `C` for a mono layout) and the output's from the AAC
  configuration (`M`). `ffprobe`'s `channel_layout` is the same for both.
- Why: `chan` is defined by the QuickTime file format, not for MP4 sound
  entries (ISO/IEC 14496-12 defines `chnl` instead); the remux leaves it out
  and reports it (`design/remux.md`, "Sample entries").
- Coverage requires: a `chan` box among the source sound entry's children
  and none in the output's.
