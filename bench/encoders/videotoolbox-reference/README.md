# M1 VideoToolbox reference figures

These are hardware-encoding reference figures, not a judged verdict or a general speed claim. On these nine prepared clips, the median per-input CoreAV/FFmpeg process-time ratio was **1.079**. CoreAV took more wall time on all nine inputs, while the decoded quality measurements were close.

| Maximum-height class | CoreAV fps | FFmpeg fps | CoreAV/FFmpeg time | PSNR dB, CoreAV / FFmpeg | SSIM, CoreAV / FFmpeg |
|---|---:|---:|---:|---:|---:|
| 360 | 699.06 | 781.73 | 1.113 | 43.79 / 43.78 | 0.98907 / 0.98906 |
| 720 | 310.29 | 334.77 | 1.079 | 37.86 / 37.86 | 0.98630 / 0.98630 |
| 1080 | 173.93 | 180.88 | 1.041 | 37.83 / 37.81 | 0.97553 / 0.97553 |

Each row is the median of three per-input figures, not a pooled frame result. Each input's timing is the median of three interleaved calls per arm after one warmup per arm. The 360 class contains 320×240 and 608×342 inputs, not exact 360p video. All 54 timed calls succeeded. The final output from each arm and input decoded to its complete expected frame count and dimensions: 18 checked outputs, 4,848 decoded frames.

## Method and limits

The host was a dedicated Mac mini M1, eight physical cores and 8 GiB RAM, running macOS 15.8.2, build 24H232. CoreAV used the existing dist standalone exporter built from `7abf723411f9dacb6e8f5dca148fa5ea03b62932`; its [build receipt](../../../docs/design/encoders/macos-builds/macos.json) records the binary. The comparison used the pinned macOS arm64 FFmpeg 9.0.2 binary and `h264_videotoolbox`, with software fallback disabled. The measured FFmpeg hash matched the [canonical macOS pin at the measurement revision](https://github.com/coreav-dev/coreav/blob/115cf3ed9d3dba81a4f1ea53775611f2a7dd6cb8/scripts/oracle-ffmpeg-macos.env). CoreAV reported hardware encoding.

The nine clips were already prepared I420 references from lossy sources: positions 1, 5 and 10 within each height class of the existing ordered [input list](../../../scripts/encoders/macos/videotoolbox-sources.json), totaling 2,424 frames. No new media was acquired. Both arms used High profile, no B-frames, a two-second keyframe interval and target bitrates of 1, 3 and 6 Mbit/s by class. Caller thread limits were one; framework-internal concurrency was not controlled.

Timing covers each complete encoding process, including startup, raw-file reading, output and report writing. It excludes preparation, transfer, hashing, decoding and quality measurement. This is not an integrated transcode or isolated encoder-kernel benchmark. Cache residency, thermal state and scheduling were not audited. Host load averages were 1.33 before and 1.94 after this short session.

PSNR and SSIM compare the final timed output's decoded pictures against the same prepared I420 frames, in frame order. They do not establish original-source colour preservation or timing conformance. Per-input PSNR ranged from 23.19 to 46.69 dB. The largest CoreAV/FFmpeg differences were 0.027132 dB PSNR and 0.000223 SSIM; actual per-input bitrates differed by less than 0.45% between arms. No confidence interval, quality threshold or acceptance gate is applied.

Software figures are omitted because CoreAV refused an encoder-list entry with no hardware flag. That refusal does not establish that a software encoder is absent.

[calls.csv](calls.csv) contains every timed call, including the CoreAV executable SHA-256 captured before this session, CPU time, output size and exit status. The CoreAV hash column is empty for FFmpeg rows, whose identity is the canonical pin above. CPU time is the encoding child's user and system time, excluding separate framework services. [quality.csv](quality.csv) contains each checked output's observed dimensions, frame count, actual bitrate and distortion measurements; its raw hashes identify the prepared inputs without distributing media.
