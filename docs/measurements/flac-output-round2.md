# Native FLAC output comparison after residual changes

The second registered comparison passes the size bar but misses the diagnostic speed bar. All 24 cases completed with 216 verified pairs and no failed encode invocation. The workflow's exit code 1 records that bar miss, not an infrastructure failure, timeout or failed CoreAV command. It was not rerun.

Native output totals **84,658,928 bytes** against **92,659,437 reference bytes**, a ratio of **0.9137**. This meets the aggregate ceiling of 1.05, and every compressible case meets its individual 1.10 ceiling. Noise remains included in the total.

The overall median native/reference elapsed-time ratio is **6.9653**, with a paired 95% interval of **[4.8543, 9.2975]**. None of the 24 cases has a median below 1.00. Every class exceeds its 1.00 median ceiling, and six cases also fail the reference-work/null requirement. These timings remain shared-host diagnostics, not a fixed-host performance claim.

## Registration and evidence

The [original failed comparison](flac-output.md) and its raw evidence remain unchanged. This round uses the same [registration](../design/flac-output.md#size-and-speed-plan), driver, generated input hashes, default reference and bars. Capture used main `f0b60dcc37a4e1ca0f03f3ce8ce4a8deb9008556` on 2026-11-03, after the completed residual coder and byte-identical residual-loop optimization merged. It is not a paired before/after measurement of the CPU optimization.

- [Raw measurement](flac-output-round2.json) retains every input hash, pair, exit status, verification flag, calibration and outcome byte-for-byte. Its SHA-256 is `3318600f231cdd4e64d84c9a745b8cd0cad2d79775ed611eaf42ac4d37c57c81`.
- [Correctness results](flac-output-round2-correctness.json) retain all 93 exact CLI comparisons. Native frame tests, 1,152 complete-file byte comparisons against the residual-coder baseline and the unjudged driver smoke passed before measurement.
- The unchanged plan SHA-256 is `182cff794f998797ed4c5680a6acef6a349eacb3e495683094562ecfe632cea9`; the unchanged driver SHA-256 is `08fc261b9808f9373d135e7f28d7e83118b07ce834037bd93108cce5fefcca42`. The report also binds both binaries by SHA-256.

The host was a GitHub-hosted Ubuntu 24.04 runner with an AMD EPYC 9V74 80-Core Processor. Both children and the driver were pinned to logical CPU 0, with input and output in the runner's temporary directory. This does not establish physical-core isolation, SMT-off execution or a fixed host class.

The reference was pinned FFmpeg `n9.0.2-17-g2a571b6068-20260930`, using default FLAC compression. CoreAV used a Cargo release build. Both commands decoded the same generated FLAC-in-Matroska input and wrote complete FLAC output with one thread. Each side had one untimed warm-up and nine alternating-order pairs per case. Complete samples, probe fields and native STREAMINFO MD5 were verified after every pair. See the unchanged registration for the commands and all acceptance predicates.

Nine reference/reference pairs have a median ratio of **0.998463**, inside the inclusive 0.90–1.10 calibration range. Nine null invocations have a median of **0.009379 seconds**. The six failed work/null cases are 16-bit mono polynomial and noise, and 24-bit mono noise, at both rates. They remain in every applicable summary; no case was excluded to improve the verdict.

Case ratios are medians of nine paired ratios. Class and overall summaries weight cases equally. The paired percentile intervals use 10,000 resamples of cases and their complete pairs with seed zero. No profile was captured in this round, so it does not identify the current performance limiter or separate encoder work from source decoding, process startup and output I/O.

## Diagnostic timing by class

| Class | Cases | Median native/reference | Paired 95% interval | Cases below 1.00 |
|---|---:|---:|---|---:|
| tones | 8 | 6.7104 | [3.9574, 10.0861] | 0/8 |
| polynomial | 8 | 7.0972 | [4.3226, 13.4175] | 0/8 |
| noise | 8 | 6.9653 | [4.8898, 11.4534] | 0/8 |
| overall | 24 | 6.9653 | [4.8543, 9.2975] | 0/24 |

## Every case

Size includes headers and metadata. The individual 1.10 ceiling applies to tones and polynomial cases, not noise. Every row has nine verified pairs; the speed bar also requires reference work of at least five times the null median.

| Case | Native bytes | Reference bytes | Size ratio | Median time ratio | Reference work at least 5× null |
|---|---:|---:|---:|---:|---|
| tones-16-1-44100 | 622,658 | 653,016 | 0.9535 | 4.1264 | yes |
| tones-16-1-48000 | 670,173 | 694,691 | 0.9647 | 4.2130 | yes |
| tones-16-2-44100 | 1,042,794 | 1,127,864 | 0.9246 | 10.1198 | yes |
| tones-16-2-48000 | 1,118,290 | 1,200,994 | 0.9311 | 10.3012 | yes |
| tones-24-1-44100 | 1,536,656 | 1,699,795 | 0.9040 | 3.8882 | yes |
| tones-24-1-48000 | 1,678,614 | 1,834,457 | 0.9150 | 3.9424 | yes |
| tones-24-2-44100 | 2,248,134 | 3,100,714 | 0.7250 | 9.2078 | yes |
| tones-24-2-48000 | 2,401,504 | 3,363,953 | 0.7139 | 9.4018 | yes |
| polynomial-16-1-44100 | 2,649,790 | 2,657,390 | 0.9971 | 4.9461 | no |
| polynomial-16-1-48000 | 2,884,138 | 2,891,678 | 0.9974 | 5.0775 | no |
| polynomial-16-2-44100 | 5,296,113 | 5,469,088 | 0.9684 | 13.4391 | yes |
| polynomial-16-2-48000 | 5,764,490 | 5,951,742 | 0.9685 | 13.7277 | yes |
| polynomial-24-1-44100 | 2,439,274 | 3,196,535 | 0.7631 | 3.7371 | yes |
| polynomial-24-1-48000 | 2,655,238 | 3,480,361 | 0.7629 | 3.8451 | yes |
| polynomial-24-2-44100 | 4,871,659 | 6,383,989 | 0.7631 | 9.1169 | yes |
| polynomial-24-2-48000 | 5,301,341 | 6,951,420 | 0.7626 | 9.2975 | yes |
| noise-16-1-44100 | 2,649,790 | 2,657,390 | 0.9971 | 5.0797 | no |
| noise-16-1-48000 | 2,884,138 | 2,891,678 | 0.9974 | 5.2300 | no |
| noise-16-2-44100 | 5,296,113 | 5,399,084 | 0.9809 | 11.4938 | yes |
| noise-16-2-48000 | 5,764,490 | 5,874,909 | 0.9812 | 11.6913 | yes |
| noise-24-1-44100 | 3,972,790 | 3,980,390 | 0.9981 | 4.6795 | no |
| noise-24-1-48000 | 4,324,138 | 4,331,678 | 0.9983 | 4.8594 | no |
| noise-24-2-44100 | 7,942,113 | 8,076,537 | 0.9834 | 8.7007 | yes |
| noise-24-2-48000 | 8,644,490 | 8,790,084 | 0.9834 | 8.7433 | yes |
