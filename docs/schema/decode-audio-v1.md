# `coreav decode-audio` summary, schema `coreav.decode-audio/1`

This is the reference for the summary `coreav decode-audio` prints after
writing a WAV file (`docs/design/audio-decode.md`): on standard output, or
on standard error when the WAV file itself goes to standard output. The
machine-readable twin is
[`decode-audio-v1.schema.json`](decode-audio-v1.schema.json) (JSON Schema
2020-12); tests validate every summary against it.

The rules of [`probe-v1.md`](probe-v1.md) apply: every key listed here is
always present, exact values come first, and adding a key keeps `/1`, while
removing or renaming a key, or changing a type or a meaning, makes `/2`.
Adding a value to an enumeration (a codec, a sample format) keeps `/1`. The
shared shape `time` is that of `probe-v1.md`.

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.decode-audio/1`. |
| `output` | output | The WAV file written. |
| `stream` | stream | The decoded stream. |
| `timeline` | timeline | Where the presented media sits in the output: the silence written for an MP4 edit list's empty edits (`docs/design/audio-decode.md`, "Edit lists"). |
| `warnings` | array of strings | Recoverable problems, in English (for example concealed packets, a container whose sample rate differs from the codec configuration's, a file that could not be read to its end). |

## Output

| Key | Type | Meaning |
|---|---|---|
| `bytes` | integer | Bytes written, header included. |
| `sample_format` | string | `s16`, `s24`, `s32` or `f32` (`--format`). |
| `sample_rate` | integer | Samples per second per channel: the stream's, or `--rate`. |
| `channels` | integer | Channels: the stream's, or `--channels`. |
| `channel_mask` | integer | Speaker positions as WAV's `dwChannelMask`; 0 when unknown. |
| `samples` | integer | Samples per channel written. |
| `duration` | time | `samples` in ticks of `sample_rate`. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `stream_index` | integer | Position of the stream among all tracks of the file (`ffprobe`'s `index`). |
| `codec` | string | The codec name of `coreav probe`: `flac`, `mp3`, `opus`, or `aac` in a build with the opt-in feature `aac`. |
| `sample_rate` | integer | The sample rate the codec configuration gives; for Opus 48,000, the rate it decodes to (its header's input rate is informative only). |
| `channels` | integer | The channel count the codec configuration gives. |
| `bits_per_sample` | integer or null | Bits per sample of a lossless codec; `null` for a lossy one. |
| `packets` | integer | Packets read, the ones that could not be decoded included. |

## Timeline

Sample n of the output is the sound at presentation time n / `sample_rate`
(the output's rate: resampling moves no sound in time, and each position
below is then the first output sample at or after the stream's):
an MP4 edit list's empty edits are written as silence, its media edits as
their windows, cut sample-exact. This holds while the media covers each
window: a window that reaches before the first or past the last media
sample writes only the samples there are, and later output moves earlier
by the missing length.

| Key | Type | Meaning |
|---|---|---|
| `start` | integer | Samples per channel before the first presented media sample: the silence of the leading empty edits, 0 without one (all the samples when no media sample was written). |
| `silence` | array | Every span of silence an empty edit wrote, in output order: `at` (its first sample per channel in the output) and `samples` (its length). Empty without empty edits. |
