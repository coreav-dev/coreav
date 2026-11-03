# Supplemental inputs for V2 commands

## Goal and boundary

Give every fixture-fixable V2 command a generated input without changing
its command line or how its output is judged. This is an input registration
for subsequent runs, not a result or a new coverage claim. Refs coreav-dev/coreav#25.

The current-main recount at `a09201d9709bdf4a59e4a99a3dc136fc958e630c`
finds 171 of the 488 media commands without an assigned input. The latest
Linux fixture records have exactly the same 171 indices as the earlier
inventory. The command set, evaluator, three generators and Linux oracle
pin are byte-identical to current main. This recount reuses reference
assignment evidence; it does not execute or score a new candidate.

| Current input owner | SHA-256 |
|---|---|
| `tools/compat-corpus/commands-0.1.0/coverage.json` | `077ed478f4693c5612ec3d018c9d3bbe96125a80eb635322ddf3d68dec6894c3` |
| `tools/compat-corpus/encode_breadth.py` | `86fd466dfa21bd4e973cfe76c9d4afd258c8e81c4fe422c470c4969630ad49d9` |
| `scripts/gen-fixtures.sh` | `baa6378c62602ce2332f2d1138766965eaa979737864a29737cd4d92b16ab6cb` |
| `scripts/gen-fixtures-mkv.sh` | `2edfdf356a214c97fc24c9e445892beaf9bd9e339d9fd519f100a800b1f05010` |
| `scripts/gen-fixtures-ts.sh` | `d7f797724e4521b1a4840ce41953cfb5f1fd8e4b79a0266feacbf325bf4d5551` |
| `scripts/oracle-ffmpeg.env` | `95601248eba142d1cbf8d81aaf46d5ebc61c1962fe1edafdc1ee2c1b7bd33f24` |

## Assignment rule

Keep the original five-fixture Cartesian traversal unchanged. Only after
all its assignments fail, try the per-command tuples registered below,
in their listed order. Every placeholder still names a file; concat
placeholders still name the existing two-entry list. Never substitute a
filter expression, device, service, shell variable, option or preset.

A global expanded fixture pool is rejected because it changes the first
successful legacy assignment for multi-input commands. Explicit fallback
tuples avoid that change and avoid exponential combinations. Bind the
tuple table to the unchanged command-set digest and original row indices.

Generate all additional media from synthetic patterns, zero-filled raw
frames, a 440 Hz tone and authored subtitle text with the pinned FFmpeg
binary as a black box. Commit the generator, not media files. The generator
and authored input text use this repository's existing license terms;
there is no third-party media license or downloaded media input.

## Verification

Tests must prove that all 171 indices are listed once, that existing
first-success assignments and their order stay unchanged, and that every
fallback tuple has the command's exact placeholder count. Fixture generation
must be bounded and fail with diagnostics. Record every supplemental file's
SHA-256 beside the original five hashes. Linux x86-64 CI verifies generation
and reference assignment with the pinned FFmpeg 9.x; it must retain failed
reference diagnostics rather than infer a fixture gap from a bare count.

Do not alter command rows, existing predicates, bars, tolerances, frozen
judging files, earlier records or held-out inputs. PNG/BMP conversion
registration and bitstream-filter implementation are separate work. A new
reference assignment is not an oracle pass. Publish no new V2 count until
a merged run produces it.

## Fixture recipes

Recipe labels without `v2_` gain that prefix in generator outputs. The
inventory uses actual file names. The original `basic.mp4` and
`audio_aac.m4a` keep their names.

| File | Committed generator recipe |
|---|---|
| `still_rgba.png`, `still_rgb.jpg` | One 64x48 synthetic pattern frame, PNG RGBA and JPEG respectively |
| `v2_still_rgba%03d.png`, `v2_still_rgba000.png` | Identical PNG frames: the real percent-named file selects image2; glob reads that file and sequence reads its declared numbered sibling |
| `audio_mp3_long.mp3` | 480 seconds of 440 Hz, 48 kHz mono audio, libmp3lame |
| `multi_audio_eng.mp4` | Two seconds of 64x48 H.264 video followed by four AAC 440 Hz tracks; first audio language eng, others und |
| `video_only.mp4` | Two seconds of 64x48 H.264 video with no audio |
| `audio_opus_long.ogg` | 130 seconds of 440 Hz, 48 kHz mono audio, libopus |
| `audio_aac_long.m4a` | 40 seconds of 440 Hz, 48 kHz mono audio, AAC |
| `video_long.mp4` | 5027 seconds of 64x48 H.264 at one frame per second, with actual packets throughout and no audio |
| `raw_yuv420p_1280x720.yuv` | One complete zero-filled 1280x720 yuv420p frame, also aligned to stereo S16 PCM for the repeated placeholder |
| `raw_yuv420p16le_1280x720.yuv` | One complete zero-filled 1280x720 yuv420p16le frame |
| `raw_yuv420p_1920x1080.yuv` | One complete zero-filled 1920x1080 yuv420p frame |
| `raw_bgr24_640x480.raw`, `raw_rgb32_800x550.raw` | One complete zero-filled frame at the named geometry and format |
| `raw_yuv420p_400x300_long.yuv` | Sparse zero-filled raw frames for 4352 seconds at 25 fps; approximately 19.6 GB logical size, sparse-preserving staging required |
| `subtitles.srt`, `subtitles_movtext.mp4` | One authored synthetic cue; the latter muxed as mov_text in MP4 |
| `mpeg_ps_video.mpg` | Two seconds of 64x48 MPEG-2 video in MPEG-PS |
| `video_hevc.mp4` | One 64x48 HEVC frame, libx265 |
| `hls_local.m3u8` | A finite local playlist naming one generated MPEG-TS segment; stage and hash that declared dependency too |

Raw input probing and unsupported codec jobs can still produce no_oracle
after reference assignment. This plan does not broaden either predicate.

## Complete fixture-less inventory

Indices are zero-based rows in the unchanged 630-command registration.
A tuple lists files in first-placeholder order. A proposed tuple is an
input-construction plan, not evidence that the command or candidate passes.
Conditional tuples need focused reference diagnostics. `None` identifies
a case for which media alone does not repair the stated blocker.

| Index | Proposed input tuple | Reason or remaining blocker |
|---|---|---|
| 8 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 14 | `v2_video_only.mp4` | Use a video-only H.264 file to remove the selected audio job without changing any option. |
| 16 | `audio_aac.m4a`, `v2_still_rgba.png` | The existing concat adapter supplies two copies of the AAC audio file; the second input must accept image framerate. Both inputs are finite. |
| 17 | `v2_still_rgba%03d.png` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 18 | `v2_still_rgba%03d.png`, `v2_audio_opus_long.ogg` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 19 | `v2_still_rgba%03d.png`, `audio_aac.m4a` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 20 | `v2_still_rgba%03d.png`, `audio_aac.m4a` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 21 | `v2_still_rgba%03d.png`, `v2_audio_aac_long.m4a` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 22 | `v2_still_rgba%03d.png` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 23 | `v2_still_rgba%03d.png`, `audio_aac.m4a` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 24 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 26 | `v2_video_long.mp4` | The registered seek is beyond the original two/three-second video fixtures. A 5027-second small H.264 video at 25 fps covers the largest seek, 01:23:45, and the short extraction windows. |
| 29 | `v2_video_long.mp4` | The registered seek is beyond the original two/three-second video fixtures. A 5027-second small H.264 video at 25 fps covers the largest seek, 01:23:45, and the short extraction windows. |
| 53 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 54 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 55 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 62 | `v2_multi_audio_eng.mp4` | Four audio streams are required by 0:a:0 through 0:a:3; the fixed extra WAV output paths remain unchanged. |
| 63 | `v2_multi_audio_eng.mp4` | 0:a:3 requires the fourth audio track. Its registered output is OUTPUT.m4a, despite container being labeled MP4. |
| 65 | `v2_audio_opus_long.ogg` | An audio-only Opus source avoids copying VP9 into Ogg; 130 seconds covers the fixed 62.5-second seek plus 63.25 seconds. |
| 72 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 81 | `v2_video_only.mp4` | Try a video-only source so no audio encoder is resolved. If the fixed encoder option is still rejected, retain the diagnostic and no_input. |
| 82 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 84 | `v2_still_rgba.png`, `audio_aac.m4a` | The first input must be a loopable still image and the second a finite audio file; shortest bounds the loop. |
| 85 | `v2_still_rgba.png`, `audio_aac.m4a` | The first input must be a loopable still image and the second a finite audio file; shortest bounds the loop. |
| 104 | `v2_raw_yuv420p_1280x720.yuv` | The SAME INPUT placeholder is opened as 1280x720 yuv420p video and 48-kHz stereo s16le PCM. One deterministic byte file must be valid for both reads; do not split this into two assignments. |
| 113 | `v2_raw_yuv420p16le_1280x720.yuv` | Supply at least one complete 1280x720 planar 16-bit yuv420p16le frame. |
| 114 | `v2_video_long.mp4` | The registered seek is beyond the original two/three-second video fixtures. A 5027-second small H.264 video at 25 fps covers the largest seek, 01:23:45, and the short extraction windows. |
| 121 | None | A required numeric argument is a literal template or invalid size token; fixture contents cannot supply its value. |
| 128 | `v2_video_long.mp4` | The registered seek is beyond the original two/three-second video fixtures. A 5027-second small H.264 video at 25 fps covers the largest seek, 01:23:45, and the short extraction windows. |
| 129 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 133 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 137 | `v2_audio_mp3_long.mp3`, `v2_still_rgb.jpg` | The fixed maps require stream 0 of the first file to be MP3 audio and stream 0 of the second to be a JPEG cover image for MP3 stream copy. |
| 146 | `v2_raw_yuv420p_1920x1080.yuv` | Supply at least one complete 1920x1080 yuv420p frame. |
| 148 | None | A focused pinned-oracle smoke shows the pinned GIF command exits 0 but writes the literal OUTPUT%02d.gif. produced requires digits where %02d stands and therefore rejects that output. |
| 150 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 151 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 152 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 172 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 173 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 178 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 186 | None | The row requires a legacy/external AAC encoder, libfdk_aac or libfaac; 186 additionally needs a second-pass log. New source media does not install encoders or create pass state. |
| 189 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 190 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 191 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 192 | `audio_aac.m4a`, `v2_still_rgb.jpg` | Mapping input 1 first makes a single JPEG picture output stream 0, where attached_pic is applied. Input 0 supplies AAC audio. |
| 193 | None | The row has no OUTPUT token and an empty registered output; choose returns None before any fixture assignment is attempted. |
| 195 | None | The input option is protocol-specific, RTSP transport or HTTP headers, while the contract permits only local media files. |
| 197 | `v2_video_long.mp4` | The registered seek is beyond the original two/three-second video fixtures. A 5027-second small H.264 video at 25 fps covers the largest seek, 01:23:45, and the short extraction windows. |
| 206 | None | The literal output encoder names videocodec and audiocodec are unresolved templates. A media output needs at least one valid selected encoder. |
| 228 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 230 | `v2_mpeg_ps_video.mpg` | The forced mpeg input demuxer needs MPEG program-stream bytes, not MP4/Matroska/TS. |
| 233 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 235 | `v2_still_rgb.jpg` | A single-frame JPEG source fits the unnumbered OUTPUT.jpg; 235 must copy JPEG rather than an H.264 stream. |
| 245 | `v2_still_rgba.png` | image2pipe can read a complete encoded PNG from a regular file. No pipe or command rewrite is needed. |
| 250 | `v2_raw_bgr24_640x480.raw` | Supply a complete 640x480 packed bgr24 frame. |
| 257 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 260 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 261 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 262 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 266 | `v2_hls_local.m3u8` | allowed_extensions belongs to a playlist demuxer. A local HLS playlist plus local segments is a plausible offline source. |
| 275 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 276 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 288 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 298 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 300 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 301 | None | The pinned VP8 encoder rejects the fixed yuva420p output with its default auto_alt_ref setting: Transparency encoding with auto_alt_ref does not work. Media cannot change that encoder configuration. |
| 302 | `v2_still_rgba.png` | A PNG with alpha is a file-backed image2 source for yuva420p output. |
| 305 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 306 | `v2_audio_mp3_long.mp3`, `v2_still_rgb.jpg` | The fixed maps require stream 0 of the first file to be MP3 audio and stream 0 of the second to be a JPEG cover image for MP3 stream copy. |
| 308 | `basic.mp4`, `v2_raw_rgb32_800x550.raw` | Keep input 0 as ordinary media and supply input 1 as complete 800x550 rgb32 frames. |
| 309 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 312 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 313 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 314 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 315 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 316 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 317 | `v2_multi_audio_eng.mp4` | Use stable stream order v0,a0,a1,a2,a3 with four AAC audio tracks; absolute stream 3 is audio. This satisfies the missing absolute/typed map indices. |
| 318 | None | -s uses a multiplication sign in 320×240 rather than the dimension separator x, and singlejpeg is a legacy muxer spelling. A longer input cannot repair either argument. |
| 320 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 323 | None | The HLS segment filename is the literal FILE with no sequence template, while no single_file flag is set. New input media cannot change that output template. |
| 326 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 333 | `v2_still_rgba.png` | A still-image demuxer supports -loop 1; the registered -t bounds the output. |
| 337 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 338 | None | The fixed -vsync option is rejected by the pinned FFmpeg 9 oracle. A new source file cannot repair an unrecognized option. |
| 339 | None | The fixed -vsync option is rejected by the pinned FFmpeg 9 oracle. A new source file cannot repair an unrecognized option. |
| 345 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 347 | `v2_audio_opus_long.ogg` | An audio-only Opus source avoids copying VP9 into Ogg; 130 seconds covers the fixed 62.5-second seek plus 63.25 seconds. |
| 348 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 350 | None | The fixed -vsync option is rejected by the pinned FFmpeg 9 oracle; the legacy -vpre also needs preset state. Raw 720x576 bytes cannot repair either. |
| 358 | None | The output is pipe:1, but execute captures stdout only for output == -. produced then looks for a file literally named pipe:1, so fixture bytes cannot satisfy succeeded. |
| 361 | None | -stream_loop -1 has no duration/frame/output bound. Repeating a valid seekable file does not terminate within the registered reference timeout. |
| 363 | `v2_multi_audio_eng.mp4` | Use stable stream order v0,a0,a1,a2,a3 with four AAC audio tracks; absolute stream 3 is audio. This satisfies the missing absolute/typed map indices. |
| 365 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 367 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 372 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 373 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 376 | None | HEVC NVENC requires an available NVIDIA backend and its registered options; a new file cannot provide hardware or encoder support. |
| 380 | None | The fixed disposition:s:s:0 stream specifier repeats the stream type. The pinned oracle rejects it with Stream type specified multiple times, independently of subtitle media. |
| 383 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 389 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 391 | None | The row has no OUTPUT token and an empty registered output; choose returns None before any fixture assignment is attempted. |
| 393 | `v2_audio_mp3_long.mp3` | -map 0 and MP3 copy need an audio-only MP3 source, not video plus AAC. Metadata mapping to stream 0 is then defined. |
| 397 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 398 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 402 | `v2_multi_audio_eng.mp4` | Provide video and an AAC audio track tagged language=eng so the nonoptional metadata stream map matches. |
| 403 | None | The fixed HTTP headers option is not accepted for local file input, including a fully local HLS playlist. The pinned oracle rejects it with Option headers not found. |
| 409 | None | An output-only option is grouped before an input: -vframes for the raw-image rows, -shortest for 550. Input bytes cannot change option placement. |
| 410 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 416 | `v2_multi_audio_eng.mp4` | Use stable stream order v0,a0,a1,a2,a3 with four AAC audio tracks; absolute stream 3 is audio. This satisfies the missing absolute/typed map indices. |
| 418 | None | -b:v is the literal $bitrate, not a number; -pass 2 also requires a prior pass log that fixture staging does not provide. |
| 423 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 424 | None | There is no INPUT placeholder. Fixed ellipsis prose cannot be replaced by a fixture; 424 also uses OUTPUT as the value of -probesize. |
| 425 | None | There is no INPUT placeholder. Fixed ellipsis prose cannot be replaced by a fixture; 424 also uses OUTPUT as the value of -probesize. |
| 431 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 436 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 441 | `v2_audio_mp3_long.mp3`, `v2_still_rgb.jpg` | MP3 audio must survive the fixed 5-to-13-second input range; the second file must be a JPEG cover for the copied stream 1. |
| 442 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 444 | None | The row requires a legacy/external AAC encoder, libfdk_aac or libfaac; 186 additionally needs a second-pass log. New source media does not install encoders or create pass state. |
| 445 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 448 | None | An output-only option is grouped before an input: -vframes for the raw-image rows, -shortest for 550. Input bytes cannot change option placement. |
| 449 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 450 | `v2_raw_yuv420p_400x300_long.yuv` | Use a sparse zero-filled 4352-second 400x300 yuv420p file at the raw demuxer default 25 fps. Preserve sparse extents while staging; no timeline header is fabricated. |
| 453 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 459 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 462 | None | The wallclock input option replaces packet timestamps with epoch time. AVI rejects the resulting frame gap as Too large number of skipped frames for both H.264 video and MP3 audio; media bytes cannot change that clock. |
| 465 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 466 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 472 | `v2_still_rgba.png` | A still-image demuxer supports -loop 1; the registered -t bounds the output. |
| 486 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 487 | `basic.mp4`, `v2_subtitles.srt` | The second input must be a real SRT subtitle file; the original pool has no subtitle streams. |
| 490 | None | Forced rawvideo input has no video_size/-s. Headerless raw bytes cannot declare their own geometry; output pixel-format options do not provide the input size. |
| 492 | `v2_video_long.mp4` | A source extending beyond five seconds avoids shifting every original frame before zero with -itsoffset -5. |
| 493 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 494 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 495 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 500 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 506 | `v2_still_rgba.png`, `audio_aac.m4a` | Use a loopable still plus a finite audio file copied to Matroska. |
| 508 | `v2_audio_mp3_long.mp3`, `v2_still_rgb.jpg` | The fixed maps require stream 0 of the first file to be MP3 audio and stream 0 of the second to be a JPEG cover image for MP3 stream copy. |
| 520 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 521 | `v2_still_rgba%03d.png` | Use an image2-selected percent-pattern PNG plus its numbered sibling. Glob inputs match the real percent-named PNG; sequence input reads the numbered frame. |
| 522 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 528 | None | A focused pinned-oracle smoke shows the pinned reference rejects the forced u16le muxer with the explicitly selected pcm_s16le codec as incompatible. |
| 529 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 530 | None | The row uses a legacy option or stream-map spelling, such as sameq, updatefirst, vfilters, newvideo/newaudio or 0.0. A media-shape change does not implement that option. |
| 535 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 537 | None | The literal activation_bytes value secret is not a synthetic media-shape requirement. Do not introduce DRM-protected media or credentials to satisfy it. |
| 540 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
| 547 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 548 | `v2_subtitles_movtext.mp4` | A focused pinned-oracle smoke shows a focused pinned reference succeeds with a synthetic mov_text MP4 and mov2textsub into rawvideo. The missing subtitle-bearing source is a credible file-shape gap. |
| 550 | None | An output-only option is grouped before an input: -vframes for the raw-image rows, -shortest for 550. Input bytes cannot change option placement. |
| 551 | None | Legacy -vpre/-fpre preset files, encoder names or old encoder options require non-media state or option support. A fixture alone is not a defensible repair. |
| 554 | `v2_video_only.mp4` | Use a video-only H.264 file to remove the selected audio job without changing any option. |
| 557 | `v2_multi_audio_eng.mp4` | Use stable stream order v0,a0,a1,a2,a3 with four AAC audio tracks; absolute stream 3 is audio. This satisfies the missing absolute/typed map indices. |
| 560 | None | A required numeric argument is a literal template or invalid size token; fixture contents cannot supply its value. |
| 565 | `v2_still_rgb.jpg` | A single-frame JPEG source fits the unnumbered OUTPUT.jpg; 235 must copy JPEG rather than an H.264 stream. |
| 566 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 576 | `v2_still_rgba.png` | image2pipe can read a complete encoded PNG from a regular file. No pipe or command rewrite is needed. |
| 577 | `v2_still_rgba.png` | A single image file lets image2 consume the registered framerate or image demuxer options. An exact filename also matches a glob or single-file sequence. |
| 582 | `v2_still_rgba.png`, `audio_aac.m4a` | The first input must be a loopable still image and the second a finite audio file; shortest bounds the loop. |
| 583 | None | The fixed -vsync option is rejected by the pinned FFmpeg 9 oracle. A new source file cannot repair an unrecognized option. |
| 584 | None | A required numeric argument is a literal template or invalid size token; fixture contents cannot supply its value. |
| 587 | `v2_audio_mp3_long.mp3` | MP3 stream copy needs an MP3 audio-only source; all original audio fixtures are AAC or Opus. Use a 480-second source for every fixed seek/range in this family. |
| 589 | None | -s is literal ${VRES}x${HRES}; output-only -vframes also stands before the input. A raw file cannot repair these arguments. |
| 590 | None | An output-only option is grouped before an input: -vframes for the raw-image rows, -shortest for 550. Input bytes cannot change option placement. |
| 591 | None | The fixed relative ./flash-mp4.ffpreset is resolved in an empty output working directory. stage copies only INPUT files elsewhere; it cannot populate this non-input file. |
| 592 | `v2_subtitles.srt` | A focused pinned-oracle smoke shows a focused pinned reference succeeds copying an SRT subtitle source to the fixed rawvideo muxer with map 0:s. |
| 605 | `v2_video_hevc.mp4` | The existing concat adapter must list a HEVC-bearing MP4 twice for hevc_mp4toannexb; H.264 input is not sufficient. |
| 613 | `v2_multi_audio_eng.mp4` | 0:a:1 requires at least two source audio tracks. With four tracks, -c:a:2 aac applies to the third original audio output; all other tracks remain copy jobs. |
| 617 | None | A forced lavfi input consumes a filtergraph expression, not a media filepath. No normal synthetic media file can satisfy this under the required file-only placeholder contract. |
| 620 | None | The forced capture input is a live device, display or audio server, not a media file. No file-only fixture assignment rescues it. |
| 621 | None | Literal prose, ellipses, shell fragments or unregistered extra output tokens remain in argv. A new INPUT file cannot repair these tokens. |
