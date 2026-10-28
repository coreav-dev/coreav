# Known divergences from FFmpeg (bar C2)

Disagreements of the measured corpus run (`evidence/corpus.jsonl`) where the
specification supports this decoder's output, recorded as known
divergences instead of copying FFmpeg's output. `scripts/investigate.py` re-examines every disagreeing
stream; its output is `evidence/divergences.jsonl`. FFmpeg 7.0.2 is a black
box here: the explanations below rest on its output and log messages only.

## KD-N1-REORDER: FFmpeg omits a picture when it grows its reorder buffer

Eight streams. FFmpeg outputs one picture fewer than this decoder. FFmpeg's
frames are an order-preserving subsequence of this decoder's; the one
missing picture is a picture with a lower picture order count than a picture
FFmpeg had already output, and FFmpeg logs "Increasing reorder buffer" (at
its verbose level) at that point. None of the eight streams carries the
VUI bitstream restriction (`max_num_reorder_frames`,
`max_dec_frame_buffering`), so a decoder that outputs early has to guess the
reordering depth.

Specification: every decoded picture with OutputFlag 1 is stored in the
DPB and leaves it through the bumping process in increasing picture order
count (C.4.5.1 to C.4.5.3), with the DPB size from `max_dec_frame_buffering`
or MaxDpbFrames (A.3.1, E.2.1). This decoder outputs all pictures, in
increasing picture order count within each coded video sequence; without
the VUI fields the DPB size is MaxDpbFrames of the stream's level, enough for
the reordering these streams use.

| Stream (corpus v1 key, short) | Frames (CoreAV / FFmpeg) | Omitted picture |
|---|---|---|
| `real/screen/…2e78223d6b4c284d.mov` | 448 / 447 | frame 10, POC 17 |
| `real/screen/…808b42394a4777b3e846.mov` | 1013 / 1012 | frame 26, POC 49 |
| `real/screen/…19554d87b6b2d92cfaa1ef7d.mov` | 54 / 53 | frame 8, POC 9 |
| `…58fa462d84b5cbe8c109c0a79aba6df8963c.mp4` | 240 / 239 | frame 233, POC 34 |
| `…3d864b4b8d98fab86f521f72f06f346c8e76.mp4` | 240 / 239 | frame 233, POC 34 |
| `…cc8d5a282e6cb5edaaf0b3e7897987854838.mp4` | 1077 / 1076 | frame 8, POC 13 |
| `…0011b5b1a2cbe686ea9fefa13bb874899734.mp4` | 240 / 239 | frame 233, POC 34 |
| `…e926ee8b8aaa0973ed91bdf10867e39ccf4a.mp4` | 240 / 239 | frame 233, POC 34 |

## KD-N1-AUTOSCALE: the FFmpeg command rescales frames after a resolution change

Two streams change their picture size mid-stream (a new SPS at an IDR
picture). The `ffmpeg … -f framemd5` command of the C2 method scales every
frame to the first frame's size before hashing, so FFmpeg's hashes after the
change are of scaled pictures, not decoded ones. With `-noautoscale` added,
FFmpeg's hashes equal this decoder's for every frame of both streams.

Specification: a decoded picture has the size its active SPS gives
(clause 7.4.2.1.1, `pic_width_in_mbs_minus1`, `pic_height_in_map_units_minus1`
and the cropping rectangle); this decoder outputs each picture at that size.

| Stream | Frames | First differing frame | Agrees with `-noautoscale` |
|---|---|---|---|
| `…ea72a0945d2b988d00ff2e22ddbe139d94e7.mov` | 4 / 4 | 2 | yes |
| `…b8e4cb0f1f5329ee77a83bd2085dc2874be9.mp4` | 2848 / 2848 | 1332 | yes |

## Not known divergences: non-conforming streams

Three streams lose pictures: a reference picture's `frame_num` skips values
(for example 2 to 4, or 119 to 123, with the picture order count skipping as
well) while `gaps_in_frame_num_value_allowed_flag` is 0. Such a stream does
not conform (clause 7.4.3, `frame_num`), and the specification says only that
the decoder "should infer an unintentional loss of pictures" (8.2.5.2); it
specifies no output. The two decoders conceal differently from the first
picture that predicts from a lost one. The specification supports neither
output, so these count against bar C2 as written; see README Results and Verdict.

| Stream | Frames | First differing frame | Gap |
|---|---|---|---|
| `…05e80723364aa7536a72ce792ded0e8fd487.mp4` | 369 / 369 | 368 (the last) | frame_num 1 after 15 |
| `…6bfeb6d45c734f877824a752ff7b0b5e9691.mkv` | 1311 / 1311 | 1096 | frame_num 123 after 119 |
| `…ee3fddedb2db07549525fb91f131932c2dac.mp4` | 9000 / 9000 | 1407 | frame_num 4 after 2, and later gaps |
