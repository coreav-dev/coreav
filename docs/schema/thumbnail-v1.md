# `coreav thumbnail` summary, schema `coreav.thumbnail/1`

This is the reference for the summary `coreav thumbnail` prints after
writing an image (`docs/design/thumbnails.md`): on standard output, or on
standard error when the image itself goes to standard output. The
machine-readable twin is
[`thumbnail-v1.schema.json`](thumbnail-v1.schema.json) (JSON Schema
2020-12); tests validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shape `rational` is that of `probe-v1.md`.

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.thumbnail/1`. |
| `output.bytes` | integer | Bytes of the image written. |
| `output.format` | string | `jpeg`, `png` or `webp` (lossless). |
| `output.width`, `output.height` | integer | The image's size, after rotation and pixel aspect. |
| `time.requested` | rational | The `--at` value, in seconds from the file's start. |
| `time.frame` | rational | The presentation time of the frame shown, in seconds from the file's start, on CoreAV's timeline (it can differ from `ffprobe`'s by known divergence KD-EDIT-SNAP). |
| `time.frame_seconds` | number | `time.frame` as a floating-point number. |
| `time.decode_start` | rational | The presentation time of the keyframe decoding started from, on the same timeline: `time.frame` without `--exact`, unless no keyframe is presented at or before the time. |
| `stream_index` | integer | The video stream's position among all of the file's streams (`ffprobe`'s `index`). |
| `codec` | string | Its codec, by `ffprobe`'s name. |
| `keyframe` | boolean | The frame is the keyframe decoding started from (always so without `--exact`, unless no keyframe is presented at or before the time). |
| `packet.offset`, `packet.size` | integer | Byte offset and size of the frame's packet in the file. |
| `packets_decoded` | integer | Packets sent to the decoder: from the keyframe to the frame's packet, less the packets nothing depends on, which `--exact` skips (`docs/design/thumbnails.md`). |
| `decoder.backend` | string | `nvdec`, `native`, `dav1d`, `libvpx` or `ffmpeg` (the sandboxed FFmpeg fallback). |
| `decoder.name` | string | The decoder within the backend, for people. |
| `decoder.isolation` | string | `sandboxed` (a worker process) or `in_process`. |
| `decoder.fell_back` | boolean | The fallback ran: FFmpeg's C decoders, sandboxed. |
| `warnings` | array of strings | Recoverable problems, for example composition offsets of a version 0 `ctts` read as signed. |
| `skipped` | array | Backends tried before the one that ran, in the order of the decode policy: `backend` and `reason` (not available in this build, does not support the stream, failed to start). |

## Errors

A file that cannot be thumbnailed prints a `coreav.error/1` object on
standard error ([`probe-v1.md`](probe-v1.md), "Errors") with exit code 1,
or 3 for an I/O error. Besides the probe's kinds: `decode` (the decoder
failed on the data, or its worker failed), `out_of_range` (the time is at
or after the end of the video), `limit` with `limit` `timeout` (decoding
took longer than `--timeout`), `codec_not_allowed` (the codec is not on the
decode allowlist) and `unsupported_feature` (for example no decoder backend
in this build for the stream).
