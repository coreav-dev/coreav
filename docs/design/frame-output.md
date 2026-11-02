# Image and raw video output

`coreav ffmpeg` writes decoded video as PNG, BMP, packed raw video or YUV4MPEG2. The input remains an MP4, MOV, Matroska or WebM file read by CoreAV. Decoder backend features and strict-safe restrictions are unchanged.

## Correctness bars

These bars are committed before the comparisons that judge this change. No speed or compression-size advantage is claimed.

- A PNG or BMP decoded by the pinned oracle must reproduce every input image sample bit for bit. For video converted to RGB, the reference samples are CoreAV's existing frame conversion, not the compressed source's YUV samples.
- Native raw video must match the pinned FFmpeg output byte for byte when both decoders produce the requested format. Padding in a decoder's planes must never enter the output.
- For the same command and input, compare the output's codec, dimensions and pixel format with the pinned FFmpeg's `ffprobe`. Compare sequence names, frame count and frame selection too. An unsupported command is a refusal, not a passing comparison.
- YUV4MPEG2 must decode to the same packed samples as raw output and report the source frame rate, aspect ratio and chroma layout. A format or timing the header cannot describe is refused.
- Command tests must prove that strict-safe mode never invokes FFmpeg, unsupported conversion is refused before output creation, and overwriting never replaces the input.

## Ownership and limits

The thumbnail crate owns the image encoders. The existing PNG writer remains the only PNG writer. The compatibility command owns image sequence names, frame limits, output seeking and frame-rate selection, independently of the image codec.

BMP follows Microsoft's [BITMAPINFOHEADER documentation](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapinfoheader): one plane, 24 bits per pixel, BI_RGB, positive height for bottom-up rows and DWORD row alignment. PNG retains the existing ISO/IEC 15948 writer, with channel-aware filter distances, big-endian 16-bit samples and an optional `pHYs` aspect ratio.

Raw output accepts only the decoded pixel format. There is no new colour conversion or resampler. Images reuse CoreAV's existing RGB conversion. Output rates may drop frames, but may not require duplicated frames. Image output accepts `%d` and `%0Nd` names with one to nine padding digits; numbering starts at one. `-update 1` replaces one image after each selected frame.

The packet reader supplies presentation seconds relative to the input start, including leading empty edits. Frame selection never treats a media timestamp as a presentation timestamp. This retains one optional rational per packet that the thumbnail planner already computed. Stream-copy commands add an output-family/default-codec check; their instruction-count guard and tolerance are unchanged.

The output loop drains ready frames before sending more packets. Pacing starts at 64 pending packets or one quarter of the decoder's pending-byte budget. A five-second no-frame stall clears the pacing estimate and starts a fresh observation interval, because a decoder may consume packets without producing pictures. Decode limits still bound the worker itself. The default decode limits apply, including 60 seconds per call and 4 GiB of decoded samples. File outputs use a temporary file; image sequences commit each image independently, and a pipe cannot be rolled back. After any output is emitted, an error can no longer become an FFmpeg fallback.

Frame limits and the exclusive end of a time window stop decoding once reached. Geometry is checked only on selected frames. Image conversion uses `resize_to_rgb` at the coded size with `ChromaUpsampling::FfmpegCompatible`. A quarter-turn of 4:2:2 converts to RGB first, as the thumbnail renderer does. YUV4MPEG2 requires a known constant frame rate, known progressive video and 8-bit planar samples. Autorotation is refused for YUV4MPEG2 rather than declare an incorrect rotated chroma location; `-noautorotate` keeps the stored planes and their original location.

Output-rate selection is limited to a video starting at the input's time zero, with constant presentation spacing. Each frame is judged at its end time in output-rate ticks; it is dropped when that end is more than 1.1 ticks behind the next output tick. This reproduces the pinned oracle's observed frame choices at lower rates without adding duplicate frames. Higher rates and delayed video starts are refused.

## Local correctness comparison

The pinned oracle is FFmpeg `n9.0.2-17-g2a571b6068-20260930`. The source is a two-second, 320 by 240, 25-fps `testsrc2` scene encoded with libx264 (`yuv420p`, GOP 25, one encoder thread), with a 440-Hz AAC sine track. Its SHA-256 is `529b0d0802c7a01de1cfd5c81811ffc2b7cec469e91cb11a7d3dff77e6a928e4`.

Nine of 19 selected command shapes execute natively. The raw pipe, raw file and YUV4MPEG2 decode match FFmpeg's samples byte for byte; a PNG packet output whose seek is beyond EOF agrees on an empty file. The other five outputs match sequence names, frame counts and the compared stream fields (codec, width, height, pixel format, sample/display aspect ratio and frame rate), but not exact RGB pixels. The remaining commands require resizing, frame duplication, other encoded containers, audio conversion or multiple inputs. These are refusals, not compatibility passes.

Across all 50 frames (3,840,000 pixels), the maximum absolute RGB differences are `(2, 3, 2)`. At least one channel differs in 3,731,998 pixels (97.1874479%). Per-channel PSNR is `(47.664325, 47.939329, 50.963785)` dB; combined PSNR is 48.621797 dB. PSNR uses `10 log10(255² / MSE)` over unmodified 8-bit samples, without alignment, scaling or quantization. These figures diagnose the existing conversion; they are not an acceptance tolerance.

The source reports left chroma location and no matrix or range tag. CoreAV uses its existing unspecified-matrix fallback (BT.601), limited-to-full range mapping and the unscaled FFmpeg-compatible chroma path (nearest sample replication). FFmpeg's default output is byte-identical to an explicit `scale=iw:ih:in_color_matrix=bt601:in_range=tv:out_range=pc` conversion with each of `flags=neighbor`, `flags=bilinear` and `flags=bicubic`; this establishes observable equivalence on this fixture, not which internal path FFmpeg used. Exact converter parity is separate work. The PNG and BMP encoder tests also pass arbitrary native image samples through the pinned FFmpeg decoder and require bit identity.

AVI, FLV and Matroska encoding, JPEG encoding through this command, audio, bitstream filters, and input raw-video demuxing are outside this change.
