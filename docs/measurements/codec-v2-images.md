# V2 coverage with exact image capture

CoreAV counts **31 of 488 commands, or 6.35%**, under the `png-bmp-yuv-v1` registration with the supplemental V2 inputs. The other 457 commands are misses. This is an observed compatibility share, not a release threshold, a speed result, or a verdict on R1 to R4, V1, or V3.

## Source and method

The [acquisition](https://github.com/coreav-dev/coreav/actions/runs/41774236056) completed on 2026-11-04 from merged source `e42fc012b13e782babc596e709c8f740840cba2d`. The workflow source, command, copy harness, and image harness all use that commit. The command and copy harness have `release-full`; the image harness also has `input-capture`. They use Cargo's release profile, Rust 1.99.1, and target `x86_64-unknown-linux-gnu`.

The reference is the pinned FFmpeg **9.0.2**, build `n9.0.2-17-g2a571b6068-20260930`, for both `ffmpeg` and `ffprobe`. The shared GitHub-hosted runner uses Ubuntu 24.04, an AMD EPYC 9V74 CPU with four logical CPUs, and 16,367,524 KiB RAM. The [host record](codec-v2-images/host.json) includes its kernel and runner-image version. No timing bar is measured on this host.

The fixed command rows and miss ordering come from the [codec coverage plan](compat-0.1.0/codec-coverage-plan.md). The [supplemental-input registration](../design/v2-fixtures.md) keeps the original five-fixture traversal, then tries only the registered per-command tuples. All 82 supplemental reference cases succeeded. The manifest binds all 27 generated fixture files, including the HLS dependency, before and after acquisition. These are generated inputs, not held-out media.

The [exact image registration](../design/png-bmp-yuv-v1.md) requires both lossless encoding of the actual pre-encode RGB and exact RGB equality with FFmpeg's output for the same command. Neither comparison has a tolerance. The [timestamp binding correction](../design/image-timestamp-binding.md) validates raw PTS and edited presentation time independently.

## Counted commands and misses

The result contains all 488 registered media-command rows exactly once. There are no tooling faults, missing records, or executed fallbacks. The registered reporter rounds the share to four decimal places, giving `0.0635`.

| Output container | Commands | Counted |
|---|---:|---:|
| ADTS | 3 | 0 |
| ASF | 5 | 0 |
| AVI | 25 | 0 |
| FLAC | 5 | 0 |
| FLV | 7 | 0 |
| GIF | 1 | 0 |
| HLS | 2 | 0 |
| MOV | 14 | 0 |
| MP3 | 60 | 0 |
| MP4 | 217 | 28 |
| MPEG-PS | 5 | 0 |
| MPEG-TS | 14 | 0 |
| Matroska/WebM | 59 | 0 |
| Ogg | 4 | 0 |
| WAV | 5 | 0 |
| YUV4MPEG | 4 | 1 |
| image | 40 | 0 |
| raw | 18 | 2 |

Every uncounted command has its first applicable miss code.

| Miss code | Commands |
|---|---:|
| `no_input` | 89 |
| `refused` | 325 |
| `rejected` | 0 |
| `timeout` | 0 |
| `failed` | 3 |
| `ffmpeg_job` | 21 |
| `no_oracle` | 10 |
| `oracle` | 9 |
| `streams_differ` | 0 |

The nine oracle misses comprise five exact image-reference pixel disagreements, three `copy_check` misses, and one `B3` miss. The complete per-command diagnostics remain in [records.jsonl](codec-v2-images/records.jsonl).

## Refused options and encoder groups

All 325 refused commands are counted once under their logged reason in [`v2.refused_options`](codec-v2-images/summary.json). The ten most frequent reasons are:

| Logged reason | Commands |
|---|---:|
| `an unsupported output container` | 23 |
| `-framerate` | 17 |
| `an unsupported input container or presentation` | 16 |
| `seeking or trimming while encoding` | 15 |
| `-strict` | 11 |
| `-shortest` | 10 |
| `a selected stream that no -c copy option covers (it would be encoded)` | 10 |
| `-bsf:v` | 9 |
| `-async` | 7 |
| `-q:v` | 7 |

Encoder groups follow FFmpeg's stream mapping, not the output filename. A command can appear in several groups. Commands without a successful reference assignment have no encoder group.

| Encoder group | Commands | Counted | Misses |
|---|---:|---:|---|
| x264 | 100 | 3 | `refused` 86, `ffmpeg_job` 10, `oracle` 1 |
| libopus | 6 | 0 | `refused` 6 |
| aac | 87 | 0 | `refused` 77, `ffmpeg_job` 10 |
| other encoders | 145 | 3 | `refused` 122, `failed` 2, `ffmpeg_job` 11, `no_oracle` 2, `oracle` 5 |
| copy only | 139 | 25 | `refused` 102, `failed` 1, `no_oracle` 8, `oracle` 3 |

## Image-reference differences

Five image commands reached capture comparison, covering 106 frames. Every decoded CoreAV image exactly equals its captured pre-encode RGB, with zero unequal samples. All five commands still miss because that RGB differs from FFmpeg's same-command output. Their only image-comparison failure is `reference_pixels`; no metadata disagreement was recorded for these five commands.

The reference comparison includes upstream decoding and frame selection as well as conversion. These differences do not isolate a converter defect.

Indices are zero-based rows in the fixed command set. Maximum errors are absolute 8-bit channel differences in R, G, B order. They are diagnostics, not permitted errors.

| Command index | Frames | Maximum R, G, B error | Unequal samples / samples | Unequal pixels / pixels |
|---|---:|---|---:|---:|
| 38 | 4 | 2, 3, 2 | 479,441 / 921,600 | 299,322 / 307,200 |
| 87 | 1 | 2, 3, 2 | 119,783 / 230,400 | 75,059 / 76,800 |
| 267 | 50 | 2, 3, 2 | 5,975,199 / 11,520,000 | 3,732,678 / 3,840,000 |
| 421 | 1 | 2, 3, 2 | 119,783 / 230,400 | 75,059 / 76,800 |
| 586 | 50 | 2, 3, 2 | 5,975,199 / 11,520,000 | 3,732,678 / 3,840,000 |

The two full PNG sequences have the same retained sample totals. The still-image commands select the same source frame. These are separate commands and retain separate receipts and outcomes. No image command counts in this acquisition.

## Evidence and replay

The JSON evidence is byte-identical to the validated acquisition artifact. The verbose reference checks are losslessly gzip-compressed, with their original JSON bytes unchanged. The files contain no home paths or machine names.

- [Records](codec-v2-images/records.jsonl) retain each command, assignment, outcome, and image difference, including every captured frame.
- [Summary](codec-v2-images/summary.json) retains the nonvoid accounting and full reason tables.
- [Manifest](codec-v2-images/manifest.json) binds the source, command set, methods, inputs, build, and oracle identities.
- [Build record](codec-v2-images/build.json) identifies the three binaries, features, compiler, and lockfile.
- [Reference fixture checks](codec-v2-images/fixture-reference.json.gz) retain all registered supplemental attempts.
- [Evidence digests](codec-v2-images/evidence-digests.json) identify these files and the complete retained archives.

Run the replay from the repository root of a checkout containing these evidence files and the measured judging-tool versions. The measured source alone predates the evidence files. The replay reproduces the retained summary exactly:

```sh
(
replay_dir="$(mktemp -d "${TMPDIR:-/tmp}/coreav-v2-replay.XXXXXX")" || exit 1
trap 'rm -rf "$replay_dir"' 0
python3 -I tools/compat-corpus/png_bmp_yuv.py summarize \
  --registration png-bmp-yuv-v1 \
  --records docs/measurements/codec-v2-images/records.jsonl \
  --manifest docs/measurements/codec-v2-images/manifest.json \
  --summary "$replay_dir/summary.json" &&
cmp "$replay_dir/summary.json" docs/measurements/codec-v2-images/summary.json
)
```

Validation also checked the downloaded binaries against the build hashes and revalidated all five retained receipts against both source probes. The ordinary command's images equal the capture harness's images byte for byte. Recomputing the integer pixel statistics from the retained RGB files reproduces all 106 frame diagnostics.

The [complete capture artifact](https://github.com/coreav-dev/coreav/actions/runs/41774236056/artifacts/13297280195) retains requests, receipts, raw RGB, both source probes, outputs, process records, and fixture checks until 2027-02-02. The [binary artifact](https://github.com/coreav-dev/coreav/actions/runs/41774236056/artifacts/13295739543) is retained until 2026-12-04. Their downloaded ZIP digests match GitHub's artifact digests. Generated media and binary files are not committed.

The [earlier acquisition](https://github.com/coreav-dev/coreav/actions/runs/41665807091) remains void after its source-clock validation fault. Its interim totals are not a baseline and contribute nothing to this result. Registrations, inputs, predicates, thresholds, and earlier evidence are unchanged.
