# Known divergences: `coreav ffprobe` versus `ffprobe` 9.0.2 on the fixtures

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
other. Every other difference, on any key or fixture, fails the run.
Keys are named as `run.py keys` names them (`streams.refs`); a side data
entry by its type (`packets.side_data_list[Skip Samples]`, with `[]` for
an entry the command prints empty because it selects none of its keys);
a stream group as `stream_groups`.

The first eleven entries are changes in what FFmpeg 9.0.2 prints: CoreAV
prints what 7.0.2 printed, and coreav-dev/coreav#24 follows 9.0.2. The last
four are CoreAV differences that the fixtures 9.0.2 writes expose: both
FFmpeg versions print the same value there, and CoreAV another.

## KD-FF9-NEW-KEYS

- Keys: `disposition.multilayer` (every stream), `mime_codec_string`,
  `view_ids_available` and `view_pos_available` (HEVC), and the AC-3 and
  E-AC-3 downmix keys `dmix_mode`, `ltrt_cmixlev`, `ltrt_surmixlev`,
  `loro_cmixlev` and `loro_surmixlev`, of `streams` and
  `programs.streams`.
- Cause: FFmpeg 9.0.2 prints these keys; 7.0.2 did not, and CoreAV does not
  yet.

## KD-FF9-DROPPED-KEYS

- Keys: `refs`, `closed_captions` and `film_grain` of a video stream (in
  `streams` and `programs.streams`), and `streams.tags.vendor_id`.
- Cause: FFmpeg 9.0.2 no longer prints a video stream's `refs`,
  `closed_captions` and `film_grain`, nor a sample entry's vendor
  `[0][0][0][0]` as a tag. CoreAV prints them, as 7.0.2 did.

## KD-FF9-SKIP-SAMPLES

- Keys: `packets.side_data_list[Skip Samples]`, `packets.side_data_list[]`,
  `packets.duration` and `packets.duration_time`.
- Cause: FFmpeg 9.0.2 lists the samples a decoder skips at the start of an
  audio stream (the encoder delay an edit list or a Matroska `CodecDelay`
  states) and discards at its end as Skip Samples side data, and gives a
  packet whose samples are trimmed another duration than 7.0.2 did (the
  whole MP3 frame at the end of an MP4 edit list, for example, where 7.0.2
  gave the part the list keeps). CoreAV lists no such side data and gives
  the durations 7.0.2 gave.

## KD-FF9-TS-STREAM-ID

- Keys: `packets.side_data_list[MPEGTS Stream ID]` and
  `packets.side_data_list[]`.
- Cause: FFmpeg 9.0.2 lists each MPEG-TS packet's PES `stream_id` as side
  data. CoreAV does not.

## KD-FF9-AAC-PADDING

- Key: `streams.initial_padding`.
- Cause: FFmpeg 9.0.2 gives an AAC stream's encoder delay (1,024 samples in
  these fixtures) as `initial_padding`, where 7.0.2 and CoreAV print 0.

## KD-FF9-MKV-START

- Keys: `format.start_time`, `streams.start_time` and `streams.start_pts`.
- Cause: FFmpeg 9.0.2 starts a Matroska or WebM file whose audio has a
  `CodecDelay` at its first block, and that audio stream at 0. FFmpeg 7.0.2
  and CoreAV start them that delay earlier (−0.021 s for AAC, −0.005 s for
  AC-3, −0.007 s for Opus in `browser.webm`).

## KD-FF9-ANNEXB-EXTRADATA

- Key: `extradata_size` of `streams` and `programs.streams`.
- Cause: for an H.264 stream in Annex B form (the MPEG-TS fixtures, and
  the two Matroska fixtures that store it so), the extradata FFmpeg 9.0.2
  gathers from the stream is two bytes longer than 7.0.2's, whose size
  CoreAV prints.

## KD-FF9-TS-PROBE-SCORE

- Key: `format.probe_score`.
- Cause: FFmpeg 9.0.2 gives an MPEG-TS file a probe score of 100, where
  7.0.2 and CoreAV give 50.

## KD-FF9-TIMECODE-GROUP

- Keys: `stream_groups` and `format.nb_stream_groups` (`timecode.mov`).
- Cause: FFmpeg 9.0.2 makes a QuickTime timecode track and the video it
  times a stream group, and prints it before the streams. CoreAV, as 7.0.2,
  prints no stream group.

## KD-FF9-MERGED-TAGS

- Key: `format.tags.creation_time` (`remux_metadata.mov`).
- Cause: given a creation time in two places of the file, FFmpeg 9.0.2
  prints both, joined by `;`. FFmpeg 7.0.2 and CoreAV print one.

## KD-FF9-FRAGMENT-BIT-RATE

- Key: `streams.bit_rate` (`fragmented_late.mp4`).
- Cause: FFmpeg 9.0.2 gives the stream twice the bit rate 7.0.2 and CoreAV
  give.

## KD-FF9-FIXTURE-COLOUR-RANGE

- Key: `streams.pix_fmt` (`color_hevc_colr_ignored.mp4`,
  `color_sps_range_only.mp4`).
- Cause: on these fixtures as FFmpeg 9.0.2 writes them, both FFmpeg
  versions print the range the parameter set signals (limited, then full)
  and CoreAV the range of the `colr` box. A CoreAV difference; a follow-up
  finds which change of the written stream it reads otherwise.

## KD-FF9-FIXTURE-PCE

- Key: `streams.channel_layout` (`audio_pce_entry_stereo.m4a`).
- Cause: both FFmpeg versions name the layout of the program config element
  of the AAC stream FFmpeg 9.0.2 writes (`hexagonal`); CoreAV names none. A
  CoreAV difference, for a follow-up.

## KD-FF9-FIXTURE-SIDX-DURATION

- Keys: `format.duration`, `format.bit_rate`, `streams.duration`,
  `streams.duration_ts` and `streams.bit_rate` (`fragmented_sidx.mp4`).
- Cause: on the fragmented file with a segment index that FFmpeg 9.0.2
  writes, both FFmpeg versions end the first stream 80 ms later than CoreAV
  (20.08 s against 20.00 s), and the file accordingly; 9.0.2 also gives
  that stream another bit rate. A CoreAV difference, for a follow-up.

## KD-FF9-FIXTURE-MKV-LAST-DURATION

- Keys: `packets.duration` and `packets.duration_time`.
- Cause: FFmpeg 9.0.2 writes a `DiscardPadding` on the last AAC block of
  these Matroska fixtures, and both FFmpeg versions shorten that block's
  duration by it (16 ms against 21 ms); CoreAV does not. A CoreAV
  difference, for a follow-up.
