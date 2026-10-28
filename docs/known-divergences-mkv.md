# Known divergences: `coreav-mkv` versus `ffprobe`

Every intentional difference from the `ffprobe` oracle is documented, with
a machine-readable twin. The twin is
[`known-divergences-mkv.json`](known-divergences-mkv.json);
`scripts/oracle_kd_mkv.py` applies it to the oracle reports of
`tools/mkv-adapter`, and `tools/oracle/tests/test_kd_mkv.py` keeps this page,
the twin and the predicates in sync. Each entry covers only the fields it
names; an item counts as agreement only if every gating difference is
covered. Covered items stay in the denominator and are reported apart from
direct agreement. Predicates read the two captures, never file names;
KD-MKV-DECLARED-SIZE and KD-MKV-TAG-LANGUAGE also read the file's EBML
elements, and `scripts/compat_check.py` applies KD-MKV-TAG-LANGUAGE to the
`--ffprobe-compat` output, as the stream oracle does not gate it. The
oracle is `ffprobe` 7.0.2. The design is in [`design/mkv.md`](design/mkv.md).

## KD-MKV-DTS

- Field: packet `dts`.
- When: H.264, HEVC, MPEG-1, MPEG-2 or MPEG-4 Part 2 video, or Theora: the
  codecs whose decode order can differ from presentation order.
- What each reports: CoreAV reports no decode timestamp. `ffprobe` derives
  one from the presentation timestamps and the codec's reorder delay, and
  prints none for the first frames.
- Why: Matroska stores presentation timestamps only (RFC 9559 11.2); the
  container gives no decode time for these codecs. For codecs that decode in
  presentation order CoreAV reports the decode time, equal to the
  presentation time, and agrees with `ffprobe`.
- Coverage requires: one of those codecs, a candidate `dts` of `null` and a
  reference `dts`, and no other differing field of the packet.

## KD-MKV-LACE-UNTIMED

- Fields: packet `pts` and `dts`.
- When: a frame after the first of a laced block, on a track without
  `DefaultDuration`.
- What each reports: CoreAV reports no timestamp. `ffprobe` adds the
  previous frame's duration, which it finds by parsing the codec (for example
  the Opus packet header).
- Why: RFC 9559 10.3.5: "each frame after the first one has an
  underdetermined timestamp"; only `DefaultDuration` gives one at the
  container level.
- Coverage requires: lace position above 0, no `DefaultDuration`, both
  candidate timestamps `null`, a reference `pts`, and no other differing
  field.

## KD-MKV-LACE-SPLIT

- Fields: packet `pts` and `dts`.
- When: a frame after the first of a laced block, on a track with
  `DefaultDuration`, whose exact start does not fall on a tick boundary.
- What each reports: CoreAV reports the block timestamp plus the frame's
  position times `DefaultDuration`, truncated to the tick in which the frame
  starts, minus `CodecDelay` rounded to the nearest tick. `ffprobe` first
  rounds the duration of the whole lace down to ticks and splits that among
  the frames, so its value can be one tick earlier. Example: eight frames of
  21.333 ms; the eighth starts at 149.333 ms, CoreAV reports tick 149 and
  `ffprobe` 148.
- Why: RFC 9559 10.3.5 and 11.1; the exact time is the block time plus the
  frame's position times `DefaultDuration` (`Packet::pts_ns` gives it in
  nanoseconds).
- Coverage requires: both values recomputed from the candidate's block
  timestamp, lace position, frame count, `DefaultDuration`, `CodecDelay` and
  `TimestampScale`, each formula matching its side, a difference of exactly
  one tick, and decode timestamps equal to the presentation timestamps.

## KD-MKV-AUDIO-KEYFRAME

- Field: packet `keyframe`.
- When: an audio track whose SimpleBlocks do not set the keyframe flag (or
  whose BlockGroups carry a `ReferenceBlock`).
- What each reports: CoreAV reports the flag as stored (not a keyframe).
  `ffprobe` marks every audio frame a keyframe.
- Why: RFC 9559 10.4: a frame is a random access point when the SimpleBlock
  keyframe flag is set or the BlockGroup has no `ReferenceBlock`; CoreAV
  reports what the file says.
- Coverage requires: an audio stream, candidate `false`, reference `true`, and
  no other differing field.

## KD-MKV-VFW-TIMESTAMPS

- Fields: packet `pts` and `dts`.
- When: a `V_MS/VFW/FOURCC` (Video for Windows) track.
- What each reports: CoreAV reports the block timestamp as the presentation
  timestamp and no decode timestamp. `ffprobe` reports it as the decode
  timestamp and prints no presentation timestamp.
- Why: RFC 9559 11.2: block timestamps are presentation timestamps, for
  every codec.
- Coverage requires: the codec ID, a reference `pts` of `null`, a candidate
  `dts` of `null`, and the reference `dts` equal to the candidate `pts` and
  block timestamp.

## KD-MKV-WEBM-WEBVTT

- Field: packet `size`.
- When: a WebM WebVTT track (`D_WEBVTT/...` codec IDs).
- What each reports: CoreAV reports the frame as stored: the cue identifier
  line, the cue settings line and the cue payload. `ffprobe` reports the
  payload only and moves the two lines into side data.
- Why: one packet is one stored frame; the WebM WebVTT layout
  stores the two lines in the frame (Matroska codec mappings 5.4.6).
- Coverage requires: a `D_WEBVTT/` codec ID, a candidate size at least two
  bytes (two line feeds) larger, and equal timestamps.

## KD-MKV-ENCRYPTED

- Field: `extra` packets.
- When: a track whose `ContentEncoding` is encryption.
- What each reports: CoreAV lists every stored frame, with the track marked
  encrypted. `ffprobe` omits the frames whose WebM signal byte marks them
  encrypted.
- Why: one packet per stored frame; the frames' byte ranges
  are exact even though their content is opaque.
- Coverage requires: the candidate track's encoding is `encrypted`; paired
  packets must agree on their own.

## KD-MKV-STREAM-DURATION

- Field: stream `duration_ts`.
- When: `ffprobe` gives a stream the segment duration; it does so for
  subtitle tracks and Video for Windows tracks.
- What each reports: CoreAV reports no stream duration. `ffprobe` reports the
  segment duration.
- Why: Matroska stores no per-track duration (RFC 9559 5.1.2.10 defines the
  segment's only); the segment duration is not a property of the track.
- Coverage requires: a candidate `null` and a reference value equal to the
  reference format duration.

## KD-MKV-FMT-ROUNDING

- Field: `format_duration_us`.
- When: `Duration` times `TimestampScale` is not a whole number of
  microseconds.
- What each reports: CoreAV rounds it to the nearest microsecond, halves away
  from zero, as the oracle schema rounds every duration. `ffprobe` truncates.
  Example: a browser recording of 2,980,208.5 µs; CoreAV reports 2,980,209,
  `ffprobe` 2,980,208.
- Why: `tools/oracle/SPEC.md` (microsecond rounding); RFC 9559 11.3 (the
  nearest nanosecond).
- Coverage requires: the exact value recomputed from the candidate's stored
  `Duration` and `TimestampScale`, the candidate equal to its rounding and
  the reference equal to its truncation.

## KD-MKV-UNREAD-BITSTREAM

- Fields: stream `profile`, `level`, `color_range`, `color_primaries`,
  `color_transfer`, `color_matrix`.
- When: MPEG-1, MPEG-2 or MPEG-4 Part 2 video, Theora, Motion JPEG, FFV1,
  DTS or Dolby TrueHD.
- What each reports: CoreAV reports no value. `ffprobe` reads it from the
  codec bitstream.
- Why: CoreAV implements the specifications it lists, and profile and
  level come from stored configuration; these codecs store none in Matroska and their bitstreams are outside
  CoreAV's specification list.
- Coverage requires: one of those codecs, a candidate `null` and a reference
  value.

## KD-MKV-HEVC-RANGE

- Field: stream `color_range`.
- When: an HEVC track whose sequence parameter set has no video signal type
  and no `Colour` element.
- What each reports: CoreAV reports no range. `ffprobe` reports `tv`.
- Why: the file does not state a range; CoreAV reports stored values only,
  as `coreav-isobmff` does.
- Coverage requires: HEVC, candidate range and color code points all `null`,
  and a reference range of `tv`.

## KD-MKV-ATTACHMENT

- Fields: stream count, and the packets of the extra streams.
- When: the segment has attached files.
- What each reports: CoreAV reports attachments apart from the tracks.
  `ffprobe` lists each attachment as a stream after the tracks (an image
  attachment as a video stream).
- Why: RFC 9559 5.1.6 and 21: attachments are files stored in the segment,
  not tracks.
- Coverage requires: as many extra reference streams, after the paired ones,
  as the candidate has attachments, each of kind `attachment` or an image
  codec when an attachment has an image media type.

## KD-MKV-DECLARED-SIZE

- Fields: stream width and height.
- When: a video track's `PixelWidth` and `PixelHeight` differ from the size
  its codec's parameter sets or frame headers give (an H.264 SPS in
  `CodecPrivate` or in the frames).
- What each reports: CoreAV reports the track header's `PixelWidth` and
  `PixelHeight`, as declared, and the verified sizes apart
  (`video.parameter_set_size`, `video.scanned_size`,
  `docs/design/policy-matroska.md`). `ffprobe` reports the decoded size.
- Why: the declared size is what the file states; replacing it would hide
  the disagreement an upload gate checks (as KD-DECLARED-SIZE of
  `docs/known-divergences.md` for ISO base media files).
- Coverage requires: the candidate's size equals `PixelWidth` and
  `PixelHeight` read from the file's EBML elements, the reference's differs
  from it, and `ffprobe`'s first decoded frame has the reference's size.

## KD-MKV-TAG-LANGUAGE

- Field: `--ffprobe-compat` `tags.language` only. The stream oracle doesn't
  gate the language, so `scripts/compat_check.py` applies this entry, with
  its evidence read in `scripts/oracle_kd_mkv.py` (`COMPAT_ENTRIES`).
- When: the segment's `Tags` hold a `SimpleTag` named `LANGUAGE` (in any
  case) in a `Tag` whose `Targets` name the track's `TrackUID`, beside the
  track's own `Language` element, and the track has no `LanguageBCP47`.
- What each reports: CoreAV prints the track's stored `Language` element
  (RFC 9559 5.1.4.1.19) as `tags.language`. `ffprobe` lists the
  `SimpleTag` under its own name, `LANGUAGE`, and gives the stream no
  `language` tag. Eight files of corpus v1, from one muxer.
- Why: without `LanguageBCP47`, the `Language` element is the track's
  language in the specification (with it, `Language` is to be ignored, and
  this entry does not apply); a `SimpleTag` of the same name is separate metadata (RFC
  9559 5.1.8), and leaving the element out to match `ffprobe`'s merged tags
  would copy a reader's quirk. A reader of `ffprobe`'s
  `tags.language` gets nothing for these tracks; CoreAV's value adds a key
  rather than changing one.
- Coverage requires: CoreAV's `tags.language` equals the track's `Language`
  element read from the file's EBML elements (`scripts/policy_ref.py`,
  written apart from CoreAV's parser); the file's `Tags` hold a
  `LANGUAGE` `SimpleTag` targeting that track's `TrackUID`; the track has
  no `LanguageBCP47`; and the
  reference has a tag named `LANGUAGE` (in any case) and no `language`. A
  different value, a tag that targets another track or none, or a
  reference `language` is not covered.
