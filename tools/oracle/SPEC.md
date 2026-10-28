# coreav-oracle: normalized schema, field mappings and thresholds

This file is the contract between the reference adapter (`ffprobe`/`ffmpeg` run as black boxes), any candidate adapter (later: CoreAV) and the comparator. It implements the black-box oracles. Where their requirements are silent, the choice made here is listed under "Open questions" with its reasoning, and this file is edited in place when a choice changes.

The Rust code never spawns FFmpeg. Shell scripts run the black boxes and save their raw output; `coreav-oracle normalize` turns that output into the JSON below; `coreav-oracle compare` reads only JSON files.

## Files

For each media file `NAME` and level `L` in `streams`, `packets`, `frames`, a directory holds `NAME.L.json`. A reference directory and a candidate directory use the same schema. A missing candidate file counts as a probe (or decode) failure of the candidate.

## Adapter interface

An adapter is any executable called as `ADAPTER LEVEL MEDIA_FILE`. It writes one normalized JSON document for that level to stdout. If it exits non-zero, times out or prints nothing, the capture script records `{"schema": "coreav-oracle/L/1", "file": NAME, "probe_ok": false}` (`decode_ok` for frames). `scripts/oracle-ffprobe.sh` is the reference adapter.

## Common rules

- Integers are JSON integers, never strings. Unknown or absent values are `null`; every documented key is always present.
- `time_base` is `[num, den]` (both positive integers). Timestamps (`pts`, `dts`, `duration`) are integers in that time base. Two timestamps agree when they denote the same rational time: `a * num_a * den_b == b * num_b * den_a`, computed in `i128`. `null` agrees only with `null`.
- All other fields agree on exact JSON equality.
- Streams are paired by position in the `streams` array (which is in container order, `index` ascending).

## Level `streams` (schema `coreav-oracle/streams/1`)

```json
{
  "schema": "coreav-oracle/streams/1",
  "file": "basic.mp4",
  "probe_ok": true,
  "format_duration_us": 2000000,
  "streams": [
    {
      "index": 0, "kind": "video", "codec_fourcc": "avc1", "codec_name": "h264",
      "width": 320, "height": 240, "rotation": 0,
      "time_base": [1, 12800], "timescale": 12800, "duration_ts": 25600,
      "color_primaries": null, "color_transfer": null, "color_matrix": null, "color_range": null,
      "sample_rate": null, "channels": null, "language": "und",
      "sample_aspect_ratio": [1, 1], "display_aspect_ratio": [4, 3], "handler_name": "VideoHandler"
    }
  ]
}
```

Reference capture: `ffprobe -v error -of json -show_format -show_streams FILE`.

| Normalized field | Gating | From `ffprobe` |
|---|---|---|
| `probe_ok` | yes | exit status 0 and stdout parses as a JSON object |
| stream count (`streams` length) | yes | length of `streams` |
| `format_duration_us` | yes | `format.duration` (decimal seconds string) parsed exactly as decimal into integer microseconds (more than 6 fractional digits: round half away from zero); missing or `N/A` gives `null` |
| `index` | no (pairing only) | `index` |
| `kind` | yes | `codec_type`, or `null` |
| `codec_fourcc` | yes | `codec_tag_string`; `null` when missing or when `codec_tag` is `0x0000` |
| `codec_name` | yes | `codec_name`, or `null` |
| `width`, `height` | yes | `width`, `height`, or `null` |
| `rotation` | yes | `rotation` of the `side_data_list` entry whose `side_data_type` is `Display Matrix`: counter-clockwise degrees normalized to `[0, 360)`. Video stream without that entry: `0`. Non-integer value: `null`. Non-video stream: `null` |
| `time_base` | no (used to compare timestamps) | `time_base` `"N/D"` parsed to `[N, D]`; unparsable or zero gives `null` |
| `timescale` | yes | `D` when `time_base` is `1/D`, else `null` |
| `duration_ts` | yes ("stream duration") | `duration_ts`, or `null` |
| `color_primaries` | yes | `color_primaries` |
| `color_transfer` | yes | `color_transfer` |
| `color_matrix` | yes | `color_space` |
| `color_range` | yes | `color_range` |
| `sample_rate` | informational | `sample_rate` (string) parsed to integer |
| `channels` | informational | `channels` |
| `language` | informational | `tags.language` |
| `sample_aspect_ratio` | informational | `sample_aspect_ratio` `"N:D"` to `[N, D]` |
| `display_aspect_ratio` | informational | `display_aspect_ratio` `"N:D"` to `[N, D]` |
| `handler_name` | informational | `tags.handler_name` |

Color values use `ffprobe`'s names; the value `unknown` and absent fields become `null`.

A file agrees when `probe_ok`, stream count, `format_duration_us` and every gating field of every paired stream agree. Informational differences are reported but never fail a file.

Agreement rate: agreeing files / (files - reference failures). A reference failure is a file whose reference `probe_ok` is `false`; it is excluded from the denominator and reported.

## Level `packets` (schema `coreav-oracle/packets/1`)

```json
{
  "schema": "coreav-oracle/packets/1",
  "file": "basic.mp4",
  "probe_ok": true,
  "streams": [
    {
      "index": 0, "time_base": [1, 12800],
      "packets": [
        {"pos": 48, "pts": 0, "dts": -1024, "size": 5131, "keyframe": true, "discard": false, "duration": 512}
      ]
    }
  ]
}
```

Reference capture: `ffprobe -v error -of json -show_streams -show_packets FILE`. Every stream from `streams` appears, in order, even when it has no packets. Packets keep `ffprobe`'s order within each stream.

| Normalized field | Gating | From `ffprobe` |
|---|---|---|
| `probe_ok` | yes (file) | as for streams |
| stream count | yes (file) | length of `streams` |
| `pos` | pairing key | `pos` (string) to integer; missing or `N/A` gives `null` |
| `pts`, `dts` | yes | `pts`, `dts` (integers in the stream `time_base`), or `null` |
| `size` | yes | `size` (string) to integer |
| `keyframe` | yes | `flags` contains `K` |
| `discard` | informational | `flags` contains `D` |
| `duration` | informational | `duration`, or `null` |

Pairing: streams by `index` when every stream on both sides has one (else by position), so a stream one side leaves out shifts no other; equal stream counts with different `index` sets are a file-level difference (`streams/indexes`). Packets within a paired stream by `pos`. When several packets share a `pos` value (including `null`), the k-th such packet of the reference pairs with the k-th of the candidate. A reference packet with no partner is a missing packet; a candidate packet with no partner is an extra packet. Both are disagreeing packets. All packets of an unpaired stream are missing (or extra).

A paired packet agrees when all gating fields agree. Agreement rate: agreeing packets / (agreeing + disagreeing packets). Files whose reference `probe_ok` is `false` are excluded. A candidate probe failure or a stream count difference is reported as a file-level failure; its unpaired packets are counted as disagreeing.

### Runs

A candidate may write consecutive packets of a stream as one entry with a `run` count: `run` packets, the first with the entry's fields, each next one `size` bytes further (`pos`) and `duration` later (`pts`, `dts`, `media_dts`, `media_cts`), with the same `size`, `duration`, `keyframe` and `discard`. The isobmff adapter does so for constant-size PCM, where one packet per sample would make the capture grow with the sample count (hundreds of megabytes of JSON for minutes of audio). A run stands for its packets everywhere: they are counted one by one, and a run is never expanded in memory. Each reference packet pairs with the run packet at its `pos`, found by arithmetic; the run packets no reference packet pairs with are extra packets, reported once per run (`packets/extra_run`, with `unpaired`, their count). The known-divergence overlay walks runs the same way (KD-PCM).

### Keyframes

`ffprobe`'s `keyframe` (flag `K`) for an H.264 stream in MP4 or MOV follows the coded data, not the sync sample table (`docs/design/random-access.md`). In the corpus it is set on exactly these frames: an IDR picture; a frame with a recovery point SEI message; and a frame whose slices are all I or SI slices when the sequence parameter set allows at most one reference frame (`max_num_ref_frames`). This holds whether `stss` lists the frame or not, and whether `stss` is present or not. The isobmff adapter writes that flag for an H.264 stream, from CoreAV's bitstream view (`FrameAccess`, `Movie::max_num_ref_frames`); for an AV1 stream `ffprobe`'s flag also follows the coded data: a key frame shown at once (`show_existing_frame` 0, `frame_type` KEY_FRAME, `show_frame` 1), read with the latest sequence header (the `av1C` configuration's or an earlier sample's), whether or not the sample holds one; the adapter writes that flag from `access::read_av1_frame_start`. It writes the sync sample flag (`Packet::keyframe`, `stss` as stored) for a frame whose NAL units or OBUs cannot be walked or whose frame header cannot be read, and for every other codec.

## Level `frames` (schema `coreav-oracle/frames/1`)

```json
{
  "schema": "coreav-oracle/frames/1",
  "file": "basic.mp4",
  "decode_ok": true,
  "streams": [
    {
      "index": 0, "time_base": [1, 25],
      "frames": [
        {"pts": 0, "dts": 0, "duration": 1, "size": 115200, "md5": "2dcedb2135ee0dff94071907f9bb52a1"}
      ]
    }
  ]
}
```

Reference capture: list the input streams whose `codec_type` is `video` or `audio` (from `ffprobe`), in index order, and run `ffmpeg -v error -i FILE -map 0:I1 -map 0:I2 ... -fps_mode passthrough -f framemd5 -`. Output stream `k` of the framemd5 text is input stream `Ik`; `index` is the input index. If there are no video or audio streams, `ffmpeg` is not run and the result is `decode_ok: true` with no streams.

| Normalized field | Gating | From `framemd5` |
|---|---|---|
| `decode_ok` | yes (file) | `ffmpeg` exit status 0 |
| `time_base` | used to compare timestamps | `#tb K: N/D` header line |
| `pts` | yes | 3rd column |
| `size` | yes | 5th column |
| `md5` | yes | 6th column |
| `dts` | informational | 2nd column |
| `duration` | informational | 4th column |

Pairing: streams by `index`; frames within a stream by position (output order). Unpaired frames are disagreeing. Agreement rate: agreeing frames / (agreeing + disagreeing frames). Files whose reference `decode_ok` is `false` are excluded.

## Thresholds

Agreement is checked with integers (`agree * 100 >= pct * total`).

| Profile | streams (files) | packets | frames |
|---|---|---|---|
| `fixtures` (default) | 100% | 100% | 100%, at least 1,000 frames compared |
| `corpus` | at least 98%, at least 100 files | at least 99% | 100%, at least 1,000 frames compared |

These are the project's correctness bars. Known divergences are not implemented yet; every mismatch counts.

## Open questions

1. Packet oracle gating fields "grouping", "pts/dts offsets" and "edit list" have no direct `ffprobe -show_packets` field. Edit lists and composition offsets already show up in `pts`/`dts`, which are gated. How should each be captured from the black box?
2. "Aspect ratio" (informational) is captured as both sample and display aspect ratio.
3. Rotation convention (counter-clockwise, `[0, 360)`, `0` when no display matrix) and `null` for non-integer angles.
4. `format_duration_us` is compared exactly at `ffprobe`'s microsecond precision; a candidate with exact ticks must round the same way. No tolerance is applied because the correctness bars name none.
5. Frame pairing is by output order with `-fps_mode passthrough`; a backend that splits audio into different frame sizes will not match until a policy exists.
6. Fixtures are cached by (ffmpeg build checksum, `scripts/gen-fixtures.sh` hash), while the fixture rule has them generated in CI on every run. `x264` output is not byte-stable across runs (`color_bt709.mp4` differed between two generations), so the reference is always captured from the same cached files.
