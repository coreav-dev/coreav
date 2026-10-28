# `coreav-mpegts` against `ffprobe`

Results of the corpus run whose pass bars `docs/design/mpegts.md` fixed
before it, with the design note in change A (`cdcd6e4`), part 1 of change B, where
no real file had yet been compared. Counts only; per-file captures stay outside git.

## Method

- Binaries: the pinned `ffprobe` 7.0.2 and MediaInfo 26.10, and
  `tools/ts-adapter` built from the commit that adds this file.
- `tools/stage3-report/corpus.py run --family mpegts` downloads each file
  from R2, checks its SHA-256, captures the `streams` and `packets` levels
  from both sides, compares them with `coreav-oracle compare`, applies
  `scripts/oracle_kd_ts.py` (`docs/known-divergences-ts.md`), runs
  `scripts/compat_check.py` on the file, captures MediaInfo, and deletes
  the download.
- Inputs: the `real` transport streams of the v1-ts-audio corpus
  (development split 40, held-out split 17; its README fixed the rules and
  the holdout before collection).
- The held-out split was compared once, with the final binary, after the
  last code change and the two reviews. An earlier capture, made while the
  development split was still being worked on, was discarded before it was
  read, and no change was made in response to it; the run reported here is
  the only one that was read.

## Results

Against `ffprobe` 7.0.2, after the documented known divergences
(`docs/known-divergences-ts.md`, each with a predicate checked against the
file by a reader written apart from CoreAV's). "Direct" agrees field for
field; "via KD" differs only in ways a documented divergence covers and
checks.

| Set | Files | Streams: files agreeing (direct + via KD) | Packets agreeing (direct + via KD) | Crashes, panics, timeouts |
|---|---|---|---|---|
| Development split | 40 | **40/40, 100%** (14 + 26) | **180,712/181,631, 99.494%** (148,128 + 32,584) | 0 |
| Held-out split | 17 | 15/17, 88.24% (6 + 9) | 44,752/50,788, 88.12% (18,138 + 26,614) | 0 |
| **Real files together** | **57** | **55/57, 96.49%** (20 + 35) | **225,464/232,419, 97.01%** (166,266 + 59,198) | 0 |
| Edge (robustness only) | 55 | not compared | not compared | 0; 4 files rejected with `coreav.error/1` (no program tables) |

**The bars are met on the development split and missed on the held-out
split and on the real files together.** The bars are at least 98% of files
and 99% of packets, on input 2 as a whole and on its holdout split alone;
the holdout gives 88.24% and 88.12%, and the two together 96.49% and
97.01%. `--ffprobe-compat` is 26 keys on 40 of 40 development files and on
40 of 57 together: the three held-out files below carry every key that
differs.

The holdout was compared once and is now spent: a later run of these 17
files, after a change made in answer to what they showed, is no longer an
estimate of files the reader has not seen. The three files are described
below so that the work they name can be done on the development split and
on files collected afresh.

## Known divergences used

Files at the streams level, packets in `ffprobe`'s packet unit.

| Entry | Development | Held-out |
|---|---:|---:|
| `KD-TS-END-ESTIMATE` (files) | 19 | 8 |
| `KD-TS-STREAM-TIMING` (files) | 15 | 5 |
| `KD-TS-AAC-PROFILE` (files) | 2 | 1 |
| `KD-TS-STREAM-ORDER` (files) | 1 | 1 |
| `KD-TS-TELETEXT-SIZE` (files) | 1 | 0 |
| `KD-TS-STREAM-ORDER` (packets) | 21,132 | 25,526 |
| `KD-TS-FRAME-TIME` (packets) | 9,404 | 151 |
| `KD-TS-POSITION` (packets) | 2,048 | 937 |

`tools/oracle/kd_audit.py --overlay ts` injects 2,845 bug-shaped changes
into CoreAV's captures of the 39 fixtures; 2,777 are caught, 68 change
nothing a gating field sees, and none is covered
(`docs/measurements/kd-audit.md`).

## Uncovered disagreements

**Development split: 919 packets of 181,631 (0.51%), no file.**

- **917** are the decoding timestamps of H.264 pictures whose PES packet
  states a presentation time and no decoding time. CoreAV takes the
  presentation time, as the design note says; `ffprobe` reconstructs an
  earlier one from the picture order, which means reading the slice
  headers. Six captures.
- **1** packet of the multi-program capture still differs under the stream
  order: the first packet of one teletext stream, which `ffprobe` gives no
  timestamp at all.
- **1** packet of a 31 MB capture whose single stream carries bytes that
  are not PES packets: `ffprobe` lists one 46-byte packet near the end of
  the file and CoreAV none.

**Held-out split: 2 files and 6,036 packets of 50,788 (11.88%), in three
files.**

- `6169159c…` (`.ts`, four programs, 32 streams, ARIB): the stream order
  entry does not reach it. 5,754 packets and every stream field from the
  twelfth stream on, which is why it carries 13 of the 13 compat keys that
  differ. The entry needs every gating field of every stream to agree or
  be covered under the permutation; here some do not.
- `b2a193ca…` (`.m2ts`, one program, H.264 and AC-3): the probe never
  reads the video stream's parameter sets, so its `width`, `height`,
  `profile` and `level` are absent where `ffprobe` has them, and the
  file's duration with them. Its packet listing has one unit.
- `d35039e8…` (`.ts`, AAC and H.264): 282 packets, read at the time as
  the same H.264 decoding timestamps as the development split's 917.
  Part 4 found otherwise: they are the file's sound, whose PES packets
  state a decoding time CoreAV dropped. Its H.264 stream agrees field
  for field.

Two of the three are work the development split does not contain; the
third is the one cause the development split also leaves open.

## After the causes the holdout showed 

The three causes the held-out split showed are fixed or documented
(`docs/design/mpegts.md`, `docs/known-divergences-ts.md`). The
development split, re-run with the binary that carries them:

| Set | Files | Streams: files agreeing (direct + via KD) | Packets agreeing (direct + via KD) | Crashes |
|---|---|---|---|---|
| Development split, part 3 | 40 | 40/40, 100% (14 + 26) | 180,712/181,631, 99.49% (148,128 + 32,584) | 0 |
| Development split, now | 40 | 40/40, 100% (14 + 26) | 181,629/181,631, 99.9989% (148,128 + 33,501) | 0 |

**Read that second row with the first.** Direct agreement did not move:
148,128 packets before and 148,128 after, the same packets. The whole
difference is `KD-TS-DECODE-TIME`, the new divergence entry, which
covers 917 packets that were uncovered before; the code this part
changes fixed nothing the development split shows, because the four
reader bugs behind it are the ones the four-program capture and the
camera's `.m2ts` show and this split holds neither. What the fixes are
worth is not measured here and is not measured on the spent holdout
either; it is for the next holdout to say.

`--ffprobe-compat` is 26 keys clean on 40 of 40, as it was. The two
packets left are
the ones part 3 reported: the first packet of a teletext stream of a
multi-program capture, which `ffprobe` gives no timestamp, and one
46-byte packet `ffprobe` lists near the end of a capture whose single
stream carries bytes that are not PES packets.

Known divergences used, development split: `KD-TS-STREAM-ORDER` 21,132
packets and 1 file, `KD-TS-FRAME-TIME` 9,404, `KD-TS-POSITION` 2,048,
`KD-TS-DECODE-TIME` 917 (new in this part); at the streams level
`KD-TS-END-ESTIMATE` 19 files, `KD-TS-STREAM-TIMING` 15,
`KD-TS-AAC-PROFILE` 2, `KD-TS-TELETEXT-SIZE` 1. The audit injects 3,415
bug-shaped changes into CoreAV's captures of the 45 fixtures: 3,331
caught, 84 change nothing a gating field sees, none covered.

**The bars are not judged on these numbers.** They are judged on a
holdout, and the v1-ts-audio corpus's is spent: it was compared once, and
every change since was made in answer to what it showed. The next
measurement waits for a holdout the reader has not seen.

What the three held-out files showed, and what became of it. These
three files were read again with the new binary **to diagnose the
causes, not to measure**: the split was spent before this part began,
and every count below is a re-run of files the reader has since been
changed in answer to. They are evidence of nothing, and no bar, agreement
rate or compat result of this project rests on them.

- `6169159c…` (four programs, 32 streams, ARIB): the one-segment caption
  stream is now named, the one-segment AAC's parameters are read, and
  the pictures of its runs are placed where the file has them.
  `KD-TS-STREAM-ORDER` now reaches a file whose list carries a PID no
  table declares; the fixtures `ts_undeclared_pid.ts` and
  `ts_undeclared_first.ts` hold that shape. Two causes this file alone shows are open and
  not yet entries: `ffprobe` lists 680 of the file's 766 Event
  Information Table sections, by no rule this project could derive from
  the file (the sections CoreAV lists are the file's own, each with a
  valid CRC-32, by a reader written apart from it); and the end of an
  audio stream whose last PES packet holds several frames, where CoreAV
  adds the duration of the one the file times and `ffprobe` the whole
  packet's.
- `b2a193ca…` (`.m2ts`, one access unit): the probe reads the parameter
  sets of a PES packet only the end of the file completes, and the
  stream's end comes from the clock its sequence parameter set states.
  The fixture `ts_one_pes.ts` holds the same structure and is where that
  is measured.
- `d35039e8…`: the stated decoding time of its sound is kept, and the
  frames the file does not time are covered by `KD-TS-FRAME-TIME`. The
  fixture `ts_audio_dts.ts` holds the same structure.

## The replacement holdout: draw 5 of v2-heldout

The v1-ts-audio holdout is spent (above). Draw 5 of the v2-heldout corpus
replaces it: 37 `real` transport streams collected
afresh under rules fixed before the draw ([`docs/corpus.md`](../corpus.md#draw-5)), and a bar of
at least 30 files that `docs/design/mpegts.md` named, before the first
file was fetched. None of them is a file, a URL or a structural signature
v1-ts-audio or an earlier draw of this set already holds. Every
one is held out; it was opened once, with the binary of this commit, and
the run below is the only one that was read.

`tools/stage3-report/corpus.py run --family mpegts-v2`, method and
binaries as above, with two differences. The bytes came from the
collection's staging directory by `--local` rather than from R2, because
the draw was not yet uploaded when it was read; the check is the same
one the download makes, the SHA-256 the manifest names. And the run was
made in two invocations of the same binary, nine seconds apart: two
files to check the new family, then the remaining 35. Nothing was
changed between them -- `runs.jsonl` records one adapter SHA-256,
`5e79d602507661bc`, for both, and it is the adapter of this commit. The
run log marks the tree dirty because the harness and the manifest were
not yet committed when it ran; no Rust changed in this pull request.

| Split | Files | Streams: files agreeing (direct + via KD) | Packets agreeing (direct + via KD) | Crashes, panics, timeouts |
|---|---|---|---|---|
| The replacement holdout | 37 | 30/37, 81.08% (16 + 14) | 184,838/250,180, 73.88% (173,382 + 11,456) | 0 |

**All three bars that apply here are missed.** The oracle bars are at
least 98% of files and 99% of packets; this gives 81.08% and 73.88%.
The `--ffprobe-compat` bar is every key equal on every file, and 21 of
its 26 keys differ on between 1 and 7 of the 37 -- `streams.duration`
and `streams.duration_ts` on 7, the stream identity keys
(`codec_name`, `codec_tag`, `codec_type`, `id`, `width`, `height`) on 4,
the rest on 3 or fewer. It rejects nothing unexplained on any of the 37:
what differs is values, not whether the file is read.

Per class (the collection's own, from what each file shows):

| Class | Files | Streams: files agreeing | Packets agreeing | Files with nothing uncovered |
|---|---:|---|---|---:|
| `group-g` | 20 | 20/20, 100% | 101,259/101,260, 100.00% | 19 |
| `group-b` | 5 | 3/5, 60% | 21,917/25,030, 87.56% | 3 |
| `group-h` | 12 | 7/12, 58.3% | 61,662/123,890, 49.77% | 7 |

**The result is a class result, not a spread one.** On streaming
segments the reader is at parity: 20 files of 20, and one packet of
101,260 differs. What it does not read is the large multi-programme
group-h multiplex -- a shape the spent holdout held two of and this
one holds twelve. Three multiplexes of 105, 135 and 184 MB carry 61,230
of the 65,342 uncovered packet units between them, because a stream
listed in the wrong place makes every packet of that stream a
disagreement.

The eight files that disagree, and what each shows:

- `71cf2bb5…` (135 MB, 15 streams) and `24143de8…` (184 MB, 62
  streams), both off-air multiplexes: CoreAV's stream list is an exact
  permutation of `ffprobe`'s -- the same streams, the same codecs, the
  same programme ids, in another order. `KD-TS-STREAM-ORDER` is written
  for exactly this and does not fire on either. Its predicate was
  settled against one 4-programme ARIB capture (part 4 above) and is
  too narrow for these; widening it is the single largest item here,
  worth 34,074 of the uncovered units by itself.
- `f2edb3b5…` (105 MB, 18 streams): the same permutation, except for
  one elementary stream `ffprobe` calls `mp3` and CoreAV does not type
  at all, which keeps even the widened predicate from matching.
- `eb26df92…` (26 MB, Blu-ray `m2ts`): CoreAV lists 32 streams where
  `ffprobe` lists 33, and types the file's TrueHD track as nothing.
- `6a5fa1d7…` and `cd85d3f0…` (off-air captures): the presentation time
  of the first packet of some streams, and the end estimate that
  follows from it; 953 and 45 units.
- `4e37f7f0…` (AVCHD `mts`): CoreAV does not type the file's PGS
  subtitle stream, which is why it misses the streams bar, and 210
  packets of that stream differ in their key-frame flag.
- `31eb0e2e…` (a 0.5 MB segment): one packet's presentation time, of
  121.

None of this is fixed here. The holdout is opened once and this is that
once; the work it names belongs to the development split and to files
collected after it, and an earlier change stays open.

## Speed

`docs/spikes/parity-bench/`, the method of its README: one file at a time,
sources in the page cache; per file one warm-up run of every engine, then
5 rounds with the engines' order rotated; a file's time per engine is the
median of its 5 rounds; ratios are per file; the median of the per-file
ratios with a bootstrap 95% interval (10,000 resamples, seed 89). The
machine is the development machine, 8 vCPU Intel Xeon at 2.90 GHz, otherwise
idle. The bars are on the held-out files, overall and per class, a class of
at least 5 held-out files gating and a smaller one reported.

| Operation | Against | Held-out median | 95% CI | Faster | Bar |
|---|---|---:|---|---:|---|
| `coreav probe` | `ffprobe -v error -of json -show_format -show_streams` | **0.229** | 0.172 to 0.339 | 17/17 (100%) | at most 1.00: **met**, faster |
| `coreav probe --ffprobe-compat` | the same | **0.231** | 0.186 to 0.338 | 17/17 (100%) | at most 1.00: **met**, faster |
| The packet listing, in process (`pbench packets-ts`) | libavformat in process (`baseline/lav_ts.c`: open, find stream info, read every packet) | **0.386** | 0.197 to 0.663 | 15/17 (88%) | at most 1.00: **met**, faster |

Each upper bound is below 1.00, so each is a claim that CoreAV is faster.

Per class, held-out (`group-g` and `group-b` gate, `group-h` has 2 held-out
files and is reported):

| Operation | group-g (8) | group-b (7) | group-h (2) |
|---|---|---|---|
| `coreav probe` | 0.315, 8/8 faster | 0.120, 7/7 | 0.311, 2/2 |
| `--ffprobe-compat` | 0.296, 8/8 | 0.111, 7/7 | 0.250, 2/2 |
| Packet listing | 0.328, 8/8 | 0.518, 6/7 | 0.875, 1/2 |

Median times per held-out file: `coreav probe` 4.17 ms against `ffprobe`
18.06 ms; the packet listing 5.10 ms against libavformat 15.53 ms and
`ffprobe -show_packets` 33.45 ms. References, not bars: the probe is 0.409
of `lav_info` (libavformat's `avformat_open_input` with
`avformat_find_stream_info`, the comparable in-process work) and 21.4
times `lav_open` alone, which reads no stream parameters; in process
without the command's start-up (`pbench probe-ts`) it is 1.77 times the
whole command, that is the command is mostly start-up.

On all 57 `real` files (a reference, not a bar): `coreav probe` 0.229
(56/57 faster), `--ffprobe-compat` 0.231 (56/57), the packet listing 0.431
(49/57). The packet listing's `group-h` class there is 1.09 on 7 files
(3 faster): its slowest two are a 31 MB capture whose single data stream
is one 31 MB access unit, and a 50 MB multi-program capture -- both spend
their time reassembling PES packets and reading the file, not parsing.

### Speed on the replacement holdout

The same method, on the 37 `real` files of draw 5, all held out
(`docs/spikes/parity-bench/evidence/probe-ts-v2.json`,
`probe-compat-ts-v2.json`, `packets-ts-v2.json`; inputs selected with
`select_ts.py` from the v2 held-out manifest, draw 5).

| Operation | Against | Median | 95% CI | Faster | Bar |
|---|---|---:|---|---:|---|
| `coreav probe` | `ffprobe -v error -of json -show_format -show_streams` | **0.238** | 0.165 to 0.365 | 36/37 (97%) | at most 1.00: **met**, faster |
| `coreav probe --ffprobe-compat` | the same | **0.220** | 0.176 to 0.360 | 36/37 (97%) | at most 1.00: **met**, faster |
| The packet listing, in process (`pbench packets-ts`) | libavformat in process (`baseline/lav_ts.c`) | **0.566** | 0.343 to 0.817 | 33/37 (89%) | at most 1.00: **met**, faster |

Each upper bound is below 1.00, so each is a claim that CoreAV is faster. Median times: `coreav probe` 5.57 ms against `ffprobe`
16.15 ms; the packet listing 18.49 ms against libavformat 26.31 ms and
`ffprobe -show_packets` 74.12 ms.

Per class, every class gating (each has at least 5 files):

| Operation | group-g (20) | group-b (5) | group-h (12) |
|---|---|---|---|
| `coreav probe` | 0.252, 20/20 faster | 0.074, 5/5 | 0.316, 11/12 |
| `--ffprobe-compat` | 0.253, 20/20 | 0.077, 5/5 | 0.324, 11/12 |
| Packet listing | 0.370, 19/20 | 0.612, 5/5 | 0.887, 9/12 |

Every per-class median is at most 1.00, so every per-class bar is met.
The packet listing's `group-h` interval is 0.359 to **1.019**, whose
upper bound is above 1.00: on that class the bar is met but the claim
that CoreAV is faster is not established. Four files are slower than their
reference on some operation: three off-air captures of 84, 106 and
135 MB and one 196 MB HLS segment, between 1.06 and 1.43 of it on the
packet listing, and the 106 MB capture again at 1.68 on the probe and
1.58 on `--ffprobe-compat`. They
spend their time reassembling PES packets and reading the file.

Across the two holdouts the speed picture does not move: the probe is
0.229 on the spent holdout and 0.238 on this one, the packet listing
0.386 and 0.566, and the share of files faster 100% and 97% for the
probe, 88% and 89% for the packet listing. Speed is not what keeps an earlier change
open.

## Spend

USD 0: the machine is the development machine; corpus downloads from R2 have no
egress fee.
