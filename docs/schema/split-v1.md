# `coreav split` summary, schema `coreav.split/1`

This is the reference for the summary `coreav split` prints on standard
output after writing the segments of a file
([`docs/design/split.md`](../design/split.md)). The machine-readable twin is
[`split-v1.schema.json`](split-v1.schema.json) (JSON Schema 2020-12); tests
validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shape `time` is that of `probe-v1.md`; `stream` and `left_out` are
those of [`clip-v1.md`](clip-v1.md).

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.split/1`. |
| `segments` | array of segment | The segments in order. Together they hold every sample of the kept streams exactly once. |
| `left_out` | array | Streams of the source that are not in the segments: `stream_index`, `track_id`, `reason` (`not_selected`; `no_samples_in_clip` when the source presents none of the chosen audio stream's samples; `codec_not_allowed`). |
| `boxes_left_out` | array of strings | Types of the child boxes of the video sample entry that the segments do not copy, at most 16. |

## Segment

Times are in the video's media ticks on the source's presentation timeline.

| Key | Type | Meaning |
|---|---|---|
| `index` | integer | The segment's number, from 0; the number in its output name. |
| `output.path` | string | The file written, the output pattern with `index` in it. |
| `output.bytes` | integer | Bytes written. |
| `start` | time | The segment's time 0 on the source's presentation timeline: where its video starts, or earlier where the source presents nothing there (the first segment's audio when that starts earlier; a gap between two windows of a source with several edits, which the segment after the gap begins with). Equal to the previous segment's `end`. |
| `video_start` | time | Where the segment's video starts: the cut, or the start of the source's video for the first segment. At or after `start`. |
| `end` | time | Where the segment's video ends: the next segment's `start`, or for the last segment the end of the source's video, rounded up to a whole tick (exactly, where the source has several edits). |
| `streams` | array of stream | The segment's video stream, then its audio stream if it holds audio samples. `first_sample` and `samples` say which samples of the source stream it holds. |
