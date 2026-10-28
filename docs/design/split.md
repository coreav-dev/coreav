# Splitting a file at keyframes into segments

Status: Accepted

Use case 7 splits a long upload at
keyframes so that the pieces can be transcoded in parallel, by stream copy
only and with memory bounded by the index and one copy window. Stage 3
lists it among the stream-copy primitives. This note records how
`split_mp4` (`crates/coreav-isobmff-write/src/split.rs`) and `coreav split`
do it, what they promise, and how that is checked. Which sync samples
count as cut points is in `docs/design/cut-points.md`, shared with `clip`.

Specifications: ISO/IEC 14496-12 (boxes, edit lists, composition offsets,
sync samples), ISO/IEC 14496-14, ISO/IEC 14496-15, the AV1 and VP codec ISO
media file format bindings, and ISO/IEC 14496-3 (AAC). No multimedia
implementation source was read.

## What it does

`split_mp4(source, limits, allowlist, options, open)` probes the source,
keeps one video track and at most one AAC track (chosen and checked
exactly as for a clip), cuts the video at
keyframes, and writes each segment as a `moov`-first MP4 file to the
writer `open(n)` returns. `coreav split (--every SECONDS | --at TIMES)
INPUT PATTERN` writes segment `n` to `PATTERN` with `%d` (or `%03d`)
replaced by `n`, and prints a `coreav.split/1` summary
([`docs/schema/split-v1.md`](../schema/split-v1.md)).

The promise, which every test and oracle below checks:

- **Every sample once.** For each kept track, the segments taken in order
  hold the source track's samples one for one: every sample in exactly one
  segment, in the source's order, with its bytes, size, decode duration,
  sync flag and composition offset. That includes samples the source never
  presents (encoder priming, frames trimmed by an edit list), so joining
  the segments gives back the source's samples. The one exception is the
  sync flag of a video track that the container marks sync throughout:
  the segments mark only the cut points among its samples
  (`docs/design/cut-points.md`, "The output's sync flags").
- **Every frame once.** Every frame the source presents is presented by
  exactly one segment, at its source time minus the segment's start, and
  every segment's video starts at a keyframe that decodes on its own. The
  segments' video intervals tile the source's video: each segment ends
  where the next starts. A segment's own video may start later than the
  segment does, after an empty edit, where the source presents nothing
  there: audio before the video in the first segment, or a gap between two
  windows of a source with several edits
  ([`edit-list-writers.md`](edit-list-writers.md), split decision 3).
- **Audio in the same place.** Each audio sample is presented in its
  segment at its source time minus the segment's start, within half an
  audio tick.

## Where it cuts

A **clean cut point** is a video sync sample `K` such that every sample
before `K` in decode order is presented before `K`, and every sample after
it is presented after `K`. Cutting there splits the decode order and the
presentation order at the same place: the samples before `K` are exactly
the frames presented before it. A sync sample with leading frames (an open
GOP: frames decoded after it but presented before it, which reference the
GOP before) is not a clean cut point. Cutting there would either duplicate
or drop those frames, or leave them in a segment where they cannot be
decoded.

The cut points are chosen like a clip's start: for
each target time, the latest clean cut point presented at or before it.

- `--every T` targets every multiple of `T` seconds on the source's
  presentation timeline, `T, 2T, 3T, ...`, so segment boundaries do not
  drift: each lies on the last clean cut point at or before a grid line,
  and a long GOP that spans several grid lines gives one cut.
- `--at t1,t2,...` targets the given increasing times. Several times that
  snap to the same cut point give one cut, and a time before the first
  clean cut point gives none.
- Cut points strictly inside the video's presentation only: a cut at its
  first frame or at or after its end would make an empty segment.

A source whose GOPs are all open (some HEVC encoders' default) has no clean
cut point after its first sample and gives one segment.

Rejected:

- **Any sync sample.** Simpler, but open-GOP leading frames would be lost
  or undecodable at every cut.
- **The keyframe at or after each target.** What `ffmpeg -f segment` does.
  At or before matches `coreav clip`, so the segment that holds time `t`
  starts at the same keyframe as `coreav clip --start t`.
- **Targets relative to the previous cut** (`T` after each cut): the
  boundaries drift by up to a GOP per segment.

## What each segment holds

Segment `i` holds the video samples from cut point `c_i` up to the sample
before `c_{i+1}` in decode order. The first segment starts with the
source's first sample, the last ends with its last, so samples before the
first sync sample or after the video's presented end are not lost.

Each audio sample goes to the segment in which its presentation starts.
Samples the source does not present go with their presented neighbours:
priming before the audio's edit with the segment where that edit starts,
padding after it with the last segment that has audio. A segment whose
range holds no audio sample has no audio track (audio that ends before the
video, say). When the source presents no audio at all, the audio is left
out of every segment and the summary says so.

A source with several edits puts each of its windows into the segments
that present it ([`edit-list-writers.md`](edit-list-writers.md), split
section): a segment inside one window has one media edit, a segment across
a splice has one per window, and a cut at a window's start opens its
segment with an empty edit for the gap before it.

A frame of AAC lasts about 21 ms and rarely starts on a cut, so a
segment's audio starts up to one frame after its video (after an empty
edit) and, but in the last segment, its last frame may run up to one frame
past its video's end; the last segment holds all the audio after its cut,
also where the audio outlasts the video. The union of the segments' audio
is the source's audio, with no gap and no overlap.

Rejected: **one frame of audio pre-roll per segment**, as a clip has. Pre-roll copies the frame before a segment into
it, so that frame would be in two segments, and joining the segments
would have to find and drop the copies. The cost of no pre-roll: decoded
on its own, a segment's first audio frame lacks the overlap of the frame
before it (ISO/IEC 14496-3), so its first ~21 ms differ from the source's.
Decoded across a join of the segments, nothing differs.

## Timing

Each segment is written by the clip writer's own code, as a clip whose run
and presentation are given instead of found:

- The segment's time 0 is its video's start: the cut point's presentation
  time, or for the first segment where the source's video starts. In the
  first segment only, when the source's audio starts earlier than its
  video, time 0 moves back to the audio's start (or less than a video tick
  before it: by a whole number of video ticks, so that the video stays
  exact), and the video gets an empty edit.
- The video edit runs from the cut to the next cut (the source's video end
  for the last segment), exactly in video ticks, with media time the cut
  sample's composition offset: the B-frame delay is removed as the
  source's own edit removes it. A clean cut point must not be presented
  before it is decoded (a negative composition offset), so that edit can
  start at it; such sync samples are not cut points.
- The audio edit is the clip's: an empty edit up to
  the first audio sample, rounded to the movie timescale, then the samples
  from their start; the first segment keeps the source's priming edit and
  the last its end. Unlike a clip's, a segment's audio edit is written also
  when it presents all of its media from its start: without an edit list,
  `ffprobe` 7.0.2 reports an audio track shorter than the movie at the
  movie's length (a file's first segment, 15.424 s of audio in a
  15.44 s movie, read as 15.44 s). Durations, the movie timescale (the least common
  multiple of the two media timescales when it fits 32 bits), the sample
  entries, the copied metadata and the box layout are the clip's.

## Memory and work

The source is listed three times in all, as for one clip:

1. Planning, counters only: one listing of the video finds every clean cut
   point (a list of 40 bytes per candidate, counted against
   `max_table_bytes`; a later sample presented at or before a candidate
   removes it, and candidates are kept in presentation order so it removes
   a suffix), the frames at the cuts are read to check them
   (`docs/design/cut-points.md`), then one listing of the audio assigns
   each sample to a segment.
2. Tables: one listing per track goes on from segment to segment, building
   each segment's tables and dropping them once it is written. For a video
   track the container marks sync throughout, it reads each sample's NAL
   unit headers for the segment's sync flags.
3. Copy: one listing per track goes on from segment to segment, copying
   each sample through the clip writer's single 64 KiB window.

So memory holds the source's parsed index, the cut points, one segment's
output tables and one window, whatever the file's size, plus a few hundred
bytes per segment for its plans and summary. Those count against
`max_table_bytes` with the cut points (320 bytes a segment), so the number
of segments is bounded too. All listings count against `max_sample_work`.
Splitting with repeated clips would list the source three times per
segment. `coreav split` writes its summary one segment at a time, as the
text `serde_json` pretty-prints for the whole document: building the
document whole held some 17 KB per segment, which the held-out memory
bar caught (`docs/spikes/split-bench/`).

## Reuse of the clip writer

`crate::clip` provides the selection and checks of tracks
(`select_video`, `select_audio`), the sample entries, the timelines
(`Timeline`, `Scaled`), the output tables of a run (`tables_of`, which
stops right after a run's last sample so a listing can go on), the file
head (`write_cut_head`: edits, durations, `ftyp`, `moov`, `mdat` header)
and the copy pass (`copy_cut`, `Run`). Splitting them out of `clip_mp4`
left every clip byte for byte the same: the fixture clips (192),
extractions (28) and remuxes (50) of `main` and of the split's branch have
the same SHA-256.

## Errors

The refusals are the clip's (codecs, `avc3` and `hev1`, ProRes, more than
one sample description, external data, edit lists outside the presentation
model, gaps in decode times anywhere in a kept track, a first sample
presented before it is decoded), plus a split interval of 0 or less and
split times that do not increase, and a source whose three listings would
exceed `max_sample_work`. Edit lists with several edits are split, each
segment carrying the windows in its range
([`edit-list-writers.md`](edit-list-writers.md), split section), with that
note's refusals: a movie timescale above 32 bits and a long run of empty
edits. They are all made while the source is listed for
planning, before `open` is first called, so nothing is written. The one
exception is a field overflowing while a segment's file head is computed,
which only crafted files reach (a timescale of 1 with decode durations near
2^32, say): that segment's writer is then opened but receives no byte.

Two edge cases follow from the first segment starting with the source's
first sample, unlike a clip, which starts at a sync sample: a source whose
first sample is not a sync sample and is presented before it is decoded is
refused; and in a source without an edit list whose first frames have
negative composition times, those frames, presented before time 0, are
presented by no segment (a clip from 0 leaves them out as well).

A failure later on (the source cannot be read, changes between listings,
or a writer fails) leaves the segments written so far;
`coreav split` then removes every segment it created, refuses an existing
segment file unless `--force` is given, and uses the exit codes of
`coreav clip`.

## Verification

- **Unit and round-trip tests** (`tests/split.rs`): synthetic movies
  (B-frames, open GOPs, audio priming, audio before the video, audio that
  ends early, every frame a keyframe, the table limit) and every fixture
  split every second and at 0.5 s and 1.3 s. For each kept track, the
  segments must hold the source's samples exactly once in order; every
  sample a segment presents must be presented in the source at its segment
  time plus the segment's start; every frame the source presents before
  the video's end must be presented by its segment.
- **Black-box oracles** (`scripts/split_check.py`, in the fixtures CI job):
  per stream, the segments' `ffprobe -show_packets -show_data_hash`
  lists, taken in order, are the source's list one for one (hash, size,
  keyframe flag, and times minus the segment's start); `ffmpeg -copyts -f
  framemd5` of the segments' video, shifted by their starts, is the
  source's decoded video frame for frame; each segment's decoded audio
  equals the source's from its second frame on; `ffprobe` and MediaInfo
  read each segment as an MP4 with the source's codecs, dimensions,
  rotation, sample rate and channels and durations equal to its edits.
  Known divergences: [`known-divergences-split.md`](../known-divergences-split.md),
  plus the clip's, applied per segment. Where the source or a segment has
  several edits, the oracle reads the samples and frames with edit lists
  ignored and applies the edits itself, from ISO/IEC 14496-12
  ([`edit-list-writers.md`](edit-list-writers.md), common decision 3 and
  the split section).
- **Fuzzing**: the `split` target of `crates/coreav-isobmff-write/fuzz`
  feeds arbitrary input and cut options through `split_mp4`; it either
  fails or writes segments that parse and hold every source sample once.
- **Fixtures**, required in CI: every fixture the clip writer accepts, plus
  `split_hevc.mp4`, `split_av1.mp4` and `split_vp9.mp4` (closed GOPs of 12
  frames, cut several times) and the `edit_multi_*` fixtures with a video
  track (several edits). The two fixtures whose picture size changes
  inside the track (`size_inband_sps.mp4`, `size_vp9_resize.mp4`) get the
  round trip but not the oracles, as for clips: `ffmpeg` reinitializes its
  decoder mid-stream there, which the size and frame comparisons do not
  model.

### Corpus bars (fixed 2026-10-19, before the corpus run)

Run in `docs/spikes/split-bench/`, with the results there.

- **Inputs**: every corpus v1 file of
  `docs/spikes/clip-bench/evidence/inputs.json` (`set: real`, extension mp4,
  mov, m4v or 3gp, a `v:h264`, `v:hevc`, `v:av1` or `v:vp9` tag: 869
  files). Each is split with `--every 10` through `split_mp4` with the
  default limits, allowlist and audio choice; a split that the default
  audio refuses because the audio is not AAC is made again without audio
  and marked `no_audio`, as the clip run did.
- **Checks**: on every file, the CoreAV round trip of the tests above and
  the packet coverage of `split_check.py` (ffprobe data hashes), plus the
  per-segment `ffprobe` stream, duration and MediaInfo checks. On files of
  at most 500 MB (790 files), also the decoded checks (`framemd5` of the
  video, decoded audio).
- **Bars**:
  - **Coverage: 100% of split files.** For every split file, every source
    packet of the kept streams is in exactly one segment, in order, by
    both the round trip and `ffprobe`'s packet hashes. Any miss is a bug
    and blocks the merge.
  - At least **98% of split files** agree on every check, directly or
    through a known divergence.
  - At least **99% of packets** agree: over all files, agreeing packets
    ÷ (agreeing + disagreeing), a packet agreeing when `ffprobe` lists it
    in its segment with the source's hash, size, keyframe flag and times
    minus the segment's start.
  - Files the writer refuses (an unsupported codec, sample entry or edit
    list, as for clips) are counted and listed apart and
    are in neither denominator; objects that fail their SHA-256 check on
    download are listed apart too. Every other failure counts as
    disagreement and is investigated.
- **Speed against FFmpeg** (held-out files only): per file,
  the wall time of `coreav split --every 10 SRC DIR/%03d.mp4` against
  `ffmpeg -i SRC -map 0:V -map 0:A -c copy -f segment -segment_time 10
  -reset_timestamps 1 DIR/%03d.mp4` with the same streams, both as
  processes writing to the same local disk, the median of 5 rounds with
  the order alternated, after one warm-up of each. The unit is the file's
  ratio ours ÷ ffmpeg. Bars: the median ratio over all held-out files is
  at most **1.0**, and the median of each class (non-fragmented,
  fragmented, longer than one hour) at most **1.5**.
- **Re-run on current code (fixed before the run).** The same 254
  held-out files and the same method on
  current `main`. Round 2 of this measurement: the same set as `perf-01`,
  which passed on its first round and was never used for tuning. The class
  bar of 1.5 above had no written reason, and a bar above 1.00 needs one, so the bar is the median ratio at most **1.00** overall and in each class
  (non-fragmented, fragmented, longer than one hour); "faster" is said only
  where the upper bound of the bootstrap 95% interval is below 1.00.
  **Result: met** (`docs/spikes/split-bench/`, `perf-03` on the crates of `1114aa6`): 0.208
  (95% CI 0.186 to 0.250) over 241 files; non-fragmented 0.197, fragmented
  0.304, longer than one hour 0.220. Every upper bound is below 1.00, so
  split is faster than FFmpeg in every class.
- **Memory** (held-out files): peak resident memory of `coreav split`
  minus that of `coreav probe` on the same file is at most the largest
  segment's `moov` box plus 24 bytes per source sync sample plus 2 MiB, on
  every held-out file; reported against the file size.
- Misses on speed or memory block the release, not the merge; the cause is
  recorded before any bar changes.
