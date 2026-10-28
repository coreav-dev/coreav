# `coreav check` verdict, schema `coreav.verdict/1`

This is the reference for the verdict `coreav check --policy POLICY FILE`
prints on standard output, and that `coreav_policy::Verdict::to_json`
returns. The machine-readable twin is
[`verdict-v1.schema.json`](verdict-v1.schema.json) (JSON Schema 2020-12);
tests validate every verdict against it. The policy is described in
[`policy-v1.md`](policy-v1.md).

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key or a `code` keeps
`/1`, while removing or renaming a key, or changing a type or a meaning,
makes `/2`.

A verdict is a pure function of the policy and the file's probe output:
the same inputs give the same bytes. Exit codes of `coreav check`: 0 for
`accept`, 1 for `reject` (the verdict is printed either way), 2 for a usage
error or an invalid policy, 3 when the file cannot be opened or read (a
`coreav.error/1` object on stderr, no verdict) or the verdict cannot be
written to stdout, 141 when stdout was closed by its reader.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.verdict/1`. |
| `policy` | string or `null` | The policy's `name`. |
| `verdict` | string | `accept` when `reasons` is empty, else `reject`. |
| `reasons` | array of reason | Every rule the file fails, in the order below. |

## Reason

| Key | Type | Meaning |
|---|---|---|
| `code` | string | `not_allowed` (the value is not one the rule allows), `above_maximum`, `below_minimum`, `unknown` (the rule needs a value the probe output leaves `null`), or `unreadable` (the probe rejected the file). |
| `rule` | string or `null` | JSON pointer of the rule in the policy, for example `/streams/video/max_width`. For a stream kind that is not allowed, where its object would be (`/streams/subtitle`). `null` for `unreadable`, and for the one `unknown` reason of a document that is neither a `coreav.probe/1` nor a `coreav.probe-matroska/1` output (its `path` is `/schema`, its `limit` the two schema names). |
| `path` | string or `null` | JSON pointer of the value the rule read in the file's probe document (`coreav.probe/1`, or `coreav.probe-matroska/1` for Matroska and WebM), for example `/streams/0/video/width`, `/dropped_streams/0/fourcc` or `/dropped_streams/0/codec_id`. `null` when the rule combines several values (the overall bit rate, `max_pixels`, stream counts) and for `unreadable`. For `unknown`, it may point inside an object the probe prints as `null`, such as `/streams/2/video/width` for a video codec under a handler that is not `vide`. |
| `stream_index` | integer or `null` | For a reason about one stream: its `stream_index` (`ffprobe`'s `index`). |
| `value` | any | The value compared with the limit, as below; `null` for `unknown` and `unreadable`. |
| `exact` | rational or `null` | For durations and bit rates, the exact value compared, in lowest terms; `null` otherwise or when it does not fit in 64 bits. |
| `limit` | any | The limit: the policy's number, or the allowed values (for `unidentified`, the listed sample entry types and CodecIDs, `[]` when the policy rejects every one; for an encrypted or unsupported Matroska encoding, `["none", "header_stripping"]`). |
| `message` | string | A sentence for people. Its wording may change within `/1`; match on the other keys. |
| `error` | object or `null` | For `unreadable`: the probe's `coreav.error/1` object, the same one `coreav probe` prints on stderr for the file. |

`value` is the value at `path`, except:

| Rule | `value` |
|---|---|
| `/file/max_bit_rate` | `file.size` × 8 ÷ `duration` seconds, as a number. |
| `/container/formats` | The container format (`mp4`, `mov`, `3gp`) the major brand at `path` names; for Matroska, the `DocType` at `path` (`webm`, `matroska`). |
| a kind that is not allowed | The kind: the family of the codec at `path` (`/streams/i/codec`), or the `kind` at `path` for a codec without a family. |
| `/duration/max_seconds` | The `seconds` of the time at `path`. |
| `max_bit_rate` of a kind | The `bits_per_second` of the bit rate at `path`. |
| `max_width`, `max_height` | The larger of the declared value (`video.width`) and the verified one (`video.scanned_size.width` or `video.parameter_set_size.width`, as `size_source` says); `path` points at the larger, the declared one when they are equal. For `unknown`, `path` points at the value that is missing. |
| `max_pixels` | The larger of the declared and the verified width × height. |
| counts | The count. |

## Order

Reasons appear in this order, so that two verdicts can be compared byte for
byte:

1. `/file/max_size`, `/file/max_bit_rate`;
2. `/container/formats`, `/container/fragmented`;
3. `/duration/max_seconds`;
4. `/streams/max_count`, then `min_count` and `max_count` of each kind in
   the order `video`, `audio`, `subtitle`, `timecode`, `data`;
5. each stream in `streams` order: its kind, `codecs`, `max_bit_rate`,
   `max_width`, `max_height`, `max_pixels`, `rotations`, `max_channels`,
   `max_sample_rate`, `invalid_tables`, `external_data`,
   `multiple_descriptions`;
6. for Matroska, each stream whose `encoding` is `encrypted` or
   `unsupported`, in `streams` order (`unidentified`, at
   `/streams/i/encoding`);
7. each dropped stream, in `dropped_streams` order (`unidentified`).

A file the probe rejects has one `unreadable` reason and nothing else.
Object keys are printed in sorted order, as in the example below.

## Example

```json
{
  "policy": "strict-gate",
  "reasons": [
    {
      "code": "not_allowed",
      "error": null,
      "exact": null,
      "limit": ["mp4"],
      "message": "file: container format \"mov\" (major brand \"qt  \") is not allowed; allowed: mp4",
      "path": "/container/major_brand",
      "rule": "/container/formats",
      "stream_index": null,
      "value": "mov"
    },
    {
      "code": "not_allowed",
      "error": null,
      "exact": null,
      "limit": ["video", "audio"],
      "message": "stream 1 (timecode): timecode streams are not allowed; allowed: video, audio",
      "path": "/streams/1/codec",
      "rule": "/streams/timecode",
      "stream_index": 1,
      "value": "timecode"
    }
  ],
  "schema": "coreav.verdict/1",
  "verdict": "reject"
}
```
