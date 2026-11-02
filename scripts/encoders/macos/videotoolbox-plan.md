# VideoToolbox ownership, smoke and speed plan

This plan precedes native implementation and every encoder or timing run of this work unit. The ownership design is in [the macOS note](../../../docs/design/encoders/macos.md). The [encoder contract](../../../docs/design/encoders.md) remains the source of the M1 through M5 acceptance bars. This PR delivers the standalone owner, hosted correctness smoke and measurement tools, not transcode integration or an acceptance result. Judged timing starts only after this plan-and-tools PR merges. Results are a later PR.

## Reference host

For this work, `ENC-MAC-1` is a dedicated Scaleway Mac mini M1-M: Apple M1, eight CPU cores comprising four performance and four efficiency cores, eight GPU cores, 8 GB unified memory and a 256 GB SSD, in Paris. This replaces the former Mac mini M4 Pro profile before measurement. Use macOS 15.7; record and commit its exact build before judging. If that OS image or machine is unavailable, commit a replacement profile before running rather than substitute a virtual machine or a different Mac silently.

Keep one physical allocation for the session. Record hardware identifiers, OS build, framework encoder ID and actual hardware-use status. Do not use a VM, shared CI runner or a different allocation between paired commands. Absolute elapsed times are M1-specific. The speed claim concerns the same-host CoreAV-to-`h264_videotoolbox` ratio, not a claim that M1 matches a newer Mac. Hosted runner timing is diagnostic only and can never satisfy this plan.

No host is provisioned by this PR. Prepare and verify tools before allocating the timed host. Do not rent or run judged inputs merely because the plan exists.

## Tools and registration

The oracle version comes only from `scripts/oracle-ffmpeg.env`. Hosted Linux conformance uses the existing authenticated archive, installer and `oracle_pin.py --verify-bin`, unchanged. macOS encoding artifacts are transferred with hashes; decoding and probing those immutable bytes does not require the encoding host.

The timed baseline is a measurement-only macOS FFmpeg with `h264_videotoolbox`, at the shared pinned source revision and with a frozen configuration. A Linux oracle cannot run Apple's encoder or establish the paired Mac time. The macOS reference acquisition/build record requires release-owner review before use. This plan does not add another FFmpeg pin, authorize source acquisition or accept an arbitrary executable with a matching banner. Verify both binary hashes and the recorded acquisition/build identity before every session, cache hit included. Keep documentation disabled and remove source/examples before black-box use. Missing or mismatched provenance stops before any input opens.

Before judging, commit an immutable registration containing the merged candidate and tooling revisions, plan digest, complete ordered input roster, source/raw-frame hashes, selected streams, presented-frame counts and clocks, colour/range descriptions, both executable digests and configurations, exact paired commands, host/OS manifest, framework identity, thread settings and metric tool/model hashes. The runner refuses absent, unmerged or dirty registrations and any changed tool, input, plan or setting. Do not leave binary identity to a mutable PATH lookup.

## Inputs and paired work

Use the contract's four disjoint height classes, bounded by 360, 720, 1080 and 2160 lines and width at most 3840. Select the first ten eligible held-out inputs in each class by SHA-256 of `coreav-encoders-v1:` followed by the full file hash. Inputs have 240 through 1800 presented frames, duration 8 through 30 seconds and 8-bit 4:2:0 SDR output. Commit all forty inputs and their order before the run. Do not replace failures, borrow training inputs, shrink an incomplete class or relax eligibility after seeing a result.

Prepare one immutable raw reference per selected stream before the timed session. Both commands consume identical frames, geometry, source clock, colour description, range and frame order. No implicit scaling, rotation, colour/range conversion, bit-depth change or frame-rate conversion is allowed. Preserve the frame-clock rationals rather than converting timestamps through floating-point seconds.

Hardware-required and software-only encoders are separate cells. The hardware cell is required. Software may be unavailable only when both APIs fail the preflight encode/drain/decode smoke before the roster is encoded. A mismatch between APIs, unknown hardware identity, missing controls or any failure after accepting a frame is a nonpassing result, not an exclusion. Record actual selected encoder IDs and hardware status, not requested preferences.

Both sides use High profile, no frame reordering and a maximum keyframe interval of two seconds. The per-class bitrate targets are 1, 3, 6 and 20 Mbit/s. At a constant frame rate, the frame-count keyframe bound is the floor of two times that rate, paired with the native two-second duration bound. Freeze the corresponding FFmpeg controls from its pinned encoder help before judging; refuse an unmatched setting. Read back effective native controls and check output configuration. Do not translate x264 CRF or preset values to a platform quality scale.

## Correctness and quality gates

Every output must decode completely with the pinned FFmpeg H.264 decoder and, within its supported profile, CoreAV's independent decoder. Frame count, dimensions, frame order, colour description and declared times must agree with the reference. Timestamp error is at most half a source tick and A/V start-offset error at most 1 ms where audio exists. Source patterns in synthetic fixtures independently check frame identity; their success is not a substitute for the held-out roster. Parameter sets and sample descriptions must match the decoded stream. Encoded-byte equality is not required.

Quality is checked before a speed result can pass. Compare every decoded frame to the common reference in fixed presentation order, with no searched alignment. Use the pinned `vmaf_v0.6.1` model at native resolution, all-plane PSNR and SSIM All. Median CoreAV-minus-baseline deltas must be at least -1.0 VMAF, -0.10 dB PSNR and -0.001 SSIM. At least 98% of inputs must meet all three per-input floors of -2.0, -0.50 dB and -0.005; every fixture meets those floors. Missing, NaN or invalid metric output fails rather than disappearing from the denominator.

For each output, all-plane clip PSNR is `10 * log10(255^2 * sum(samples) / sum(squared_error))`, pooling every Y/U/V sample of every presented frame. Retain each frame's integer sample count and squared-error sum. Zero total error is explicitly perfect (positive infinity), not a finite cap; equal-perfect CoreAV/baseline PSNR has delta zero, CoreAV-perfect versus finite is an improvement, and finite versus baseline-perfect fails every finite PSNR floor. Replay recomputes these values from retained frame facts bound to decoded/source bytes. VMAF is the arithmetic mean of per-frame `vmaf_v0.6.1` scores at native resolution; SSIM is the arithmetic mean of the pinned filter's per-frame SSIM All values. No frame is omitted.

Each input's elementary-stream payload-size ratio must be within 0.95 through 1.05. For clips of at least eight seconds, each side's mean payload bitrate must be within 10% of its target. Also record MP4 size when a container is present. Extra bits cannot purchase a quality pass. A standalone elementary-stream result does not establish MP4 indexing or integrated transcode acceptance.

## Timing method and speed bar

Use one warmup and five measured invocations per command and input, alternating AB and BA order. Inputs are warm in the page cache and outputs use the same RAM filesystem. Each invocation begins at process creation and ends after successful output close and direct-child exit. Use a process-exit event or blocking wait, not timeout polling sleeps. Keep finite child/job deadlines, preserve crash and timeout classification, terminate the process group on timeout and reap the child. A failed command cannot produce a time ratio.

The paired command registration includes native frame reporting and FFmpeg's post-encode packet-statistics output; writing that instrumentation is part of the whole-process cost. Retain exact rational packet timestamps and packet sizes. An elementary H.264 stream alone cannot recover the original submission timestamps. Any black-box trace needed to establish the baseline's actual selected encoder, hardware use and effective controls is also frozen in the reviewed build record and command registration; missing observability fails preflight rather than becoming an inferred identity.

For each input divide the median CoreAV elapsed time by the median FFmpeg elapsed time. The median of those ratios must be at most 1.00 overall and in every height class of each available encoder cell. Report 95% input-level bootstrap confidence intervals with 10,000 resamples and seed 0, plus the share of inputs faster than FFmpeg. A faster claim additionally requires the interval's upper bound below 1.00. Do not pool hardware and software or hide a class miss in an overall median.

Run A/A and null gates before and after the session. A/A median times differ by at most 3%, and the null comparison's 95% interval contains 1.00. No swap, thermal warning, throttling or competing encoder work is allowed. Record thread limits, affinity where the OS supports it, memory limits, OS build and background load. If a gate fails, void the session and retain all evidence. Do not rerun a missed arm unless an evidenced host fault voided it.

The PCM/raw-frame driver and an integrated video-plus-audio command are different results. This work can judge only the raw-frame driver it implements. Integration remains with its owner and needs the same frozen original streams, selected/copied tracks, controls and timestamp/container policy before it can claim M4. Do not present a raw-driver pass as an end-to-end pass.

## Hosted smoke and safety

PR CI runs safe-model and tooling regressions, native builds and synthetic correctness smoke on hosted macOS. The smoke submits small, bounded frames, drains, independently decodes and checks exact count/order and source-pattern identity. It records runner image, OS build, selected implementation and hardware status. A recognized unavailable software encoder exercises refusal cleanup and is reported as unavailable, not encoded success. Linux verification of transferred artifacts retains their hashes and verifies the shared pinned decoder before use.

All callback lifetime, reentrancy, overlapping-copy completion, cancellation, unavailable encoder, oversized output, malformed configuration, bounded-queue and cleanup fixtures must pass. A dropped frame, missing/duplicate identity, early completion, crash, timeout, output change or missing case is nonpassing. No use-after-free, panic or partial final output is permitted. Affected output-reader fuzzing remains in CI. The in-process native API cannot promise an interruptible framework call or a hard teardown deadline; report that limit rather than claim strict-safe isolation.

Timed roster execution is unreachable from PR events. A manual judged mode requires the merged candidate, tooling and registration, a matching fixed-host manifest and complete preconditions above. A hosted diagnostic label cannot be promoted to a judged result. Unit tests cover altered/missing evidence, case coverage, wrong clocks/settings/identity, metric and bitrate failures, ratio aggregation, host-gate failure, process exit/timeout cleanup and retained nonpassing reports without opening the held-out roster.
