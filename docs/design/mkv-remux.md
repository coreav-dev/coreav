# Matroska and WebM to MP4 by stream copy

Status: Accepted. Scope: use case 7, which "remuxes a WebM browser
recording to MP4", and the stage 3 stream-copy
primitives. Built on the Matroska demuxer (`design/mkv.md`) and the shared MP4
writer of the full-file remux (`design/remux.md`, the `mux` module); there
is no second MP4 writer.

## What it does

`remux_matroska(source, limits, allowlist, options, out)` in
`coreav-isobmff-write` rewrites a Matroska or WebM file as an MP4 file by
stream copy, `moov` first, written front to back to any `std::io::Write`,
and returns a summary of the kept and dropped streams. `coreav remux INPUT
OUTPUT` detects the container from the first four bytes and runs it for
Matroska and WebM, printing the same `coreav.remux/1` summary as for MP4
sources. No sample is decoded or re-encoded.

## Streams

- **Kept** (the codecs MP4 carries that names), each with the
  sample entry its ISO binding defines:
  - H.264 → `avc1` with the `avcC` of `CodecPrivate` (configuration
    version 1, at least 7 bytes). Without `CodecPrivate`, or with an empty
    one (Chrome's MediaRecorder writes H.264 in Matroska as Annex B byte
    streams with in-band parameter sets), the `avcC` is built from the first
    keyframe's sequence and picture parameter sets (at most 31 and 255), and
    every sample is rewritten from start codes to 4-byte NAL unit lengths
    (ISO/IEC 14496-15 5.3.2): the NAL units themselves, parameter sets
    included, are copied unchanged. When a later keyframe carries other
    parameter sets (a recording whose resolution changes), the sample entry
    is `avc3` instead, which tells readers to take the parameter sets from
    the samples (5.4.2.1); the `avcC` still holds the first ones.
  - HEVC → `hev1` with the `hvcC` of `CodecPrivate` (version 1, at least 23
    bytes; `hev1` allows parameter sets in the samples as well, which
    Matroska files often repeat).
  - AV1 → `av01` with the `av1C` of `CodecPrivate` (marker and version
    byte `0x81`), as stored.
  - VP9 → `vp09` with a `vpcC` (VP codec ISO binding, version 1) built from
    the WebM feature metadata, the first frame header (profile, bit depth,
    chroma subsampling) and `Colour`; level 0 when nothing states it. 4:4:0,
    which the binding has no value for, is dropped.
  - Opus → `Opus` with a `dOps` built from the `OpusHead` of `CodecPrivate`
    (Opus in ISOBMFF 4.3; RFC 7845 5.1), timescale 48,000.
  - AAC → `mp4a` with an `esds` holding the `AudioSpecificConfig` of
    `CodecPrivate`, or one built from a legacy `A_AAC/...` codec ID and the
    track's sample rate and channels; timescale = sample rate.
  - FLAC → `fLaC` with a `dfLa` holding the metadata blocks of
    `CodecPrivate`; Apple Lossless (`A_ALAC`) → `alac` with the magic
    cookie of `CodecPrivate`; AC-3 → `ac-3` with a `dac3` and E-AC-3 →
    `ec-3` with a `dec3`, both built from the first frame's headers;
    timescale = sample rate (`remux.md`, "Audio sample entries").
- **Dropped and reported**, with a precise reason, by default; `--fail-on-
  unsupported` fails instead, as for MP4 sources:
  - every other codec, for example VP8 ("VP8 in MP4 does not play in
    Safari; keep the WebM, which plays in Chrome, Firefox and Safari, or
    re-encode to VP9, H.264 or AV1 outside CoreAV": the VP codec binding
    defines `vp08`, but Safari 27 refuses such files while it plays the
    WebM, and Chrome plays them only from a file URL, with `canPlayType`
    and MSE reporting no support; an earlier change records the test and the decision
    not to write `vp08`), "MP4 has no standard sample entry for Vorbis",
    MP3 and PCM;
  - subtitle and data tracks, tracks whose frames are compressed
    (`ContentCompression` other than header stripping), encrypted tracks,
    tracks with `TrackTimestampScale` other than 1.0;
  - a stream without what its sample entry needs (a malformed
    configuration record, no parameter set in the first H.264 keyframe, no
    valid `OpusHead`, no `AudioSpecificConfig`, an unreadable first VP9
    frame header) or without frames;
  - a stream with a problem found while its frames are listed: an H.264
    frame without `CodecPrivate` that is not an Annex B byte stream (or has
    more than 65,536 NAL units), a video frame without a timestamp, an
    Apple Lossless packet without a valid element header, a frame
    above 4 GiB, a time beyond 2^62 ns (146 years, so that no time
    computation overflows), a sample that lasts too long for a 32-bit
    duration, a composition offset beyond 32 bits, a stream of which
    nothing would be presented (all priming or `DiscardPadding`: an empty
    edit list would present all of it).

  Tracks outside the allowlist are `codec_not_allowed`. A file with nothing
  to copy fails with the reason of the video stream it could not copy, else
  of the first dropped stream, or "no stream can be copied to MP4" when
  every stream is outside the allowlist.
- **A video stream is never lost silently.** When a video stream with
  frames cannot be copied, or the allowlist leaves it out, while other
  streams could be written, the remux fails before writing anything with
  `video stream N cannot be copied to MP4: REASON`, the same rule as clip
  and audio extraction: fail rather than silently drop. Asking for a
  video to MP4 conversion and getting an audio-only file (for example from
  a Chrome VP8 + Opus recording) is data loss from the user's point of
  view. `MatroskaRemuxOptions::drop_video`
  (`coreav remux --drop-unsupported`) asks for the other streams anyway,
  with the video stream listed among the dropped ones; naming streams with
  `--streams` is asking too. Audio, subtitle and data streams are dropped
  and listed by default, as for MP4 sources. A video stream without frames
  (cut off before its first frame) loses nothing and is dropped.
- **`BlockAdditions`** (for example the alpha plane Chrome records for
  transparent VP8 or VP9) have no place in MP4: the video is kept and each
  `BlockAdditions` element is listed in `dropped_boxes` of the summary
  (`Segment/Cluster/BlockGroup/BlockAdditions`, `not_carried`), at most 256
  (each stream lists up to 256 while the file is read, so that a stream
  dropped later cannot use up the kept ones' share), then counted.

## Timing

- Matroska stores presentation times only. Each kept track's samples are
  listed in storage order, which is decode order (RFC 9559 10).
- **Video:** media timescale = Segment Ticks per second (1,000 for the
  default `TimestampScale`), or 90,000 with times rounded to it when a tick
  does not divide a second. For every codec the decode times are the
  presentation times sorted, shifted back by the smallest amount that keeps
  every decode time at or before its presentation time (for VP9 and AV1,
  which decode in presentation order, nothing moves); the composition
  offsets are the differences (non-negative, `ctts` version 0).
  MP4 decode times must increase, so a repeated presentation time decodes
  one tick after the previous frame (and the shift grows to match); the
  presentation times stay those of the source.
  Durations are the differences of decode times; the last sample's is its
  `BlockDuration`, else `DefaultDuration`, else the previous sample's.
- **Audio:** media timescale = sample rate (48,000 for Opus). Each frame
  lasts what it decodes to: the duration its Opus TOC byte gives (RFC 6716
  3.1, a 1- or 2-byte read per packet), one AAC frame (1,024 or 960
  samples, from `frameLengthFlag`), the block size of its FLAC frame
  header (RFC 9639 9.1.1), the count an Apple Lossless packet's header
  states, else the cookie's `frameLength` (the header's layout is taken
  from Apple's ALAC reference source, `codec/ALACDecoder.cpp` of the
  macosforge/alac release, Apache-2.0, under a maintainer-approved
  clean-room exception of 2026-10-31 for the ALAC packet header layout
  only; `remux.md`, "Apple Lossless packets from Matroska"), or
  1,536 samples for AC-3 and E-AC-3. Matroska's millisecond times do not:
  Chrome's MediaRecorder writes 60 ms Opus frames at times that jitter by up
  to about 8 ms, so their differences would give 52 to 68 ms frames that do
  not match the audio. Times are accumulated from the first frame's, and
  the source's times are followed only when the next frame's time is more
  than half a frame away from where this one ends (a pause in a recording,
  a lost frame, times that do not follow the frames): then the frame lasts
  up to that time, rounded to the codec's quantum (120 samples, 2.5 ms, for
  Opus; one sample for FLAC; one frame for AAC, Apple Lossless, AC-3 and
  E-AC-3), at least one quantum. Each output time
  therefore stays within half a frame of the source's, except after the
  source's times go backwards: frames then last one quantum each until the
  source's times catch up. Frames without a timestamp (later frames of a
  lace without `DefaultDuration`) last what they decode to, and only the
  source's own times can move a frame; a frame whose duration is unknown
  (a malformed TOC) lasts up to the next frame's time, an interpolated one
  when the next frame has no timestamp either.
- **Edit lists:** each track starts where its first frame is presented on
  the segment's timeline: a positive start becomes an empty edit; the media
  edit starts at the first presented sample's composition time and lasts to
  the end of the last presented frame (for reordered video, the last
  presentation time plus the last frame's duration, so every frame is
  shown). `CodecDelay` (Opus pre-skip, AAC priming) is not presented: the
  media edit starts after it, as the Opus and AAC bindings describe
  priming; the last frame's `DiscardPadding` (Opus end trimming) is not
  presented either. No edit list is written when it would only restate the
  media.
- The movie timescale is the video timescale (Segment Ticks per second, or
  90,000), so empty edits keep the source's precision.

## One video stream replaced

`remux_matroska_replacing` is `remux_matroska` with one video stream's
samples taken from the caller (`coreav transcode`'s writer;
[`remux.md`](remux.md), "One video stream replaced", has the replacement,
its validation and the rejected alternatives). Matroska specifics:

- **Times** are Segment Ticks on the segment's timeline, the blocks'
  timestamps as `coreav_mkv::Packet::pts` gives them for video (no
  `CodecDelay`); the track must have a `TrackTimestampScale` of 1.0. They
  become media ticks of the video timescale above (Segment Ticks per
  second, or 90,000 with times rounded to it); two decode times that meet
  after that rounding, or a presentation time that falls before its decode
  time, fail with `Unsupported`.
- **Timing** is the video rule above with the caller's decode times
  instead of sorted presentation times: the composition offsets are the
  differences, the empty edit lasts to the first presented frame's time,
  and the media edit to the end of the last presented frame (the largest
  presentation time plus the last sample's duration). So a frame with
  presentation time `P` is presented at segment time `P`, where the plain
  remux presents a copied frame with that timestamp, whatever the decode
  times.
- **Kept:** the track's place, language, enabled flag and rotation, with
  the rotation's translation computed from the replacement's presentation
  size. None of the source stream's frames is listed or copied (its
  `BlockAdditions` are not reported), so its codec may be one MP4 cannot
  carry: a VP8 WebM recording's video can be replaced, and the
  lost-video rule does not apply to the replaced stream. The replacement's
  samples are charged against `max_table_bytes` as source samples are. A
  stream the allowlist leaves out fails with `CodecNotAllowed`, as for MP4
  sources.
- **The last frame's duration** in the media edit's end is the last
  decoded sample's, as for a copied stream (the MP4 path uses the frame
  presented last's own duration instead, since the source's edit list is
  kept there): the replacement then ends where the plain remux of the same
  frames ends.

## Cut-off recordings

A recording whose writer stopped early (a closed tab, a crash) ends inside
a Cluster or a block. The demuxer lists every complete frame and then
reports the truncation; the remux copies those frames and writes a
complete MP4 file, and the summary gives the offset where the source ends
(`source.truncated_at`). A file cut off before the frames of a stream drops
that stream ("the file ends before the stream's first frame"); one cut off
before any frame fails with `truncated_structure`. Truncation inside the
header (EBML header, `Info`, `Tracks`) fails as for `coreav probe`.

## Copying and memory

Two passes over the source, as the shared writer requires: the first lists
every packet once and keeps, per kept sample, a record of its source
offset, stored and output size, presentation and block times,
`BlockDuration`, frame duration and keyframe flag; each sample is charged
against `Limits::max_table_bytes` as the record's in-memory size plus 64
bytes for the per-sample vectors of the time computation, apart from the
output tables. It then computes the times above and pushes the samples; the
second copies the samples in the writer's chunk order through one 64 KiB
`CopyWindow`. Annex B samples are read twice in each pass (once to find the
NAL units, at most 65,536 per sample, once to copy), through the same
window. Memory is the per-sample records, the output tables, one sample's
NAL unit list and the window, never the size of the media.

## Verification

- Unit and integration tests in `crates/coreav-isobmff-write/tests/
  mkv_remux.rs` on hand-built Matroska files: every kept codec, the
  dropped-stream reasons, Annex B conversion, B-frame decode times,
  repeated times, Opus pre-skip edits, header stripping, `BlockAdditions`,
  live and cut-off recordings, other `TimestampScale` values, limits and
  write errors; CLI tests for the summary schema. A fuzz target
  `mkv_remux`: arbitrary input, alone or after a fixed header with VP9,
  Opus, H.264 and AAC tracks, fails or gives an output that
  `coreav-isobmff` probes, with every frame of each kept stream (the same
  bytes, or NAL units that cover each Annex B sample exactly), the same
  keyframes and increasing decode times.
- `scripts/mkv_remux_check.py` checks an output against its source with the
  black-box oracles:
  - layout: top-level boxes `ftyp`, `moov`, `mdat`, in that order; `ffprobe`
    reads an MP4 file;
  - kept: every source stream of a copyable codec (in `ffprobe`) is in the
    output unless the summary drops it as encrypted, without frames or with
    nothing presented;
  - streams: the kept streams are exactly the copyable source streams; each
    has the source's codec, profile, dimensions, colour description, sample
    rate and channels in `ffprobe`;
  - packets: `ffmpeg -c copy -f framemd5` of each kept stream has the
    source's packets in order with the same size and MD5; for Annex B H.264
    the decoded `framemd5` frames are compared instead (the bytes change
    form, the pictures must not, nor their count); each packet's
    presentation time equals the source's within 1 ms for video, and
    within 1 ms plus half the frame's duration (as ffmpeg reads it from
    the source packet) for audio;
  - presentation: every packet is inside the output stream's presented
    range (start time and duration from the edit list, in `ffprobe`), audio
    priming aside;
  - MediaInfo reads an MPEG-4 file with one track per kept stream.
  The summary marks Annex B streams (`streams[].annex_b`), so the checker
  does not guess.
- Fixtures in CI (`mkv-fixtures` workflow): every Matroska fixture
  (`scripts/gen-fixtures-mkv.sh`) is remuxed by `coreav remux`; the ones
  with nothing MP4 can carry must fail with the expected message, and
  100% of the others pass every check above; the summaries are validated
  against the schema.
- The corpus run: `tools/mkv-adapter/remux_corpus.py` (streams each file
  from R2, remuxes, checks, times `ffmpeg -c copy -movflags +faststart` on
  the same streams, deletes; `report` gives counts per split).

### Corpus bars (fixed 2026-10-19, before the corpus run)

- **Inputs:** the corpus v1 `real` Matroska and WebM files (209: 140
  development, 69 held out) and the 41 MediaRecorder recordings
  ([`docs/corpus.md`](../corpus.md#mediarecorder-recordings)), one remux each with the library defaults.
- **Refusals** (nothing MP4 can carry, for example VP8 with Vorbis) are
  counted by reason, apart from the denominators. Any other failure counts
  as disagreement and is investigated.
- **Pass**, on all remuxed files together and on the held-out split alone:
  - at least **99% of remuxed files** pass every check of
    `scripts/mkv_remux_check.py`;
  - at least **99.9% of the packets** of kept streams agree (a missing or
    extra packet disagrees);
  - **100%** of outputs have the `ftyp`, `moov`, `mdat` layout and are read
    by `ffprobe` and MediaInfo;
  - no panic, crash or timeout (one hour per file).
- **Reported, not gated:** dropped streams by reason; time and peak memory
  against `ffmpeg -c copy -movflags +faststart` on the same files.
- Infrastructure: R2 reads only.

### Speed bar (fixed before the run)

- **Operation:** `remux_matroska` with the library defaults (buffered file
  source in, 64 KiB buffered writer out), the streams it keeps by default.
- **Baseline (gates):** libavformat remuxing the same streams to MP4 by
  stream copy with `+faststart`, 64 KiB custom I/O on both sides
  (`lav_avio`). **Reported:** the
  same with libavformat's own file I/O, and the `ffmpeg -c copy -movflags
  +faststart` command line.
- **Inputs:** the held-out corpus v1 `real` Matroska and WebM files the
  remux accepts; refusals (nothing MP4 can carry) are counted apart.
  Classes WebM and Matroska.
- **Method** (the method of `docs/spikes/remux-bench/`, in `docs/spikes/parity-bench/`):
  one file at a time on the development machine, sources in the page cache,
  outputs on a tmpfs; per file, one warm-up run of every engine, then 5
  rounds with the engines' order rotated; a file's time per engine is the
  median of its 5 rounds; ratios are per file, ours over the baseline.
  CoreAV and the baselines are timed inside their processes, around the
  operation only. The baselines are libavformat 7.0.2 in-process (the pinned
  LGPL package of Spike C, written from the public API
  documentation), with 64 KiB custom I/O like CoreAV's buffered source.
- **Bar:** the median of the per-file ratios is at most **1.00** overall and
  in each class.

**Result: met.** The median ratio is **0.448** (95% CI 0.412 to 0.515) over
43 held-out files: WebM 0.455 (41), Matroska 0.346 (2). Against
libavformat's own file I/O 0.444, against the `ffmpeg` command line 0.185.
20 files were refused (19 with VP8 video, 1 with nothing MP4 can carry). On
6 Matroska H.264 files the baseline program stops with `Invalid argument`;
against the `ffmpeg` command line, which remuxes them, the median there is
0.252. Evidence: `docs/spikes/parity-bench/` (`remux-03`).

## Alternatives considered

- **Dropping every stream MP4 cannot carry by default, video included**
  (the MP4 sources' default): turned Chrome's VP8 + Opus recordings into
  audio-only files with exit status 0, which a caller can easily miss.

- **A second MP4 writer for Matroska:** forbidden by the issue and
  pointless; the shared `Muxer` takes any sample source.
- **Re-listing each track's packets in the copy pass** (what MP4 sources
  do): Matroska interleaves tracks in clusters, so every listing scans every
  cluster; keeping a record per sample from the first pass is cheaper and
  still bounded by the table limit.
- **`avc3` for every Annex B source:** `avc3` is less widely played than
  `avc1`, and with one set of parameter sets `avc1` describes the stream
  exactly. Only a parameter set change needs `avc3`: there, an `avc1`
  reader that configures its decoder from the `avcC` alone would decode the
  later pictures with the first resolution.
- **Refusing a cut-off recording** (`truncated_structure`): loses a whole
  recording for its last partial block.
- **Durations from Matroska timestamps alone, or rounded to a quantum:**
  millisecond timestamps make audio frame durations jitter (1,014 and 1,058
  samples for 1,024-sample AAC frames); rounding them to 2.5 ms still left
  Chrome's 60 ms Opus frames at 57.5 to 67.5 ms, because its times jitter
  by more than a quantum. The frames' own durations are exact.
