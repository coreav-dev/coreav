# `coreav remux` and `coreav sanitize` summary, schema `coreav.remux/1`

This is the reference for the summary `coreav remux` and `coreav sanitize`
print after rewriting a file as MP4 ([`design/remux.md`](../design/remux.md);
for Matroska and WebM sources [`design/mkv-remux.md`](../design/mkv-remux.md);
[`design/sanitize.md`](../design/sanitize.md)): on standard output, or on
standard error when the MP4 file itself goes to standard output. The machine-readable twin is
[`remux-v1.schema.json`](remux-v1.schema.json) (JSON Schema 2020-12); tests
validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`. The
shared shape `time` is that of `probe-v1.md`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.remux/1`. |
| `output.bytes` | integer | Bytes written. |
| `metadata.mode` | string | `copy` (`coreav remux`: metadata as the remux keeps it, copied as stored for MP4 sources) or `sanitize` (`coreav sanitize`: only allowlisted boxes written). |
| `metadata.keep` | array of strings or `null` | For `sanitize`, what is kept (`--keep`): `rotation`, `language`, `dates`, `gapless`, `kinds`; `null` for `copy`. |
| `source.moov_first` | boolean | Whether the source's movie box already came before its media data; always `false` for Matroska and WebM sources. |
| `source.truncated_at` | integer or `null` | Matroska and WebM sources: the file ends inside an element at this offset (a recording cut off), and every complete frame before it was copied; `null` otherwise, and always for MP4 sources. |
| `streams` | array of stream | The kept streams, in source order; output track `i` is `streams[i]`. |
| `dropped_streams` | array of dropped stream | Streams of the source that are not in the output, in source order. |
| `dropped_boxes` | array of dropped box | Boxes of the source that are not in the output, in file order, at most 256; boxes of dropped streams are not listed. |
| `dropped_boxes_not_listed` | integer | Dropped boxes beyond those listed. |
| `cleared` | array of cleared field | Header fields of the source that sanitization cleared; empty for `copy`. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `kind` | string | `video` or `audio`. |
| `stream_index` | integer | Position of the stream among all tracks of the source (`ffprobe`'s `index`). |
| `source_track_id` | integer | `track_ID` of the source track; `TrackNumber` for Matroska and WebM sources. |
| `track_id` | integer | `track_ID` in the output: the kept streams are numbered from 1. |
| `codec` | string | The codec name of `coreav probe`: `h264`, `hevc`, `av1`, `vp9`, `aac`, `mp3`, `opus`, `flac`, `alac`, `ac3` or `eac3`. |
| `samples` | integer | Samples copied: every sample of the source stream. |
| `duration` | time | Sum of the decode durations, in media ticks. |
| `annex_b` | boolean | The samples were rewritten from H.264 Annex B start codes to 4-byte NAL unit lengths (H.264 in Matroska without `CodecPrivate`); always `false` for MP4 sources. |

## Dropped stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the stream among all tracks of the source. |
| `track_id` | integer | `track_ID` of the source track; `TrackNumber` for Matroska and WebM sources. |
| `reason` | string | `not_selected` (not among `--streams`), `codec_not_allowed` (outside `--allow`) or `unsupported` (MP4 cannot carry it, or the remux cannot copy it). |
| `message` | string or `null` | For `unsupported`, why, in English (for example `PCM audio is not copied to MP4`); else `null`. |

## Dropped box

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer or `null` | The kept stream the box belongs to; `null` outside the tracks. |
| `path` | string | Box types from the top level down to the box, joined by `/`, for example `moov/trak/mdia/minf/stbl/saiz`. Types that are not printable ASCII are written as `\xNN`. For Matroska and WebM sources the dropped elements are `BlockAdditions` (for example a VP8 or VP9 alpha plane) of kept streams, with the path `Segment/Cluster/BlockGroup/BlockAdditions`. |
| `offset` | integer | Offset of the box in the source. |
| `size` | integer | Size of the box, header included. |
| `reason` | string | `not_carried` (the remux rebuilds or leaves out what the box describes), `too_large` (a metadata box beyond the retained-bytes limit) or `sanitized` (removed by sanitization: a metadata box or item, or a sample entry child outside the allowlist). |
| `kind` | string or `null` | For a sanitized metadata item, what it describes, from its type or key: `location`, `device` (the recording device or software), `date` or `other`; `null` otherwise. A hint, not a guarantee. |
| `name` | string or `null` | For a QuickTime metadata key, an item that refers to one, or a free-form item (`----`), the name, when it reads as an identifier (letters, digits, `.`, `_`, `-`, at most 64 bytes); `null` otherwise. |

## Cleared field

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer or `null` | The kept stream; `null` for the movie header. |
| `field` | string | `creation_time` (creation and modification times, written as 0), `rotation` (a transformation matrix other than the identity, written as the identity), `language` (written as `und`) `compressor_name` (a visual sample entry's compressor name, written empty), `sample_entry_extras` (parts of allowlisted sample entry boxes beyond their defined fields, left out: NAL units other than parameter sets in an H.264 or HEVC configuration, such as encoder SEI; bytes past a fixed-layout box; unknown children of a spherical video box) or `spherical_source` (a spherical video header's metadata source, written empty). |
