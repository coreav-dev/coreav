# `coreav probe` output, schema `coreav.probe/1`

This is the reference for the output of `coreav probe`. The
machine-readable twin is [`probe-v1.schema.json`](probe-v1.schema.json)
(JSON Schema 2020-12); tests validate every output against it.

Rules:

- Every key listed here is always present. A value that is unknown or not
  representable is `null`.
- Exact values come first. A time is ticks of a timescale; a rational is
  `{num, den}`. Floating-point `seconds` and `bits_per_second` are
  convenience copies.
- Codec code points are numbers from the specification that defines them;
  names, where given, sit next to them.
- Adding a key keeps `/1`. Removing or renaming a key, or changing a type or a
  meaning, makes `/2`.

## Shared shapes

| Shape | Keys | Notes |
|---|---|---|
| rational | `num` (integer), `den` (integer, at least 1) | In lowest terms. `null` in place of a rational whose numerator is outside the 64-bit range, from −2^63 to 2^64 − 1. |
| time | `ticks` (rational or `null`), `timescale` (integer), `seconds` (number or `null`) | `seconds` = `ticks` / `timescale`. |

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.probe/1`. |
| `file.size` | integer | File size in bytes. |
| `container.major_brand` | string or `null` | `major_brand` of `ftyp`. |
| `container.fragmented` | boolean | The movie has `mvex` and at least one `moof`. |
| `container.timescale` | integer | Movie timescale (`mvhd`). |
| `container.creation_time` | string or `null` | `mvhd` creation time, RFC 3339 UTC; `null` when stored as 0. |
| `duration` | time or `null` | Presentation duration in movie ticks: the movie header duration without fragments, else `mehd`, else the longest known stream. |
| `presentation_duration` | time or `null` | Derived duration in movie ticks (an exact rational): the latest stream end minus the earliest stream start, over every stream whose timing is known, the streams the allowlist dropped included (`docs/design/probe-duration.md`). A stream starts at its leading empty edit rounded to its own ticks, or without an edit list at its first sample's composition time, and lasts its `duration` (a dropped stream: its media edit capped by its media header duration). `null` when no stream's timing is known. `duration` is the declared value; the two differ when the movie header is rounded to the movie timescale, stale, or counts an empty edit at the start. `ffprobe`'s format duration is this value. |
| `streams` | array of stream | Streams in file order, without dropped ones. |
| `dropped_streams` | array | Streams left out because their codec is not allowed: `stream_index`, `track_id`, `fourcc`, `reason` (`codec_not_allowed`). |
| `warnings` | array of strings | File-level notes, for example that fragments were not read. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `index` | integer | Position in `streams`. |
| `stream_index` | integer | Position among all tracks of the file, dropped ones included (`ffprobe`'s `index`). |
| `track_id` | integer | `track_ID` of `tkhd`. |
| `kind` | string | From the handler: `video`, `audio`, `subtitle`, `timecode`, `data`. |
| `codec` | string | `h264`, `hevc`, `av1`, `vp9`, `prores`, `aac`, `mp3`, `opus`, `flac`, `pcm`, `timed_text`, `timecode`, `other`. These are also the names `--allow` takes, except `other`, which can never be allowed. `flac` (sample entry `fLaC`) is not on the default allowlist. |
| `fourcc` | string | Sample entry type of the first sample description. |
| `handler`, `handler_name` | string; string or `null` | `hdlr` type and name: a counted string in a QuickTime file, else the bytes up to the first NUL (ISO/IEC 14496-12 8.4.3), whose first byte is text even when it equals the length of the rest. |
| `language` | string or `null` | ISO 639-2/T code from `mdhd`; `null` for QuickTime Macintosh codes and for packed values that are not three letters. |
| `timescale` | integer | Media timescale (`mdhd`). |
| `duration` | time or `null` | Stream duration in media ticks. |
| `duration_source` | string | `media_header`, `media_edit`, `observed` or `unknown`. |
| `sample_count` | integer or `null` | Samples in the movie box plus fragments; `null` when fragments were not read. |
| `sample_descriptions` | integer | Entries in `stsd`; only the first is described. |
| `bit_rate` | object or `null` | `exact` (rational, bits per second: sample bytes × 8 / `duration`) and `bits_per_second` (number). |
| `declared_bit_rate` | object or `null` | As stored in `btrt`, else `esds`: `source`, `average`, `maximum`, `buffer_size`. |
| `profile` | object or `null` | Code points of the codec configuration, with `type` (`h264`, `hevc`, `av1`, `vp9`, `aac`, `prores`) and `name` (the profile name `ffprobe` uses, or `null`). Keys per type are in the JSON Schema. |
| `edit_list` | object | `{"type": "none"}`, `{"type": "unsupported"}`, `{"type": "multiple"}` (several edits applied exactly, `docs/design/edit-lists.md`), or `{"type": "simple", "empty", "media_time", "segment"}`: leading empty edit and segment duration in movie ticks, first media time in media ticks. Every form also has `entries`: the `elst` entries as stored, each `segment_duration` (movie ticks), `media_time` (media ticks, -1 for an empty edit) and `media_rate` (16.16 fixed point as stored: 65536 is 1, 0 a dwell); empty without an edit list. |
| `creation_time` | string or `null` | `mdhd` creation time, RFC 3339 UTC. |
| `external_data` | boolean | The data is in another file; the stream has no packets. |
| `tables_valid` | boolean | The sample tables are consistent; packets are available. |
| `video` | object or `null` | Video streams only: `width`, `height` (as the sample entry declares them), `parameter_set_size` (`{width, height}`: the bound of the picture sizes the parameter sets of the codec configuration define, every SPS of `avcC` or `hvcC`, cropped, or the largest frame size of the `av1C` sequence header; `null` when the record holds none or one cannot be read), `scanned_size` (`{width, height}`: the bound over the configuration and every parameter set or frame header in the samples, with `--scan-frame-sizes`; `null` when not scanned or a sample cannot be read; `docs/design/frame-sizes.md`), `pixel_aspect` (`pasp`, stored `{num, den}` or `null`), `rotation` (clockwise degrees 0, 90, 180 or 270 when the matrix is a quarter-turn rotation, else `null`), `matrix` (the nine stored `tkhd` values), `color` (`primaries`, `transfer`, `matrix`: ITU-T H.273 code points as the sample entry's `colr` box stores them; `full_range`: boolean, from an `nclx` box, else the range flag of the codec configuration, `hvcC`, `av1C` or `vpcC`), `parameter_set_color` (the same keys as the parameter set of the codec configuration signals them, raw: the video usability information of the first SPS of the first `avcC` or `hvcC`, or the colour configuration of the first `av1C`'s sequence header; code points as stored, "unspecified" (2) included; a field it does not signal is `null`; `null` as a whole when the entry has no such record or its parameter set cannot be read). Which of the two a decoder reports is codec-specific (`docs/design/probe-color.md`); probe reports both as stored. |
| `audio` | object or `null` | Audio streams only: `channels`, `implicit_ps_possible`, `sample_rate` (Hz), `pcm` (`float`, `bits`, `little_endian`, `signed`, for PCM only). For AAC of a general audio object type (ISO/IEC 14496-3 table 1.1: main, LC, SSR, LTP, scalable, TwinVQ and their error-resilient forms, also under SBR or parametric stereo), `channels` comes from the `AudioSpecificConfig`: its `channelConfiguration` (ISO/IEC 14496-3 table 1.19), or, when that is 0, the channel elements of its program config element (4.4.1.1: one per single channel element, two per channel pair element, one per LFE). A mono configuration counts as 2 with explicit parametric stereo, or with an LC core under a sample entry of 2 channels, where parametric stereo may be signalled only in the bitstream, unless the backward compatible extension rules parametric stereo out explicitly: no SBR (`sbrPresentFlag` 0), or SBR without parametric stereo (`psPresentFlag` 0). This is because the sample entry's channel count is definitive only for codecs that do not record their own configuration (ISO/IEC 14496-12, 12.2.3). For other codecs, and when neither the `channelConfiguration` nor the program config element can be read (including explicit SBR signalling, object types 5 and 29, with a `channelConfiguration` of 0), it is the sample entry's. `implicit_ps_possible` (boolean) is true exactly when the entry's 2 is kept for a mono LC configuration because parametric stereo may be signalled only in the bitstream: a decoder reports 1 channel if it carries none. |
| `warnings` | array of strings | Recoverable problems in this stream. |

## Errors (`coreav.error/1`, on stderr)

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.error/1`. |
| `kind` | string | `invalid_structure`, `truncated_structure`, `limit`, `unsupported_container`, `unsupported_feature`, `codec_not_allowed`, and for `coreav thumbnail` also `decode` and `out_of_range` (`thumbnail-v1.md`), for `coreav transcode` also `decode` (`transcode-v1.md`) (exit code 1), or `io` (exit code 3). |
| `limit` | string or `null` | For `limit`: the `Limits` field that was exceeded, for example `max_depth`, or for `coreav chapters` and `coreav extract-subtitles` the `TextLimits` field: `max_sample_bytes`, `max_chapters` or `max_title_bytes`. |
| `offset` | integer or `null` | Byte offset of the offending structure, when known. |
| `message` | string | Human-readable description. |

## `--ffprobe-compat`

Prints `{"streams": [...], "format": {...}}` with `ffprobe`'s key names and
value formats. Only these keys are printed, and a key is left out when its
value is unknown:

- Stream: `index`, `codec_name`, `profile`, `codec_type`, `codec_tag_string`,
  `codec_tag`, `width`, `height`, `sample_aspect_ratio`,
  `display_aspect_ratio`, `color_range`, `color_space`, `color_transfer`,
  `color_primaries`, `level`, `sample_rate`, `channels`, `time_base`,
  `duration_ts`, `duration`, `tags.language`, `tags.handler_name`, and
  `side_data_list`: for a video entry with a Dolby Vision configuration
  record (`dvcC`, `dvvC` or `dvwC`), a `DOVI configuration record` entry
  (`dv_version_major`, `dv_version_minor`, `dv_profile`, `dv_level`,
  `rpu_present_flag`, `el_present_flag`, `bl_present_flag`,
  `dv_bl_signal_compatibility_id`), then, for rotated video, a
  `Display Matrix` entry (`side_data_type`, `rotation`).
- Format: `filename`, `nb_streams`, `format_name`, `duration`, `size`,
  `bit_rate`. The format `duration` is the native `presentation_duration`
  (the streams' span, as `ffprobe` defines it), else the native `duration`.

Every printed key is compared with `ffprobe` by `scripts/compat_check.py`. A difference counts as a known divergence only where
the stream oracle's overlay covered, on that file and stream, the field the
key follows (a stream's `duration_ts` for `duration_ts` and `duration`, the
format duration for the format `duration` and `bit_rate`), with the
evidence its entry requires (`docs/known-divergences.md`), and only when the
printed value is the value the entry was checked on: the same ticks or
microseconds, the stream duration those ticks give in `ffprobe`'s time base,
the bit rate `ffprobe`'s size gives over that duration, and a display aspect
ratio the checked size gives with the sample aspect ratio both report. Not printed: stream `bit_rate`, because `ffprobe`
divides by its own duration and counts part of the samples of fragmented
files, so CoreAV's exact value does not match it (`docs/measurements/0025-probe-metadata.md`);
the native `bit_rate` has it. Also not printed: frame rates, frame counts,
pixel formats and other fields `ffprobe` derives from decoding.

Dropped streams are left out of `streams` but counted in `nb_streams`, so
`index` keeps `ffprobe`'s numbering.

`sample_rate` for AAC is what its `AudioSpecificConfig` declares: the SBR
extension frequency when SBR is signalled explicitly (object types 5 and
29, or the backward compatible extension with `sbrPresentFlag` 1), else the
core frequency (ISO/IEC 14496-3 1.6.2.1, table 1.18); SBR signalled only in
the bitstream is not seen, as probe does not decode. The native
`sample_rate` is the sample entry's, as stored.

`channels` is what the AAC configuration declares: where the native
document keeps a stereo sample entry's 2 for a mono configuration
(`implicit_ps_possible`), this output prints 1, as a reader that does not
decode sees it.
