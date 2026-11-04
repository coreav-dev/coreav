# H.264 PGO held-out evaluation

PGO reduced the overall one-thread median elapsed time by 2.85%, but did not meet the registered 3% median-improvement bar. Its ratio to base was 0.971482 [0.965980, 0.980164]. The confidence interval excludes parity, and all four eight-thread class guards passed. The combined registered verdict is **not met**. This completed evaluation changes no build or release default, and the result was not rerun.

## PGO against base

Ratios divide each file's nine-call PGO median by its base median. Lower is better. Each interval is a two-sided 95% paired-round and file bootstrap interval. Faster-file shares count ratios strictly below 1.

| Threads | Class | n | Median ratio [95% CI] | Files faster than base |
| --- | --- | --- | --- | --- |
| 1 | `sd` | 16 | 0.966404 [0.957910, 0.978864] | 15/16 (93.75%) |
| 1 | `720p` | 16 | 0.975797 [0.965219, 0.991481] | 14/16 (87.50%) |
| 1 | `1080p` | 16 | 0.966724 [0.953484, 0.980331] | 15/16 (93.75%) |
| 1 | `4k` | 9 | 0.992058 [0.977490, 1.012942] | 5/9 (55.56%) |
| 1 | `overall` | 57 | 0.971482 [0.965980, 0.980164] | 49/57 (85.96%) |
| 8 | `sd` | 16 | 0.991095 [0.981982, 0.997965] | 13/16 (81.25%) |
| 8 | `720p` | 16 | 0.991399 [0.983752, 1.004555] | 11/16 (68.75%) |
| 8 | `1080p` | 16 | 0.980469 [0.969367, 0.992714] | 15/16 (93.75%) |
| 8 | `4k` | 9 | 0.992833 [0.983327, 1.007356] | 6/9 (66.67%) |
| 8 | `overall` | 57 | 0.989957 [0.985013, 0.993847] | 45/57 (78.95%) |

The one-thread overall bar requires a median at most 0.97 and a confidence-interval upper bound strictly below 1.00. The observed median 0.971482 misses the first condition. Each eight-thread class requires a confidence-interval upper bound at most 1.01, including `4k` with n=9. All four pass that guard. The overall eight-thread row is descriptive, not an additional acceptance condition.

## Reference comparisons

These comparisons use pinned FFmpeg 9.0.2 and do not replace the PGO-versus-base bars. The one-thread PGO `4k` interval crosses parity, so the result does not establish an all-class one-thread faster-than-FFmpeg claim.

| Threads | Class | n | Base/FFmpeg median [95% CI] | Base faster files | PGO/FFmpeg median [95% CI] | PGO faster files |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `sd` | 16 | 0.940280 [0.896421, 0.958729] | 14/16 (87.50%) | 0.912733 [0.862931, 0.931460] | 15/16 (93.75%) |
| 1 | `720p` | 16 | 0.962810 [0.947043, 1.001588] | 11/16 (68.75%) | 0.955336 [0.915730, 0.983849] | 15/16 (93.75%) |
| 1 | `1080p` | 16 | 0.962684 [0.918942, 0.969606] | 15/16 (93.75%) | 0.931528 [0.893593, 0.954522] | 16/16 (100.00%) |
| 1 | `4k` | 9 | 0.973984 [0.940898, 0.990636] | 9/9 (100.00%) | 0.976349 [0.900893, 1.008427] | 7/9 (77.78%) |
| 1 | `overall` | 57 | 0.959230 [0.948122, 0.968070] | 49/57 (85.96%) | 0.934365 [0.914125, 0.952965] | 53/57 (92.98%) |
| 8 | `sd` | 16 | 0.888601 [0.874304, 0.910189] | 16/16 (100.00%) | 0.876905 [0.863084, 0.895955] | 16/16 (100.00%) |
| 8 | `720p` | 16 | 0.891445 [0.828945, 0.911733] | 15/16 (93.75%) | 0.869721 [0.821850, 0.910000] | 15/16 (93.75%) |
| 8 | `1080p` | 16 | 0.771646 [0.676700, 0.856039] | 16/16 (100.00%) | 0.750905 [0.665938, 0.834368] | 16/16 (100.00%) |
| 8 | `4k` | 9 | 0.770920 [0.651971, 0.830472] | 9/9 (100.00%) | 0.778576 [0.641737, 0.839283] | 9/9 (100.00%) |
| 8 | `overall` | 57 | 0.858829 [0.801504, 0.869731] | 56/57 (98.25%) | 0.854220 [0.787550, 0.865383] | 56/57 (98.25%) |

## Qualification and host checks

All 342 qualification cells match complete cropped planar bytes, SHA-256 and exact presented frame counts across base, PGO and FFmpeg, at both thread counts. Every file presents exactly the smaller of its source frame count and registered class cap. The run includes all 57 metadata checks, 342 warmups, and 3,078 judged calls. No failed or incomplete call becomes a timing sample. An earlier untimed [qualification preflight](h264-pgo/heldout/preflight.json) passed 24 cells on four fixed cohort files. The cohort was already registered and unchanged.

| Class | n | Minimum frames/file | Maximum frames/file | Total frames across files |
| --- | --- | --- | --- | --- |
| sd | 16 | 45 | 1800 | 26335 |
| 720p | 16 | 417 | 900 | 13036 |
| 1080p | 16 | 176 | 300 | 4676 |
| 4k | 9 | 150 | 150 | 1350 |

The host was a standard on-demand GCP `c4-standard-16`, with eight physical cores, SMT off, Intel Xeon Platinum 8581C, PMU `STANDARD`, and provider-accepted `ALL_CORE_MAX`. One-thread calls were pinned to CPU 2 and eight-thread calls to CPUs 0 through 7. OS frequency controls were not recorded. The provider turbo setting is not evidence of an exactly fixed clock. Startup load averages were 1.31, 0.75 and 0.31 on eight available cores.

The 288 retained calibration calls produce the following median-of-four-class ratios, all within [0.99, 1.01]. Beginning and ending host checks passed. Maximum reference drift was 2.1155%, below the registered 5% limit.

| Threads | Comparison | Ratio |
| --- | --- | --- |
| 1 | aa | 0.999516 |
| 1 | null | 0.999769 |
| 8 | aa | 0.998128 |
| 8 | null | 0.996136 |

## Non-H.264 CLI diagnostics

These separate diagnostics use the fixed generated 60-second stereo signal, nine paired rounds per command and affinity, and a round-only bootstrap. They are non-gating for the H.264 verdict. None of the six 1% confidence-interval guards is met, so neutrality within that bound is not established. One-thread FLAC decode is slower in this run: its confidence interval is entirely above 1.00. Wide intervals on the other commands are not proof of a regression.

| Command | Affinity | Rounds | Base median (ms) | PGO median (ms) | PGO/base [95% CI] | 1% guard |
| --- | --- | --- | --- | --- | --- | --- |
| probe | 1 | 9 | 1.4501 | 1.5312 | 1.055961 [0.979523, 1.146941] | Not met |
| probe | 8 | 9 | 1.4630 | 1.4555 | 0.994846 [0.969519, 1.074250] | Not met |
| mp4-remux | 1 | 9 | 7.0968 | 7.1067 | 1.001395 [0.918337, 1.029495] | Not met |
| mp4-remux | 8 | 9 | 7.0691 | 7.0152 | 0.992372 [0.949423, 1.047551] | Not met |
| flac-decode | 1 | 9 | 51.0741 | 51.8680 | 1.015545 [1.006295, 1.017802] | Not met |
| flac-decode | 8 | 9 | 13.8660 | 14.0078 | 1.010229 [0.955719, 1.086801] | Not met |

All 18 diagnostic qualification cells and 12 warmups completed before the 108 diagnostic timed calls. Probe JSON and complete remux/audio bytes match between the CLI arms, with independently checked decoded signal samples and counts. These fixture results do not establish a whole-CLI speed benefit.

## Acquisition accounting and inference

The cohort was registered before qualification at n=16, 16, 16 and 9. It retains the original frozen source order and metadata-only eligibility rules. No CoreAV result selected an input, and nothing was backfilled after registration. The eight failed source probes all have unknown class and reason `stderr`, at stage `source-probe`; there are no known-class capture failures. Unknown failures charge every class still unfilled at that decision, so class charges must not be summed as distinct failed-source counts.

| Class | Selected n | Known charges | Unknown charges | Known successors | Unknown successors | Known unmatched | Unknown unmatched |
| --- | --- | --- | --- | --- | --- | --- | --- |
| sd | 16 | 0 | 4 | 0 | 4 | 0 | 0 |
| 720p | 16 | 0 | 4 | 0 | 4 | 0 | 0 |
| 1080p | 16 | 0 | 4 | 0 | 3 | 0 | 1 |
| 4k | 9 | 0 | 8 | 0 | 6 | 0 | 2 |

Successors discharge the oldest outstanding charge in their class for accounting only. They do not establish an excluded file's class or outcome. Inference is limited to the capture-clean eligible cohort, not excluded inputs or the entire frozen population. Class labels use displayed-dimension bounds; `4k` is the residual above 1920 by 1088, not a guarantee of UHD geometry. The registered counts are not a statistical power guarantee.

The [acquisition report](h264-pgo/heldout/acquisition-report.json) is preserved unchanged from the registered snapshot. Its acquisition status is separate from this completed evaluation's status and speed verdict. The earlier [preparation record](h264-pgo-preparation.md) remains historical. Merged amendments defined [capture replacement](https://github.com/coreav-dev/coreav/pull/354), [removal of the aggregate acquisition time cutoff](https://github.com/coreav-dev/coreav/pull/367), [exit-zero source-probe diagnostic rejection](https://github.com/coreav-dev/coreav/pull/372), and the [exact evaluation cohort](https://github.com/coreav-dev/coreav/pull/380). Per-job resource limits and all judging thresholds remained unchanged.

## Method, artifacts and replay

The [registered plan](../design/h264-pgo-release.md) uses nine interleaved paired rounds, 10,000 hierarchical bootstrap replicates, and seed 20261103. Each replicate samples the registered number of files with replacement within each class, then nine shared paired-round indices per sampled file. Class and overall medians use file ratios, and the overall replicate contains all 57 resampled files. The 95% interval uses linearly interpolated 2.5th and 97.5th percentiles. Calls measure complete process exit with decoded output discarded; process startup is included and qualification hashing is outside timing.

Measured artifacts come from [`01a3daae9158a89bb04a98eda846d660e36408a7`](https://github.com/coreav-dev/coreav/commit/01a3daae9158a89bb04a98eda846d660e36408a7), built with Rust 1.99.1 and matching LLVM 23.1.1, the locked `dist` recipe, and the retained training-only profile. The independent clean rebuild matches the PGO binary exactly. No evaluation input was used to train or rebuild an artifact. The judging tools are separately pinned to [`0d182ce36a53be731ee594cb63aa6504d18637af`](https://github.com/coreav-dev/coreav/commit/0d182ce36a53be731ee594cb63aa6504d18637af). The black-box reference build is `n9.0.2-17-g2a571b6068-20260930`.

The [record](h264-pgo/heldout/record.json) binds artifacts, actual input/frame work and sanitized evidence hashes. The [summary](h264-pgo/heldout/summary.json) retains every class interval, faster-file count, per-file ratio and each arm's median, minimum, maximum and range. Compressed [H.264 raw calls](h264-pgo/heldout/events.jsonl.gz) and [diagnostic raw calls](h264-pgo/heldout/diagnostics.jsonl.gz) contain no media, source identities, VM names, cloud project names or home paths.

From a checkout containing this record, `python3 -I -m unittest discover -s scripts/tests -p 'test_h264_pgo_results.py'` checks evidence hashes, complete work, point estimates and the final verdict without media execution. The complete artifact/oracle audit and original bootstrap were independently replayed at the measured tooling revision, with an identical summary. Full artifact auditing also requires the retained private media and acquisition receipts. This evaluation is final for these arms; no threshold, input or method was tuned after the result.
