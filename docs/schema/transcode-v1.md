# `coreav transcode` summary, schema `coreav.transcode/1`

This is the reference for the summary `coreav transcode` prints on
standard output after writing an MP4 file
([`design/transcode.md`](../design/transcode.md)). The machine-readable
twin is [`transcode-v1.schema.json`](transcode-v1.schema.json) (JSON
Schema 2020-12); tests validate every summary against it.

**Stability.** The rules of [`probe-v1.md`](probe-v1.md) apply: every key
listed here is always present, and a change of a key is a new schema
version. Transcoding a source whose video is not H.264 is experimental
([`design/transcode.md`](../design/transcode.md), "Support"): the summary
is the same for it, but what the command does with such a source may
change.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.transcode/1`. |
| `output.bytes` | integer | Bytes written. |
| `video` | video | The re-encoded video stream. |
| `audio` | array of stream | The audio streams copied, in source order. |
| `encoded_audio` | encoded audio or `null` | With `--audio opus` or `--audio aac`, the audio stream encoded to Opus or to AAC-LC; `null` otherwise. |
| `other` | array of other stream | Streams copied that are neither the re-encoded video nor audio (another video stream, MP4 text), in source order: the remux keeps every stream MP4 can carry. |
| `dropped` | array of dropped stream | Streams of the source that are not in the output, in source order. |

## Video

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | The source stream's position among all of its streams (`ffprobe`'s `index`): the stream `coreav thumbnail` decodes, FFmpeg's default choice. The output keeps its place in the track order. |
| `codec` | string | The source stream's codec, by `ffprobe`'s name. |
| `frames` | integer | Frames encoded: every frame the source presents, less `dropped_frames`. The output's stream has as many. |
| `dropped_frames` | integer | Frames the decoder returned whose timestamp was not above the one before (or that had none): not encoded. |
| `not_presented` | integer | Frames decoded that the source does not present (before or after its edit list's media edit), not encoded. The decoder is told it need not return them, so this counts only those it returned anyway. |
| `backend` | string | The decoder that ran: `nvdec`, `native`, `dav1d`, `libvpx` or `ffmpeg` (the sandboxed FFmpeg fallback). |
| `encoder` | string | The encoder program: `x264`, or `ffmpeg` (FFmpeg's libx264). |
| `width`, `height` | integer | The output's size in pixels (after `--scale`). |
| `sample_aspect` | string or `null` | The output's sample aspect ratio as `N:D` when it is not 1:1 (`--scale` changes it to keep the display aspect ratio, as FFmpeg's `scale` filter does); `null` otherwise. |
| `frame_rate.num`, `frame_rate.den` | integer | The presented frames' average rate in frames a second, snapped to an integer or `N×1000/1001` rate within 0.1% of one, for information: the encoder is given every frame's own time (`docs/design/transcode.md`, "Rate control and timestamps"), and the output's timestamps are the source's. |

## Encoded audio

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | The source stream's position among all of its streams. The output's encoded stream keeps its place in the track order. |
| `codec` | string | The source stream's codec: `flac` or `mp3`. |
| `sample_rate` | integer | The source stream's sample rate in Hz. The Opus stream is always 48 kHz: other rates are resampled. The AAC stream has the source's rate, 8 to 64 kHz. |
| `encoder` | string | `libopus` for Opus; `aac`, FFmpeg's built-in AAC encoder, for AAC-LC. |
| `channels` | integer | The encoded stream's channels, 1 or 2: the source's. |
| `bit_rate` | integer or `null` | The bit rate the encoder was asked for, in bits a second: `--audio-bitrate` times 1,000, else for Opus 64,000 for mono and 96,000 for stereo, and for AAC `null` (the `aac` encoder's own default). |
| `samples` | integer | Samples per channel that the output presents, at the encoded stream's rate: the decoded source stream's. |
| `pre_skip` | integer | The encoder's delay in samples at the encoded stream's rate, which the edit list leaves out (for Opus, `ffprobe`'s `initial_padding`). |
| `packets` | integer | Packets written: Opus packets of 20 ms, or AAC frames of 1,024 samples. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the stream among all streams of the source. |
| `codec` | string | Its codec, by `coreav probe`'s name (`aac`, `mp3`, `opus`, ...). |

An other stream has `kind` too (`video`, `subtitle`, ...).

## Dropped stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the stream among all streams of the source. |
| `reason` | string | `not_selected` (`--audio none` drops every stream but the video, `--audio opus` every stream but the video and the encoded audio), `codec_not_allowed` (left out by the container's default allowlist) or `unsupported` (MP4 cannot carry it, as `coreav remux` drops it: Vorbis, PCM in MOV, a timecode track). |
| `message` | string or `null` | For `unsupported`, why. |

## Errors

A file that cannot be transcoded prints a `coreav.error/1` object on
standard error ([`probe-v1.md`](probe-v1.md), "Errors") with exit code 1,
or 3 for an I/O error, and leaves no output and no spool file. Besides the
probe's kinds: `unsupported_feature` for refusals by design (a pixel format
the encoder does not take, an odd size without `--scale`, an edit list
outside the simple model, no encoder program; with `--audio opus`, audio
CoreAV does not decode, more than two channels, or a build without the
`libopus` feature), `decode` (the decoder failed
on the data: the message starts with `decoding:`; or the encoding side
failed: the encoder program, the reading of its output, which must be one
picture for each frame sent, or the MP4 writer: the message starts with
`encoding:`), `limit` with `limit` `timeout` (the transcode took longer than
`--timeout`), and `codec_not_allowed`. Only the input side (the demuxers,
the decoder and the checks before the encoder starts) reports
`unsupported_feature`, `unsupported_container`, `truncated_structure` or
`invalid_structure`; the encoding side reports `decode` (or `limit`, or
`io` for an I/O error), so that a defect there never reads as a refusal. Usage errors (an invalid `--scale`,
`--crf` with `--bitrate`, an output that is the input, exists without
`--force`, or is `-`) exit with 2. In strict-safe mode (`--strict-safe`,
`COREAV_STRICT_SAFE`) what would run an `ffmpeg` program, the `ffmpeg`
encoder (`--encoder ffmpeg`, or `auto` without an `x264` program) or the
FFmpeg decoder for a video no other decoder of the build takes, is refused
with exit status 4 and kind `unsupported_feature`, leaving no output; its
message names the way out (`docs/design/compat-cli.md`, "Strict-safe
mode").
