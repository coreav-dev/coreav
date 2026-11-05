# M1 VideoToolbox HEVC reference figures

These are hardware HEVC Main reference figures, not a judged verdict or a general speed claim. Across nine prepared clips, the median per-input CoreAV/FFmpeg process-time ratio was **1.098**: CoreAV took about 9.8% more wall time and was faster on zero of nine inputs. Decoded PSNR and SSIM were close, but actual bitrates were not identical despite matched targets.

| Maximum-height class | CoreAV fps | FFmpeg fps | CoreAV/FFmpeg time | PSNR dB, CoreAV / FFmpeg | SSIM, CoreAV / FFmpeg |
|---|---:|---:|---:|---:|---:|
| 360 | 555.82 | 642.10 | 1.133 | 44.27 / 44.22 | 0.98895 / 0.98895 |
| 720 | 247.59 | 271.83 | 1.098 | 38.10 / 38.10 | 0.98638 / 0.98638 |
| 1080 | 158.31 | 166.45 | 1.051 | 37.90 / 37.89 | 0.97666 / 0.97660 |

Each row is the median of three per-input figures, not pooled frames or a ratio of class-median times. Each input's time is the median of three interleaved calls per arm after one warmup per arm. All 18 warmups and 54 timed calls succeeded. The final output from every arm/input pair decoded to its expected dimensions and complete frame count: 18 outputs, 4,848 decoded frames.

## Method and limits

The host was a dedicated Mac mini M1, eight physical cores and 8 GiB RAM, running macOS 15.8.2, build 24H232. CoreAV used the dist standalone exporter at [the measured revision](https://github.com/coreav-dev/coreav/commit/7f981d5e7cf8908f9dcfeeea14875f64c58138aa), built for generic arm64 with minimum macOS 15.0. The binary's pre-execution SHA-256 is retained in the CoreAV call rows. This measures that development revision, not an unspecified later build.

The baseline was the pinned macOS arm64 FFmpeg 9.0.2 binary with `hevc_videotoolbox`. Its measured executable hash matched the [canonical macOS pin at the measurement revision](https://github.com/coreav-dev/coreav/blob/7f981d5e7cf8908f9dcfeeea14875f64c58138aa/scripts/oracle-ffmpeg-macos.env). Software fallback was disabled; CoreAV reported hardware encoding. Software HEVC and Main10 were not measured.

The inputs are the same nine prepared I420 clips as the [H.264 reference](../videotoolbox-reference/README.md): positions 1, 5 and 10 within each covered height class of the existing [input list](../../../scripts/encoders/macos/videotoolbox-sources.json), totaling 2,424 frames. They came from lossy sources; no new media was acquired. The 360 class contains 320×240 and 608×342 frames, not exact 360p. Both arms requested Main profile, 8-bit 4:2:0, BT.709 limited range, no B-frames, a two-second maximum keyframe interval and 1, 3 or 6 Mbit/s by class. Caller thread limits were one; framework-internal concurrency was uncontrolled.

Timing covers the complete encoding process, including startup, raw-file reading, output and report writing. It excludes transfer, hashing, decoding and quality analysis. This is neither integrated transcode nor isolated encoder-kernel timing. Cache residency, thermals and scheduling were not audited; the initial host load average was 1.03. No limiter was profiled, confidence interval computed or acceptance threshold applied.

Analysis stopped after the first two inputs because the null-output sink used a guessed frame rate and reported duplicate DTS. Its first correction used an incompatible combination of output-rate options. Both analysis attempts are retained. All 16 successful encoder calls from those inputs were preserved without repetition; only the 56 missing calls ran afterward. Quality was recomputed for every final output with the same corrected method. The interruption limits comparisons across inputs, even though each input's paired calls remained interleaved.

PSNR and SSIM compare decoded output against prepared I420 in frame order. Both filter inputs receive the same exact rational frame-index clock; the null sink uses passthrough and that explicit encoder timebase, rather than a guessed elementary-stream rate. This establishes neither original-source colour preservation nor container timing conformance. Every corrected analysis completed with the expected metric-frame counts and no error-level or mismatched-timebase diagnostic. Per-input PSNR ranged from 33.64 to 46.35 dB. The largest arm differences were 0.062507 dB PSNR and 0.000673 SSIM. Final-output bitrates differed by up to 8.704% relative to FFmpeg, so these are matched-target settings, not equal-bitrate quality comparisons.

[calls.csv](calls.csv) contains all 72 encoder calls, including warmups, individual wall times, CPU times, sizes and exit statuses. Its CoreAV hash is empty for FFmpeg rows, whose identity is the immutable pin above. The retention flag marks calls completed before the analysis recovery. CPU time covers the child process, excluding separate framework services. [quality.csv](quality.csv) binds each final timed output to its encoded and prepared-raw hashes and retains observed decoded dimensions, frame counts, bitrate and metric counts without distributing media.
