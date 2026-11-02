# Counterbalanced B4 calibration

This design applies only to the future [round 10 B4 plan](../measurements/compat-ffprobe/round10-plan.md). It changes calibration inference, sample size, order, conditioning and receipts. The product bars, estimator, predicates, command/input selection and whole-process timing stay unchanged. No recorded round is re-scored.

## The inference problem

The earlier calibration resamples three whole files but leaves their repeated timings fixed. It has only 27 ordered file draws. More bootstrap draws cannot supply missing repeated-round uncertainty. Fixed AB order also confounds logical side with chronological position.

Balancing order alone is insufficient. With a first-position penalty `r`, balanced raw call ratios are `r` and `1/r`. Their arithmetic median is `(r + 1/r) / 2`, above 1 even under a perfectly unbiased logical-side null. Round 10 therefore changes the **calibration point estimator**, not merely its interval. The product point estimator is not changed.

## Balanced crossover statistic

A crossover consists of two adjacent pairs: AB then BA, or BA then AB. Its contrast is `(log(A1/B1) + log(A2/B2)) / 2`, always using logical A in the numerator. The calibration estimate is `exp(median(contrasts))`. This estimates the central multiplicative logical-side effect after balancing chronological position. It is not the earlier median of individual arithmetic ratios. Do not subtract startup time or adjust any observed duration.

Use eight development files per container, two commands and 16 timed pairs per case. There are eight crossover contrasts per case and 384 per arm. A chronological resampling block contains four pairs, hence two complete crossovers. Execute four global blocks, visiting every selected file and both commands/arms within each block. A global block is the same visit ordinal for every file, not an assertion that those serial visits happened simultaneously.

For each bootstrap draw, first draw four global block ordinals with replacement. Apply these same block ordinals to every contributing file, command and arm to retain shared visit effects. Then, within each container, draw eight file identifiers with replacement. Carry each selected file's two commands and both contrasts in each selected block. A repeated file occurrence repeats its complete selected cluster. Never resample A and B separately or break a crossover.

Use 999 draws and `random.Random(0)`. For each draw call `randrange(4)` four times, then call `randrange(8)` eight times per container, in ISO base media, Matroska/WebM, MPEG-TS order. Calculate `exp(median(resampled contrasts))` for each arm. Sorted bootstrap estimates give the diagnostic 95% interval at zero-based indices 24 and 974 and the **99.5% gate interval** at indices 2 and 996. These are inverse empirical-CDF percentiles, `ceil(p * 999) - 1`.

Both arms still require an estimate in [0.98, 1.02], interval containment of 1.00, every expected observation and no fallback records. The gate confidence changes from 95% to 99.5%. Two chances to reject, few file/time blocks and correlated repetitions justify conservative tails. Nominal confidence does not prove actual coverage: the complete two-arm gate, including the point band, must pass the synthetic validation below. The tail endpoints depend on about three draws; the fixed second-seed sensitivity is also checked before registration, never after a real miss.

Four global blocks remain little information about time-wide disturbances. No bootstrap promises protection against arbitrary long-memory behavior, a nonzero systematic logical-side effect, or noise outside the simulated envelope. Opposing command-specific effects can cancel in an aggregate; publish case/position diagnostics without adding post-hoc gates.

The product statistic remains the ratio of each side's median duration over five rounds, then the median of command/input ratios. Its 95% intervals retain 10,000 whole-file draws within containers, seed 0 and endpoints 249 and 9,749. Do not apply calibration inference to a product group.

## Synthetic validation

[`calibration_sim.py`](../../tools/compat-corpus/calibration_sim.py) is a media-free design experiment, not a timing driver. It reads no media and invokes neither executable. NumPy 1.24.4 supplies PCG64 and vectorized arithmetic. The resampling index generator uses the exact standard-library algorithm above.

The 12 rounded shapes summarize earlier same-executable calls: median duration, median log first/second ratio, sample standard deviation of log pair ratios and early-round excess over later calls. They span approximately 0.99 ms to 1.28 s, log-ratio standard deviations 0.0022 to 0.0754, positional gaps up to 0.0435 and initial excesses up to 18.05%. No file identity or payload is included.

Chronological log durations combine those shapes with call noise, position effects and a decaying initial transient after the registered conditioning calls. Residuals follow each scenario's AR(1) coefficient across pairs. A zero-centered random file-specific logical-side offset is shared across commands. Every nontrivial scenario adds positive exponential absolute call jitter with a 5 microsecond mean, so short calls have a larger relative disturbance. Convert chronological durations to logical ratios using independently randomized balanced crossover orientations, shared between arms.

The five fixed scenarios are:

- A noise-free 4% first-position penalty, whose raw arithmetic ratio median would be above 1.
- Recorded-shaped independent residual noise and the recorded position/transient effects.
- AR(1) 0.8 residuals, 5% case-wide drift and file-offset standard deviation 0.2%.
- Doubled residual noise, AR(1) 0.5, 10% drift and file-offset standard deviation 0.5%.
- AR(1) 0.8 residuals, 10% drift, file-offset standard deviation 0.5%, shared shocks and asymmetric outliers.

The last three also have a chronological call-position disturbance shared across every file, command and arm: AR(1) 0.8, stationary log standard deviation 0.01. The final scenario adds a 2% chance of a shared positive exponential delay and a 1% chance of an independent delay, both with scale 0.1 log units. It also adds one shared first-position disturbance spanning eight pairs, longer than a four-pair resampling block, drawn with log standard deviation 0.02. The synthetic model includes both shared timing variation and within-file repeated-round noise; it does not make calls independent simply because files differ.

Design pilots used seed 35610. The committed validation uses seed 35612, 2,000 trials per scenario and frozen bootstrap seed 0. A predeclared second simulation seed, 35613, with bootstrap seed 1 checks sensitivity to finite bootstrap tails. This second seed is a design diagnostic, not permission to choose a seed for a real run. Each trial evaluates both arms and the whole timing gate, conditional on complete successful calls. Process/capture failures are not simulated. Multiplying logical ratios by 1.005, 1.02 and 0.98 measures power on the same noise. Per-arm power is reported separately from either-arm power; shifting both arms does not establish higher per-arm power.

For each null scenario, require the one-sided 99% Wilson upper bound for either-arm rejection to be at most 5%. Report individual counts and bounds, not just this boolean. The [primary results](../measurements/compat-ffprobe/calibration-simulation.json) and [second-seed results](../measurements/compat-ffprobe/calibration-simulation-sensitivity.json) are model-conditional checks, not an empirical GCP rejection rate or a proof of universal coverage. The 2% effect is the practically important scale because the retained estimate band is [0.98, 1.02]; 0.5% power describes sensitivity below that scale.

Both fixed validations passed. Each column contains 2,000 trials per scenario:

| Null scenario | Primary either-arm rejections | Second-seed rejections | Larger one-sided 99% upper bound |
|---|---:|---:|---:|
| Noise-free position penalty | 0 | 0 | 0.270% |
| Recorded shape | 0 | 0 | 0.270% |
| Serial drift | 5 | 4 | 0.677% |
| Double noise | 10 | 12 | 1.157% |
| Shared shocks | 19 | 20 | 1.666% |

Observed per-arm power across both validations and all scenarios was 99.65–100% at a persistent positive or negative 2% effect, and 55.05–100% at positive 0.5%. These are Monte Carlo estimates, not guaranteed detection rates. Counts and one-sided 99% bounds for each arm are in the result files. Every generated trial contributes to the counts, including low-noise cases and statistical misses.

```sh
OPENBLAS_NUM_THREADS=1 python3 tools/compat-corpus/calibration_sim.py \
  --out docs/measurements/compat-ffprobe/calibration-simulation.json
OPENBLAS_NUM_THREADS=1 python3 tools/compat-corpus/calibration_sim.py \
  --seed 35613 --bootstrap-seed 1 \
  --out docs/measurements/compat-ffprobe/calibration-simulation-sensitivity.json
```

## Observe outside the timer

Keep the existing whole-process start, event-driven deadline, exit notification and blocking-reap boundary. Before entering the timed function, snapshot `resource.getrusage(RUSAGE_CHILDREN)`, pinned-CPU steal ticks from `/proc/stat` and exposed pinned-CPU frequency files. Snapshot them again immediately after it returns. Compute deltas and construct receipts afterwards. Launch exactly one timed child at a time and no other child between resource snapshots; aggregate child-resource deltas then belong to that invocation. Add no telemetry syscall to the timed function.

Mandatory child fields are user/system seconds, minor/major faults and voluntary/involuntary context switches. Frequency and steal have explicit available/unavailable status and a reason; missing counters are not zero. A virtual frequency reading is not a cycle counter or evidence of thermal stability. Validate mandatory capture before conditioning. Missing or invalid child-resource deltas make the arm incomplete. Record reset/backwards host counters as invalid; never use them to correct duration.

Buffer receipts in memory and serialize after the timing phase or on failure. Retain phase, arm, file alias, command, pair/block ordinal, logical side, actual chronological position, monotonic start/end offsets, elapsed time, exit status and telemetry. Child CPU time is diagnostic and is never subtracted from whole-process wall time. Outside-timer observation can perturb the following call, so its sequence is fixed and identical for both sides.

The separate driver change must test the schedule, ratio orientation, exact resampling/quantiles, capture boundaries, unsupported telemetry, failure accounting and historical-schema rejection before any run.
