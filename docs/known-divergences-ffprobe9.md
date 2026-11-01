# Known divergences: `coreav ffprobe` versus `ffprobe` 9.0.2 on the fixtures

The fixture entries below also describe historical output that has since
been corrected. Matching values need no exception. The corpus notes at
the end of this page are not fixture exceptions and do not turn a failed
key comparison into agreement.

## AV1 codec-string colour and chroma

The [AV1 ISOBMFF binding section 5](https://aomediacodec.github.io/av1-isobmff/#codecsparam)
requires the optional fields of a codec string to describe the actual
monochrome, chroma and colour values; omitting them implies limited-range
BT.709 with non-monochrome 4:2:0 samples. It also makes an `nclx` colour
box override sequence-header colour fields in that string.

On small generated inputs, the pinned oracle omits the suffix of a
full-range monochrome sequence and of a position-only chroma change,
prints a full-range RGB sequence's range as 0, and ignores an `nclx`
override of a BT.2020/PQ sequence. CoreAV follows the specified values
and omits only the complete default suffix. The byte-validated generated
fixture exception at the end of this page covers one exact `nclx`
disagreement. Other differing strings still fail B3a on the corpus.

The command-coverage set (`tools/compat-corpus/run.py commands`, bar B3a on
the fixtures of [`design/compat-cli.md`](design/compat-cli.md)) compares
every `native` `ffprobe` command byte for byte. Moving the oracle from
FFmpeg 7.0.2 to 9.0.2 changed what `ffprobe` prints on most fixtures, and
the fixtures themselves, which the pinned `ffmpeg` writes. This page gives
the cause of each resulting difference. Its machine-readable twin,
[`tools/compat-corpus/ffmpeg9.json`](../tools/compat-corpus/ffmpeg9.json),
names under each entry the keys and, for each key, the fixtures it differs
on; `tools/compat-corpus/tests/test_ffmpeg9.py` keeps the two in step.

A difference is set aside only when the two outputs are byte for byte the
same once the keys the twin names for that fixture, and nothing else, are
taken out of both, in the command's own output format. A writer that hides
keys (`csv`, `nokey`, `noprint_wrappers`) is read through the same
command with them shown, and each tool's two outputs must agree with each
other. Each entry also accepts only the difference it names (its
`accepts` in the twin, the `Accepts` line here): `run.py` lists the
whole file with both tools (`-show_format -show_streams -show_programs
-show_packets` in JSON), and every value of a named key that differs
there must be that difference, on the stream or packet the entry names,
and every value that differs in the command must be one of those. Every
other difference, on any key or fixture, fails the run.
Keys are named as `run.py keys` names them (`streams.refs`); a side data
entry by its type (`packets.side_data_list[Skip Samples]`, with `[]` for
an entry the command prints empty because it selects none of its keys);
a stream group as `stream_groups`.

The first ten entries are changes in what FFmpeg 9.0.2 prints: CoreAV
prints what 7.0.2 printed, and coreav-dev/coreav#24 follows 9.0.2. The
eleventh is an FFmpeg error on a fixture 9.0.2 writes, which ISO/IEC
14496-12 decides for CoreAV. The next four are CoreAV differences that the
fixtures 9.0.2 writes expose: both FFmpeg versions print the same value
there, and CoreAV another. The next entry validates an AV1 fixture's
stored `nclx` bytes and fails when its exact oracle disagreement changes.
The last three are corpus records for Matroska and WebM AC-3 and AV1
tracks; they name no fixture keys and set nothing aside on the fixtures.

## KD-FF9-NEW-KEYS

- Keys: `disposition.multilayer` (every stream), `mime_codec_string`,
  `view_ids_available` and `view_pos_available` (HEVC), and the AC-3 and
  E-AC-3 downmix keys `dmix_mode`, `ltrt_cmixlev`, `ltrt_surmixlev`,
  `loro_cmixlev` and `loro_surmixlev`, of `streams` and
  `programs.streams`.
- Cause: FFmpeg 9.0.2 prints these keys; 7.0.2 did not, and CoreAV does not
  yet.
- Accepts: the key printed by `ffprobe` alone.

## KD-FF9-DROPPED-KEYS

- Keys: `refs`, `closed_captions` and `film_grain` of a video stream (in
  `streams` and `programs.streams`), and `streams.tags.vendor_id`.
- Cause: FFmpeg 9.0.2 no longer prints a video stream's `refs`,
  `closed_captions` and `film_grain`, nor a sample entry's vendor
  `[0][0][0][0]` as a tag. CoreAV prints them, as 7.0.2 did.
- Accepts: the key printed by CoreAV alone.

## KD-FF9-SKIP-SAMPLES

- Keys: `packets.side_data_list[Skip Samples]`, `packets.side_data_list[]`,
  `packets.duration` and `packets.duration_time`.
- Cause: FFmpeg 9.0.2 lists as Skip Samples side data both the samples a
  decoder skips at the start of an audio stream (the encoder delay an edit
  list or a Matroska `CodecDelay` states) and those it discards at its
  end. FFmpeg 7.0.2 listed one of the two (the start of an MP4 stream, the
  end of a Matroska one). In MP4 CoreAV lists what 7.0.2 did; in Matroska
  and WebM it lists both, as 9.0.2 does, so the entry lists no Matroska or
  WebM fixture for the side data keys. A packet that
  carries the side data gets another duration than 7.0.2 gave it (the
  whole MP3 frame at the end of an MP4 edit list, for example, where 7.0.2
  gave the part the list keeps); CoreAV gives 7.0.2's.
- Accepts: side data entries CoreAV lists only some of, each of them one
  `ffprobe` lists too; another duration only on an audio packet that
  carries Skip Samples side data in `ffprobe`'s listing.

## KD-FF9-PACKET-DURATION

- Keys: `packets.duration` and `packets.duration_time`.
- Cause: FFmpeg 9.0.2 gives these packets other durations than 7.0.2, and
  CoreAV gives 7.0.2's on every packet of these files (compared packet by
  packet with 7.0.2): a sample the decode times give a duration (the next
  decode time less its own) now gets that duration, where 7.0.2 gave it one
  frame or none (the samples of a track with a `ctts` box in `vfr.mp4`,
  the samples next to the edits of `edit_multi_*`, the first audio sample
  and the timed text of a movie with fragments); the last AAC frame an
  edit or the stream's end cuts gets the part kept, where 7.0.2 gave the
  whole frame (1,024 samples); the laced AC-3 blocks of
  `mkv_lace_ac3_44k.mkv` get 35 ms, where 7.0.2 rounded to 34 ms.
- Accepts: only the packets listed in the twin, each by its position in
  the whole file's listing and with the two values found there.

## KD-FF9-TS-STREAM-ID

- Keys: `packets.side_data_list[MPEGTS Stream ID]` and
  `packets.side_data_list[]`.
- Cause: FFmpeg 9.0.2 lists the PES `stream_id` as side data on every
  packet of an MPEG-TS stream. CoreAV, as 7.0.2 did, lists it only on the
  first packet each PES packet gives (and not on the one left at the end
  of the file), so not on the further audio frames a PES packet carries.
- Accepts: the side data on packets where `ffprobe` alone lists it; on
  every packet where both list it, the same side data.

## KD-FF9-AAC-PADDING

- Key: `streams.initial_padding`.
- Cause: FFmpeg 9.0.2 gives an audio stream's encoder delay as
  `initial_padding` (1,024 samples for AAC, 256 for AC-3 and E-AC-3,
  1,105 for MP3 and 50 for AMR-NB in these fixtures), where 7.0.2 and
  CoreAV print 0.
- Accepts: on an audio stream, 0 from CoreAV and more from `ffprobe`.

## KD-FF9-ANNEXB-EXTRADATA

- Key: `extradata_size` of `streams` and `programs.streams`.
- Cause: for an H.264 or HEVC stream in Annex B form (the MPEG-TS
  fixtures, and the two Matroska fixtures that store H.264 so), the
  extradata FFmpeg 9.0.2 gathers from the stream is longer than 7.0.2's,
  whose size CoreAV prints: two bytes for H.264, three for HEVC.
- Accepts: those two bytes more from `ffprobe` on an H.264 stream, three
  on an HEVC stream.

## KD-FF9-TIMECODE-GROUP

- Keys: `stream_groups` and `format.nb_stream_groups` (`timecode.mov`).
- Cause: FFmpeg 9.0.2 makes a QuickTime timecode track and the video it
  times a stream group, and prints it before the streams. CoreAV, as 7.0.2,
  prints no stream group.
- Accepts: the group printed by `ffprobe` alone (CoreAV's JSON writes an
  empty list), and one group against none.

## KD-FF9-MERGED-TAGS

- Key: `format.tags.creation_time` (`remux_metadata.mov`).
- Cause: given a creation time in two places of the file, FFmpeg 9.0.2
  prints both, joined by `;`. FFmpeg 7.0.2 and CoreAV print one.
- Accepts: `ffprobe`'s joined values, one of them CoreAV's.

## KD-FF9-FRAGMENT-BIT-RATE

- Key: `streams.bit_rate` (`fragmented_late.mp4`).
- Cause: FFmpeg 9.0.2 gives the stream twice the bit rate 7.0.2 and CoreAV
  give.
- Accepts: twice CoreAV's bit rate from `ffprobe`.

## KD-FF9-FIXTURE-SIDX-DURATION

- Keys: `format.duration`, `format.bit_rate`, `streams.duration` and
  `streams.duration_ts` (`fragmented_sidx.mp4`); and where
  `tools/compat-corpus/copy_check.py` places the next file of a
  `-f concat` join after it.
- Cause: FFmpeg ends the file 80 ms after its boxes do. No track has an
  edit list, so each track's media timeline is its presentation timeline
  (ISO/IEC 14496-12 8.6.6). A sample presents from its composition time,
  its decode time plus its composition offset (8.6.1.3), the decode times
  counting from each fragment's `tfdt` (8.8.12) by the `trun` durations
  (8.8.8). The video's 500 samples of 512 ticks at 12,800 decode from 0
  and present from 1,024 (its first composition offset) to 257,024, that
  is 0.08 s to 20.08 s; the audio's 863 samples present from 0 to 885,528
  at 44,100, also 20.08 s. The segment indexes say the same (8.16.3.3): the
  video's `earliest_presentation_time` 1,024 and its subsegments 256,000
  ticks, the audio's 0 and 885,528. So the file lasts 20.08 s and the
  video's media 20.00 s (256,000 ticks, the sum of its sample durations),
  which CoreAV prints. Both FFmpeg versions print a file of 20.16 s and a
  video of 257,024 ticks from 0.08 s, which ends at 20.16 s, past every
  sample; the bit rate of the file follows the duration. The same streams
  without the index (`fragmented_many.mp4`) last 20.08 s in FFmpeg too.
  Its concat demuxer places a second file at 20.16 s, CoreAV's join at
  20.08 s, where the first one ends. The copy check takes the end from the
  boxes for an input with a segment index and no edit list where FFmpeg's
  duration exceeds it by exactly the video's first composition time, and
  moves FFmpeg's packets after that input back by as much; any other
  difference still fails.
- Accepts: these values and no other: a file of 20.16 s from `ffprobe`
  and 20.08 s from CoreAV, a video stream of 20.08 s (257,024 ticks) and
  20.00 s (256,000 ticks), and a file bit rate from `ffprobe` that is
  20.08/20.16 (0.99603) of CoreAV's, the same size over each duration.
  The ratio, not the bit rate, is fixed: the encoder's output, and so the
  file's size, depends on the number of processors of the machine that
  generates the fixtures.

## KD-FF9-FIXTURE-COLOUR-RANGE

- Key: `streams.pix_fmt` (`color_hevc_colr_ignored.mp4`,
  `color_sps_range_only.mp4`).
- Cause: on these fixtures as FFmpeg 9.0.2 writes them, both FFmpeg
  versions print the range the parameter set signals (limited, then full)
  and CoreAV the range of the `colr` box. A CoreAV difference; a follow-up
  finds which change of the written stream it reads otherwise.
- Accepts: on a video stream, `yuv420p` against `yuvj420p` either way.

## KD-FF9-FIXTURE-PCE

- Key: `streams.channel_layout` (`audio_pce_entry_stereo.m4a`).
- Cause: both FFmpeg versions name the layout of the program config element
  of the AAC stream FFmpeg 9.0.2 writes (`hexagonal`); CoreAV names none. A
  CoreAV difference, for a follow-up.
- Accepts: on the AAC stream, `hexagonal` from `ffprobe` and none from
  CoreAV (`unknown`, or the key left out).

## KD-FF9-FIXTURE-SIDX-BIT-RATE

- Key: `streams.bit_rate` (`fragmented_sidx.mp4`).
- Cause: on the fragmented file with a segment index that FFmpeg 9.0.2
  writes, CoreAV gives each stream a bit rate far below the file's (37 and
  3 kbit/s against 749 and 66); on the same streams without the index
  (`fragmented_many.mp4`) it gives FFmpeg's. A CoreAV difference, for a
  follow-up.
- Accepts: on each stream, a bit rate from `ffprobe` between 19.5 and
  20.5 times CoreAV's (20.0 for the video, 19.9 for the audio here); the
  bit rates themselves follow the file's size, which the fixture machine
  changes (KD-FF9-FIXTURE-SIDX-DURATION).

## KD-FF9-FIXTURE-MKV-LAST-DURATION

- Keys: `packets.duration` and `packets.duration_time`.
- Cause: FFmpeg 9.0.2 writes a `DiscardPadding` on the last AAC block of
  these Matroska fixtures, and both FFmpeg versions shorten that block's
  duration by it (16 ms against 21 ms; 3 ms against 23 on one fixture);
  CoreAV does not. A CoreAV difference, for a follow-up.
- Accepts: a shorter duration from `ffprobe` on the last packet of an AAC
  stream, and no other packet's.

## KD-FF9-AV1-NCLX

- Key: `streams.mime_codec_string` (`color_av1_colr_ignored.mp4` only).
- Cause: [AV1 ISOBMFF section 5](https://aomediacodec.github.io/av1-isobmff/#codecsparam)
  requires an `nclx` box to override the sequence-header colour and
  range in the codec string. The fixture's stored values are 6/6/6 and
  full range. The pinned oracle instead prints the short default string.
- Accepts: exactly `av01.0.00M.08` from the oracle and
  `av01.0.00M.08.0.110.06.06.06.1` from CoreAV, on a video stream
  identified as AV1 with the `av01` codec tag. No other field is excluded.
- Evidence: the runner walks this fixture's boxes, requires one AV1
  sample entry, reads its `av1C` configuration and the exact `nclx`
  bytes, and derives the expected string from them. Changed bytes or
  outputs fail the gate. Both command runners check this before comparing
  commands, including commands on which the outputs already agree. If
  the oracle starts agreeing on the codec string, they fail explicitly
  and require removal of the obsolete exception. Other files, including
  corpus files, do not use it.

## KD-FF9-ANNEX-D-DOWNMIX

- Keys: `streams.dmix_mode`, `streams.ltrt_cmixlev`,
  `streams.ltrt_surmixlev`, `streams.loro_cmixlev` and
  `streams.loro_surmixlev`, on Matroska and WebM corpus files; no fixture.
- Cause: the same as KD-TS-ANNEX-D-DOWNMIX in MPEG-TS
  (coreav-dev/coreav#146). An AC-3 frame with `bsid` 6 uses the alternate
  syntax of ATSC A/52 Annex D, whose 2.3.1.2 to 2.3.1.6 give the preferred
  downmix and the Lt/Rt and Lo/Ro mix levels their meaning. Annex D 3.2
  lets a legacy decoder ignore them; it does not make the stream state no
  downmix. On a track whose frames state these values on more than two
  channels (`xbsi1e` set, `acmod` above 2), CoreAV prints them, each only
  for the channel modes Annex D defines it for. The pinned oracle prints
  `0` and `0.000000`, as for a stream that states no downmix: an oracle
  error.
- Corpus scope: the record's `level`, `fields` and `conditions` in the
  twin define the track from the file's bytes alone. Every frame of the
  track must be one whole AC-3 syncframe that passes `crc1`, and all must
  state the same values, so that the stream, not only its first frame,
  states them. A file whose blocks cannot all be reached gives no
  evidence: an element of unknown size other than the Segment, a header
  cut short, or an element past its parent or the file. The conditions
  also give the exact pair of values.
  `annex_d_downmix` in `scripts/oracle_kd_mkv.py` reads that evidence and
  retains a `spans` tuple of (block data offset, frame size) pairs in file
  order. The offset points to the block's TrackNumber vint; every frame
  of a lace shares it. `policy_ref.matroska_block_frames` exposes each
  block's offset and frames from the same complete walk that supplies
  `matroska_frames`. These pairs let a consumer bind packets by their
  bytes rather than treating TrackEntry order as stream order. Its tests
  fail when the pinned oracle stops printing `0` and `0.000000`. Not excludable yet: no corpus row is excluded until the
  corpus comparator gates rows on that evidence.

## KD-FF9-AV1-DEFAULT-SUFFIX

- Key: `streams.mime_codec_string`, on Matroska and WebM corpus files; no
  fixture.
- Cause: [AV1 ISOBMFF section 5](https://aomediacodec.github.io/av1-isobmff/#codecsparam)
  says that a codec string ending with `.0.110.01.01.01.0` (not
  monochrome, 4:2:0 with chroma sample position 0, BT.709 colour, studio
  range) SHOULD omit that suffix, and that omitted optional fields take
  those values. For an AV1 track whose sequence header states exactly
  these values, CoreAV omits the suffix, as the section recommends. The
  pinned oracle keeps it, which the section permits. Both strings state
  the same values.
- Corpus scope: the record's `level`, `fields` and `conditions` in the
  twin define the track from the file's bytes alone: its `CodecPrivate`,
  laid out as the AV1 codec mapping in Matroska requires, the sequence
  header in it, and any stored `Colour` elements, which must agree with
  that header. They also give the exact pair of strings.
  `av1_default_suffix` in `scripts/oracle_kd_mkv.py` reads that evidence,
  and its tests fail when the pinned oracle stops printing its string.
  Not excludable yet: no corpus row is excluded until the corpus
  comparator gates rows on that evidence.

## KD-FF9-AV1-CHROMA-POSITION

- Key: `streams.mime_codec_string`, on Matroska and WebM corpus files; no
  fixture.
- Cause: [AV1 ISOBMFF section 5](https://aomediacodec.github.io/av1-isobmff/#codecsparam)
  requires the third digit of the chroma subsampling field to equal the
  sequence header's `chroma_sample_position`, and without a colour
  description in the sequence header the colour fields SHOULD take the
  default values. A 4:2:0 track with chroma sample position 1 (vertical)
  and no colour description therefore gets the full string ending in
  `.0.111.01.01.01.0`, which CoreAV prints. The pinned oracle omits the
  optional fields, and omitted fields mean chroma subsampling `110`, so its
  string states position 0: an oracle error. The oracle gives position 2
  (co-located) without a colour description the same short string; no
  record covers that case, which still fails.
- Corpus scope: as for KD-FF9-AV1-DEFAULT-SUFFIX, with its own conditions
  in the twin and its own evidence, `av1_chroma_position` in
  `scripts/oracle_kd_mkv.py`.

## Corpus differences not set aside

These notes explain remaining differences in `run.py keys`. They do not
mask them, and the measured agreement rates still count them as failures.

- **EPG packets.** An EIT transport packet can finish several sections
  (ETSI EN 300 468 5.2.4; ISO/IEC 13818-1 2.4.4.2). CoreAV lists every
  complete section once. FFmpeg can list only the last one completed by
  a packet and repeat it when a later packet completes none. CoreAV does
  not discard a complete section or invent a repeat to match this. Once
  the packet counts differ, positional comparison also differs on the
  packets after that section.
- **Padding of a first Matroska block.** `CodecDelay` and a block's
  `DiscardPadding` each name samples a decoder discards (RFC 9559
  5.1.4.1.25 and 5.1.3.5.7). When the first block of a track with a codec
  delay also has padding, CoreAV's Skip Samples side data on that packet
  counts both; the pinned ffprobe keeps the delay and drops the padding.
- **Padding of a laced Matroska block.** RFC 9559 5.1.3.5.7 places a
  block's `DiscardPadding` at the end of the block when positive and at
  its start when negative. CoreAV takes it from the last frames of a lace,
  or from the first, each frame at most its own samples, so the samples
  trimmed are the padding, capped at the lace's. The pinned ffprobe lists
  the whole padding on every frame of the lace and trims each decoded
  frame by it.
- **Opus that states another input rate.** Opus decodes at 48 kHz
  whatever input rate its track and `OpusHead` state (RFC 7845 5.1), and
  both CoreAV and the pinned ffprobe count `CodecDelay` and
  `DiscardPadding` at 48 kHz. With a stated 24 kHz, a first block at 0
  and 6.5 ms of delay, the pinned ffprobe starts the stream at 0.006 s;
  CoreAV starts it at 0, its first block's timestamp and the first sample
  played.
- **Implicit AAC extensions.** ADTS and LATM configurations can state
  AAC LC while raw data blocks carry SBR or parametric stereo (ISO/IEC
  14496-3 1.5.1.1, 4.6.18 and 8.6.4). The bounded header reader detects
  some SBR payloads but does not decode arbitrary raw data blocks or
  parametric stereo. It leaves the decoded profile, sample rate or channel
  layout unknown where the configuration cannot establish them, rather
  than assuming that a mono core stays mono. Without a stored audio
  configuration, MIME strings depend on the same profile information.
- **AAC average bit rates.** Neither ADTS nor LATM states an average bit
  rate for a variable-sized stream. CoreAV averages the frames its bounded
  head read sees; FFmpeg's analysis can stop at a different frame. These
  estimates can differ without a sample size or sample rate being wrong.
- **Estimated base frame rates.** An irregular timestamp sequence need
  not have one base frame rate. CoreAV estimates it from a bounded decode
  timestamp window, using the signalled clock when no standard rate fits.
  FFmpeg's estimator can choose another rate. Neither estimate replaces
  the stored timestamps; no container clause chooses between them.
- **Cover picture positions in a compressed movie.** A cover picture's
  bytes are stored inside the movie box. When that box is compressed
  (QuickTime `cmov`, [`design/compressed-movie.md`](design/compressed-movie.md)),
  the picture has no position in the file. The pinned `ffprobe` (black
  box) prints the picture's offset inside the inflated movie box as its
  packet's `pos`. In the file, that offset falls on other bytes (inside
  `mdat`, on a synthetic file with three pictures). CoreAV lists those
  packets without a position. Track packets keep their file positions in
  both.
- **Highlight metadata as chapters.** Some movies store highlight times
  in an `HMMT` user-data atom rather than in a referenced chapter track or
  a `chpl` list. FFmpeg prints a chapter from that atom. CoreAV reads the
  documented QuickTime chapter and Nero chapter formats, and leaves this
  proprietary atom uninterpreted. The chapter formats' specifications do
  not settle the semantics of `HMMT`; this is unsupported metadata, not a
  claim that FFmpeg's interpretation is wrong.
