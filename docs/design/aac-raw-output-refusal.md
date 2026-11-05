# AAC raw output refusal

The most common refused audio shape in the GitHub command-shape sample requests one-channel, 48 kHz `pcm_f32le` output from an MP4 input, with one input decoder thread. Its generated representative contains AAC. The existing decoder eligibility record does not admit AAC, so this command must remain a refusal rather than publish samples that are not proven equal to pinned FFmpeg 9.0.2.

## Plan

1. Reproduce this command on a small generated AAC MP4 and the pinned binary, without running the frozen measurement or changing its records.
2. Name the lack of exact AAC decoding in the raw-output refusal, keeping the existing decoder admission boundary and every eligibility record unchanged.
3. Add a generated-fixture oracle regression for F32 and S16 output to stdout and files. Strict-safe must exit 4, emit no stdout, preserve existing destinations and leave no staging files. Normal mode must delegate the unchanged command to the pin and reproduce its complete bytes.
4. Retain a lossless native control so this refusal cannot become a blanket ban on raw audio output. Run the focused CLI tests and public-text checks before opening the pull request.

This change does not implement AAC decoding, encoding, resampling or channel mixing. It changes no frozen shape, judging rule, tolerance, baseline or historical result. Refs coreav-dev/coreav#25.
