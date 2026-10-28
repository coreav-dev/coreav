# `coreav chapters` output, schema `coreav.chapters/1`

This is the reference for the chapter list `coreav chapters` prints on
standard output (`docs/design/text.md`). The machine-readable twin is
[`chapters-v1.schema.json`](chapters-v1.schema.json) (JSON Schema 2020-12);
tests validate every document against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shapes `rational` and `time` are those of `probe-v1.md`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.chapters/1`. |
| `stream` | stream or `null` | The QuickTime chapter track; `null` when the file has none (Nero `chpl` boxes are not read). |
| `chapters` | array of chapter | One per presented sample of the chapter track, in order; empty without a chapter track. |
| `samples` | integer | Samples read. |
| `dropped` | dropped | As in [`extract-subtitles-v1.md`](extract-subtitles-v1.md); `empty` and `too_short` stay 0, since empty titles and chapters of no duration are kept. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the chapter track among all tracks of the source (`ffprobe`'s `index`). |
| `track_id` | integer | `track_ID` of the chapter track: the first text track that an enabled track names in a `chap` track reference. |
| `codec` | string | `quicktime_text` or `timed_text`. |

## Chapter

| Key | Type | Meaning |
|---|---|---|
| `start` | time | Start, in media ticks of the chapter track on the presentation timeline: the sample's presentation time, cut to the track's edit. |
| `end` | time | End: the end of the sample, cut to the track's edit. |
| `title` | string | The sample's text, decoded (UTF-8, or UTF-16 after a byte order mark) up to its first NUL character; may be empty. |
