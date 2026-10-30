# Transcoding to H.264 in MP4

Status: Accepted

`coreav transcode IN OUT.mp4` re-encodes one video stream to H.264 and
copies the audio, the shape of the most common `ffmpeg` job:

    ffmpeg -i IN -c:v libx264 -crf 23 -preset medium -c:a copy OUT.mp4

It is the fourth item of the transcoding order, "video transcoding
pipelines", for one encoder: x264, which
that order allows only "as a separately installed program that the
operator provides, run as a separate process like the FFmpeg fallback". This note sets which program that is, how frames reach
it, how its output becomes MP4 samples, and the bars, fixed before any
measured run.

**Support.** With an H.264 source, the command is supported: on the
corpus, H.264 sources met B1 to B4 on both encoder arms in both
configurations and B5 (timed without scaling) on both arms, and the
command met B3, B6 and B7 (Results). Two misses
remain that are not about the H.264 corpus: the fixture `size_inband_sps.mp4`
(H.264 without composition offsets) misses B2, where the oracle's times
follow FFmpeg's decoder delay and the baseline misses as well; and B8's
fixture CI job is not in place. The encoder's memory cap (half the host's
memory, "Limits") applies to every source codec: an 8K output can need
more on a small host, measured on three 8K HEVC files. From this release, its options
and its summary document (`coreav.transcode/1`) are stable and change only
with a deprecation period, or a new schema version for the summary. With
any other source codec (HEVC, AV1, VP9, VP8, ProRes) it is experimental:
those classes missed bars (Results), and what the command does with them
may change, while its options and summary stay the same. The help text
and the README say so, with the measured numbers.

Specifications: ITU-T H.264 (7.3 and 7.4 for the NAL unit, parameter set
and slice header syntax the encoder output is read with, 8.2.1 for picture
order counts, Annex B for the byte stream format), ISO/IEC 14496-15 (the
`avc1` sample entry and `avcC`), ISO/IEC 14496-12 (sample tables, edit
lists, `pasp`, `colr`), ITU-T H.273 (colour code points). x264 and FFmpeg
are used only as programs, through their documented command-line options
and observed as black boxes; no encoder source is read, and no library is
linked.

## Scope

In:
- Inputs: MP4, MOV, Matroska and WebM, read by CoreAV's own demuxers
  (`coreav-isobmff`, `coreav-mkv`).
- Video: the stream `coreav thumbnail` decodes (FFmpeg's default choice:
  the highest resolution, the first on a tie), decoded through the decode
  boundary (`docs/design/decode.md`) with the backends of the build: the
  native H.264 decoder (`native-h264`), dav1d for AV1 (`dav1d`), libvpx
  for VP8 and VP9 (`libvpx`), the sandboxed FFmpeg fallback (`ffmpeg-fallback`) for
  the rest. Encoded to
  H.264 by x264, with `--crf` or `--bitrate`, `--preset`, and `--scale`
  (`coreav-frame`'s resize).
- Audio: every audio stream that MP4 can carry, stream-copied as
  `coreav remux` copies it. Streams it cannot carry (Vorbis, PCM in MOV)
  are dropped and listed in the summary, as `coreav remux` does by
  default; `--audio none` drops all audio. The remux is `coreav remux`'s
  default selection (every stream it can carry), so another stream MP4
  carries (a second video stream, MP4 text) is copied too and listed in
  the summary's `other`; naming the audio streams instead would fail the
  whole transcode, after encoding, on one the writer cannot carry. For
  Matroska sources another video stream MP4 cannot carry is dropped and
  listed rather than failing the remux.
- Audio encoded to Opus instead (`--audio opus`, in builds with the
  `libopus` feature): the first audio stream CoreAV decodes (FLAC or MP3,
  mono or stereo) through libopus, in a sandboxed process of its own; the
  other streams are dropped and listed ("Opus audio").
- Audio encoded to AAC-LC instead (`--audio aac`): the first audio stream
  CoreAV decodes (FLAC or MP3, mono or stereo, 8 to 64 kHz) through the
  `ffmpeg` program's built-in `aac` encoder, run as a separate process;
  the other streams are dropped and listed ("AAC audio").
- Output: MP4 with the movie box first, written as the other writing
  commands write (a temporary file renamed over `OUT` on success).

Out, each needing its own roadmap item:
- Other audio encoders and AAC profiles (HE-AAC), Opus or AAC of more
  than two channels (Opus's channel mapping family 1), resampling to an
  AAC rate, and audio CoreAV does not decode yet (AAC, Opus).
- Other video encoders (NVENC, SVT-AV1, `rav1e`): policy record 0042
  allows them as backends; each is an item of its own.
- Smart rendering, the copy-when-possible router, ladders, other
  filters, MKV output, MPEG-TS input,
  trimming (`-ss`, `-t`), two-pass rate control.

## Pipeline

```
CoreAV process                                   separate processes
-------------------------------------------      ------------------------
demux (safe Rust) --packets--> Decoder ----------> worker (fallback, dav1d)
                              (native in-process)
frames <-------------------------------------------- shared memory, copied
  | presentation order, limits checked (decode.md)
  | resize to the output size (coreav-frame)
  v
writer thread --raw planes, pipe--------------> encoder program (x264, or
                                                 ffmpeg -c:v libx264)
reader thread <--H.264 Annex B, pipe-------------'
  | NAL units -> access units -> picture order count -> presentation index
  v
spool file (encoded video, decode order) + sample list
  |
  v
MP4 writer: the source remuxed with the video stream's samples replaced
```

Each stage runs while the others do: the decoder (its own threads or
process), the main thread (demuxing, sending packets, receiving frames,
resizing), the writer thread (the encoder's standard input), the encoder,
and two reader threads, one for its standard output and one for its
standard error, which keeps the last 64 KiB for an error message and
discards the rest (the encoder runs with `--log-level error
--no-progress` or `-loglevel error -nostats`, but nothing depends on it
staying quiet: x264 prints its closing statistics line even then). Queues between them are bounded (two
frames to the writer), so a slow encoder slows decoding instead of growing
memory, and the readers drain both of the encoder's output pipes, so no
pipe can fill and stall it.

The main thread sends the next packet only after it has handed every frame
the decoder has ready to the writer's queue. Packets in flight therefore
stay near the decoder's reorder depth, far below
`Limits::max_pending_bytes`: the known limit of `docs/design/decode.md` (a
caller that streams packets ahead of a slower decoder fails with `Limit`)
does not arise, because the consumer here paces itself by what the
decoder returns.

Before anything is decoded, an HEVC stream whose first sequence
parameter set has a conformance window that leaves no picture, which
ITU-T H.265 7.4.3.2.1 does not allow, is rejected as an invalid
structure (`invalid_structure`, exit status 1). The check is in
`coreav-thumbnail`'s reading of the video stream, so `coreav transcode`,
`coreav ffmpeg` and `coreav thumbnail` reject it alike: the pinned
FFmpeg 9.0.2 drops such a parameter set and decodes nothing, where 7.0.2
ignored the window (`KD-HEVC-WINDOW`; `size_hevc_window.mp4`,
`size_hevc_mixed_window.mp4`). The writers that copy the stream refuse it
too (`docs/design/remux.md`, "Streams"). The bars set the two fixtures
apart as they set any rejected file apart.

### The encoder program

The roadmap leaves the choice between an `x264` binary and
`ffmpeg -c:v libx264` to the first design note that needs it. Both are
supported behind one boundary, raw frames in and an H.264 Annex B byte
stream out:

    x264 --demuxer raw --input-csp <csp> --output-csp <csp>
         --input-depth <d> --output-depth <d> --input-res WxH
         (--tcfile-in FILE | --fps N/D)
         [--colorprim .. --transfer .. --colormatrix ..]
         [--input-range R --range R] [--sar N:D]
         --preset P (--crf Q | --bitrate K) [--threads T]
         --log-level error --no-progress --muxer raw --output - -

    ffmpeg -nostdin -hide_banner -loglevel error -nostats
         (-f matroska | -f rawvideo -pix_fmt <fmt> -video_size WxH -framerate N/D)
         [-color_primaries .. -color_trc .. -colorspace ..] [-color_range R]
         -i pipe:0 -map 0:v:0 -fps_mode passthrough
         -c:v libx264 -preset P (-crf Q | -b:v Kk) [-x264-params sar=N/D]
         [-threads T] -f h264 pipe:1

(`coreav-transcode`'s `encoder::x264_args` and `encoder::ffmpeg_args`.)
`coreav transcode` uses the first forms, which give the encoder every
frame's own time ("Rate control and timestamps"); the constant-rate forms
(`encoder::Timing::Constant`) are kept for the encoder's own tests.
The flags were checked on the pinned programs as black boxes
(`crates/coreav-transcode/tests/encoder.rs`), and four differ from a first
reading of the programs' help:

- `--output-csp`: x264 encodes 4:2:0 unless told otherwise, and this build
  (no swscale) refuses 4:2:2 input without it, with nothing on standard
  error under `--quiet`.
- `--log-level error`, not `--quiet`: `--quiet` also silences the errors
  of x264's filters (the refusal above), which the error message needs.
- The range on both sides, `--input-range R --range R`: x264 converts the
  levels when the two differ. With both `pc`, flat frames at 0 and at the
  maximum (8 and 10 bits) decode to 0 and the maximum again, with either
  program; an unknown range is not signalled (`--range auto`, and no
  `-color_range`), which H.264 reads as limited.
- The sample aspect ratio through `-x264-params sar=N/D`, not `-aspect`:
  `-aspect` takes a display aspect ratio as a number, and 4095:4096 at
  720x480 came back as 8:9. With `-x264-params` the stream's VUI has the
  ratio asked for (10:11, 64:45 and 4095:4096 checked), in every sequence
  parameter set of the stream. x264's `--sar` is exact too. A ratio whose
  reduced terms exceed H.264's 16 bits is refused.

FFmpeg's colour options work as input options (on the raw video: the
frames carry the description to libx264, as a decoder's frames do) and as
output options alike; they are given as input options. Every H.273 code
point CoreAV passes comes out in the VUI as asked, with both programs:
primaries 1 and 4 to 12, transfer 1 and 4 to 18, matrix 0, 1 and 4 to 14.
Primaries 22 (EBU Tech. 3213) are not passed: x264 has no name for them,
and libx264 drops them when FFmpeg passes them. 10-bit input comes out as
High 10, 4:2:2 as High 4:2:2 and 4:4:4 as High 4:4:4 Predictive, with the
input's depth, from both programs. The chroma location is not passed.

`--encoder x264|ffmpeg` picks one; the default, `auto`, takes an `x264`
program when one is named (`--x264 PATH`, `COREAV_X264`) or on `PATH`, and
otherwise the `ffmpeg` program the fallback uses (`--ffmpeg PATH`,
`COREAV_FFMPEG`, `PATH`), which must have `libx264`. In strict-safe mode
(`coreav --strict-safe`, `COREAV_STRICT_SAFE`; `compat-cli.md`,
"Strict-safe mode") no `ffmpeg` program runs: `--encoder ffmpeg` is
refused, and `auto` takes an `x264` program only, refused when there is
none; both with exit status 4, before the input is read, with a message that
names installing x264 and turning the mode off. The decoder is held to the
same rule (`decode.md`, "The FFmpeg fallback").

- **Why both.** An operator who installs x264 gets the encoder with no
  FFmpeg at all; with a decoder backend of CoreAV's own (native H.264,
  dav1d, libvpx), no FFmpeg C code then runs. An operator who has FFmpeg for the
  fallback already has libx264 in most builds. The pinned oracle FFmpeg
  (7.0.2, static) carries libx264 core 164 r3191 (4613ac3), so CI and the
  measurements have the same encoder in the baseline and in CoreAV.
- **What the encoder sees.** Only raw pixels and the command line CoreAV
  builds: never the input file, its container or its bitstream. The
  untrusted bytes are parsed by CoreAV's demuxers and decoded by the
  decode boundary's backends; FFmpeg as an encoder runs only its raw-video
  reader and libx264.
- **Sandbox.** The encoder runs under the worker's resource limits
  (`docs/design/decode.md`, "Process" and "Exec"): CoreAV starts itself
  again with a hidden argument (`coreav_worker::exec`), which sets the limits on itself through `rustix`'s
  safe `setrlimit` and replaces itself with the encoder through `std`'s
  safe `exec`, so no process sits between the pipes and the encoder. The
  limits: the address-space budget (`Limits::max_memory_bytes`; advisory
  on macOS, `docs/design/decode.md`, "Memory", so strict-safe mode starts
  no encoder there), no core
  files, no regular-file writes (`RLIMIT_FSIZE` 0; the encoder writes only
  to its pipe), a small descriptor count, and CPU seconds bounded by the
  timeout times the CPU count. For the x264 program on input of more than
  8 bits the launcher also turns transparent huge pages off: its conversion
  of that input stalls on them, and without them it takes about 0.84 times
  as long on 4K frames (`docs/measurements/transcode-x264-10bit.md`, "Huge
  pages"). 8-bit input and `ffmpeg` keep them. It runs in its own process group with a
  cleared environment, and is killed with its group at the deadline, on an
  error, or when the transcode is dropped. Its keeper looks every 20 ms
  whether it has exited (Linux's `waitid` with `WNOWAIT`, which leaves it
  unreaped); when it has, the keeper kills what is left of its group,
  reaps it and ends the session at once with its status, so neither an
  encoder that fails early nor a process that inherited its pipes holds
  the transcode until the deadline. On other Unix systems an exit cannot be
  seen without reaping, which would let the group's id be reused before
  the kill: there the encoder counts as exited 0.5 s after its output
  ended, and is then killed with its group and reaped (one that had
  exited keeps its own status). Reaching the CPU limit is a `SIGKILL` (the
  limit is both soft and hard), reported as the encoder's failure like any
  other signal. Like the decode worker, this gives crash isolation and
  resource bounds, not confinement (seccomp and namespaces are the stage 5
  server-hardening item): a process the encoder starts in a session or
  group of its own (`setsid`) is outside the group kill, and only the
  `RLIMIT_CPU` it inherited bounds it; the transcode still ends at its
  deadline.
- **Rate control and timestamps.** Frames are written in presentation
  order, each with its source presentation time, as FFmpeg hands libx264
  the decoded frames' own times when it reads the source itself; the
  rate control (x264 is told the input is variable-rate) then sees the
  frames' real spacing. The encoder's own timestamps are still not used
  for the output: each output picture is mapped back to the frame it
  encodes ("From Annex B to samples"), and that frame's source timestamp
  is the sample's.
  - **x264** reads the times from a timecode file (`--tcfile-in`, format
    v2: one time a line in milliseconds from the first frame, six
    decimals, so exact to the nanosecond for any source time base), which
    CoreAV writes next to the spool file before the encoder starts and
    removes with it. The file lists the presented frames' times from the
    packet list (after the edit list, each repeat moved or dropped as in
    "Which frames"), which are the frames sent: a frame the decoder does
    not return, or returns out of order (dropped and counted), shifts the
    later times of the file
    against the frames, which changes only the rate control's view of
    their spacing, never the output's timestamps. x264 takes its time
    base from the file: `--timebase` set to the source's changed the
    encode of `vfr.mp4`, and without it the frames decode exactly as those
    of `ffmpeg -c:v libx264` on the source (only the stream's headers
    differ: x264's own VUI timing and settings string).
  - **FFmpeg** cannot read timestamps with raw video (`-f rawvideo` has a
    constant rate), so its input is a Matroska stream CoreAV writes
    (`coreav-transcode`'s `rawmkv`): one `V_UNCOMPRESSED` track whose
    `ColourSpace` FourCC names the layout (`I420`, `Y42B`, `444P`, and
    `Y3` with the chroma shifts and the depth at 10 bits; each read back
    byte for byte by the pinned FFmpeg, odd sizes included), and each frame
    a SimpleBlock in a Cluster of its own whose timestamp is the frame's
    time from the first. `TimestampScale` is the source time base in
    nanoseconds when that is a whole number (78,125 for 1/12,800), so
    FFmpeg's stream keeps the source's time base, and 1 ns otherwise (the
    times rounded to it; a millisecond time base gave the same encode).
    FFmpeg finds the stream's frame rate from the frames' times and hands
    it to libx264. From one frame's time it finds none and took the
    inverse of the time base, and `codec_h264_one_frame.mp4` came out 7 dB
    below the baseline in PSNR (bar B9's `x11`). So a stream of one frame
    names the rate, the track's `DefaultDuration` at the summary's
    `frame_rate`, and its encode is the baseline's; CoreAV holds the
    first frame until the second or the end of the input says which. A
    stream of more frames names none: FFmpeg times the frames by a rate it
    is told, which changed the encode of frames at irregular times. On
    `vfr.mp4` the H.264 stream is then byte for byte the baseline's
    (`ffmpeg -noautorotate -i vfr.mp4 -map 0:0 -c:v libx264 -preset
    veryfast -crf 23 -fps_mode passthrough`), and x264's decodes to the
    same frames.
  - **VUI timing** is what each encoder derives from those times, as for
    FFmpeg's own output: no constant rate is told, so the earlier problem
    of a told rate below the closest two frames' (`ffmpeg -i OUT -f null
    -` reporting frames on one tick, bar B1) does not arise; `vfr.mp4`
    decodes cleanly from both arms. The summary's `frame_rate` is the
    presented frames' average rate (frames over the first frame's time to
    the last one's plus its packet duration), snapped to `N×1000/1001` or
    an integer rate within 0.1% of one, for information, and the rate
    FFmpeg is told for a stream of one frame.
- **Options.** `--preset` (x264's names), `--crf` (default 23, x264's and
  FFmpeg's default) or `--bitrate` (single pass, ABR), and `--threads`
  (default: the encoder's own choice). Nothing else of x264's option space
  is exposed in this version; profile and level are x264's choice for the
  input format.

### Frames to the encoder

- **Formats.** The decoded format goes to the encoder unchanged, as FFmpeg
  hands libx264 the decoder's format by default: planar Y'CbCr 4:2:0, 4:2:2
  or 4:4:4 at 8 or 10 bits (`i420`/`i422`/`i444`; at 10 bits,
  `--input-depth 10 --output-depth 10` with two little-endian bytes a
  sample, which is `coreav-frame`'s layout, so x264 encodes 10 bits as
  libx264 does for FFmpeg's 10-bit formats), and
  NV12 (NVDEC's output) converted to planar by `Picture::to_planar`
  without loss. Monochrome, GBR, other depths and packed formats are
  refused with `Unsupported` before the encoder starts, and counted apart
  in the measurements.
- **Size.** 4:2:0 and 4:2:2 need an even width (and 4:2:0 an even height);
  an odd source size is refused unless `--scale` gives an even one.
  `--scale W:H` takes FFmpeg's `scale` filter forms: `-1` keeps the aspect
  ratio, `-2` keeps it and rounds to an even number. The resize is
  `Picture::resize_with` with the bicubic filter and
  `ChromaSiting::FfmpegCompatible`: FFmpeg's default scaler at the same
  pixel format, measured as a black box (`docs/design/thumbnails.md`,
  "Y'CbCr to Y'CbCr"), which takes chroma as centred whatever the stream
  signals. With the stream's own siting, `size_vp9_resize.mp4` was 25 to
  28 dB further from FFmpeg's chroma; with this mode every plane is within
  FFmpeg's fixed-point rounding (at least 61 dB). The
  sample aspect ratio is changed so the display aspect ratio is kept, as
  FFmpeg's `scale` does. A frame whose size differs from the first (a
  stream that changes resolution) is resized to the output size, as
  FFmpeg's default `autoscale` does. Measured on the pinned FFmpeg as a
  black box (`coreav-transcode`'s `scale` tests): `-1` is the other side
  times the input's width-to-height ratio (of its pixel grid, not its
  display aspect ratio) rounded to the nearest pixel, halves up; `-2` the
  same rounded to the nearest even number, halves up; both sides negative
  keep the input's size, odd or not; a side that rounds to 0 is refused.
  The output's sample aspect ratio is the source's (`pasp` or Matroska's
  display size; 1:1 when not stated) times `(in_w × out_h) / (in_h ×
  out_w)`, reduced; it reaches the encoder (and `pasp`, and the track
  header's width, stretched by it as FFmpeg writes it) only when it is not
  1:1. The size comes from the first presented frame, and the encoder
  starts with it: a format the encoder does not take, an odd size, or a
  `--scale` that gives none is refused then, before the encoder runs; a
  later frame in another pixel format is `Unsupported`.
- **Transfer.** Planes are written to the encoder's standard input packed,
  without row padding, by the writer thread (for FFmpeg inside the
  Matroska blocks of "Rate control and timestamps", a few bytes of
  element headers a frame). A 1080p 8-bit 4:2:0 frame is
  3.1 MB; through a pipe that is about a millisecond of copying, against
  tens of milliseconds of encoding a frame at the default preset.
- **Colour.** The colour description the decoder applies (H.273 code
  points, as `coreav thumbnail` reads it) goes to the encoder's VUI
  options, as FFmpeg passes the decoded frames' description to libx264.
  The stream's description (`coreav_thumbnail::VideoStream`) is taken, the
  first frame's filling what it leaves unknown. The `colr` box (`nclx`)
  says the same: written when a code point is known, unknown ones as 2
  (unspecified), without primaries 22, which the VUI cannot carry either.
- **Orientation.** Frames are encoded as decoded, and the source track's
  matrix is kept: the rotation stays metadata, as `ffmpeg -noautorotate`
  does, instead of a turn of every frame.
- **Which frames.** Every frame the source presents, in presentation
  order: frames the source's edit list does not present are decoded as
  references and not encoded. Edit lists of the simple model (at most one
  empty edit, then one media edit at rate 1) are applied; others
  (`EditList::Multiple`, `docs/design/edit-lists.md`) are refused in this
  version. A frame whose timestamp is below the one before it is dropped
  and counted in the summary (`dropped_frames`), as is a frame without
  one. A frame whose timestamp repeats the one before it (Matroska's
  millisecond block times can give several frames one time: a burst
  of frames from a recording) is kept and sent one tick of the source's time base
  after the frame before, when that is below the source's next time
  (after the last time, always); otherwise it is dropped and counted. An
  MP4 cannot hold two frames at one time (its decode times increase), so a
  repeat takes the nearest time the track can give it, and no frame is
  ever moved onto or past a later frame's own time: the video does not
  drift from the copied audio, and a stream of more frames a second than
  ticks keeps one frame a tick, as before. Rejected: dropping every
  repeat, as the first version did, which lost 4 of 344 and 116 of 1,646
  frames of two corpus WebM files and kept the oldest picture of a burst
  on screen until the next time; moving repeats without a bound, which at
  more frames a second than ticks moves every later frame further (a
  1,500 frames a second stream in milliseconds plays half as long again)
  and puts the video out of step with the audio; spreading a burst over
  the gap that follows it, which moves frames further from the times the
  source states for a spacing nothing in the file gives; a finer output
  time scale where the source repeats a time, which would keep every
  frame within a source tick, but gives those files another time base
  than the source's (B2 is exact only where the output keeps it) and
  another edit list and audio offset arithmetic, for frames that a viewer
  sees for a millisecond either way (a judgment, not measured).
  "Presents" is the demuxer's: an MP4 frame is encoded when the edit
  presents any part of it (`coreav_isobmff::Packet::presented`), a
  Matroska frame unless it is invisible. A frame that starts before the
  media edit and ends inside it (an edit list that `ffmpeg -ss` with
  stream copy writes) is therefore encoded; FFmpeg's demuxer drops such a
  frame when it reads the source, and the output alike, so `ffprobe`
  counts the same frames in both. Packets the source does not present are
  sent with `discard` set, so a sandboxed decoder need not copy their
  frames back.
- **Times in decode order.** An MP4 track whose samples have no
  composition offsets (no `ctts`, or only zeros: what FFmpeg writes when
  it copies raw H.264 with B-frames into MP4) gives every sample its
  decode time as its presentation time, and a decoder that reorders
  pictures returns them with times out of order. The rule above would
  then drop every reordered frame (23 of the 48 of the
  `size_inband_sps.mp4` fixture). For such a track each presented frame
  instead takes the earliest presented time of the packets sent that no
  frame took yet: the presented times in increasing order, in the order
  the decoder returns the frames. This applies to the codecs whose
  pictures can be decoded out of presentation order (H.264, HEVC, MPEG-1,
  -2 and -4 video); AV1, VP8, VP9 and ProRes keep the rule above. On a
  track without reordering whose times increase this is each frame's own
  time; where two samples share a time, both frames take it and the
  later one is moved one tick or dropped, as above. Rejected: FFmpeg's
  `best_effort_timestamp`,
  which takes the packets' decode times shifted by the decoder's delay
  as the decoder finds it, so its times on that fixture skip one frame
  interval where the delay grows; the file states no other times than
  its samples'. A frame the decoder leaves out of such a track moves the
  later frames' times by one frame; with no presentation times in the
  file nothing tells which frame was left out.

### From Annex B to samples

The reader thread splits the encoder's byte stream into NAL units (start
codes, Annex B), groups them into access units (a new one starts at an
access unit delimiter, a sequence or picture parameter set, an SEI, or a
slice whose `first_mb_in_slice` is 0, after a slice; ITU-T H.264
7.4.1.2.3), and reads, in safe Rust and from the specification, what each
needs:

- the sequence and picture parameter sets, as far as the slice header
  needs them, and the slice header through `dec_ref_pic_marking` (the
  picture order count fields, and `memory_management_control_operation`
  5 after the reference list modifications and weights);
- the picture order count (8.2.1, types 0, 1 and 2), per IDR period, and
  from it the presentation index of each picture: within an IDR period,
  pictures are presented in increasing order count, and periods follow
  each other. A picture with `memory_management_control_operation` 5
  starts a new period, as an IDR does: every earlier picture is presented
  before it, and its order count becomes 0 (8.2.1). An IDR with
  `no_output_of_prior_pics_flag` set, whose earlier pictures would never
  be output, fails the transcode.

Because frames went in in presentation order and x264 never drops or adds
frames, the `n`-th picture in presentation order encodes the `n`-th frame
sent. That is checked, not assumed: the encoder must return exactly as
many pictures as frames were sent, with presentation indices that are a
permutation of them; anything else fails the transcode with `Backend`.

Samples: each access unit, in decode order, converted from start codes to
4-byte length prefixes, without its parameter sets when they equal the
ones in `avcC` (x264 repeats them before every IDR; the sample entry has
them, as FFmpeg's MP4 output has). Parameter sets that change within the
stream fail the transcode: x264 writes one of each. The first unit's SEI
(x264's settings string) stays in the first sample, as in FFmpeg's output.
`avcC` is built from the parameter sets (profile, compatibility and level
from the sequence parameter set; `chroma_format`, the bit depths and the
sequence parameter set extensions, which ISO/IEC 14496-15 requires for
every `profile_idc` other than 66, 77 and 88: x264's 8-bit 4:2:0 output is
High, 100, and its 10-bit, 4:2:2 and 4:4:4 outputs 110, 122 and 244; the
record equals the one FFmpeg writes for the same stream, checked in the
reader's tests). IDR access units are the
sync samples (`stss`); other I pictures are not.

Timestamps: picture `k` in decode order presents source frame `r(k)`, at
the source's presentation time `T[r(k)]`, in the source track's time
scale. Decode times are the sorted presentation times shifted back by the
largest reorder lead, `D[k] = T[k] − δ` with
`δ = max(T[k] − T[r(k)])`, so decode times increase and never pass
presentation times (`ctts` offsets are non-negative). The edit list maps
the first presented time to where the source track's presentation started,
so the video keeps its place against the copied audio. The last sample's
duration is the source frame's: the packet duration of the last frame
sent (as in the frame rate above), else the interval before it, else one
tick.

The encoded stream goes to a spool file next to the output (removed on
success and on failure), because the movie box comes first and needs every
sample's size; the encoded video is small next to the frames, and a
sequential write and read of it costs little against encoding. It is
`.{name}.coreav-{pid}.spool` beside the output path (x264's timecode file
beside it as `.{name}.coreav-{pid}.timecodes`, both removed on success and
on failure), created new (never shared with a file already there) with
mode 0600, and kept open: the encoder's reader thread writes the spool
file through a copy of that handle as the Annex B reader finishes each
period's pictures, the timecodes are written through it, and the spool
file is read back as the replacement's data through it too, never by path
again, so a link put in its place in a shared directory cannot redirect a
write or a read (x264 alone opens the timecode file by its path; a swap
there changes only the times it reads, which the output does not take). The output is the command's temporary file renamed
over `OUT`, so `OUT` must be a file: `-` is refused, since the spool file
and the movie box first need a place and a known size.

The reader's picture limit follows the frames sent: the count goes up
before each frame is queued, so the encoder can never return a picture
for a frame it has not been given, and once the encoder has finished the
limit is the frames sent. Encoder failures, a stream that is not one
picture for each frame, the reader's errors and the MP4 writer's are
`coreav.error/1` objects of the existing kind `decode` (`limit`, with
`timeout` at the deadline; `io` for an I/O error), their messages starting
with `encoding:`; the input side's refusals and malformed-input kinds are
never used for them ("Bars"). That includes errors reading the source
again during the final remux: the demuxer read it once already, so they
surface as `decode` and count against the bars.

### The MP4 writer

`coreav-isobmff-write` gains one operation: remux a source with one of its
video streams replaced. The source's other kept streams go through the
existing remux paths unchanged (`remux_mp4` for MP4 and MOV,
`remux_matroska` for Matroska and WebM), so audio is copied exactly as
`coreav remux` copies it, with its edit list, priming and language. The
replaced stream keeps the source track's place, language, matrix and
presentation timeline, and takes the new sample entry (`avc1`, `avcC`;
`pasp` when the sample aspect ratio is not 1:1; `colr` with the colour
code points when they are known), the samples above and an edit list
shifted by the new decode times. The source track's per-sample boxes
(`sdtp`, sample groups) are not copied: they describe the old samples.

The contract between the transcoder and the writer
(`remux_mp4_replacing`, `remux_matroska_replacing`; details in
[`remux.md`](remux.md), "Replaced streams"): the samples' decode
and presentation times are the source stream's own timestamps, in its
units and on its timeline. For MP4 and MOV, media ticks of the track's
media timescale, presentation being the composition time (decode time plus
composition offset) its sample tables give; for Matroska and WebM, Segment
Ticks (`TimestampScale` units), the blocks' timestamps as
`coreav_mkv::Packet::pts` gives them. A frame with presentation time `P`
is then presented where `coreav remux` presents a copied source frame with
that time. The media edit keeps the gap the source's own edit leaves
before its first frame (none when the source has no edit list), the empty
edit growing when leading frames are left out; with `--scale`, a rotation
matrix's translation follows the new size.

## Opus audio

`coreav transcode --audio opus [--audio-bitrate KBPS]` encodes one audio
stream to Opus (RFC 6716) with libopus, as policy record 0042 allows an
encoder backend: a crate of its own, `coreav-backend-libopus`, built only
with the `libopus` feature, which links the system's libopus (BSD-3-Clause)
through the `opusic-c` crate and its `opusic-sys` binding (both
BSD-3-Clause), with their bundled build of libopus turned off: the library
is found where the loader looks by default, or through a runtime search
path built into the binary, since the encoder process runs with a cleared
environment (`LD_LIBRARY_PATH` does not reach it). The default features
do not link it. The release builds do, with a static libopus 1.6.1 that
`packages/cli/scripts/codecs.sh` builds from a pinned tarball and
`opusic-sys` links from `OPUS_LIB_DIR` (`docs/design/release.md`, "Two
builds"). A build without the feature refuses `--audio opus`
(`unsupported_feature`, exit status 1) before it opens the input; its help
says so. The crate has no `unsafe` of its own: the FFI surface is the
binding's, and the safe API it uses is `Encoder::new` (48 kHz, mono or
stereo, `Application::Audio`), `set_bitrate`, `set_complexity(10)`,
`set_vbr(true)`, `set_vbr_constraint(false)`, `get_look_ahead` and
`encode_float_to_slice`, one 20 ms frame per call. These are libopus's
defaults for the `audio` application (complexity 10, unconstrained
variable bit rate), set explicitly, and the
settings FFmpeg documents for `-c:a libopus`; the default rates are its
too, 64 kbit/s for mono and 96 kbit/s for stereo.

**Pipeline.** The audio is encoded before the video, so that a stream
the encode does not take is refused before the video encoder starts:

1. `coreav-audio`'s `decode` reads the stream (`--audio opus` takes the
   first audio stream it decodes; in a build with the opt-in `aac`
   feature, AAC is passed over or refused as a build without it does, so
   the stream taken does not depend on the build) through the existing demuxers, applies the container's trimming (an MP4 edit list,
   Matroska's `CodecDelay` and `DiscardPadding`; leading empty edits are
   silence) and resamples to 48 kHz 32-bit float
   (`docs/design/audio-decode.md`, "Resampling and downmixing"). Sample 0
   is presentation time 0. More than two channels, an edit list the
   decoder does not apply exactly, or a source the demuxer stops reading
   early is refused (`unsupported_feature`): the samples would not be the
   stream's presentation.
2. The samples go to the Opus encoder process: the `coreav` program run
   again with the hidden first argument `__coreav_opus_encode`, the
   channels and the bit rate, through the sandbox launcher with the
   encoder program's limits ("Sandbox": no file writes, no core files, an
   address space of 1 GiB, the CPU time to the deadline, its own process
   group, killed at the deadline or on an error). It gets nothing but
   interleaved float samples on its standard input, never the file. The
   last frame is padded with silence, and silent frames follow until the
   packets cover the pre-skip and every sample.
3. Its standard output is untrusted (policy record 0042, point 3):
   `coreav_worker::opus::PacketReader` checks a 6-byte header (`Opus`,
   then the pre-skip, at most 3,840), every record's length (1 to 4,000
   bytes) and every packet's structure against RFC 6716 section 3.4
   (requirements R1 to R7) and its duration (960 samples) before a byte is
   used. While it runs, the packets may not outnumber those the samples
   sent so far make with the largest pre-skip; at the end there must be
   exactly `ceil((pre_skip + samples) / 960)`. The packets go to a spool
   file next to the output, as the video's do, within `max_spool_bytes`.
   The reader has a fuzz target, `coreav-worker/fuzz`'s `opus_packets`.
4. The MP4 writer replaces the source stream with the packets
   ([`remux.md`](remux.md), "One audio stream replaced"): an `Opus`
   sample entry (48 kHz, 16-bit sample size) with a `dOps` box (version 0,
   the output channels, the pre-skip, the source's sample rate as the
   input rate, gain 0, channel mapping family 0), media timescale 48,000,
   every packet a 960-tick sync sample, an edit list that presents the
   decoded samples from movie time 0 and leaves out the pre-skip and the
   padding (the edit's media time is the pre-skip), and a `roll` group
   of distance −4, 80 ms of 20 ms packets. The movie timescale becomes a
   multiple of 48,000 where the edit would round in it and the multiple
   fits 32 bits, so that the edit is exact.

The summary's `encoded_audio` (`docs/schema/transcode-v1.md`) names the
stream, its codec and rate, the channels, the bit rate asked, the samples
presented, the pre-skip and the packets.

**Tests** (policy record 0042, point 6: decoder conformance and a quality
bar, never a hash of the encoded output; libopus's version pinned):
`coreav-worker`'s unit tests of the reader (every requirement, every
record error), `coreav-transcode`'s stand-in encoders (`/bin/sh` scripts
that write a wrong header, an empty or wrong-length packet, a cut record,
nothing, or packets without end: each ends in an error, the last at once),
and `coreav-cli`'s `tests/transcode.rs` with the pinned FFmpeg, on
FLAC in MP4 and Matroska and MP3 in Matroska: the summary validated, two
streams, the Opus stream's `initial_padding` the summary's pre-skip, its
`start_pts` 0 and `duration_ts` the presented samples, a clean decode;
AAC and six channels refused first, and AAC passed over for the FLAC
stream after it, both in a build with `aac` too (`ci.yml`, job
`isobmff-decode`).
`tests/strict_safe.rs` checks that
`--audio opus` runs no `ffmpeg` program in strict-safe mode.

### Opus bars

Fixed before the run, judged by `scripts/opus_encode_check.py` in the
`opus-encode` workflow against the pinned oracle FFmpeg (the build
`scripts/oracle-ffmpeg.env` names, installed by
`scripts/install-oracle-ffmpeg.sh` and checked against the pin by
`scripts/oracle_pin.py --verify-bin`; its libopus is its own, linked
statically), with CoreAV linking the libopus the release links: the
static library `packages/cli/scripts/codecs.sh` builds from the tarball
it pins (`OPUS_VERSION`, `OPUS_SHA256`), the one pin of the release build
and of this workflow. The script reads the version from there and stops
before any run when the CoreAV binary carries another libopus or loads one
at run time, and the report records the libopus version each binary
carries. The oracle FFmpeg's libopus names itself `libopus unknown`, as
libopus does when it is built without a release version, so which
version the FFmpeg arm runs is not known; a version difference can
explain a difference between the arms but excuses no miss. Only a manual run of the workflow judges the bars; a run for a
pull request or a push builds CoreAV, checks its libopus and the oracle,
and judges nothing. Each input runs two arms on the same source:

    coreav transcode SRC OUT.mp4 --audio opus [--audio-bitrate K] --encoder ffmpeg --ffmpeg FFMPEG
    FFMPEG -i SRC -c:v libx264 -c:a libopus [-b:a Kk] OUT.mp4

The set: six sources that the oracle FFmpeg makes from its `lavfi`
sources, 10 s each with 160x120 H.264 video, each at three rates (the
default, 32 and 128 kbit/s), 18 inputs:

| Source | Audio | Container |
|---|---|---|
| S1 | 440 Hz sine, mono, 48 kHz, FLAC | MP4 |
| S2 | 440 Hz left and 660 Hz right, 48 kHz, FLAC | MP4 |
| S3 | logarithmic sweep 50 Hz to 15 kHz, stereo, 48 kHz, FLAC | Matroska |
| S4 | pink noise (seed 1), stereo, 48 kHz, FLAC | MP4 |
| S5 | noise bursts at a syllable rate (4 Hz), mono, 48 kHz, MP3 at 192 kbit/s | Matroska |
| S6 | 1 kHz sine, stereo, 44.1 kHz, FLAC | MP4 |

`ci.yml`'s `check` job makes the six sources with the oracle FFmpeg, a
second each, on every run (`scripts/tests/test_bar_sources.py`), so a
source it cannot make fails there and not in a judged run.

1. **O1, decodes.** The oracle FFmpeg decodes the audio of every CoreAV
   output (`-v error -map 0:a -f null -`) with exit status 0 and no message:
   18 of 18.
2. **O2, the timeline.** For every CoreAV output, the oracle `ffprobe`
   gives the Opus stream `start_pts` 0, `initial_padding` equal to the
   summary's `pre_skip`, and `duration_ts` equal to the summary's
   `samples` and to the FFmpeg arm's `duration_ts`, within 48 samples
   (1 ms; the two resamplers may end a 44.1 kHz source a sample apart):
   18 of 18.
3. **O3, the bit rate.** The ratio of the Opus stream's `bit_rate`
   (`ffprobe`) to the FFmpeg arm's is within 0.85 to 1.15 on every input,
   and its median over the 18 within 0.95 to 1.05. The two libopus builds
   may differ in version, so the bar allows the drift of variable bit rate
   between them, not a different rate control.
4. **O4, quality.** On the tonal sources at 48 kHz (S1 to S3, 9 inputs),
   the SNR against the source, every output decoded by the oracle FFmpeg to
   48 kHz float (the source too) and compared over the presented samples,
   of CoreAV's output is at least the FFmpeg arm's minus 1.0 dB, per input.
   A waveform SNR does not measure a perceptual codec on noise (S4, S5),
   and on S6 the reference would be FFmpeg's own resampler, so those are
   judged on O1 to O3 only.

Speed is reported for information only, from the same shared runner (the
wall time of each arm's whole command, median of three): GitHub-hosted
runners vary in CPU model between jobs, so no claim is made from it.

## AAC audio

`coreav transcode --audio aac [--audio-bitrate KBPS]` encodes one audio
stream to AAC-LC with FFmpeg's built-in `aac` encoder, run as an
operator-provided program under policy record 0042, point 5, as
`ffmpeg -c:v libx264` is: nothing is linked.

The program is the `ffmpeg` that `--ffmpeg` names, else the file
`COREAV_FFMPEG` names, else `ffmpeg` on `PATH` ("The encoder program"),
whatever program encodes the video. None found is refused
(`unsupported_feature`, "no ffmpeg program for AAC audio"), and so is one
whose answer to `-h encoder=aac`, run as the encoder runs, does not
begin `Encoder aac [` ("has no aac encoder"), both before the audio is
decoded. Strict-safe mode refuses `--audio aac` before the input is
read, since the encoder is the `ffmpeg` program. The bit rate is
`--audio-bitrate`, 16 to 512 kbit/s, given to the encoder as `-b:a`;
without it the encoder's own default applies, which the summary gives as
`null` rather than restating it.

**Pipeline.** As for Opus, the audio is encoded before the video:

1. `coreav-audio`'s `decode` reads the stream as it does for Opus (the
   first audio stream it decodes, FLAC or MP3, with AAC passed over or
   refused in every build, and the container's trimming applied) to
   32-bit float at the stream's own rate: nothing is resampled. More than
   two channels, and a rate other than the AAC rates of 8 to 64 kHz
   (ISO/IEC 14496-3 Table 1.18; an `mp4a` sample entry carries at most
   65,535 Hz), are refused (`unsupported_feature`), as are the edit lists
   and early stops that Opus refuses.
2. The samples go to `ffmpeg -nostdin -hide_banner -loglevel error -f
   f32le -ar RATE -ac CHANNELS -i pipe:0 -map 0:a:0 -c:a aac [-b:a BITS]
   -f matroska pipe:1`, through the sandbox launcher with the encoder
   program's limits ("Sandbox": no file writes, no core files, an address
   space of 2 GiB, the CPU time to the deadline, its own process group,
   killed at the deadline or on an error). It gets the samples on its
   standard input, never the file. Of the containers FFmpeg writes to a
   pipe, Matroska is the one that keeps the encoder's delay
   (`CodecDelay`): ADTS has no field for it, and FFmpeg's fragmented MP4
   leaves the edit list out.
3. Its standard output is untrusted (policy record 0042, point 3). As it
   comes, it goes to a spool file next to the output, within
   `max_spool_bytes` and within what the samples sent so far make (the
   frames that cover the largest delay and the samples, and one more, each
   of 8,184 bytes and 64 bytes of Matroska, and 64 KiB of header):
   an encoder that writes more is stopped at once. Once it has exited,
   `coreav_transcode::aac::read_output` reads the file with CoreAV's
   Matroska parser and takes only one audio track of `A_AAC`, stored as it
   is, at the rate and channels sent, whose `CodecPrivate` is the
   `AudioSpecificConfig` of AAC-LC at that rate and channels (object type
   2, 1,024-sample frames), alone or followed by the explicit signalling
   that the stream has no SBR (ISO/IEC 14496-3 1.6.2.1: `syncExtensionType`
   0x2B7, `sbrPresentFlag` 0), as FFmpeg writes it; a delay of at most two
   frames; frames of 1 to 8,184 bytes, the largest that ADTS carries (the
   decoder's buffer of 6,144 bits a channel bounds the average, and
   FFmpeg's encoder writes larger mono frames at 192 kbit/s); and as many
   frames as cover
   the delay and the samples, or one more. The frames are copied to a
   second spool file. The reader has a fuzz target,
   `coreav-transcode/fuzz`'s `aac_output`.
4. The MP4 writer replaces the source stream with the frames
   ([`remux.md`](remux.md), "One audio stream replaced"): an `mp4a` sample
   entry (the stream's rate, 16-bit sample size) with an `esds` box
   (ISO/IEC 14496-1 7.2.6.5: object type 0x40, an audio stream, the
   `AudioSpecificConfig` as read), media timescale the sample rate, every
   frame a 1,024-tick sync sample, an edit list that presents the decoded
   samples from movie time 0 and leaves out the delay and the padding (the
   edit's media time is the delay), and a `roll` group of distance −1,
   since a frame overlaps the one before it (ISO/IEC 14496-3 4.6.11).

The summary's `encoded_audio` (`docs/schema/transcode-v1.md`) names the
stream, its codec and rate, the encoder `aac`, the channels, the bit
rate asked, the samples presented, the delay (`pre_skip`) and the frames
(`packets`).

**Tests** (policy record 0042, point 6: decoder conformance and a quality
bar, never a hash of the encoded output): `coreav-transcode`'s unit tests
of the reader (every refusal) and of stand-in `ffmpeg` programs (`/bin/sh`
scripts without the `aac` encoder, failing, or writing without end, the
last stopped at once); `coreav-isobmff-write`'s `tests/replace.rs` with
an `mp4a` replacement in MP4 and Matroska sources; and `coreav-cli`'s
`tests/transcode.rs` with the pinned FFmpeg as the encoder, on FLAC in MP4
and Matroska and MP3 in Matroska at 44.1, 48 and 32 kHz: the summary
validated, two streams, the AAC stream LC at the source's rate,
`start_pts` 0 and `duration_ts` the presented samples, a clean decode;
AAC, six channels, 96 kHz, no `ffmpeg` and an `ffmpeg` without `aac`
refused first, AAC also in a build with `aac` (`fixtures.yml`).
`tests/strict_safe.rs` checks that `--audio aac` is refused before the
input is read.

### AAC bars

Fixed before the run, judged by `scripts/aac_encode_check.py` in the
`aac-encode` workflow against FFmpeg 9.0.2 (the oracle FFmpeg that
`scripts/install-oracle-ffmpeg.sh` installs, the build the Opus bars name).
The same FFmpeg is both arms' encoder: CoreAV runs it as its AAC encoder
program. While the oracle is another FFmpeg version, the workflow runs no
bar. Each input runs two arms on the same source:

    coreav transcode SRC OUT.mp4 --audio aac [--audio-bitrate K] --encoder ffmpeg --ffmpeg FFMPEG9
    FFMPEG9 -i SRC -c:v libx264 -c:a aac [-b:a Kk] OUT.mp4

The set: the six sources of the Opus bars (S1 to S6, made the same way),
each at three rates (the encoder's default, 64 and 192 kbit/s), 18 inputs.

1. **A1, decodes.** FFmpeg 9.0.2 decodes the audio of every CoreAV output
   (`-v error -map 0:a -f null -`) with exit status 0 and no message:
   18 of 18.
2. **A2, the timeline.** For every CoreAV output, `ffprobe` 9.0.2 gives
   the audio stream `codec_name` `aac`, `profile` `LC`, the source's
   `sample_rate`, `start_pts` 0, and `duration_ts` equal to the
   summary's `samples` and to the FFmpeg arm's `duration_ts`, exactly:
   neither arm resamples. 18 of 18.
3. **A3, the bit rate.** The ratio of the AAC stream's `bit_rate`
   (`ffprobe`) to the FFmpeg arm's is within 0.95 to 1.05 on every input,
   and its median over the 18 within 0.98 to 1.02. Both arms run the same
   encoder with the same settings; only the samples' way to it differs
   (CoreAV's decoder and a pipe, against FFmpeg's decoder).
4. **A4, quality.** On the tonal sources (S1, S2, S3 and S6, 12 inputs),
   the SNR against the source, every output decoded by FFmpeg 9.0.2 to
   48 kHz float (the source too) and compared over the presented samples,
   of CoreAV's output is at least the FFmpeg arm's minus 0.5 dB, per
   input. Noise (S4, S5) is judged on A1 to A3 only, as for Opus.

Speed is reported for information only, from the same shared runner (the
wall time of each arm's whole command, median of three), as for Opus.

## Through `coreav ffmpeg`

`coreav ffmpeg` ([`compat-cli.md`](compat-cli.md)) runs the common
`ffmpeg` command lines of this job through this pipeline: one input, the
video re-encoded with libx264, the audio copied or dropped, one MP4
output. The command line is read by the compatibility parser and its
stream selection, the same as for `-c copy`; a command that selects a
video stream for `libx264` is routed here before the refusal of encoder
values, and every option this section does not list is still refused,
naming it (exit status 4). The encoder is `--encoder auto`'s ("The encoder
program"); in strict-safe mode that is an `x264` program only, and a
command without one, or with a video only the FFmpeg decoder would decode,
is refused (exit status 4) and never handed to FFmpeg.

| Option | Taken as |
|---|---|
| `-c:v`, `-vcodec` (and `-c`/`-codec` with a specifier naming the video) `libx264` | the encoder; the selected video stream must be the one this pipeline encodes (FFmpeg's default choice, "Scope"), and only one |
| `-c:a`, `-acodec` `copy`, or `-c copy` for the other streams | the selected audio streams copied (`AudioMode::Streams`) |
| `-an` | no audio |
| `-crf Q` | `--crf`, 0 to 51, fractions as given |
| `-b:v RATE` | `--bitrate`: FFmpeg's number syntax (`k`, `M`, a fraction), in bits a second, divided by 1,000 with the remainder dropped as libx264 receives it (`1M` 1,000, `1234567` 1,234 kbit/s, checked on the pinned FFmpeg as a black box); a rate below 1,000 is refused |
| `-preset P` | `--preset` |
| `-threads N` after the input | `--threads`; `0` is the encoder's own choice |
| `-vf`, `-filter:v` `scale=W:H` | `--scale W:H`, the forms it takes; any other filter or filter chain is refused |
| `-s WxH` after the input | `--scale W:H` |
| `-pix_fmt yuv420p` | the encoder's input must be 8-bit 4:2:0 (`TranscodeOptions::pixel_format`); frames are not converted, so a source decoded to another format is refused before the encoder starts (FFmpeg converts it) |
| `-movflags +faststart`, `faststart` | nothing: the movie box is always first |
| `-f mp4`, an output named `.mp4` | the output container |
| `-noautorotate` before the input | rotation kept as metadata, which this pipeline always does |
| `-fps_mode passthrough`, `-vsync passthrough`, `-vsync 0` | the frames' own times, which this pipeline always keeps |

FFmpeg's defaults, where the command leaves them, are this pipeline's:
CRF 23 and `medium`. Where FFmpeg's behaviour differs from this
pipeline's, the command is refused where the difference would change the
output, before the encoder starts:

- **Autorotation.** Without `-noautorotate`, FFmpeg applies a track's matrix
  to the frames and writes the identity; this pipeline keeps the matrix
  and the frames as they are. A source whose MP4 track matrix is not the
  identity is refused without `-noautorotate`, whatever the matrix (a quarter
  turn, −45°, a mirror). With `-noautorotate` both write the source's matrix
  unchanged (checked on the pinned FFmpeg as a black box at −45°, 90° and
  180°, also with `-s`), except a matrix that translates or projects,
  which is refused either way, as is a Matroska track with a projection
  or a pose other than zero.
- **Constant rate.** Without `-fps_mode passthrough`, FFmpeg writes MP4 at
  a constant rate and duplicates or drops frames to keep it (`vfr.mp4`:
  33 frames in, 50 out). A source whose presented frames are not evenly
  spaced (one interval above zero between every two, in the track's time
  base, so a repeated time is not), or
  whose video does not start at time 0 (an empty edit, a late first
  frame: FFmpeg repeats the first frame to fill the start, `edit_list.mp4`
  50 frames in, 51 out), is refused without it; otherwise the two give the same
  frames (B9's Q2 pairs every frame with FFmpeg's output of the same
  command, and fails when the counts differ).
- **Times in decode order.** A source whose frames are reordered
  (`has_b_frames` above 0) but whose container gives no composition
  offsets: FFmpeg derives other times for its frames ("Times in decode
  order" above, `size_inband_sps.mp4`). Refused without
  `-fps_mode passthrough`.
- **Left crops.** By default the pinned FFmpeg decodes an H.264 left crop
  that is not aligned uncropped (`size_left_crop.mp4`, the B4 baseline
  above), where CoreAV's native decoder crops as specified. An H.264
  picture cropped at its left or top edge (its sequence parameter set in
  the stored configuration) is refused.
- **Size changes.** Where the frame size changes within the stream,
  FFmpeg resizes the later frames to the first size with its own scaler,
  and this pipeline with its own (`size_vp9_resize.mp4` at `-crf 0`:
  0.77 dB below FFmpeg against the source). `TranscodeOptions::one_size`
  refuses such a stream when the first other size comes, before anything
  is written.
- **Chapters and timecodes.** FFmpeg writes a source's chapters as a text
  track and its timecode as a `tmcd` track; this pipeline writes neither.
  A source with chapters, or a stream with a `timecode` tag, is refused.
- **Stream order.** FFmpeg writes the selected video first, then the
  audio; this pipeline keeps the source's order. A source whose selected
  audio comes before its video is refused.
- **Audio.** Without `-c:a copy` or `-an`, FFmpeg re-encodes the
  selected audio to AAC; audio encoding is not in this pipeline, so a
  command that selects an audio stream without copying it is refused.
  Copied audio the MP4 writer does not take (PCM; anything but AAC or MP3
  in an `mp4a` sample entry and Opus in an `Opus` one from MP4/MOV, AAC
  and Opus from Matroska) is refused before the encode, where the writer
  would fail after it.

A refusal found once the input is read (the four above, a pixel format,
an edit list, anything this pipeline refuses as `Unsupported`, a video
no decoder of the build takes or allows, such as HEVC in a build with the
native H.264 decoder alone) goes
through the same refusal point as an option, so a script sees exit
status 4, and nothing is written.

## Crates

| Crate | What it gets | `unsafe` |
|---|---|---|
| `coreav-transcode` (new) | Options, the pipeline, the encoder process, the Annex B reader, the AAC encoder's output reader | forbidden |
| `coreav-isobmff-write` | Remux with one video stream, one audio stream, or one of each replaced | forbidden |
| `coreav-backend-libopus` (new) | The libopus encoder, behind the `libopus` feature | forbidden: the `opusic-c` crate's `unsafe` is a dependency's |
| `coreav-audio` | Used as it is: the audio decoded to float, at 48 kHz for the Opus encoder and at its own rate for the AAC encoder | forbidden |
| `coreav-thumbnail` | The video stream's packets as a stream, for a whole file | forbidden |
| `coreav-worker` | The hidden sandbox entry that sets limits and `exec`s; the Opus encoder process and its reader (`opus`, the `libopus` feature) | crate `deny`, these modules `forbid` |
| `coreav-cli` | `coreav transcode`, in builds with a decoder backend (the `transcode` feature, which `ffmpeg-fallback`, `native-h264`, `dav1d` and `libvpx` turn on, and which brings in `coreav-worker` for the sandbox launcher, so a `native-h264`-only build has it too) | forbidden |

x264 and FFmpeg are programs: nothing GPL is linked or bundled, and the
operator installs the video encoder and the AAC encoder. libopus is the one library an
encoder crate links, opt-in (policy record 0042).

## Rejected alternatives

- **Linking libx264.** GPL: CoreAV never bundles it, and FFmpeg's
  libraries are never linked.
- **An encoder crate in this item** (OpenH264, NVENC, or an AV1 encoder
  for AV1 output). Each needs a recorded encoder policy
  first; OpenH264 adds a patent question of its own.
- **libopus in the `coreav` process.** Policy record 0042 runs software
  encoders sandboxed by default: the samples come from untrusted files,
  and a crash or a runaway allocation in the encoder would take the
  transcode with it. The process costs one copy of the samples through a
  pipe, small beside the video's frames.
- **The `opus` crate.** Its 0.3 binding, `audiopus_sys`, is unmaintained
  (RUSTSEC-2026-0150), and 0.4 always builds the libopus that
  `opusic-sys` bundles. `opusic-c` without its default features links the
  system's libopus, as the dav1d and libvpx backends link theirs, and its
  safe API sets the complexity too, which the bars compare at FFmpeg's
  documented default of 10.
- **Downmixing more than two channels to stereo.** FFmpeg's `-c:a
  libopus` writes such a stream as surround Opus (channel mapping family
  1), so a downmix would be another output than the command's; it is
  refused instead, and family 1 is a follow-up.
- **Encoder output in a container** (x264's `--muxer mkv` or `flv`,
  `ffmpeg -f matroska`) for its timestamps. The two programs would need
  different readers, FLV keeps milliseconds only, and the timestamps the
  encoder knows are the constant-rate ones it was given; the source's are
  better, and Annex B is the one format both programs write.
- **A constant rate for the encoder**, the source's average snapped to a
  common rate, as this note first had it: simpler (raw video to both
  programs, no timecode file), and the output's timestamps are the
  source's either way. Measured by the harness on `vfr.mp4`: ΔPSNR −0.98
  dB and a size ratio of 0.88 against FFmpeg's baseline (which hands
  libx264 the real times), failing Q1; and a told rate below the closest
  two frames' made `ffmpeg -f null` report colliding frames (B1), which
  needed a rate rule of its own. With the frames' own times both programs
  encode as the baseline does.
- **Other carriers of the times for FFmpeg.** NUT carries raw video with
  timestamps too, but needs more of a format (frame codes, checksums,
  syncpoints) for no gain; `setpts` expressions or a concat list of
  per-frame durations grow with the stream and are fragile; Y4M has a
  constant rate.
- **Y4M instead of raw video.** Self-describing, but fixed to one size and
  rate for the stream just as raw video is, and the size is known anyway.
- **Shared memory to the encoder.** Both programs read pipes or files.
- **Decoding and encoding in one FFmpeg** (the fallback's FFmpeg also
  running libx264). Fewer copies for the codecs the fallback decodes, but
  a second pipeline to verify, frames that never cross the decode
  boundary (its limits and checks), and no CoreAV frame operations. It can
  be measured later against the same bars.
- **The encoder inside the decode worker.** A compromised decoder would
  reach the encoder's output; and the native and dav1d paths decode in
  this process anyway.
- **Writing `mdat` first and the movie box last** (FFmpeg's default
  without `+faststart`): no spool file, but the output would not start
  playing before it is fully read, which web delivery needs.

## Limits

The decode limits of `docs/design/decode.md` apply to the decoder, and the
container limits to the demuxers and the MP4 writer, with one
change: `max_decoded_bytes` (4 GiB of frames a decoder outputs, the work
of one thumbnail request) is raised, where that is larger, to the stream's
packets times a frame of its declared size at 6 bytes a pixel (4:4:4 at two
bytes a sample, the widest a decoder outputs; four times an 8-bit 4:2:0
frame, room for a stream that grows within it), or of `max_frame_bytes`
where the container declares no size. A transcode decodes the whole stream
and each packet decodes to at most one frame, so the decoded bytes of a
request stay bounded by its input, as the limits policy asks; at 4 GiB, any 1080p
video longer than about 55 s failed (46 of the corpus files of the first
correctness run, with every backend). The configured value stays the
floor, so a caller cannot set a lower cap for a transcode than for a
thumbnail; a stream that grows past four times its declared area (a
recording that starts small and ramps up) can still reach the cap once
past the configured 4 GiB, and a stream that declares a huge size gets
room for frames of that size. Rejected: no cap, with `--timeout` as the
only bound, which the limits policy rejects; and packets times `max_frame_bytes`
(512 MiB), which never trips. Added:

| Limit | Default | Why |
|---|---|---|
| Output size | 16,384 × 16,384 | The frame model's limit; x264's own levels are lower. x264 encodes up to it (16,384 × 1,024 checked; 16,400 wide refused), within the sandbox's address-space budget. |
| Frame to FFmpeg | 2^28 − 32 bytes | FFmpeg's Matroska reader refuses an element over 2^28 bytes (a black box: a frame 32,772 bytes over was refused, one 16,380 under read), and each frame is one block: a larger output (16,384 × 5,462 at 4:4:4, 8 bits) is refused before the encoder starts with `--encoder ffmpeg`. |
| Encoder access unit | `max_packet_bytes` (128 MiB) | The container limit; a coded 8K frame is far below. |
| NAL unit | 128 MiB | As an access unit. |
| NAL units per access unit | 1,024 | x264 writes one slice a frame by default; a few headers. |
| Pictures held in one open period | 65,536 | A period's pictures wait until the next IDR picture (or the end) fixes their order; x264 writes one at least every `--keyint` (250) frames. |
| Bytes held in one open period | 1 GiB | The same, in sample bytes. The period is held in memory: an open-GOP stream (x264's `--open-gop`, which CoreAV does not pass) is one period, so its whole encoded video must fit these two. |
| Parameter set | 65,535 bytes | The `avcC` length field. The 32 + 256 stored sets are outside the access unit and period caps; this bounds them at 18 MiB. |
| Parameter sets | 32 SPS, 256 PPS | The id ranges of H.264. |
| Pictures from the encoder | frames sent | More is a protocol error. |
| Spool file | 64 GiB | Far above any CRF 23 output of a file within the decode limits. |
| Encoder messages kept | 64 KiB of standard error | For the error message. |
| `--timeout` | 3,600 s | Wall clock for the whole transcode; the decoder's `Limits::timeout` is set to it. |
| Encoder address space | `max_memory_bytes` (8 GiB), or more for large frames: the bytes of an output frame times 128 + 16 per encoder thread (x264's default is 1.5 a CPU), plus 1 GiB; at most 4 × `max_memory_bytes` and half the memory the process may use (`MemTotal`, or a container's cgroup limit at `/sys/fs/cgroup` where lower; 2 × `max_memory_bytes` where neither is known), never below `max_memory_bytes`, so in a container of less than twice that the encoder still gets `max_memory_bytes` | x264 holds a frame per frame thread and lookahead position, and its references: 8K 8-bit at `medium` on 8 CPUs reaches about 8.1 GB resident (measured with the pinned x264), so 8 GiB failed 8K sources in `malloc`. The caps keep a hostile stream of large frames (the hostile suite declares 16,384 × 8,192) from taking the host's memory: on a 16 GB host the budget stays 8 GiB. The thread count is `available_parallelism`, which follows a container's CPU quota where x264 counts the host's CPUs, so in such a container the budget can be lower than x264 needs; `--threads` sets both. |

**Pacing.** The decode boundary's `send_packet` never blocks, and the
fallback's packets wait in the worker up to `max_pending_bytes` (1 GiB)
before the decoder fails with `Limit` (`docs/design/decode.md`, "Threads
and backpressure"); a transcode sends every packet of the stream, faster
than FFmpeg decodes a high-bit-rate one. So it keeps the presented packets
sent whose frames have not come (each frame received retires the oldest),
and past 64 of them and a quarter of `max_pending_bytes`, it waits for a
frame (polling every millisecond) before it sends the next packet. Only
frames of presented packets retire one. After 5 s without any frame,
nothing that will come is in flight (the decoder dropped those frames, or
holds them until it has more), so every packet counted is retired and the
transcode goes on: a decoder that holds frames back, or drops corrupt
ones, cannot stall it, while a slow one (8K ProRes at a few frames a
second) is waited for. The wait needs the worker to hand on frames while
no packet comes (`docs/design/decode.md`, "Threads and backpressure"); in
round 2 it did not, so each wait ended only at the 5 s stall, which
retired every packet counted while FFmpeg had not read them, and a
5472x3078 HEVC stream at 155 Mb/s passed the worker's limit after a few
such waits. Packets the source does not present are not
counted: the sandboxed decoder returns no frame for them, so nothing would
retire them; a
trimmed prefix of more than `max_pending_bytes` sent faster than the
fallback decodes it still fails with `Limit`. Rejected: blocking in the
trait, which `docs/design/decode.md` leaves to frame extraction; and a
fixed packet count, which does not bound bytes.

## Bars

Fixed before any measured run, and judged on the commit that adds the
measurement. Inputs: the fixtures (`scripts/gen-fixtures.sh`,
`scripts/gen-fixtures-mkv.sh` and the decode fixtures), and corpus v1
`real` files in MP4, MOV, Matroska or WebM whose video stream's codec is on
the default decode allowlist. Classes are that codec: H.264, HEVC, AV1,
VP9, VP8, ProRes. A file CoreAV refuses (a pixel format outside the
accepted ones, an edit list outside the simple model, a refusal of the
demuxer: a `coreav.error/1` of kind `unsupported_feature`,
`unsupported_container`, `truncated_structure` or `invalid_structure`)
is counted apart by reason, never dropped silently; any other failure
counts against the bars. Only the input side (the demuxers, the decoder
and the format and edit-list checks before the encoder starts) reports
those kinds: errors of the encoder program, the Annex B reader and the MP4
writer are reported with the kind of a failed backend (`decode` in
`coreav.error/1`, with a message naming the encoding side), never with
those, so a defect there cannot pass as a refusal.
Refusals of files whose baseline succeeds are listed by file. The corpus
shares of B2 and B3 are judged over all classes together and reported
per class, and every class must also reach **95%**. The A/V offset of B2 is
taken against the first audio stream written. Where `coreav remux IN`
cannot write the source (a video codec MP4 cannot carry), B3's
timestamps are compared with `coreav remux --streams <audio> IN`, and
where that refuses an audio codec MP4 does not carry, with each audio
stream remuxed alone: the streams it writes are the ones B3 expects, and a
file where it writes none is not one with audio.

Arms: CoreAV with `--encoder ffmpeg` (the pinned FFmpeg 7.0.2) and with
`--encoder x264` (an x264 program built from 4613ac3, the commit of that
FFmpeg's libx264, without its own demuxers or muxers; the binary's
SHA-256 is recorded with the results). Both are judged. The baseline is
the pinned FFmpeg:

    ffmpeg -nostdin -noautorotate -flags +unaligned -i IN -map 0:<video> [-map 0:<audio>]... \
           -c:v libx264 -preset P -crf Q -c:a copy -fps_mode passthrough OUT.mp4

with the `-map` options naming the streams CoreAV's summary lists as
written. Section
8.8's baseline is the fastest `ffmpeg` command whose output passes the
same oracle. `-fps_mode passthrough` is needed for B2: without it, FFmpeg
writes MP4 at a constant rate and duplicates frames (the `vfr.mp4` fixture:
33 frames in, 50 out). `-noautorotate` keeps the matrix instead of turning
every frame (checked on `rotate_ccw90.mp4`), which is the cheaper output
and the one B4 compares. `-flags +unaligned` makes the pinned FFmpeg apply an H.264 left crop
exactly (ITU-T H.264 7.4.2.1.1): by default FFmpeg 7.0.2 ignores a left
crop that is not aligned and outputs the uncropped picture (the
`size_left_crop.mp4` fixture: 1280 columns instead of columns 48 to 1279),
where CoreAV's native decoder crops as specified. Every FFmpeg decode of a
source in these bars (the baseline, the Q1 reference, the PSNR inputs)
takes it; on every other fixture it changes no decoded frame. B9's
FFmpeg command does not: it is the command line a user runs, and
`coreav ffmpeg` refuses an H.264 picture cropped at its left or top edge.

The baseline is timed on every file, though its own output does not pass
B2 or B3 on every file (FFmpeg moves the audio against the video on some edit
lists and fragmented files, by up to 20 ms on the fixtures); those files
are counted and reported, and no other FFmpeg command is searched for:
choosing a command per file would make the baseline depend on the results,
and the commands that could pass those files differ from this one only in
how they mux, so their time is about the same. A
file whose baseline fails to run is counted apart for B4 and B5 only.
The command most often used
(`ffmpeg -i IN -c:v libx264 -crf Q -preset P -c:a copy OUT.mp4`) is timed
beside it for information, never for a claim.

Correctness runs use `--preset veryfast --crf 23` (B-frames and
B-pyramid, as `medium`), on the fixtures and on corpus files whose video
is at most 120 s long, taken per class in the order of
`sha256("coreav-transcode-v1:" + sha256)`: at most 200 files a class, and
a file is skipped (counted apart as "over budget") when it would take the
class past 1.2 × 10^11 pixel-frames (width × height × the video
stream's packets, from `ffprobe` before any run; packets stand for frames
so that selection needs no decoding). A second configuration, `--scale -2:360`,
runs on the first 100 selected files, over all classes in the order of
that hash, whose video is taller than 360 rows, against
FFmpeg's `-vf scale=-2:360`. The correctness runs may run on any host;
they are not timed.

Size of the runs, estimated before them: at `veryfast` an 8-vCPU host
encodes roughly 3 × 10^8 pixels a second and decodes several times that,
so one pixel-frame costs about 2 × 10^-8 s across the three encodes and
the PSNR passes: at most about 40 minutes a class for correctness. For
speed, `medium` encodes about 10^8 pixels a second, and each file is
encoded ten times (three commands, three rounds each, and the
information command once).

1. **B1, the output decodes cleanly.** `ffprobe -v error -select_streams
   v:0 -show_frames OUT`, which decodes every video frame and reports the
   decoder's errors without a muxer in between, exits 0 and prints nothing
   on standard error, and `ffprobe` reads every stream: 100% of outputs. (The copied audio is judged by B3; its own
   defects are the source's. A cover image in the movie metadata, copied
   as `coreav remux` copies it (`docs/design/remux.md`),
   which `ffprobe` lists as a video stream with the attached-picture
   disposition, has to be read but is not a written track.) Where the build has the native decoder, it
   also decodes the output's video, and its frames equal FFmpeg's
   `framemd5` of it (decoder conformance): 100% of the
   outputs within that decoder's scope (`docs/design/h264.md`: 8-bit
   4:2:0); the others are counted apart by profile.
2. **B2, frames and timestamps exact.** The output's video frames
   (`ffprobe -show_frames`, presentation order, `best_effort_timestamp`
   as an exact rational) equal the source's presented frames of the same
   stream in count, and in time relative to the first frame within half a
   tick of the source's time base (exact when the output keeps it), a
   source time that repeats the one before expected one tick after the
   frame before while that is below the next source time, and not
   expected otherwise ("Which frames"; for the rounds after round 2);
   the
   first video frame's time minus the first audio packet's equals the
   source's within 1 ms. Fixtures 100%; corpus at least 99% of files.
3. **B3, audio copied exactly.** For every audio stream written, its
   packets (`ffprobe -show_packets -show_data_hash MD5`) equal the source
   stream's in count and data, and the same stream of `coreav remux` of
   the same input in timestamps and durations. Fixtures 100%; corpus at
   least 99% of files with audio.
4. **B4, quality.** PSNR is the `average` of FFmpeg's `psnr` filter, over
   all planes, pairing the `n`-th presented frame of each input: both
   inputs read with `-noautorotate` and `-reinit_filter 0`, each through
   `scale=W:H` at the output's size (so a source that changes size reaches
   `psnr` at one geometry; on CoreAV's and the baseline's outputs it does
   nothing) and then `settb=1,setpts=N` before `psnr` (the filter pairs
   frames by timestamp, and `setpts=N` alone pairs wrongly when the two
   time bases differ). The comparison fails when the frame counts differ,
   and a comparison that crashes or reports no value fails that file's
   B4.
   - **Q1, against the source.** The reference is the source decoded by
     FFmpeg with `-noautorotate`, through the `scale=W:H` above (which
     is FFmpeg's `scale` filter to the output size in the scaled
     configuration). Per file, `ΔPSNR` is CoreAV's PSNR minus the
     baseline's, both against that reference. Per class and arm: median
     `ΔPSNR` at least **−0.10 dB**, and at least **98% of files** at
     least **−0.50 dB**; and the median of the video streams' size ratio
     (CoreAV / baseline) between **0.98 and 1.02**, so neither side buys
     quality with bits.
   - **Q2, against FFmpeg's output.** CoreAV's output against the
     baseline's: per class and arm, median at least **35 dB** and at least
     **98% of files** at least **30 dB**. The same encoder with the same
     settings on the same frames differs only through rate-control inputs;
     a wrong frame, order, colour range or shifted line costs far more.
   - Fixtures: Q1 and Q2 for every fixture; `--bitrate 1M` on the
     fixtures of at least 5 s has the video bit rate within 10% of the
     baseline's with `-b:v 1M`.
5. **B5, speed.** Held-out files (`holdout: true`) of the
   classes above, video 1 to 120 s long, taken per class in the order of
   `sha256("coreav-transcode-speed-v1:" + sha256)`: at most 25 files a
   class, and a file is skipped (counted apart as "over budget") when it
   would take the class past 4.8 × 10^10 pixel-frames, about 80 minutes a
   class at the estimate above, so the whole run stays near 8 hours of a
   `c4-standard-8` (under 5 USD at the script's 0.60 USD an hour bound).
   Refused files are counted apart. When the budget leaves a class with
   fewer than 5 timed files, its remaining eligible files are taken in
   ascending order of pixel-frames, past the budget, until 5 are timed or
   none is left (at most about 2 more hours over all classes, so the run
   stays under 10 hours and 6 USD). Every class with at least one timed
   file is judged, and its file count and interval are reported. `--preset medium --crf 23`, no scaling. Each command is
   a process timed from outside, the source in the page cache and the
   output on tmpfs; 3 rounds a file in alternating order, a file's time the
   median of its rounds, its ratio CoreAV / baseline. Bar: the median of
   the per-file ratios at most **1.05**, overall and in every class, for
   each arm, with a bootstrap 95% confidence interval (10,000 resamples).
   Backends: a build with `native-h264`, `dav1d` and `ffmpeg-fallback`,
   under the default policy; the results say which ran per class. Host: a
   Compute Engine `c4-standard-8` (`scripts/gcp_vm_job.py`), with a gate
   file's baseline time before and after the run; the run is void when
   they differ by more than 5% (the gate command is the baseline
   without audio, `-an`: it measures the host, not the job).
   - **Why above 1.00.** The encoder is the same libx264 with the same
     settings, and its time is most of the job; the roadmap says "with the
     same encoder and settings, encoding itself is no faster than
     FFmpeg's". What CoreAV adds is set by policy: the encoder must run as
     a separate program (GPL), and third-party decoders in a sandboxed
     worker, so each frame crosses a process boundary on its way in and
     out (a copy from the worker's shared memory and a pipe to the
     encoder), and each run starts two or three processes where FFmpeg
     starts one. FFmpeg passes frames by reference in one process. 1.05
     bounds that cost at 5% of the median file's time. A "faster" claim
     still needs the upper bound of its interval below 1.00, and none is
     expected from this item: the roadmap expects CoreAV's transcoding
     wins from encoding fewer frames (smart rendering, the
     copy-when-possible router and ladders).
6. **B6, hostile input.** Every file of the hostile-input suite
   (`tools/hostile/generate.py`) through `coreav transcode`:
   exit 0, or 1 with a `coreav.error/1` object; no crash, signal, hang
   past the timeout, leftover spool file or out-of-memory. A test encoder
   that writes garbage, more or fewer pictures than frames, an oversized
   unit, changing parameter sets, nothing, or that exits early or hangs:
   every case ends with an error, never a panic or a hang past the
   deadline.
7. **B7, fuzz targets**: the Annex B reader (units, access
   units, parameter sets, slice headers, picture order counts, `avcC`)
   and the sample timing from presentation indices
   (`coreav-transcode/fuzz`), the replaced-stream remux with arbitrary
   samples (`coreav-isobmff-write/fuzz`), and the `--scale` parser. The
   input files are parsed only by the existing demuxers and decoders,
   which keep their own targets.
8. **B8, regression guard.** `coreav transcode` joins the CI guard
   (`docs/design/benchmarks.md`) on the fallback build, as `thumbnail`
   does: instructions of the `coreav` process (demux, frames from the
   worker, writes to the encoder, the Annex B reader, the MP4 writer) on
   the guard's H.264 fixture with `--encoder ffmpeg --preset ultrafast`,
   under the guard's tolerance, with `--scale` to a smaller size so the
   resize (`Picture::resize_with`) is counted. The worker, the decoder's
   FFmpeg and the encoder are other processes and not counted; their time
   is in B5. A CI job also runs B1 to B4 on the fixtures
   (`scripts/transcode_check.py`).
9. **B9, through `coreav ffmpeg`.** On the fixtures, both encoder arms,
   each of these command lines (`IN` and `OUT.mp4` substituted), the
   first nine as they appear in the command-coverage set
   (`commands-v1`), the rest covering each accepted option once:

       -i IN -c:v libx264 OUT.mp4
       -i IN -c:v libx264 -crf 19 OUT.mp4
       -i IN -crf 0 -c:v libx264 OUT.mp4
       -y -i IN -c:v libx264 -crf 24 OUT.mp4
       -i IN -c:v libx264 -crf 23 -preset medium -pix_fmt yuv420p OUT.mp4
       -i IN -vcodec libx264 -f mp4 OUT.mp4
       -i IN -vf scale=1920:-2 -vcodec libx264 -crf 20 OUT.mp4
       -y -i IN -c:a copy -c:v libx264 -pix_fmt yuv420p OUT.mp4
       -i IN -c:v libx264 -crf 1 OUT.mp4
       -noautorotate -i IN -c:v libx264 -preset veryfast -b:v 1M -c:a copy -movflags +faststart OUT.mp4
       -i IN -c copy -c:v libx264 -preset veryfast -s 320x240 -threads 2 -fps_mode passthrough OUT.mp4
       -i IN -an -c:v libx264 -preset veryfast OUT.mp4

   - Each command either runs, or is refused with exit status 4 for a
     reason this section names, writing nothing. A file CoreAV rejects
     (exit status 1 naming a truncated or invalid structure, which
     `compat-cli.md` keeps as CoreAV's verdict) is set apart and listed,
     as B1 to B4 set a rejected file apart. Any other failure counts
     against the bar.
   - A command that runs passes B1, B3 and B4 against FFmpeg running the
     same command line (Q1 at least −0.50 dB, Q2 at least 30 dB), and B2,
     on 100% of the fixtures it runs on, except the fixtures that miss B2
     with `coreav transcode` too (listed in Results); its output lists
     the same streams, by kind and codec, in the same order, as FFmpeg's.
   - FFmpeg itself running a command that CoreAV refused is listed by
     fixture and reason, for information.
   - Speed: the job is `coreav transcode`'s, with the same encoder and
     settings, so B5 is its speed and no other run is made; the
     compatibility command's own bar (`compat-cli.md`, B4, at most 1.00
     against the same FFmpeg command) is B5's 1.05 for these commands, for
     B5's reason. What the command adds of its own is reading the command
     line and the source's video index once more before the transcode.

   Changed 2026-10-29, after a local run on the fixtures and before the
   GitHub Actions run that decides B9 (the first run at the commit that
   makes this change; earlier runs do not count). The criterion first
   committed was "refused with exit status 4 for a reason this section
   names, writing nothing; any other failure counts against the bar".
   Two things changed:

   - A file CoreAV rejects with exit status 1 naming a truncated or
     invalid structure is set apart. `truncated.mp4` is rejected so, as
     `-c copy` rejects it (`compat-cli.md`) and as B1 to B4 set it
     apart; under the first criterion it would count against the bar for a
     defect of the input, which no encoder change can pass.
   - The refusals this section names grew: a late start, reordered frames
     without composition offsets, size changes, and chapters and
     timecodes were added where the local run showed FFmpeg's output
     differing from this pipeline's, and autorotation was widened from a
     quarter turn to any matrix other than the identity.

   Changed again 2026-10-29, after the run that decides B9, from review
   of the pull request. Three refusals were added, each for sources where
   the command ran with another output than FFmpeg's or failed only after
   the encode: copied audio the MP4 writer does not take (PCM, which
   failed with exit status 1 after the encode), repeated frame times
   without `-fps_mode passthrough` (FFmpeg cannot keep both frames at a
   constant rate), and an H.264 picture cropped at its left or top edge.
   B9's FFmpeg command no longer takes `-flags +unaligned`: with it,
   FFmpeg decoded `size_left_crop.mp4` as CoreAV does, where the command
   line a user runs does not. On the fixtures the only change is that
   `size_left_crop.mp4` is refused on every line, where it ran and passed
   in the deciding run. That run stands for the code it ran on; the CI job
   checks the changed code.

   A CI job runs B9 on every change (`scripts/transcode_check.py
   --compat`), with `COREAV_FALLBACK` naming no program, so that a refusal
   stands and is not handed to FFmpeg (`compat-cli.md`, "Fallback to
   FFmpeg"). Until 2026-10-30 it ran in strict-safe mode for that; strict-safe
   mode now also refuses the FFmpeg decoder and the `ffmpeg` encoder that
   B9's build and its `ffmpeg` arm use (`compat-cli.md`, "Strict-safe
   mode"). The bar, its commands and its arms are unchanged.

Spend: the correctness and speed runs together are capped at 10 USD in
the scripts that run them, and the actual spend is reported with the
results.

## Results

[`docs/measurements/transcode.md`](../measurements/transcode.md), with its
evidence. Two rounds; each is final for the code it ran on, and the bars
above are unchanged. Round 2 (`e4b215ed`):

| Bar | Result |
|---|---|
| B1 | Missed: fixtures 71/71; corpus 239/245 (`ffmpeg` arm) and 240/245 (`x264`), scaled configuration 90/92 (HEVC 14/16); H.264 125/125 and 57/57. Native decoder 65/65 and 209/209 (one 8K output past the decode oracle's own budget on the `x264` arm). |
| B2 | Missed: fixtures 70/71 (`size_inband_sps.mp4`, "Times in decode order"); corpus 97.6% and 98.0% (bar 99%), HEVC 78% (bar 95%); scaled configuration 96.7% (89/92), HEVC 15/16, VP9 7/8 and VP8 6/7; H.264 125/125 and 57/57. |
| B3 | Met: fixtures 38/38, corpus 99/99 (VP8 and ProRes have no file whose audio MP4 carries, so no B3 share). |
| B4 | Missed: the share of files within −0.50 dB and at 30 dB is 0.976 and 0.980 over all classes, 0.78 in HEVC, 0.97 in VP8, 0.96 in AV1 on the `ffmpeg` arm, and 0.94, 0.88 and 0.86 in the scaled configuration's HEVC, VP9 and VP8 (the files that failed or lost frames); every median ΔPSNR within ±0.02 dB and size ratio within 0.997 to 1.002; H.264 1.00. |
| B5 | Missed on both arms: the bar holds overall and in every class, and some classes are above it. At most 1.05: H.264 on both arms (`ffmpeg` 1.038 [1.031, 1.050], `x264` 1.024 [1.019, 1.034]); VP8 (1.046) and all classes (1.046 [1.034, 1.063]) on the `x264` arm. Above: HEVC, AV1, VP9 and ProRes on both arms, VP8 and all classes (1.058) on the `ffmpeg` arm. VP9 through libvpx, a later run on the same files: 1.075 and 1.074, missed (`docs/measurements/vp9-libvpx.md`). |
| B6 | Met: the hostile suite (321 files) and 276 mutated copies through the command, no failure (`scripts/cli_hostile_check.py --transcode`, in CI and in the evidence); the hostile encoders (`hostile_encoders_end_in_an_error_and_leave_nothing`). |
| B7 | Met: the fuzz targets `coreav-transcode/fuzz` (`annexb`, which also takes the sample timing, and `scale`) and `coreav-isobmff-write/fuzz` (`replace`). |
| B8 | Missed: the guard counts `transcode` (`bench/guard/baseline.json`), but the CI job that runs B1 to B4 on the fixtures is not in place: it would fail on B2's fixture miss above. |

What the misses are, in the measurement: a 16 GB host's memory cap at
8K, the pacing on one HEVC stream, repeated millisecond times in WebM,
one 8K AV1 stream through `--encoder ffmpeg`, and, for speed, the cost of
the sandboxed decoders above the 5% that B5 allows. On the `x264` arm, 10-bit
sources add the x264 program's own conversion of 10-bit input, which no input
CoreAV can stream avoids. With transparent huge pages off for that program its
median over the `ffmpeg` arm on 10-bit sources went from 1.428 to 1.172, still
above B5
([`docs/measurements/transcode-x264-10bit.md`](../measurements/transcode-x264-10bit.md)).
