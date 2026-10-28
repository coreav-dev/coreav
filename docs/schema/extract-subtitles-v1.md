# `coreav extract-subtitles` summary, schema `coreav.extract-subtitles/1`

This is the reference for the summary `coreav extract-subtitles` prints after
writing a WebVTT file (`docs/design/text.md`): on standard output, or on
standard error when the WebVTT file itself goes to standard output. The
machine-readable twin is
[`extract-subtitles-v1.schema.json`](extract-subtitles-v1.schema.json) (JSON
Schema 2020-12); tests validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.extract-subtitles/1`. |
| `output.bytes` | integer | Bytes written. |
| `stream` | stream | The source stream. |
| `samples` | integer | Samples read. |
| `cues` | integer | Cues written: one per presented sample with text. |
| `dropped` | dropped | What the output does not carry. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the source stream among all tracks of the source (`ffprobe`'s `index`). |
| `track_id` | integer | `track_ID` of the source track. |
| `codec` | string | `timed_text` (3GPP timed text, `tx3g`) or `quicktime_text` (QuickTime text, `text`); `ffprobe` calls both `mov_text`. |
| `language` | string or `null` | ISO 639-2/T language code of the track, as in `coreav probe`. |

## Dropped

Counts of samples, except `styling`.

| Key | Type | Meaning |
|---|---|---|
| `empty` | integer | Samples with no text, or only white space and line breaks: no cue. |
| `not_presented` | integer | Samples outside the track's edit. |
| `too_short` | integer | Samples presented for less than a millisecond once rounded: no cue. |
| `malformed` | integer | Samples shorter than their declared text length: skipped. |
| `invalid_text` | integer | Samples whose text is not valid UTF-8 or UTF-16: written with U+FFFD in place of the invalid sequences. |
| `trailing_bytes` | integer | Samples with bytes after the text that are not whole boxes: those bytes are ignored. |
| `styling` | object | Modifier boxes dropped, by type: `styl`, `hlit`, `hclr`, `krok`, `dlay`, `href`, `tbox`, `blnk`, `twrp`, `disp` (3GPP TS 26.245 5.17.1), and `other` for every other type. |
