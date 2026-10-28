# Known divergences: HLS renditions versus the oracles

Every intentional difference between an output and its source, as the
black-box oracles see it, is documented with a machine-readable twin. For HLS packaging (`docs/design/hls.md`) the twin is
[`known-divergences-hls.json`](known-divergences-hls.json);
`scripts/hls_check.py` applies it with one predicate per entry and refuses
to run when the twin and its predicates disagree. Each entry covers only
the fields it names; a packaged input counts as agreement only if every
difference is covered, and covered inputs are reported apart from direct
agreement.

The oracles are `ffprobe` and `ffmpeg` 7.0.2 and MediaInfo 26.10.
Predicates use the files' own box headers and the oracles' output, never
file names. Every init segment's edit list is first checked against the
source's own (read from its box headers), so the edit lists these
predicates rely on are the source's.

## KD-HLS-1

- Field: the presentation times of a video rendition's packets and decoded
  frames (`ffprobe -show_packets`, `ffmpeg -copyts -f framemd5`).
- When: some of the source's composition offsets are negative: stored
  signed (`ctts` version 1), so the media segments carry them in `trun`
  version 1 boxes; or stored as values of 2^31 and more in a version 0
  table, as some recorders write negative offsets, so the segments
  carry the same 32 bits in version 0 boxes, which `ffmpeg` reads as
  negative in both files.
- What each reports: the frames, hashes and order agree; `ffmpeg` presents
  every frame of the rendition later than the source's by the same amount,
  the magnitude of the rendition's most negative composition offset. Examples: `negative_cts.mp4` (B-frames with offsets down to
  −1,024 ticks at 12,800 Hz), every frame 0.08 s late; a recording
  whose version 0 table stores −50 ticks at 600 Hz as 4,294,967,246, every
  frame 0.083 s late.
- Why: ISO/IEC 14496-12 8.8.8 presents a sample at its decode time plus
  its composition offset, negative or not, then applies the edit list;
  CoreAV copies offsets as stored. The pinned `ffmpeg` reads
  negative offsets in movie fragments shifted, as it does in its own
  `-hls_segment_type fmp4` output, and differently from the same offsets
  in a `ctts` box.
- Coverage requires: a video rendition; one shift for every packet and
  every frame; the shift equal to the magnitude of the most negative
  composition offset of its media segments' `trun` boxes, read from their
  box headers as signed 32-bit values.
- With KD-HLS-3: see KD-HLS-3 for a source whose edit also starts
  mid-frame.

## KD-HLS-2

- Fields: a rendition's packet list (`ffprobe -show_packets`) and decoded
  frames (`ffmpeg -copyts -f framemd5`).
- When: the source's edit hides samples that the rendition copies because
  presented samples depend on them: before the edit's start (the GOP that
  holds the first presented frame starts before it, or audio pre-roll), or
  at its end (a reference frame presented after the edit).
- What each reports: `ffmpeg` drops such samples from the source's list,
  and their decoded frames from the source's output, when it reads sample
  tables; from movie fragments it lists and decodes them. Conversely,
  trailing samples that no presented sample needs are left out of the
  rendition, while `ffprobe` lists them in the source flagged as discarded. The output has
  extra packets and frames before the source's first, and every later one
  is the source's. Example: a trimmed recording whose edit starts 3.36 s
  into a 5.3 s GOP; the output decodes 401 frames, the source 301, the last
  301 equal.
- Why: ISO/IEC 14496-12 8.6.6 (the edit list leaves these samples
  unpresented; decoding the first presented sample needs them); one packet is
  one sample, and samples are never dropped (`docs/design/hls.md`, "Timing").
- Coverage requires: an edit list in the init segment; every source packet
  (frame) found in the output, by hash and time; every extra one before the
  first of the others or after the last; and no more extra ones at either
  end than the rendition has samples that start before its edit or after
  its end, read from the segments' box headers; or, for packets the
  rendition leaves out, every one of them at the end of the source's list
  and flagged as discarded by `ffprobe`, and the source's decoded frames
  the rendition lacks at most one per such packet, at its time. Example of
  the latter: an AV1 recording whose last sample has a decode duration of
  0 and lies at the edit's end; `ffmpeg` decodes it from the source, 4,723
  frames against the rendition's 4,722.

## KD-HLS-3

- Fields: a rendition's packet and frame times.
- When: the source's media edit starts at a media time between two
  samples' composition times.
- What each reports: CoreAV applies the edit exactly; `ffmpeg` starts the
  source's timeline at a sample's composition time after the edit's start
  (the next one for constant frame rates, one a few samples later in some
  variable frame rate QuickTime files), while it reads the output's
  fragments without that adjustment. Every time differs by the same amount.
  The same cause as `KD-C-EDIT-MIDFRAME` for clips. Example: an edit
  starting at media time 100,793 with frames of 1,001 ticks; every frame
  308 ticks apart.
- Why: ISO/IEC 14496-12 8.6.6 (an edit maps its media time to the start
  of the presentation); the simple edit list is applied exactly.
- Coverage requires: one shift for every packet and frame, above 0; the
  init segment's edit media time is no sample's composition time, and the
  media time plus the shift is one, with at most three samples between
  them (composition times read from the segments' box headers); and both
  the source's own: the segments' composition times are the source track's
  stored ones (`ffprobe` with edit lists ignored), and the init segment's
  edit media time is the source's media edit (from its `elst`).
- With KD-HLS-1: a video rendition with negative offsets whose source edit
  also starts mid-frame is shifted by both. Coverage then requires the
  shift minus KD-HLS-1's to meet the conditions above, and the input counts
  as covered by both entries. Example: a variable frame rate QuickTime
  recording at 600 Hz with offsets down to −335 ticks and an edit at media
  time 169, one tick before a frame; every frame 336 ticks apart.

## KD-HLS-4

- Fields: an audio rendition's packet and frame times.
- When: the source's audio has no edit list and an `iTunSMPB` tag whose
  delay, padding and length add up to its samples, so the rendition's edit
  list is made from the tag, as for clips and extraction.
- What each reports: `ffmpeg` applies the source's tag by discarding the
  delay while keeping the source's timeline; the rendition's edit list maps
  the delay to presentation time 0. Every time differs by exactly the
  delay. Example: a delay of 2,112 samples at 44.1 kHz, every time
  0.0479 s earlier in the output. The same cause as `KD-X-GAPLESS-EDIT`.
- Why: a tag that adds up exactly stands for an edit list, which the
  rendition carries; ISO/IEC 14496-12 8.6.6.
- Coverage requires: an audio rendition; the source's tag as `ffprobe`
  reports it; the init segment's media edit starting at the tag's delay;
  and one shift of every packet and frame, equal to minus the delay.

## KD-HLS-5

- Field: an audio rendition's decoded frames.
- When: the source has an edit list and an `iTunSMPB` tag that disagree.
- What each reports: the rendition keeps the edit list and does not copy
  the tag (no metadata is copied); `ffmpeg` applies the tag to the source,
  so it decodes the source's first and last frames differently (two frames
  more in the output in the corpus). The same cause as
  `KD-X-GAPLESS-CONFLICT`.
- Why: the edit list is applied, and a tag that does not stand for the
  edit list is not applied.
- Coverage requires: an audio rendition; the source's tag as `ffprobe`
  reports it; an init segment edit list whose media time is not the tag's
  delay; and every source frame except the first and the last in the
  output, in order and unchanged.

## KD-HLS-6

- Fields: the master playlist's `CODECS` for an audio rendition, and
  MediaInfo's sample rate for its init segment.
- When: the source's AAC is HE-AAC or HE-AAC v2 whose configuration does
  not signal SBR or parametric stereo (implicit signalling: the extension
  is found only in the bitstream).
- What each reports: CoreAV reads no bitstream, so `CODECS` names the
  configuration's object type, `mp4a.40.2`; `ffprobe` decodes the extension
  and names HE-AAC. MediaInfo reads the core's sample rate (half the
  decoded one) from the init segment, which holds no frames, and the
  decoded rate from the source and from every segment. The same cause as
  `KD-AAC-IMPLICIT-SBR`.
- Why: RFC 6381 3.3 (the object type of the configuration); the
  configuration is copied as stored.
- Coverage requires: an audio rendition; `ffprobe` naming HE-AAC or
  HE-AAC v2 for the source; `CODECS` object type 2; and, for MediaInfo, the
  init segment's rate half the source's.

## KD-HLS-7

- Field: the audio stream's `profile` in `ffprobe`.
- When: `ffprobe` names no profile for the source's AAC stream, and `LC`
  for the rendition.
- What each reports: the rendition's configuration is the source's, byte
  for byte; `ffprobe` names the profile only for the one-stream output.
  The same cause as `KD-X-PROFILE-UNNAMED`.
- Why: the `esds` box is copied as stored.
- Coverage requires: `aac` on both sides, no source profile, `LC` on the
  output.

## KD-HLS-8

- Field: a rendition's decoded frames.
- When: `ffmpeg` decodes the source's stream from its sample tables to
  other frames than from movie fragments holding the same packets.
  Example: a fixture of two joined encodes, where decoding the source
  drops a frame at the joint that decoding fragments outputs.
- What each reports: every packet of the rendition is the source's; the
  rendition decodes, frame for frame, as `ffmpeg`'s own stream copy of the
  source stream into fragmented MP4 does.
- Why: the samples are copied as stored; how `ffmpeg` reads
  sample tables and fragments is its own.
- Coverage requires: no other entry covers the frames; and the rendition's
  decoded frames equal, in order and at the same times up to one constant,
  those of `ffmpeg -i SOURCE -map 0:N -c copy -movflags
  frag_keyframe+empty_moov+default_base_moof`.

## KD-HLS-9

- Field: the master playlist's `RESOLUTION`.
- When: the codec's parameter sets crop the picture on the left or the top.
- What each reports: `RESOLUTION` is the picture size the codec
  configuration's parameter sets bound, which leaves out left and top crops
  by design (`docs/design/frame-sizes.md`); `ffprobe` applies them.
- Why: `docs/design/hls.md` (the display size comes from the parameter
  sets, so that understated sample entries and conformance windows are
  right).
- Coverage requires: `RESOLUTION` equal to the size of the first frame
  `ffprobe` decodes with cropping off (`-apply_cropping 0`), and at least
  its displayed size in both dimensions.

## KD-HLS-10

- Field: where a segment starts on the media playlist's timeline.
- When: the source's track fragments carry decode times (`tfdt`) past the
  ones their stored sample durations reach, and its segment index gives
  each subsegment the sum of its stored durations. Example: an audio-only
  CMAF file whose every next fragment starts 10 to 15 ticks (at 90 kHz)
  after its durations reach; by its 155th subsegment the index's timeline
  is 23 ms behind the fragments', more than one AAC frame.
- What each reports: the playlist places each segment where the index
  does, and `ffprobe` places its first sample at the decode time its
  fragment carries; both values are the source's.
- Why: each segment's `EXTINF` is its `subsegment_duration`
  (`docs/design/hls.md`, "The segments are the subsegments"), and each
  segment's `tfdt` is its first sample's decode time; the samples are
  copied as stored. The writer does not read every fragment
  to build the playlist (#248). ISO/IEC 14496-12 8.8.12 makes a fragment's
  decode time the sum of the earlier samples' durations; the source's
  `tfdt` values break it, so its index (8.16.3.3) disagrees with its own
  fragments.
- Coverage requires, read from the source's box headers: one top-level
  segment index of the rendition's track whose references are all to
  media, and each segment holding exactly the samples of the track
  fragments in that subsegment's byte range; from the second segment on,
  each segment's start on the playlist the running sum of the index's
  durations plus one constant, within a microsecond; each subsegment's
  earliest presentation time past that sum by exactly the decode time gaps
  the source's fragments leave before it, at least one not 0; and every
  segment start, moved by its gaps, starting as the oracle requires without
  the entry (a video segment at its first frame's time, an audio segment
  within one frame of it, after one constant origin).
