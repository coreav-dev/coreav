# Native FLAC output comparison

The registered comparison failed its size bar and did not meet the diagnostic speed bar. All 24 cases completed, with 216 verified encode pairs and no failed encode invocation. No rerun was made.

Total native output was 94,279,247 bytes against 92,659,437 reference bytes, a ratio of **1.0175**. This meets the aggregate ceiling of 1.05, but four 24-bit polynomial cases exceed the per-compressible-case ceiling of 1.10. Their ratios range from 1.2424 to 1.2441, so the size bar fails.

The overall median native/reference elapsed-time ratio was **5.9600**, with a paired 95% interval of **[4.2072, 8.5246]**. No case had a median ratio below 1.00. These are shared-host diagnostics, not a fixed-host performance claim or evidence about unseen real files.

## Registration and evidence

The [plan](../design/flac-output.md#size-and-speed-plan) and driver merged before the comparison. Capture used main commit `17ea08c6d9f1b8bf152e6f1385db931e757b9a74` on 2026-11-03. Its encoder, Linux oracle, timer, plan and driver were unchanged from the FLAC merge at `1cf20f438f16a5cb07a21a19978a837e643e015f`.

- [Raw measurement](flac-output.json) retains every input PCM hash, pair, exit status, verification flag, calibration and outcome, byte-for-byte as captured. Its SHA-256 is `c06f814e9ec24b0e832503b12c2327dd130ddc0fdf84eaf4da4686d59b20937a`.
- [Correctness results](flac-output-correctness.json) retain all 93 exact command comparisons from the same job. Native frame tests and the unjudged driver smoke also passed before measurement.
- The measurement binds the candidate binary, reference binary, plan and driver by SHA-256. The plan hash is `182cff794f998797ed4c5680a6acef6a349eacb3e495683094562ecfe632cea9`; the driver hash is `08fc261b9808f9373d135e7f28d7e83118b07ce834037bd93108cce5fefcca42`.

The workflow returned failure after completing the measurement because its registered bars were unmet. This is a completed failed comparison, not an interrupted or incomplete capture. Neither thresholds nor evidence were changed to make it pass.

## Method and host

The host was a GitHub-hosted Ubuntu 24.04 runner with an AMD EPYC 9V74 80-Core Processor. The driver and both children were pinned to logical CPU 0. This does not establish physical-core isolation, an SMT-off host or a fixed host class. Both commands used the runner's temporary directory for input and output.

The reference was pinned FFmpeg `n9.0.2-17-g2a571b6068-20260930`, with default FLAC compression. CoreAV used a Cargo release build. Both commands decoded the same generated FLAC-in-Matroska source, encoded FLAC and wrote the complete output file, using one input thread and one output thread. This is an end-to-end command comparison, not an isolated encoder measurement.

```sh
coreav --strict-safe ffmpeg -i SOURCE.mka -y -map_metadata -1 -threads 1 NATIVE.flac
ffmpeg -v error -threads 1 -i SOURCE.mka -y -map_metadata -1 -threads 1 REFERENCE.flac
```

The frozen generator crossed three classes, tones, changing piecewise polynomial signals and full-range deterministic noise, with 16-bit and 24-bit samples, mono and stereo, and 44,100 and 48,000 Hz. Each case contained 30 seconds of audio. Each side had one untimed warm-up, followed by nine pairs with alternating first side. Every accepted pair required complete independent sample decoding, matching probe fields and the native STREAMINFO MD5.

Case ratios are medians of the nine native/reference elapsed-time pairs. Class and overall medians weight cases equally. The percentile interval uses 10,000 resamples of cases and their complete pairs, with seed zero. All per-call times are in the raw measurement. Native calls ranged from 0.152407 to 0.917358 seconds; reference calls ranged from 0.039653 to 0.096859 seconds.

Nine null `ffmpeg -version` calls had a median of 0.008578 seconds. Nine reference/reference pairs had a median ratio of 0.999042, inside the registered inclusive range 0.90 to 1.10. Four cases failed the requirement that median reference work take at least five times the null median: 16-bit mono polynomial and noise at both rates. Their work/null ratios were 4.6827, 4.9324, 4.7038 and 4.9398 respectively. All cases remain in the report; none was dropped to improve a median.

No profile or CPU-versus-I/O attribution was captured, so these records do not identify the performance limiter. The registered shared-host diagnostics cannot establish fixed-host speed parity.

## Diagnostic timing by class

The registered median ceiling is 1.00 in every class and overall. A faster claim also requires the interval's upper bound below 1.00. Neither condition was met.

| Class | Cases | Median native/reference | Paired 95% interval | Cases with median below 1.00 |
|---|---:|---:|---|---:|
| tones | 8 | 5.3490 | [3.3071, 8.5103] | 0/8 |
| polynomial | 8 | 5.2719 | [3.4009, 11.7956] | 0/8 |
| noise | 8 | 7.4480 | [5.1952, 12.4444] | 0/8 |
| overall | 24 | 5.9600 | [4.2072, 8.5246] | 0/24 |

## Every case

Size includes headers and metadata. The per-case size ceiling of 1.10 applies to tones and polynomial cases, not noise. Case names give class, stored bits, channels and sample rate. All rows have nine verified pairs.

| Case | Native bytes | Reference bytes | Size ratio | Median time ratio | Reference work at least 5× null |
|---|---:|---:|---:|---:|---|
| tones-16-1-44100 | 622,658 | 653,016 | 0.9535 | 3.5010 | yes |
| tones-16-1-48000 | 670,173 | 694,691 | 0.9647 | 3.5297 | yes |
| tones-16-2-44100 | 1,042,794 | 1,127,864 | 0.9246 | 8.5426 | yes |
| tones-16-2-48000 | 1,118,290 | 1,200,994 | 0.9311 | 8.7448 | yes |
| tones-24-1-44100 | 1,537,117 | 1,699,795 | 0.9043 | 3.1420 | yes |
| tones-24-1-48000 | 1,679,637 | 1,834,457 | 0.9156 | 3.1875 | yes |
| tones-24-2-44100 | 2,249,893 | 3,100,714 | 0.7256 | 7.1682 | yes |
| tones-24-2-48000 | 2,402,561 | 3,363,953 | 0.7142 | 7.1896 | yes |
| polynomial-16-1-44100 | 2,649,790 | 2,657,390 | 0.9971 | 4.0588 | no |
| polynomial-16-1-48000 | 2,884,138 | 2,891,678 | 0.9974 | 4.1996 | no |
| polynomial-16-2-44100 | 5,296,113 | 5,469,088 | 0.9684 | 11.8203 | yes |
| polynomial-16-2-48000 | 5,764,490 | 5,951,742 | 0.9685 | 12.0681 | yes |
| polynomial-24-1-44100 | 3,972,790 | 3,196,535 | 1.2428 | 2.7927 | yes |
| polynomial-24-1-48000 | 4,324,138 | 3,480,361 | 1.2424 | 2.8406 | yes |
| polynomial-24-2-44100 | 7,942,113 | 6,383,989 | 1.2441 | 6.3442 | yes |
| polynomial-24-2-48000 | 8,644,490 | 6,951,420 | 1.2436 | 6.4371 | yes |
| noise-16-1-44100 | 2,649,790 | 2,657,390 | 0.9971 | 5.3739 | no |
| noise-16-1-48000 | 2,884,138 | 2,891,678 | 0.9974 | 5.5758 | no |
| noise-16-2-44100 | 5,296,113 | 5,399,084 | 0.9809 | 12.4942 | yes |
| noise-16-2-48000 | 5,764,490 | 5,874,909 | 0.9812 | 12.7497 | yes |
| noise-24-1-44100 | 3,972,790 | 3,980,390 | 0.9981 | 5.0636 | yes |
| noise-24-1-48000 | 4,324,138 | 4,331,678 | 0.9983 | 5.1314 | yes |
| noise-24-2-44100 | 7,942,113 | 8,076,537 | 0.9834 | 9.3202 | yes |
| noise-24-2-48000 | 8,644,490 | 8,790,084 | 0.9834 | 9.5085 | yes |
