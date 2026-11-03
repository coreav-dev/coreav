# AV1 transcode summary, schema `coreav.transcode/2`

This reference describes completed explicit AV1 jobs. The library serializer is `coreav_cli::transcode::av1::document`. Public command selection is not connected yet. H.264 jobs retain the unchanged [`coreav.transcode/1` contract](transcode-v1.md).

The machine-readable contract is [`transcode-v2.schema.json`](transcode-v2.schema.json). The [example document](transcode-v2.example.json) uses illustrative settings and counters. It is not a native encoding result.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.transcode/2`. |
| `output.bytes` | integer | Bytes written to the completed output. |
| `video` | object | The selected source video and its AV1 encoding result. |
| `audio` | array | Audio streams copied, in source order, with the same fields as version 1. |
| `encoded_audio` | object or `null` | Independently encoded audio, with the same fields as version 1. |
| `other` | array | Copied streams other than the encoded video and audio, with the same fields as version 1. |
| `dropped` | array | Source streams omitted from the output, with the same fields as version 1. |
| `input_capture` | optional object | Present only for an explicitly requested AAC input capture in a build with `input-capture`. It retains the existing capture fields described below. |

## Video

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | The source stream's position among all source streams. |
| `codec` | string | The source codec, not the output codec. |
| `frames` | integer | Presented pictures submitted and reconciled with complete output samples. |
| `not_presented` | integer | Known reference-only source pictures the decoder returned but the job did not encode. |
| `backend` | string | The decoder backend that ran. |
| `width`, `height` | integer | Encoded dimensions, including an explicitly requested scale. |
| `sample_aspect` | string or `null` | Exact output sample aspect as `N:D`, or `null` for square samples. |
| `encoding` | object | The software AV1 encoder, effective settings and completed session counts. |

AV1 does not repair, drop or snap presented timestamps. Missing, duplicate, changed or extra presented pictures fail the job. Version 2 therefore has no `dropped_frames` field. Settings cadence is separate from the original sample clock.

## Video encoding

Every key below is present. Byte counts are integers, not strings.

| Key | Type | Meaning |
|---|---|---|
| `codec` | string | Always `av1`. |
| `backend` | string | Always `svt-av1`. |
| `version` | string | Native version from the validated worker stream. |
| `kind` | string | Always `software`. |
| `isolation` | string | Always `sandboxed`: the job supplied the existing sandbox launcher. This is not proof that every operating-system limit was enforced. |
| `preset` | integer | SVT numeric preset, 0 through 13. The AV1 default is 8. |
| `crf` | integer | Integer constant-rate factor, 0 through 63. The AV1 default is 32. |
| `cadence.num`, `cadence.den` | integer | Reduced exact presented-average frame rate supplied to the encoder settings. This is not a constant-frame-rate conversion or a replacement for source timestamps. |
| `bit_depth` | integer | Always 8. |
| `chroma` | string | Always `4:2:0`. |
| `color` | object | Applied colour metadata described below. |
| `keyint` | integer | Configured five-second closed-GOP interval in pictures, `max(1, floor(5 * cadence.num / cadence.den))`. Memory-limited sessions can force earlier keyframes. |
| `parallelism_level` | integer | Always 1. This SVT setting is not a thread count or a processor cap. |
| `thread_limit` | `null` | No thread-count limit is claimed. Explicit AV1 thread requests are unsupported. |
| `retained_budget` | integer | Assigned retained-input budget B, at most 1 GiB. It excludes SDK-internal allocations. |
| `fixed_retained_bytes` | integer | Fixed retained overhead F, including the separate EOF owner. |
| `retained_picture_bytes` | integer | Charged per-picture retained capacity P, including planes, descriptors, headers and owner slots. |
| `segment_frames` | integer | Fixed maximum pictures S in each nonoverlapping native session. A short job or final session can contain fewer pictures. |
| `segments` | integer | Sessions whose complete drain and successful destruction were reported in validated completion records. |
| `address_space` | integer | Requested worker address-space allowance in bytes. It is separate from retained-input accounting and is not proof of operating-system enforcement. |

The session cap is `min(keyint, floor((B - F) / P))`. A job starts only if one whole picture fits. Its length is not capped by one session. All reserved owner slots remain charged in a short session. The [backend design](../design/encoders/svt-av1.md) defines the exact retained-peak calculation and cleanup rules.

## Applied colour

`primaries`, `transfer` and `matrix` are assigned ITU-T H.273 codes. Unspecified primaries or matrix use code 2. Transfer must positively identify supported SDR, so it is neither absent nor code 2. PQ and HLG are unsupported.

`full_range` is a boolean for the applied source range. `chroma_location` is `left`, `topleft` or `null` when unspecified. Other source sitings are refused, not converted silently. The job refuses contradictory source and decoded metadata and changes after the first presented picture.

## Optional input capture

`input_capture.format` is `f32le`. `pcm` contains the caller-owned capture `path`, successfully written `bytes` and their `sha256`. `encoder_sha256` identifies the encoder program. `argv` and `launcher_argv` retain the supplied encoder and launcher arguments.

`sample_rate`, `channel_mask` and `start_ticks` describe the actual audio input clock and channel order. `sandboxed` says whether a launcher was supplied. `address_space` and `open_files` are requested limits, or `null` without a launcher. They do not prove operating-system enforcement. Capture metadata is not a complete transcode or independent-decoder verdict.

## Errors and evidence

Failures retain the existing `coreav.error/1` contract and temporary-output publication rules. A failed encode produces no successful version 2 summary. An expired prepared job returns timeout before source access.

A valid summary is not independent decoder evidence, a quality score, a speed result or proof of a release acceptance bar. Those gates require their separate native and independent-decoder checks.
