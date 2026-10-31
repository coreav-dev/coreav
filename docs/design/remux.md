# Full-file remux to MP4: faststart and MOV-to-MP4 normalization

Design note for `remux_mp4` and the `mux` module of
`crates/coreav-isobmff-write`, and the `coreav remux` command. Scope:
use case 7, which "moves the movie header to the front, rewrites a MOV as
MP4", and the stage 3 stream-copy
primitives. Written from ISO/IEC 14496-12 (boxes, sample tables, edit
lists, sample groups), ISO/IEC 14496-14 (MP4), ISO/IEC 14496-15 (AVC and
HEVC sample entries), the AV1 and VP codec ISO media file format bindings,
the Opus in ISOBMFF encapsulation, ISO/IEC 14496-3 (AAC), the FLAC in
ISOBMFF mapping (RFC 9639 10.3), Apple's ALAC magic cookie description,
ETSI TS 102 366 Annex F (AC-3 and E-AC-3) and the QuickTime
File Format (sound sample entry versions, Macintosh language codes). No
multimedia implementation source is read, with one exception: the layout
of the Apple Lossless packet header ("Apple Lossless packets from
Matroska", below). `ffmpeg`, `ffprobe` and MediaInfo are black-box
oracles only.

## What it does

`remux_mp4(source, limits, allowlist, options, out)` rewrites a whole
MP4/MOV/M4A/3GP file as an MP4 file by stream copy: every stream MP4 can
carry, every sample of it unchanged, and the movie box (`moov`) before the
media data (`mdat`), written front to back to any `std::io::Write`. It
returns a summary: the kept streams, the dropped streams with a reason, the
dropped boxes (path, offset, size, reason) and whether the source was
already `moov`-first. `coreav remux INPUT OUTPUT` runs it with the library
defaults and prints the summary as `coreav.remux/1`
([`schema/remux-v1.md`](../schema/remux-v1.md)). It writes the file as
the other writing commands do (`coreav_cli::output`): `OUTPUT` must not be
the input, nor exist unless `--force` is given, and a temporary file next
to it is renamed over it only on success, so a failure never leaves a
partial file or touches an existing one.

It is the shared full-file path: metadata sanitization builds on
`remux_mp4`, and the Matroska/WebM remux (and any other demuxer) on the
`mux` module below.

## The shared writer (`mux`)

`Muxer` writes an MP4 file whose samples are copied from elsewhere, in two
passes, and reads nothing itself:

1. `add_track(TrackHeader)` per track (media kind and track header fields,
   timescale, language, creation times, edit list, sample entry, extra
   boxes), then `push_sample(track, Sample)` for every sample in decode
   order (size, duration, sync flag, composition offset). Only the output
   sample tables are kept, in serialized form, all tracks together charged
   against `Limits::max_table_bytes`.
2. `write_head(out)` writes `ftyp`, `moov` and the `mdat` header. Then
   `for_each_chunk(|track, samples| ...)` names, chunk by chunk, which
   track's next samples to copy, in `mdat` order; the caller lists that
   track's samples again and copies them, for example through one
   `CopyWindow` (64 KiB). `sample_size` checks the second listing against
   the first.

`visual_sample_entry` and `audio_sample_entry` build ISO sample entries
from a codec configuration and child boxes. `set_edits`,
`set_media_duration`, `set_duration`, `set_boxes`, `set_movie_timescale`
and `set_file_type` let a caller settle what depends on the samples
(edits, durations, brands) once they are known, and `pop_track` gives up on
the last track added; the timescale and the sample entry are fixed when a
track is added, and nothing changes once the head is written.

The copy pass lists each track's samples again, in decode order, one
track at a time as `for_each_chunk` asks for them. A demuxer can drive the
muxer if it can list one track's samples on its own, as MP4 sample tables
and fragments can; for a container that interleaves tracks in clusters
(Matroska), that is one listing per track over the cluster headers, or a
layout that follows the push order instead, which this module does not
offer yet.

This is the layer the clip writer and the audio extractor already used
internally, generalized from two tracks to any number
and from computed edits to edit lists as stored; their outputs are
unchanged byte for byte (checked on every fixture).

## Streams

- **Kept:** H.264 (`avc1`, `avc3`), HEVC (`hvc1`, `hev1`), AV1 (`av01`) and
  VP9 (`vp09`) video; AAC and MP3 (`mp4a` with `esds`), Opus (`Opus`
  with `dOps`), FLAC (`fLaC` with `dfLa`), Apple Lossless (`alac` with
  its `alac` cookie box), AC-3 (`ac-3` with `dac3`) and E-AC-3 (`ec-3`
  with `dec3`) audio: the allowlisted codecs whose MP4 sample entries the
  cited bindings define ("Audio sample entries" below). Unlike a clip, a whole-file copy keeps every
  sample, so in-band parameter sets of `avc3`/`hev1` are copied too and
  those entries are kept.
- **Dropped and reported** (`unsupported`, with a reason): ProRes (its
  sample entries are defined for QuickTime only), PCM (MP4 carries PCM
  only as ISO/IEC 23003-5 `ipcm`/`fpcm`, a conversion of the sample
  description this remux does not do yet), timed text, timecode and data
  tracks, a stream with more than one sample description, external data,
  invalid tables, a media timescale of 0, no samples, a gap or overlap
  between fragment decode times, an AAC rate above 65,535 Hz (the version 0
  sound entry's limit), or a missing codec configuration. Streams outside
  the allowlist are reported as `codec_not_allowed`; `--streams` leaves the
  others out as `not_selected`.
- **Refused** (`invalid_structure`), never dropped: a stream whose coded
  data its codec's specification does not allow, so that no output
  carries it. That is an HEVC stream whose first sequence parameter set has
  a conformance window that leaves no picture (ITU-T H.265 7.4.3.2.1); a
  conforming decoder needs to decode only conforming bitstreams (C.5.1).
  The clip, split, join and HLS writers refuse it alike: `conforming` in
  `clip.rs` is the one check, which their video sample entry and this
  remux's stream plan call. Dropping it would hand on a file that lost its
  picture without saying why it cannot be played.
- **Choice:** drop and report by default (the issue asks for it, and an
  upload pipeline usually wants the playable streams);
  `OnUnsupported::Fail` (`--fail-on-unsupported`) fails before writing
  instead, and a stream named in `RemuxOptions::streams` always fails when
  it cannot be copied. A file with nothing to copy fails with the first
  dropped stream's reason.
- Track IDs are renumbered from 1 in source order, as the summary maps
  them. Track references (`tref`) are not copied: they point at tracks by
  ID, usually at dropped ones (timecode, chapters, metadata); each is
  reported.

## Samples and timing

Every sample is copied with its size, decode duration, composition offset
(`ctts` version 1 when one is negative) and sync flag; output decode times
start at 0. The source's media timescales are kept, and its movie
timescale too, except for a fragmented source where a computed value would
round (below), so:

- **Edit lists are copied entry for entry as stored** (duration, media
  time, rate), including shapes outside CoreAV's presentation model (dwells,
  several media edits): a container rewrite keeps the
  presentation as the source defines it. Media times move with the first
  decode time, which is 0 except in some fragmented sources.
- The media and track header durations are the source's declared ones
  when the source has no fragments, also where they disagree with the
  sample durations or the edits, as in M4A extraction since its amendment
  (MediaInfo, for one, reports the track header's). Otherwise, and where the source
  declares 0, the media duration is the sum of the sample durations and
  the track duration the sum of the edits, or the media duration in movie
  ticks. The movie duration is the longest kept track (ISO/IEC 14496-12
  8.2.2, 8.3.2), so it no longer counts a dropped stream.
- **Fragmented sources** become one unfragmented file. Decode times must be
  contiguous within a stream (the output stores durations only); a gap
  drops the stream. Without an edit list, a first decode time above 0
  becomes an empty edit of that length, then a media edit from media time
  0 to where the frame presented last ends: the source presents every
  frame at its composition time, which with reordered frames runs past
  the decode durations' sum (a media edit of that sum left the last
  presented frame out); a last edit of duration 0, which in
  a fragmented file lasts to the end of the media, gets that length. These
  and the track durations are computed. Where one would round in the
  source's movie timescale, the movie timescale becomes the least common
  multiple of it and that stream's media timescale, when it fits 32 bits
  (the source's edits are scaled exactly); otherwise it is kept, so most
  files keep 32-bit headers. Rounded to a coarse movie timescale (often
  1,000 or 600), the computed values shifted presentation by up to half a
  tick, which the corpus found (ffmpeg trimmed a last frame to a rounded
  edit; MediaInfo reported durations a millisecond off).

## Sample entries and per-sample boxes

- **Video:** rebuilt as an ISO visual sample entry with the source's coded
  width and height, compressor name and depth; QuickTime's version, revision,
  vendor and quality fields (reserved in ISO/IEC 14496-12) are written as
  zero. Every child box is copied as stored, in order: the codec
  configuration and whatever else describes the coded video (`pasp`,
  `colr`, `clap`, `btrt`, HDR and Dolby Vision boxes, field order). The
  sample entry is found whatever the order of the media box's children, as
  probe accepts it.
- **Audio:** rebuilt as an ISO version 0 sound entry (QuickTime version 1
  and 2 layouts have no place in MP4) with the codec configuration first,
  taken from QuickTime's `wave` wrapper when it is there, then the other
  children as stored, except QuickTime's channel layout `chan`, which MP4
  does not define (ISO/IEC 14496-12 has `chnl`): it is reported. MediaInfo
  reads `chan` inside an MP4 file differently from inside a MOV, and
  `ffmpeg` leaves it out of the MP4 files it writes (both observed as black
  boxes).
- **Per-sample information** that names samples by number, never by file
  offset (`sgpd`, `sbgp`, `sdtp`, `subs`, `cslg`, `stps`, `padb`, `stdp`,
  `stsh`), stays valid when every sample keeps its place, so it is copied
  as stored from an unfragmented source. A fragmented source carries more
  of it in its fragments, which the output does not, so there it is
  dropped and reported. Boxes with file offsets (`saiz`/`saio`) are dropped
  and reported.

## Audio sample entries

Every command that copies audio into MP4 (`coreav remux` from MP4/MOV and
from Matroska/WebM, `clip`, `split`, `join`, and `coreav ffmpeg` with
`-c copy` or `-f concat ... -c copy`) writes the same entries, in
`src/audio.rs`. None decodes the audio: copying AC-3 and E-AC-3 links no
AC-3 decoder, so these entries are in the default build.

| Codec | Entry | Configuration | From MP4/MOV | From Matroska |
|---|---|---|---|---|
| FLAC | `fLaC` | `dfLa`: version 0, then the metadata blocks | as stored | `CodecPrivate` (`fLaC`, then the blocks, RFC 9639 10.2) |
| Apple Lossless | `alac` | `alac`: version 0, then the magic cookie | as stored | `CodecPrivate`, the cookie (24 or 48 bytes, with or without a 12-byte atom header) |
| AC-3 | `ac-3` | `dac3` (F.4) | as stored | the first syncframe's `bsi()` |
| E-AC-3 | `ec-3` | `dec3` (F.6) | as stored | the first sample's substreams |

- **Sample entry fields.** `samplesize` is FLAC's bits per sample
  (`STREAMINFO`) or the cookie's `bitDepth`, else 16. The 16.16
  `samplerate` cannot hold more than 65,535 Hz; FLAC in ISOBMFF 3.3.1 has
  it hold the rate divided by the smallest power of two that fits (96 kHz
  is written 48,000), and Apple Lossless gets the same rule. The
  configuration keeps the real rate, and the media timescale is the real
  rate, so sample times are exact. Annex F has readers take AC-3's and
  E-AC-3's channels from `dac3`/`dec3`, whatever the entry says.
- **Samples.** One FLAC frame (FLAC in ISOBMFF 3.3.3), one Apple Lossless
  packet, one AC-3 syncframe, or one E-AC-3 sample: the syncframes that
  together deliver six blocks of every substream (F.2), 1,536 samples.
  From Matroska, a FLAC frame lasts its header's block size (RFC 9639
  9.1.1), an Apple Lossless packet the samples its header states, else the
  cookie's `frameLength` (below), an AC-3 or
  E-AC-3 frame 1,536 samples. The stream is dropped with a reason when its
  first frame is not one AC-3 syncframe of the size its header states, is
  not E-AC-3 syncframes of six audio blocks in all,
  holds independent substreams out of order (E.1.3.1.2), or is larger than
  288 KiB.
- **Apple Lossless packets from Matroska.** Apple's cookie description
  makes `frameLength` a packet's sample count only when the packet's
  header states no other, and a shorter packet states its own; Matroska's
  millisecond times cannot give the count. The header's layout is taken,
  under a maintainer-approved clean-room exception (2026-10-31) for the
  ALAC packet header layout only, from Apple's ALAC reference source,
  `codec/ALACDecoder.cpp` and `codec/ALACBitUtilities.h` of the
  macosforge/alac release (Apache-2.0): a 3-bit element tag (0 for one
  channel, 1 for a channel pair), a 4-bit instance tag, 12 bits of zeros,
  a flag for a stated count, a 2-bit shift other than 3, an escape bit,
  then, when flagged, the count in 32 bits. Nothing else of that source
  is used, and CoreAV's reader (`alac_samples` in `src/audio.rs`) is
  written from this description. Only a packet's first element header is
  read, so a packet must hold one element: mono is one single channel
  element and stereo one channel pair, and a stream of more than two
  channels is dropped with "Apple Lossless from Matroska with more than
  two channels: a packet's later channel elements are not read". A
  stated count is the packet's duration whatever the source's times say.
  A packet that does not start with the element its channels need, or
  states 0 samples or more than `frameLength`, drops the stream with "an
  Apple Lossless packet without a valid element header" rather than
  being given a guessed length.
- **`dac3` and `dec3` from the bitstream.** `fscod`, `bsid`, `bsmod`,
  `acmod` and `lfeon` come from the first frame's `bsi()` (4.4.2,
  E.1.3); `bit_rate_code` from `frmsizecod`; E-AC-3's `data_rate` is the
  first sample's size times 8 over its 1,536 samples, in kbit/s, and each
  independent substream lists its dependent substreams' channel locations
  (`chan_loc`, from `chanmap`).
- **Parts of a stream.** A clip, a split segment and a join hold part of
  a stream, or parts of several: `STREAMINFO`'s frame sizes, total samples
  and MD5 signature, and the cookie's `maxFrameBytes` and `avgBitRate`,
  describe the whole stream, so they are written as unknown (0, RFC 9639
  8.2 and the cookie description). A join compares inputs' configurations
  without these fields.
- **Sanitize** keeps `dfLa`'s `STREAMINFO` and drops its other blocks
  (Vorbis comments, pictures and application data carry text and images);
  no other configuration carries metadata.
- **Not taken:** HLS packages AAC only, and `extract-audio` copies AAC
  only; a `coreav ffmpeg` line that encodes video (`-c:v libx264`) still
  refuses to copy these codecs beside it.

## Metadata and other boxes

- Copied as stored: the movie's and each kept track's `udta` and `meta`
  boxes, so titles, iTunes tags, chapters lists and also location and
  device tags survive a normalization. Stripping them is the sanitizer's job
  (metadata sanitization builds on this path). Copied boxes count against
  `max_retained_bytes` (16 MiB by default) in total, not against the 1 MiB
  `max_box_bytes` that bounds what probe reads: cover art often exceeds a
  megabyte (found on the corpus). What every kept stream needs (its sample
  entry children, 48 bytes per edit list entry) is charged first and fails
  the remux when it does not fit; the optional boxes (track and movie
  `udta` and `meta`, per-sample boxes) then get what is left, and one that
  does not fit is left out and reported as `too_large`.
- Copied from the track header: flags (enabled, in movie, in preview),
  layer, alternate group, volume, matrix (rotation) and presentation size;
  the language, as ISO 639-2/T, with QuickTime Macintosh language codes
  mapped through the QuickTime File Format table (code 0, English, becomes
  `eng`); creation times.
- Rebuilt: `ftyp` (major brand `isom`, minor version 512, compatible
  `isom`, `iso2`, `mp41`, plus `avc1`, `av01` or `Opus` when such a stream is
  kept), `mvhd`, `tkhd`, `mdhd`, `hdlr` (generic handler names), media
  headers, `dinf` and the sample tables.
- Everything else is dropped and reported with its path: top-level boxes
  other than `ftyp`, `moov`, `mdat`, fragments, their indexes and padding
  (for example `uuid` XMP, `meta`, `pnot`); `moov` children such as `iods`;
  `trak` children such as `tref`, `tapt` and `load`; and the per-sample
  boxes above where they do not apply; inside a QuickTime sound entry, the
  contents of `wave` other than the configuration and format boxes. The
  summary lists the boxes outside the tracks, then each kept stream's in
  output order, then those too large, each group in file order: at most 256
  (64 per track and 64 outside the tracks), and counts the rest.

## Output layout and memory

`ftyp`, `moov`, one `mdat` (64-bit size above 4 GiB); chunks of about one
second per stream or 1 MiB, whichever is less, the streams alternating by
decode time, so a reader of a partial file gets every stream from the
start; `stco`, or `co64` above 4 GiB. The byte limit matters for
high-bit-rate video: one second of a 4K file is several megabytes,
and with one-second chunks the first audio sample lay beyond the 5 MB that
`ffprobe` reads by default to identify streams, so it reported no AAC
profile (found on the corpus). For the same reason, of chunks that start
at the same time the smaller is stored first: a single key frame of
high-bit-rate video (7.6 MB in a 446 Mb/s HEVC corpus file) would
otherwise push the first audio chunk out of `ffprobe`'s window. The clip
writer and the audio extractor keep their one-second chunks and track
order. The source is listed twice per kept stream: once to build the tables,
once to copy, through one shared 64 KiB read window. Memory beyond the
parser's own is the output sample tables plus the copied metadata (bounded
by `max_retained_bytes`) plus a fixed amount per kept stream, never the size
of the media. The copy pass of each stream gets exactly the sample work its
first listing took, and both passes together stay within
`max_sample_work`, checked before anything is written.

## Replaced streams

`remux_mp4_replacing(source, limits, allowlist, options, replacements,
out)` is `remux_mp4` with the samples of one video stream, one audio
stream, or one of each taken from the caller instead of the source
(`Replacements { video, audio }`): the operation `coreav transcode` writes
its output with (`docs/design/transcode.md`, "The MP4 writer" and "Opus
audio"). A stream named by both, or by one twice, fails with `Unsupported`.
This section is the video replacement; "One audio stream replaced" below
is the audio one. `remux_matroska_
replacing` is the same for Matroska and WebM sources
([`mkv-remux.md`](mkv-remux.md)). Both run the remux paths above: the
replaced stream is one more kind of kept stream, so every other stream is
written exactly as the plain remux writes it, and the plain remux is
unchanged byte for byte (checked on every MP4, MOV, M4A, Matroska and WebM
fixture against the previous build).

- **The replacement** (`Replacement`): the source `stream_index`, one
  complete sample entry box (`mux::avc1_sample_entry` builds `avc1` from
  the coded size, an `avcC` payload, an optional `pasp` and an optional
  `colr` of type `nclx`; any `avc1`, `avc3`, `hvc1`, `hev1`, `av01` or
  `vp09` box is accepted), the track header's presentation width and
  height (16.16), the samples in decode order (size, sync flag, decode
  time, presentation time), the last sample's duration, and a `Source`
  holding the samples' bytes back to back from offset 0 (the transcoder's
  spool file). The samples are borrowed: the writer keeps only the output
  tables, and copies the bytes in its chunk order through a second 64 KiB
  window, so the replacement is read once, in order.
- **Times.** The samples' times are the replaced stream's own timestamps:
  media ticks of the source track and its composition times (decode time
  plus composition offset, as `Packet::cts` gives them). Decode times may
  start anywhere, negative included; output decode times start at 0 as
  for every stream, and each sample lasts up to the next one's decode
  time.
- **Edit list.** The source track's edit list must be absent or of the
  simple model (at most one empty edit, then one media edit at rate 1);
  others fail with `Unsupported`, as the transcoder refuses them. A frame
  with presentation time `P` is presented where the plain remux presents a
  source sample with composition time `P`: the media edit's media time
  moves by the replacement's first decode time instead of the source's,
  and may move later by any amount the empty edit grows by. It moves to
  keep the gap the source's own edit leaves before its first frame: with
  the source edit's media time `M` and its first frame at `s0` (the
  smallest composition time in the sample tables, at least `M`, listed
  under `max_sample_work` like any listing, whose failure fails the remux;
  `M` when the tables are invalid or the data is in another file), the
  replacement's media edit starts at
  `M + max(0, p_min − s0)`, never before its first decode time, `p_min`
  being its first presentation time. The `ffmpeg` binary, as a black box,
  shows the first frame of a media edit that starts before it at the
  edit's start (80 ms early on `fragmented.mp4` with a two-frame encoder
  delay, when the edit started at the old media start): keeping the
  source's gap makes it show the replacement where it shows the source,
  and the identity replacement gives the source's edit list. When the
  transcoder leaves leading frames out (the source does not present
  them), the empty edit grows by the difference. Without an edit list,
  media time is movie time and there is no gap: a first decode time of 0
  needs no edit list, any other gets an empty edit up to the first frame
  (when it is above 0) and a media edit from it. A media edit that lasts
  to the end of the media, or one computed here, lasts to the end of the
  frame presented last (its presentation time plus its own duration), so
  every frame is presented. An empty edit computed in media ticks that
  would round in the movie timescale makes the movie timescale a common
  multiple of it and the media timescale when one fits 32 bits (the other
  tracks' edits and declared durations are scaled exactly), as for
  fragmented sources; not when a fragmented source's other track has a
  computed value that rounds in the movie timescale, which the plain
  remux rounds there and a switch would move (the audio must equal the
  plain remux's): the replaced stream's empty edit rounds then, so its
  video can be presented up to half a movie tick (0.5 ms at the common
  1,000) from the time the contract gives.
- **Plain path.** The replaced stream's copy pass is a function of its
  own, so the plain remux's loop is the one it was.
- **Matrix.** The source track's matrix places the picture for the
  source's presentation size. When the replacement's size differs and the
  matrix is a quarter turn or a flip (`a`, `b`, `c`, `d` each 0 or ±1, no
  perspective) whose translation is the one that keeps the old size in
  the positive quadrant (what a rotation tag writes), the translation is
  computed again for the new size; another translated matrix fails with
  `Unsupported` when the size changes, since its meaning for another size
  is unknown. A matrix without a translation is kept as stored.
- **Kept from the source track:** its place in the track order, language,
  matrix (rotation, its translation as above), flags, layer, alternate
  group, creation times, media timescale, and its `udta` and `meta` boxes
  (or what sanitization keeps).
  **Taken from the replacement:** the sample entry, the presentation size,
  the samples; the media duration is the samples' sum and the track
  duration the sum of the edits. The source track's per-sample boxes
  (`sdtp`, `sgpd`, `sbgp`, `subs`, ...) describe the old samples and are
  listed as dropped (`not_carried`). None of the source track's samples or
  sample entry is read, so it may be a stream MP4 cannot carry (ProRes,
  VP8 in WebM).
- **Refusals,** before anything is written: a sample entry that is not one
  visual sample entry box (data reference index 1) of a known type, a
  translated matrix that cannot follow a new size, no samples, decode times that do not strictly
  increase or step by more than 32 bits, a presentation time before its
  decode time, a last duration of 0, samples that run past the end of the
  data, a sample above `max_packet_bytes`, a stream index that is not a
  video stream the allowlist keeps or that `options.streams` leaves out,
  and a replacement of which nothing is presented. The output tables count
  against `max_table_bytes` with the other streams'.
- **Summary:** `RemuxTrack::replaced` marks the stream, whose `codec` and
  `samples` are the replacement's.

Rejected:

- **Times on the output timeline** (decode times from 0, an edit list
  from the caller): the caller would have to redo the remux's edit list
  rules (fragment flattening, priming, movie timescale changes) to keep
  the video in place against the copied audio. With the source's own
  timeline the writer owns those rules, and the identity replacement (the
  source's own samples) must reproduce the plain remux, which the tests
  check.
- **Presentation-ordered input with computed decode times** (as the
  Matroska remux does): the encoder's decode order is known only to the
  encoder's output, and the transcoder computes it exactly.
- **Holding the samples in the writer** (a `Vec` of owned samples, or the
  bytes in memory): the transcoder has the sample list already, and the
  encoded video can be gigabytes; the spool file is read in order.
- **Copying the source's per-sample boxes:** `sdtp` and sample groups name
  samples by number, which no longer exist.
- **Refusing a first frame after the edit's media time:** the transcoder
  drops frames the source's edit list does not present, so the first
  encoded frame can be later than the edit's start.

### One audio stream replaced

The audio replacement (`AudioReplacement`) is an encoded stream on a
timeline of its own, Opus in ISO BMFF as opus-codec.org's "Encapsulation
of Opus in ISO Base Media File Format" (version 0.6.8) lays it out, or AAC
in an `mp4a` sample entry (ISO/IEC 14496-14), laid out the same way:

- **The replacement:** the source `stream_index` (an audio stream the
  allowlist keeps and `options.streams` selects), one complete `Opus` or
  `mp4a` sample entry box (an ISO audio sample entry with data reference
  index 1; `coreav-transcode` builds it with `mux::audio_sample_entry` and
  a `dOps` box, or an `esds` box from `mux::esds`), the media timescale
  (48,000 for Opus, the sample rate for AAC), the samples in order
  (size and duration, every one a sync sample, so no `stss`), `start`
  (media ticks of silence before the presentation), `priming` (the media
  ticks at the start that are not presented: Opus's pre-skip, AAC's
  encoder delay),
  `presented` (the media ticks presented from there), `roll` (the
  samples a decoder needs before any one it starts at) and the samples'
  bytes back to back.
- **Edit list:** an empty edit of `start` when it is above 0, then a
  media edit from `priming` lasting `presented` (the spec's 4.4): the
  encoder's delay and the last packet's padding are not presented, and
  the first presented sample is at movie time `start`. The edits are in
  the movie timescale; where they would round there, the movie timescale
  becomes a common multiple of it and the replacement's media timescale
  when one fits 32 bits (as for
  the video replacement, and not when another stream's computed value would
  move), else each boundary rounds to the nearest movie tick.
- **Roll group** (the spec's 4.3.6.2, ISO/IEC 14496-12 10.1): one `sgpd`
  of type `roll` (version 1, one `AudioRollRecoveryEntry` of
  `roll_distance` `-roll`) and an `sbgp` that leaves the first `roll`
  samples out of the group (fewer than `roll` samples come before them)
  and puts every later one in it. The `ftyp` already lists `iso2`, a
  brand that requires roll groups (4.1).
- **Kept from the source track:** its place in the track order, language
  and flags, and its volume and `udta`/`meta` boxes (or what the metadata
  option keeps of them). Taken from the
  replacement: the sample entry, timescale, samples and edits. None of the
  source track's samples or sample entry is read, so it may be a stream MP4
  does not carry as it is (FLAC, which the caller adds to the allowlist).
- **Refusals,** before anything is written: a sample entry that is not one
  `Opus` or `mp4a` audio sample entry box with data reference 1, a timescale of 0,
  no samples, a duration of 0, nothing presented or more presented than
  the samples hold after `priming`, a roll distance above 32,767, samples
  that run past the data or above `max_packet_bytes`, and a stream index
  that is not an audio stream kept and selected.
- **Summary:** the stream is `replaced`, its codec `Opus` or `AAC`, as its
  sample entry names it.

Verification, `tests/replace.rs`: an Opus replacement takes the audio
stream's place in an MP4 source, in a Matroska source after an empty edit,
and beside a video replacement; read back by `coreav-isobmff`, the track
is Opus at 48 kHz with the edit list exact in the movie timescale, every
sample's bytes and duration as given and a sync sample (no `stss` of its
own), and the roll group's boxes as above; every refusal comes before a
byte is written. An AAC replacement takes the audio stream's place in an
MP4 and a Matroska source: `mp4a` at its rate, the delay left out by the
edit, every frame as given, and a roll distance of −1.
`coreav transcode --audio opus` and `--audio aac` check the output with
the pinned FFmpeg (`transcode.md`, "Opus audio" and "AAC audio").

Verification: `tests/replace.rs` (synthetic movies and Matroska files:
the identity replacement, decode times shifted back by two frames and by
other amounts, leading frames left out, no edit list with and without an
exact empty edit, a source edit with a gap before its first frame,
per-sample boxes, track order, size, language and translated quarter and
half turns, a source codec MP4 cannot carry, every refusal and the
limits); with the fixtures in CI
(`fixtures` workflow), ten H.264 fixtures (`basic.mp4`, `basic.mov`,
`edit_list.mp4`, `clip_no_stss_bframes.mp4`, `rotate_ccw90.mp4`,
`vfr.mp4`, `negative_cts.mp4`, `fragmented.mp4`, `clip_video_late.mp4`,
`multitrack.mp4`) and a Matroska H.264 and AAC file made by the pinned
`ffmpeg`, each with its own samples and with them decoded two frames
earlier, must give the plain remux's video frames (`ffprobe -show_frames`,
`best_effort_timestamp_time`) and audio packets (`-show_packets
-show_data_hash MD5`), 100%; so must two synthetic movies (a source edit
with a gap before its first frame, and frames 100 ticks after their
decode times without an edit list, whose shifted replacement switches the
movie timescale), compared by their packets' presentation times;
`rotate_ccw90.mp4` keeps its matrix at its size and its rotation at half
the size; `edit_multi_bframes.mp4` is refused. The
fuzz target `replace`: arbitrary sample lists over three fixed sources
(MP4 with and without an edit list, Matroska) either fail or give an output
the parser accepts, with the replacement's bytes, sync flags and
composition offsets, presented at the plain remux's times for the same
source times, and the other streams as the plain remux writes them. Its
speed is part of `coreav transcode`'s bars (B5, B8), whose writer runs it
once per output.

## Alternatives considered

- **Move `moov` and patch its chunk offsets in place** (the classic
  faststart): cheaper when the source is already MP4, but it cannot drop a
  stream, rewrite QuickTime sample entries, flatten fragments or report what
  it carried, and it holds the whole movie box in memory; the table writer
  already exists and bounds memory by the output tables.
- **Keep source track IDs:** would keep references valid, but most
  references point at dropped tracks; renumbering gives a dense, predictable
  output, and the summary maps the IDs.
- **Rebuild sample entries from parsed fields only** (the clip writer's
  approach): loses HDR, Dolby Vision and other configuration boxes of the
  video, which a normalization should keep. The clip writer keeps its
  stricter rule.
- **Copy `chan`:** a lossless copy, but MediaInfo then reports the channel
  count only as "original", and MP4 defines `chnl` for layouts.
- **Fail on any stream MP4 cannot carry:** safer for archiving, worse for
  upload normalization; available as an option.

## Verification

- Unit tests of the muxer and the integration tests in
  `crates/coreav-isobmff-write/tests/remux.rs`: `moov` first, samples,
  times, edit lists (including unsupported shapes), QuickTime sound entries
  and language codes, fragment flattening, metadata copy and the dropped-box
  report, refusals, limits, write and read errors, every truncation, and
  deterministic mutations.
- `fuzz/fuzz_targets/remux.rs`: arbitrary input either fails or gives an
  output the parser accepts, `moov` first, with every sample of every kept
  stream (bytes, durations, composition offsets, sync flags).
- Fixtures, in CI: every MP4, MOV and M4A fixture is remuxed and checked by
  `scripts/remux_check.py`: top-level boxes exactly `ftyp`, `moov`, `mdat`;
  the track header fields, edit lists (unfragmented sources) and sample
  entry children read from both files' boxes;
  `ffprobe` streams (codec, profile, dimensions, colour, sample rate,
  channel layout, time base, start, duration, frame count, rotation,
  disposition, tags) and format tags equal the source's; `ffmpeg -c copy -f
  framemd5` of every kept stream equals the source's, header and every
  packet; MediaInfo tracks equal the source's. Differences explained by the
  specifications are in
  [`known-divergences-remux.md`](../known-divergences-remux.md). Threshold:
  100% of the fixtures.
- Audio sample entries, in CI: `scripts/audio_copy_check.py` makes FLAC,
  Apple Lossless, AC-3 and E-AC-3 sources in MP4, MOV, M4A and Matroska
  with the pinned FFmpeg 9.0.2 (stereo, 5.1, 44.1 kHz and 96 kHz
  variants, and files with H.264 video), runs remux, `coreav ffmpeg -c
  copy`, clip, split, join and `coreav ffmpeg -f concat -c copy` on them,
  and checks with FFmpeg 9.0.2's `ffprobe` and `ffmpeg` that each output
  has the source's codec, channels and sample rate and the source's
  packets (a run of them for a clip, all of them across a split's
  segments, twice for a join of a file with itself), and that a copy of a
  whole file decodes to the source's audio, by MD5. Threshold: 100% of the
  cases. `tests/remux.rs` and `tests/mkv_remux.rs` check the four
  entries on synthetic files, `dac3` and `dec3` byte for byte.

### Corpus v1 bars (fixed 2026-10-19, before the corpus run)

- **Inputs:** every corpus v1 `real` file with an ISO base media extension
  (`mp4`, `mov`, `m4v`, `3gp`, `m4a`): 1,043 files, 301 of them held out.
  One remux per file with the library defaults (every stream that can be
  copied, default limits and allowlist), from a local file.
- **Checks:** the library round trip of the fixture tests, then
  `scripts/remux_check.py` with the known divergences, on every output.
- **Refusals:** files the remux refuses with an unsupported-feature error
  (nothing MP4 can carry) are counted and listed by reason, apart from the
  denominators. Every other failure (a parse error on a file `ffprobe`
  reads, a limit, a crash, a timeout) counts as disagreement, and every
  failure is investigated and recorded.
- **Pass:**
  - at least **99% of remuxed files** agree with every check, directly or
    through a known divergence;
  - at least **99.9% of packets** agree: `framemd5` packet lines of the kept
    streams, a missing or extra packet counting as disagreeing;
  - **100%** of outputs have `ftyp`, `moov`, `mdat` in that order and pass
    the library round trip;
  - no panic, crash or timeout (one hour per file).
  These are tighter than the roadmap's stage 3 criteria (98% of files,
  99% of packets): a copy that keeps every sample should reach them.
- **Reported, not gated:** streams dropped, by reason; dropped boxes, by
  path.

### Performance bars (fixed 2026-10-19, before the run)

On the 301 held-out files, on one machine, each file's time the median of
5 rounds with the engines' order rotated:

- **Speed.** Baseline: libavformat in-process, as a black-box baseline,
  remuxing the same streams to MP4 with `+faststart`, 64 KiB custom I/O on
  both sides. Bar: the median of the per-file ratios ours / baseline is at
  most **1.10**, and the median of every class (unfragmented, fragmented,
  longer than one hour) at most **1.5**. The `ffmpeg -c copy -movflags
  +faststart` command line is reported for reference. Re-runs use the
  FFmpeg speed bar: at most 1.00 overall and per class.
- **Re-run on current code (fixed before the run).** The same 301
  held-out files (`docs/spikes/remux-bench/evidence/inputs.json`), the same
  method (`perf.py`: one warm-up, five rounds with the engines' order
  rotated, outputs on a tmpfs) on current `main`. Round 2 of this
  measurement: the same set as `perf-01`, which passed on its first round
  and was never used for tuning. Bar: the median of the
  per-file ratios ours / `lav_avio` is at most **1.00** overall and in each
  class (unfragmented, fragmented, longer than one hour); "faster" is said
  only where the upper bound of the bootstrap 95% interval is below 1.00.
  The `ffmpeg` command line is reported again.
  **Result: met** (`docs/spikes/remux-bench/`, `perf-02` on the crates of `1114aa6`): 0.328
  (95% CI 0.305 to 0.350) over 268 files; unfragmented 0.327, fragmented
  0.358, longer than one hour 0.314. Every upper bound is below 1.00, so
  the remux is faster than libavformat in every class. Against the
  `ffmpeg` command line it is 0.095.
- **Memory.** Peak heap growth beyond the parser's own use (probe and the
  listings of the kept streams) at most the output sample tables plus the
  copied metadata plus 64 KiB plus 16 KiB per kept stream, on every
  held-out file, with the counting allocator of the earlier benchmarks.
- Misses block claims about speed or memory, not the merge; the cause is
  recorded before any bar changes.
