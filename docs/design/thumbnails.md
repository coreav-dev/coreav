# Thumbnails and frame operations

Status: Accepted

Use case 2, and the roadmap's step 1 job
"decode the needed keyframe, resize, encode an image": a service takes
an upload and makes a poster image, decoding only what the image needs, on
CPU, with unsupported codecs going to the sandboxed fallback and never to
in-process C code. This note records the frame operations
(`crates/coreav-frame`), the image encoders and the thumbnail path
(`crates/coreav-thumbnail`, `coreav thumbnail`), and the bars they are
measured against. The decode boundary under them is in
`docs/design/decode.md`.

Specifications: ITU-T T.81 and JFIF 1.02 (JPEG), ISO/IEC 15948 and RFC
1950/1951 (PNG and its zlib stream), the WebP lossless bitstream through
the `image-webp` crate, ITU-T H.273 and BT.601, BT.709 and BT.2020 (colour
conversion), ISO/IEC 14496-12 and RFC 9559 (times, edit lists, rotation).
FFmpeg was used only as a binary. No multimedia implementation source was
read.

## What it does

    coreav thumbnail INPUT --at TIME [--exact] [--width W] [--height H]
                     [--format jpeg|png|webp] [--quality Q] -o OUTPUT

writes one image of the first video stream (or `--stream N`) of an MP4,
MOV or Matroska/WebM file, local or at a URL, and prints a
`coreav.thumbnail/1` summary (`docs/schema/thumbnail-v1.md`): the frame's
time, whether it is a keyframe, the image's size and format, the decoder
that ran and whether it was the fallback. The Rust API is
`coreav_thumbnail::thumbnail(source, &options)`, which returns the image
bytes and the same facts.

**Which stream.** By default the video stream FFmpeg chooses by default:
the one with the highest resolution, the first of those on a tie
(documented `ffmpeg` behaviour); `--stream N` names another.

**Which frame.** `TIME` is a time on the file's presentation timeline
(after edit lists), counted from the file's start as `ffmpeg -ss` counts
(`ffmpeg -ss 1` on a file whose timestamps start at 10 s gives the frame at
11 s): the earliest stream start, as the probe's presentation spans
compute it (a leading empty edit, or a stream's first composition time
without an edit list).

- By default, the **latest keyframe presented at or before `TIME`**: the
  frame `ffmpeg -ss K` outputs for that keyframe's own time `K`. It needs
  one packet decoded, which is the point of the principle "decoding
  only the frame a thumbnail needs". (`ffmpeg -noaccurate_seek -ss TIME` does
  not give it for MP4: on a fixture with a keyframe at 10.0 s,
  `-noaccurate_seek -ss 10.5` output the frame at 10.467 s, so FFmpeg's
  MP4 demuxer still drops the frames before `TIME` after its seek.) A keyframe is a sync sample whose coded data agrees,
  as for cut points (`docs/design/cut-points.md`): for H.264 one that
  starts decoding by itself, for HEVC an IRAP picture, for AV1 a key frame
  shown at once with a sequence header; other codecs keep the container's
  flag.
- With `--exact`, the **first frame presented at or after `TIME`**, the
  frame `ffmpeg -ss TIME` (accurate seeking, its default) outputs. The
  packets from the keyframe before it through that frame, in decode order,
  are decoded; frames after it in decode order are not, unless the
  decoder holds the frame back: FFmpeg's H.264 decoder keeps the pictures
  after an open GOP's I picture (a recovery point, ITU-T H.264 D.2.8)
  until later packets arrive, and drops them at the end of the stream.
  When no frame comes, the packets are decoded again with those after the
  frame, in decode order up to the next keyframe and at most 32 of them
  (the seek window's margin), within what is left of the timeout; decoded
  from the I picture, it is the frame a decode from the start gives. The
  second pass stops once the frame comes, which through the sandbox is
  some packets after the decoder output it. Rejected:
  sending the later packets in the first pass whenever the frame has not
  come yet, which the sandboxed decoder, whose frames arrive late, would
  pay on every image; and starting from the keyframe before, which
  decodes a whole extra GOP and meets the same held-back frame.
- When no keyframe is presented at or before `TIME` (a time before the
  first keyframe, or an edit list that starts after the stream's only
  keyframe, as trimmed files do), keyframe mode gives the frame `--exact`
  gives instead, rather than a later keyframe.
- A time before the first frame gives the first frame; a time at or after
  the end of the stream is an error, as `ffmpeg` writes no image then.
- Times are CoreAV's: the edit list applied exactly. FFmpeg snaps an edit's
  start to the next composition time (known divergence KD-EDIT-SNAP), so
  its timestamps can be smaller by up to one frame, and a keyframe CoreAV
  presents for a fraction of a frame at the start can be one FFmpeg
  discards. A time can therefore pick a neighbouring frame of FFmpeg's.
- Composition offsets of a version 0 `ctts` of an ISO file that wrap
  (2^31 or more: negative offsets written unsigned, known divergence
  KD-CTTS-UNSIGNED) are read signed for presentation times, with a
  warning in the summary; the probe keeps reporting them as stored.
  Presentation times near 2^32 ticks cannot place a frame.

**Size and orientation.** The frame is turned by the track's rotation
(MP4 matrix, Matroska projection), and pixels are made square using the
pixel aspect ratio (`pasp`, Matroska display size): the display size.
`--width` alone keeps the display aspect ratio, `--height` alone too,
both give exactly that size, neither gives the display size. A computed
side is rounded to the nearest integer, at least 1. `ffmpeg -vf scale=W:-1`
keeps the stored aspect ratio instead and leaves non-square pixels in the
file's aspect flag, which few image viewers read; for content with
square pixels, nearly all of it, both give the same size.

**Formats.** JPEG (default, quality 91), PNG and lossless WebP. Encoders
are pluggable through the `ImageEncoder` trait; the three built in are
royalty-free and permissively licensed:

- JPEG: the `jpeg-encoder` crate, baseline JFIF, 4:2:0, the example
  tables of T.81 Annex K scaled by quality (see "JPEG encoder" below).
- PNG: CoreAV's own writer (8-bit RGB, adaptive per-row filters) with
  `miniz_oxide`'s deflate, already a dependency of the workspace, at
  level 3.
  - Level 6, the first choice, missed S4 for VP8, and for AV1 through
    dav1d on the development set (1.082), while S2 (the same frame as
    JPEG) was met. On two such development files (224×224 and 320×240
    sources at 320 wide), level 6 took 17 to 18 ms longer than level 3.
    Level 3 was within 1 to 2 ms of level 1 and gave files 3 to 5%
    larger than level 6, still smaller than FFmpeg's PNG of the same
    frame.
  - Rejected: level 1, whose files were 17 to 27% larger than level 6's
    for at most 2 ms.
  - S4 is measured again on the development set's AV1 and VP8 files, the
    classes it missed, as a development measurement; the held-out
    partition is not timed again for it. Fixed before that run: the bar is
    unchanged (median per-file ratio at most 1.00 per class).
  - Result (`docs/spikes/thumbnail-bench/`, "PNG at deflate level
    3"):
    - S4 is met for VP8 (0.899) and for AV1 through dav1d (0.895);
    - AV1 through the fallback misses by 0.002 (1.002), within run-to-run
      noise.
- WebP: `image-webp` (MIT OR Apache-2.0, image-rs), lossless only: the
  crate has no lossy encoder, and a lossy VP8 encoder of our own is not in
  this step.

Rejected: an image encoder through FFI (libjpeg-turbo, libwebp):
permissively licensed, but C code in the parent process would need a
backend exception crate for a few milliseconds of work on a
small image.

## H.264 without a configuration record

Matroska's codec mapping for `V_MPEG4/ISO/AVC` (RFC 9559) stores an
`AVCDecoderConfigurationRecord` (ISO/IEC 14496-15 5.3.3.1) in
`CodecPrivate`, and the frames then hold length-prefixed NAL units. Some
writers store no `CodecPrivate` at all and mux the byte stream as it came
(ITU-T H.264 Annex B: start codes, with the parameter sets before the
pictures that use them). The decode boundary already reads those frames
as a byte stream when the stream has no configuration record, so the
pictures decode; what such a file needs besides is a cut point its blocks
really hold:

- **The keyframe flag of such a track is checked against the frame's own
  NAL units**, read from the packet's bytes rather than walked through
  the source a unit at a time, since a byte stream's units are the
  stretches between its start codes
  (`coreav_isobmff::access::annex_b_frame_access`, the thumbnail's
  `CutRule::H264ByteStream`). The rule is the one cut points use
  (`docs/design/cut-points.md`): an IDR picture, or an exact recovery
  point with `recovery_frame_cnt` 0. FFmpeg's Matroska demuxer does not
  take the flag for such a track either: it parses the byte stream, and
  `ffprobe` reports only its IDR pictures as keyframes however the blocks
  are flagged. Without a configuration record no
  `max_num_ref_frames` is known, so an intra reference picture of a
  one-reference stream is not taken for a cut point; that is the
  reversible choice, and a stream with no record keeps its parameter sets
  before its IDR pictures anyway. The first 8,192 bytes of the packet are
  read: a frame's parameter sets, its SEI messages (at most 4,096 bytes
  of one are read for a recovery point) and its first slice header are in
  them. A frame whose units cannot be read keeps Matroska's flag, which
  the container always states: a packet with no start code in those bytes,
  one whose slice header does not parse, and one whose first slice lies
  past them, which 8,192 bytes of parameter sets and SEI messages before a
  slice would cause. The test is the IDR picture and the recovery point
  alone, not whether the frame carries parameter sets: a stream whose
  later IDR pictures do not repeat theirs still cannot be decoded from
  them, and FFmpeg's decoder cannot either ("non-existing PPS 0
  referenced"), so neither writes an image at such a time.
- **A listing that holds no cut point at or before the time is read
  again.** The Matroska glue lists the blocks from the Cues' cluster at
  or before the time, until the first keyframe presented after it. A
  writer that flagged every block leaves both the Cues and that stop rule
  pointing at blocks that do not start decoding, so the listing is read
  again from the first cluster, and then, if no block at or before the
  time starts decoding either, to the end of the clusters. Which blocks
  count is keyframe mode's own rule (`select::offered_by`: a flagged
  block presented at a known time at or before the time), not a filter
  of the glue's: a copy kept in sync by hand lost the presented test once
  (coreav-dev/coreav#16), and a listing it accepted without the frame keyframe mode
  gives yields the frame at the time instead. A test pins that the two
  agree. The blocks are checked from the last in decode order back, not
  in keyframe mode's order (latest presented first): reading a block can
  fail (a work limit), and with `--exact` a block before the one found
  is never read, so sharing keyframe mode's search was rejected because
  it made such a file fail where it had given an image. A thumbnail
  near the end of a long file whose flags are wrong therefore lists the
  blocks from its start, which costs a pass over the file's block headers
  rather than one cluster's; stepping back one Cue at a time instead would
  keep the cost nearer the Cues' and is a possible later change. Files
  whose flagged keyframes are cut points, which is all of the fixtures and
  the corpus (the instruction-count guard is unchanged), take neither
  listing.
- **When no frame at or before the time starts decoding, the first one
  after it that does is the image** (`select::exact`'s last resort, taken
  only from a listing of every packet; it holds for every container, an
  ISO file whose first sync sample comes after the time included). Its pictures before that frame
  cannot be decoded at all: FFmpeg's decoder reports "non-existing PPS 0
  referenced" for each, gives no frame for them, and the first frame it
  outputs is that cut point's. A time before the first cut point
  therefore gives the same picture in both, rather than an error.

This is why a Matroska file whose first pictures precede its first
parameter sets now gives an image at every time. On such a file
(Lavf 53.6.0, 720×480, 18 s, every block flagged a keyframe, parameter
sets and an IDR picture in every fourth), `thumbnail` refused at 40% of
the duration with "a slice names a missing picture parameter set"; it now
decodes the IDR picture at or before the time, 43.57 dB against FFmpeg's
image of the same frame in keyframe mode (I1) and 43.59 dB in exact mode
(I2), against the 40 dB median of those bars; its JPEG is 1.93 dB closer
to the lossless image than FFmpeg's of the same frame (I3), and the
decode oracle's 118 compared frames of the file are all equal to
FFmpeg's.

Rejected alternatives:

- *Decode past the undecodable pictures and present the frame that comes*,
  as FFmpeg does with its error resilience. It gives the same image here,
  for more decoding and a frame whose time is not the one asked for; and a
  thumbnail that decodes pictures known to be undecodable cannot say
  which frame it presents.
- *Treat an empty `CodecPrivate` as a missing one.* An empty record holds
  no parameter sets, so such a stream must carry them in-band, but it
  says nothing about the framing: a writer that wrote one with
  length-prefixed frames decodes today through the fallback backend and
  would not with the byte stream assumed. Left alone until a file needs
  it.
- *Read the whole packet for the cut point.* The parameter sets and the
  first slice header are at its start, and a frame's later slices only
  bear on the intra and reference tests, which no stream without a record
  can pass.
- *A Matroska fixture of this shape for the oracle gates.* `mkv_craft.py
  annexb` already makes a byte-stream fixture whose flags are honest
  (`mkv_h264_annexb.mkv`); one with every block flagged would also need a
  known divergence for the Matroska probe oracle, whose `ffprobe`
  reference flags only the IDR pictures while `coreav-mkv` reports the
  blocks' flags as stored. The shape is covered by
  `crates/coreav-thumbnail/tests/mkv_byte_stream.rs` instead, and by the
  file above.

## Frames nothing depends on

`--exact` decodes from the keyframe before the frame at `T` up to it.
Through the fallback, every frame decoded on the way is written by FFmpeg
to the worker and dropped there, while `ffmpeg -ss T` drops them inside the
decoder. That cost made S3 miss its bar on the first held-out run
(`docs/spikes/thumbnail-bench/`, "Results").

So a packet before the target that no later picture depends on is not
sent to the decoder at all (`crates/coreav-thumbnail/src/skip.rs`). Such a
packet's picture cannot be a reference, and it carries nothing else that
later pictures read:
- H.264 (ITU-T H.264 7.4.1): every slice has `nal_ref_idc` 0. Removing such
  an access unit changes no reference and no picture order count of the
  pictures after it. A non-reference picture has no
  `dec_ref_pic_marking`. Without `gaps_in_frame_num_value_allowed_flag`
  its `frame_num` is `PrevRefFrameNum + 1`; with it, gap inference gives
  the same frames with or without the picture.
- HEVC (ITU-T H.265 7.4.2.2): every slice is a sub-layer non-reference
  picture (`TRAIL_N`, `TSA_N`, `STSA_N`, `RADL_N`, `RASL_N` and the
  reserved `_N` types) of the highest TemporalId among the packets that
  are decoded. Only a picture of a higher TemporalId may reference such a
  picture. With none decoded, nothing decoded references it, and it is
  never `prevTid0Pic`.
  - That highest TemporalId is read from the packets themselves first, not
    taken from `hvcC`. Its `numTemporalLayers` is container metadata, and a
    file that understates it would have referenced pictures skipped.
  - The first pass reads only each unit's length prefix and two-byte
    header (the whole packet only for Matroska header stripping). A packet
    whose units cannot be read, or that has more than 4,096, turns
    skipping off.
  - Rejected: reading the packets twice whole (twice the I/O of the
    range), and keeping them from the first pass to the second (up to a
    group of pictures in memory).

Besides slices, such a unit may hold only access unit delimiters, filler,
and SEI whose every message is local to its picture: buffering period,
picture timing, filler, user data, and HEVC's decoded picture hash. A
message that persists past its picture (film grain, mastering display,
content light level, frame packing, display orientation, ...) keeps the
unit, as do a parameter set, an end of sequence, anything unknown, or a
length that cannot be read. The packet decoding starts from, the target,
and any packet with the target's presentation time are always sent.

VP8 and VP9 are not skipped. A frame that refreshes no reference may still
update probability contexts, loop-filter deltas or segmentation that
later frames read, and telling which frames do needs most of their header
decoding. AV1 gets dav1d instead, and VP8 and VP9 libvpx in builds with
it (`docs/design/decode.md`, "libvpx").

This holds for conforming bitstreams. A stream that labels a picture other
pictures reference as non-reference (`nal_ref_idc` 0, or an HEVC `_N`
type) breaks its own specification. A decoder that still uses that
picture as a reference gives a different image with skipping on: each
mode then shows that stream's frame differently.

`ThumbnailOptions::skip_disposable` turns this off. Off, every packet from
the keyframe is decoded.

Bars, fixed before any run of this code:
- **Same image.** With skipping on and off, `--exact` gives byte-identical
  images, in PNG at the display size. This must hold on every H.264 and
  HEVC video fixture and on every H.264 and HEVC file of corpus v1's
  development set (100%), and is checked in CI on the fixtures. CI checks
  at four times spread over each fixture, and on a Matroska copy of each.
- **Speed.** S3 is measured again on the held-out files, as a second,
  disclosed measurement that does not replace the first. Its bar and
  method are S3's ("Bars", "Speed"): a median per-file ratio of at most
  1.00, overall and per class, with the share of files faster reported.
  The change is profiled and tuned on the development files only.

## Frame operations

On `Picture`, in safe Rust, every one validating its input sizes and
returning an error rather than indexing outside a plane:

- `crop(rect)`: keeps a rectangle; on subsampled formats its edges must
  fall on chroma samples.
- `rotate(quarter_turns)`: 90°, 180° or 270° clockwise.
- `resize(width, height, filter, chroma_location)`: each plane to its
  share of the new size, chroma from its signalled siting (left when
  unknown).
- `resize_with(width, height, filter, siting)`: the same, with
  `ChromaSiting::Location` (what `resize` does) or
  `ChromaSiting::FfmpegCompatible`, which gives what FFmpeg's default
  `scale` filter gives at the same pixel format ("Y'CbCr to Y'CbCr"
  below). `coreav transcode` uses the latter.
- `to_rgb(color)`: Y'CbCr or GBR at any depth to 8-bit RGB.
- `resize_to_rgb(width, height, filter, color)`: the thumbnail path in
  one pass: luma scaled to the target size, each chroma plane scaled from
  its own size and siting straight to the target size (chroma upsampling
  and scaling in one filter), then converted. One filter pass per plane
  instead of upsampling first and scaling the RGB result.

**Filters.** Separable, from precomputed per-position weights in `f32`,
vertical pass first (it runs along whole rows, which the compiler
vectorizes), then horizontal; above a million multiply-adds the output
rows are split between up to 8 threads. `Bicubic` (the default),
`Bilinear` and `Area`. When shrinking, the kernel is stretched by the
scale factor, so every source pixel contributes (no aliasing); weights are
normalized per output position, sample centres are aligned, edges clamp.
The bicubic is Keys' cubic with a = −0.6: FFmpeg's default scaler measured
as a black box (its impulse response, in 16-bit grey at several scale
factors) matches it within its fixed-point rounding, where Catmull-Rom
(a = −0.5) was visibly softer on screen recordings.

**Chroma** (`ChromaUpsampling`). Thumbnails use `FfmpegCompatible`, what
FFmpeg's default scaler does when it converts to RGB, measured as a black
box with chroma impulses: chroma is taken as centred whatever the stream
signals, resampled to half the output width (rounded up) at the full
output height, and each chroma sample shown on two output pixels; at the
source size each chroma sample is repeated over its pixels; on the
development set this lifted the files that agreed worst with FFmpeg's
images (the results are in `docs/spikes/thumbnail-bench/`). `Full` (each chroma plane from its own siting
to every output pixel) keeps more chroma detail and stays available; for a
JPEG, which is 4:2:0 anyway, the difference is lost in the encoding.
Rotation is applied to the decoded picture before resampling, as FFmpeg's
autorotation does, so that chroma pairs follow the image's orientation; a
quarter turn of 4:2:2, which has no Y'CbCr format, turns the RGB result.
So does `Full`, since the chroma siting it honours describes the picture
as decoded, not turned.

**Colour conversion.** BT.601, BT.709, BT.2020 non-constant luminance,
FCC, SMPTE 240M, YCgCo and identity (GBR) matrices from H.273's
`matrix_coefficients`; limited or full range; any depth. An unspecified
matrix is converted as BT.601 and an unspecified range as limited, which
is what `ffmpeg` does (checked on an untagged 1920×1080 stream: its RGB
matches BT.601, not BT.709). H.273 leaves unspecified values to the
application; matching `ffmpeg` gives users of a drop-in replacement the
colours they had. Results are rounded to nearest and clamped. FFmpeg's
own conversion of 10-bit input is about two levels darker than this (its
white at 10 bits is 253, not 255); CoreAV keeps the exact values.

Which description applies is what the decoder applies, per codec, read by
CoreAV from the coded data in safe Rust (`crates/coreav-thumbnail/src/color.rs`):
for H.264, HEVC and AV1 the precedence of `docs/design/probe-color.md`
(the parameter set's over the container's, as FFmpeg's decoders apply
them); for VP9 the key frame's uncompressed header; for VP8 BT.601, which
is the only colour space RFC 6386 defines (FFmpeg's VP8 decoder reports
BT.601 under a container that says BT.709); for ProRes the frame header's
code points. Without this, a ProRes file whose container says nothing but
whose frames say BT.709 is converted with the wrong matrix.

CUDA frames are copied to the CPU before these operations; operations on
the GPU come with the NVDEC backend's production path.

### Y'CbCr to Y'CbCr

`coreav transcode` resizes decoded frames at their own pixel format
(`--scale`, and the frames of a stream that changes size,
`docs/design/transcode.md`), and its bar B4 Q1 compares against the
source scaled by FFmpeg's `scale` filter. With `resize` and the stream's
siting (left when unknown), `size_vp9_resize.mp4` scored ΔPSNR −2.12 dB
against FFmpeg's output, while unscaled files were bit-identical. So
FFmpeg's default scaler (the oracle binary, FFmpeg 7.0.2, `scale=W:H`
with no flags) was measured as a black box at the same pixel format, raw
in and raw out: weights from one impulse per source column in 16-bit
grey and 16-bit 4:2:0, then test pictures at 8 and 10 bits.

- **Kernel.** Keys' cubic with a = −0.6 in every plane, stretched by the
  plane's own factor when shrinking, as for RGB output ("Filters").
- **Chroma.** Every plane is resampled on its own grid: output sample `i`
  sits at `(i + 0.5) · s − 0.5` of the plane's own samples, with `s` the
  plane's source size over its output size. For 4:2:0 at even sizes that
  is centred chroma; at odd sizes the chroma factor is not the luma
  factor (98×8 → 37×8 and 98×8 → 38×8 give the same chroma, from 49 to
  19 samples). The frame's `chroma_location` is not used: streams coded
  losslessly by x264 and tagged left, centre and top-left come out
  identical. Taking 4:2:0 chroma as left-sited moves it by a quarter of a
  chroma sample at 2:1 and three quarters at 4:1, which is the −2 dB.
- **Step.** Positions advance by `s` rounded to the nearest multiple of
  2^−16. With the exact `s`, 1280×720 → 1920×1080 differed by up to 11
  levels on noise toward the right and bottom edges, by up to a hundredth
  of a sample of drift. Also tried, on that pair, 1920×1080 → 1280×720,
  176×144 → 501×333 and 352×288 → 117×95: 2^−12, 2^−14, 2^−18 and 2^−20
  rounded, and 2^−16 truncated, were 3 to 60 levels off on the first
  pair; 2^−16 rounded was the only one within 1 there (59.4 dB luma
  against 28.2 to 52.0) and within 0.02 dB of the best on the other three.
- **Edges.** Positions outside clamp to the edge sample, except at the
  left and top edges when the bicubic enlarges: a tap before the first
  sample farther than 1.5 samples from the output position is dropped,
  and the remaining weights are normalized. At 16 → 32 the first output
  weighs samples 0 and 1 by 1.0821 and −0.0821 where clamping gives
  1.0844 and −0.0844. Checked on 14 size pairs; at exactly 1.5, the
  rounded step decides which side a position falls on (8 → 12 drops,
  16 → 40 keeps). Shrinking, and the right and bottom edges, clamp.
- **Intermediate rows.** The horizontal pass comes first, and its results
  are cut at 2^depth before the vertical pass, with no lower cut:
  negative overshoots pass through. On a bright
  block enlarged 2:1, a model with that cut matched FFmpeg exactly; no cut
  differed by up to 16 levels, a cut at 0 and 255 by 2, and vertical
  first by 16. 256 and 255.99 fit 8 bits equally, 255 worse; 1024 fits 10
  bits best.
- **Rounding.** FFmpeg computes in fixed point. What remains is ±1 on up
  to 20% of samples (most on noise and on shrinking): no ordered-dither
  pattern showed in the rounding thresholds per 8×8 phase, and the mean
  difference changes sign with the factor (−0.07 to +0.11 levels). It is
  not modelled.

`ChromaSiting::FfmpegCompatible` does all five; bilinear and area use the
same geometry, and agree with FFmpeg's `bilinear` and `area` flags within
±1 on the three size pairs tried (2:1 area is bit-identical).

Per-plane PSNR against FFmpeg's `scale`, Y / Cb / Cr in dB, the lowest
over six pictures (noise, a zone plate, ramps, `testsrc2`, `mandelbrot`
and a frame of `basic.mp4`), with the largest difference in levels; 10-bit
PSNR is against 1023:

| Format | Size | `resize` (left siting) | `FfmpegCompatible` |
|---|---|---|---|
| yuv420p | 352×288 → 176×144 | 55.1 / 30.5 / 35.6 (28) | 54.8 / 57.1 / 57.2 (1) |
| yuv420p | 176×144 → 352×288 | 53.4 / 27.9 / 34.0 (35) | 57.7 / 59.0 / 59.0 (1) |
| yuv420p | 1920×1080 → 640×360 | 55.0 / 30.9 / 37.5 (35) | 55.0 / 57.1 / 55.8 (1) |
| yuv420p | 640×360 → 1280×720 | 52.8 / 31.1 / 34.1 (35) | 59.4 / 57.8 / 58.3 (1) |
| yuv420p | 1280×720 → 1920×1080 | 49.0 / 33.3 / 37.9 (25) | 59.3 / 59.4 / 59.3 (1) |
| yuv420p | 352×288 → 117×95 | 57.6 / 33.2 / 30.0 (59) | 57.6 / 55.4 / 54.5 (1) |
| yuv420p | 1280×720 → 320×240 | 55.0 / 31.5 / 34.0 (40) | 55.0 / 56.7 / 54.9 (1) |
| yuv422p | 176×144 → 352×288 | 53.4 / 27.9 / 30.3 (29) | 57.7 / 55.1 / 57.3 (1) |
| yuv422p | 352×288 → 117×95 | 57.6 / 34.4 / 30.8 (56) | 57.6 / 57.1 / 57.7 (1) |
| yuv444p | 176×144 → 352×288 | 53.4 / 53.4 / 53.4 (14) | 57.7 / 55.1 / 58.6 (1) |
| yuv444p | 1280×720 → 1920×1080 | 49.0 / 49.0 / 49.0 (25) | 59.3 / 59.0 / 59.4 (1) |
| yuv420p10le | 176×144 → 352×288 | 54.7 / 27.9 / 34.0 (115) | 72.3 / 77.0 / 73.4 (1) |
| yuv420p10le | 1280×720 → 1920×1080 | 49.9 / 33.4 / 38.0 (101) | 74.3 / 77.0 / 77.0 (1) |

On the 25 frames of `size_vp9_resize.mp4` that change size
(1280×720 → 320×240, FFmpeg's autoscale gives the same bytes as
`scale=320:240`): 64.16 / 37.60 / 33.96 dB with `resize`, 64.16 / 62.55 /
61.59 dB with `FfmpegCompatible`, no sample more than 1 apart. Luma was
already at rounding level; chroma siting was the gap.

At 12 bits (4:2:0) every plane is within 2 levels of FFmpeg's (at least
77 dB); at 16 bits within 24 levels of 65535 (at least 81 dB), where the
cut at 2^16 and FFmpeg's wider intermediate formats are not separately
measured. The mode is for planar Y'CbCr and grey (and NV12, through its
planar form); GBR and packed RGB, which were not measured, are refused
with `Unsupported`.

The fixtures workflow runs `crates/coreav-frame/tests/ffmpeg_scale.rs`
against the pinned FFmpeg (a missing `ffmpeg` or fixture fails there;
elsewhere the test skips): every plane at least 50 dB, no sample more
than 2 levels apart (2 · 2^(depth − 12) above 12 bits), on generated
pictures (noise, zone plate, ramps and impulses; above 0.4 megapixels
only the first two) at nine size pairs in 4:2:0 and four each in 4:2:2,
4:4:4 and 4:2:0 at 10, 12 and 16 bits, and on the fixture's frames. A
unit test holds the measured weights.

Cost: horizontal first filters every source row to the output width,
where vertical first filters only the output rows. Each source row is
filtered once into one scratch row and added into the output rows whose
taps cover it, so memory stays at a few rows plus the output whatever the
factors (1×16384 → 16384×1 peaks at 4 MB). Time, best of nine runs on
this machine (8 threads), 25 frames of 1280×720 4:2:0: → 320×240 50 ms
against 39 ms (+30%), → 640×360 101 ms against 80 ms (+26%), → 1920×1080
the same (339 and 353 ms). Next to encoding a frame (tens of
milliseconds) that is small. A strong vertical shrink with a horizontal
enlargement costs the most (1×16384 → 16384×1 grey: 0.61 s against
0.09 s), as it would in FFmpeg's order. The mode is a step of
`coreav transcode` (`--scale` and the resolution-change path), so its
speed bar is the transcode's B5 and its regression guard the transcode's
B8 case (`docs/design/transcode.md`) rather than a guard of its own; B8
counts the resize only in a case that scales, so that case passes
`--scale`.

Rejected:
- Centred siting as `resize`'s default for unknown locations: H.273's
  default for 4:2:0 is left, and `resize` would still differ from FFmpeg
  at odd sizes, at the edges and through the cut. `resize` keeps
  honouring the signalled siting; FFmpeg compatibility is a separate,
  named mode.
- Honouring `chroma_location` in the compatible mode: FFmpeg's `scale`
  does not, so every tagged stream would differ from the baseline.
- Vertical first with the cut: the cut is not linear, so the order
  matters (16 levels off on the bright block).
- Reproducing FFmpeg's fixed-point arithmetic: its weight quantization and
  intermediate formats can only be partly inferred from outside, and the
  result is already within ±1 (at least 54 dB at 8 bits).
- The same step, edges and cut in `resize_to_rgb`: thumbnails meet their
  bars, and leaving them byte-identical (checked by hashing its output
  before and after this change over 4:2:0, 4:2:2, 10-bit, NV12 and RGB,
  four sizes, three filters and both chroma modes) keeps the thumbnail
  agreement measurement valid. Whether they lift FFmpeg agreement of the
  RGB path is a measurement of its own.

## Faster rendering and JPEG

Once the plan stopped listing every packet (`docs/design/seek.md`), most of a short thumbnail's own work was rendering and encoding. On the 24-minute file profiled there, conversion to RGB and the resize took 25.3M instructions and the JPEG encoder 18.5M, out of 59.4M. This change makes both cheaper:
- **RGB conversion** (`coreav-frame`, `color::row_to_rgb8`): the colour matrix is chosen once per row instead of per pixel, and a chroma sample shared by two pixels is read once.
  - The arithmetic is the same, operation for operation, so the images are byte-identical.
  - Rejected: whole-row loops over separate R, G and B arrays. With the same arithmetic they took more instructions (8.8M against 6.2M on that file), because of the extra passes over memory.
- **JPEG encoder** (`coreav-thumbnail`, `jpeg.rs`), since replaced by the `jpeg-encoder` crate ("JPEG encoder" below). That change gave CoreAV's own encoder:
  - an AAN forward DCT (Arai, Agui and Nakajima, 1988) with its scale folded into quantization, checked by a unit test against the DCT of T.81 A.3.3;
  - one write per Huffman code and its bits;
  - a per-pixel colour loop.

  Its JPEGs could differ in bytes from before, and the image bars below judged them.
- **The `jpeg-encoder` crate** encoded a 320×180 image in 0.86 ms against this encoder's 1.90 ms, but its licence includes IJG's, which needed a licence exception; the encoder that ships is now the crate ("JPEG encoder" below).
- **Resampling** is unchanged. Making it faster without changing its sums means changing their order, which changes the images; that is a separate change, judged by the image bars.

Bars, fixed before any measured run:
1. **Same images where nothing should change.** Every PNG thumbnail of the fixtures (both modes, two times, at the display size and at 320 wide) is byte-identical to before.
2. **JPEG against FFmpeg** (I3 of "Image agreement"), on corpus v1's development set, every class, before and after this change, the same host and day:
   - the per-class median of CoreAV's PSNR minus FFmpeg's JPEG's is at least 0 dB;
   - at least 98% of files are at least 28 dB.

   Each is reported before and after.
3. **Speed.** S1 and S3 on the 611 development MP4 and MOV files, before (`main` with `docs/design/seek.md`) and after. This is a development measurement, reported overall, per class and by FFmpeg's time (under 50 ms, 50 to 300 ms, over 300 ms), against the thumbnail bars, unchanged.

   The confirmation is the joint run on corpus v2-heldout draw 4 (`docs/design/seek.md`, bar 3). Its bars are committed after this change is merged and before the draw is opened.
4. **Instructions.** On the profiled file, rendering's and the encoder's instructions are reported before and after.

Results: `docs/spikes/thumbnail-bench/`, "Faster rendering and JPEG".

## JPEG encoder

JPEG thumbnails are encoded by the `jpeg-encoder` crate 0.6.1 (vstroebel), pinned exactly in `crates/coreav-thumbnail/Cargo.toml`: baseline JFIF, 4:2:0 (`SamplingFactor::F_2_2`), at a quality from 1 to 100, as the crate documents.
- **Quality: the default is 91**, the crate's operating point equivalent to CoreAV's own encoder at 90. On the fixtures' thumbnails at 320 wide, measured by the thumbnail spike's quality script (59 files, PSNR against CoreAV's lossless PNG of the same frame, output in `evidence/jpeg-quality-fixtures.txt`):

  | Crate's quality | PSNR, median against own at 90 | Bytes, median against own at 90 |
  |---|---|---|
  | 90 | −0.60 dB | +0.4% |
  | 91 | +0.02 dB | +3.8% |
  | 92 | +0.67 dB | +7.5% |

  At the same nominal quality, the crate spends about as many bytes for a lower PSNR. So 91, not 90, is the like-for-like setting, and the speed comparison below is not a lower quality bought for time.
  - Rejected: keeping 90, which on the development set gave 0.32 dB less (median) for 1.1% more bytes.
  - Rejected: mapping the user's `--quality` onto a different crate setting, which would hide what the number means.
- **Why.** Encoding a 320×180 image takes 0.86 to 1.00 ms with the crate (two test images), against 1.90 ms with CoreAV's own encoder after "Faster rendering and JPEG", and 2.43 ms before it (`crates/coreav-thumbnail/tests/speed.rs`). JPEG is the default format, so that is about a millisecond of every default thumbnail.
- **Licence.** The crate is "(MIT OR Apache-2.0) AND IJG". IJG is allowed for this crate alone: `deny.toml` scopes it as a licence exception for `jpeg-encoder`.
  - IJG asks binary distributions to state "This software is based in part on the work of the Independent JPEG Group". `NOTICE` holds it, and `docs/release/notices.md` lists the distributions that ship it.
  - The crate has no dependencies of its own, and `cargo deny check` passes on the graph with it.
- **Pinned.** A new version changes the bytes of every JPEG, so it is reviewed and the image bars are run again before the pin moves. Its SIMD paths are the crate's own `unsafe`, as for any dependency; CoreAV adds none.
- **Image agreement**, at the default 91, on corpus v1's development set (I3, 735 files with images; `docs/spikes/thumbnail-bench/evidence/images-jpeg-encoder-q91.jsonl`). The bar is at least 98% of files at 28 dB or more, and each class's median of CoreAV's PSNR minus FFmpeg's JPEG's at least 0:
  - 729 files (99.18%) are at least 28 dB: the bar needs 721 of 735, so 8 files of margin (1.18 points);
  - per-class medians are AV1 1.514, H.264 1.385, HEVC 1.392, ProRes 1.403, VP8 0.932 and VP9 0.762 dB. The thinnest margin is VP9's, 0.76 dB;
  - against CoreAV's own encoder at 90, file by file: 0.07 dB less (median) for 5.7% more bytes. Against FFmpeg's JPEG of the same frame, 1.77 times the bytes (1.68 for the own encoder);
  - for comparison, the crate at 90 had 729 of 735 (99.18%) and medians of 0.568 to 1.266 dB (`evidence/images-render-jpeg-encoder.jsonl`), and the own encoder 732 of 735 (99.59%) and 0.92 to 1.67 dB.
- **Speed at 91.** The timing test's image: 1.00 ms and 43,005 bytes, against 1.90 ms and 38,233 bytes for the own encoder at 90.
- **Rejected:** keeping CoreAV's own encoder (`jpeg.rs` before this change, removed with it), about 1 ms slower per JPEG. Keeping both behind a feature was also rejected: the encoder that ships is the one measured.
- **Speed.** The thumbnail bars on corpus v2-heldout draw 4 are measured with this encoder (`docs/design/seek.md`, bar 3, and the joint run's bars).

## Joint confirmation on held-out draw 4

The fixed-cost changes of an earlier change were measured on corpus v1's development set only:
- planning from a window (`docs/design/seek.md`);
- faster rendering ("Faster rendering and JPEG");
- the `jpeg-encoder` crate ("JPEG encoder").

One fresh held-out draw confirms them together: corpus v2-heldout draw 4 ([`docs/corpus.md`](../corpus.md#draw-4)), the unspent draw selected for `operation: thumbnail`. Everything below was fixed and committed before any file or result of the draw was opened.

**What is measured.**
- **Code:** `main` at `0f1c3c4` (0f1c3c4bbf7ad36892772110b0313dc7464fce31), which holds all three changes.
- **Build:** `cargo build --release -p coreav-cli --features ffmpeg-fallback,dav1d`.
- **Encoder:** JPEG through the `jpeg-encoder` crate 0.6.1 at the default quality, 91.
- **Rest of the configuration:** PNG at deflate level 3; AV1 through dav1d; every other allowlisted codec through the sandboxed FFmpeg fallback.
- **FFmpeg:** the pinned binary (7.0.2), as in every earlier run.

**Inputs.**
- Every `real` file of draw 4 that is held out and not spent, classed by the codec of the stream the thumbnail takes (the manifest's `codec`).
- A codec off the default allowlist is counted apart.
- No file is left out after the run starts, except for a failure the host itself causes (below).

**Method.** `docs/spikes/thumbnail-bench/` on this host, with the runners of the development measurements:
- `speed.py --set draw4`: S1 to S5, five rounds per command in rotated order after a warm-up, the median per file;
- `oracle.py --set draw4 --images-only`: I1 to I3;
- `seek_corpus.py --set draw4 --times 16`: the same choice from a window, on draw 4's MP4 and MOV files.

The timed run is alone on the host.

**Bars.**
1. **Speed**. For each of S1 to S5, the median per-file ratio is at most 1.00 overall and in each class of at least 10 files. A class of fewer files is reported and not judged.
   - A claim that CoreAV is faster than FFmpeg for a bar, overall or in a class, needs the upper bound of the 95% bootstrap confidence interval below 1.00 (10,000 resamples, seed 207).
   - Reported beside each bar: the share of files faster, overall, per class, and by FFmpeg's own time for that bar's command (under 50 ms, 50 to 300 ms, over 300 ms).
2. **Images** (I1 to I3 of "Image agreement"), on the same files, with the values of that section.
3. **Same choice** (`docs/design/seek.md`, bar 1): 100% of draw 4's MP4 and MOV files at 16 times each, both modes.

**Rules of the run.**
- One run.
- A file whose download fails, or that the host cannot finish (shown by the host's own logs), is run again once, with the same build. No other file is run again.
- The results are reported as they come out, every bar met or missed.
- After the report, the draw is marked spent (the top-up script's `spend` command). It is then a tuning pool and backs no later claim.

**Results:** `docs/spikes/thumbnail-bench/`, "Joint confirmation on draw 4".
- Every bar is met.
- For each of S1 to S5, the upper bound of the 95% confidence interval is below 1.00, overall and for H.264.
- The share of files faster is 98%, 96%, 79%, 92% and 98%, counting the one refused file as slower. The bars did not say how a refused file counts; the results give both counts, and both meet every bar.
- VP8's `--exact` is slower than FFmpeg's (1.411 on 7 files, a class too small to judge).
- One file is refused.

## Limitations

- In a stream whose H.264 parameter sets change the frame size mid-stream
  (fixture `size_inband_sps.mp4`), FFmpeg's decoder labels some frames
  with other packets' timestamps, and CoreAV chooses frames by the labels
  the fallback reports, so its image can be a neighbouring frame; frame
  agreement (decode bar 1) holds. The fixture gate leaves its images out
  and says why.
- Frames whose timestamp FFmpeg discards (KD-EDIT-SNAP) have no FFmpeg
  image to compare with; the image bars count them apart.

## Limits

The decode limits of `docs/design/decode.md` apply, plus the container
limits of the probe. The output size is at most 16,384 × 16,384
and the encoded image at most 256 MiB. The CLI's default decode timeout is
30 s.

## Bars

Fixed before any measured run. Inputs: corpus v1, files
whose first video stream's codec is on the default decode allowlist.
Classes are that codec: H.264, HEVC, AV1, VP9, VP8, ProRes. `T` is 40% of
the stream's presented duration, rounded down to the millisecond; the
width is 320 and the height follows the display aspect ratio as above.
Files CoreAV refuses or FFmpeg cannot thumbnail are counted and reported
apart, by reason, never dropped silently.

How the oracle commands below reach the same frame changed after the
preliminary runs (`docs/spikes/thumbnail-bench/`, README) and before the final
one; no threshold changed:
- The image reference seeks FFmpeg's input to the keyframe CoreAV decoded
  from (`decode_start`, on FFmpeg's own timeline) and then its output to
  the frame (`-ss K -i FILE -ss P−K`), or decodes from the start when the
  keyframe is at the file's start. An input seek to `P` alone let FFmpeg
  start from another keyframe on files whose sync table is not exact, so
  the two tools decoded different references. A frame whose packet FFmpeg
  discards (`D` flag) has no FFmpeg image and is counted apart.
- Times given to FFmpeg are on FFmpeg's timeline, from the packet's exact
  pts (known divergence KD-EDIT-SNAP), not CoreAV's.
- Speed: each file also records, untimed, the PSNR between CoreAV's and
  FFmpeg's PNG of S2's and S3's frame; a bar's results count the files
  where they differ (under 30 dB). A bar whose command failed on a file
  leaves that file out of that bar only, with the reason.

### Image agreement (all `real` files)

The reference is `ffmpeg -ss P -i FILE -frames:v 1 -vf scale=W:H
-pix_fmt rgb24 ref.png`, where `P` is the presentation time CoreAV reports
for the frame it chose, rounded down to the microsecond, so FFmpeg's
accurate seek lands on the same frame. PSNR is over the three 8-bit RGB
channels of the whole image.

- **I1, PNG, keyframe mode**: CoreAV's PNG against the reference. The
  median per class at least **40 dB**, and at least **98% of files** at
  least **35 dB**. Two good resampling filters of the same frame differ
  by little; a wrong frame, matrix, range, chroma siting, rotation or a
  shifted line costs far more.
- **I2, PNG, `--exact`**: the same bar, on the frame at `T`.
- **I3, JPEG**: CoreAV's JPEG at its default quality, decoded by `ffmpeg`
  to RGB, against the reference: per class, the median of (CoreAV's PSNR
  − the PSNR of `ffmpeg`'s own JPEG of the same frame and size) at least
  **0 dB**, i.e. at least as close to the lossless image as FFmpeg's; and
  at least **98% of files** at least **28 dB**. File sizes are reported
  beside it.

### Speed (held-out files)

CoreAV's `coreav thumbnail` against the `ffmpeg` command line, both as
processes timed from outside (start-up, sandbox and worker included), the
source in the page cache, outputs on tmpfs. Per file, one warm-up of each
command, then 5 rounds in alternating order; a file's time is the median
of its rounds and its ratio CoreAV / FFmpeg. Bars on the median of the
per-file ratios, **at most 1.00 overall and in every class**, with a
bootstrap 95% confidence interval (10,000 resamples); a "faster" claim
needs its upper bound below 1.00.

- **S1, keyframe mode** against `ffmpeg -ss T -i FILE -frames:v 1 -vf
  scale=W:H out.jpg`, the reference command (FFmpeg decodes from the
  keyframe to `T`; CoreAV decodes the keyframe).
- **S2, keyframe mode, same frame** against `ffmpeg -ss K -i FILE
  -frames:v 1 -vf scale=W:H out.jpg`, where `K` is the time CoreAV reports
  for the keyframe it chose, rounded down to the microsecond: the same
  keyframe, and FFmpeg seeks straight to it, so equal decoding work. (Set
  before any run, in place of `-noaccurate_seek -ss T`, which on MP4 does
  not output the keyframe and decodes past it; see "Which frame".)
- **S3, `--exact`** against `ffmpeg -ss T -i FILE -frames:v 1 -vf
  scale=W:H out.jpg`, the same frame.
- **S4, PNG** (keyframe mode, `-o out.png`) against S2's command writing
  `out.png`.
- **S5, WebP** (keyframe mode, lossless) against S2's command writing
  `out.webp` with `-c:v libwebp -lossless 1`.

The backend is what the default policy picks on the measuring host, built
with every backend feature; the results say which ran per class. A later
backend for a class (dav1d for AV1, NVDEC on a GPU host) is measured
against the same bars before it becomes that class's default.

Results are in `docs/spikes/thumbnail-bench/`, "Results":
- the image bars are met;
- decode bar 1 is met for streams and missed for frames, because of three malformed files CoreAV refuses (148 frames). Three runs whose containers failed count as disagreeing streams, and no frame CoreAV decoded differs from FFmpeg's;
- S1, S2 and S5 are met;
- S3 is missed overall and for H.264, AV1 and VP8 in round 1.
- Round 2, after "Frames nothing depends on", reused the same held-out partition, so it is a development measurement.
  - It has 0.854 overall (72% of files below 1.00). It meets the bar overall and for H.264, HEVC and ProRes, and misses it for AV1, VP8 and VP9 (1.010, within the ±5% run-to-run noise of 1.00).
  - It backs no "faster" claim, which waits for a fresh held-out draw;
- S4 is missed for VP8.
- Faster rendering and JPEG (`docs/spikes/thumbnail-bench/`, "Faster rendering and JPEG"): PNG byte-identical; I3 unchanged and met. Development set: S3 0.772 → 0.760; under 50 ms of FFmpeg time, 84% → 87% of files faster.
- Planning from a window of the packets (`docs/design/seek.md`) gives the same choice on all 611 development MP4 and MOV files. Development set: S1 0.441 → 0.419, S3 0.807 → 0.780, and under 50 ms of FFmpeg time S3 goes from 65% to 81% of files faster (`docs/spikes/thumbnail-bench/`, "Planning from a window").
- AV1 through dav1d (`docs/spikes/thumbnail-bench/`, "AV1 through dav1d"), a development measurement on the development set and on the held-out AV1 files, which the fallback's rounds had timed: S3 is met (0.927 and 0.934, 60% and 62% of files below 1.00; fallback round 2: 1.186). S4 is missed on the development set (1.082), because of the PNG writer.
- VP9 through libvpx (`docs/measurements/vp9-libvpx.md`), a development measurement on both partitions of the VP9 files: S1 to S5 are met, S3 at 0.994 (51% of files below 1.00; through the fallback in the same run, 1.114).

The cost of `--exact` grows with the distance from the keyframe: through the fallback, FFmpeg writes every decoded frame to the worker, and the worker drops the unwanted ones.

### Regression guard

`coreav thumbnail` joins the CI guard (`docs/design/benchmarks.md`):
instructions of the `coreav` process (probe, frame choice, the frame
copied out of the worker, resampling, conversion and encoding) on the
guard's H.264 fixture, under the guard's tolerance. It runs on a second
build with the FFmpeg fallback, beyond that build's own start-up; every
other case and start-up stay on the shipped build.

The subcommand and the thumbnail code are compiled only with a decoder
backend (the `thumbnail` feature, which `ffmpeg-fallback` turns on). A
build without one could not make a thumbnail, and in the default build
the subcommand's arguments and linked code cost every other command
about 11% more start-up (80,000 instructions of 711,000 in the `dist`
build on the development machine): the loader's relocations and symbol
binding for the frame and image code, and the argument parser building
13 more options and freeing them. The worker and FFmpeg
are not counted: FFmpeg's decoding is FFmpeg's work, and Valgrind cannot
run the worker inside its sandbox (traced with `--trace-children`, the
worker exits before its first message). The worker's cost is in the speed
bars, which time both processes from outside.
