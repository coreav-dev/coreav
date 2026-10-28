# HLS packaging with CMAF segments cut on request

- Status: Accepted
- Code: `crates/coreav-isobmff-write/src/hls/`, `coreav hls` in `crates/coreav-cli`
- Scope: use case 7, which "serves HLS with fragmented MP4 (CMAF) segments cut on request from an index built once"; stage 3 stream-copy primitives

## What it does

A plain MP4 or MOV file in object storage is served as HLS without
pre-packaging:

- **Once:** `build_index` lists the source and returns an index (a few bytes
  per sample). The master playlist, the media playlists and the init
  segments come from the index alone.
- **Per request:** `HlsIndex::media_segment` reads one segment's sample
  records from the index (or, for a source with a segment index, the one
  movie fragment the entry names) and returns a `MediaSegment`: the `moof` box and the
  `mdat` header as bytes (a few hundred bytes to a few kilobytes), then the
  list of source byte ranges whose concatenation is the rest of the
  segment. A server sends the header and then streams the ranges from object
  storage or an HTTP range source straight into the response; nothing is decoded,
  re-encoded or buffered beyond one range read. `MediaSegment::write` does
  the same through one 64 KiB window for any `Source` and `Write`.

The core is sans-IO in the sense of the rest of the crate: index building
reads through the synchronous `Source` trait, and the per-request path needs no I/O beyond
the index bytes the caller hands it and the ranges it returns, plus, for a
deferred rendition, one small read of the segment's `moof`.

Command line, for local use and tests:

    coreav hls index INPUT INDEX [--segment-duration 6] [--video S|none] [--audio S|none] [--tag ETAG]
    coreav hls get INDEX INPUT NAME OUTPUT      # master.m3u8, video.m3u8, video-init.mp4, video-3.m4s, ...
    coreav hls package INPUT DIR                # the index and every resource, for static hosting and tests

`index` and `get` write a file, or standard output for `-` (then
`index` prints its summary on standard error), as the other commands do. A
file is written to a temporary file next to it and renamed into place, so a
failed command never leaves a partial output or destroys an existing one;
`get` checks the source's length before it opens the output, and refuses an
output that is the input or the index. `package` writes into a new or empty
directory and removes what it wrote if it fails. Segment
ranges are copied with `std::io::copy` between files, which Linux turns
into `copy_file_range`.

## Choices

### Renditions

One video track and at most one AAC track, chosen and refused as for clips: H.264 `avc1`, HEVC `hvc1`, AV1 and VP9; `avc3`,
`hev1` and ProRes are refused, as is non-AAC audio under the default audio
choice. A source without a video track (kept or dropped) is packaged as
audio alone. Each track is its own rendition with its own init segment and
media playlist; the audio is an `EXT-X-MEDIA` alternative rendition of the
one variant stream. Rejected: audio and video muxed into one segment. A
CMAF fragment holds one track, players handle separate renditions, and
separate renditions keep each segment's byte ranges in one track's chunks.

### Segment boundaries

- Video segments start at a sync sample presented after every sample before
  it in decode order (no frame of an earlier segment is presented after it),
  at the first such sample at or after each multiple of the target duration
  on the presentation timeline (default 6 s, `--segment-duration`). The
  grid keeps boundaries near multiples of the target whatever the GOP
  length; a cut moves the next target to the multiple after it, so a GOP
  longer than the target gives one segment per GOP. Rejected: a target
  counted from the previous cut. That drifts, and its boundaries depend on
  every earlier GOP.
- Sync flags are the container's, except where they say nothing: a track
  without a sync sample table makes every sample a sync sample (ISO/IEC
  14496-12 8.6.2), which some QuickTime writers rely on for inter-coded
  H.264. While every sample so far is marked sync, an H.264 or HEVC cut
  must also start with an IDR (H.264) or IRAP (HEVC) picture, read from the
  NAL unit headers of that one sample (a few small reads, in safe Rust).
  Rejected: trusting such flags (segments that start on a P picture), and
  checking the bitstream at every cut (open-GOP I pictures that a sync
  table marks are not IDR pictures, and are good cuts).
- An open GOP's leading samples (decoded after its sync sample, presented
  before it) belong to the segment of that sync sample. A cut is confirmed
  at the next sync sample, when all its leading samples have been seen. It
  is kept when every sample of the new segment is presented after every
  earlier sample, and the segment then starts at its earliest sample.
  Otherwise the cut is undone. A rendition with such a cut does not carry
  `EXT-X-INDEPENDENT-SEGMENTS`, because its leading samples need the
  previous segment. Rejected: refusing open GOPs (x265 makes them by
  default), and claiming independence anyway.
- Audio segments start at the first audio sync sample presented at or
  after each video segment's start (every AAC frame, unless a source marks
  some otherwise), so both playlists share one timeline; empty audio
  segments are merged into their neighbour. Audio alone uses the grid. The
  first segment of every rendition starts at 0 and the last ends where the
  rendition's presentation ends (its edit's end, or its last presented
  sample).
- `EXTINF` is the difference of consecutive segment starts, each rounded
  to the microsecond so that the sum never drifts; `EXT-X-TARGETDURATION` is the largest `EXTINF` rounded to the
  nearest integer (RFC 8216 4.3.3.1), at least 1.

### Sources with a segment index

A fragmented source whose `sidx` (ISO/IEC 14496-12 8.16.3) lists every
subsegment already says what the playlists need: where each subsegment
starts, how long it lasts, how many bytes it holds and that it starts at a
stream access point. The index then records each subsegment's byte range
instead of its samples (a *deferred* rendition), and a request lists the
movie fragments of that one subsegment. A subsegment holds one or more
fragments with their media data (8.16.1), and the boxes between them
(`styp`, `emsg`, a segment index of its own) are skipped. Building the index reads the
movie box, the segment index and the first fragment, which is the work
libavformat's open does on such a file; listing every fragment is not
(measured in `docs/spikes/w2-248/`).

- **The segments are the subsegments**, one each. The target duration
  (`--segment-duration`) does not apply to such a rendition: a subsegment
  is the unit the packager made addressable, and cutting inside one would
  mean reading it. The summary's `deferred` says which renditions these
  are. Rejected: grouping subsegments up to the target, which needs a
  request to read and merge several fragments for one segment; and cutting
  at a sync sample inside a subsegment, which needs every fragment read.
- **Each segment's `EXTINF`** is its `subsegment_duration` and its start
  the running sum from `earliest_presentation_time`: that is what
  `subsegment_duration` measures (8.16.3.3), so the segments partition the
  presentation timeline whatever the access point is. The presentation end
  is the last one's end. A source whose fragments' decode times (`tfdt`)
  run past the sums of its stored durations, which its index gives, is
  followed as indexed: segment starts then drift from the samples' times by
  those gaps (`KD-HLS-10`; 28 ms over 12 minutes on the corpus's one such
  file). Rejected: correcting the starts from every fragment's `tfdt`,
  which needs every fragment read (#248). `EXT-X-INDEPENDENT-SEGMENTS`
  is carried only
  where every reference states a `SAP_type` of 1 or 2, which present
  nothing before the subsegment's first decoded sample (Annex I); a type
  of 3 (open GOP) or 0 (not stated) leaves it out. `SAP_delta_time` is not
  read: packagers write it against the decode time as often as against the
  earliest presentation time, so that a `SAP_type` of 1 comes with a
  non-zero delta on real files. That a segment starts with a sync sample
  is checked against the fragment instead, for the first subsegment while
  the index is built and for every other when it is requested.
- **`BANDWIDTH` and `AVERAGE-BANDWIDTH`** come from `referenced_size`, the
  subsegment's bytes as the source stores them, rather than from the bytes
  of the segment CoreAV writes: RFC 8216 4.3.4.2 counts whole segment
  files. Those bytes are the packaged track's samples and nothing else's,
  because the movie must declare fragment defaults for one track only
  (below); the two figures then differ by the fragment headers alone — a
  subsegment may hold several movie fragments where the written segment
  holds one, and may carry a `styp`, an `emsg` or a segment index of its
  own. On the corpus's sources with a segment index they are within 1.1%;
  the oracle checks them within a tenth for such a rendition rather than
  the 1% it checks a listed one within.
- **What the segment index cannot give.** The maximum frame rate needs
  sample durations, so the **first fragment is read** and its shortest
  sample duration stands for the rendition's (a uniform packaging is what
  a segment index describes). The samples and their bytes are not counted
  at all: the summary reports `null` for both.
- **When the source is listed instead.** The deferred form is taken for a
  source packaged as **one** rendition only: two renditions would have to
  share a timeline, which needs their subsegments to start at the same
  presentation times, and the segment indexes of a multiplexed source
  name the same byte ranges, so neither rendition could say how many of
  those bytes are its own and the bit rates above would be the whole
  subsegment's for each. Rejected: deferring the video and listing the
  audio against it, which still reads every fragment and so saves
  nothing.
  For the same reason the movie must declare fragment defaults (`trex`)
  for **one** track only: a fragment that may hold another track makes the
  subsegment's bytes more than the packaged track's.
  The rendition qualifies when the track has no samples in the movie box,
  the movie declares fragments and `trex` defaults for it, exactly one
  segment index names it in the track's own timescale, every reference is
  media (not another index) with a non-zero duration and size,
  `starts_with_SAP` and a `SAP_type` of at most 3 (4 and above are
  gradual refreshes, which a player cannot start from), and the first
  subsegment lists on its own: it holds at least one movie fragment that
  says where its data lies, keeps that data inside the subsegment, starts
  with a sync sample and carries a decode time (`tfdt`), which a request
  takes from it. The edit must present every sample: no leading empty
  edit (it would shift decode times only the fragments carry), no media
  edit that ends before the media, and a media time inside the first
  subsegment and not at a sync sample within it that the listing would
  restart from.
- **The references must name every fragment, not just the first ones.**
  Probe reads the segment indexes that come before the first fragment, so
  a file written with one index per subsegment (`ffmpeg -movflags +dash`
  writes one each) offers only its first, which names one subsegment of
  many; taking it would package a fraction of the media and say nothing.
  The references are contiguous by construction, so it is enough that
  they begin at or before the first fragment and end where the media
  does: from the last reference's end to the end of the file there must
  be nothing but a movie fragment random access box or padding (`mfra`,
  `mfro`, `free`, `skip`, `wide`), at most eight boxes of them. Bytes
  that are not a box end the walk too, which is the case of a source
  whose references stop a little short of their own media. Such a source
  is listed, as it was before.
- **What changes for such a source.** Against the listing path the
  segments are the packager's rather than the target's, so the playlists,
  the `EXTINF` values and the segment contents differ; everything else
  (the init segment, the codec strings, the sample bytes) is the same. On
  the corpus this affects only fragmented sources with a segment index;
  every other input is packaged byte for byte as before.

### Timing

- Samples, sizes, durations, sync flags and composition offsets are copied
  as stored; decode times are the
  source's (`tfdt` of each segment is its first sample's decode time).
  Offsets go into a `trun` of version 1 when any is negative and of
  version 0 otherwise. Offsets of at least 2^31 that a version 0 `ctts`
  holds, which some writers use for negative values, are copied as the same
  32 bits, so every reader interprets them as it does the source's; segment
  boundaries and durations are planned with them read as signed, as players
  read them.
- Every sample is copied except those that no presented sample needs:
  - samples decoded after the last sample the edit list presents (readers
    that apply the edit list drop them; the clip writer's rule);
  - when an edit starts inside the media (a trimmed file), the video before
    the latest sync sample at or before the edit's start that every earlier
    sample is presented before (the clip writer's snapping; a sync sample
    presented exactly at the edit's start, or across it, is that sample:
    an earlier change).
  Unpresented samples, such as encoder priming or offsets read as large
  unsigned values, take no part in where segments start.
- Audio keeps every sample up to the last one presented, however early the
  edit starts. The pinned ffmpeg applies a fragmented track's edit
  media time counting from its first sample's decode time rather than from
  media time 0: an audio rendition that left out its leading samples would
  lose that much presented audio there. Rejected: leaving out audio that
  ends more than 0.5 s before the video's first copied sample (an AAC
  pre-roll), which is what ISO/IEC 14496-12 8.6.6 allows and what this note
  first chose; readers that count the edit from the first decode time,
  ffmpeg among them, would drop presented audio. The cost of keeping it:
  the first audio segment of a source trimmed far into its media holds the
  whole hidden lead, which a player downloads before it starts and which
  raises the audio's peak bit rate in the master playlist (still an upper
  bound, as RFC 8216 asks). Video has no such reading: ffmpeg presents a video
  rendition that leaves out a GOP at the source's times.
- A source whose edit starts inside a GOP keeps that GOP's hidden frames,
  which the first presented frame needs. hls.js 1.5 drops frames presented
  before 0, the GOP's sync sample included, and so starts such a video at
  its next sync sample; only re-encoding avoids that.
- The source's edit list is carried into the init segment, in media ticks
  (the init's movie timescale is the track's media timescale). Empty edits
  that are not whole ticks are rounded to the nearest tick. A media edit
  with a duration of 0, which in a movie with fragments lasts to the end of
  the media (ISO/IEC 14496-12 8.6.6; `coreav-isobmff` accepts it only
  there), keeps its duration of 0: the init segment heads a fragmented
  movie too.
- A leading empty edit (a stream that starts after the other) becomes a
  decode time shift instead, with a media edit that lasts that much longer,
  when no sample is presented during it. Per ISO/IEC 14496-12 the two forms
  present the same samples at the same times; hls.js 1.5 ignores empty
  edits but keeps decode-time gaps (measured below). When the shift would
  expose a sample, for example audio priming with a late start, the empty
  edit stays.
- Rejected: CMAF-style negative composition offsets (`trun` version 1,
  offsets reduced by the B-frame delay) and no video edit list. Measured on
  the fixtures, the pinned ffmpeg (7.0.2) presents such fragments later by
  the magnitude of the most negative offset of the first fragment, as it
  does its own `-hls_segment_type fmp4` output. hls.js 1.5 in Chrome
  honours a single media edit's media time, so the B-frame delay is removed
  either way. Copying the offsets as stored makes ffmpeg's decoded frames
  and times equal to the source's exactly, which is the verification the
  issue asks for. The divergence remains for sources whose own offsets are
  negative (`ctts` version 1), recorded as KD-HLS-1.

| Fixture | Source video − audio start | hls.js 1.5, as packaged |
|---|---|---|
| B-frames with a delay edit (`hls_h264`) | 0 | 0 |
| audio 0.278 s late (`clip_audio_late`) | −0.278 s | −0.278 s |
| both 0.5 s late, audio by 0.476 s (`edit_list`) | 0.024 s | 0 |
| video 0.3 s late (`clip_video_late`) | 0.3 s | 0 (hls.js starts its timeline at the video) |

### Boxes

- **Init segment:** `ftyp` (major `iso6`, compatible `iso6`, `cmfc` and the
  codec brand `avc1` or `av01` where the binding defines one), `moov` with
  `mvhd`, one `trak` (track 1: the source's sample entry rebuilt as for
  clips, its language, creation times, matrix and
  size; empty `stts`, `stsc`, `stsz`, `stco`; the edit list if any) and
  `mvex`/`trex` with no defaults. Durations are 0: the samples are in the
  fragments and the playlists give the durations. MediaInfo names the
  `iso6` major brand by its code rather than "MPEG-4"; the track fields
  are read either way.
- **Media segment:** `moof` (`mfhd` with the segment number plus 1; one
  `traf` with `tfhd` `default-base-is-moof`, `tfdt` version 1 and one
  `trun`), then `mdat`. `tfhd` carries a default duration when every sample
  has the same one and default flags for the samples after the first when
  they agree, so audio segments and video segments with one sync sample
  store sizes (and, for video, composition offsets) only. Sample flags:
  `sample_depends_on = 2` for sync samples, `sample_is_non_sync_sample`
  otherwise. No `styp` and no `sidx`: HLS addresses whole segments.

### Playlists

`EXT-X-VERSION:7`, VOD media playlists with `EXT-X-MAP` and `EXT-X-ENDLIST`.
`BANDWIDTH` is the sum of the renditions' peak segment bit rates as
RFC 8216 4.3.4.2 defines them (the largest bit rate of any run of
consecutive segments lasting between half and one and a half of the media
playlist's own target duration, whole segment files counted),
`AVERAGE-BANDWIDTH` the sum of the averages. `CODECS` comes from the stored configuration: `avc1.PPCCLL`
(ISO/IEC 14496-15 Annex E.2), `hvc1` with profile space, reversed
compatibility flags, tier, level and constraint bytes (Annex E.3),
`av01.P.LLT.DD` and `vp09.PP.LL.DD` (their bindings), `mp4a.40.N` (RFC 6381,
5 or 29 when SBR or PS is signalled). `RESOLUTION` is the display size:
the picture size the codec configuration's parameter sets bound
(`docs/design/frame-sizes.md`), else the sample entry's size, widened by
the pixel aspect ratio (`pasp`), else the track header's size, swapped
when its matrix turns the picture by a quarter turn. The bound is right for
understated sample entries and conformance windows; it leaves out left and
top crops (KD-HLS-9); `FRAME-RATE` the maximum frame rate, one over the shortest
presented sample duration but the last's (no later decode time constrains
it), left out when that duration is under half the mean, which a stray
sample of a tick or two gives rather than a frame rate. The audio rendition is named by its ISO 639-2
code; `LANGUAGE` is left out, because RFC 5646 (which RFC 8216 requires)
takes the two-letter code where one exists and the index has the
three-letter one. `CHANNELS` is the `AudioSpecificConfig`'s channel
configuration (2 for a mono configuration with explicit parametric stereo),
which decoders follow, not the sample entry's count; it is left out when
the configuration defers to a program config element. Resource names are
fixed and relative (`master.m3u8`, `video.m3u8`, `audio.m3u8`,
`video-init.mp4`, `video-N.m4s`, ...); `Resource` parses and prints them, so
a request handler routes by name.

### The index

Big-endian binary, versioned (`CAVHLSIX`, version 3): a header with the
source length, the caller's tag (for example the object's entity tag, at
most 255 bytes), the target duration and one record per rendition (codec
string, timescale, sample entry, track header values, edit, bit rates, the
offsets of its directory and records). Then per rendition a directory of
40-byte entries (start, decode time, sample count, sample bytes, where its
records are), and the records: per sample a flags byte, the size, and only
when it changes the offset (else the previous sample's end), the duration
and the zigzag composition offset, in LEB128. The first record of a segment
is complete, so segments decode independently. About 3 to 5 bytes per
sample; a two-hour 30 fps film with AAC at 48 kHz has about 550,000 samples,
so an index of about 2 MB and a directory of about 100 KB.

The index is read as untrusted input, since it may come back from object
storage changed. Every length, count and offset is checked before it
drives an allocation or a read. A deferred entry's byte range must lie
inside the recorded source length, its fragment must be a `moof` box of at
most `max_box_bytes`, and every sample the fragment lists must lie inside
that one subsegment, so a changed index cannot make a request read
anywhere else. The header is at most `max_box_bytes`.
Directories and one segment's decoded samples count against
`max_table_bytes`, and every sample against `max_packet_bytes`. Every
sample range must lie inside the recorded source length, and the codec
string, which is printed into playlists, may hold letters, digits and dots
only. `MediaSegment::write` refuses a source whose length differs from the
recorded one; callers should key the index by the object's version (the
tag). Rejected: a text (JSON) index, about ten times larger and slower to
read per request; and no index, re-parsing the source's `moov` per
request, which costs a megabyte-scale read and parse per segment.

### Memory and limits

Building reads the source through `coreav-isobmff` with the caller's limits
and allowlist. Memory is the index under construction (charged against
`max_table_bytes`) plus one segment's samples. A request holds the
directories, one segment's samples and header, and one 64 KiB copy window
(or nothing, when the caller streams the ranges itself). Neither depends on
the file size beyond the index.

## Verification

- **Unit and synthetic tests** (`src/hls/*`, `tests/hls.rs`): segment
  boundaries on GOPs shorter and longer than the target, open GOPs, audio
  alone, video alone, late streams with and without priming, the source tag,
  a changed source, every one-byte change and truncation of an index, and
  the table limit. Each round trip probes the init segment and all media
  segments concatenated with `coreav-isobmff`: all samples, with the source's
  bytes, sizes, durations, sync flags, composition offsets and presentation
  times (exact, within half a tick for a fractional empty edit), segments
  starting with sync samples and partitioning the presentation timeline.
- **Fixtures** (CI): the same round trip on every fixture at targets of 1 s
  and 6 s (`fixtures_round_trip`, with the required ones listed), and the
  black-box oracles of `scripts/hls_check.py` on every MP4, MOV and M4A
  fixture with MediaInfo, at 6 s:
  - the master playlist, read by ffmpeg's HLS demuxer, holds the source's
    streams and decodes to its frames;
  - per rendition, the concatenation is read by ffprobe with the source's
    codec, profile, dimensions, pixel format, sample rate and channels;
  - its packets (`-show_data_hash`) are all of the source stream's packets,
    in order, with the same hash, size and keyframe flag, at the source's
    presentation times;
  - `ffmpeg -copyts -f framemd5` of the concatenation equals the source's,
    video and audio;
  - every media segment, after the init segment, starts with a keyframe and
    holds the next run of the source's packets;
  - `EXTINF` values add up to the presented duration within one sample, and
    each segment starts on the playlist's timeline at its first sample's
    presentation time (audio within one frame);
  - MediaInfo reads the init segment, each segment after it and the
    concatenation with the source's format, profile, dimensions, sample rate
    and channels and frame counts equal to the packets.
  Threshold: 100%. Differences only through `docs/known-divergences-hls.md`:
  ffmpeg's shift of negative composition offsets in fragments (KD-HLS-1),
  its listing of samples an edit hides in fragments but not in sample
  tables (KD-HLS-2), its frame-aligned start of an edit that starts inside
  a frame (KD-HLS-3), its handling of `iTunSMPB` tags (KD-HLS-4, KD-HLS-5),
  implicitly signalled HE-AAC (KD-HLS-6), an AAC profile it names only
  for the output (KD-HLS-7), its decoding of sample tables and fragments to
  different frames, checked against its own fragmented copy (KD-HLS-8) and left or top crops (KD-HLS-9).
  Four new fixtures (20 s; closed GOPs with B-pyramid, open GOPs, HEVC, 5 s
  GOPs) give several segments at the default target.
- **Fuzzing:** `hls` packages arbitrary input and checks the round trip of
  every rendition; `hls_index` opens arbitrary bytes as an index and cuts
  every segment.
- **Player:** hls.js 1.5 in Chrome plays the packaged fixtures (H.264 with
  B-frames, open GOP, audio and video late; HEVC is not decodable by the
  headless Chrome here) with the audio and video buffered over the
  source's interval (table above).

### Corpus and speed bars (fixed before the runs)

Measured by `docs/spikes/hls-bench/` on corpus v1. Inputs: every
`set: real` file with extension mp4, mov, m4v or 3gp and a `v:h264`,
`v:hevc`, `v:av1` or `v:vp9` tag (the 869 inputs of the clip
measurements), and every `real` m4a or m4b file tagged `a:aac`.

- **Correctness:** `scripts/hls_check.py` on every input at the default
  6 s target, with MediaInfo, segments checked on their own at the first,
  middle and last segment (all segments on the fixtures).
  - At least **98% of packaged files** agree: every check agrees directly or
    through a known divergence.
  - At least **99% of packets** agree: over all files, agreeing packets ÷
    (agreeing + disagreeing), a packet being one source packet of a packaged
    rendition matched with its output packet by data hash, size and
    keyframe flag; a missing or extra packet counts as disagreeing.
  - Inputs refused for a stated reason (codec, sample entry, edit list,
    non-AAC default audio, decode-time gaps) are counted and listed apart
    and are not in either denominator. Any other failure counts as
    disagreement, and every failure is investigated and recorded.
- **Speed against FFmpeg,** on the held-out inputs, local files, one
  machine, median of 5 runs after one warm-up per engine and input, engine
  order rotated:
  - **Segment on request:** the middle video segment of each input. Ours is
    `coreav hls get INDEX INPUT video-N.m4s OUT` with a prebuilt index.
    FFmpeg's is `ffmpeg -ss START -i INPUT -t DURATION -map 0:v:0 -c copy
    -f mp4 -movflags frag_keyframe+empty_moov+default_base_moof OUT` with
    the same segment's start and duration. Both are command lines, so
    both pay process start-up. Bar: the median ratio ours ÷ ffmpeg is at
    most **0.5**.
  - **Whole package:** `coreav hls package` against `ffmpeg -i INPUT -map
    0:v:0 -map 0:a:0? -c copy -f hls -hls_segment_type fmp4 -hls_time 6
    -hls_playlist_type vod`. FFmpeg muxes both streams into one segment
    per interval, so this is a reference for a static packaging job, not
    equal work. Bar: the median ratio is at most **1.0**.
  - **Index:** time and index size per input, as a share of the source.
    Reported.
  - **Index against FFmpeg (fixed before the run).** No FFmpeg
    operation writes a playlist without writing every segment (the whole
    package above is that job). The work `hls index` does is reading the
    sample tables into an index of every packet's position, time and sync
    flag, then choosing the segments; FFmpeg's equal work is libavformat
    opening the file, which builds that packet index. So the index is
    measured in process against it, with W1's method (as
    `docs/spikes/large-tables/` ran it): engine A-hls (`probe-bench --hls`,
    `coreav_isobmff_write::hls::build_index` with the command's defaults,
    the index held until the request ends) against B-lean (`probe-libav
    --format-mov --lean`), both reading through a read-only mapping, each a
    long-lived process pinned to its own core, one warm-up pass and 15
    rounds. Inputs: the held-out inputs of `docs/spikes/hls-bench/` (265), round 1
    of this comparison (their index times were reported before, not
    compared). Classes: seven groups of the held-out inputs. Bar: the median
    of the per-input ratios A-hls / B-lean is at most **1.00** overall and in
    each category, over the inputs both open; "faster" only where the upper
    bound of the bootstrap 95% interval is below 1.00. Inputs `hls index`
    refuses are listed apart.
    (The engines ran pinned to one shared core, one request at a time,
    rather than each to its own core as written above; requests never
    overlap, so the timings are the same.)
    **Result: missed** (`docs/spikes/w1-227/`, run `w1-01`): 0.699 (0.610 to
    0.821) overall over 232 inputs, but group-a 1.967, group-c 1.406 and group-g
    2.971.
    - Group-g: its slowest inputs (4.2 to 36.8) are the seven fragmented files
      with a segment index, which libavformat's open lists by subsegment
      without reading the fragments, while the HLS index reads every
      fragment's run: not the equal work the bar assumed. The other nine
      group-g inputs miss too (median 1.50), from the per-sample cost below.
    - Group-a, group-c and group-g: building the index takes, per input, 3.35,
      2.59 and 2.71 times (medians) what listing the samples with the
      packet index takes (A-index), a cost per sample at every file size.

    An earlier change tracks the speed-up; its result needs a new held-out set.
  - **Index against FFmpeg again (fixed before the run).** The
    method above, unchanged: engine A-hls against B-lean, in process,
    through a read-only mapping, long-lived processes pinned to one core,
    one warm-up pass and 15 rounds, summarized by `docs/spikes/w2-248/`.
    Inputs: a draw of corpus **v2-heldout** whose
    rules and salt were committed before it was collected; `w1-01`'s set
    is spent, and so is each draw once a number comes from it. Classes: the corpus categories of
    that set (group-a, group-b, group-c, group-e, group-g). Bar: the
    median of the per-input ratios A-hls / B-lean is at most **1.00**
    overall and in each class, over the inputs both open; "faster" only
    where the upper bound of the bootstrap 95% interval is below 1.00.
    Inputs `hls index` refuses are listed apart. A class with fewer than
    five inputs is reported without a verdict, since one input would
    decide it; the overall bar still covers them.
    **Result: met overall and in every class with a verdict but group-a**
    (`docs/spikes/w2-248/`, run `w3-01`, on draw 2 of the set): 0.613 (0.497
    to 0.752) over 41 inputs, with group-c 0.613, group-e 0.465 and group-g
    0.500 — each, and the whole, faster than libavformat's open, since
    every one of those intervals ends below 1.00 — group-b 0.271 on its
    one input, without a verdict and not independent of draw 1 (below),
    and **group-a 1.788** (1.028 to 2.268). Beside the median, every speed
    report states the share of inputs CoreAV is faster on, meaning a
    ratio below 1.00: here **30 of the 41** (73%), with 9 of the
    11 it is slower on `group-a` and the other 2 `group-c`. A median says
    where the middle of the set is, the share how much of it is on each
    side.
    - The index build now costs 1.54 to 2.30 times the packet index of
      the same file (a median of 1.89), against 3.35 (group-a), 2.59
      (group-c) and 2.71 (group-g without a segment index) in run `w1-01`: the
      per-sample speed-up did what an earlier change asked of it. The six inputs with
      a segment index are the tightest group in the set, 0.475 to 0.522,
      where `w1-01`'s seven ran at 4.2 to 36.8 times libavformat's open.
    - Group-a misses because libavformat has little to lose there: CoreAV's
      own packet index is at 0.833 of its open on audio-only files,
      against 0.334 (group-c) and 0.264 (group-g), so an index build costing
      about twice the table walk lands near 2. The work the ratio
      measures is the index's own — the segment boundaries, the
      per-sample records and the serialized index — and the equal-work
      premise of the bar does not cover it on this class.
    - On draw 1, which run `w2-04` measured on the same `build_index`,
      the overall median was 0.902 (0.632 to 1.290), group-a 2.056, group-c
      0.857, group-g 0.702 and group-b 0.271, with no `group-e` input. CoreAV
      was faster on 18 of the 30 inputs compared there (60%), that draw
      holding 40 files of which `hls index` refuses ten. The bar is met
      overall on both draws; "faster than libavformat's open" holds on
      draw 2, whose interval ends at 0.752, and not on draw 1, whose
      interval reaches 1.290. The two overlap, and the difference is
      what the draws hold.
    - What the small classes rest on: `group-b` is one input, and it
      repeats a draw-1 file's structural signature — a different clip of
      the same recording session — so the two draws' 0.271 is
      not independent evidence; `group-g`'s five all have
      a segment index, and at five inputs the bootstrap interval is the
      sample's range.
    - The miss is recorded with its cause; the bar does not change.
      Draw 2 is smaller than its own rules aimed at (46 `real` files of a
      60-to-100 target, `group-b` at two and `group-e` at six), and two of
      those rules did not hold on it, for the reasons
      [`docs/corpus.md`](../corpus.md#v2-heldout) records.
  - **Index against FFmpeg a third time (fixed before the run).**
    The method above, unchanged, on the next unspent draw of corpus
    v2-heldout selected for this operation (`operation: hls` in the
    manifest; draw 4 is the thumbnail and decode draw and is not one),
    after the sample walk of
    [`docs/design/sample-tables.md`](sample-tables.md) removed the run
    record between the tables and the index's per-sample work. Bar: as above, the median of A-hls / B-lean at most
    **1.00** overall and in each class of five inputs or more; and,
    paired on the same inputs in the same run, the median of A-hls new /
    A-hls base at most **0.90** in the group-a class and **1.00** overall
    and in every class, a class median between 1.00 and 1.02 being
    reported as unchanged rather than as a regression. Every median is
    reported with the share of inputs CoreAV is faster on.
    **Result: met overall and in every class with a verdict but group-a,
    and missed in group-a again, by less** (`docs/spikes/w3-308/`, run `w4-01`,
    on draw 3 of the set): **0.622** (0.479 to 0.809) over 32 inputs,
    with group-c 0.463 and group-g 0.479 — both, and the whole, faster than
    libavformat's open — group-e 0.589 on two inputs without a verdict,
    and **group-a 1.547** (1.311 to 1.925). CoreAV is faster on **23 of
    the 32** (72%); the nine it is slower on are the eight group-a inputs
    and one group-g input at 1.080.
    - Paired against the base commit on the same inputs in the same run,
      the index takes **0.907** of what it took (0.852 to 0.935), and
      **0.805** on group-a: the walk of
      [`sample-tables.md`](sample-tables.md) took 22% off the group-a
      miss, from 1.991 to 1.547, and no class of the index is above
      1.00.
    - Group-a still misses for the cause an earlier change recorded, now smaller.
      CoreAV's packet index is at 0.352 of libavformat's open on
      audio-only files (0.850 before the change), so there is less under
      the index to win back; what is left above 1 is the index's own
      work — the segment boundaries, the per-sample records and the
      serialized index — which is **4.370** times the walk on this class
      (2.240 overall). That per-sample record is where the rest of the
      miss is, and it is an earlier change's remaining work.
    - The draw holds 46 `real` files: group-a 13, group-c 20, group-g 11, group-e
      2 and group-b 0, so group-e and group-b carry no verdict. Four
      audio-only files are refused by `coreav probe` (`KD-STRICT`: a top-level
      box past the end of the file, which the pinned `ffprobe` reads),
      and `hls index` refuses fourteen inputs in all.
    - The miss is recorded with its cause; the bar does not change.
  - **Memory:** the peak resident set of `coreav hls get` for the middle
    segment is at most **32 MiB on every held-out input**, independent of
    file size, and reported against ffmpeg's.
- Misses are recorded with their cause before any bar changes.

Results (`docs/spikes/hls-bench/`): every bar is met. 857 of 858
packaged files agree (99.88%; the other is a source the pinned ffmpeg
cannot decode) and every packet does; a segment on request takes 0.197 of
FFmpeg's time and a whole package 0.212 (medians, every class and every
95% upper bound below 1.00); `hls get` peaks at 4.4 MiB.

### The start of a trimmed source

The packager considered a restart only at samples the edit does not
present, so a sync sample presented exactly at the edit's start (five
corpus files: an edit 1 s into the media, on a sync sample) or across it
was passed over, and the rendition kept the GOP before it; and it left out
audio ending exactly 0.5 s before the video. The first version of the fix
followed the rule "Timing" then gave for audio (leave out what ends more
than 0.5 s before the video's first copied sample). Leaving out the earlier
GOP moved the audio's start too: the rendition then left out leading audio
packets that `ffprobe` lists as discarded, which that version let KD-HLS-2
cover at either end of the list.

First version. Bars, fixed before the run: on the corpus inputs of the correctness bar
above, with `scripts/hls_check.py` at the default target and MediaInfo,
`main` and this change on one machine with the same oracle binaries:
- every input that agrees with `main`'s packager and oracle agrees with
  this change's, and the five files above agree;
- every packet of every packaged rendition agrees, as before;
- KD-HLS-2's leading packets: `tools/oracle/writer_kd_audit_cli.py
  --oracle hls` on the corpus files that reach it covers and absorbs
  nothing.

Results (`docs/spikes/hls-bench/evidence/summary-344.json`): **the first bar
is missed** for two of the five files; the miss is final for this change.
Packaging all 904 inputs with both binaries changes the output of exactly
the five files; every other package is byte for byte the same, and the
oracle's new branch runs only where the first output packet differs from
the source's first, which `main`'s oracle already fails, so those verdicts
cannot change. Three of the five agree with this change, video starting at
the sync sample, and no packet of any of the five disagrees. The other two
leave out 12 and 15 discarded leading audio packets, and 13 and 16 of the
source's decoded frames differ. The branch of KD-HLS-2 that covered those
frames never required them to be unpresented; limited to frames before the
source's first presented packet, it covers neither file, because the
frames are presented audio. The cause: the pinned ffmpeg
applies the audio rendition's edit media time counting from its first
sample's decode time rather than from media time 0, so it drops that much
presented audio (0.348 s in one file). The rendition is as ISO/IEC 14496-12
8.6.6 reads it: with the edit's media time made relative to the first
decode time, ffmpeg decodes the source's 233 frames hash for hash. On
`main` any source whose audio is left out this way meets the same reading;
no corpus v1 input was one.

Second version: audio keeps every sample ("Timing"), and the KD-HLS-2
extension is withdrawn, so `scripts/hls_check.py` is `main`'s. Bars, fixed
before the run: on the corpus inputs of the correctness bar above, `main`
and this change on one machine with the same oracle binaries;
the spike's comparison script packages every input with both binaries and
compares every resource, the index included; `scripts/hls_check.py` at the
default target with MediaInfo runs with both binaries on every input whose
resources differ or whose refusal changes:
- every such input that agrees with `main`'s packager agrees with this
  change's, and the five files above agree;
- every packet of every packaged rendition of those inputs agrees, so each
  audio rendition lists every audio packet of its source;
- inputs whose resources are byte for byte the same keep their verdicts,
  oracle and outputs being the same.

Results (`summary-344.json`, `second_version`): every bar is met. Of the
904 inputs, the resources of exactly the five files differ from `main`'s:
the video segment, the master playlist (bit rates) and the index; every
audio segment is `main`'s, byte for byte. All five agree with both
binaries, with this change through KD-HLS-1 only (packet times, as with
`main`), video starting at the sync sample, without the 30 or 60 packets
of the GOP before it that `main` keeps under KD-HLS-2; no packet disagrees.
The final code, whose segment-index check also runs for an edit with a
media time of 0, packages all 904 inputs byte for byte as the measured
binary does, index included.

## Not covered

- Several audio renditions, subtitles, I-frame playlists, byte-range
  playlists of one packaged file, encryption, low-latency HLS and DASH
  manifests.
- Parameter sets in samples (`avc3`, `hev1`) and decode-time gaps inside
  a segment are refused, as for clips.
