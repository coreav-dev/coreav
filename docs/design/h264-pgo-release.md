# Opt-in H.264 PGO builds

## Scope

This recipe builds the native H.264 `decode` example with profile-guided optimization (PGO). It is an opt-in Linux x86-64 experiment, not a change to published packages, the full CLI, Cargo's default profiles, or release defaults. It uses `dist`, default features, and no post-link optimizer. Other targets and compiler versions need their own evidence.

The earlier training-only diagnostic used a merged profile containing three build-script profiles. Reproduction must retain that profile and its relocation flag. Removing those profiles creates a different artifact. The clean recipe instead separates host compilation from target instrumentation and collects each decoder invocation in a new directory. It records an explicit inventory rather than merging everything in a shared directory.

## Registered held-out evaluation

The plan and tools in the first commit precede all runs. Neither this PR nor its verification may fetch, extract, probe, decode, calibrate on, or time held-out media. The allocation reservation and this PR must both merge before acquisition or selection. Results go in a separate PR. The results do not decide a release default.

The neutral allocation label is `h264-pgo-heldout-v1`. Its private reservation fixes the source population and keeps source names out of public records. The deterministic rule uses seed `coreav-h264-pgo-heldout-v1:20261103`. Order candidate identities by SHA-256 of the seed, a NUL byte, and the identity. Acquire in that order, without extending the frozen population. Exclude every previously allocated or used input, including all training and prior speed inputs, encoder inputs, and other operations' draws. The exclusion registry binds full file hashes, normalized source identities, and source groups. Check identities before acquisition and full hashes after acquisition. A missing registry component, a duplicate, or an overlap is fatal. Do not relabel another operation's draw.

Select sixteen independent files in each of `sd`, `720p`, `1080p`, and `4k`, using the first eligible video track of each file. Classification uses displayed width and height: at most 640 by 480, 1280 by 720, 1920 by 1088, or larger. Admission is metadata-only: progressive H.264, 8-bit 4:2:0, supported profile, positive dimensions and frame count. No CoreAV result or elapsed time selects an input. Stop collection at the fixed population or preparation budget. An unfilled class is an incomplete evaluation, not permission to reduce its count or substitute an input after decoding begins.

Sixteen files per class improve on the four-file diagnostic's uncertain class intervals. They do not guarantee that a one-percent bound can be resolved. There is no claim of a numerical power calculation: this is a budget-bounded evaluation with an explicit inconclusive outcome. Keep naturally short inputs. Measure up to 1,800, 900, 300, and 150 frames respectively, without looping or concatenating media. Publish actual presented frame counts.

Freeze one clean PGO artifact and a matching unprofiled artifact before opening the allocation. Both use the same source, locked dependencies, compiler, target, features, and path normalization. An independent clean-target-directory rebuild using the retained merged profile must match the PGO binary byte for byte. Regenerating a concurrent profile is not the reproducibility check. Retain binary, compiler, linker, training, raw-profile, merged-profile, source-tree and tool hashes.

### Host and execution

Use one standard on-demand `c4-standard-16`, eight physical cores, SMT off, Xeon Platinum 8581C, and provider-accepted `ALL_CORE_MAX`. Pin 1T to CPU 2 and 8T to CPUs 0 through 7. Never time on spot, development machines, or a laptop. The host records CPU topology, affinity, provisioning and turbo settings before media execution. Lack of an OS frequency control is disclosed, not described as exactly fixed frequency.

Calibrate on the four fixed training references, one per class, not the allocation. For each thread count, collect nine interleaved rounds of two independently copied identical base binaries. Both the A/A ratio and an identical-copy null ratio must be within [0.99, 1.01], using the median of four class medians. Check the same references at the beginning and end. A greater-than-five-percent reference drift invalidates the entire run. Warm each measured input and arm once, then execute nine paired rounds. Shuffle stream order and the three arms (`base`, `pgo`, `ffmpeg`) with the registered seed. Time complete process exit with a Linux process-exit descriptor, not timeout polling. Timing excludes hashing and raw-output qualification.

Before timing, compare complete cropped planar output against the pinned FFmpeg oracle for base and PGO, at both thread counts. Require equal bytes, SHA-256, and positive integral frame counts within the registered frame cap. Hash output as it is read, with a geometry-derived byte ceiling. A nonzero exit, timeout, unexpected diagnostic, output overflow, missing cell, changed input or binary, or nonfinite timing aborts the run. Do not tolerate unresolved streams or analyze a successful subset. Native stderr must be empty. Oracle stderr must also be empty; no diagnostic exception is authorized for this new allocation.

Keep an append-only event log and a terminal completion or abort record. The offline audit requires the exact qualification and timed-cell roster, positive finite times, all registered rounds, matching hashes, successful calibration and both host checks. Partial output never produces a speed verdict. Retain invalid attempts. A speed miss is final; only a documented host fault can justify a separately recorded recovery, never a narrow miss or a method change.

### Estimator and thresholds

Every ratio is PGO elapsed time divided by base elapsed time, so lower is better. Divide each file's nine-call medians. Class and overall point estimates are medians of those file ratios. All four classes have equal file counts. Use 10,000 hierarchical bootstrap replicates with `random.Random(20261103)`: sample sixteen files with replacement within each class, then nine paired round indices with replacement per sampled file. Use those same round indices for both arms. Take the median within each class and across all sixty-four resampled file ratios. The two-sided 95% interval uses linearly interpolated 2.5th and 97.5th percentiles.

Both conditions are mandatory:

- Overall 1T median ratio is at most **0.97**, and its 95% interval upper bound is strictly below **1.00**.
- In **every** 8T class, the 95% interval upper bound is at most **1.01**, including equality. A passing point estimate does not pass this condition.

Report every class and overall estimate, interval, faster-file share, and each file's median, minimum, maximum and range. Report the matching base and PGO comparisons against the pinned oracle as information, not as substitute PGO bars. A claim of faster-than-oracle performance additionally requires its interval upper bound below 1.00 and point estimates at most parity overall and per class. Do not tune after seeing this allocation.

## Design choices

One serial build recipe owns profile provenance. Separate base, instrumented and optimized target directories prevent stale build reuse. The historical reproduction is a separate mode because its flags and profile are intentionally different.

The measurement executor writes events, while the auditor checks completeness before the shared `h264_parity_bars.py` estimator computes intervals. Reusing the older parity runner unchanged would retain tolerated diagnostics and unresolved-stream behavior that this evaluation does not allow. The VM lifecycle remains owned by `gcp_vm_job.py`, always with explicit standard provisioning for timing.

No decoder instructions or guarded command are changed. PGO may change generated instructions and executable size; the held-out evaluation measures those consequences rather than presuming a gain.
