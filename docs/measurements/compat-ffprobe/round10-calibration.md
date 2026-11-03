# B4 round 10 development calibration

Both registered calibration controls pass. This is a development-only calibration result, not a B4 product speed result: no product or held-out input was opened, and `b4_passes` is null.

## Verdict

The [registered plan](round10-plan.md) and [calibration design](../../design/b4-calibration.md) govern this attempt. The method is `b4-counterbalanced-v2`, implemented in coreav-dev/coreav#283. Both point estimates must lie in [0.98, 1.02], both **99.5% gate intervals** must contain 1, every planned call must complete, and fallback records must be empty. The 95% intervals below are diagnostic, not acceptance thresholds.

| Identical-argument control | Estimate | 99.5% gate interval | 95% diagnostic interval | Gate |
|---|---:|---|---|---|
| FFprobe / FFprobe (`aa`) | 0.999789 | [0.995891, 1.006069] | [0.997430, 1.003233] | Pass |
| CoreAV / CoreAV (`null`) | 1.001459 | [0.996143, 1.005693] | [0.997598, 1.004717] | Pass |

All 4,992 planned child calls completed: 3,072 timed and 1,920 conditioning calls, with zero missing or failed calls. There were no fallback records, refusals or aborts. Each arm has 1,536 timed calls across 24 development files, two commands and 16 timed pairs per case. Every selected container has eight files. The timed schedule was fixed before execution; no input, round or case was dropped or repeated after observing its result.

The two commands are `-show_format -show_streams` and `-show_packets`. Four global visits interleave files, containers, commands and arms; the balanced two-pair log contrasts retain logical A/B when chronological order reverses. The registered synchronized block/file bootstrap uses 999 draws. The complete conditioning and timed receipt sequence is published below.

## Host and binaries

The attempt ran on 2026-11-03 on on-demand GCP `c4-standard-8`, with SMT disabled and four visible physical cores. Parent and child processes inherited affinity to CPU 0. The CPU reported `INTEL(R) XEON(R) PLATINUM 8581C CPU @ 2.30GHz`; the observed platform was Intel Emerald Rapids. The operating system reported Linux `6.12.113+deb13-cloud-amd64`, x86-64, glibc 2.41. Timing used `clock_gettime(CLOCK_MONOTONIC)`, reported resolution 1 ns, including process launch and blocking reap.

No turbo override was requested. Frequency governor, driver, bounds and per-call frequency readings were not exposed; the receipts retain their unavailable status. Mandatory child CPU, fault and context-switch counters and available steal ticks were captured outside the timer, without correcting elapsed times or adding rejection thresholds. These diagnostics do not establish a physical cause of variation.

Candidate, tooling and fetched-main baseline were all [0ba3215b5752aec98652b60d7aa591eb61adff51](https://github.com/coreav-dev/coreav/commit/0ba3215b5752aec98652b60d7aa591eb61adff51), with required CI verified before freezing. A default-feature GNU release build used Rust 1.99.1 with no feature or `RUSTFLAGS` overrides:

```sh
cargo build --locked --release -p coreav-cli --bin coreav -j 2
```

Candidate and baseline refer to the same actual build, SHA-256 `fb5acef700e129b729be6df38562e3d63de14dc284447f8666eaeca2dca0802b`, not two different implementations. The CoreAV control invokes that executable twice with identical arguments. The FFprobe control uses the shared FFmpeg 9.0.2 oracle, verified with `scripts/oracle_pin.py --verify-bin`; executable identities are retained in the receipts. Both programs use the registered environment, including strict-safe mode and an empty gconv path.

The private manifest bound all five registered metadata manifests, held-out disjointness, the historical product roster, exact binary and judging-file identities, commands and the complete schedule before payload access. Only its 24 development files were downloaded. The historical product selection and prior results remain unchanged; no current product measurements or other bars are reported here. Passing these controls does not establish real-host coverage guarantees, a product speed ratio or permission to run a product comparison.

## Evidence and replay

- [Complete call receipts](b4-round10-calibration.calls.json.gz), compressed without changing their JSON bytes. File identities use opaque aliases; no media hashes, source paths or instance identifiers are published.
- [Recomputed report](b4-round10-calibration.report.json), including case, position and early/late diagnostics, both interval levels and completeness checks.

The collected report and a local replay were exactly equal. Replay requires no media, private manifest or cloud credentials:

```sh
tmp=$(mktemp -d)
gzip -dc docs/measurements/compat-ffprobe/b4-round10-calibration.calls.json.gz > "$tmp/calls.json"
python3 -I tools/compat-corpus/b4_round10.py report --results "$tmp/calls.json" --out "$tmp/report.json"
diff -u docs/measurements/compat-ffprobe/b4-round10-calibration.report.json "$tmp/report.json"
rm -r "$tmp"
```
