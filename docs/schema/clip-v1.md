# `coreav clip` summary, schema `coreav.clip/1`

This is the reference for the summary `coreav clip` prints after writing a
clip: on standard output, or on standard error when
the clip itself goes to standard output. The machine-readable twin is
[`clip-v1.schema.json`](clip-v1.schema.json) (JSON Schema 2020-12); tests
validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shapes `rational` and `time` are those of `probe-v1.md`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.clip/1`. |
| `output.bytes` | integer | Bytes written. |
| `start.requested` | rational | The `--start` value, in seconds (decimal, at most 12 digits before the point and 9 after it). |
| `start.snapped` | time | Where the clip starts, in the video's media ticks on the source's presentation timeline: the latest sync sample at or before the requested start that loses no frame, or the start of the source's edit where that sync sample is presented before it. The clip's time 0. |
| `end.requested` | rational or `null` | The `--end` value, in seconds; `null` for the end of the video. |
| `end.written` | time | Where the clip ends, in the video's media ticks: the end of the video edit as written, rounded up to a whole tick. |
| `streams` | array of stream | The kept video stream, then the kept audio stream if there is one. |
| `left_out` | array | Streams of the source that are not in the clip: `stream_index`, `track_id`, `reason` (`not_selected`, `no_samples_in_clip`, `codec_not_allowed`). |
| `boxes_left_out` | array of strings | Types of the child boxes of the video sample entry that the clip does not copy, at most 16. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `kind` | string | `video` or `audio`. |
| `stream_index` | integer | Position of the source stream among all tracks of the source (`ffprobe`'s `index`). |
| `track_id` | integer | `track_ID` of the source track. |
| `codec` | string or `null` | The codec name of `coreav probe`: `h264`, `hevc`, `av1`, `vp9` or `aac`. |
| `first_sample` | integer | Zero-based number of the first copied sample in the source track. |
| `samples` | integer | Samples copied; they follow `first_sample` in order. |
| `duration` | time | Sum of the copied decode durations, in media ticks. |
