# `coreav join` summary, schema `coreav.join/1`

This is the reference for the summary `coreav join` prints after writing a
joined file ([`docs/design/join.md`](../design/join.md)): on standard
output, or on standard error when the joined file goes to standard output.
The machine-readable twin is [`join-v1.schema.json`](join-v1.schema.json)
(JSON Schema 2020-12); tests validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shape `time` is that of `probe-v1.md`; `stream` and `left_out` are
those of [`clip-v1.md`](clip-v1.md). Times are in the first input's video
media ticks on the output's presentation timeline.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.join/1`. |
| `output.bytes` | integer | Bytes written. |
| `duration` | time | Where the last input's video ends. |
| `edits.video`, `edits.audio` | integer | Entries of the output's video and audio edit lists, empty edits included; 0 for a track without one. One media edit per track when the inputs meet exactly (the segments of a split), else up to one per input. |
| `inputs` | array of input | The inputs, in order. |
| `boxes_left_out` | array of strings | Types of the child boxes of the first input's video sample entry that the output does not copy, at most 16. |

## Input

| Key | Type | Meaning |
|---|---|---|
| `index` | integer | The input's position, from 0. |
| `path` | string | The input file. |
| `offset` | time | Where the input's time 0 lies in the output: a time `t` on the input's own presentation timeline is presented at `t + offset`. |
| `video_start`, `video_end` | time | Where the input's video is presented in the output. The next input's video starts at `video_end`; when it starts later on its own timeline, this input's last frame is held until then. |
| `extended` | integer | Ticks added to the decode duration of the input's last video sample, so that none of its frames is composed in the next input's media time. |
| `streams` | array of stream | The input's video stream, then its audio stream if the join has audio; `first_sample` is 0 and `samples` every sample of the stream. |
| `left_out` | array | Streams of the input that are not in the output: `stream_index`, `track_id`, `reason` (`not_selected`, `codec_not_allowed`). |

## Errors

A failed join prints a `coreav.error/1` object ([`probe-v1.md`](probe-v1.md))
with one more key, `input`: the position of the input the error is about,
or `null`. An input that cannot be joined to the first has `kind`
`incompatible_inputs`, and `message` names the input and the difference, for
example `input 2 cannot be joined to the first: its video timescale is
25600, the first input's is 12800`. Exit codes are those of `coreav clip`.
