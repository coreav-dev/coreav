# Joining files that share a codec configuration

Status: Accepted

Use case 7 joins clips that share a
codec configuration, by stream copy only, with memory bounded by the index
and one copy window. Stage 3 lists it among the stream-copy primitives.
This note records how `join_mp4` (`crates/coreav-isobmff-write/src/join.rs`)
and `coreav join` do it, what they promise and refuse, and how that is
checked. It builds on the clip writer and the split
([`split.md`](split.md)).

Specifications: ISO/IEC 14496-12 (boxes, edit lists, composition offsets,
sync samples), ISO/IEC 14496-14, ISO/IEC 14496-15, ISO/IEC 14496-1 (the
`esds` descriptors), the AV1 and VP codec ISO media file format bindings,
and ISO/IEC 14496-3 (AAC). No multimedia implementation source was read.

## What it does

`join_mp4(inputs, limits, allowlist, options, out)` probes every input,
keeps its first video track or the video stream at the index the caller
names (`VideoChoice::Stream`) and, unless the caller asks for video only,
its first audio track or the audio stream at the index the caller names
(`JoinAudio::Stream`), chosen and checked as for a clip. `coreav ffmpeg`
names both indexes, and a stream there that the allowlist leaves out fails
instead of being skipped. It writes one `moov`-first MP4 with one video
track and at most one audio track. `coreav join [--audio none] -o OUTPUT INPUT...` prints a
`coreav.join/1` summary ([`docs/schema/join-v1.md`](../schema/join-v1.md)).

The promise:

- **Every sample once.** Each output track holds the inputs' samples, one
  input after another, every sample once and in order, with its bytes,
  size, sync flag, composition offset and decode duration. The one
  exception is the last video sample of an input, whose decode duration may
  be extended (see Timing).
- **Each input as it presents itself.** Every sample an input presents is
  presented in the output at its time on the input's own presentation
  timeline plus the input's offset, and no other sample is: edit lists,
  B-frame delays, audio priming and pre-roll, and frames a clip keeps only
  for decoding stay as each input has them.
- **Inputs end to end.** The first input keeps its own timeline (offset 0).
  Each next input starts where the previous input's video ends: its
  earliest presented sample is placed there.
- **The segments of a split join back into the source.** When the inputs
  meet exactly, as a split's segments do, the output has one edit per track
  and the source's samples, times and edits.

## What must match

Every input must match the first input in everything the one sample entry
and track header of each output track describe:

- the video codec and sample entry type, the codec configuration box
  (`avcC`, `hvcC`, `av1C`, `vpcC`) byte for byte, the coded width and
  height, the `pasp`, `clap` and `colr` boxes, the media timescale, and the
  track header's transformation matrix and display size;
- an audio track in all inputs or in none (or `--audio none`); and for the
  audio, the codec, channel count, sample rate, the AAC decoder
  configuration (object type and decoder-specific information from `esds`)
  and the media timescale.

Anything else fails before anything is written, with `JoinError::Mismatch`
naming the input and the difference, for example `input 2 cannot be joined
to the first: its video timescale is 25600, the first input's is 12800`.
Declared bit rates (`btrt`, and the buffer size and bit rates in `esds`) are
not compared; the output keeps the first input's. Each input is also
checked on its own as a clip would be (`JoinError::Input` with its
position), and two more refusals concern inputs after the first:

- its video must start with a sync sample, so that it decodes after the
  previous input's samples;
- no frame decoded after its first may be presented before it (an input
  that starts at an open-GOP sync sample with leading frames). Those frames
  refer to frames before the input, which in the output would be the
  previous input's. ISO/IEC 14496-12 edit lists can hide them, but they
  still decode against the wrong frames, and `ffmpeg` 7.0.2 then decodes the
  following frames out of order (an HEVC clip that starts at a CRA picture).
  Such a clip can be the first input. A split's segments never start like
  this, since a split cuts only at clean cut points.

Rejected: comparing whole sample entries byte for byte (two encodings with
the same configuration differ in declared bit rates); accepting different
codec configurations with several sample entries (ISO/IEC 14496-12 allows
it, but many players read only the first).

## Timing

Each output track's decode times follow the inputs one after another; its
edit list holds one edit per media edit of each input (one per input
where an input has at most one), with the empty edits between an input's
media edits kept:

- **Offsets.** The first input keeps its own timeline. Each next input is
  placed so that its earliest presented sample (its video's start, or its
  audio's when that starts earlier, moved back by whole video ticks so that
  the video stays exact) starts where the previous input's video ends.
- **Video edits.** One media edit per media edit of the input (see
  "Inputs with several edits" below for how they are cut): from the media
  time the input's own edit presents first, for as long as the input
  presents video:
  until its edit ends, its samples end, or its track ends, whichever comes
  first, its track ending no earlier than its last presented frame (the
  media header's duration, the sum of the decode durations, ends before
  that frame where frames are composed further apart than they are
  decoded: variable frame rate, or a frame left out of a reordered
  stream); but until its edit ends when the edit lies within the media's
  declared duration (a sample is shown until the next one's composition
  time, the last until the edit ends: where decode durations are shorter
  than the frames' spacing, variable frame rate, the last frame's own
  duration can end well before the edit, and ending the input there would
  cut its audio), when it ends within the spacing of the input's last two
  frames after the last one starts (how long a player shows it: a segment
  cut where its source's next frame starts), or less than a frame after
  its samples. Ending a segment earlier would place every later segment
  that much early, and the segments would no longer join back into their
  source. A segment or clip cut before a frame of its source ends
  there, while its last frame, by its own duration, can end a tick or so
  earlier where reordered frames overlap (the source showed it until the
  next frame); ending the input at its samples would place the next input
  that much early, and a split's segments would no longer meet. When the
  next input's video starts later than this input's ends (its own audio
  starts first), this input's last frame is held until then: its edit runs
  on, so there is no gap. It is not held where a frame of the input is
  composed after its last edit ends (a frame its edits cut out, or one a
  clip keeps only for decoding): the running edit would show that frame,
  so the gap stays an empty edit.
- **Decode times.** Each input's samples follow the previous input's, and
  every composition time of an input lies before every composition time of
  the next. When an input's frames are composed past where the next input's
  media would start (a clip that ends inside a group of pictures keeps
  frames that are presented after its end), the input's last video sample's
  decode duration is extended so that they are not; the summary reports the
  ticks added (`extended`). Where every composition time of an input lies
  before its edit's end, the edit ends its last frame, so frames whose
  durations overlap the next input's first frame by a tick need no room;
  otherwise its frames' whole durations must end first, so that none of
  them is shown in the next input's edit. A sample's decode duration is not part of its
  bytes. Samples composed 2^31 ticks or more after their decode (version 0
  `ctts` offsets that ISO/IEC 14496-12 reads as unsigned, where the muxer
  most likely meant negative ones; `KD-C-CTTS-UNSIGNED`) are left out of
  this rule: they lie past every edit, and keeping them before the next
  input would move every later input by some 2^32 ticks. The join is
  refused if one of them would fall inside an edit of the output.
- **Audio edits.** One media edit per media edit of the input: from its edit's media time
  (after priming or pre-roll) to its end, cut where the next input's audio
  starts so that the two never overlap; an empty edit where an input
  presents no audio. AAC frames are never shortened: a decoded frame is
  1,024 samples whatever its stored duration.
- **Merging.** An edit that starts where the previous one ends, in both
  presentation and media time, extends it. The segments of a split meet
  exactly, so they give one edit per track: the source's.
- **Movie timescale.** The least common multiple of the two media
  timescales when it fits 32 bits (as for a clip), so that edits are exact;
  boundaries are rounded to it only where an input's own edits are not, and
  never accumulate.

Rejected:

- **One edit per track with adjusted decode durations.** Readers that model
  only one edit (CoreAV's own) would time the output, but an
  edit list of one edit cannot hide the frames a clip keeps only for
  decoding, or an input's audio priming, in the middle of the output; they
  would be presented. A single edit is used exactly where it is enough.
- **Dropping hidden samples** (priming, pre-roll, frames past a clip's end):
  every sample of every input would no longer be in the output, and the
  samples after them would decode differently.
- **An empty edit for a video gap.** `ffmpeg` 7.0.2 plays the next input's
  first frames during an empty edit after a media edit and again after it.

### Inputs with several edits

An input whose edit list has several media edits (`EditList::Multiple`,
`docs/design/edit-lists.md`) is joined as it presents itself: each of its
media edits becomes an output edit, cut to the span the input presents
(`[start, end)` above), and the empty edits between them stay empty
edits. Where the input's presentation would end between two of its edits,
it ends where the earlier one does. The placement, the decode times and
the refusals are those of any input. The design, its oracle and its
results are in `docs/design/edit-list-writers.md` ("Join").

### Edit lists of more than one edit

ISO/IEC 14496-12 defines what such a list presents, and the output follows
it exactly. CoreAV's own reader models at most one empty edit and one media
edit, so it reports such an output's edit list as unsupported
(presentation times `null`), and `coreav clip` or `coreav split` refuse it.
`ffmpeg` 7.0.2 applies edits after the first approximately, which players
built on it show:

- frames of a later input with a composition delay are presented late, by
  up to that delay (`KD-J-VIDEO-LATER-EDIT`);
- a frame cut short by its edit is shown whole (`KD-J-VIDEO-EDIT-END`);
- a later audio edit that starts inside a frame starts at the next frame,
  less than a frame (about 21 ms) early (`KD-J-AUDIO-EDIT-START`).

Joins whose inputs meet exactly (the segments of a split) have one edit per
track and play exactly in `ffmpeg` as well. Clips of one source cut at the
same closed-GOP boundary meet exactly in their video, but the second clip's
audio pre-roll still needs an edit of its own.

## Memory and work

Every input is listed three times: to find its presentation, to build the
output tables, and to copy its samples. Memory holds every input's parsed
index (as a probe of each would), the output's tables and one 64 KiB copy
window, which is cleared whenever the copy moves to another input. Each
input's listings count against its own `max_sample_work`. `coreav join`
writes its summary one input at a time, as `coreav split` does its
segments: built whole, a summary held some 17 KB per entry
([`split.md`](split.md), "Memory and work"). Every input's source stays
open for the whole join, and the join releases each between the passes
that read it (`Source::release`: a buffered source frees its read window,
filled again on the next read). Kept, the windows of 748 segments held
some 20 MiB, twice the bound of the corpus run (`docs/spikes/join-bench/`).
Every input is an open file, so a join of more inputs than the process may
open files fails cleanly (about 1,000 under Linux's default limit of
1,024); reopening inputs by path between passes would lift that, but would
change the library's input from sources to paths. The three listings
visit each input's samples once each, so an input whose first listing
already uses more than a third of `max_sample_work` is refused before
anything is written, as for a split.

## `ffmpeg -f concat` lists

`coreav_cli::concat::read(list, safe)` reads the list file of FFmpeg's
concat demuxer (its documentation's "Syntax" and `safe` option) and
returns the files to pass to `coreav_cli::join::run`, in order:

- `file PATH` names a file. The path is one token by FFmpeg's quoting and
  escaping rules (`'...'` and `\`; whitespace ends it; the rest of the
  line is ignored), resolved against the list's directory. A `\` that ends
  the line stays in the path, as `ffmpeg` keeps it (black box).
- A directive is the line's first word as written, neither unquoted nor
  unescaped: `'file' a.mp4` and `fi\le a.mp4` are unknown directives, as
  they are to `ffmpeg` (black box).
- `ffconcat version 1.0` is accepted on any line; other versions are
  refused. Empty lines and lines whose first token starts with `#` are
  skipped.
- With `safe` (the demuxer's default; `-safe 0` turns it off), a path
  must be relative, and each component non-empty, of letters, digits,
  `.`, `_` and `-`, and not start with `.`.
- Refused, with the line: every other directive, documented (`duration`,
  `inpoint`, `outpoint`, `option`, `stream`, ...: each changes what is
  read or where a file is placed) or not; a path that names a protocol
  (`file:`, `https:`); `file` without a path; a list with no file; a list
  that is not UTF-8.
- At most `MAX_LIST_BYTES` (1 MiB) of the list file is read, and a longer
  one is refused. That bound also limits every line, every path and the
  number of files, and it makes an endless file such as a device fail
  at the limit.

Each rule was checked against the pinned `ffmpeg` as a black box (the
same list accepted or refused, the same files read); the unit tests of
`crates/coreav-cli/src/concat.rs` hold the cases.

## Reuse

`crate::clip` provides track selection and checks, sample entries,
timelines and the copy window; `crate::moov` the file head, edit lists and
chunk interleaving; `crate::tables` the output tables. Joining adds the
compatibility checks, the placement, the edit lists of several edits and a
copy run that goes from one input to the next.

## Verification

- **Round trips** (`tests/join.rs`): a split's segments joined back give
  the source's samples, times and edits; two clips with pre-roll that end
  inside a group of pictures; a later input with an empty edit; video only;
  every refusal with its input and difference. Each output track must hold
  the inputs' samples in order, and the output's edit lists, read by the
  test independently of the writer, must present every sample an input
  presents at its time plus the offset and nothing else. Every fixture is
  split every second and joined back, two of its clips are joined, and every
  pair of fixtures is joined or refused with a mismatch.
- **Black-box oracles** (`scripts/join_check.py`, in the fixtures CI job):
  a join of a source's segments must give back the source in `ffprobe`'s
  packet lists (data, size, keyframe flag, times) and `ffmpeg`'s decoded
  video (the frames the source presents before the joined video's end) and
  audio frames. Any other join: `ffprobe` and MediaInfo read the
  output with the first input's codecs and properties; for each stream the
  output's packets are the inputs' packets in order; the decoded video is
  the inputs' frames at their times plus their offsets; the inputs follow
  one another without gaps (each starts where the previous one's video
  ends, its frames lie in its span, and the span is no longer than the
  input presents on its own, from its own file, but for the hold before an
  input whose audio starts first); the output's edit lists, read from the
  file, end its video where the last input's video ends and no later than
  that input's file presents it, and its audio no later than the last
  input's file presents audio (earlier where gapless information trims the
  padding; an audio edit cut short leaves packets out, which the coverage
  shows), with track and movie header durations to match; the decoded
  audio is the inputs' packets decoded in sequence and presented by the
  output's edits. Known
  divergences: [`known-divergences-join.md`](../known-divergences-join.md).
  Each packet known divergence needs every input packet present once and
  each moved input's times off by the one documented constant in `pts` and
  `dts`; `tools/oracle/tests/test_join_kd.py` runs bug-shaped changes to
  the output's packet listing (a packet left out or added, an isolated
  `pts` or `dts` error, other data, size or keyframe flag, an input moved
  past its bound, the output's smallest composition offset moved alone or
  with the decode times it moves) through the comparison and the
  predicates, and each must stay an uncovered difference (the earlier method).
- **Inputs with several edits** (`scripts/join_check.py`): where an input,
  or the source of joined segments, has several edits, the checks above
  that read ffmpeg's edit-applied view give way to those of common
  decision 3 of `docs/design/edit-list-writers.md` ("Join").
- **`ffmpeg -f concat`** (`scripts/join_check.py --ffmpeg-concat`, in the
  fixtures CI job): the inputs of every join, joined by `ffmpeg -f concat
  -safe 0 -i LIST -c copy` as a black box, give the same packets per kept
  stream (data hash, size, sync flag), one for one. Where ffmpeg converts
  H.264 to Annex B on the way (its `auto_convert` option, on by default),
  it puts the parameter sets into sync samples; those packets agree once
  both files are read without NAL units of types 7 and 8. Not compared:
  inputs whose kept streams sit at other stream indexes than the first
  input's (the demuxer pairs streams by index), joins with several edits
  (its demuxer applies its own edit model, which drops and repeats samples
  near edits), and inputs ffmpeg refuses to join. Presentation times are
  not compared: ffmpeg places each file at the end of the previous file's
  duration, the join at the end of its video.
- **Fuzzing**: the `join` target of `crates/coreav-isobmff-write/fuzz` cuts
  arbitrary input into two files and joins them; the join either fails or
  writes an output that parses and holds both files' samples once.

### Corpus bars (fixed 2026-10-19, before the corpus run)

Run in `docs/spikes/join-bench/`, with the results there.

- **Inputs**: the corpus v1 files of `docs/spikes/clip-bench/evidence/inputs.json`
  (869 files), as for the split, with the default limits, allowlist and
  audio choice, and the split's `no_audio` rule.
- **Joins per file**:
  - *segments*: the file split every 10 s (`docs/design/split.md`) and the
    segments joined back, checked against the source;
  - *clips*: its head clip `[0, 10 s)` and tail clip from `D - 10 s` to the
    end, the clip writer's corpus clips (`D` the video duration; files of
    20 s or less skipped), joined, checked against the
    two clips.
- **Checks**: the CoreAV round trip of the tests and the packet coverage of
  `join_check.py` on every join; the decoded video and audio on files of at
  most 500 MB.
- **Bars**:
  - **Coverage: 100% of joins.** Every input sample in the output once, in
    order, by both the round trip and `ffprobe`'s packet hashes. Any miss is
    a bug and blocks the merge.
  - **Segments**: at least **99% of files** give back their source on every
    check, directly or through a known divergence.
  - **Clips**: at least **98% of joins** agree on every check, directly or
    through a known divergence.
  - At least **99% of packets** agree over all joins.
  - Joins the writer refuses (an input the clip writer refuses, or a tail
    clip that starts at an open-GOP sync sample) are counted and listed
    apart, in neither denominator; so are objects that fail their SHA-256
    check. Every other failure counts as disagreement and is investigated.
- **Speed against FFmpeg** (held-out files only): per file, the wall time
  of `coreav join -o OUT SEGMENTS...` on its 10-second segments against
  `ffmpeg -f concat -safe 0 -i LIST -c copy OUT` on the same segments, both
  as processes on the same local disk, the median of 5 rounds with the order
  alternated after one warm-up of each. Bars: the median of
  ours ÷ ffmpeg over all held-out files at most **1.00**, and the median of
  each class (non-fragmented, fragmented, longer than one hour) at most
  **1.00**. A claim that joins are faster than FFmpeg needs the upper bound
  of the median's 95% bootstrap interval below 1.00.
- **Memory** (held-out files): peak resident memory of `coreav join` on the
  segments at most twice the segments' total `moov` bytes plus the output's
  `moov` bytes plus 8 MiB, on every held-out file; reported against the
  file size.
- Misses on speed or memory block the release, not the merge; the cause is
  recorded before any bar changes.

### Speed guard in CI

Every user-facing operation has a CI regression guard: the CI benchmark guard (`scripts/bench_guard.py`,
`docs/design/benchmarks.md`) counts the instructions of `coreav join` with
Valgrind, as for every other command, so a pull request that slows joins
fails before the corpus speed bar would notice.

- **Cases:** `join`, the one-second segments of `basic.mp4` (two, with
  audio) joined back into one edit; `join-fragmented`, the segments of
  `fragmented_many.mp4`, whose inputs are read fragment by fragment; and
  `join-clips`, two clips of `basic.mp4` (0.3 to 1.1 s and 1.1 to 1.9 s)
  that keep their own edits, so the output holds one edit per input. Each
  joins to standard output, which is discarded.
- **Method:** the guard writes the inputs first with the same binary
  (`coreav split` and `coreav clip`, into a new directory under its work
  directory, not counted) and counts `coreav join` on them, beyond
  start-up. Two runs of the same build count the same on the development
  machine.
- **Baseline and tolerance:** the guard's own. The entries in
  `bench/guard/baseline.json` come from a `workflow_dispatch` run of the
  guard with `update: true` on the runners (run 39384585652; rustc 1.99.1,
  Valgrind 3.22.0):
  `join` 851,521 instructions, `join-fragmented` 7,500,330, `join-clips`
  862,497 beyond start-up. A case fails when its count exceeds the
  baseline by more than the guard's tolerance (5%).
- **What it does not catch:** costs that grow with the number of inputs
  or the file's size beyond what these files show, and I/O or memory; the
  corpus speed and memory measurements above cover those.
