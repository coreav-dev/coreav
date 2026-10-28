# `coreav extract-audio` summary, schema `coreav.extract-audio/1`

This is the reference for the summary `coreav extract-audio` prints after
writing an M4A file: on standard output, or on standard
error when the M4A file itself goes to standard output. The machine-readable
twin is [`extract-audio-v1.schema.json`](extract-audio-v1.schema.json) (JSON
Schema 2020-12); tests validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shapes `rational` and `time` are those of `probe-v1.md`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.extract-audio/1`. |
| `output.bytes` | integer | Bytes written. |
| `stream` | stream | The extracted stream; the output's only track. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the source stream among all tracks of the source (`ffprobe`'s `index`). |
| `track_id` | integer | `track_ID` of the source track. |
| `codec` | string | The codec name of `coreav probe`; always `aac`. |
| `samples` | integer | Samples copied: every sample of the source track, in order. |
| `duration` | time | Sum of the copied decode durations, in media ticks of the source track's timescale (which the output keeps). |
