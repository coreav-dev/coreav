# HLS packaging measurements on corpus v1

Throwaway measurement code, outside the Cargo workspace. It
measures `coreav hls` (`crates/coreav-isobmff-write/src/hls`, design note
`docs/design/hls.md`) against the bars that design note fixes, on corpus v1. The results are at the end.

## Hypothesis and bars

HLS packaging by stream copy, with segments cut on request from an index
built once, plays back the source's packets exactly, is faster than FFmpeg
cutting the same segment on request, and holds memory independent of the
file size. The bars, quoted from `docs/design/hls.md` ("Corpus and speed
bars") and committed before these runs:

- **Correctness:** `scripts/hls_check.py` on every input at the default 6 s
  target, with MediaInfo; segments on their own at the first, middle and
  last segment. "At least 98% of packaged files agree"; "at least 99% of
  packets agree"; inputs refused for a stated reason (codec, sample entry,
  edit list, non-AAC default audio, decode-time gaps) are listed apart and
  are not in either denominator; every other failure counts as
  disagreement and is investigated.
- **Segment on request:** the middle video segment of each held-out input,
  `coreav hls get` against `ffmpeg -ss START -i INPUT -t DURATION -map
  0:v:0 -c copy -f mp4 -movflags frag_keyframe+empty_moov+default_base_moof`;
  "the median ratio ours ÷ ffmpeg is at most 0.5".
- **Whole package:** `coreav hls package` against `ffmpeg ... -f hls
  -hls_segment_type fmp4 -hls_time 6 -hls_playlist_type vod`, a reference;
  "the median ratio is at most 1.0".
- **Index:** time and size per input. Reported.
- **Memory:** "the peak resident set of `coreav hls get` for the middle
  segment is at most 32 MiB on every held-out input".
- Misses are recorded with their cause before any bar changes.

### A choice made before the corpus runs (2026-10-19)

The bars do not say which audio the corpus packages keep. As for the clip
measurements (`docs/spikes/clip-bench/`), an input whose default audio the
packager refuses because it is not AAC (mostly PCM in group-b files) is
packaged again without audio (`--audio none`), so that its video is checked
too; such inputs are marked `no_audio`.

### Speed and memory again on the final binary (2026-10-19)

`perf-03` met the bars above on binary `5675214`. The code changed after it
(review fixes, the rebase onto `main`), so speed and memory are measured
again on the final binary as `perf-04`, which replaces `perf-03` as the
result (`perf-03` stays as evidence). The default speed bars against
FFmpeg apply to this new measurement, on top of the design note's bars:

- segment on request: median ratio at most 0.5 overall (the design note's
  bar, tighter than parity), and at most 1.00 in every class;
- whole package: median ratio at most 1.00 overall and in every class;
- memory: as above, at most 32 MiB on every held-out input;
- classes are seven groups of the held-out inputs (`group-a` to `group-g`);
- "faster than FFmpeg" is said only where the upper bound of the 95%
  confidence interval of the median (bootstrap) is below 1.00.

### perf-05: the no-audio choice for speed too, on one binary (2026-10-19)

Review of `perf-04` found two gaps, fixed before this run: `perf.py` did not
apply the choice above (20 held-out inputs with PCM audio were refused
rather than measured without group-a, so the group-b class rested on 9 of its
25 inputs), and 4 of its records came from a later binary than the rest.
`perf-05` measures every held-out input with the final binary (its SHA-256
in each record), packaging an input whose default audio is not AAC without
audio, with FFmpeg's package mapping the video alone for it (its segment
command maps the video alone already). The bars are those of `perf-04`
above, unchanged; `perf-05` replaces `perf-04` as the result.

## Inputs

`evidence/inputs.json` (`make_inputs.py`): every `set: real` file with
extension mp4, mov, m4v or 3gp and a `v:h264`, `v:hevc`, `v:av1` or `v:vp9`
tag, and every `real` m4a or m4b file tagged `a:aac`: 905 files (265 held
out, 36 audio only, 148.2 GB). Correctness uses every file; speed and memory
the held-out files only. Media come from the private corpus bucket,
verified by SHA-256 and deleted after use; nothing is committed but the
results.

## Method

- **Correctness** (`check.py`): per file, `coreav hls package`, then
  `hls_check.check_one` (ffprobe streams and packet hashes of every
  rendition and of the master playlist through ffmpeg's HLS demuxer, ffmpeg
  framemd5 of every rendition against the source's, the first, middle and
  last segments on their own, EXTINF and segment starts, MediaInfo), with
  `docs/known-divergences-hls.md` applied. Four files at a time, ffmpeg with
  two threads each. Correctness does not depend on the machine.
- **Speed and memory** (`perf.py`): one held-out file at a time on an
  otherwise idle machine; one warm-up run of every engine, then 5 rounds
  with the engine order rotated; the median of the 5 is the file's time;
  memory is the peak resident set of each process, from GNU time (which
  forks from a small process, so the harness's own pages are not counted).
  Every output is deleted before each run, outside the timing.
- Correctness runs on a cloud host: one call per input on at
  most 16 containers of 4 CPU and 8 GiB, ffmpeg with four threads; the
  launcher stops the app at a deadline. Speed and memory run on the development machine (8 vCPU, AWS).
  Downloads from R2 are within the free tier.

`check-03` ran past its deadline: the launcher only stopped at the
deadline when a result arrived, and containers that ran out of memory were
retried. The launcher now stops at the deadline on its own. Every later run
(`perf-03`, `recheck-01`, `perf-04`) ran on the development machine. `check-03b` reran the 20 largest inputs in 16 GiB containers; it stopped after 1,465 s with 1 of the 20 done, out of memory, and its result is not used.

## Reproduce

```sh
python3 -I make_inputs.py
cargo build --release -p coreav-cli          # from the repository root
# the corpus check ran on a cloud host (launcher not published), or check.py on any machine
python3 -I affected.py --inputs evidence/inputs.json --old OLD --new NEW --out OUT --work /tmp/aff
python3 -I check.py --inputs evidence/inputs.json --out evidence/runs/recheck-01 --work /tmp/hlswork \
  --coreav ../../target/release/coreav --mediainfo /opt/coreav-oracle-bin/mediainfo --only SHA,...
python3 -I perf.py --inputs evidence/inputs.json --out evidence/runs/perf-04 --work /tmp/hlsperf \
  --coreav ../../target/release/coreav --ffmpeg /opt/coreav-oracle-bin/ffmpeg
python3 -I summarize.py --check evidence/runs/check-03 --overlay evidence/runs/recheck-01 --perf evidence/runs/perf-04
```

`evidence/records.tar.xz` holds the per-input records of every run
(`runs/*/files/`); the summaries are `evidence/summary-*.json`.

## Results

Every bar is met, on the final binary (commit `f8e7f7e`).

### Correctness

`evidence/summary-check.json`: `check-03` with the records of
`recheck-01` in its place where the code changed after it.

- **Re-checks instead of a second corpus run.** After each change,
  `affected.py` packaged every input with the binary of the last check and
  with the new one, and compared every resource byte for byte; only inputs
  whose outputs changed, or whose refusal became a package, were checked
  again (`evidence/recheck-why.json`, `evidence/affected-rebase.json`),
  with failures, missing records and covered inputs re-checked too. The
  last fixes only turn refusals into packages, so every refused input was
  re-checked after them. Every record in the summary therefore stands for
  outputs byte-identical to the final binary's.
- **904 inputs** (corpus v1 dropped one damaged entry after `check-03`):
  858 packaged, 46 refused for a stated reason (21 encrypted video, 14 other
  video codecs, 6 edit lists outside the model, 4 damaged files, 1 video
  track that does not start with a sync sample).
- **Files: 857 of 858 agree (99.88%; bar 98%)**, 767 directly and 90
  through documented divergences (KD-HLS-1 45, KD-HLS-2 38, KD-HLS-3 14,
  KD-HLS-4 9, KD-HLS-5 7, KD-HLS-6 6, KD-HLS-7 3, KD-HLS-8 3, KD-HLS-9 1).
  62 of them were packaged without their non-AAC audio. The one failure is
  not a disagreement found: the pinned ffmpeg aborts while decoding the
  source itself (`Assertion pkt failed`, after 3,284 of 4,723 AV1 frames),
  so the input cannot be checked; it counts as a failure all the same.
- **Packets: 21,963,954 of 21,963,954 agree (100%; bar 99%)**, in 101,377
  segments.
- Found and fixed through the corpus, each with a regression test:
  overlapping and non-sync AAC samples, edit lists and offsets as stored,
  hidden leading and trailing samples, `RESOLUTION`, `CHANNELS`, IDR checks
  when every sample is marked sync, audio alone beside a dropped chapter
  text or chapter image track (9 audio-only files), and media edits
  that last to the end of the media (9 fragmented recordings). The
  checker's own misses (EXTINF tolerance without packet durations, KD-HLS-1
  and KD-HLS-3 together) are fixed in `scripts/hls_check.py`.

### Speed and memory

`evidence/summary-perf-05.json`: the 265 held-out inputs, 251 measured (20
of them without their PCM audio, as in the correctness run) and 14 refused,
all with the final binary (its SHA-256 in every record). Ratios are CoreAV
/ FFmpeg, medians of per-input medians, with bootstrap 95% confidence
intervals.

| Operation | CoreAV | FFmpeg | Median ratio (95% CI) | Bar |
|---|---:|---:|---|---|
| A media segment on request (`hls get` against `ffmpeg -ss ... -c copy`, fragmented) | 2.5 ms | 15.6 ms | **0.197** (0.181 to 0.215) | at most 0.5 |
| The whole package (`hls package` against `ffmpeg -f hls -hls_segment_type fmp4`) | 6.5 ms | 39.5 ms | **0.212** (0.200 to 0.227) | at most 1.00 |

| Class | n | Segment ratio (95% CI) | Package ratio (95% CI) |
|---|---:|---|---|
| group-a | 11 | 0.261 (0.172 to 0.311) | 0.150 (0.081 to 0.212) |
| group-b | 25 | 0.196 (0.101 to 0.273) | 0.177 (0.126 to 0.209) |
| group-c | 35 | 0.148 (0.110 to 0.205) | 0.295 (0.232 to 0.374) |
| group-d | 108 | 0.234 (0.222 to 0.242) | 0.250 (0.237 to 0.271) |
| group-e | 21 | 0.123 (0.104 to 0.150) | 0.116 (0.107 to 0.137) |
| group-f | 35 | 0.118 (0.096 to 0.136) | 0.136 (0.115 to 0.151) |
| group-g | 16 | 0.258 (0.178 to 0.320) | 0.197 (0.172 to 0.241) |

- Every class is far below 1.00, and every upper bound is below 1.00, so
  CoreAV cuts a segment on request and packages a whole file faster than
  FFmpeg. The slowest single input is at 0.51
  (segment) and 0.50 (package).
- **Memory:** `hls get` peaks at 4.4 MiB on every held-out input (median
  4.1 MiB; FFmpeg's segment command 18.6 MiB); bar 32 MiB. `hls package`
  peaks at 17.2 MiB.
- **Index:** built in 2.3 ms (median; at most 81 ms), 0.20% of the source's
  size (median; at most 8.4%, for a short file with many small samples, and
  at most 3.9 MB).
- Earlier runs, kept as evidence: `perf-03` (binary `5675214`, before the
  review fixes and the rebase) gave 0.191 and 0.221; `perf-04` (231 inputs,
  without the no-audio choice) gave 0.206 and 0.220. Absolute times differ
  between runs with the machine's load and the inputs measured; the ratios
  hold.

## Sources with several edits

Whether the writers can package them, measured in
`several-edits/` and recorded in `docs/design/edit-list-writers.md`,
"The HLS writers". They stay refused; an earlier change is the design that lifts it.
