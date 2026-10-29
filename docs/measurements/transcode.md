# `coreav transcode`: bars B1 to B6, and B9

The correctness bars (B1 to B4) and the speed bar (B5) of
[`docs/design/transcode.md`](../design/transcode.md), "Bars", fixed before
any of these runs. Two rounds: the first on the build that first had every
part of the command, the second on the build with the defects the first
round found fixed. A round is final for the code it ran on; the first
round's numbers stay here as they came out.

| | Round 1 | Round 2 |
|---|---|---|
| Build | `cb0589b2` (the head of the branch that added the round), features `native-h264`, `dav1d`, `ffmpeg-fallback` | `e4b215ed` (`main` after the earlier two changes), the same features |
| Evidence | [`transcode/round-1/`](transcode/round-1) | [`transcode/round-2/`](transcode/round-2), [`transcode/fixtures/`](transcode/fixtures) |

Arms: `--encoder ffmpeg` (the pinned FFmpeg 7.0.2, libx264 core 164
r3191 4613ac3) and `--encoder x264` (x264 built from 4613ac3, SHA-256
`3977f112…`, in each `build.txt`). The baseline is the note's FFmpeg
command. Correctness runs: `--preset veryfast --crf 23`, on a development
VM (8 vCPU, 16 GB), not timed, by `scripts/transcode_check.py`. Speed:
`--preset medium --crf 23` on a Compute Engine `c4-standard-8`, by
`scripts/transcode_speed_gcp.py`. The corpus selection is the same in both
rounds (`corpus-selection.json`: 270 files, 128 H.264, 20 HEVC, 28 AV1, 32
VP9, 38 VP8, 24 ProRes; 100 of them in the scaled configuration), and so is
the speed selection ([`transcode/speed-selection.json`](transcode/speed-selection.json):
68 held-out files).

## B9, through `coreav ffmpeg` (bars 100%)

The bar of [`docs/design/transcode.md`](../design/transcode.md), item 9, with its criterion as changed on 2026-10-29 (the dated note there). The run that decides it is the first `isobmff-fixtures` run on `90590d8c`, GitHub's merge of `6cbbf6c1` (the commit that made that change) into `57ce872b`, the base branch at the time. Earlier runs do not count. Arms as above: `x264` (the x264 program, SHA-256 `c4d88654…`) and `ffmpeg` (no x264 program, so FFmpeg's libx264), each against the pinned FFmpeg 7.0.2 running the same command line. Lines x01 to x12 are the command lines of the note, in its order (also in `manifest.json`). Of the 100 fixture files, 79 have a video stream and are judged.

| Line | Runs | B1 | B2 | B3 | Q1 median, min (dB), `x264` arm | Q1 median, min (dB), `ffmpeg` arm | Q2 min (dB), `x264` / `ffmpeg` |
|---|---|---|---|---|---|---|---|
| x01 | 27/79 | 27/27 | 27/27 | | 0.000, 0.000 | 0.000, 0.000 | identical / identical |
| x02 | 27/79 | 27/27 | 27/27 | | 0.000, 0.000 | 0.000, 0.000 | identical / identical |
| x03 | 27/79 | 27/27 | 27/27 | | 0.000, 0.000 | 0.000, 0.000 | identical / identical |
| x04 | 27/79 | 27/27 | 27/27 | | 0.000, 0.000 | 0.000, 0.000 | identical / identical |
| x05 | 21/79 | 21/21 | 21/21 | | 0.000, 0.000 | 0.000, 0.000 | 42.1 / 42.1 |
| x06 | 27/79 | 27/27 | 27/27 | | 0.000, 0.000 | 0.000, 0.000 | identical / identical |
| x07 | 27/79 | 27/27 | 27/27 | | −0.035, −0.085 | −0.035, −0.092 | 46.5 / 46.5 |
| x08 | 46/79 | 46/46 | 46/46 | 25/25 | 0.000, −0.007 | 0.000, 0.000 | 42.1 / 42.1 |
| x09 | 27/79 | 27/27 | 27/27 | | 0.000, 0.000 | 0.000, 0.000 | identical / identical |
| x10 | 55/79 | 55/55 | 55/55 | 28/28 | −0.096, −0.154 | −0.096, −0.194 | 44.7 / 44.7 |
| x11 | 62/79 | 62/62 | 62/62 | 33/33 | 0.000, −0.037 | 0.000, −0.036 | 39.5 / 39.5 |
| x12 | 52/79 | 52/52 | 52/52 | 25/25 | 0.000, 0.000 | 0.000, 0.000 | 99.2 / identical |

Both arms ran on the same fixtures for every line, and every call either ran, or was refused with exit status 4 for a reason the note names, writing nothing. Of the 523 refused calls per arm, 312 are audio that would be re-encoded (x01 to x07, x09, x11 on a source with audio; `-c:a aac` is not covered), 48 chapters or a timecode, 36 edit lists of more than one edit, 32 a late start, 20 a 12-bit 4:4:4 source the encoder does not take, 16 a decoded pixel format other than `-pix_fmt`, 14 uneven frame spacing, 13 a size change, 11 reordered frames without composition offsets, and 9 a track matrix other than the identity without `-noautorotate`. Set apart: `truncated.mp4` on each of the 12 lines, exit status 1, "truncated structure", as `-c copy` rejects it. The one fixture that misses B2 with `coreav transcode` too (`size_inband_sps.mp4`) was refused on every line, so no call relied on that exception. Q2 "identical" means every frame equals FFmpeg's output's.

Since that run, three refusals were added (the note's second dated change). On the fixtures they change one file: `size_left_crop.mp4`, which ran and passed on all 12 lines in the deciding run (against an FFmpeg command with `-flags +unaligned`), is refused with exit status 4. The evidence below is the deciding run's and is unchanged.

Evidence, in [`transcode/b9/`](transcode/b9): `calls.jsonl.gz` (one row per line, fixture and arm, the baseline included: status, exit status, reason, B1 to B3, Q1, Q2, size ratio), `manifest.json` (commit, command lines, arms, SHA-256 of the tools and the fixtures, FFmpeg version) and `results.tar.gz` (`run.json`, `summary.json` and every per-file result). They are the job's `oracle-output` artifact, `coreav-b9/evidence`; `calls.jsonl.gz` is its `calls.jsonl` gzipped. To recompute the bars and the tables the job printed (paths aside: the evidence names files by name):

    mkdir b9 && tar -xzf docs/measurements/transcode/b9/results.tar.gz -C b9
    python3 -I scripts/transcode_check.py --summarize --out b9

It exits 0 when every bar is met. The table above, from the per-call rows:

    zcat docs/measurements/transcode/b9/calls.jsonl.gz | jq -c -s 'map(select(.arm != "baseline" and .status == "ok")) | group_by([.config, .arm])[] | {line: .[0].config, arm: .[0].arm, runs: length, B1: map(select(.B1)) | length, B2: map(select(.B2)) | length, Q1_median: (map(.Q1_delta_db) | sort | if length % 2 == 1 then .[length / 2 | floor] else (.[length / 2 - 1] + .[length / 2]) / 2 end)}'

## Round 2

### Fixtures (B1 to B4, bars 100%)

| | `ffmpeg` arm | `x264` arm | Baseline |
|---|---|---|---|
| B1, decodes cleanly | 71/71 | 71/71 | 71/71 |
| B1, native decoder equals `framemd5` | 65/65 (6 apart: 10-bit, 4:2:2) | 65/65 | |
| B2, frames and times | **70/71** | **70/71** | 67/71 |
| B3, audio copied | 38/38 | 38/38 | 31/38 |
| Q1 ≥ −0.50 dB, Q2 ≥ 30 dB | 71/71, 71/71 | 71/71, 71/71 | |
| `--bitrate 1000` within 10% of `-b:v 1M` | 10/10 | 10/10 | |

B6 on the same build: the hostile suite (321 files) and 276 mutated copies of
the fixtures through the command with the x264 program, no failure
([`transcode/fixtures/hostile-transcode.json`](transcode/fixtures/hostile-transcode.json)).

Apart: 3 edit lists of more than one edit, 2 pixel formats the encoder does
not take, 1 truncated file. The B2 miss is `size_inband_sps.mp4`: H.264
with B-frames in an MP4 without composition offsets ("Times in decode
order" in the note). All 48 frames are encoded with the container's
times; `ffprobe`'s `best_effort_timestamp` of the source, the oracle,
takes the packets' decode times shifted by FFmpeg's decoder delay, which
grows twice in the stream, so the last frame differs by 0.08 s. The
baseline misses the same file.

### Corpus (B1 to B4)

Default configuration, each arm (245 files judged, 25 apart):

| Class | B1 | B1 native | B2 | B3 | Q1 median ΔPSNR, share ≥ −0.50 dB, size ratio | Q2 share ≥ 30 dB |
|---|---|---|---|---|---|---|
| H.264 | 125/125 | 122/122 | 125/125 | 73/73 | 0.000 dB, 1.00, 1.000 | 1.00 |
| HEVC | **13/18** | 12/12 | **14/18** | 6/6 | 0.000 dB, **0.78**, 1.000 | **0.78** |
| AV1 | 27/28 (`ffmpeg`), 28/28 (`x264`) | 18/18, **18/19** | 27/28, 28/28 | 10/10 | 0.000 dB, **0.96** (`ffmpeg`) and 1.00 (`x264`), 1.000 | **0.96**, 1.00 |
| VP9 | 26/26 | 19/19 | 26/26 | 10/10 | −0.000 dB, 1.00, 1.000 | 1.00 |
| VP8 | 38/38 | 38/38 | 37/38 | 0/0 | 0.000 dB, **0.97**, 1.000 | **0.97** |
| ProRes | 10/10 | 0/0 | 10/10 | 0/0 | 0.000 dB, 1.00, 1.000 | 1.00 |
| All | **239/245** (`ffmpeg`), **240/245** (`x264`) | 209/209, **209/210** | **239/245, 240/245** (97.6%, 98.0%; bar 99%) | 99/99 | 0.000 dB, **0.976** (`ffmpeg`) and **0.980** (`x264`), 1.000 | **0.976**, **0.980** |

Bars: B1 100%; B2 99% over all classes and 95% in each; B3 the same over
files with audio; Q1 median at least −0.10 dB, 98% of files at least
−0.50 dB and a median size ratio between 0.98 and 1.02, per class and arm;
Q2 median at least 35 dB and 98% at least 30 dB. Scaled configuration
(`--scale -2:360`, 92 judged), the same on both arms: B1 90/92, B2 89/92
(96.7%; HEVC 15/16, VP9 7/8, VP8 6/7), B3 46/46, Q1 and Q2 shares 0.967
(HEVC 0.9375, VP9 0.875, VP8 0.857); H.264 57/57 throughout.

Met: B3, and every bar for H.264 on both arms in both configurations.
Missed: B1 (also in the scaled configuration, HEVC 14/16); B2 over all
classes and in HEVC in both configurations (scaled: 96.7% overall, HEVC
15/16), and in the scaled configuration's VP9 and VP8; Q1 and Q2 (the share of files within −0.50 dB and at 30 dB)
over all classes, in HEVC and VP8, in AV1 on the `ffmpeg` arm, and in the
scaled configuration's HEVC, VP9 and VP8. Every Q1 median ΔPSNR is
0.000 dB to within ±0.02 dB and every median size ratio within 0.997 to
1.002: the shares miss through the files that failed or lost frames. The
misses, each counted in every column it touches:

- Three 8K HEVC files: the encoder's address space on this 16 GB host is
  capped at half its memory (8 GiB), and x264 needs more at 8K (`malloc`
  failed; with `--encoder ffmpeg`, one also ended with 12 pictures for 330
  frames). By design: the cap keeps a stream of large frames from taking
  the host's memory. The baseline, which has no such cap, ran two of them.
- One HEVC file: `packets waiting to be decoded exceed the limit`; the
  pacing of an earlier change did not hold this stream's packets back. The cause, found
  after this round: the worker handed on its frames only when a message
  from the parent came, so the pacing's wait got none and ended at its 5 s
  stall, retiring every packet counted while FFmpeg had not read them; a
  few such waits passed the 1 GiB limit. The worker now hands on frames
  while idle, and in a spot check (not a measured round) the file
  transcodes, 2,428 of 2,428 frames on an 8-CPU host; this round's
  counts stay as run.
- One HEVC file (B1, `ffprobe` reads every stream): a fault of the check,
  not of the output. The source's movie metadata (`udta`) carries a cover
  image (an `ilst` `covr` item), which the output keeps as stored, as
  `coreav remux` does, and which `ffprobe` lists as a second video stream
  with the attached-picture disposition. `ffprobe` reads it and the output
  decodes cleanly, but the check counted it as a written video track.
  From the next round the check sets attached pictures apart; this
  round's counts stay as run.
- One VP8 file (4 frames) and, in the scaled configuration, one VP9 file
  (116 frames): WebM blocks whose millisecond times repeat. A frame whose
  time is not above the last one's is dropped and counted (the note's
  rule); FFmpeg keeps them and moves their times, and its output misses
  B2 as well. After this round the command keeps such frames, each one
  tick after the frame before while that is below the next time, and B2
  expects those times.
- One 8K AV1 file through the `ffmpeg` arm: the encoder returned 1,759
  pictures for 1,761 frames, which ends the transcode with an error.
- B1 native, one 8K AV1 output (`x264` arm): the decode oracle's own
  64 GiB decoded-bytes budget, not the output (a limit of
  `tools/decode-oracle`).

Baseline (the FFmpeg command): B1 241/241, B2 186/241, B3 64/99; its B2
and B3 misses are listed in `corpus-summary.json` (`baseline_misses`).

### Speed (B5, bar: median of per-file ratios at most 1.05, overall and in every class, each arm)

| Class (files) | `ffmpeg` arm | `x264` arm | Decoder |
|---|---|---|---|
| H.264 (25) | 1.038 [1.031, 1.050] | 1.024 [1.019, 1.034] | native (24), fallback (1) |
| HEVC (8) | **1.057** [1.044, 1.074] | **1.200** [1.036, 1.408] | fallback |
| AV1 (6) | **1.068** [1.043, 1.075] | **1.054** [1.028, 1.270] | dav1d |
| VP9 (10) | **1.099** [1.067, 1.151] | **1.095** [1.058, 1.123] | fallback |
| VP8 (13) | **1.060** [1.047, 1.095] | 1.046 [1.032, 1.069] | fallback |
| ProRes (4; 2 refused) | **1.108** [1.084, 1.146] | **1.890** [1.660, 2.363] | fallback |
| All (66) | **1.058** [1.048, 1.069] | 1.046 [1.034, 1.063] | |

Medians with bootstrap 95% intervals (10,000 resamples), CoreAV over the
baseline. The gate (the baseline without audio on a 20 s 720p
`testsrc2` file) took 2.736 s before and 2.608 s after, 4.7% apart (void
above 5%). VM time 5.23 h, 3.14 USD at the script's 0.60 USD an hour
bound.

Missed on both arms: the bar holds overall and in every class, for each
arm. Within it, H.264 on both arms, and VP8 and all classes together on
the `x264` arm, are at most 1.05; every other class, and all classes on
the `ffmpeg` arm, are above. The classes decoded by the
sandboxed FFmpeg worker and by dav1d cost 5% to 10% more than FFmpeg doing
both in one process on the `ffmpeg` arm (ProRes 10.8%), and more on the
`x264` arm in HEVC (1.200) and ProRes (1.890); the note's reason for a bar above 1.00 (frames
crossing two process boundaries) holds, but the cost is above the 5% it
allowed for those decoders. The `x264` arm's HEVC and ProRes medians are
above the `ffmpeg` arm's on the same files. A profile after the run, not
part of the bars, points at 10-bit sources: on the same raw 10-bit 4K
frames the x264 program took 15.9 s and FFmpeg's libx264 6.8 s, the
program's input conversion running serially, while 8-bit frames took the
same time (4.26 s and 4.35 s). It was not measured on these files.

## Round 1

On `cb0589b2`, every correctness bar but B3 in some classes missed, and
speed missed in every class but H.264 (all classes together met on the
`x264` arm, 1.050), mostly through defects that earlier changes fixed:

- 47 files failed with `the frames decoded exceed the decode limits` or
  `decoded bytes exceed the limit` (46 on the `x264` arm, where one of them
  failed in x264's `malloc` instead): the decode default of 4 GiB decoded
  bytes per request, about 55 s of 1080p, applied to a whole transcode.
- 2 files failed with `packets waiting to be decoded exceed the limit`:
  no pacing of packets ahead of the sandboxed decoder.
- 3 8K files failed in x264's `malloc` under the 8 GiB address space.
- 19 B3 misses were the harness's: a WebM whose only audio is Vorbis, or
  a MOV with PCM, which `coreav remux` does not copy, counted as a file
  with audio whose audio went missing.

| | `ffmpeg` arm | `x264` arm |
|---|---|---|
| Corpus B1 | 193/245 | 193/245 |
| Corpus B2 | 193/245 | 193/245 |
| Corpus B3 | 84/123 | 84/123 |
| Speed, H.264 | 1.040 [1.030, 1.066] | 1.026 [1.010, 1.031] |
| Speed, all | **1.081** [1.056, 1.147] | 1.050 [1.032, 1.129] |

The speed run counted 22 of its 66 files as failed on both arms (ratio
+∞), by the same limits; the other classes' medians are +∞ or above 1.05.
VM time 2.45 h, 1.47 USD bound: the job had ended (`job.status` 1) when the
script was interrupted during the VM's deletion, so the run's hours were
taken by hand from the VM's creation and deletion, which
was confirmed by listing the zone's instances.

## Spend

4.61 USD of Compute Engine at the bound (round 1 1.47, round 2 3.14); the
correctness runs ran on the development VM at no cost.
