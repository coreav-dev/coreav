# R3 checker deadline calibration

The R3 checker has an **8,580-second total deadline**. This is a tooling safety limit, not a release-performance result or a change to the correctness bar.

## Method

One instrumented rehearsal on the already-used inputs recorded elapsed wall time with `time.monotonic` inside each oracle and selection worker and around each complete `copy_check.py` process. Individual check times exclude worker queueing; sequence times include startup and queueing. The complete distribution, not a shard's duration or a single inner subprocess's limit, sets the budget.

The source was `e7f36e034a61e1e8e7900b9f7d92762b06694031`. The workflow built the CLI and harness with the `dist` profile, verified the shared FFmpeg 9.0.2 pin in every shard, and put the pinned oracle directory first on R3's PATH. The downloaded binaries' SHA-256 values match the build log and are retained in [summary.json](summary.json). It used GitHub-hosted `ubuntu-24.04` x86-64 runners, one shard at a time, four file workers, one checker worker per sequence, and chunks of at most 16 records. The CPU model was not recorded; shared-runner variation is one reason for the margin, not evidence for a speed claim.

The earlier attempt is not part of this distribution. It recorded 143 raised oracle checks and 961 Matroska pair diagnostics naming a missing program. In the corrected rehearsal all 5,114 checks completed without a raised exception. The retained timings do not identify each check's container or output, so the artifacts cannot identify 143 replacement invocations one-to-one. No held-out inputs or judged release run were used.

The separate `matroska_validation` counts in [summary.json](summary.json) come from the retained per-pair records and checker diagnostics, not inferred from the timing samples' job names. The records were matched privately by file, command ID and spelling index. All 1,988 Matroska pairs match between attempts: 961 reached the copy checker; the other 1,027 were unchanged refusals, ineligible pairs or failed oracle commands. Of the 961, the same 960 that previously failed their oracle now pass both oracle and selection, and one keeps its selection disagreement. The executed `copy_check.oracle` dispatches Matroska inputs to `mkv_remux_check.check`, so those 960 passes establish successful work on that oracle path. The missing-program diagnostic count falls from 961 to zero. This establishes recovered Matroska oracle work without claiming an unavailable per-invocation identity trail.

## Distribution

| Timed unit | Count | Median (s) | p90 (s) | p99 (s) | Maximum (s) |
| --- | ---: | ---: | ---: | ---: | ---: |
| Individual check | 5,114 | 0.115 | 1.187 | 25.197 | 2,818.506 |
| Oracle check | 1,162 | 0.826 | 10.068 | 77.025 | 2,818.506 |
| Selection check | 3,952 | 0.075 | 0.339 | 1.844 | 2.985 |
| Complete sequence | 384 | 5.041 | 41.721 | 564.435 | 2,842.740 |

Percentiles use nearest rank; the median averages the central two values for an even count. All 384 sequences terminated: 364 returned 0 and 20 returned 1 for correctness disagreements. Both exits contribute to calibration; the longest sequence returned 0. Failures, refusals and oracle-failed pairs remain in the correctness records, not converted into passes by workflow success. Aggregate pair outcomes are in the summary.

The rule is `max(600, 60 * ceil(3 * longest_sequence_seconds / 60))`: three times the observed maximum, rounded up to a minute, with a 600-second floor. For 2,842.73956501 seconds it gives **8,580 seconds**. A compound oracle alone took 2,818.506 seconds despite its inner subprocess limits; a 600-second sequence limit would cut valid work. This budget is finite headroom for this checker configuration, not proof that every possible future sequence finishes in it.

## Recompute and verify

[checks.csv](checks.csv) and [sequences.csv](sequences.csv) retain every elapsed sample, check kind, job, record count and completion or exit status. Sequence numbers are synthetic ordinals joining those two files; they are not PIDs, input identities or timestamps. No file names, media hashes, paths or command arguments are retained.

From the repository root, this regression reads the samples, reconciles every sequence's selection count, and checks the production default against their maximum and the stated rule:

```sh
python3 -I -m unittest discover -s tools/compat-corpus/tests -p test_r3.py
```

The same tests preserve a valid six-invocation, three-record sequence with the production ratio of total to inner-call budget scaled down. A stopped-check regression requires the checker, its child and its grandchild to be absent after timeout, including no zombies, and requires all pairs to fail even when a passing report was already written. R4 and its reason table are unchanged.

Encode breadth shares the checker entry point, including the deadline, timing events and Linux subreaper. Its regression runs a stopped checker and requires a tooling fault that voids the run, not a CoreAV oracle disagreement.
