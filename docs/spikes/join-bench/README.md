# Joins on the corpus

Measurement code, outside the Cargo workspace. It measures
`join_mp4` of `crates/coreav-isobmff-write` and the `coreav join` command
against the corpus bars of [`docs/design/join.md`](../../design/join.md)
on corpus v1. Results are added below as the runs finish.

## Hypothesis and bars

`join_mp4` joins files that share a codec configuration into one MP4 that
holds every input sample once and presents each input as it presents
itself; the segments of a split join back into their source; at least as
fast as `ffmpeg -f concat`, with memory bounded by the inputs' indexes and
the output's tables. The bars, quoted from the design note's "Corpus bars",
were fixed on 2026-10-19 in the commit that adds this README, and the
speed bars tightened to the default bar against FFmpeg in a later commit, both before any run:

- **Inputs**: the 869 corpus v1 files of
  `docs/spikes/clip-bench/evidence/inputs.json`, with the default limits,
  allowlist and audio choice, and the split's `no_audio` rule.
- **Joins per file**: *segments* (split every 10 s and joined back, checked
  against the source) and *clips* (head clip `[0, 10 s)` and tail clip from
  `D - 10 s` to the end joined, checked against the two clips; files of
  20 s or less skipped).
- **Checks**: the CoreAV round trip and the packet coverage of
  `join_check.py` on every join; the decoded video and audio on files of at
  most 500 MB.
- **Coverage: 100% of joins.** Every input sample in the output once, in
  order, by both the round trip and `ffprobe`'s packet hashes. Any miss is a
  bug and blocks the merge.
- **Segments**: at least **99% of files** give back their source on every
  check, directly or through a known divergence.
- **Clips**: at least **98% of joins** agree on every check, directly or
  through a known divergence.
- At least **99% of packets** agree over all joins.
- Refused joins and objects that fail their SHA-256 check are counted and
  listed apart, in neither denominator. Every other failure counts as
  disagreement and is investigated.
- **Speed against FFmpeg** (held-out files only): `coreav join -o OUT
  SEGMENTS...` on a file's 10-second segments against `ffmpeg -f concat
  -safe 0 -i LIST -c copy OUT`, both as processes on the same local disk,
  the median of 5 rounds with the order alternated after one warm-up of
  each. Bars: the median of ours ÷ ffmpeg over all held-out
  files at most **1.00**, and the median of each class (non-fragmented,
  fragmented, longer than one hour) at most **1.00**. A claim that joins
  are faster than FFmpeg needs the upper bound of the median's 95%
  bootstrap interval below 1.00.
- **Memory** (held-out files): peak resident memory of `coreav join` at
  most twice the segments' total `moov` bytes plus the output's `moov`
  bytes plus 8 MiB, on every held-out file.
- Misses on speed or memory block the release, not the merge; the cause is
  recorded before any bar changes.

## Method

- `jbench segments|clips SRC OUTDIR` (`src/main.rs`) makes the joins
  through `split_mp4`, `clip_mp4` and `join_mp4` and runs the round trip of
  `crates/coreav-isobmff-write/tests/join.rs`, with the output's edit lists
  read independently of the writer.
- `check.py` downloads each file from the private corpus bucket (SHA-256
  verified), runs both joins and `scripts/join_check.py`, writes one JSON
  record per file and deletes the media. `perf.py` (held-out files) times
  both engines and records peak resident memory.
- Spend: the development machine and R2 reads, which have no egress charge.

## Reproduce

```sh
cargo build --release                      # jbench
(cd ../.. && cargo build --release -p coreav-cli)
CLIP_CHECK_THREADS=2 python3 -I check.py --inputs ../clip-bench/evidence/inputs.json \
  --out evidence/runs/check-01 --work /tmp/jbwork --bin /opt/coreav-oracle-bin \
  --jbench target/release/jbench --workers 4
python3 -I perf.py --inputs ../clip-bench/evidence/inputs.json --out evidence/runs/perf-01 \
  --work /tmp/jbperf --bin /opt/coreav-oracle-bin --coreav ../../target/release/coreav
python3 -I summarize.py check evidence/runs/check-01 > evidence/summary-check-01.json
python3 -I summarize.py perf evidence/runs/perf-01 > evidence/summary-perf-01.json
```

## Results

| Bar | Result | Status |
|---|---|---|
| Coverage: 100% of joins | **1,225 of 1,225** joins: every input sample in the output once, in order, by the round trip and by `ffprobe`'s packet hashes over every stored sample | **Met** |
| Segments: at least 99% of files give back their source | **824 of 824** (100%): 809 directly, 15 through known divergences | **Met** |
| Clips: at least 98% of joins agree | **399 of 401** (99.5%): 163 directly, 236 through known divergences (`check-08-final`) | **Met** |
| At least 99% of packets agree | **16,010,351 of 16,010,351** | **Met** |
| Speed: median ours ÷ `ffmpeg -f concat` at most 1.00 | **0.114** (95% CI 0.104 to 0.129) over 241 files; p90 0.405, max 1.517; `perf-02`, round 2 after a fix (a development measurement, below) | **Met** |
| Speed: each class's median at most 1.00 | non-fragmented 0.112 (234 files), longer than one hour 0.078 (6), fragmented 0.207 (7); `perf-02`, round 2 after a fix | **Met** |
| Share of files where `coreav join` is faster (ratio below 1.00; information, no bar) | **231 of 241** (95.9%): non-fragmented 224 of 234, longer than one hour 6 of 6, fragmented 7 of 7; `perf-02`, round 2 after a fix, not a held-out result | — |
| Memory within the bound on every file measured | **241 of 241**, the closest at 83% of its bound; `perf-02`, round 2 after a fix | **Met** |

### Correctness (`check-08-final`)

`check-08-final` is `check-07-final` with every clip join replaced by its
re-run with the final checker:
- The segment joins come from `check-07`, which ran every corpus file with
  the final writer, `main` merged in. The run was resumed with more workers
  once the largest files were done, on the same tree, and its commit was
  later rewritten with another author address, again the same tree. Six
  large files ran out of disk; `check-07b` ran them again with two
  workers.
- The clip joins come from `check-08`, at `79c5845`, with `main` merged
  again. After the review, the checker's edit known divergences need
  every packet present and the documented times in `pts` and `dts`, as
  well as the input's own edit in the output, and it checks the output's
  presentation ends against the last input's files. Those rules apply to
  every clip join, and none to a join of segments, which is checked
  against its source.
- `check-08` ran all 401 files whose clip join ran. Nine of them failed
  `KD-J-AUDIO-EDIT-START`'s new gap check, each off by a fraction of a
  tick: their second input's offset falls between ticks of the audio time
  base.
- `check-08b`, at `c952e21`, took that residue within a tick and ran the
  11 disagreeing files again: the nine agree, and the two disagreements
  below remain.
- To compose it from the committed records: take each record of
  `check-07.json.gz`, replaced by `check-07b.json.gz`'s for the same
  `sha256`. Replace its `clips` entry with `check-08.json.gz`'s, then
  `check-08b.json.gz`'s, where they have the file. Write one JSON file per
  record into `RUN/files/`, then run `summarize.py check RUN`; that gives
  `summary-check-08-final.json`.

Earlier full runs: `check-06` (with
`check-06b` and `check-06c`) before the review's fixes to the checker (824
of 824 segments, 397 of 401 clips) and `check-05` before the merge with
`main` (822 of 823, 395 of 399). Earlier runs found what the writer and
checker fix, and were stopped: `check-01` after 3 files (a 2^32-tick
extension from unsigned composition offsets, and a full disk), `check-02`
after 61 files (segments of variable frame rate sources that did not join
back into one edit); the runs between them are not kept. Their findings,
all in the join or the checker:

- The writer: an input's video now ends at its edit where the edit lies
  within the media's declared duration, ends within the spacing of its last
  two frames after the last one, or less than a frame after its samples
  (frames whose decode durations are shorter than their spacing had ended
  inputs early, cut their audio and kept segments from merging); samples
  composed 2^31 ticks or more after their decode are left out of the
  ordering of inputs; frames whose durations overlap the next input's by a
  tick need no room where the edit bounds them.
- The checker: `ffmpeg`'s decoder gives no frame for the first packet of
  some AAC streams; it delivers packets again before a later audio edit,
  which changes the noise decoded with perceptual noise substitution (the
  reference is now those packets decoded in the order it delivers them);
  HE-AAC frames are 2,048 samples; a first sample of duration 0 shares its
  decode time; `start_time` is printed to six decimals; joins of segments
  are compared on every stored sample; a source without an edit list is
  moved by `ffmpeg` (`KD-J-PTS-SHIFT`); a later input's frames can be shown
  early by up to the first input's composition delay.

- **Refused** (counted apart): segments 41 (21 encrypted `encv` video, 14
  video other than `avc1`, `hvc1`, AV1 or VP9, 6 edit lists of several
  edits, which the split does not take yet); clips 54 (24 without a video
  duration, 14 unsupported video, 10 whose tail clip starts at an open-GOP
  sync sample with leading frames, 3 without video in a clip's range, 3
  whose clips keep several edits, which a join does not take yet) and 413
  files of 20 s or less, which have no two clips.
- **Malformed sources** (refused by the parser): 3; the fourth of the
  split's run, a movie box without a movie header, is read by `main`'s
  parser now and joins.
  **Not downloadable**: `184becaa…`, removed from the corpus by an earlier change.
- **The 2 disagreements**, both in decoded audio of clips, none in packets:
  - `bbd77a523fd5`: the source's AAC packets are stored with durations of
    1,019 and 1,020 ticks for frames of 1,024 samples, so its times and its
    decoded samples drift apart (the clip run's `KD-C-AUDIO-JITTER` class).
  - `2680e5010bcd`: one decoded AAC frame of the head clip's edit, which
    starts 23 ms in, differs from the reference.

  `4c82d16e5037` and `985a28c5abe5`, whose sources mark frames as sync that
  a decoder cannot start from, disagreed before `main`'s cut rule
  (`docs/design/cut-points.md`) came in: their tail clips started at such
  a frame. Clips now start only where the coded data does, and they agree.
- **Known divergences** covering joins: `KD-J-AUDIO-EDIT-START` 345,
  `KD-J-AUDIO-NOISE` 59, `KD-J-VIDEO-LATER-EDIT` 19, `KD-C-AAC-PROFILE` 14,
  `KD-J-VIDEO-EDIT-END` 8, `KD-J-PTS-SHIFT` 3, `KD-J-DTS-SHIFT` 3. The
  join of `e5715db399f5`'s segments (a source whose edit ends inside its
  last frame) agrees here; it disagreed in `check-05`, before the merge
  with `main`. `KD-J-SYNC-VERIFIED` covers no corpus join; it covers the
  fixture `clip_hevc_stss_all.mp4`.

### Speed and memory (`perf-02`, round 2 after a fix)

`perf-02` measured the same held-out partition as `perf-01`, whose memory
misses drove the fix below (`Source::release`). A set used to tune after a
failed round is no longer held out, so `perf-02` is a development
measurement: its verdicts on the bars (at most 1.00, memory on every file)
stand as round 2 after a fix, and no claim that joins are faster than
FFmpeg is made from it. That claim waits for a fresh held-out set that
neither round has measured, with the implementation frozen first: the
`v2-heldout` top-up planned under an earlier change.

254 files of that partition, 13 refused, 241 timed, on the final code
(`perf-02.commit`, before the merge of `main`'s change A and cut rule; change A
leaves every fixture's join the same byte for byte, and the cut rule moves
the cuts only of sources whose container marks frames as sync that the
coded data does not start from); Intel Xeon at 2.9 GHz, 8 vCPU, no noisy file. Each file
is split every 10 s with `coreav split`, and the segments are joined by
`coreav join` and by `ffmpeg -f concat -c copy`. Median wall time per file
5.3 ms for `coreav join` against 57 ms for `ffmpeg`; the upper bound of the
median's 95% interval is 0.129. `coreav join` is faster on 231 of the 241
files (95.9%; information beside the bars). The slowest ratios (1.52, 1.51, 1.50) are files of
0.9 to 2.6 GB joined from 5 to 14 segments.

`perf-01` ran the same measurement before the merge with `main` and missed
the memory bar on 9 of 239 files, all with many segments (72 to 786): the
join keeps every input's source open, and each buffered source kept its
read window (the 748-segment file peaked at 38 MiB against a bound of 18
MiB). Measured first (`massif` on a generated file of 900 segments: 3.7 MB
of the 9.5 MB peak heap were the inputs' 4 KiB windows), then fixed: the
join releases each input's window between the passes that read it
(`Source::release`). In `perf-02` the 748-segment file peaks at 14 MiB; the
bar is unchanged. One `perf-01` file failed to download (R2 HTTP 502); in
`perf-02` none did.

### Spend

0 USD: the development machine and R2 reads (no egress charge); the cloud host was
not used.
