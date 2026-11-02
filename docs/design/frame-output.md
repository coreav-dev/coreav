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

Raw output accepts only the decoded pixel format. There is no new colour conversion or resampler. Images reuse CoreAV's existing RGB conversion. Output rates may drop frames, but may not require duplicated frames. Image output accepts `%d` and `%0Nd` names with one to nine padding digits; numbering starts at one. `-update 1` replaces one image after each selected frame.

AVI, FLV and Matroska encoding, JPEG encoding through this command, audio, bitstream filters, and input raw-video demuxing are outside this change.
