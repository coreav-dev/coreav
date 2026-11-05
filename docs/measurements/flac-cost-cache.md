# Native FLAC cost-cache comparison

The registered comparison completed, but it does not establish a qualified speed pass. The observed candidate/before-change median elapsed-time ratio is **0.4863 [0.4693, 0.5071]**, below the 0.60 milestone overall and in every class. Four cases miss the reference-work/startup condition, so the qualified milestone remains false. This completed outcome was not rerun.

Against default FFmpeg, the overall median ratio is **4.3876 [3.5034, 5.7547]**, with **0/24** cases faster. Every class misses the unchanged 1.00 reference-speed ceiling. Native size is **84,658,928 / 92,659,437 = 0.9137** of the reference: the 1.05 total and all compressible-case 1.10 size ceilings pass. Both native arms emit identical complete files.

All 24 cases, 216 complete triples and 747 encode/calibration invocations verify, with zero failed invocations. The remote exit code 1 reports the failed verdict, not an infrastructure error or failed encode. There is no timeout, incomplete case, excluded case or replacement arm.

## Registration and provenance

The [merged plan](../design/flac-cost-cache.md) fixes both optimizations, the before-change encoder, commands, ordering and bars. Candidate source is merged main `62f4af2a4a3adb477583f91d396f6daeaebc2055`, tree `63ad632f9662c472bf81a88acd53987640c4d7c7`. The exact build, tools, source files, PCM hashes, host and analysis were registered in Git before dispatch on 2026-11-04. The capture registration SHA-256 is `25eb161a40cda8e90e5754944a0092bb4e39753c5f878c0e70eb9d01de7c002d`.

The before arm uses that same merged tree and build environment, restoring only `crates/coreav-audio/src/flac/encode.rs` from `cbe3f6db46668fbd910c8a3a21d92802fe759e3a`, blob `8626b99fb83072129160cf99f35ff2805398c3a0`. Candidate encoder SHA-256 is `1ccb0c4a52f24bc9e107a7f295476e8368f2961b2b0547720b2d2485fc87b983`; before encoder SHA-256 is `a12f68790e31ce8bbb4567ee86f750b7ae99f04cdadf7da286597519ddfac665`. No other CLI, decoder or output option differs between native arms.

Both executables were built with `cargo build --locked --release -p coreav-cli`, rustc 1.99.1, LLVM 23.1.1, empty `RUSTFLAGS` and `CARGO_INCREMENTAL=0`, targeting x86_64-unknown-linux-gnu. Before freezing these exact builds, candidate library/fixture tests passed 61/7 and before tests passed 58/7; all 1,152 complete-file comparisons matched. The merged implementation also passed all 93 CLI comparisons and the unchanged startup guard.

The [raw record](flac-cost-cache.json) is byte-identical to capture, with SHA-256 `372000c03218e89cc24c7120ae2261d3fb0297778baa3a4afed3b2ceb3c59180`. It contains complete triples, every normalized invocation and exit, verification flags, output identities, calibration, source/build/binary hashes and host details. The candidate CLI SHA-256 is `03343621193f85f29f7e09ea92657821d269e4e26cb4f08f89cfe0a4aaa9126f`; before CLI SHA-256 is `bbf45f13da914971e3e23a6dabed473d59695121041128be87ee98b6a58037ec`. Independent replay reproduces every statistic and verdict; `scripts/tests/test_flac_cost_cache_record.py` also recomputes both ratio axes with the unchanged historical summary function.

The [first comparison](flac-output.md), [second comparison](flac-output-round2.md) and their raw records remain unchanged. The earlier rounds used different shared hosts; comparing their absolute ratios with this fixed-host round is not a causal estimate of the optimization.

## Host and method

Execution used an on-demand STANDARD c4-standard-8 in us-central1-a, Intel Emerald Rapids (Xeon Platinum 8581C), SMT disabled, STANDARD PMU and ALL_CORE_MAX. The driver and all children were pinned to guest logical CPU 0. The host ran Debian 13, kernel `6.12.113+deb13-cloud-amd64`, Python 3.13.5 and glibc 2.41. Provider receipt, one-thread-per-core topology and fully scheduled instruction/cycle preflight passed before timing. Input and output used the same local boot disk. The raw record retains before/after load and CPU topology.

The frozen 24 cases cover tones, polynomial signals and noise; 16/24-bit samples; mono/stereo; and 44.1/48 kHz. Each is 30 seconds, with PCM hashes matching the previous roster. These are already-used synthetic development inputs, not held-out evidence about real-file performance.

The reference is pinned FFmpeg `n9.0.2-17-g2a571b6068-20260930` at default FLAC settings, not the bounds-aligned diagnostic configuration. Each arm decodes the same FLAC-in-Matroska input and writes a complete FLAC file, stripping metadata and selecting one thread. Native commands require strict-safe execution and disable fallback. Full normalized commands are retained per invocation.

Each case has one untimed warmup per arm, then nine triples. Candidate/reference order alternates; before-change is inserted at position `i modulo 3`, giving three calls at each position. Complete output samples, CRC, STREAMINFO MD5 and source probe fields are checked after every encode. Both native outputs must match as complete files on every invocation, not only decode to equal samples. Nine null calls, 72 warmups, 18 A/A calls and 648 timed calls account for all 747 invocations.

Ratios are computed within complete triples. Case summaries are medians of nine ratios; class and overall medians weight cases equally. The joint percentile bootstrap resamples cases and complete triples, preserving both ratio axes together, with seed zero and 10,000 replicates. Intervals describe this synthetic roster, not a held-out population. Every case's median is below 1.00 against before-change (24/24); none is below 1.00 against default FFmpeg (0/24).

Reference/reference A/A is **1.000836**, inside 0.90–1.10. Median null time is **0.007883 seconds**, so the five-times-null threshold is **0.039416 seconds**. Four cases fail it: 16-bit mono polynomial and noise at both sample rates. They remain in all summaries, and neither a qualified CPU milestone nor a qualified reference-speed pass is granted. This is a completed calibration/bar miss, not a host fault authorizing a rescue rerun.

The timed region covers the complete command, including startup, input decoding and output I/O. No profile was collected in this round, so it does not assign the remaining cost to a component or separate the bundle's two optimizations. The earlier [component-erasure diagnostics](../design/flac-cost-cache.md#evidence-before-choosing-the-change) motivated the bundle; their replay ratios are not shipped-encoder speed claims.

## Observed timing by class

Both ratios use candidate elapsed time as the numerator. The observed before-change medians meet 0.60, but the failed startup qualification remains part of the overall verdict.

| Class | Cases | Candidate/default FFmpeg | Paired 95% interval | Candidate/before-change | Paired 95% interval |
|---|---:|---:|---|---:|---|
| tones | 8 | 4.1743 | [2.7412, 5.7732] | 0.4713 | [0.4363, 0.5052] |
| polynomial | 8 | 4.7091 | [3.1574, 8.3801] | 0.5064 | [0.4625, 0.5549] |
| noise | 8 | 4.3876 | [3.5264, 6.7140] | 0.4871 | [0.4642, 0.5122] |
| overall | 24 | 4.3876 | [3.5034, 5.7547] | 0.4863 | [0.4693, 0.5071] |

## Every case

Native byte counts are identical in candidate and before-change. Size includes metadata and headers. The per-case 1.10 size ceiling applies to tones and polynomial cases; noise remains in the total. Every row has nine verified triples.

| Case | Native bytes | Reference bytes | Size ratio | Candidate/reference time | Candidate/before time | Reference work at least 5× null |
|---|---:|---:|---:|---:|---:|---|
| tones-16-1-44100 | 622,658 | 653,016 | 0.9535 | 2.7505 | 0.4690 | yes |
| tones-16-1-48000 | 670,173 | 694,691 | 0.9647 | 2.7796 | 0.4658 | yes |
| tones-16-2-44100 | 1,042,794 | 1,127,864 | 0.9246 | 5.7829 | 0.4342 | yes |
| tones-16-2-48000 | 1,118,290 | 1,200,994 | 0.9311 | 5.9101 | 0.4362 | yes |
| tones-24-1-44100 | 1,536,656 | 1,699,795 | 0.9040 | 2.7321 | 0.5115 | yes |
| tones-24-1-48000 | 1,678,614 | 1,834,457 | 0.9150 | 2.7342 | 0.5080 | yes |
| tones-24-2-44100 | 2,248,134 | 3,100,714 | 0.7250 | 5.5691 | 0.4735 | yes |
| tones-24-2-48000 | 2,401,504 | 3,363,953 | 0.7139 | 5.6126 | 0.4753 | yes |
| polynomial-16-1-44100 | 2,649,790 | 2,657,390 | 0.9971 | 3.5770 | 0.4936 | no |
| polynomial-16-1-48000 | 2,884,138 | 2,891,678 | 0.9974 | 3.6511 | 0.4927 | no |
| polynomial-16-2-44100 | 5,296,113 | 5,469,088 | 0.9684 | 8.4464 | 0.4612 | yes |
| polynomial-16-2-48000 | 5,764,490 | 5,951,742 | 0.9685 | 8.6424 | 0.4624 | yes |
| polynomial-24-1-44100 | 2,439,274 | 3,196,535 | 0.7631 | 2.8369 | 0.5570 | yes |
| polynomial-24-1-48000 | 2,655,238 | 3,480,361 | 0.7629 | 2.8800 | 0.5566 | yes |
| polynomial-24-2-44100 | 4,871,659 | 6,383,989 | 0.7631 | 5.7671 | 0.5196 | yes |
| polynomial-24-2-48000 | 5,301,341 | 6,951,420 | 0.7626 | 5.8705 | 0.5192 | yes |
| noise-16-1-44100 | 2,649,790 | 2,657,390 | 0.9971 | 3.6322 | 0.4943 | no |
| noise-16-1-48000 | 2,884,138 | 2,891,678 | 0.9974 | 3.7243 | 0.4944 | no |
| noise-16-2-44100 | 5,296,113 | 5,399,084 | 0.9809 | 6.7157 | 0.4623 | yes |
| noise-16-2-48000 | 5,764,490 | 5,874,909 | 0.9812 | 6.8673 | 0.4632 | yes |
| noise-24-1-44100 | 3,972,790 | 3,980,390 | 0.9981 | 3.4271 | 0.5138 | yes |
| noise-24-1-48000 | 4,324,138 | 4,331,678 | 0.9983 | 3.5105 | 0.5144 | yes |
| noise-24-2-44100 | 7,942,113 | 8,076,537 | 0.9834 | 5.0508 | 0.4799 | yes |
| noise-24-2-48000 | 8,644,490 | 8,790,084 | 0.9834 | 5.0898 | 0.4778 | yes |
