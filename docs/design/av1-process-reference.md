# AV1 process-boundary reference

## Question and scope

Measure the end-to-end H.264-to-AV1 time attributable to the software encoder process boundary, and characterize the remaining difference against FFmpeg. These are reference figures, not a performance acceptance test or authorization to change software encoder isolation.

The method is committed before opening any measurement input. Production defaults, release artifacts, encoder tuning, decoder algorithms, and frame transport remain unchanged.

## Arms

1. CoreAV native H.264 decoding with SVT-AV1 in the existing sandboxed process.
2. The same CoreAV decoding and output validation with SVT-AV1 in-process, selected only in a measurement build.
3. FFmpeg at the revision declared in `scripts/oracle-ffmpeg.env`, built as a test-only black box with the same SVT-AV1 source revision as CoreAV.

Use SVT-AV1 4.2.0 from the existing pinned recipe for all arms. Record the exact source commit, build commands, executable identities, and reported library versions. Build FFmpeg without documentation and remove installed examples. Do not inspect its source.

Use preset 8, integer CRF 32, tune 1, adaptive quantization 2, parallelism level 1, 8-bit planar 4:2:0, the unchanged rational source cadence, and matching closed-GOP intervals and colour declarations. Disable audio, scaling, rotation, and implicit frame duplication. All arms encode one continuous session per input. CoreAV normally bounds sessions by its retained-input envelope and five-second interval. A measurement-only configuration must retain all submitted pictures until native destruction, with a checked finite memory envelope, rather than releasing input storage early. Thus the sandbox arm preserves process isolation but deliberately removes production session segmentation for this comparison.

The hidden in-process selector and continuous-session configuration require a non-default Cargo feature. Default builds cannot select them. Release and distribution profiles must reject that feature. A separate optimized measurement profile enables the experiment without changing release flags or default features. Check both feature absence and explicit release-profile refusal.

The in-process arm gives up crash isolation, a separately enforceable address-space limit, and the ability to kill a stuck encoder without killing the caller. Its native calls cannot be interrupted safely. The capture driver therefore puts the complete command in its own process group and enforces an external timeout. None of these experimental failure semantics apply to production builds.

## Inputs and host

Select three to five existing lossy H.264 inputs of at least 30 seconds, spanning available resolutions. Prefer fixed even geometry, SDR, planar 8-bit 4:2:0, and a constant cadence that both pipelines can preserve without conversion. Select by metadata before timing, record every admission refusal, and do not replace an input because of its speed. Use the whole admitted input or a single preselected stream-copy interval of at least 30 seconds. Record private object keys outside the repository. Public evidence uses neutral labels, content hashes, durations, frame counts, and geometry only.

Use one dedicated on-demand c4-standard-8-class x86-64 Linux VM with simultaneous multithreading disabled. Record actual CPU topology, machine configuration, affinity, toolchain, load, memory, and storage. No competing measurement runs. Keep input and output files local. Bound the VM lifetime and all transfers before provisioning. Run a synthetic smoke check before the input comparison.

Keep CoreAV's native decoder host-core default and set FFmpeg's decoder worker request to the same visible CPU count. Both encoders retain parallelism level 1, which is not a one-thread limit. The optimized measurement profile inherits the distribution profile's fat LTO and single code-generation unit. Its debug information does not enable debug assertions.

The test-only musl FFmpeg build requests a 2 MiB default thread stack. A synthetic initialization failed with musl's smaller default and succeeded when only the ELF stack size changed. The final comparator is rebuilt with the linker option, not patched. CoreAV's Rust encoder actor also has a 2 MiB stack. This is a build compatibility requirement, not an encoder setting change.

SVT's textual `keyint` is the GOP length, while CoreAV supplies `intra_period_length` directly. Pass CoreAV's recorded key interval plus one to FFmpeg's SVT parameter parser. A direct synthetic worker reported a GOP length of 126 for CoreAV's field value 125. Match that effective configuration rather than passing identical tokens with different meanings.

## Correctness and execution

Before timing, require native H.264 backend selection in both CoreAV arms from the actual decode receipt, with no external fallback. Require one completed native encoder session and the expected frame submission count. Retain those observations for every timed call.

Check every output's frame count and geometry exactly against its source reference. Require the same rational cadence and pixel format. Compare video payload bitrate across arms within 2% and whole-stream PSNR against the same decoded source within 0.20 dB. Treat two infinite PSNR values as equal. Report violations as non-equivalent comparisons rather than speed results. Preserve all failures and diagnostics. A violation does not permit encoder tuning or dropping an inconvenient timed call.

Run three complete calls per arm per input. Rotate arm order across rounds, using sandbox/in-process/FFmpeg, in-process/FFmpeg/sandbox, and FFmpeg/sandbox/in-process. Time process start through successful completion, including native initialization, source decoding, frame transfer, encoding, teardown, output validation, and muxing. Exclude only setup and the separate correctness analysis. Do not discard statistical outliers. Report the median, minimum, and maximum of all three calls. A timeout, crash, incomplete output, wrong backend, extra session, or equivalence failure makes that input's comparison incomplete. Repeat only for a recorded infrastructure failure, preserving the failed attempt.

## Attribution and reporting

For each input let S, I, and F be the median elapsed seconds for sandboxed CoreAV, in-process CoreAV, and FFmpeg. Report S/F and I/F, total extra time S-F, boundary time S-I, and residual time I-F. When S-F is positive, report the signed boundary share `(S-I)/(S-F)`. Do not clamp negative differences or shares above one. Label shares unstable when the total difference is no larger than the observed per-arm spread.

Report the geometric mean of per-input time ratios and an aggregate time decomposition using sums of the per-input medians. Do not average percentages to derive the aggregate boundary share.

The residual includes decoder, orchestration, and muxing differences. It is not automatically the native H.264 decoder's isolated cost. Add decode-only calls with both implementations producing the same complete raw sample stream, or a separate function-level profile, if feasible within the run envelope. Keep diagnostic timing separate from the end-to-end calls, state whether decode and encode overlap, and do not add overlapping wall times. Record the actual limiting work from CPU accounting or a profile. If the split cannot be established, report that limitation instead of assigning the residual entirely to decoding.

The measurement build also records elapsed time inside decoder packet, EOF, and receive calls, with packet and frame counts. That sum can include waits for asynchronous decoding and overlaps encoding. The separate decode diagnostic first requires identical complete raw-output hashes and byte counts, then makes three alternating calls per implementation with raw output sent to the same null sink. It includes demuxing and raw-output preparation, so it is a decode-path reference rather than a decoder-kernel microbenchmark.

Publish sanitized per-call data and replayable arithmetic with the results in this change. State what the measured boundary cost would imply for a future isolation-policy proposal, but make no policy decision.

## Results

The reference figures, per-call records and host details are in [bench/encoders/av1-process-reference](../../bench/encoders/av1-process-reference/README.md).
