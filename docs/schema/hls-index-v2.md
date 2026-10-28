# `coreav hls index` summary, schema `coreav.hls-index/2`

This is the reference for the summary `coreav hls index` and `coreav hls
package` print after building an HLS index of a file
([`docs/design/hls.md`](../design/hls.md)): on standard output, or on
standard error when the index itself goes to standard output (`-`). The
machine-readable twin is [`hls-index-v2.schema.json`](hls-index-v2.schema.json)
(JSON Schema 2020-12); tests validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/2`, while
removing or renaming a key, or changing a type or a meaning, makes `/3`. The
shared shapes `rational` and `time` are those of `probe-v1.md`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.hls-index/2`. |
| `source.bytes` | integer | The length of the source the index was built from; `coreav hls get` refuses a source of another length. |
| `index.bytes` | integer | The length of the index. |
| `segment_duration` | rational | The target segment duration asked for, in seconds. |
| `renditions` | array of rendition | The video rendition, then the audio rendition, each present when packaged. |

## Rendition

Times are in the rendition's media ticks on its presentation timeline.

| Key | Type | Meaning |
|---|---|---|
| `rendition` | string | `video` or `audio`; the resources are named after it (`video.m3u8`, `video-init.mp4`, `video-N.m4s`). |
| `stream_index` | integer | The source stream, as `coreav probe` numbers them. |
| `track_id` | integer | The source track's ID. |
| `codecs` | string | The RFC 6381 codec string in the master playlist's `CODECS`. |
| `segments` | integer | The number of media segments, `N` from 0 to `segments` − 1. |
| `deferred` | boolean | Whether the segments are the source's own subsegments, taken from its segment index, and their samples read from the source per request ([`docs/design/hls.md`](../design/hls.md), "Sources with a segment index"). The target segment duration does not apply to such a rendition. |
| `samples` | integer or null | The samples copied into the segments; `null` for a deferred rendition, whose index does not count them. |
| `sample_bytes` | integer or null | Their bytes, as copied from the source; `null` for a deferred rendition. |
| `duration` | time | The presented duration: the sum of the segments' `EXTINF` before rounding. |
| `longest_segment` | time or null | The longest segment's duration before rounding; rounded to the nearest second (at least 1), the media playlist's `EXT-X-TARGETDURATION`. |
| `peak_bit_rate` | integer | Bits per second: this rendition's part of the master playlist's `BANDWIDTH`, the largest bit rate of any run of consecutive segments lasting between half and one and a half of the media playlist's target duration (RFC 8216 4.3.4.2). |
| `average_bit_rate` | integer | Bits per second over the whole rendition: its part of `AVERAGE-BANDWIDTH`. |
