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

The first twelve entries are changes in what FFmpeg 9.0.2 prints: CoreAV
prints what 7.0.2 printed, and coreav-dev/coreav#24 follows 9.0.2. The
thirteenth is an FFmpeg error on a fixture 9.0.2 writes, which ISO/IEC
14496-12 decides for CoreAV. The last four are CoreAV differences that the
fixtures 9.0.2 writes expose: both FFmpeg versions print the same value
there, and CoreAV another.

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
  end of a Matroska one), and CoreAV lists what 7.0.2 did. A packet that
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

## KD-FF9-MKV-START

- Keys: `format.start_time`, `streams.start_time` and `streams.start_pts`.
- Cause: FFmpeg 9.0.2 starts a Matroska or WebM file whose audio has a
  `CodecDelay` at its first block, and that audio stream at 0. FFmpeg 7.0.2
  and CoreAV start them that delay earlier (−0.021 s for AAC, −0.005 s for
  AC-3, −0.007 s for Opus in `browser.webm`), and an attachment stream with
  the file (`mkv_attachment.mkv`).
- Accepts: 0 from `ffprobe` and less from CoreAV, for the file and for an
  audio or attachment stream.

## KD-FF9-ANNEXB-EXTRADATA

- Key: `extradata_size` of `streams` and `programs.streams`.
- Cause: for an H.264 or HEVC stream in Annex B form (the MPEG-TS
  fixtures, and the two Matroska fixtures that store H.264 so), the
  extradata FFmpeg 9.0.2 gathers from the stream is longer than 7.0.2's,
  whose size CoreAV prints: two bytes for H.264, three for HEVC.
- Accepts: those two bytes more from `ffprobe` on an H.264 stream, three
  on an HEVC stream.

## KD-FF9-TS-PROBE-SCORE

- Key: `format.probe_score`.
- Cause: FFmpeg 9.0.2 gives an MPEG-TS file a probe score of 100, where
  7.0.2 and CoreAV give 50.
- Accepts: 100 from `ffprobe` and 50 from CoreAV.

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
  and 20.08 s from CoreAV, its bit rate 844,284 and 847,648, and a video
  stream of 20.08 s (257,024 ticks) and 20.00 s (256,000 ticks).

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
- Accepts: these two pairs and no other: 749,136 and 37,457, 65,892 and
  3,310.

## KD-FF9-FIXTURE-MKV-LAST-DURATION

- Keys: `packets.duration` and `packets.duration_time`.
- Cause: FFmpeg 9.0.2 writes a `DiscardPadding` on the last AAC block of
  these Matroska fixtures, and both FFmpeg versions shorten that block's
  duration by it (16 ms against 21 ms; 3 ms against 23 on one fixture);
  CoreAV does not. A CoreAV difference, for a follow-up.
- Accepts: a shorter duration from `ffprobe` on the last packet of an AAC
  stream, and no other packet's.
