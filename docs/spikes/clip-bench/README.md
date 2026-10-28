# Release measurements: keyframe-aligned clips to MP4

Throwaway measurement code, outside the Cargo workspace. It
measures `clip_mp4` of `crates/coreav-isobmff-write` and the `coreav clip`
command against its release bars on corpus v1.

All results below are final; the writer is `5998d7f` in every final run.

## Hypothesis and bars

The design of the clip writer claims it cuts a keyframe-aligned clip of one video
track and at most one AAC track into a valid MP4 file with the source's
packets and timing, at least as fast as libavformat clipping the same
streams in-process, with memory bounded by the output tables. The bars,
quoted from its design record and fixed before these runs:

- **Correctness (point 10).** Round trip: "`coreav-isobmff` probes every
  output. For each kept track: the samples equal the source's kept run
  (bytes, sizes, durations, sync flags, composition offsets; decode times
  shifted by the run's first decode time), and every sample presented in
  the output is presented in the source at its output time plus the
  snapped start." Black-box oracles: "`ffprobe` and MediaInfo read the
  output as a valid MP4 with the source's codecs, dimensions, rotation,
  sample rate and channels, and stream durations equal to the clip's edits;
  packet hashes from `ffprobe -show_packets -show_data_hash` equal those of
  the source packets in the kept runs; and `ffmpeg -copyts -f framemd5` of
  the decoded output equals the source's decoded frames presented inside
  the clip, shifted by the snapped start, for video and audio."
- **Corpus clips (point 10).** Every corpus v1 `real` file with an ISO base
  media extension and a video tag the writer may accept; four clips per
  file (head `[0, 10 s)`, middle `[D/2 - 5 s, D/2 + 5 s)`, tail from
  `D - 10 s`, whole), `D` the video stream's duration in whole
  milliseconds, clips equal to an earlier one of the same file not
  repeated.
- **Thresholds (point 10, choice 7).** "at least 98% of clips agree: every
  check agrees directly or through a known divergence"; "at least 99% of
  packets agree: over all clips, agreeing packets ÷ (agreeing +
  disagreeing packets)"; "clips that the writer refuses under points 2 to 5
  ... are counted and listed apart ... and are not in either denominator;
  every other failure counts as disagreement".
- **Speed (point 11).** "Baseline: libavformat in-process (black-box
  boundary) seeking to the video sync sample at or before
  the start and stream-copying the same video and audio streams to MP4
  until the end, with the `ffmpeg -ss ... -to ... -c copy` command line for
  reference. The unit is one corpus clip; its ratio is our time over the
  baseline's. Bars: the median ratio over all held-out clips is at most
  1.10, and the median of every class is at most 1.5. The classes are the
  file classes [of audio extraction] (non-fragmented,
  fragmented, longer than one
  hour) and the clip positions (head, middle, tail, whole)."
- **Memory (point 11).** "Peak growth beyond the parser's own use is
  bounded by the output sample tables of both tracks plus the one 64 KiB
  read window, independent of the file size and of the clip's position;
  checked with the test-only counting allocator on every
  held-out clip, on a file longer than one hour (its tail clip and the
  whole file) and on a 2 GB generated fragmented file with video and
  audio."
- **Remote input (point 11).** Bytes read over `RangeReader` as a share of
  the object, and the time against a full download plus a local clip.
  "Reported, not gated."
- "Misses on speed or memory block the release, not the merge of the
  code; the cause is recorded before any bar changes."

### A choice made for the corpus clips (2026-10-18)

Point 10 does not say which audio the corpus clips keep. 71 of the 869
selected files (20 held out) have only non-AAC
audio, mostly PCM, which the default audio choice refuses (point 3). A
clip that the default audio refuses for that reason is cut again without
audio (`--audio none`), so that the video of those files is checked too. Such clips are marked `no_audio`.

## Results at a glance

| Bar | Result | Status |
|---|---|---|
| Correctness: at least 98% of clips and 99% of packets | **2,634 of 2,634** clips (763 of 763 held out) and **16,511,491 of 16,511,491** packets agree, 431 clips through known divergences (`check-04` with `check-04b` and `check-04c`); as `check-04` ran, 99.66% of clips and 99.77% of packets | **Met** |
| Speed: median `ours / lav_avio` at most 1.10 over the held-out clips | **0.437** (95% CI 0.406 to 0.472) over 763 clips of 238 files, `perf-06` on the final code (`5998d7f`) | **Met** |
| Speed: the median of every class at most 1.5 | largest class median 0.555 (longer than one hour) | **Met** |
| Memory: growth at most the output tables plus 64 KiB on every clip | **767 of 767** clips within the bound (`perf-06`), closest 1,569 bytes under; the 2.2-hour file's whole clip 3.49 MB against 3.55 MB; the generated 2.2 GB fragmented file's whole clip 0.24 MB against 0.25 MB | **Met** |
| Remote (reported) | a head or middle clip requests 57% of the object at the median, 3.3% for objects of 100 MB to 1 GB and 1.8% above 1 GB; never faster than a download plus a local clip where timed (8.5 times slower at the median, 272 clips) | Reported |

## Correctness (point 10, `check-04`)

Every corpus file (869; 254 held out), its four clips through `clip_mp4`
with default limits and allowlist, each clip checked by the round trip
and the oracles of `scripts/clip_check.py`. Writer `5998d7f`; checker as
described under "Oracle failures during the correctness run" below.

| | Clips | Agree directly | Through known divergences | Disagree | Packets agree |
|---|---:|---:|---:|---:|---|
| `check-04` with `check-04b`'s 4 files and `check-04c`'s 1 | 2,634 | 2,203 | 431 | **0** | 16,511,491 of 16,511,491 |
| held out only | 763 | 668 | 95 | **0** | |
| `check-04` as it ran | 2,634 | 2,201 | 424 | 9 | 16,457,889 of 16,495,891 (99.77%) |

- **The 9 clips that failed as `check-04` ran**, in 3 files, and what
  `check-04b` found with the final checker: 4 clips of a held-out AV1 file
  whose source `ffmpeg` could not decode with two threads (agree with the
  one-thread retry); 3 clips of a development HEVC file with negative
  composition offsets and no edit list (`KD-C-PTS-SHIFT`); 2 clips of a
  development H.264 file whose fragment defaults mark every sample as sync
  (`KD-C-SYNC-FLAGS`).
- **Known divergences** covering clips: `KD-C-AUDIO-RESTART` 190,
  `KD-C-STTS-SHORT` 99, `KD-C-CTTS-UNSIGNED` 88, `KD-C-AAC-PROFILE` 84,
  `KD-C-EMPTY-EDIT-DURATION` 65, `KD-C-EDIT-MIDFRAME` 18, `KD-C-DTS-SHIFT`
  15, `KD-C-PTS-SHIFT` 6, `KD-C-AUDIO-EDIT-RATE` 5, `KD-C-SYNC-FLAGS` 4,
  `KD-C-AUDIO-JITTER` 1 (a clip can be covered by several).
- **Refused clips** (point 10: counted apart, in neither denominator): 117.
  61 for an edit list outside the supported model, 52 for video other than
  `avc1`, `hvc1`, AV1 or VP9 (all `hev1`, choice 6), 4 for a range that
  holds no video (head clips of videos that start after 10 s).
- **Files without a clip**: 26. 21 encrypted files (`encv`, which the
  allowlist drops, so no allowed video track is left), 4 that the parser
  rejects (missing or second movie header, truncated), and 1 development
  file whose corpus object failed its SHA-256 check on every download (the
  stored object, not the writer).
- **Without audio**: 233 clips of 62 files whose only audio is PCM (211)
  or MP3 (22) were cut again without audio (the choice above).
- **By clip**: head 810, middle 608, tail 608, whole 608, all agreeing.
- **Earlier runs.** `check-01` to `check-03` were stopped when fixes made
  them stale: two writer fixes (`b4a2ad4`, `6edb775`) after the development
  sweeps, and in `check-03` the end of the kept run (`913846a`) and
  `KD-C-AUDIO-EDIT-RATE` (two development HE-AAC files).

## Speed (point 11, `perf-06`)

Per-clip ratio `ours / lav_avio`, the median of 5 rounds each, held-out
clips only; lower is faster. Final code (`5998d7f`), an AWS ap-south
machine at 3.1 GHz. 16 held-out files have no clip the writer accepts
(refusals, listed in the correctness results).

| Class | Clips | Median | 95% CI | p90 | Max |
|---|---:|---:|---|---:|---:|
| All held out | 763 | **0.437** | [0.406, 0.472] | 0.948 | 2.003 |
| Non-fragmented | 743 | 0.435 | [0.403, 0.471] | 0.948 | 1.133 |
| Fragmented | 20 | 0.522 | [0.431, 0.612] | 0.729 | 2.003 |
| Longer than one hour | 24 | 0.555 | [0.454, 0.639] | 1.042 | 1.112 |
| Head | 238 | 0.333 | [0.304, 0.357] | 0.899 | 1.126 |
| Middle | 175 | 0.496 | [0.411, 0.539] | 0.943 | 1.118 |
| Tail | 175 | 0.517 | [0.469, 0.578] | 0.968 | 2.003 |
| Whole | 175 | 0.494 | [0.426, 0.534] | 0.954 | 1.133 |
| Equal work only (both engines copy the same packet count) | 460 | 0.435 | | | |
| Sensitivity: against `lav_native` | 763 | 0.475 | | | |
| Reference: against the `ffmpeg` command line | 763 | 0.101 | | | |
| Generated 2.2 GB fragmented file (not held out): head, middle, tail, whole | 4 | 0.649, 0.697, 0.841, 0.674 | | | |

- **Time to the first byte**: ours 0.62 ms, libavformat 1.60 ms at the
  median; ratio 0.32 (head) to 0.46 (whole) at the median, up to 5.6 on a
  tail clip, where the three listings walk the source before the first
  byte.
- **Where the advantage shrinks**: clips of fragmented files (the listings
  read every fragment header before the clip, choice 10), tail clips, and
  short clips of large files, whose open costs as much as the copy. The
  slowest clip (2.00) is a tail clip of a fragmented file: 8.7 ms against
  4.3 ms.
- **Every round.** By the rule fixed before each repeat ("Noisy runs"
  below), the noisy rounds are kept and reported; the bars hold in all of
  them.

| Run | Code | Machine | Noisy clips | Median [95% CI] | Largest class median |
|---|---|---|---:|---|---|
| `perf-02` | `0a1f812` | Azure us-east, 3.70 GHz | 135 | 0.657 [0.604, 0.728] | 0.848 (tail) |
| `perf-03` | `0a1f812` | GCP eu-west, 3.05 GHz | 9 | 0.490 [0.448, 0.525] | 0.613 (fragmented) |
| `perf-04` | `5b7dc4a` | GCP us-east, 2.45 GHz, preempted once | 158 | 0.645 [0.605, 0.692] | 0.768 (tail) |
| `perf-05` | `5b7dc4a` | AWS ap-southeast, 3.6 GHz | 1 | 0.442 [0.408, 0.478] | 0.541 (longer than one hour) |
| `perf-06` | `5998d7f` | AWS ap-south, 3.1 GHz | 0 | **0.437** [0.406, 0.472] | 0.555 (longer than one hour) |

- **Earlier runs.** `perf-01` and its resumption `perf-01b` measured the
  code before the last memory fix and were stopped for the final round;
  the evidence files keep what the volume held. Between `0a1f812` and
  `5998d7f` the writer changed where a clip starts and ends (commits
  `b4a2ad4`, `6edb775` and `913846a`), not how it copies.

## Memory (point 11)

`cbench-mem` on every clip of `perf-06` (the final code; memory does not
depend on the machine, and `perf-02` to `perf-05` gave the same numbers): **767 of 767** within the bound, all
allocator counts valid. Growth over bound: median 0.974, at most 0.992,
closest 1,569 bytes under.

| File | Size | Duration | Clip | Growth | Output tables | Bound |
|---|---:|---:|---|---:|---:|---:|
| `3d8f6047…` (held out, longest) | 987 MB | 2.2 h | head / tail | 31,977 / 34,129 | 4,516 / 6,724 | 70,052 / 72,260 |
| | | | whole (564,919 samples) | 3,492,788 | 3,486,888 | 3,552,424 |
| `8ca1a5e6…` (held out, largest) | 3.15 GB | 4 min | whole | 94,196 | 31,320 | 96,856 |
| Generated fragmented | 2.20 GB | 10 min | whole (44,065 samples) | 242,948 | 183,844 | 249,380 |

- **The fix found by the first round.** In `perf-01` seven clips exceeded
  the bound by up to 45 KB. On development files with the same traits (small
  files, the clip near the end) a trace showed the cause: near the end of
  the file the copy window refilled with fewer than 64 KiB, a read that
  `BufferedSource` keeps in a second buffer of its own. Refills now end at
  the end of the file, and `BufferedSource` hands reads that end at the end
  of its source straight to the caller (commit `ab76480`). Before that, two
  smaller excesses on development files had come from the copy pass's
  bookkeeping on the heap and the summary built while the copy buffer was
  allocated (commit `7654791`).

## Remote input (reported, not gated; `remote-01`)

Per held-out file, the head and middle clips: their exact requests and
bytes over `RangeReader` (default policy), replayed from a local copy, and
where a clip needs at most 300 requests (within a 1,500 s budget for the
run) the same over R2 from a cloud host us-east container, against downloading
the whole object and clipping it locally.

| Object size | Clips | Bytes requested / object (median) | Requests (median) | Timed over R2 | Range / (download + local), median |
|---|---:|---:|---:|---:|---:|
| below 10 MB | 222 | 0.931 | 12 | 222 | 8.3 |
| 10 to 100 MB | 88 | 0.207 | 22 | 50 | 9.5 |
| 100 MB to 1 GB | 87 | 0.033 | 120 | 0 | |
| above 1 GB | 16 | 0.018 | 436 | 0 | |

- Range reads were never faster where timed; each R2 request took its own
  round trip from us-east to the bucket in APAC. Above 100 MB a short clip
  reads a few percent of the object, which a routing rule like the one for
  remote indexing could exploit; gating it needs its own record (point 11).

## Sources with several edits (`docs/design/edit-list-writers.md`)

The clip writer takes sources with several edits since that note. Its
clip section fixes the bars before these runs: fixtures and the six corpus
files with several edits at 100% with no known divergence, and speed at
most 1.00 against `lav_avio` in a class of the six files' clips, in every
existing class (not worse than `perf-06`), and over all held-out clips.

**Correctness (`check-me-02`, the final writer, on the development
machine).** All **21 of 21** clips of the six files (8 of 8 held out)
pass every clip oracle directly, and **642,555 of 642,555** packets
agree. The oracle reads them with edit lists ignored and applies the
edit lists by ISO/IEC 14496-12 itself (common decision 3 of the note).
The second view, ffmpeg's edit-applied frames, agrees on 14 clips. On
the other 7 it differs, and in each case both files follow ffmpeg's
measured edit model: later edits anchored on decode times on all seven
(on ea690039d902, the source's second edit 2.375 s earlier than ISO/IEC
14496-12 places it), and on ea690039d902 also the first edit snapped to
its first composition time and frames whose times ffmpeg's output
clamped. No difference is unexplained. (`check-me-01`, before the
review's fixes, gave the same counts.)

**Speed and memory (`perf-me-05`, the six files; `perf-09`, every
held-out file; the cloud host 2 CPU).**

| Bar (fixed in the note before the runs) | Result | Status |
|---|---|---|
| Class `several edits`: median `ours / lav_avio` at most 1.00 | **0.576** (95% CI 0.327 to 0.773), 21 clips of 6 files (`perf-me-05`) | **Met** |
| Its held-out part, reported apart | 0.605 (0.211 to 1.194), 8 clips of 2 files: no claim | Reported |
| Every existing class at most 1.00 | largest class median 0.726, longer than one hour (`perf-09`) | **Met** |
| Every existing class not worse: median at most `perf-06`'s upper CI bound | above it in every class, e.g. non-fragmented 0.494 against 0.471, tail 0.595 against 0.578 (`perf-09`) | **Not met** (cause below) |
| All held-out clips at most 1.00 | **0.497** (0.468 to 0.528), 783 clips of 243 files (`perf-09`) | **Met** |
| Memory: the six files' whole clips within the point 11 bound | **6 of 6**; a166d09989f9 1,225,937 bytes against 1,258,024 (`perf-me-05`) | **Met** |

- **Why "not worse" is not met: it predates this change, and part of it
  is the machine.**
  - This writer against `main` on the development machine, for files
    without several edits: 18 clips of 6 held-out files, 21 alternating
    runs each, median ratio 1.009 (95% bootstrap 0.996 to 1.029), and
    the same memory on every clip.
  - `perf-09` ran on Azure at 3.7 GHz and `perf-06` on AWS at 3.1 GHz.
    Over the 763 clips both timed, `lav_avio` took 0.70 of its
    `perf-06` time and CoreAV's open-and-probe 0.685, but CoreAV's clip
    0.79, at every position.
  - So the clip's own work grew by about 13% relative to the rest since
    `perf-06`'s writer (`5998d7f`), in code already on `main`, which also holds the numbers). The likely part is the
    cut-point checks of an earlier change, which read coded data before the copy
    (`docs/design/cut-points.md`).
- **Memory.**
  - The writer keeps no state per source edit: the windows are read from
    the track's own edit list entries (clip decision 7 of the note). With
    them copied as a table (24 bytes per edit), a166d09989f9 (6,069 audio
    edits) was 113,673 bytes over the bound on its whole clip
    (`perf-me-03`) and 141,688 on its 10-second clips; with exact
    rationals before that, 796,856 bytes of growth on a 10-second clip.
  - Outside the bound in `perf-me-05`: 6e67814d336c's middle and tail
    clips (72,261 and 145,989 bytes against 69,088 and 69,296), the
    cut-point candidate queue of an earlier change, already on `main`. In `perf-09`,
    40 of 787 held-out clips, all but one middle or tail clips of files
    with many sync samples before the start; a2d42b18's head clip
    is 398 bytes over with `main`'s writer too.
  - With `main`'s fix for an earlier change merged into this branch, those two clips
    measure 67,538 and 67,730 bytes on the development machine (bounds
    69,088 and 69,296), so every clip of the six files is within the
    bound; the whole clips are unchanged (a166d09989f9 1,225,937,
    6e67814d336c 1,491,478).
- **Slowest clips.** The tail clips of the two long files, a166d09989f9
  (2.39) and 6e67814d336c (2.02). The packet listing of a track with
  several edits costs about three times as much per sample as one with a
  single edit (a166d09989f9's audio: 189,662 packets in 17 ms), and a clip
  lists each track from its start several times, while libavformat seeks
  in its index.
- The baseline moved decode times on 10 of the 21 clips (`dts_adjusted`,
  285,212 times on a166d09989f9's whole clip).
- After `perf-me-05` the writer changed only by removing work and in
  what it reports: its per-sample lookups are inlined, window walks stop
  at the range's end, and a clip that ends where a window does between
  two video ticks reports that end exactly (second review). The
  comparison with `main` above and `check-me-02` are from the final
  writer.

**Fixed before `perf-09` and `perf-me-03`.**
- **Baseline.** With an edit list of several edits, libavformat lists
  some samples twice at edit boundaries, and their decode times go
  backwards. The MP4 muxer then rejects the packet, so `baseline/clip.c`
  failed 10 of the six files' 21 clips in `perf-me-02`. It now moves such
  a decode time to one past the previous one, which is what the ffmpeg
  command line reports doing ("Non-monotonic DTS; previous: P, current:
  C; changing to P+1"). The count is reported as `dts_adjusted`. Clips
  whose decode times increase are unchanged.
- **Superseded runs.**
  - `perf-07` and `perf-me-01` downloaded nothing: the R2 credential
    had lost its keys.
  - `perf-08` (stopped after 147 files) and `perf-me-02` measured the
    writer before its windows were made compact, and `perf-me-02` the old
    baseline.
  - The final numbers are `perf-09` and `perf-me-03`, both from one
    binary.
- **After review, `perf-me-04`.** The review of an earlier change found two defects
  in clips that start or end inside a gap between windows. Their fixes
  and the memory change (the windows read from the track's own entries,
  not copied) change the writer for sources with several edits only. So
  the six files are measured again (`perf-me-04`, the same command as
  `perf-me-03`), and the paths without several edits are compared with
  `main` on one machine, in place of a second held-out run.
- **`perf-me-05`.** `perf-me-04` showed the tail clips of a166d09989f9
  slower than with the copied windows: each walk started at the first
  window. The walks now start near the time, and the cursor keeps its
  window, which is measured again as `perf-me-05` (the same command).
  The speed and memory results above are from it.

## Re-measured on current `main`

Memory alone, on a new draw of the corpus, against the memory bar as it
stands.

- **Why a new set.** `perf-09` had 40 of 787 v1 held-out clips over the
  bound, measured before an earlier change bounded the cut-point candidate queue.
- **Draw.** `python3 -I make_inputs.py --fresh-holdout coreav-clip-memory-216`
  (`evidence/inputs-holdout-coreav-clip-memory-216.json`): 265 files, by
  the held-out draw rule (30% of each `real` category)
  with that salt.
  It is a new stratified sample, not a set no run has seen: every corpus
  file has been through correctness runs. 193 of the files are v1
  development files, among them 5de2646e4ea0, one of the six files an earlier change's
  fix was measured on; the other 72 are v1 held-out files, measured in
  `perf-09`. The bar is a fixed bound per clip, not a ratio tuned on a
  set, so the draw only needs to be broad and not chosen by hand.
- **Run.** `memory.py` runs `cbench-mem` on every clip of every file, on
  the development machine: the counting allocator counts the same bytes on
  any machine. `memory.py --summary mem-216` writes the evidence.
- **Bar.** Every clip within the output tables plus
  the 64 KiB window.
- **Result (`main` at 1114aa6): not met by three clips.** 822 of 825
  clips of the 265 files are within the bound (`evidence/mem-216.json.gz`,
  `summary-mem-216.json`). No file or clip failed to measure. 18 clips
  were not cut, by the writer's own refusals (16 `hev1`, 2 with no video
  in range), and 10 encrypted files have no video track the allowlist
  keeps, so no clip. An earlier change's fix holds: no clip has the candidate queue's
  growth (the closest has 1,005 bytes to spare).
- **The three over** are the head, middle and tail clips of a2d42b189482,
  398, 410 and 434 bytes over (in `perf-09`, 414 and 426 for the head and
  middle, and 5,614 for the tail, which had the queue's growth too). It is
  a separate cause: the cut kept the video sample entry's bytes (2,230)
  through the copy, outside the bound. With its fix, all four clips of that file are within the bound, measured
  again with `memory.py` on `main` at b8e0bb2
  (`evidence/mem-216-a2d42b189482-b8e0bb2.json`): head 66,548 against
  68,380, middle 66,784 against 68,604, tail 66,740 against 68,536, whole
  271,040 against 275,432. That closes an earlier change by its second
  branch: every clip still over the bound has an issue of its own, now
  fixed.

## Method

### Inputs

- `evidence/inputs.json` (`make_inputs.py`): every `set: real` file of the
  corpus v1 manifest with extension mp4, mov, m4v or 3gp and a
  `v:h264`, `v:hevc`, `v:av1` or `v:vp9` tag: **869 files** (254 held out,
  146.5 GB). Files whose video the writer refuses stay in the list and are
  reported as refused.
- Correctness uses every file; speed, memory and remote use the held-out
  files only. Development runs used non-held-out files only.
- Media come from the private corpus bucket, every object verified by
  SHA-256, and are deleted after use. No media is committed; files are
  named by SHA-256.

### Engines and drivers

| Program | What it does |
|---|---|
| `cbench clips SRC OUTDIR` | cuts the corpus clips through `clip_mp4` (default limits and allowlist, `BufferedSource(FileSource)` in, 64 KiB `BufWriter` out) and runs the coreav-isobmff round trip on each (`src/lib.rs`) |
| `cbench time ...` | one timed clip (`ours`) or open and probe only (`ours_probe`) |
| `cbench remote ...` | a clip over `RangeReader`, reads served by the parent through a pipe |
| `cbench-mem ...` | the clip under the test-only counting allocator (`src/counting_allocator.rs`, copied unchanged from the audio extraction bench, confined test-only `unsafe`) |
| `baseline/clip.c` | libavformat 7.0.2 (the pinned LGPL package of Spike C) in-process: seek backward to the video keyframe at or before the start, copy the video while its decode time is before the end and the audio from one frame before the first video presentation time while its presentation time is before the end, to MP4 by stream copy; custom 64 KiB I/O on both sides (`lav_avio`, headline) or its own file I/O (`lav_native`); written from the public API documentation only |
| `ffmpeg -ss S [-to E] -i SRC -map V [-map A] -c copy` | the command line, reference only (includes process start-up) |

- **Correctness** (`check.py`, on the development machine: correctness does
  not depend on the machine): per file, `cbench clips`, then
  `scripts/clip_check.py` on every clip (ffprobe streams, durations and
  packet hashes; ffmpeg framemd5 of the video and decoded audio; MediaInfo)
  with the known divergences of `docs/known-divergences-clip.md`, the
  source's decodes shared by its clips. Packets are counted for the packet
  bar.
- **Speed and memory** (`perf.py` on a cloud host, `remote_d31.py`, one container,
  2 CPU, 16 GiB): per held-out file, discovery with `cbench clips`, memory
  with `cbench-mem` on every clip, then per clip one warm-up run of every
  engine and 5 rounds with the engine order rotated; the median of the 5
  is the clip's time, and `ours / lav_avio` per clip is the unit. One file
  at a time is downloaded, measured and deleted, so that no download
  overlaps a timed run. The generated fragmented file (about 2.2 GB,
  lossless H.264 1080p looped by stream copy, AAC, a fragment per 1 s
  keyframe) is measured apart from the held-out numbers.
- **Remote** (`remote.py` on a cloud host): per held-out file, the head and middle
  clips over range reads from R2 (when they need at most 300 requests),
  their exact requests and bytes replayed from a local copy, and a full
  download plus a local clip.
- **Memory parser phase** (`cbench-mem`): probe, two listings of each kept
  track, then the copy pass's listings of both tracks alive at once over
  one shared source with every sample's bytes read through a 64 KiB window,
  as the writer's copy pass does; growth is the clip's peak minus that
  phase's peak, the bound the output's `stts`, `ctts`, `stsc`, `stsz`,
  `stco`/`co64` and `stss` box bytes plus 64 KiB.

### Noisy runs (fixed before `perf-03`)

`perf-02` ran on a machine whose timings were noisy: 135 of its 767 clips
had a `lav_avio` run more than three times as long as another of the same
clip (and above 5 ms), and the same for `ours`, against 3 of 280 in
`perf-01`. The run is kept and reported. It is repeated once, unchanged,
as `perf-03`; the headline speed numbers come from `perf-03` if fewer than
5% of its clips are noisy in that sense, otherwise both runs are reported
and the speed bars must hold in both. Memory is deterministic and comes
from `perf-02`.

The same rule applies to the final round on the final code (`5b7dc4a`):
`perf-04` was noisy in that sense (158 of 763 clips; a us-east machine,
preempted once and resumed). It is kept and reported, and repeated once,
unchanged, as `perf-05`; the final speed numbers come from `perf-05` if
fewer than 5% of its clips are noisy, otherwise both are reported and the
speed bars must hold in both. The final memory numbers come from
`perf-04`.

The corpus check then found a writer fix (commit `913846a`: the kept run
ends before the stream's end), so the final code is `5998d7f`, measured
as `perf-06` under the same rule: its speed and memory numbers are final
if fewer than 5% of its clips are noisy; otherwise it is repeated once,
unchanged, as `perf-07`, both are reported and the bars must hold in
both.

### Oracle failures during the correctness run (fixed before `check-04b`)

During `check-04` (writer `5998d7f`), `ffmpeg` aborted (`Assertion pkt
failed`) while decoding a held-out source whose last video sample lies
exactly at its edit's end: frame-threaded AV1 decoding aborts on such a
discarded last packet, so none of that file's clips could be compared.
The harness now decodes again with one thread after a decode killed by a
signal (commit `fff1ed9`; one thread gives the same frames). Every file of
`check-04` whose record holds a harness failure is checked again,
unchanged otherwise, as `check-04b`, with the same writer build; its
records replace those files' `check-04` records in the result, and the
number of such files is reported. No other file is checked again.

Amended, before `check-04b` ran: two more known divergences were found on
development files while `check-04` ran (`KD-C-PTS-SHIFT` and
`KD-C-SYNC-FLAGS`, commit `6a28265`; no held-out file was looked at to
write them). So `check-04b` checks again every file of `check-04` with a
failing clip, held out or not, with the checker of that commit and the
same writer build (`cbench` of `5998d7f`). Both results are reported:
`check-04` as it ran, and with those files' records replaced.

Committing those entries while `check-04` ran broke it: its workers kept
the checker they had loaded but read the known-divergence list afresh for
each file, found predicates they did not have, and recorded a harness
`KeyError` for every clip of the next 56 files. Those 56 records say
nothing about the writer; they were set aside and the run resumed with the
checker of commit `6a28265`, which checked those files again. So the files
of `check-04` were checked by two versions of the checker, which differ
only in the known divergences they cover and in the one-thread retry;
`check-04b` then checks every file with a failing clip with the later one.

### After merging `main` (fixed before `check-04c`)

Merging `main` brought a parser change: an AAC track's
channel count now comes from its `AudioSpecificConfig` where the sample
entry disagrees, and the clip's `mp4a` entry copies that count. Nothing
else in the clip changes. A scan of the movie box of every corpus file
with AAC audio (`evidence/aac-channel-scan.json`, 443 files) finds one
whose count changes: `9e3a1adccd…`, a 5.1 configuration under a stereo
entry. That file alone is checked again as `check-04c`, with `cbench`
built at the merge commit `8495a0f`, and its record replaces the earlier
one. No other file is checked again.

## Reproduce

```sh
python3 -I make_inputs.py
cargo build --release                       # cbench, cbench-mem
# Correctness, on any machine with the pinned black boxes:
CLIP_CHECK_THREADS=2 python3 -I check.py --inputs evidence/inputs.json \
  --out evidence/runs/check-04 --work /tmp/cbwork --bin /opt/coreav-oracle-bin \
  --cbench target/release/cbench --workers 4
# Speed, memory and remote runs used a cloud host (launcher and poll scripts not published).
```

## Clip speed across earlier changes (development files, one machine)

A check that the cut point work did not slow clips down
(`evidence/commit-speed-185/`): `speed.py` times clips of the 24
non-held-out inputs with the lowest SHA-256 among those of at most 150 MB
(69 clips of 21 files; the other 3 are refused), with `cbench time` built
at five commits (`commits.txt`). Per clip: one warm-up of every build, then
5 rounds with the build order rotated; a build's time is its median; the
unit is the per-clip ratio, with a bootstrap 95% interval. Run twice,
on the development machine.

| Ratio of medians | Run 1 | Run 2 |
|---|---|---|
| An earlier change against the commit before it | 1.003 [0.991, 1.010] | 0.994 [0.974, 1.007] |
| Change A against change B | 1.020 [1.010, 1.037] | 1.010 [0.988, 1.026] |
| main (`7574fa2f`) against an earlier change | 1.030 [1.008, 1.043] | 1.020 [0.996, 1.038] |
| main against the commit before an earlier change | 1.045 [1.021, 1.093] | 1.022 [1.002, 1.051] |
| An earlier change's fix against main | **0.961** [0.926, 0.976] | **0.962** [0.935, 0.990] |
| Change A's fix against the commit before change B | **0.995** [0.981, 1.005] | **0.998** [0.973, 1.018] |

Main had become 2 to 4.5% slower than before the cut point check; with the
start candidates kept as 24-byte entries clips are back at the time
they took before it.
