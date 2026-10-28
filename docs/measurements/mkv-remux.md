# Matroska and WebM to MP4: `coreav remux` against the black-box oracles

These are the results of the corpus run whose pass bars `docs/design/mkv-remux.md` fixed
before the run (commit `58f2f12`). They are counts only. Per-file
records stay outside git.

## Method

- **Binaries:**
  - `coreav` built from the PR's code after the first review's fixes. The second review's fix only changes streams that would have failed the whole file before (no corpus file did), DiscardPadding longer than the last frame, and trailing laced Opus frames with an unreadable TOC; every fixture and MediaRecorder output is byte-identical with it, so the corpus was not run again.
  - The pinned oracles: `ffprobe` and `ffmpeg` 7.0.2, and MediaInfo 26.10.
- **`tools/mkv-adapter/remux_corpus.py run`**, file by file:
  1. Downloads the file from R2 and checks its SHA-256.
  2. Runs `coreav remux` with the library defaults.
  3. Checks the output with `scripts/mkv_remux_check.py` (see below).
  4. Runs `ffmpeg -c copy -movflags +faststart` on the streams `coreav` kept, for time and memory only.
  5. Deletes the outputs and the download.

  Three files run at a time. Each command runs under a small `wait4` helper that records its wall time and peak RSS. `report` counts per split.
- **Checks**, all against the source:
  - the output's top-level boxes are `ftyp`, `moov`, `mdat`;
  - `ffprobe` reads the output with the source's codec, profile, dimensions, colour description, sample rate and channels;
  - every packet has the same MD5 and size (`ffmpeg -c copy -f framemd5`). For Chrome's Annex B H.264, the decoded frames' MD5s and their count are compared instead;
  - every packet keeps its presentation time: within 1 ms for video, and within 1 ms plus half the frame for audio;
  - every packet falls inside the presented range of the edit list;
  - MediaInfo reads an MPEG-4 file with the kept tracks.
- **Inputs:**
  - corpus v1 `real` files with extension `mkv` or `webm`: development split 140, held-out split 69;
  - the 41 Chrome 155 MediaRecorder recordings ([`docs/corpus.md`](../corpus.md#mediarecorder-recordings)).
- **Held-out split:** it ran twice.
  - First with the binary that existed before the independent review: 49/49 files and 2,154,877/2,154,877 packets.
  - Then with the final binary.

  The review fixes came from the reviewer's findings. None was a response to held-out results.
- **Re-checks after the lost-video rule** (all in one change): a video stream that cannot be copied now fails the remux instead of leaving an audio-only file, and the VP8 refusal says what to do instead. These change only files with such a video stream: the 12 VP8 + Opus files that had been remuxed without their video, and refused files, whose messages change. The other 175 files' outputs do not change. The affected files were run again with the final binary; the records of the 175 remuxed files come from the earlier binary.
- **Last review's fixes:**
  - The code fixes cover laced Opus without times, streams with nothing presented, composition offsets beyond 32 bits, and the BlockAdditions count. With them, every one of the 175 remuxed files gives a byte-identical output and summary. Both binaries ran on every file, and the files were downloaded again to compare.
  - The checker's new test needs no new run. It requires every source stream of a copyable codec to be kept, unless it is encrypted, has no frames or has nothing presented. No remuxed file dropped such a stream: their dropped streams are subtitles, Vorbis, and AC-3, E-AC-3 and MP2 outside the default allowlist.
- **One re-check:** the final run flagged one development file. Its last Opus frame starts 0.5 ms before the end of the media edit, after the edit was rounded to the 1 ms movie timescale. That frame is presented, but the checker's half-millisecond slack pointed the wrong way. After the checker was fixed, only that file was checked again.

## Results

| Group | Files | Remuxed | Refused | Files passing every check | Packets agreeing | `ftyp`/`moov`/`mdat`, read by `ffprobe` and MediaInfo | Crashes, panics, timeouts |
|---|---|---|---|---|---|---|---|
| Development split | 140 | 99 | 41 | 99/99, 100% | 4,743,346/4,743,346, 100% | 99/99 | 0 |
| Held-out split | 69 | 49 | 20 | 49/49, 100% | 2,154,877/2,154,877, 100% | 49/49 | 0 |
| MediaRecorder (Chrome 155) | 41 | 27 | 14 | 27/27, 100% | 5,446/5,446, 100% | 27/27 | 0 |
| **All** | **250** | **175** | **75** | **175/175, 100%** | **6,903,669/6,903,669, 100%** | **175/175** | **0** |

Every bar of `docs/design/mkv-remux.md` holds, on all remuxed files and on the held-out split alone:

- at least 99% of files pass;
- at least 99.9% of packets agree;
- 100% of outputs have the layout and are read by `ffprobe` and MediaInfo;
- there are no crashes, panics or timeouts.

Before the lost-video rule, the same run remuxed 187 files (the 12 VP8 + Opus files as audio only), all passing, with 6,914,154/6,914,154 packets.

The remuxed files total 35.0 GB.

- **Kept streams:**
  - video: VP9 83, H.264 31 (the 9 from MediaRecorder rewritten from Annex B), AV1 35, HEVC 6;
  - audio: Opus 96, AAC 13.
- **Truncated source:** one file, copied up to its last complete frame.
- **BlockAdditions:** 14 files had them on kept streams (7,973 elements), reported and not carried.

Refusals, by the codecs in the file (read by `coreav probe`), counted apart from the denominators. A video stream that cannot be copied, in a file whose other streams could be written, fails the remux with `video stream N cannot be copied to MP4: REASON; --drop-unsupported writes the other streams without it`. When nothing else could be written either, the error is the video stream's reason alone. The VP8 reason reads: "VP8 in MP4 does not play in Safari; keep the WebM, which plays in Chrome, Firefox and Safari, or re-encode to VP9, H.264 or AV1 outside CoreAV".

| Codecs in the refused file | Error | Development | Held-out | MediaRecorder | Total |
|---|---|---|---|---|---|
| VP8 only | the VP8 reason | 13 | 6 | 1 | 20 |
| VP8 + Vorbis | the VP8 reason | 20 | 13 | 0 | 33 |
| VP8 + PCM | the VP8 reason | 0 | 0 | 1 | 1 |
| VP8 + Opus | video stream 0, the VP8 reason, `--drop-unsupported` | 1 | 0 | 11 | 12 |
| MPEG-2 + AC-3, MPEG-4 Part 2 + MP2 (outside the default allowlist) | no stream can be copied to MP4 | 2 | 1 | 0 | 3 |
| Vorbis only | MP4 has no standard sample entry for Vorbis | 4 | 0 | 0 | 4 |
| PCM only | PCM audio is not copied to MP4 | 0 | 0 | 1 | 1 |
| Encrypted Opus | encrypted streams are not copied | 1 | 0 | 0 | 1 |

VP8 is in 66 of the 75. `vp08` output from the VP codec ISO binding was tested and is not planned: Safari 27 refuses such files while it plays the WebM, so the error tells the user to keep the WebM or re-encode outside CoreAV.

Streams dropped from files that were remuxed (audio, subtitle and data streams are dropped and listed by default):

| Reason | Development | Held-out | MediaRecorder |
|---|---|---|---|
| Subtitle streams are not copied to MP4 | 27 | 0 | 0 |
| `codec_not_allowed` (default allowlist; audio and subtitles) | 19 | 2 | 0 |
| MP4 has no standard sample entry for Vorbis | 9 | 2 | 0 |

## Time and memory against FFmpeg (informational)

These numbers come from the same run: three files at a time on the 8-core x86_64 VM, with each file just downloaded and in the page cache. Both sides start a process per file and write a complete MP4 file with the movie box first. FFmpeg needs a second pass for `+faststart`, and CoreAV writes `moov` first in its single write. No bar applies.

| Group | Files | CoreAV total | FFmpeg total | Median per-file speedup | CoreAV peak RSS, median / max | FFmpeg peak RSS, median / max |
|---|---|---|---|---|---|---|
| Development split | 99 | 30.2 s | 49.7 s | 2.00x | 3.7 / 56.1 MiB | 17.4 / 77.6 MiB |
| Held-out split | 49 | 12.9 s | 23.6 s | 2.02x | 3.6 / 50.4 MiB | 16.9 / 74.9 MiB |
| MediaRecorder | 27 | 0.30 s | 0.49 s | 1.84x | 3.2 / 3.7 MiB | 14.6 / 20.4 MiB |

CoreAV's memory grows with the number of samples (one record each) and not with the file size. The largest file, 4.3 GB, peaked at 34 MiB against FFmpeg's 41 MiB.

## Spend

R2 reads only, which have no egress fees: about 123 GB downloaded across the two corpus runs, the re-checks and the output comparison, and a few thousand read requests, well under 0.01 USD. No other infrastructure was used.
