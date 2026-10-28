# MediaRecorder corpus generator

Makes real WebM and Matroska files written by Chrome's own `MediaRecorder`
muxer. They are test input for the CoreAV Matroska/WebM parser: live-recorded
files have traits that offline muxers rarely produce, such as unknown-size
Segment and Cluster elements without Cues, chunked delivery, pause/resume
gaps and mid-stream resolution changes.

No camera or microphone is used. `record.html` draws an animated canvas
(moving shapes, a frame counter and shifting colors) and captures it with
`canvas.captureStream(fps)`; audio is a swept `OscillatorNode` routed to a
`MediaStreamAudioDestinationNode`. The page records the stream, joins the
chunks into one Blob exactly as delivered (no re-muxing) and POSTs it back.

## Run

    python3 -I tools/mediarecorder/generate.py --out /tmp/mediarecorder

Options: `--chrome PATH` (default `google-chrome`), `--only NAME,NAME` to run
a subset. The script uses the Python standard library only. It serves the page
on `127.0.0.1` with an ephemeral port, starts headless Chrome once per
configuration with a fresh profile in a temp directory, waits for the upload
(recording time + 45 s), then kills Chrome. Media files never go under
`tools/`.

Chrome flags: `--headless=new --disable-gpu --no-first-run
--autoplay-policy=no-user-gesture-required --use-fake-ui-for-media-stream`
plus the three background-throttling switches, so timers and the canvas keep
running at full rate.

Before recording, the page checks `MediaRecorder.isTypeSupported(mimeType)`;
unsupported types are listed in the manifest as skipped. Every file is then
probed with `ffprobe -v error -show_streams -show_format -of json` as a black
box; files ffprobe rejects are kept and show a non-zero `ffprobe.exit`.

`DIR/manifest.json` has one entry per configuration: `name`, `file`, `size`,
`sha256`, `mimeType` (requested), `recorderMimeType` (what Chrome reported),
`chunks` (number of `dataavailable` blobs), `docType` and
`segmentSizeUnknown` (read from the first bytes), `chrome` (`google-chrome
--version`), `params`, `skipped` and `reason`, and `ffprobe` (exit status,
stderr, codec names, stream summary, format name, duration).

## Matrix

`MATRIX` in `generate.py` is the source of truth. Parameters: size `w`x`h`,
`fps`, `dur` (wall-clock ms from `start()` to `stop()`, pauses included),
`slice` (`start(timeslice)` in ms, or `none`), `audio`/`video` on or off,
optional `vbps`/`abps`, `pause` (`AT:LEN` ms pairs) and `resize`
(`WxH@AT`, canvas resize mid-recording).

| mimeType | Configs | Covers |
|---|---|---|
| `video/webm;codecs=vp8,opus` | 9 | 160x120 to 1280x720, portrait 360x640, 15/30 fps, 2 to 20 s, slice none/100/1000, pause, resize up, low bitrate (100 kbit/s) |
| `video/webm;codecs=vp9,opus` | 7 | 60 fps, 1280x720 10 s, portrait, two pauses, resize down, high bitrate (8 Mbit/s) |
| `video/webm;codecs=vp9` | 1 | video only |
| `video/webm;codecs=av1,opus` | 4 | 320x240 to 1280x720, slice 100/1000, pause |
| `video/webm;codecs=h264,opus` | 4 | 1280x720 60 fps, portrait, resize |
| `video/x-matroska;codecs=avc1,opus` | 4 | `.mkv`, 1280x720 60 fps, slice 100/1000, pause |
| `video/x-matroska;codecs=avc1` | 1 | `.mkv`, video only |
| `video/webm` (browser default) | 3 | with audio, with slice, video only |
| `audio/webm;codecs=opus` | 3 | audio only, slice 100, pause |
| `audio/webm` (browser default) | 2 | 3 s, 20 s at 32 kbit/s |
| `audio/webm;codecs=pcm` | 1 | PCM audio track |
| `video/webm;codecs=vp8,pcm` | 1 | VP8 with PCM |
| `video/x-matroska;codecs=vp9,opus` | 1 | VP9 in `.mkv` |

## Last run

Chrome: Google Chrome 155.0.8059.84 (Amazon Linux 2023, x86_64, headless).
41 configurations, 41 recorded, 0 skipped, about 29 MB in total. ffprobe
read every file with exit status 0 and no errors.

What this Chrome wrote:

- Without a timeslice (24 files) Chrome finalizes the file: known Segment and
  Cluster sizes, a SeekHead padded with a Void element, Duration, and Cues at
  the end. With a timeslice (17 files) the Segment and every Cluster have the
  8-byte unknown size `01 ff ff ff ff ff ff ff`, with no SeekHead and no Cues.
- `video/webm;codecs=h264,opus` is accepted, but Chrome reports
  `video/x-matroska;codecs=avc1,opus` and writes DocType `matroska` (file
  names keep `.webm` from the requested type). `audio/webm;codecs=pcm` and
  `video/webm;codecs=vp8,pcm` also get DocType `matroska` (PCM is
  `pcm_f32le`). `video/x-matroska;codecs=vp9,opus` gets DocType `webm`.
- AV1 is reported as `codecs=av01`. The browser defaults are VP8 + Opus for
  `video/webm` and Opus for `audio/webm`.
- Canvas resizes change the coded frame size mid-stream (for example 53
  frames at 320x240 then 79 at 640x360); the track header keeps the initial
  size.
