# Matroska and WebM demux (`coreav-mkv`)

Status: Accepted. Scope: MKV and WebM and
stage 3 ("Matroska and WebM demux (`coreav-mkv`), pulled forward from stage 5
for probe and the WebM-to-MP4 remux, with the stage 3 criteria on fixtures
plus at least 50 real files").

## What it does

- `probe(source, limits, allowlist)` reads the EBML header, the Segment and
  its metadata (`SeekHead`, `Info`, `Tracks`, `Attachments`), records where
  the first Cluster and the `Cues` are, and returns a `Segment` with its
  tracks. It never reads clusters, except the first keyframe header of
  tracks whose header lacks what that frame holds (below).
- `Segment::packets(source, limits)` lists every frame of every kept track in
  storage order: one packet per frame, laced frames split, with byte range,
  timestamps and flags.
- `Segment::scan_tracks(source, limits)` lists every frame once for each
  track's span, frame bytes and verified picture size, for `coreav check`
  (`docs/design/policy-matroska.md`).
- `Segment::index(source, limits)` builds a seek index from `Cues`, or from a
  cluster scan when there are none; `Index::cluster_for(track, time)` and
  `Segment::packets_from(source, limits, cluster)` seek.
- `coreav probe` detects the container from the first four bytes and prints
  the Matroska document (`coreav.probe-matroska/1`) or the ISO base media
  one (`coreav.probe/1`).

## Specifications

Clean room. Written from RFC 8794 (EBML), RFC 9559 (Matroska), the IETF
CELLAR Matroska codec mappings (draft-ietf-cellar-codec-20: `CodecID` values
and what `CodecPrivate` holds), the WebM container guidelines (the `webm`
DocType and the VP9 codec feature metadata), the VP9 bitstream specification
(section 6.2, uncompressed header) and RFC 6386 (section 9.1, VP8 frame tag).
`ffprobe` and MediaInfo are black-box oracles only. The codec private records
that Matroska shares with ISO base media files (`avcC`, `hvcC`, `av1C`,
`AudioSpecificConfig`) are read by `coreav-isobmff`, which gains a small
public function for that (`describe_config_record`,
`describe_audio_specific_config`); there is no second copy of those readers.

## Design

### Layers

- `ebml`: sans-IO variable-size integers, element headers (`Parsed::Done` or
  `NeedMore`) and the element value types. IDs are 1 to 4 octets (RFC 9559
  4.3); a data size may be unknown (all ones).
- `block`: sans-IO Block and SimpleBlock headers and the three lacings.
- `segment`, `track`, `packets`, `index`: the I/O shell over a `Source`
  (positioned reads), the same trait and `BufferedSource` window as
  `coreav-isobmff`. `coreav-io` does not exist yet; reusing the trait keeps
  one `Source` for both containers instead of a second copy.

### Probe

- The walk reads top-level elements in order until the first Cluster, then
  follows `SeekHead` entries for what it has not seen (Info, Tracks, Cues,
  Attachments, Chapters, Tags, a second SeekHead). That is what a range
  reader wants: the head of the file plus a few targeted reads.
- Masters read into memory (EBML header, `SeekHead`, `Info`, one `TrackEntry`
  at a time, one `CuePoint` at a time) are bounded by `max_element_bytes`.
  `Tracks`, `Cues`, `Attachments` and Clusters are walked element by element
  through the source; `FileData` and block payloads are never read during a
  probe.
- `Tags` and `Chapters` (for `coreav ffprobe`, `docs/design/compat-cli.md`)
  are read only by `probe_with_metadata`, as are each attached file's
  `FileUID` and `FileDescription`; `probe`, which `coreav probe`, the
  remux, `check` and the WebAssembly operations call, only notes that they
  exist (`has_tags`, `has_chapters`). They are often stored after the
  clusters, so reading them for every caller would cost those paths reads
  and work they never use (the benchmark guards measured 5 to 16% more
  instructions on a probe). Rejected: a flag in `Limits` (limits bound
  work, they do not choose what is read). `probe_with_metadata` walks them
  through the source too: each `Tag` and each `EditionEntry` is
  read into memory on its own, bounded by `max_element_bytes`, and one
  larger than that is left out rather than failing the probe (a cover
  image stored as a `TagBinary` must not reject the file). Nested
  `SimpleTag` and `ChapterAtom` elements deeper than 8 levels are left out;
  the strings kept count against `max_retained_bytes`. `TagBinary` is
  sized, not kept.
- Missing `Info` or `Tracks`, a TrackEntry without TrackNumber, duplicate
  TrackNumbers, `EBMLReadVersion` above 1, `DocTypeReadVersion` above 4 or a
  DocType other than `matroska` or `webm` are errors. A segment whose
  declared size runs past the file is read to the end of the file with a
  warning, as is metadata cut off after the tracks.
- Unknown-sized elements are allowed only for the Segment and Clusters (RFC
  9559 schema). An unknown-sized Cluster ends at the next top-level element
  or EBML header, or at the end of the segment (RFC 8794 6.2).
- A second EBML document (chained segments) ends the walk; this crate reads
  the first Segment.

### Codecs and the allowlist

- `Codec::from_codec_id` maps every `CodecID` the codec mappings define for
  the codecs CoreAV names; anything else is `Other`, which no allowlist a
  decoding caller builds can allow (as in `coreav-isobmff`). The one
  exception is `Allowlist::any()`, for `coreav ffprobe`, which describes
  every track and lists its packets but hands no frame to a decoder.
- Default allowlist: the codecs in CoreAV's scope as Matroska carries them (H.264,
  HEVC, AV1, VP9, ProRes, AAC, MP3, Opus, PCM), the audio the remux copies
  into MP4 (FLAC, AC-3, E-AC-3, Apple Lossless), WebM's own codecs (VP8,
  Vorbis), and the text subtitle formats (SubRip, ASS/SSA, WebVTT). A track
  outside it is dropped and reported; strict mode rejects the file. Callers
  widen the list explicitly (`Allowlist::all()` names every recognized
  codec, which the oracle adapter uses).
- Profile and colors come from `Colour` first, then from `CodecPrivate`:
  the H.264 and HEVC sequence parameter set (VUI colour description) and the
  AV1 sequence header in the `avcC`, `hvcC` and `av1C` records, the AAC
  `AudioSpecificConfig`, and the VP9 feature metadata. The legacy
  `A_AAC/MPEG2/...` and `A_AAC/MPEG4/...` codec IDs name their profile and
  carry no `CodecPrivate`.
- Some values live only in the first keyframe, so the probe reads its first
  1 KiB (among the first 4,096 elements of the clusters, best effort): VP8
  and VP9 profile, bit depth, matrix and color range (frame header); the H.264
  sequence parameter set when there is no `CodecPrivate` (Chrome's
  MediaRecorder writes H.264 in Matroska as Annex B with in-band parameter
  sets); and the AV1 sequence header when the `av1C` has no configuration
  OBUs (Chrome writes a constant level 9 in the `av1C` header; the sequence
  header wins, as in `coreav-isobmff`). The first frame flagged a keyframe is
  read, else the track's first frame, which is a key frame for these codecs
  even where a writer left the flag unset. Best effort covers the file, not
  the source: damaged or missing clusters leave the values unknown, but a
  read the source fails fails the probe with its I/O error, as every other
  read does. Rejected: leaving the values unknown on any error,
  which reported a probe of a failing source as a clean result. The VP9
  `color_space` maps to the ITU-T H.273 matrix the way the VP9
  specification names the standards
  (BT.601 as 5, BT.709 as 1, SMPTE 170M as 6, SMPTE 240M as 7, BT.2020 as 9,
  RGB as 0).
- `V_MS/VFW/FOURCC` tracks are named by the `biCompression` code of their
  `BITMAPINFOHEADER`, and VobSub tracks report the `size:` line of their
  `.idx` lines as width and height.
- Rotation is `ProjectionPoseRoll` (counter-clockwise, RFC 9559 15.2) when
  the projection is rectangular with zero yaw and pitch and the roll is a
  multiple of 90 degrees; otherwise it is unknown and the raw pose is
  reported.

### Packets and timestamps

- One packet per frame; frames of a laced block are separate packets that
  share the block's offset (`block_offset`) and know their own byte range.
- The block timestamp is `Cluster\Timestamp` plus the block's relative
  timestamp, in Track Ticks (`TimestampScale` nanoseconds), kept as stored.
- The presentation timestamp subtracts `CodecDelay` (RFC 9559 11.2), rounded
  to the nearest tick, halves up, so that it stays an integer in the stream's
  time base. A later frame of a lace adds its position times
  `DefaultDuration`, truncated to the tick in which the frame starts; without
  `DefaultDuration` its timestamp is unknown (RFC 9559 10.3.5:
  "underdetermined"). `Packet::pts_ns` gives the exact time in nanoseconds.
  Lacing is common in real files: mkvmerge laces audio by default.
- Matroska stores no decode timestamps. The decode timestamp equals the
  presentation timestamp for codecs that decode in presentation order (audio,
  subtitles, VP8, VP9, AV1 temporal units, intra-only video) and is unknown
  for H.264, HEVC, MPEG-1/2/4 video and Theora. Deriving it for those
  (needed to write MP4) belongs to the remux, which can see the
  whole presentation order.
- Keyframe: the SimpleBlock keyframe flag, or a BlockGroup without
  `ReferenceBlock` (RFC 9559 10.4), as stored.
- `TrackTimestampScale` other than 1.0 makes presentation timestamps unknown
  for that track, with a warning.
- Header stripping (`ContentCompAlgo` 3) is undone by reporting the stripped
  bytes with each packet. Encrypted tracks list their frames as stored, with
  a warning; their byte ranges are exact and their content opaque.
  zlib-compressed frames (`ContentCompAlgo` 0), which mkvmerge still writes
  for image subtitles (VobSub, PGS), are listed with their stored byte
  ranges, with a warning. A listing that asks
  (`Packets::with_inflated_sizes`: the frame-size scan behind `check` and
  `probe --scan-frame-sizes`, and the oracle adapter) also gets their
  inflated sizes, the sizes a decoder sees and `ffprobe` reports, by
  inflating each frame from the file in 64 KiB reads into a 32 KiB window
  that wraps (`miniz_oxide`, which `coreav-isobmff` already uses for
  compressed movies), with one decompressor reset per frame, so memory
  stays constant. Each frame is bounded by `max_packet_bytes`, and the
  listing, across its tracks, by `max_inflated_bytes`, against which each
  frame is charged what it inflated to (all of the bound for a frame that
  went past it) plus 4 KiB, so a flood of empty frames is bounded too, and
  so is a file of many tracks of bombs. A broken frame costs only what it
  inflated, so only real inflation drains the budget; a bomb that does
  leaves the later zlib frames of every track with unknown sizes, which
  fails closed. The budget is far above the corpus's tracks: the largest
  image subtitle track (430 VobSub frames, 1.2 MiB inflated) is charged
  about 3 MiB; the corpus has no multi-language Blu-ray remux with many
  PGS tracks, where it would be tightest. Rejected: a budget per track,
  which let the work grow with the number of tracks (up to 1,024 times
  256 MiB). A
  frame that does not inflate within these, or is not a complete zlib
  stream, has an unknown size and the listing goes on: the scan marks its
  track not listed, so the policy check's `invalid_tables` rule rejects
  it, and a remux or the probe, which never inflate, are not affected. A
  broken frame affects only its track; a frame that goes past its bound is
  charged all of it, so bombs can empty the budget for every track, as
  above. Bytes after the end of a frame's zlib stream are not
  read. Nothing reads their content: the frame-size scan and the
  first-frame headers skip them as they skip encrypted frames, and a remux
  refuses to copy them.
  bzlib, lzo, chains of encodings and encodings of the codec private data
  leave the track without packets, with a warning. Rejected: keeping zlib
  tracks without packets, which hid their frames from the listing and the
  policy check's frame count for a mainstream subtitle encoding; and
  listing them with their stored sizes, which differ from the frames';
  inflating in every listing, which made a flood of empty frames cost
  seconds in the probe and the remux; failing the whole listing on a frame
  that does not inflate, which broke unrelated tracks.
- A file that ends inside a Cluster or a block yields every frame of every
  complete block, then `Error::Truncated` once; a block cut off by the end
  of the file yields none of its frames, laced ones included.

### Seek index

- `Cues` are read one `CuePoint` at a time; points with a cluster position
  outside the segment are dropped with a warning. With no `Cues` (live
  recordings, MediaRecorder), a cluster scan reads each Cluster's header and
  `Timestamp`; known-sized Clusters are skipped by size, unknown-sized ones
  are walked element by element (headers only) to find their end.

### Limits

| Limit | Default | Bounds |
|---|---|---|
| `max_elements` | 1,000,000 | element headers read while probing or indexing Cues |
| `max_element_bytes` | 1 MiB | one element read into memory, and one block's header and lacing |
| `max_retained_bytes` | 16 MiB | codec private data and strings kept in the result |
| `max_index_bytes` | 256 MiB | seek points kept by `index` |
| `max_packet_bytes` | 128 MiB | one frame |
| `max_block_work` | 2^26 | element headers visited per packet iteration or cluster scan |
| `max_tracks` | 1,024 | tracks in one segment |
| `max_inflated_bytes` | 256 MiB | a listing that sizes zlib-compressed frames, across its tracks: inflated bytes plus 4 KiB per frame |

These are the Matroska and EBML defaults the limits policy left open. There is no depth limit:
the reader descends only into the masters it understands, at fixed depths
(at most five below the Segment), and skips every other element by its size,
so nesting costs no work. Lacing holds at most 256 frames by construction.
Memory is bounded by the metadata, the seek points and one block header,
never by the file size.

### Output

- `coreav.probe/1` describes ISO base media files and its keys are
  MP4-shaped (`major_brand`, `handler`, `fourcc`, `edit_list`). Matroska
  files print `coreav.probe-matroska/1`, documented in
  `docs/schema/probe-matroska-v1.md` with a JSON Schema twin. MP4 output is
  unchanged byte for byte.
- `coreav ffprobe -of json -show_format -show_streams` prints the
  ffprobe-compatible document. Native probe output stays separate.
- `--allow` accepts the names of both containers' codecs; names a container
  does not have are ignored for that container.

## Verification

- Unit tests on the RFC examples (VINTs, element IDs, the three lacing
  tables; Table 38's offsets are one octet short, the test says how), tests
  on hand-built files, and hostile-input tests: size bombs, every limit,
  every truncation, byte mutations, 50,000-deep nesting, and a pinned 2.2 GB
  virtual live recording listed with reads of at most 1 MiB. Fuzz targets
  `probe`, `packets` (with the index and a seek) and `block` run 60 seconds
  each on `main`, 20 on a pull request (temporary).
- Fixtures: `scripts/gen-fixtures-mkv.sh` makes Matroska and WebM files with
  `ffmpeg` (codecs, B-frames, rotation, color, multitrack, subtitles,
  attachments, live recordings, Cues at the front, CRC-32, truncation) and
  rewrites some with `scripts/mkv_craft.py` into structures ffmpeg never
  writes (Xiph, EBML and fixed lacing, header stripping, BlockGroups with
  ReferenceBlock, another TimestampScale, unknown-sized Clusters). The
  `mkv-fixtures` workflow compares streams and packets with `ffprobe` through
  `tools/mkv-adapter`, applies the known divergences of
  `docs/known-divergences-mkv.md` with `scripts/oracle_kd_mkv.py`, checks
  every `coreav ffprobe` key with `scripts/compat_check.py`, validates the
  native output against its schema, and runs MediaInfo as the informational
  second oracle.
- Corpus: `tools/mkv-adapter/corpus.py` streams the corpus v1 Matroska files
  from R2 and compares them file by file (the comparator holds a file's
  packets in memory). Browser recordings come from
  `tools/mediarecorder/generate.py` ([`docs/corpus.md`](../corpus.md#mediarecorder-recordings)).

### Corpus pass bars (fixed 2026-10-19, before the corpus run)

Inputs:

1. Real files: the corpus v1 `real` files with extension `mkv` or `webm`, development and holdout splits, plus the browser
   MediaRecorder recordings made by `tools/mediarecorder` (Chrome; at least
   30 recordings over codecs, resolutions, frame rates, audio-only, pauses
   and chunked recording).
2. Robustness set: every Matroska file of corpus v1 (`real`, `edge` and
   `hostile`) and the MediaRecorder recordings.

Bars, against `ffprobe` 7.0.2 with the documented known divergences:

| Measure | Bar | Applies to |
|---|---|---|
| Stream oracle: files agreeing (direct or covered) over files `ffprobe` reads | at least 98% | input 1 as a whole, and its holdout split alone |
| Packet oracle: packets agreeing over all paired and unpaired packets | at least 99% | input 1 as a whole, and its holdout split alone |
| Crashes, panics, hangs (120 s per file) or non-structured failures | 0 | input 2 |
| Fixtures, streams and packets | 100% | CI, every change |

MediaInfo is reported three-way and never gates. Known
divergences are added only where CoreAV follows the specification and
`ffprobe` differs; each needs a predicate that checks the file
independently. Infrastructure spend: R2 reads only, under 10 USD.

### Speed bar (fixed before the run)

- **Operation:** `coreav_mkv::probe` with the library defaults on a
  buffered file source, from the open file to the `Segment`.
- **Baseline (gates):** libavformat `avformat_open_input` with the input
  format given (`matroska`), the least libavformat does to list the streams.
  **Reference (reported):** the same plus `avformat_find_stream_info`, the
  work behind `ffprobe -show_streams`, and the `ffprobe` command line.
  Gating on the cheaper call keeps the bar from passing on work the
  baseline does and CoreAV does not.
- **Inputs:** the held-out corpus v1 `real` Matroska and WebM files (69: 59
  WebM, 10 Matroska); classes WebM and Matroska (by the file's DocType).
  Files either side rejects are counted apart.
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

**Result: met.** The median ratio is **0.227** (95% CI 0.218 to 0.233) over
the 69 files: WebM 0.230 (60), Matroska 0.227 (9); at most 0.450 on any
file. Against `avformat_find_stream_info` 0.022, against the `ffprobe`
command line 0.008. Evidence: `docs/spikes/parity-bench/` (`probe-02`).

## Alternatives considered

- One container-neutral `coreav.probe/2`: cleaner for consumers, but a
  breaking change for every existing caller and for work in flight; a
  separate schema keeps MP4 output stable. Revisit at the stage 6 API
  review.
- A `coreav-io` crate for `Source` now: the right home eventually, but moving
  the trait touches every `coreav-isobmff` caller in parallel workstreams;
  depending on `coreav-isobmff` for it is the reversible choice.
- Codec-specific frame durations for later laced frames (Opus TOC, AAC
  frame length): the RFC leaves these timestamps underdetermined;
  `DefaultDuration` is the only container value, and the corpus shows how
  often lacing without it occurs.
- Resynchronizing after damaged data (scanning for the next Cluster ID):
  guesses at structure on hostile input; a damaged file is an error.
- Inflating zlib-compressed frames for their content (to copy or decode
  them): only their sizes are needed by any primitive today.

## Results

The corpus run passed every bar: 250 of 250 real files and 99.970% of 9.7
million packets (held-out split alone: 69 of 69 files, all but one of 3.3
million packets), no crash, panic or hang on any of the 464 Matroska files,
and 100% of the fixtures. Details, uncovered disagreements and timings are
in [`measurements/mkv-demux.md`](../measurements/mkv-demux.md).

