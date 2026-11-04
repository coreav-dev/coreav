# H.264 PGO preparation

The registered profile-guided optimization evaluation did not start. Acquisition stopped on an oracle-capture failure on 2026-11-03 UTC. No CoreAV command opened the allocation, and there is no speed or parity verdict.

The [registered plan](../design/h264-pgo-release.md#registered-held-out-evaluation) requires sixteen independent files in every class. The fixed metadata listing completed, and the allocation contract was recorded before media acquisition. Seven acquisition decisions completed before the eighth file's probe failed the capture predicate.

| Class | Selected | Required |
|---|---:|---:|
| SD | 1 | 16 |
| 720p | 1 | 16 |
| 1080p | 3 | 16 |
| 4K | 0 | 16 |
| Total | 5 | 64 |

## Why preparation stopped

The pinned `ffprobe -count_frames` returned exit code 0 but emitted 25,351 bytes of stderr, including `Invalid NAL unit size` and `Error splitting the input into NAL units.` Its 396-byte stdout did not exceed the capture limit. The nonempty stderr caused the capture failure.

An independent readback of the complete downloaded file matched its frozen declared checksums. Download integrity did not make the oracle capture clean. The failed capture was retained, not converted into an unsupported-input exclusion. Acquisition stopped instead of skipping or substituting the failed file.

The partial roster fails the registered completeness check. Calibration, native output qualification, judged timing, and the non-H.264 CLI diagnostics did not run. This outcome does not establish a PGO improvement, regression, or oracle disagreement in CoreAV.

## Method and evidence

Acquisition used the pinned FFmpeg and ffprobe build `n9.0.2-17-g2a571b6068-20260930` on an untimed GCP `c4-standard-4` spot VM with SMT disabled. No timing host was provisioned.

The build and registered tools were pinned to public commit [`01a3daae9158a89bb04a98eda846d660e36408a7`](https://github.com/coreav-dev/coreav/commit/01a3daae9158a89bb04a98eda846d660e36408a7). The [sanitized preparation record](h264-pgo/preparation.json) retains the plan and allocation digests, class counts, failed-capture facts, and explicit absence of judged calls. It contains no media or source identities.

The registered predicates and thresholds remain unchanged. This record changes no build or release default.
