# Constant cadence and sample aspect ratio

Implement Rec. ITU-T H.264 (08/2024), Annex E.1.1 and E.2.1, Table E-1 and Table E-6.

Add checked `FrameRate` and `SampleAspectRatio` values to public `Settings`. Reduce positive rational inputs before checking the 32-bit timing and 16-bit Extended_SAR bounds. For progressive frames, write `num_units_in_tick = den`, `time_scale = 2 * num`, and `fixed_frame_rate_flag = 1`. Check the level 5.1 macroblocks-per-second limit against the actual rational cadence.

Use Table E-1 for matching aspect ratios, otherwise Extended_SAR. Explicit square pixels use entry 1. The default aspect ratio preserves the existing absent signalling, which means unspecified in the specification; callers requiring explicit 1:1 use the checked constructor. The default timing remains 30/1, preserving existing encoded bytes.

Tests independently inspect SPS fields and decode complete pictures with the native decoder, which consumes but does not retain timing or aspect metadata. A generated-fixture test compares rate and SAR with the qualified ffprobe pair on Actions. No CLI, container cadence, variable frame rate, or decoder API changes.
