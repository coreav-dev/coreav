# H.264 decoder: correctness of the production crates

The runs behind bars C1 and C2 of `docs/design/h264.md`, for the decoder in
`crates/coreav-h264` with the kernels of `crates/coreav-kernels-h264`.
Evidence: [`h264-decoder/`](h264-decoder/).

## C1 conformance: met

Every in-scope JVT conformance bitstream decodes bit-exactly: each decoded
picture, cropped and in output order, equals the suite's reference decoded
file byte for byte.

| Run | Streams | Result | Evidence |
|---|---|---|---|
| x86-64, `Auto::detect()` kernels (AVX2 where the measurement says so) | 97 | **97 of 97 bit-exact**, 13,329 frames | [`conformance-x86_64.json`](h264-decoder/conformance-x86_64.json) |
| x86-64, scalar kernels forced | 97 | 97 of 97 bit-exact | same test, `--kernels scalar` |

- Set: the 97 in-scope streams of
  `crates/coreav-h264/tests/data/conformance-streams.json`, the list N1 and
  N2 used (every sequence parameter set with `profile_idc` 77 or 100 or
  `constraint_set1_flag`, `chroma_format_idc` 1, 8-bit, frame-only).
- Method: `scripts/h264_conformance.py`, which drives the `#[ignore]`d
  integration test `crates/coreav-h264/tests/conformance.rs`. Each zip is
  checked against its SHA-256 in the manifest before it is used.
- Machine: the development machine, Intel Xeon at 2.90 GHz, Linux x86-64;
  28 s for the whole suite in a release build. The committed report is the
  `h264-conformance` workflow's run of the same script on a GitHub runner,
  which carries both bars per stream.
- The two rows differ in which kernels run: the SIMD step added the vector
  paths, and the suite passes with them and without them
  (`docs/measurements/h264-kernels.md`). The arm64 run of C1 is in the
  `h264-conformance` workflow, on a push to main.

## C5 keyframe-only mode: met on the conformance set; the corpus half is owed

The mode a thumbnail uses decodes only the pictures that need no other
picture, and skips the rest. The bar is that what it returns for an access
unit is the picture full decoding decodes for it, and that it returns one
for every IDR access unit (`docs/design/h264.md`, "Pictures that need no
other picture").

| Set | Result |
|---|---|
| The 97 conformance streams, each decoded twice | **1,309 keyframes of 13,329 pictures, every one identical to the picture full decoding returns for the same access unit**, no picture missing and none extra, in decode order ([`conformance-x86_64.json`](h264-decoder/conformance-x86_64.json)) |

- Method: `the_keyframe_mode_returns_what_full_decoding_returns` in
  `crates/coreav-h264/tests/conformance.rs`. Each run marks every NAL unit
  with its index, so a picture carries the access unit it came from
  through the reordering; the check is then two-sided. Every picture the
  mode returns must be the picture full decoding returns for that same
  access unit, sample for sample, and the access units it returns must be
  exactly the stream's pictures that need no other picture — those whose
  every slice is intra, counted by reading the stream without the
  decoder — in that order. The first version of this test asked only for
  a subsequence, which a mode that returned nothing passed; the second
  counted only IDR access units, which a mode that dropped a non-IDR
  intra picture passed. `scripts/h264_conformance.py` runs it after bar
  C1 over the same extraction and reports both bars per stream.
- A picture is a keyframe only if every one of its slices is intra. Two
  of the 97 streams (`CABAST3_Sony_E`, `CABASTBR3_Sony_B`) mix intra and
  predicted slices in one picture; those pictures are decoded in part and
  not returned, which is what the first version of the mode got wrong.
- **What the mode saves.** The same streams decoded twice, on the
  development machine, in a release build with the `Auto` kernels
  (`cargo run -p coreav-h264 --release --example decode -- [--keyframes]`):

  | Stream | Full decoding | Keyframes only | Ratio |
  |---|---|---|---|
  | `00d2057ba2cd` 720p | 5.18 s | 0.08 s | 0.016 |
  | `0758bb0f1c23` 720p | 2.22 s | 0.31 s | 0.140 |
  | `0bb05726e428` sd | 2.87 s | 0.17 s | 0.060 |
  | `060b839eaa34` 720p | 4.24 s | 0.21 s | 0.050 |
  | `05f55a4982d8` 1080p | 14.24 s | 1.65 s | 0.116 |

  Median 0.060: a keyframe costs a few per cent of the stream it is in,
  and how few depends on how often the stream carries one. This is a
  number, not a bar; the bar for the operation that uses the mode is the
  thumbnail speed of `docs/design/thumbnails.md`, measured
  with the native backend in `docs/spikes/h264-native-thumbnail/`.
- **The corpus half of C5 is not run yet.** The bar is pre-registered over
  the conformance set *and* the corpus; the corpus run belongs with bar
  C2's, which waits for the same reason (below). What the conformance set
  does not hold, synthetic streams in `crates/coreav-h264/tests/decode.rs`
  do: an open group of pictures whose second intra picture's
  `pic_order_cnt_lsb` has wrapped, a stream that allows gaps in
  `frame_num` with 199 skipped pictures between two keyframes, and a
  picture a later access unit discards. Each was a defect found in review
  before it was a test.

## C2 corpus: a first check, not yet the bar

Bar C2 compares the whole corpus run against N1's recorded per-stream
outcome. That run belongs to the step that finishes the kernels, so that it
measures the build users get; running it twice would spend the budget
twice. What is measured here is the same method on the streams that were
already on the machine:

| Set | Frames | Result | Evidence |
|---|---|---|---|
| The 32 tuning streams of the N2 spike (real files, sd to 4k) | first 60 each | **32 of 32 agree** with FFmpeg 7.0.2, frame for frame | [`agreement-tuning.json`](h264-decoder/agreement-tuning.json) |

- Method: `scripts/h264_agreement.py`. Both sides decode the same Annex B
  elementary stream; FFmpeg reports a per-frame MD5 (`-f framemd5
  -fps_mode passthrough -threads 1`) and the script hashes the frames the
  decoder writes (`cargo run --example decode`), so only decoded samples
  are compared.
- These are the N2 spike's tuning streams, which carry no measurement
  weight: they say the decoder agrees on real files, not how fast it is.
- Oracle: FFmpeg 7.0.2 built from the official release tarball, as in N2.

## Spend

None. Both runs are local: the conformance suite and the streams were
already on the development machine.
