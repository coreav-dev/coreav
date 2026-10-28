# Writer oracles: what only the boxes show

Status: Accepted

The writer audit ([`kd-audit-writers.md`](../measurements/kd-audit-writers.md))
changed one value of an output at a time and found changes that no writer
oracle saw, with or without its known divergences. `ffprobe`,
`ffmpeg` and MediaInfo do not read those values, or read them only through
something else:

- **header durations**: clip and split checked neither the `mdhd`, `tkhd`
  and `mvhd` durations nor most edit durations ±1 tick (remux checked its
  track and movie headers, not `mdhd`);
- **sync samples**: a sync sample moved by one went unseen in 265 of 308
  fixture clips, 173 of 196 split segments, 51 of 61 remuxes and 53 of 62
  HLS video renditions. For H.264 and AV1, `ffprobe` takes the keyframe
  flag from the bitstream;
- **the sound sample entry**: `ffprobe` and MediaInfo take an AAC track's
  channels and sample rate from its decoder configuration, never from the
  entry's fields;
- **audio timing within a tick**: an audio edit's media time +1 or its
  media timescale +1 moves the audio by a fraction of a millisecond, under
  every tolerance;
- **samples outside the edit**: `ffprobe` does not list a sample the edit
  leaves out, so its bytes went unchecked (an edit that starts after a
  remux's first key frame; a discarded priming sample of an extraction).

## The checks

`scripts/track_boxes.py` reads both files, apart from CoreAV's parser and
from the black boxes: the movie header, and for each track its header,
edit list, media header, handler, first sample entry and every sample
(offset, size, decode duration, composition offset, sync flag) from the
sample tables and from movie fragments (ISO/IEC 14496-12 8.6 to 8.8;
QuickTime's sound entry versions 1 and 2). `scripts/writer_boxes.py`
compares the output with its source by the rule each writer's record
states:

| Check | Rule | Writers |
|---|---|---|
| samples | the output's samples are the source's kept run, one for one: bytes, size, decode duration, composition offset as stored (the same 32 bits; a QuickTime source's version 0 table may hold signed values that the output states in a version 1 table) | all |
| sync | each sample's flag is the source's; the first kept video sample of a clip, and of every split segment but one starting with the source's first sample, is a cut point (`cut-points.md`); where the container claims every copied sample is sync, clip and split mark exactly the samples the coded data starts from (`cut-points.md`): for H.264 an IDR picture, a recovery point SEI message with `recovery_frame_cnt` 0 and `exact_match_flag` 1, or an intra reference picture when the sequence parameter set keeps at most one reference frame (an unknown count allows none); for HEVC a sample whose first coded slice is an IRAP picture, as its NAL unit header says (a sample without one is none); for AV1 a sync sample of its binding (2.4). An H.264 or AV1 frame whose coded data cannot be read keeps a flag the table or the fragment states and has none where the flag is only implied; other codecs, and tracks whose configuration record cannot be read, keep the flags as stored | all |
| entry | the sample entry's type and, for video, width and height are the source's; for audio, the sample rate as stored, the `AudioSpecificConfig` bytes (copied as stored; empty where the decoder configuration has none, as for MPEG-1 audio, and a problem where the `esds` cannot be read; ISO/IEC 14496-12's version 1 entry, in a version 1 `stsd`, read apart from QuickTime's), and a channel count that is the source entry's or the one that configuration states (ISO/IEC 14496-3 table 1.19 or its program config element; 2 for a mono one that signals parametric stereo, object type 29) | all |
| timescale | the media timescale is the source's | all |
| media duration | clip and split: the kept decode durations, plus the source's own difference between its declared duration and its sum when the run keeps its last sample; for video at least the edits from the first media edit on (amendment point 3). Remux, sanitize, extraction (`remux.md`): the source's declared duration when it has no fragments and declares one, else the sum | all but HLS |
| edit rate | every media edit plays at rate 1 (the writers refuse other rates) | clip, split |
| track and movie durations | the track's is the sum of its edits (8.3.2), or for extraction without an edit list the media duration in movie ticks rounded to the nearest; the movie's is the longest track's (8.2.2) | clip, split, extraction (remux already checks its own) |
| movie timescale | clip and split: the least common multiple of the media timescales when it fits 32 bits, else the video's; with several edits a multiple of the video's (`edit-list-writers.md`, clip decision 5). Extraction: the source's | clip, split, extraction |
| audio edit | clip and split, for a source with one media window: the audio is presented from the latest of the output's start, the window's start and the first kept sample's start, to the earliest of the clip's end (the requested end, or the video's end where that is earlier; the video edit alone is rounded up to a whole video tick), the window's end and the last kept sample's end; the empty edit and the media time are those times rounded to the nearest tick, and the media edit ends at its end so rounded. For a clip of a source with several edits (`edit-list-writers.md`, clip decisions 4 to 6): the source's entries, empty ones included, cut to the clip from its start to its requested end, each media time moved by the first kept sample's decode time, each entry ending at its exact end rounded to the nearest tick and left out when that leaves it none, no empty entry last. For a split segment of such a source (split decision 5), the same, the range running from the first showing of the first of its samples a window shows (a sample that starts in a gap is shown from the window's start) to the end of the first showing of the last of them, after one empty entry from the segment's start | clip, split |
| HLS | the media segments' samples, taken in order, against the source track's run, sync flags as stored (the packager rewrites no claim). The run starts where `hls.md` ("Timing") starts it: video at the latest sync sample presented at or before the edit's start after every earlier sample in decode order (an IDR or IRAP picture while every flag so far is sync); audio at the source's first sample, however early the edit starts. It ends where the edit stops presenting: no sample left out after it is presented. The init segment's sample entry and timescale | HLS |

The clip oracle's video edit check now compares the edit itself with the
clip, not the edit capped by the media duration, which hid an edit one
tick too long.

A box check's problem is a problem: no known divergence covers a field
under `box:`. A real difference that a writer makes on purpose would get its
own entry with a predicate on raw values from both files.

## Choices

- **A reader of its own.** Rejected: CoreAV's parser, which would agree
  with the writer it shares code with; `ffprobe`, which cannot see these
  values (that is the issue). The reader is small, reads only what the
  checks need, and gives up (an `unreadable` problem) rather than guess:
  a `ctts` or `stss` that does not fit the sample count, a compressed
  movie that does not inflate to one `moov`. Two things it accepts: an
  `stts` longer than the sample count is cut at it, and fragments whose
  `tfdt` leaves a decode time gap are read, marked, and refused only by
  the checks that derive times from the durations (the audio edit, the
  left-out samples of an HLS rendition). Any failure while a check runs
  is a problem, never a pass.
- **Compare with the source by the writer's rule, not only check the
  output against itself.** The output's own consistency (the track
  duration is the sum of its edits, the movie's the longest track's)
  catches a single changed field, but not a writer that computes a wrong
  edit and writes it consistently. So the edits, the media durations and
  the sync flags are derived from the source's boxes.
- **Sync flags where the container claims every sample.** The oracle
  classifies the coded data itself, from ITU-T H.264 (NAL unit types, the
  slice type and `nal_ref_idc` of every coded slice, the recovery point
  SEI message, `max_num_ref_frames` from the `avcC` sequence parameter
  set), ITU-T H.265 (NAL unit types) and AV1 (OBU and frame header bits),
  by the rule `cut-points.md` states. Rejected: comparing with `ffmpeg`'s
  decoded key frames, a black box's own rule; requiring only a subset of
  the source's flags, which lets a moved flag through; a looser H.264 rule
  (any intra slice or recovery point allowed), which the review showed
  passes an earlier change's own regression (`clip_stss_all_flash.mp4`: non-IDR I
  frames of a stream with 16 reference frames marked sync).
- **The AAC channel count as a set.** CoreAV writes the configuration's
  count where the entry disagrees (one corpus file, a 5.1 configuration
  under a stereo entry), and keeps a stereo entry over a mono
  configuration whose parametric stereo may be signalled only in the
  bitstream. So either count passes. Rejected: mirroring the parser's rule
  for implicit parametric stereo, which would make the oracle a copy of
  the code it checks; also allowing 2 for every mono configuration, which
  let a mono entry changed to 2 through (the first fixture audit, 41 of 45
  remuxes).
- **No packet counts change.** The packet bars stay `ffprobe`'s; a box
  problem fails its output.
- **Audio edits exactly, also for several windows.** The check of
  `edit-list-writers.md` (common decision 3) maps every edit, but compares
  the decoded audio within a sample, so a media time one tick off went
  unseen on the five clips of `edit_multi_av.mp4` (the first fixture
  audit). The box check derives each entry from the source's.

## Not checked

- **Audio edits derived from `iTunSMPB`.** The clip writer gives a
  source without an edit list but with that item an edit list derived
  from it; the audio edit check takes such a source's media from 0, so a
  clip of one would fail (fail-closed). No fixture and no corpus file with
  video has one. The same holds for an audio-only HLS rendition, whose
  packager drops the padding frames the item names.
- **The smallest movie timescale for several edits.** With several edits
  the check requires a multiple of the video's timescale, not the smallest
  one that makes every kept window whole (`edit-list-writers.md`, clip
  decision 5); a larger one changes no time.
- **The composition offset table's version.** Offsets compare as the same
  32 bits; a version 0 table written as version 1 (or the reverse) for
  offsets of 2^31 and more is not seen here. `ffmpeg` reads such offsets
  as negative either way.

## Out of scope

- **Policy.** `policy_check` judges verdicts. A probe value that changes
  without crossing a rule leaves the verdict unchanged, as it should; the
  values themselves are the probe oracle's, audited in
  [`kd-audit.md`](../measurements/kd-audit.md).
- **Text.** Its audit found no change unseen (`kd-audit-writers.md`).

## Pass criteria

Fixed here before the runs. "Before" is `main`'s oracles at the commit
this change starts from, "after" this change's, on the same inputs with the
same `coreav` binary and black boxes, on one machine.

1. **Mutation audit, fixtures** (clip, split, remux, sanitize, extraction
   with `tools/oracle/writer_kd_audit.py`, HLS with
   `writer_kd_audit_cli.py`): after, **no effect is 0** for each mutation
   class an earlier change names, on every output it applies to:
   - header durations: `mdhd` duration +1, `tkhd` duration +1, `mvhd`
     duration +7, edit duration +1 and −1;
   - sync: `stss` sync moved; HLS sync flag cleared;
   - sound entry: audio channels +1, audio rate halved; HLS init channels
     +1;
   - audio timing: the audio edit's media time +1, the audio `mdhd`
     timescale +1;
   - sample data: sample byte flipped, first sample size +1.

   COVERED and ABSORBED stay 0. Other classes are reported.
2. **Mutation audit, corpus**: the same bar on these inputs, chosen by
   this rule from the corpus run below: per writer, the 6 smallest real
   files it writes with audio and the 4 smallest without (clip, split,
   HLS), or the 10 smallest it writes (remux, sanitize, extraction); and
   for remux the 2 smallest whose source video edit starts after its first
   sample's composition time, for extraction the 2 smallest whose audio
   edit starts after its first sample's end (the cases an earlier change names).
3. **Fixtures**: every writer's fixture outputs pass every oracle, as in
   CI (100%).
4. **Corpus**: every real ISOBMFF file of corpus v1 (1,049) through clip
   (four clips each), split (every 10 s), remux,
   sanitize, extraction and HLS (6 s), with `tools/oracle/writer_corpus.py`
   on the final commit. `main`'s oracle judges every output this change's
   fails and a 5% sample of those it passes (by SHA-256). Every output that
   fails after and passed before has each new problem explained as a
   writer bug (fixed in this change when it is in the writers, filed
   otherwise) or as a new known divergence whose predicate compares raw
   values of both files, with the mutation audit at 0 covered for it. None
   unexplained. Every sampled output that passes after passes before.

Results: [`kd-audit-writers.md`](../measurements/kd-audit-writers.md),
"Blind spots".
