# Opt-in H.264 encoding through the CLI

## Selection

`coreav --native-h264-encoder ffmpeg -i in.mp4 -an -c:v libx264 -crf 23 out.mp4` prefers CoreAV's encoder. The `native-h264-encoder` Cargo feature enables the flag and the library wiring. The feature and the `coreav-h264-encoder` crate remain absent from default, release-default, and release-full builds. Both release feature sets already include the shared `transcode` dependency. A decoder backend remains a separate opt-in.

The selector belongs to CoreAV, before the subcommand. A new codec name would require rewriting fallback arguments. A flag inside FFmpeg's arguments could collide with an option value or filename. An environment variable would make selection implicit. The outer flag avoids all three problems. Without the flag, the existing path remains unchanged, including in a feature-enabled build.

## Admission and fallback

The existing argument parser and transcode planner remain authoritative. Native selection requires explicit effective `-c:v libx264`, a file output in MP4, and no unsupported encoder options. Integer `-crf` values from 10 through 51 map to `RateMode::Crf`; integer `-qp` values in the same range map to `RateMode::FixedQp`. Neither option means CRF 23. Both together, fractional values, bitrate, VBV, profiles, B-frame controls, presets, filters, and other unmodeled controls keep the existing path. The original FFmpeg arguments are never rewritten for fallback.

Verification narrowed the initial MP4 and Matroska design to MP4. The existing Matroska cadence calculation omits `DefaultDuration` when rounded nanosecond intervals differ. Pinned ffprobe then reports 60 fps for the native stream instead of 30. Matroska therefore keeps the existing external path; correcting its shared cadence calculation is separate work.

Decoded pictures must be progressive planar 8-bit 4:2:0, with even dimensions from 16 through 1920 horizontally and 16 through 1080 vertically. The source must also fit those dimensions. The [metadata design](cli-native-h264-metadata.md) admits supported positive constant rational frame rates from the exact presentation clock and retains declared non-square sample aspects in both VUI and MP4. Variable or rounded alternating clocks and unrepresentable ratios keep the external path. Absent, unspecified and square aspects preserve the former native signalling. Unsupported color descriptions also keep the external path. The adapter maps absent color code points to unspecified, without converting samples.

Native output uses Constrained Baseline, enabled deblocking, and an IDR interval of 250. The native encoder receives the requested thread count, but its output cannot depend on that count. Existing decoding policy, staging, spool limits, timestamps, audio handling, and muxing are reused. Strict-safe selection retains today's path and policy rather than changing decoder or process authorization.

Native admission and library failures are transactional. Before publication, an unsupported frame or library refusal discards the native staging and spool files, then reruns the unchanged command through today's external encoder or FFmpeg fallback. I/O, malformed-input, and deadline failures remain terminal. A default command never enters this retry path.

The native path checks its deadline before and after each bounded picture. It does not interrupt a picture already executing. One original command deadline covers both CoreAV encode attempts; a coding-limit retry receives only the remaining budget. A native timeout remains terminal, even if an external encoder could have completed the input within that limit.

## Verification

- Compare default-build command output with the pre-change binary, and compare feature-enabled commands without the flag with today's external path.
- Exercise every excluded option and input category, requiring unchanged fallback arguments and byte-identical output with and without the flag.
- Encode CRF and fixed-QP boundary values through the CLI, check emitted syntax and clocks, and decode the stored output with pinned FFmpeg 9.0.2 and CoreAV.
- Compare complete native MP4 bytes across thread counts, retain exact external Matroska output, and run the existing CLI limit and timeout tests.
- Check default and release feature graphs, generated command documentation, public text, DCO, CI, and exact-head similarity.

Speed and quality comparisons are outside this change. Matroska output admission remains separate work.
