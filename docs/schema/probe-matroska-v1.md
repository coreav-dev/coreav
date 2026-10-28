# `coreav probe` output for Matroska and WebM, schema `coreav.probe-matroska/1`

This is the reference for the output of `coreav probe` on a Matroska or WebM
file (`docs/design/mkv.md`). The machine-readable twin is
[`probe-matroska-v1.schema.json`](probe-matroska-v1.schema.json) (JSON Schema
2020-12); tests validate every output against it. ISO base media files
print [`coreav.probe/1`](probe-v1.md) instead; the command picks the schema
from the first four bytes of the file (`1A 45 DF A3`, the EBML header).

The rules of `coreav.probe/1` apply: every key listed here is always present,
unknown values are `null`, exact values come first, codec code points are
numbers from the specification that defines them, and adding a key keeps
`/1`. The shared shapes (rational, time) and the `profile` objects are those
of `coreav.probe/1`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.probe-matroska/1`. |
| `file.size` | integer | File size in bytes. |
| `container.doc_type` | string | `matroska` or `webm` (EBML header `DocType`). |
| `container.doc_type_version`, `container.doc_type_read_version` | integer | `DocTypeVersion` and `DocTypeReadVersion`. |
| `container.timestamp_scale` | integer | `TimestampScale`: nanoseconds per tick. |
| `container.tick` | rational or `null` | One tick in seconds. |
| `container.live` | boolean | The Segment has an unknown size, as a live recording writes it. |
| `container.cues`, `container.chapters`, `container.tags` | boolean | The segment has `Cues`, `Chapters`, `Tags`. |
| `container.title`, `container.muxing_app`, `container.writing_app` | string or `null` | `Title`, `MuxingApp`, `WritingApp`. |
| `container.date_utc` | string or `null` | `DateUTC`, RFC 3339 UTC. |
| `container.attachments` | array | Attached files: `name`, `media_type`, `size` (bytes of `FileData`). |
| `duration` | time or `null` | `Duration`, rounded to the nearest nanosecond (RFC 9559 11.3), in ticks when a tick divides a second, else in nanoseconds; `null` when absent (live recordings). |
| `streams` | array of stream | Tracks in `Tracks` order, without dropped ones. |
| `dropped_streams` | array | Tracks left out because their codec is not allowed: `stream_index`, `track_number`, `codec_id`, `codec`, `reason` (`codec_not_allowed`). |
| `warnings` | array of strings | File-level notes, for example a segment size past the end of the file, or a block scan that failed and why (its values are then `null`, and `frames_listed` is `false`). |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `index` | integer | Position in `streams`. |
| `stream_index` | integer | Position among all `TrackEntry` elements, dropped ones included (`ffprobe`'s `index`). |
| `track_number`, `track_uid` | integer; integer or `null` | `TrackNumber` (what blocks refer to) and `TrackUID`. |
| `kind` | string | From `TrackType`: `video`, `audio`, `subtitle`, `data`. |
| `codec` | string | CoreAV's codec name, which `--allow` takes; `other` for unrecognized codec IDs. |
| `codec_id`, `codec_private_size` | string; integer or `null` | `CodecID` and the length of `CodecPrivate`. |
| `fourcc` | string or `null` | The `biCompression` code of a `V_MS/VFW/FOURCC` track. |
| `name`, `language` | string or `null`; string | `Name`; `LanguageBCP47`, else `Language` (default `eng`). |
| `enabled`, `default`, `forced`, `lacing` | boolean | `FlagEnabled`, `FlagDefault`, `FlagForced`, `FlagLacing`. |
| `default_duration_ns`, `codec_delay_ns`, `seek_pre_roll_ns` | integer or `null`; integer; integer | `DefaultDuration`, `CodecDelay`, `SeekPreRoll` in nanoseconds. |
| `encoding` | string | `none`, `header_stripping`, `encrypted` (packets listed as stored), `zlib` (packets listed with their stored ranges and inflated sizes), `unsupported` (no packets). |
| `packets_available` | boolean | The track's packets can be listed. |
| `frames_listed` | boolean or `null` | With `--scan-frame-sizes`: every frame of the track was listed, with a known size. `false` when the track's frames cannot be listed (`packets_available` is `false`), for a zlib-compressed track with a frame that does not inflate within the limits (`docs/design/mkv.md`), and for every track when the scan failed: on a structure behind which a reader may find frames the listing would miss (bytes after the Segment's declared end, an EBML header or unknown element among the Segment's children, an element running past its parent or the file, a truncated file, a second `Info` or `Tracks`, a BlockGroup with two Blocks, a block of an undefined track, an element whose length its type does not allow, a string or binary longer than the scan allows; `docs/design/policy-matroska.md`, decision 8), or on a limit. `null` when not scanned. |
| `duration` | time or `null` | With `--scan-frame-sizes`: the span from the earliest frame's presentation time to the latest frame end (its time plus its `BlockDuration`, which covers the whole block, or `DefaultDuration`; a frame with neither ends where it starts), from a scan of every block (`docs/design/policy-matroska.md`). `0` for a track without frames; `null` when not scanned, when a frame's time is unknown (a later frame of a lace without `DefaultDuration` whose codec does not state its frames' durations; CoreAV reads Opus's), when the track's frames cannot be listed (`packets_available` is `false`), or when the scan failed. An Opus frame without `BlockDuration` or `DefaultDuration` lasts what its TOC byte states (RFC 6716 3.1); a frame of another codec without them ends where it starts. |
| `frame_count` | integer or `null` | With `--scan-frame-sizes`: the frames listed; `null` when not scanned or when the track's frames cannot be listed. |
| `bit_rate` | bit rate or `null` | With `--scan-frame-sizes`: the frames' bytes (stripped headers included) × 8 ÷ `duration`, as in `coreav.probe/1`; `null` when either is unknown or the duration is 0. |
| `profile` | object or `null` | Code points from `CodecPrivate`, or from the first keyframe when the track header lacks them (VP9; H.264 without `CodecPrivate`; AV1 whose `av1C` has no sequence header), with `type` and `name` as in `coreav.probe/1`. |
| `frame_profile` | integer or `null` | The VP9 `Profile` or VP8 `version` from the first frame header. |
| `video` | object or `null` | Video tracks: `width`, `height` (`PixelWidth`, `PixelHeight`), `parameter_set_size` and `scanned_size` (as in `coreav.probe/1`: the bound of the sizes the parameter sets in `CodecPrivate` define, for H.264, HEVC and AV1; and with `--scan-frame-sizes`, the bound over those and every parameter set, sequence header or frame header in the blocks, VP8 key frames included; `null` when not derivable, when a frame cannot be read, or when the frames are encrypted), `display_width`, `display_height`, `display_unit`, `crop` (`top`, `bottom`, `left`, `right`), `rotation` (clockwise 0, 90, 180 or 270, from `ProjectionPoseRoll`; `null` for other poses), `projection` (`type`, `yaw`, `pitch`, `roll` in degrees), `interlaced`, `stereo_mode`, `alpha_mode`, `bits_per_channel`, `color` (`primaries`, `transfer`, `matrix`: ITU-T H.273 code points; `full_range`: boolean). Colors come from `Colour`, then the codec's parameter set. |
| `audio` | object or `null` | Audio tracks: `channels`, `sample_rate` (integer Hz when `SamplingFrequency` is one), `sampling_frequency`, `output_sampling_frequency`, `bit_depth`. |
| `warnings` | array of strings | Recoverable problems in this track, and a declared size that differs from a verified one (`declared size WxH, parameter sets give WxH`, `... the scanned frames give WxH`). |

## `--ffprobe-compat`

The keys of `coreav.probe/1`'s compat output, with Matroska values:
`format_name` `matroska,webm`; `codec_tag_string` `[0][0][0][0]` and
`codec_tag` `0x0000`; `time_base` one tick (`1/1000` by default); no
`duration_ts` or `duration`, because Matroska stores no stream duration;
`sample_rate` the output rate (`OutputSamplingFrequency` when stored, as
for SBR, else `SamplingFrequency`);
`tags.language` the stored `Language` element, left out when the track
stores none or `und` (not `LanguageBCP47` or the default `eng`, which the
native `language` reports; where the segment's `Tags` hold a `LANGUAGE`
SimpleTag for the track, `ffprobe` lists that instead: KD-MKV-TAG-LANGUAGE);
`nb_streams` counts attachments, which
`ffprobe` lists as streams, so `index` keeps its numbering.

## Errors

`coreav.error/1`, as for `coreav.probe/1`. The `limit` names are the fields of
`coreav_mkv::Limits`: `max_elements`, `max_element_bytes`,
`max_retained_bytes`, `max_index_bytes`, `max_packet_bytes`,
`max_block_work`, `max_tracks`.
