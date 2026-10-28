# Upload policy, schema `coreav.policy/1`

This is the reference for the policy that `coreav check --policy FILE` and
the `coreav-policy` crate read. The machine-readable twin is
[`policy-v1.schema.json`](policy-v1.schema.json) (JSON Schema 2020-12);
tests validate the [examples](policy-examples/) against it. The verdict a
check prints is described in [`verdict-v1.md`](verdict-v1.md), and the
design in [`../design/policy-check.md`](../design/policy-check.md).

A policy is a JSON object of at most 1 MiB. Every rule reads one value of the
file's probe document, as `coreav probe --scan-fragments --scan-frame-sizes`
prints it: `coreav.probe/1` ([`probe-v1.md`](probe-v1.md)) for ISO base
media and QuickTime files, `coreav.probe-matroska/1`
([`probe-matroska-v1.md`](probe-matroska-v1.md)) for Matroska and WebM
files, which `coreav check` probes with every codec CoreAV names allowed
(`--allow` listing them all). The verdict names that value by its JSON
pointer. One policy serves both containers; where a rule reads a different
key in the Matroska document, the tables below say so, and the full mapping
is in [`../design/policy-matroska.md`](../design/policy-matroska.md).

A stream's kind, for the policy, is the family of its codec: `h264`,
`hevc`, `av1`, `vp8`, `vp9`, `prores`, `mpeg1video`, `mpeg2video`,
`mpeg4`, `theora`, `mjpeg` and `ffv1` are video; `aac`, `mp3`, `mp2`,
`opus`, `vorbis`, `flac`, `ac3`, `eac3`, `dts`, `truehd`, `alac` and `pcm`
are audio; `timed_text`, `subrip`, `ass`, `webvtt`, `pgs`, `vobsub` and
`dvbsub` are subtitle; `timecode` is timecode. The probe's `kind` (from the
track's handler, or a Matroska track's `TrackType`) counts only for a codec
without a family. So an H.264 track under a handler other than `vide` gets the video
rules; its `video` object is `null` in the probe output, so its size rules
reject it as `unknown`.

## Resolution rules check what a decoder produces

A sample entry declares a width and a height, but a decoder takes the
picture size from the codec's parameter sets and frame headers, which can
say more (a file can declare 320×240 and decode to 7680×4320). The
resolution rules therefore compare the larger of the declared size and a
verified one: by default the size from scanning every sample's parameter
sets and frame headers (`size_source`, below; the design is in
[`../design/frame-sizes.md`](../design/frame-sizes.md)). Scanning reads the
headers of every sample, not their coded data. A Matroska track header's
`PixelWidth` and `PixelHeight` are its declared size, and the scan reads
its blocks the same way.

## Rules for reading a policy

- Unknown keys, values of the wrong type and unknown names are errors: a
  misspelled rule never passes silently. `coreav check` exits with code 2 and
  names the offending value by its JSON pointer.
- A missing limit does not limit, and a missing allowlist (`formats`,
  `codecs`, `rotations`) allows every value. The exception is stream kinds:
  a kind without its own object under `streams` is not allowed.
- Limits are compared exactly. Counts, bytes, pixels, bits per second and
  hertz are non-negative integers (`1000`, or `1000.0` and `1e3`, which
  JSON Schema also calls integers; up to 2^53 when written with a fraction
  or an exponent). `max_seconds` is a non-negative JSON number, read as the
  shortest decimal that names its binary64 value: `10.01` is 1001/100 s,
  not 10.0099999999999997868371792719699442386627197265625 s. That is the
  decimal as written for up to 15 significant digits.
- A key given twice in one object is read as its last value, as the JSON
  parser keeps it; JSON (RFC 8259) leaves duplicate names undefined, so do
  not rely on them.
- A rule whose value the probe output leaves `null` rejects the file with
  the reason `unknown`: a check never accepts what it could not verify.
- Adding a key or a name keeps `/1`. Removing or renaming one, or changing a
  meaning or a default, makes `/2`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Required. Always `coreav.policy/1`. |
| `name` | string | Echoed in the verdict's `policy`, for logs. |
| `file` | object | Rules on the file, below. |
| `container` | object | Rules on the container, below. |
| `duration` | object | Rules on the duration, below. |
| `streams` | object | Required. Rules on streams, below. |

## `file`

| Key | Type | Probe value | Rule |
|---|---|---|---|
| `max_size` | integer | `file.size` | Bytes, at most this. |
| `max_bit_rate` | integer | `file.size` and `duration` | Overall bit rate in bits per second, `file.size` × 8 ÷ `duration` seconds (exact; the format `bit_rate` of `--ffprobe-compat` is this value truncated), at most this. Unknown when the duration is unknown or 0. For a Matroska file without `Duration` (`duration` is `null`, a live recording), the duration is the longest stream duration, when every stream has one. |

## `container`

| Key | Type | Probe value | Rule |
|---|---|---|---|
| `formats` | array of strings | `container.major_brand`; Matroska: `container.doc_type` | The container format must be listed: `mov` for the major brand `qt  ` or no `ftyp` box (`null`), `3gp` for a major brand starting with `3g` (3GPP and 3GPP2), `mp4` for every other major brand; `webm` and `matroska` for a Matroska file of that `DocType`. Every other file (AVI and others) is rejected as `unreadable` with the probe's `unsupported_container` error. |
| `fragmented` | boolean | `container.fragmented`; Matroska: `container.live` | `false`: fragmented files are rejected, and so are live Matroska recordings (a Segment of unknown size, written while recording). Default `true`. |

## `duration`

| Key | Type | Probe value | Rule |
|---|---|---|---|
| `max_seconds` | number | `duration` and every `streams[i].duration` | The longest of the file's duration and each stream's duration, in seconds (`ticks` ÷ `timescale`, exact), at most this. Streams count because a movie header (or a Matroska `Duration`) can understate how long its media lasts. One reason at most: at the longest value (the file's when equal), or at the first unknown one. A Matroska file's stream durations come from a scan of its blocks, which `coreav check` runs when a rule needs them; a Matroska file without `Duration` (`null`) is judged by its streams alone when every stream's duration is known. |

## `streams`

| Key | Type | Rule |
|---|---|---|
| `max_count` | integer | Streams in the file, `streams` plus `dropped_streams`, at most this. |
| `video`, `audio`, `subtitle`, `timecode`, `data` | object | Streams of that kind (the codec's family, above) are allowed, under the rules of the object (below; `{}` allows them with no further rule). A stream whose kind has no object is rejected. |
| `unidentified` | `"reject"`, `"ignore"`, or an array of strings of 1 to 64 printable ASCII characters | Streams whose codec CoreAV does not identify: the probe's `dropped_streams` (codec `other`). `"reject"` (the default) rejects every one. An array lists the sample entry types (the probe's `fourcc`) or Matroska `CodecID`s (the probe's `codec_id`) to accept, for example `["mebx"]` for Apple timed metadata; any other rejects. A CodecID names a container mapping, not always a codec: `V_MS/VFW/FOURCC` accepts every Video for Windows codec CoreAV does not name. A Matroska track whose frames are encrypted or encoded in a way the probe does not undo (`encoding` is `encrypted` or `unsupported`) carries something CoreAV cannot identify either: it is rejected unless `unidentified` is `"ignore"`, whatever the array lists. `"ignore"` accepts every one, whatever it carries, including video in a codec CoreAV does not read. Accepted unidentified streams count towards `max_count` but are otherwise unchecked: the probe reports neither their kind nor their duration. |
| `invalid_tables` | `"reject"` or `"ignore"` | Streams whose sample tables are inconsistent (`tables_valid` is `false`), or Matroska streams whose frames could not all be listed (`frames_listed` is `false`: an encoding the probe does not undo, or a block structure the scan cannot account for, a truncated file included). Default `reject`; `coreav check` then scans the blocks of every Matroska file. With `"ignore"`, a Matroska file whose structure the scan cannot account for is not rejected for it: for example, a stream that a second `Tracks` element defines is not listed by the probe, so the `codecs`, count and kind rules do not see it, and only rules that need the scanned values (which are then `unknown`) reject the file. |
| `external_data` | `"reject"` or `"ignore"` | Streams whose data is in another file (`external_data` is `true`). Default `reject`. Does not apply to Matroska, which has no external data references. |
| `multiple_descriptions` | `"reject"` or `"ignore"` | Streams with more than one sample description (`sample_descriptions` above 1). The probe describes only the first, so the codec and size rules cannot see the others, which later samples may use. Default `reject`; with `"ignore"`, those rules check the first description only. Does not apply to Matroska, which has one `CodecPrivate` per track. |

### Every kind

| Key | Type | Probe value | Rule |
|---|---|---|---|
| `codecs` | array of strings | `streams[i].codec` | The codec must be listed. Names: `h264`, `hevc`, `av1`, `vp8`, `vp9`, `prores`, `mpeg1video`, `mpeg2video`, `mpeg4`, `theora`, `mjpeg`, `ffv1`, `aac`, `mp3`, `mp2`, `opus`, `vorbis`, `flac`, `ac3`, `eac3`, `dts`, `truehd`, `alac`, `pcm`, `timed_text`, `subrip`, `ass`, `webvtt`, `pgs`, `vobsub`, `dvbsub`, `timecode` (the names `coreav probe` prints; most occur in Matroska files only). `other` cannot be listed; see `unidentified`. |
| `min_count` | integer | count of `streams` of the kind | At least this many streams of the kind. At most `max_count` when both are given. |
| `max_count` | integer | count of `streams` of the kind | At most this many streams of the kind (unidentified streams are not counted here; see `unidentified`). |
| `max_bit_rate` | integer | `streams[i].bit_rate` | The stream's bit rate in bits per second (sample bytes × 8 ÷ duration, exact; for Matroska, frame bytes × 8 ÷ the scanned span), at most this. |

### `video` only

The size rules compare the larger, in each dimension, of the size the
sample entry declares (`video.width`, `video.height`) and the verified size
`size_source` names. A verified size that the probe output leaves `null`
(not derivable, or a sample that could not be walked) gives `unknown`, so
the rule rejects.

| Key | Type | Probe value | Rule |
|---|---|---|---|
| `size_source` | `"scan"`, `"configuration"` or `"declared"` | | The verified size: `"scan"` (the default) is `video.scanned_size`, the bound over the codec configuration and every parameter set and frame header in the samples (or Matroska blocks, VP8 key frames included); `coreav check` scans the samples when a resolution rule needs it, which reads the headers of every sample. `"configuration"` is `video.parameter_set_size`, from the codec configuration alone (`CodecPrivate` for Matroska), without reading samples; it misses parameter sets carried in samples, and it is `null` for VP8, VP9 and ProRes (no parameter sets in the record) and for records without one (`avc3`, `hev1`), which then reject. `"declared"` uses the sample entry (`PixelWidth`, `PixelHeight`) alone, unverified: a file can understate its size. Codecs whose frames the scan does not read (MPEG-1, MPEG-2, MPEG-4 Part 2, Theora, MJPEG, FFV1) have no verified size and reject under `"scan"` and `"configuration"`. |
| `max_width` | integer | `video.width` and the verified width | Width in pixels (before rotation), at most this. |
| `max_height` | integer | `video.height` and the verified height | Height in pixels, at most this. |
| `max_pixels` | integer | width × height, declared and verified | The larger of the two areas, at most this; it does not depend on the orientation. |
| `rotations` | array of 0, 90, 180, 270 | `streams[i].video.rotation` | The clockwise rotation must be listed. A display matrix (or Matroska projection pose) that is not a quarter-turn rotation (`rotation` is `null`: mirrored, scaled or skewed) is rejected whenever `rotations` is given. |

### `audio` only

| Key | Type | Probe value | Rule |
|---|---|---|---|
| `max_channels` | integer | `streams[i].audio.channels` | At most this many channels. |
| `max_sample_rate` | integer | `streams[i].audio.sample_rate` | At most this many hertz. A Matroska `SamplingFrequency` that is not a whole number gives `null`, so `unknown`. |

## Example

[`policy-examples/strict-gate.json`](policy-examples/strict-gate.json), a
gate in front of an existing encoder:

```json
{
  "schema": "coreav.policy/1",
  "name": "strict-gate",
  "file": {"max_size": 536870912, "max_bit_rate": 20000000},
  "container": {"formats": ["mp4"], "fragmented": false},
  "duration": {"max_seconds": 600},
  "streams": {
    "max_count": 2,
    "video": {
      "codecs": ["h264"],
      "min_count": 1,
      "max_count": 1,
      "max_width": 1920,
      "max_height": 1920,
      "max_bit_rate": 16000000,
      "rotations": [0, 90, 180, 270]
    },
    "audio": {
      "codecs": ["aac"],
      "max_count": 1,
      "max_bit_rate": 320000,
      "max_channels": 2,
      "max_sample_rate": 48000
    }
  }
}
```

The other examples are [`web-upload.json`](policy-examples/web-upload.json)
(video uploads to a web service: MP4, MOV, WebM and Matroska, any common
codec up to 4096 pixels on a side, browser recordings in VP8 or VP9 with
Opus or Vorbis included, one video stream, and the timed metadata of phones and action
cameras, `mebx`, `gpmd` and `camm`, accepted; any other unidentified stream
rejects) and
[`transcription.json`](policy-examples/transcription.json) (at least one
audio stream in a codec a speech model's decoder takes; everything else,
unidentified streams included, tolerated with `"ignore"`).
