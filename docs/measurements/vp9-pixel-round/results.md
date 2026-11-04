# VP9 pixel reconstruction results

The merged changes reduce native user CPU by 4.01× at 720p, 4.54× at 1080p and 2.76× at 4K on the six registered development prefixes. Most of that gain is already present without the explicit AVX2 kernel. The scalar-only build still takes about 14–18 times pinned FFmpeg’s user CPU, so this is not close to parity.

This is the post-merge diagnostic specified by the [registered plan](../../design/vp9-pixel-round.md), not a performance vote or a held-out speed result. No acceptance bar, input, prediction or implementation was changed after observing the results. The native backend remains opt-in.

## Source and method

- Candidate: `117a41ae83923b59f401bab1a380f00d2c21b1ae`; baseline: `8f894f51d02795a16c63c35a2361dc1a9783281b`.
- The unchanged [plan](plan.json) has SHA-256 `0cdd1ae3a3e7598ee1129eea0f46b27a601e43f55bb888899f97f005044ac651`. The [build recipe](build.json) and [content-bound provenance](provenance.json) record all six binaries, compiler, features, profile overrides and script hashes.
- Host: on-demand `c4-standard-16`, Intel Emerald Rapids, SMT off, `ALL_CORE_MAX`, standard PMU, CPU 0 affinity. Native and FFmpeg decoder thread counts are one. Rust 1.99.0, `dist`, fat LTO, one codegen unit; debug symbols retained without changing optimization. No global native-CPU or target-feature override was used.
- Each of the six original development inputs contributes its first 120 packet-copy frames. The same prepared bytes feed every arm. Complete packet bytes, frame hashes, dimensions, presentation, repeated oracle sequences and consumed-work counts passed before timing. No held-out media was used by this diagnostic.
- The native adapter receives and drops every decoded frame without hashing during timing. Hashing is a separate preflight mode. FFmpeg 9.0.2 uses its native `vp9` decoder and null output with passthrough timing and explicit demux encoder time base. It repeats the prefix 80, 40 or 5 times for 720p, 1080p or 4K; all times are normalized to 120 frames.
- One warm-up per arm/input precedes five balanced, unprofiled rounds. Input order alternates the arm-order seed; each round rotates it. Every arm occupies every position once. The 150 timed calls and 24 separate profiles are complete, with no failed or discarded cell.
- Decode-only here means the receive-and-drop/null-output paths, including each decoder’s required reference storage and frame management. It is not file export or transcoding; native reference-frame packing is not equivalent to FFmpeg’s reference storage.
- The small untimed preflight passed before the full allocation. Independent replay of all 349 retained invocations reproduces the host summary exactly. [Public evidence](summary.json) retains every timed call and every aggregate profile; larger raw command/profile records are retained privately.

## Paired class results

Ratios use user CPU. Larger baseline/candidate and safe/automatic ratios mean an improvement; larger native/FFmpeg ratios mean native remains slower. Each class value is the median of its two per-input medians of the five paired rounds, so columns do not necessarily multiply. Ranges below are observed pair ranges, not confidence intervals.

| Class | Baseline / safe | Baseline / automatic | Safe / automatic | Safe / FFmpeg | Automatic / FFmpeg | Frozen prediction | Outcome |
|---|---:|---:|---:|---:|---:|---|---|
| 720p | 4.01× | 4.01× | 1.010× | 14.06× | 13.81× | 1.5–3.5× | Above upper prediction |
| 1080p | 4.59× | 4.54× | 0.990× | 14.69× | 14.83× | 1.6–4.0× | Above upper prediction |
| 4K | 2.72× | 2.76× | 1.011× | 17.79× | 17.66× | 1.5–3.2× | Within range |

The 720p and 1080p gains exceed the unchanged prediction ranges; the 4K gain falls inside its range. These are hypothesis comparisons, not pass/fail speed bars. All six prefixes remain slower than pinned FFmpeg at one thread: 0/6 are faster, on development data only.

The automatic arm does not show a uniform incremental benefit over the safe build. Both 1080p prefixes are slightly slower in all five paired rounds; one 720p prefix has a range crossing 1.000. This is the whole-build effect of enabling the one kernel and its dispatch, not an isolated intrinsic benchmark. It can include compiler and code-layout effects. No extra kernel, retuning or replacement run follows from this result.

**Default threads were not measured at this merged commit.** The registration fixes FFmpeg at `-threads 1`; it contains no default-thread arm. The scalar build’s one-thread gap cannot be reused as a default-thread comparison. This run supplies no default-thread parity claim.

| Class | Baseline / automatic, user CPU | Baseline / automatic, wall | Safe / automatic, user CPU | Safe / automatic, wall |
|---|---:|---:|---:|---:|
| 720p | 4.01× | 3.98× | 1.010× | 1.011× |
| 1080p | 4.54× | 4.50× | 0.990× | 0.992× |
| 4K | 2.76× | 2.75× | 1.011× | 1.011× |

## Per-input times and variation

Times are median user-CPU seconds per 120 frames. The symbol-build column measures the attribution binary, not the shipped comparison baseline.

| Input | Class | Baseline | Safe | Automatic | Pinned FFmpeg | Symbol FFmpeg | Baseline / automatic, median [min, max] | Safe / automatic, median [min, max] |
|---|---|---:|---:|---:|---:|---:|---|---|
| 0 | 720p | 5.074232 | 1.965532 | 1.890892 | 0.102080 | 0.103160 | 2.6827 [2.6745, 2.6897] | 1.0392 [1.0371, 1.0437] |
| 1 | 720p | 2.671215 | 0.490464 | 0.499398 | 0.055394 | 0.055968 | 5.3433 [5.3188, 5.4676] | 0.9801 [0.9405, 1.0140] |
| 2 | 1080p | 6.468620 | 1.329988 | 1.346943 | 0.084736 | 0.083134 | 4.8024 [4.7961, 4.8290] | 0.9876 [0.9840, 0.9914] |
| 3 | 1080p | 8.427908 | 1.955894 | 1.970709 | 0.143673 | 0.142992 | 4.2766 [4.2635, 4.2856] | 0.9932 [0.9872, 0.9971] |
| 4 | 4K | 67.873536 | 25.232234 | 25.156849 | 1.148305 | 1.156844 | 2.6940 [2.6862, 2.7096] | 1.0027 [1.0008, 1.0099] |
| 5 | 4K | 60.495301 | 21.883467 | 21.462879 | 1.595878 | 1.619907 | 2.8186 [2.8133, 2.8482] | 1.0186 [1.0171, 1.0206] |

## Component estimates

These are exclusive user-cycle shares scaled to each arm’s unprofiled median user CPU, in seconds per 120 frames. Native profiles use 199 Hz and the symbol build 999 Hz, with `cycles:u`, 32 KiB DWARF stacks and a 1024-page recording buffer. Profiling occurs after all unprofiled rounds. Parent and child costs are never added together.

FFmpeg component shares come from the source-blind symbol build and are scaled to the shipped pinned binary’s CPU. Its source, archive and build log were deleted unread before profiling; it contains no debug sections, and only function names were requested. Symbol-build timing drift spans −1.89% to +1.51% across the six inputs. Component values remain attribution estimates, not component stopwatches.

Cells show estimated seconds and sample count. `†` marks fewer than 20 samples as a descriptive warning, not a new gate. `— (0)` means the profile observed no sample and supplies no estimate; it does not mean zero cost or removed work. Ratios involving such cells are omitted. Small bins, missing callers and non-equivalent reference storage make some component ratios weak evidence.

Loop filtering retains 33.8–68.1% of automatic-arm exclusive weight across the six prefixes. It is the largest named component on five prefixes; input 1 retains slightly more interpolation weight (34.8%) than filtering (33.8%). The scalar changes cut much of the old interpolation and packing cost, but the remaining FFmpeg gap is much larger than the measured safe-to-automatic difference.

### Input 0: 720p

| Component | Baseline s (samples) | Safe s (samples) | Automatic s (samples) | FFmpeg proxy s (samples) | Baseline / automatic | Automatic / FFmpeg proxy |
|---|---:|---:|---:|---:|---:|---:|
| Entropy and coefficients | 0.0601 (13†) | 0.0697 (14†) | 0.0528 (11†) | 0.0201 (1681) | 1.14× | 2.63× |
| Inverse transform | 0.0861 (20) | 0.1211 (28) | 0.0797 (18†) | 0.0006 (52) | 1.08× | 129.29× |
| Intra prediction | — (0) | 0.0092 (2†) | 0.0042 (2†) | 0.0011 (95) | — | 3.74× |
| Interpolation | 2.2200 (444) | 0.4283 (86) | 0.4376 (88) | 0.0166 (1388) | 5.07× | 26.38× |
| Other inter work | — (0) | 0.0200 (4†) | 0.0100 (2†) | 0.0075 (631) | — | 1.33× |
| Loop filter | 2.5426 (509) | 1.2823 (258) | 1.2869 (259) | 0.0519 (4343) | 1.98× | 24.78× |
| Reference packing | 0.1454 (29) | — (0) | 0.0100 (2†) | — (0) | 14.54× | — |
| Frame management | 0.0150 (3†) | — (0) | 0.0050 (1†) | 0.0009 (75) | 3.01× | 5.64× |
| Shown-picture copy | 0.0050 (1†) | — (0) | 0.0048 (1†) | — (0) | 1.04× | — |
| Unresolved | — (0) | 0.0349 (7†) | — (0) | 0.0033 (280) | — | — |

### Input 1: 720p

| Component | Baseline s (samples) | Safe s (samples) | Automatic s (samples) | FFmpeg proxy s (samples) | Baseline / automatic | Automatic / FFmpeg proxy |
|---|---:|---:|---:|---:|---:|---:|
| Entropy and coefficients | 0.0540 (12†) | 0.0640 (14†) | 0.0607 (16†) | 0.0264 (2284) | 0.89× | 2.30× |
| Inverse transform | 0.0379 (9†) | 0.0467 (11†) | 0.0294 (7†) | 0.0007 (59) | 1.29× | 43.15× |
| Intra prediction | 0.0261 (7†) | 0.0201 (6†) | 0.0132 (3†) | 0.0028 (247) | 1.98× | 4.77× |
| Interpolation | 1.9988 (400) | 0.1543 (31) | 0.1736 (35) | 0.0107 (949) | 11.52× | 16.15× |
| Other inter work | 0.0150 (3†) | 0.0150 (3†) | 0.0200 (4†) | 0.0040 (351) | 0.75× | 4.95× |
| Loop filter | 0.3552 (71) | 0.1598 (32) | 0.1690 (34) | 0.0071 (597) | 2.10× | 23.70× |
| Reference packing | 0.1541 (32) | 0.0050 (1†) | — (0) | — (0) | — | — |
| Frame management | 0.0150 (3†) | 0.0093 (2†) | — (0) | 0.0003 (28) | — | — |
| Shown-picture copy | 0.0150 (3†) | 0.0113 (3†) | — (0) | — (0) | — | — |
| Unresolved | — (0) | 0.0049 (1†) | 0.0336 (7†) | 0.0033 (294) | — | 10.22× |

### Input 2: 1080p

| Component | Baseline s (samples) | Safe s (samples) | Automatic s (samples) | FFmpeg proxy s (samples) | Baseline / automatic | Automatic / FFmpeg proxy |
|---|---:|---:|---:|---:|---:|---:|
| Entropy and coefficients | 0.0860 (19†) | 0.0717 (17†) | 0.0931 (21) | 0.0181 (745) | 0.92× | 5.14× |
| Inverse transform | 0.0014 (1†) | — (0) | — (0) | 0.0001 (4†) | — | — |
| Intra prediction | 0.0009 (1†) | 0.0036 (2†) | 0.0031 (1†) | 0.0006 (25) | 0.28× | 5.35× |
| Interpolation | 4.7303 (947) | 0.4115 (84) | 0.3658 (74) | 0.0299 (1225) | 12.93× | 12.22× |
| Other inter work | 0.0050 (1†) | 0.0097 (2†) | — (0) | 0.0089 (363) | — | — |
| Loop filter | 1.2542 (251) | 0.7790 (158) | 0.7818 (158) | 0.0221 (905) | 1.60× | 35.33× |
| Reference packing | 0.3407 (68) | 0.0248 (5†) | — (0) | — (0) | — | — |
| Frame management | — (0) | — (0) | — (0) | 0.0008 (33) | — | — |
| Shown-picture copy | — (0) | 0.0298 (6†) | — (0) | — (0) | — | — |
| Unresolved | 0.0501 (10†) | — (0) | 0.1030 (22) | 0.0043 (176) | 0.49× | 24.18× |

### Input 3: 1080p

| Component | Baseline s (samples) | Safe s (samples) | Automatic s (samples) | FFmpeg proxy s (samples) | Baseline / automatic | Automatic / FFmpeg proxy |
|---|---:|---:|---:|---:|---:|---:|
| Entropy and coefficients | 0.1663 (37) | 0.1123 (26) | 0.1505 (34) | 0.0486 (2000) | 1.10× | 3.10× |
| Inverse transform | 0.0538 (11†) | 0.0702 (15†) | 0.0583 (12†) | 0.0011 (46) | 0.92× | 51.87× |
| Intra prediction | 0.0094 (2†) | 0.0195 (4†) | 0.0170 (4†) | 0.0026 (108) | 0.55× | 6.49× |
| Interpolation | 5.0030 (999) | 0.4248 (86) | 0.4359 (88) | 0.0327 (1354) | 11.48× | 13.32× |
| Other inter work | 0.0100 (2†) | 0.0147 (3†) | 0.0199 (4†) | 0.0123 (510) | 0.50× | 1.62× |
| Loop filter | 2.8043 (558) | 1.2255 (246) | 1.2402 (249) | 0.0401 (1644) | 2.26× | 30.94× |
| Reference packing | 0.3461 (69) | — (0) | 0.0290 (6†) | — (0) | 11.95× | — |
| Frame management | — (0) | — (0) | — (0) | 0.0013 (54) | — | — |
| Shown-picture copy | — (0) | — (0) | 0.0199 (4†) | — (0) | — | — |
| Unresolved | 0.0350 (7†) | 0.0889 (18†) | — (0) | 0.0049 (204) | — | — |

### Input 4: 4K

| Component | Baseline s (samples) | Safe s (samples) | Automatic s (samples) | FFmpeg proxy s (samples) | Baseline / automatic | Automatic / FFmpeg proxy |
|---|---:|---:|---:|---:|---:|---:|
| Entropy and coefficients | 0.8899 (179) | 0.8641 (177) | 0.8185 (168) | 0.2531 (1293) | 1.09× | 3.23× |
| Inverse transform | 5.3335 (1065) | 5.0936 (1030) | 5.0748 (1026) | 0.0223 (114) | 1.05× | 227.46× |
| Intra prediction | 0.5049 (104) | 0.2889 (61) | 0.3543 (74) | 0.0759 (389) | 1.43× | 4.67× |
| Interpolation | 21.0176 (4203) | 2.7093 (552) | 2.5476 (519) | 0.1909 (978) | 8.25× | 13.35× |
| Other inter work | 0.1401 (28) | 0.1230 (25) | 0.1376 (28) | 0.0423 (217) | 1.02× | 3.25× |
| Loop filter | 38.1808 (7602) | 15.4342 (3111) | 15.4174 (3110) | 0.5456 (2795) | 2.48× | 28.26× |
| Reference packing | 1.6051 (321) | — (0) | — (0) | — (0) | — | — |
| Frame management | 0.0860 (18†) | — (0) | 0.0045 (1†) | 0.0031 (20) | 19.11× | 1.45× |
| Shown-picture copy | 0.1155 (23) | — (0) | — (0) | — (0) | — | — |
| Unresolved | — (0) | 0.7190 (147) | 0.8021 (164) | 0.0152 (80) | — | 52.89× |

### Input 5: 4K

| Component | Baseline s (samples) | Safe s (samples) | Automatic s (samples) | FFmpeg proxy s (samples) | Baseline / automatic | Automatic / FFmpeg proxy |
|---|---:|---:|---:|---:|---:|---:|
| Entropy and coefficients | 1.3939 (280) | 1.2399 (252) | 1.2811 (259) | 0.6049 (3090) | 1.09× | 2.12× |
| Inverse transform | 1.3945 (279) | 1.4753 (296) | 1.3784 (277) | 0.0137 (70) | 1.01× | 100.45× |
| Intra prediction | 0.0898 (20) | 0.0860 (19†) | 0.0995 (22) | 0.0219 (113) | 0.90× | 4.54× |
| Interpolation | 20.9061 (4179) | 3.8853 (780) | 3.4910 (702) | 0.2458 (1255) | 5.99× | 14.20× |
| Other inter work | 0.3007 (60) | 0.3437 (69) | 0.3735 (75) | 0.1244 (636) | 0.81× | 3.00× |
| Loop filter | 34.4929 (6871) | 14.4035 (2873) | 14.4838 (2892) | 0.5601 (2862) | 2.38× | 25.86× |
| Reference packing | 1.4963 (299) | 0.0050 (1†) | 0.1503 (30) | — (0) | 9.95× | — |
| Frame management | 0.0050 (1†) | 0.0048 (1†) | 0.0710 (16†) | 0.0022 (11†) | 0.07× | 32.87× |
| Shown-picture copy | — (0) | — (0) | 0.1343 (27) | — (0) | — | — |
| Unresolved | 0.4159 (85) | 0.4402 (89) | — (0) | 0.0228 (122) | — | — |

## Profile quality and reconciliation

All 24 cells retain a complete graph and flat report. Their exclusive periods and sample counts reconcile with the component totals. Recorded lost samples total zero. Unknown leaves can be assigned through known callers, so unknown-leaf, missing-caller, leaf-only and unresolved shares overlap and must not be summed. Missing callers reach 67.4% in one FFmpeg profile; inferred ownership is not a complete call tree.

| Input | Arm | Samples | Lost | Unknown leaf | Missing caller | Leaf only | Unresolved |
|---|---|---:|---:|---:|---:|---:|---:|
| 0 | baseline-dist | 1019 | 0 | 0.49% | 0.00% | 0.00% | 0.00% |
| 0 | merged-safe-dist | 399 | 0 | 2.02% | 1.77% | 1.27% | 1.77% |
| 0 | merged-auto-dist | 384 | 0 | 2.10% | 0.00% | 0.00% | 0.00% |
| 0 | ffmpeg-symbols | 8545 | 0 | 1.63% | 34.22% | 65.73% | 3.27% |
| 1 | baseline-dist | 540 | 0 | 1.49% | 0.00% | 0.00% | 0.00% |
| 1 | merged-safe-dist | 104 | 0 | 7.08% | 0.00% | 0.00% | 1.00% |
| 1 | merged-auto-dist | 106 | 0 | 7.00% | 6.72% | 4.01% | 6.72% |
| 1 | ffmpeg-symbols | 4809 | 0 | 3.09% | 67.35% | 35.39% | 5.93% |
| 2 | baseline-dist | 1298 | 0 | 0.77% | 0.70% | 0.40% | 0.78% |
| 2 | merged-safe-dist | 274 | 0 | 4.25% | 0.00% | 0.00% | 0.00% |
| 2 | merged-auto-dist | 276 | 0 | 7.71% | 7.65% | 3.61% | 7.65% |
| 2 | ffmpeg-symbols | 3476 | 0 | 2.89% | 38.77% | 61.10% | 5.03% |
| 3 | baseline-dist | 1685 | 0 | 0.42% | 0.42% | 0.18% | 0.42% |
| 3 | merged-safe-dist | 398 | 0 | 4.55% | 4.55% | 2.58% | 4.55% |
| 3 | merged-auto-dist | 401 | 0 | 3.59% | 0.00% | 0.04% | 0.00% |
| 3 | ffmpeg-symbols | 5920 | 0 | 1.91% | 49.85% | 49.95% | 3.41% |
| 4 | baseline-dist | 13543 | 0 | 0.98% | 0.00% | 0.01% | 0.00% |
| 4 | merged-safe-dist | 5103 | 0 | 2.83% | 2.83% | 1.18% | 2.85% |
| 4 | merged-auto-dist | 5090 | 0 | 3.21% | 3.21% | 1.32% | 3.19% |
| 4 | ffmpeg-symbols | 5886 | 0 | 0.89% | 37.03% | 62.75% | 1.32% |
| 5 | baseline-dist | 12074 | 0 | 0.66% | 0.65% | 0.26% | 0.69% |
| 5 | merged-safe-dist | 4380 | 0 | 2.01% | 2.03% | 1.12% | 2.01% |
| 5 | merged-auto-dist | 4300 | 0 | 2.26% | 0.00% | 0.00% | 0.00% |
| 5 | ffmpeg-symbols | 8159 | 0 | 0.79% | 50.28% | 50.44% | 1.43% |

## Limits

Two prefixes per class and five paired rounds describe this development diagnostic only. Observed ranges are not statistical confidence intervals, and a small safe/automatic difference is not evidence of a general SIMD gain. The selected prefixes, warm input loops, short native decodes and different reference-storage paths limit generalization. No held-out speed, default-thread result, broader-profile coverage or release-selection change is established.

The [public summary](summary.json) retains full-precision times, paired observations, all profile components and quality fields. The [provenance](provenance.json) binds those observations to the merged source, compiler, binaries, scripts and observed host. Replaying retained raw command outputs, rather than trusting stored pass flags, reproduces this summary.
