# Buffered VideoToolbox export reference figures

On these nine prepared clips, buffering JSON reports reduced median per-input process time by 7.8% for H.264 and 6.2% for HEVC. Every final old/new report and encoded-output pair was byte-identical. These are reference figures, not significance tests or general speed claims.

| Codec | New/old time | New/FFmpeg time | Old/FFmpeg time | New has lower median than old | New has lower median than FFmpeg |
|---|---:|---:|---:|---:|---:|
| H264 | 0.921725 | 1.000641 | 1.082459 | 9/9 | 3/9 |
| HEVC | 0.938116 | 1.015599 | 1.094694 | 9/9 | 1/9 |

## Class figures

| Codec | Maximum-height class | Old fps | New fps | FFmpeg fps | New/old time | New/FFmpeg time | Old/FFmpeg time |
|---|---:|---:|---:|---:|---:|---:|---:|
| H264 | 360 | 675.32 | 765.34 | 747.32 | 0.902762 | 0.981018 | 1.106606 |
| H264 | 720 | 310.64 | 337.02 | 337.23 | 0.921725 | 1.000641 | 1.085618 |
| H264 | 1080 | 173.89 | 180.51 | 180.60 | 0.963320 | 1.001629 | 1.038587 |
| HEVC | 360 | 528.86 | 606.08 | 610.06 | 0.880432 | 1.006574 | 1.130989 |
| HEVC | 720 | 246.75 | 262.72 | 270.12 | 0.938116 | 1.026950 | 1.094694 |
| HEVC | 1080 | 156.81 | 162.66 | 164.80 | 0.963274 | 1.012954 | 1.051001 |

Each input's time is the median of three timed calls after one warmup per arm. Class and overall ratios are medians of per-input ratios, not ratios of class-median times. Fps columns are medians of per-input fps. Lower time ratios favor the numerator. H.264's overall new/FFmpeg ratio is close to one, and HEVC's is about 1.016; the remaining gap is not attributed to VideoToolbox internals.

## Revisions and method

The old CoreAV revision was reviewed [089e96cb](https://github.com/coreav-dev/coreav/commit/089e96cb31db1f722f05339e90688c6a2d96013b). The new revision was [f220c719](https://github.com/coreav-dev/coreav/commit/f220c7195a766b33d6b7ebda4ca33e8d76f468ae), **unreviewed at launch**, from the [report-buffering change](https://github.com/coreav-dev/coreav/pull/415). Both uninstrumented standalone exporters used locked generic arm64 dist builds, Rust 1.99.1, minimum macOS 15.0 and SDK 27.2. Their observed executable hashes are in the call records.

The baseline was pinned macOS arm64 FFmpeg 9.0.2 with `h264_videotoolbox` or `hevc_videotoolbox` and software fallback disabled. Its observed binary matched the [canonical macOS pin at the measurement revision](https://github.com/coreav-dev/coreav/blob/f220c7195a766b33d6b7ebda4ca33e8d76f468ae/scripts/oracle-ffmpeg-macos.env). The host was a dedicated Mac mini M1 with eight physical cores and 8 GiB RAM, macOS 15.8.2 build 24H232. Load averages were 1.12 before and 1.71 after the approximately five-minute session.

The same nine lossy-derived prepared I420 clips were positions 1, 5 and 10 within each height class of the existing [input list](../../../scripts/encoders/macos/videotoolbox-sources.json), totaling 2,424 frames. The 360 class contains 320×240 and 608×342 images, not exact 360p video. No new media was acquired. Controls were hardware-only H.264 High or HEVC Main, BT.709 limited range, no B-frames, a two-second GOP and class targets of 1, 3 and 6 Mbit/s. CoreAV used one submitting thread; FFmpeg requested `-threads 1` before its raw input. Framework-internal concurrency and FFmpeg's pipeline workers were not controlled.

Arm order rotated across old, new and FFmpeg. All 216 encoder calls succeeded: 54 warmups and 162 timed calls. The unchanged exit-event clock measured the complete child process, including startup, raw reads, pixel conversion, elementary output, JSON reporting and teardown. Preparation, transfer, hashing, decoding and quality analysis are outside that clock. No profiler or instrumentation was attached. Cache residency, thermal state and scheduling were not audited, and no confidence interval, acceptance threshold or conformance verdict is claimed. The change removes exporter reporting overhead; it does not demonstrate a faster native codec implementation.

## Output and quality

All 54 final outputs decoded to their expected geometry and frame count, totaling 14,544 decoded frames. PSNR and SSIM each contain 14,544 frame records. Every one of the 18 final old/new native output pairs and reports was byte-identical. CoreAV's parameter-set and picture checks remained enabled before output delivery.

| Codec | Maximum-height class | PSNR dB, new / FFmpeg | SSIM, new / FFmpeg |
|---|---:|---:|---:|
| H264 | 360 | 43.7944 / 43.7755 | 0.989068 / 0.989059 |
| H264 | 720 | 37.8568 / 37.8568 | 0.986297 / 0.986297 |
| H264 | 1080 | 37.8319 / 37.8070 | 0.975525 / 0.975525 |
| HEVC | 360 | 44.2721 / 44.2172 | 0.988946 / 0.988946 |
| HEVC | 720 | 38.0980 / 38.0980 | 0.986375 / 0.986375 |
| HEVC | 1080 | 37.8988 / 37.8878 | 0.976662 / 0.976604 |

Quality compares decoded pictures against the prepared I420 frames, not original-source colour or integrated-transcode timing. Both filter inputs use `settb=1/num,setpts=N*den`; the null output uses `-fps_mode passthrough -enc_time_base 1/num`, without output `-r`. Every metric file has exactly one sequential record per expected frame, with no error-level or timebase-mismatch diagnostic.

Actual bitrates are not equal merely because targets match. The largest new/FFmpeg bitrate difference was 0.442% for H.264 and 8.704% for HEVC. HEVC 720-3 produced approximately 2.9348 versus 2.6998 Mbit/s at the same 3 Mbit/s target. Maximum new/FFmpeg quality differences were 0.027132 dB PSNR and 0.000223 SSIM for H.264, and 0.062507 dB and 0.000673 for HEVC. These are matched-target comparisons, not equal-bitrate quality claims.

## Quality-clock correction

The earlier [H.264 reference](../videotoolbox-reference/README.md#quality-clock-correction) used different filter timebases and emitted mismatch warnings. Its 720-1 PSNR/SSIM pair for each arm was misaligned despite matching frame counts. All 18 original final encoded files and prepared input identities were checked against this run's corresponding old/FFmpeg files and were byte-identical. The common-clock analysis therefore also corrects that earlier pair. All other earlier quality scalars, timing calls, class medians and encoded bytes are unchanged; no encoder call was added for the correction.

## Data

[calls.csv](calls.csv) contains all 216 calls. Repeat 0 is the warmup; repeats 1–3 are timed. It retains wall/child CPU time, output size, actual bitrate, status and output hash. The CoreAV executable-hash column is empty for FFmpeg, whose identity is the canonical pin above. Child CPU time excludes separate framework services. Per-input ranges can be recomputed directly from these calls.

[quality.csv](quality.csv) contains all 54 final outputs, their raw/output identities, observed decoded geometry, profile and colour fields, actual bitrate and PSNR/SSIM frame counts. The prepared-input hashes identify the inputs without distributing media.
