# Spike C: parsing-layer guard

**Spike C pass criterion: met at group level after the fix. Verdict: PASS. Graduated at the stage gate.**
Paired in-process measurements against libavformat 7.0.2 on the current parser
(run-04, after the correctness fixes) give CoreAV **2.57x** libavformat
throughput over all 38 eligible files (warm, 95% interval 2.52 to 2.61); run-03,
before those fixes, gave 2.75x (2.67 to 2.83), and run-02, before the throughput
fix, 0.21x. Every group is above 1 in run-04: fixtures 1.99x, corpus 1.85x,
one-hour MP4 4.65x, 2.2 GB fragmented MOV 3.13x. Per file, 28 of 33 eligible
fixtures are at or above parity in run-04 (30 in run-03); the three large-frame
ProRes fixtures (`prores_hq`, `prores_4444`, `prores_4444xq`) are below it in
both runs (0.70x to 0.84x in run-04), and `prores_lt` (0.98x) and `pcm_f32le`
(0.95x) are below it with intervals that include 1. The roadmap's memory
bounds are numerically met again in run-04 (whole-process peak heap 1.06 MiB
on the 2.2 GB fragmented file, largest read exactly 1 MiB, 10,000-level nesting
rejected in at most 0.28 ms); the test-only allocator and heap meter
were accepted at the stage gate. The
correctness gate now **passes on the 36 in-scope MP4/MOV fixtures** that judge
the verdict under the proposed correctness scope: streams
36/36 (32 direct, 4 via KD; 35/36 before) and packets 386,598/386,598 (2,409
direct, 384,189 via KD; 386,597/386,598 before). This follows the accepted rule for stream duration
conflicts (declared stream duration, with a warning and both values exposed on
conflict) and two
narrow known divergences, C-STTS-SHORT (`vfr.mp4` durations) and
C-EDIT-DISCARD (one `mov_text` sample outside the edit list that CoreAV emits
as discarded and ffprobe omits). On all 37 fixtures the gate still **fails**
only on `browser.webm` (unsupported container): streams 36/37, packets
386,598/386,749. The first 0.6001%
packet score was dominated by legitimate PCM sample grouping, **not evidence of
a defective compact representation**.

Hypothesis: **H6**, safe parsers meeting oracle criteria. Spike C has no separate
H-number. Its pass criterion is unchanged:

> Pass: at least on par with libavformat; memory within the roadmap's memory bounds.

No additional kill threshold is invented. The human stage-gate decision
is no longer pending: graduated at the stage gate (PASS), which also accepts
the test-only allocator and heap meter. Graduation means production code is written fresh under the hardened gates;
this spike code is not moved into `crates/`. Real-corpus oracle agreement and frame/decode agreement are unmeasured. The
corpus `fragmented.mp4` rejection found by run-02 ("unknown trex track") is
fixed: CoreAV now reports the same 14,256 packets, payload bytes and
position/size/PTS/DTS checksum as libavformat.

## First measurement and revised agreement

All **37 fixtures**, including optional cases, were generated and captured with
the checksum-pinned ffmpeg/ffprobe 7.0.2 oracle. **Zero reference failures**;
no required fixture skipped. Raw `tools/oracle` comparison is retained; the
spike-local known-divergence overlay verifies its direct counts and denominators exactly.
Original captures are hashed, original packet boundaries remain unchanged.

| Measurement | Direct agreement | Agreement via documented KD | Uncovered | Total agreement |
| --- | ---: | ---: | ---: | ---: |
| First measurement: streams | 15/37 (40.5405%) | 0 | 22 | 40.5405% |
| First measurement: presentation packets | 2,321/386,749 (0.6001%) | 0 | 384,428 | 0.6001% |
| Revised: streams | 29/37 (78.3784%) | 2/37 | 6/37 | 31/37 (83.7838%) |
| Revised: presentation packets | 2,409/386,749 (0.6229%) | 384,188/386,749 | 152/386,749 | 386,597/386,749 (99.9607%) |
| Color-range fix, all fixtures: streams | 32/37 (86.4865%) | 2/37 | 3/37 | 34/37 (91.8919%) |
| Color-range fix, all fixtures: presentation packets | 2,409/386,749 (0.6229%) | 384,188/386,749 | 152/386,749 | 386,597/386,749 (99.9607%) |
| Color-range fix, in scope (MP4/MOV, verdict): streams | 32/36 (88.8889%) | 2/36 | 2/36 | 34/36 (94.4444%) |
| Color-range fix, in scope (MP4/MOV, verdict): presentation packets | 2,409/386,598 (0.6231%) | 384,188/386,598 | 1/386,598 | 386,597/386,598 (99.9997%) |
| Edit-window fix and C-FMT-ORIGIN, all fixtures: streams | 32/37 (86.4865%) | 3/37 | 2/37 | 35/37 (94.5946%) |
| Edit-window fix and C-FMT-ORIGIN, all fixtures: presentation packets | 2,409/386,749 (0.6229%) | 384,188/386,749 | 152/386,749 | 386,597/386,749 (99.9607%) |
| Edit-window fix and C-FMT-ORIGIN, in scope (MP4/MOV, verdict): streams | 32/36 (88.8889%) | 3/36 | 1/36 | 35/36 (97.2222%) |
| Edit-window fix and C-FMT-ORIGIN, in scope (MP4/MOV, verdict): presentation packets | 2,409/386,598 (0.6231%) | 384,188/386,598 | 1/386,598 | 386,597/386,598 (99.9997%) |
| Duration rule, C-STTS-SHORT and C-EDIT-DISCARD, all fixtures: streams | 32/37 (86.4865%) | 4/37 | 1/37 | 36/37 (97.2973%) |
| Duration rule, C-STTS-SHORT and C-EDIT-DISCARD, all fixtures: presentation packets | 2,409/386,749 (0.6229%) | 384,189/386,749 | 151/386,749 | 386,598/386,749 (99.9610%) |
| Duration rule, C-STTS-SHORT and C-EDIT-DISCARD, in scope (MP4/MOV, verdict): streams | 32/36 (88.8889%) | 4/36 | 0/36 | **36/36 (100%)** |
| Duration rule, C-STTS-SHORT and C-EDIT-DISCARD, in scope (MP4/MOV, verdict): presentation packets | 2,409/386,598 (0.6231%) | 384,189/386,598 | 0/386,598 | **386,598/386,598 (100%)** |

**Scope.** The Spike C correctness verdict is judged on the 36
MP4/MOV fixtures, the scope the spike had from the start; the decision was
committed before this re-run. All-fixture counts stay above with unchanged
denominators. `browser.webm` is an **unsupported container** (WebM, a stage 5
format): it stays in every all-fixture count as 1 uncovered stream item and 151
uncovered packets, and no known divergence covers it. 100% is required: the
in-scope verdict now passes, the all-fixture count still fails on
`browser.webm` alone.

The color-range re-run (local, pinned ffprobe 7.0.2 verified by hash, no cloud host)
regenerated all fixtures; every fixture except `browser.webm` and every
reference capture are byte-identical to the revised run, and only the
`av1.mp4`, `hevc_hev1.mp4` and `hevc_hvc1.mp4` candidate stream captures
changed. The previous revised evidence is kept in
[evidence/before-color-range](evidence/before-color-range).

The edit-window re-run (local, same pinned and hash-verified ffprobe 7.0.2, no
the cloud host, at commit `adedcde`) followed two commits: the fix that marks samples
outside the edit list, and the C-FMT-ORIGIN definition, committed and pushed
before the run. Every fixture except `browser.webm`, every reference capture
and every candidate stream capture are byte-identical to the color-range run.
Every candidate packet capture changed only by the new `presented` field and
the `discard` value derived from it. `edit_list.mp4` moves from uncovered to
agreement via C-FMT-ORIGIN. `vfr.mp4` stays uncovered on purpose (see below).
The color-range evidence is kept in
[evidence/before-edit-window](evidence/before-edit-window). Throughput, heap
and timing were not re-measured in that run; the run-03 numbers predate this
fix, and run-04 (below) re-measured them on the current parser.

The duration-rule re-run (local, same pinned and hash-verified ffprobe 7.0.2,
no cloud host, at commit `11ae829`) followed three commits, all pushed
before the run: the accepted stream duration rule, its implementation with C-STTS-SHORT, and C-EDIT-DISCARD.
Every fixture except `browser.webm`, every reference capture and every candidate
packet capture are byte-identical to the edit-window run. Each candidate stream
capture changed only by four new stream properties (`mdhd_duration_ticks`,
`stts_duration_ticks`, `media_edit_duration_ticks`, `warnings`); no gating
field changed. Only `vfr.mp4` carries the new `duration conflict` warning. The
raw harness counts are unchanged; `vfr.mp4` moves to agreement via
C-STTS-SHORT and the `multitrack.mp4` packet via C-EDIT-DISCARD. The
edit-window evidence is kept in
[evidence/before-adr-0013](evidence/before-adr-0013). Throughput, heap and
timing were not re-measured in that run; probe now also sums the `stts` entries
once per track. Run-04 (in-process comparison below) and the heap test then
re-measured throughput, heap and timing at commit `369e056`.

The separate media-timeline diagnostic remains **2,410/386,749 (0.6231%)**
direct, both first and revised; it has no KD overlay and is not the presentation
verdict. The first measurement is preserved in [evidence/first-measurement](evidence/first-measurement).
[Raw revised evidence](evidence/agreement.json), [covered agreement and exhaustive
field counts](evidence/covered-agreement.json), [media diagnostic](evidence/media-agreement.json)
and [capture/input hashes](evidence/provenance.json) retain traceability.

Covered packet units: **C-PCM 384,000**, **C-CTTS 50**, **C-STRICT 138**,
**C-EDIT-DISCARD 1** (`multitrack.mp4`, the final `mov_text` sample).
Covered stream items: **C-STRICT 2** (`nested_10000.mp4`, `truncated.mp4`),
**C-FMT-ORIGIN 1** (`edit_list.mp4`, `format_duration_us` only) and
**C-STTS-SHORT 1** (`vfr.mp4`, `duration_ts` and `format_duration_us`).
`nested_10000.mp4` additionally has a covered zero-packet file status, without
adding a fictitious packet denominator. Each compared item must have every
gating mismatch covered; a KD for one field cannot excuse another.

### Root causes by fixture, codec and field

This table covers every fixture with an original or revised gating mismatch.
Packet counts show original disagreement → revised **uncovered** disagreement.
Per-field counts (including duration/discard informational differences), codec,
cause, KD identifier and per-fixture direct/covered/uncovered totals are in the
machine-readable evidence, with [first-measurement field/codec counts](evidence/first-measurement/breakdown.json). Original stream informational fields and field counts
remain in the raw evidence rather than affecting agreement.

| Fixture | Codec(s) | Originally differing gating fields | Cause category / disposition | Packets first → uncovered | Revised result |
| --- | --- | --- | --- | ---: | --- |
| `audio_51.m4a` | aac | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `audio_aac.m4a` | aac | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `audio_mp3.mp4` | mp3 | codec_name, duration_ts | timestamp/edit-list modeling (fixed except VFR); true parser bug (fixed) | 0 → 0 | direct |
| `audio_opus.mp4` | opus | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `av1.mp4` | av1 | color_range | true parser feature gap (fixed av1C) | 0 → 0 | direct (after color-range fix; before: uncovered 1 file/0 packets) |
| `basic.mov` | aac, h264 | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `basic.mp4` | aac, h264 | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `browser.webm` | opus, vp9 | probe_ok | unsupported container (outside the MP4/MOV verdict scope, kept in all-fixture counts) | 151 → 151 | KD 0 file/0 packets; uncovered 1 file/151 packets |
| `edit_list.mp4` | aac, h264 | dts, format_duration_us, pts | timestamp/edit-list modeling (fixed exact rationals); declared presentation duration (C-FMT-ORIGIN) | 88 → 0 | KD 1 file/0 packets (format_duration_us, C-FMT-ORIGIN); uncovered 0 (before the edit-window run: uncovered 1 file) |
| `faststart.mp4` | aac, h264 | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `fragmented.mp4` | aac, h264 | duration_ts, format_duration_us | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `hevc_hev1.mp4` | hevc | color_range | true parser feature gap (fixed hvcC) | 0 → 0 | direct (after color-range fix; before: uncovered 1 file/0 packets) |
| `hevc_hvc1.mp4` | hevc | color_range | true parser feature gap (fixed hvcC) | 0 → 0 | direct (after color-range fix; before: uncovered 1 file/0 packets) |
| `multitrack.mp4` | aac, h264, mov_text | duration_ts, extra | sample outside the edit list, marked not presented and discarded but still emitted (C-EDIT-DISCARD); timestamp/edit-list modeling (fixed except VFR) | 1 → 0 | KD 0 file/1 packet (C-EDIT-DISCARD); uncovered 0 (before the duration-rule run: uncovered 1 packet) |
| `negative_cts.mp4` | h264 | dts | oracle normalization | 50 → 0 | KD 0 file/50 packets; uncovered 0 file/0 packets |
| `nested_10000.mp4` | ? | probe_ok | oracle normalization | 0 → 0 | KD 1 file/0 packets; uncovered 0 file/0 packets |
| `pcm_f32le.mov` | pcm_f32le | extra, size | grouping (one packet per sample) | 96,000 → 0 | KD 0 file/96,000 packets; uncovered 0 file/0 packets |
| `pcm_s16le.mov` | pcm_s16le | extra, size | grouping (one packet per sample) | 96,000 → 0 | KD 0 file/96,000 packets; uncovered 0 file/0 packets |
| `pcm_s16le.mp4` | pcm_s16le | codec_name, extra, size | grouping (one packet per sample); true parser bug (fixed) | 96,000 → 0 | KD 0 file/96,000 packets; uncovered 0 file/0 packets |
| `pcm_s24le.mov` | pcm_s24le | extra, size | grouping (one packet per sample) | 96,000 → 0 | KD 0 file/96,000 packets; uncovered 0 file/0 packets |
| `rotate_ccw180.mp4` | aac, h264 | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `rotate_ccw270.mp4` | aac, h264 | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `rotate_ccw90.mp4` | aac, h264 | duration_ts | timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | direct |
| `truncated.mp4` | aac, h264 | probe_ok | oracle normalization | 138 → 0 | KD 1 file/138 packets; uncovered 0 file/0 packets |
| `vfr.mp4` | h264 | duration_ts, format_duration_us | declared duration versus `stts` sum (C-STTS-SHORT); timestamp/edit-list modeling (fixed except VFR) | 0 → 0 | KD 1 file/0 packets (duration_ts, format_duration_us, C-STTS-SHORT); uncovered 0 (before the duration-rule run: uncovered 1 file) |
| `vp9.mp4` | vp9 | color_range | true parser metadata gap (fixed vpcC) | 0 → 0 | direct |

The other generated fixtures have direct agreement at both gating levels.
Specific diagnoses and limitations:

- **PCM grouping:** four PCM fixtures contribute 384,000 of the original
  384,428 packet disagreements (99.8887%). Constant-size container samples
  remain separate. `compare.py` verifies a complete byte-range tiling, summed
  duration, anchored PTS/DTS, bit depth/channel size and keyframe equality before
  counting C-PCM. It never merges, splits or rewrites parser packets.
- **True bugs/model gaps fixed:** MPEG-4 ESDS object types distinguish MP3 from
  AAC; `pcmC` and QuickTime `wave/enda` model PCM identity/endian/bit depth;
  `vpcC` supplies VP9 color range. Simple empty/media edits retain exact rational
  presentation time (476 ms at 44.1 kHz = 104,958/5 ticks), with integer rounding
  confined to the legacy oracle adapter. Edited media durations exclude priming;
  fragmented duration is measured through descriptor iteration rather than
  reporting initial zero headers. Prior review fixes preserve mixed-file DTS
  and ordinals, share a total-work cap, reject failed/incomplete reference runs,
  and verify both cached oracle binaries.
- **Format duration origin, now a KD:** the edit-list file declares a 2.5 s
  movie including leading empty edits (500 ms video, 476 ms audio). CoreAV
  reports the declared 2,500,000 µs; ffprobe reports 2,023,991 µs, the latest
  stream end minus the earliest stream start. Black-box patching shows ffprobe
  never reads the movie header duration for this value. As an earlier pull request settled
  it, C-FMT-ORIGIN covers exactly this
  difference and nothing else (see [KNOWN_DIVERGENCES.md](KNOWN_DIVERGENCES.md)):
  every stream needs a positive leading empty edit, the declared duration must
  equal the longest edit timeline and the oracle's latest stream end, stream
  durations must agree, and the oracle value must equal that end minus the
  earliest start to the microsecond.
- **VFR stream duration, now a KD under the duration rule:** the VFR container declares
  1.96 s (`mdhd` 25,088 ticks, matching `tkhd`, `mvhd` and the edit), while the
  sample durations in `stts` add up to 1.8 s (23,040 ticks), which ffprobe
  reports. The format duration follows from that. All packet fields agree. The
  specification does not say which value wins; the accepted rule keeps the
  declared value. CoreAV now warns
  (`duration conflict: mdhd 25088 ticks, stts sum 23040 ticks; mdhd kept`) and
  exposes `mdhd_duration_ticks`, `stts_duration_ticks` and
  `media_edit_duration_ticks`. Unsupported edit lists now keep the `mdhd`
  duration instead of `null`; no fixture has one.
  C-STTS-SHORT covers only an oracle stream duration equal to the `stts` sum
  and shorter than CoreAV's declared value, and a format duration that follows from the
  oracle's own stream durations.
- **True parser feature gaps:** HEVC/AV1 codec-bitstream color range was not
  parsed (a feature gap, never a known divergence) and is now fixed. For HEVC,
  `video_full_range_flag` comes from the first SPS in `hvcC`, through
  `profile_tier_level`, scaling lists and inter-predicted short-term reference
  picture sets to the VUI (ITU-T H.265 7.3.2.2, 7.3.7, E.2.1); without a VUI
  signal type the range stays unknown. For AV1 it comes from `color_range` in
  the sequence header OBU in the `av1C` configOBUs (AV1 specification 5.5.2),
  including the full range implied by sRGB 4:4:4. A `colr` box keeps
  precedence. Every read is bit-bounded by the configuration bytes, loop counts
  use the H.265 value ranges, and a malformed record leaves the range unknown
  rather than rejecting the file. Besides the three fixtures, 8 extra black-box
  encodes (HEVC full range, 10-bit, 4:4:4, B-pyramid; AV1 full range, 4:4:4
  10-bit, sRGB, monochrome) match ffprobe. WebM is outside the MP4/MOV spike
  scope but remains in all-fixture counts (151 uncovered packets),
  reported as an unsupported container. It is not excused by
  structural-rejection KD.
- **Sample outside the edit list, fixed in part, now a KD:** the final
  `mov_text` sample of `multitrack.mp4` starts at media time 2,000,000 µs,
  exactly at the end of the only media edit (2,000 ms, `media_time` 0), so the
  edit list does not present it (ISO/IEC 14496-12 edit list semantics, clause
8.6.6 per the research notes; the ISO text was not available to check, so no
wording is quoted). Its payload `0000` is a 16-bit `text-length` of 0, which
3GPP TS 26.245 V19.0.0 clause 5.17 allows as a plain count; that clause says
nothing about empty samples, so the payload is not used as a reason. CoreAV
  used to report it as presented (`discard` false). Each packet now carries
  whether its composition interval `[pts, pts + duration)` overlaps the media
  window `[media_time, media_time + segment_duration)`; a zero-duration sample
  is presented when `media_time <= pts < end`. This applies to every track and
  to leading as well as trailing samples. Samples are still emitted one per
  sample with their mapped timestamps.
  C-EDIT-DISCARD covers ffprobe's omission only for an extra packet that `compare.py` independently finds outside the supported
  media edit and that CoreAV marks `presented: false` and `discard: true`
  (see [KNOWN_DIVERGENCES.md](KNOWN_DIVERGENCES.md)). After the fix every paired
  packet agrees on the informational `discard` flag (10 informational
  mismatches before, all leading AAC priming samples). Black-box checks on 77
  files (4 fixture copies plus 73 patched or remuxed variants) found no sample
  that CoreAV marks presented and ffprobe drops.
- **Why ffprobe keeps some samples outside the edit and drops others
  (black-box, ffprobe 7.0.2):** leading samples before `media_time` are kept
  with the `D` flag in the audio and video tracks tested. For trailing samples after the edit
  end, the deciding factor is whether the track has non-sync samples, not
  whether it is video. With a sync sample table (`stss`, H.264 with P or B
  frames), trailing samples are kept with `D`; without one (every sample is a
  sync sample: audio, `mov_text`, MJPEG, ProRes, all-intra H.264, or H.264 with
  its `stss` renamed to `free`), they are dropped; the one exception seen is
  `vfr.mp4` (B-frames) with its `stss` renamed, where the first trailing sample
  is kept with `D` and the other 7 are dropped. Relabelling the handler of
  an audio track as `vide` did not keep its samples. How many trailing samples
  are kept differs: through the next sync sample in an H.264 file without
  B-frames (31 of 50 read), all 8 in `vfr.mp4` with a shortened edit. That
  extent was not isolated. Two smaller differences affect only the
  informational `discard` flag: ffprobe flags a sample that starts before
  `media_time` but ends after it, which CoreAV counts as presented in part; and
  with a zero `segment_duration` CoreAV marks all samples outside, while
  ffprobe keeps 26 of 33 with `D`.
- **Oracle normalization covered:** signed version-1 CTTS with negative minimum
  preserves the stored decode timeline; C-CTTS covers only an exact DTS shift
  equal to that minimum, with PTS/size/keyframe/position directly equal.
  C-STRICT covers bounded ISO BMFF structural/nesting rejection where the oracle
  accepts malformed data. No arbitrary parser error is covered.

[Human-readable KDs](KNOWN_DIVERGENCES.md) and their [machine-readable twins](known-divergences.json)
use generic codec/container properties and wildcard file patterns. There are no
fixture-name predicates. Tests check documentation IDs, generic patterns,
malformed tilings, timestamp/keyframe changes and unsupported error cases.

## Representation, limits and clean room

This throwaway standalone crate stays outside the workspace. Packed big-endian
sample tables are retained once, timing runs use constant-size cursors, fixed-size
samples need no size vector, and fragment buffers are dropped between fragments.
No expanded per-sample packet array exists in the parser. The JSON adapter streams
output and re-scans per track; its serialization is outside parser timing and
heap measurements. Streams and packet iteration use safe Rust and caller-supplied
`Source`/`Limits`; external references are never opened.

Default limits: 10,000 top-level boxes, depth 32,
1,000,000 total boxes, 1 MiB reads, 16 MiB retained metadata, 256 MiB tables and
128 MiB packet ranges. Arithmetic/ranges/counts and fallible allocations are
checked. Extension boxes share depth/header limits. The configurable total
`sample_work` budget is shared across regular samples, tracks, fragments and
runs, defaulting to 67,108,864 visits; it is a work count, not a timeout.
Unsupported edit shapes/rates yield unknown presentation times; structural
errors conservatively reject. Unknown opaque boxes are not recursively validated.

Only the isolated allocator integration test permits unsafe; library and CLI
forbid it. `serde_json` remains the only direct dependency. No workspace member,
workflow, contributor rules or `bench/**` changed.

Clean-room sources: [Apple QTFF](https://developer.apple.com/documentation/quicktime-file-format),
[W3C ISO BMFF structure](https://www.w3.org/TR/mse-byte-stream-format-isobmff/),
[ISO/IEC 14496-1 public specification](https://ossrs.net/lts/zh-cn/assets/files/ISO_IEC_14496-1-System-2010-d28d1ab7f929c646bb7317698f2386d6.pdf)
(descriptors), [ISO/IEC 23003-5 public sample](https://cdn.standards.iteh.ai/samples/77752/a17f98e0bb664a939b031b6a969995d9/ISO-IEC-23003-5-2020.pdf)
(PCMConfig), [WebM VP9 MP4 binding](https://www.webmproject.org/vp9/mp4/),
ISO/IEC 14496-15 (HEVCDecoderConfigurationRecord), [ITU-T H.265](https://www.itu.int/rec/T-REC-H.265)
(SPS and VUI), the [AV1 bitstream specification](https://aomediacodec.github.io/av1-spec/)
and the [AV1 ISOBMFF binding](https://aomediacodec.github.io/av1-isobmff/) (`av1C`).
Only public specification material and black-box oracle behavior informed code.
No multimedia implementation source was opened or studied, and no history,
old tags or archived prototype were read or ported.

## Heap and throughput measurements

The test-only counting global allocator replaces process RSS as current memory
evidence. [Its contract](ALLOCATOR.md) (accepted at the stage gate) explicitly
identify every unsafe forwarding operation and confinement. The first RSS values
remain archival evidence only: peaks 3,608,576 / 2,584,576 bytes and baseline growth
1,318,912 / 196,608 bytes; they do not establish Rust heap usage.

| Input | Actual bytes / duration | Packets Rust / oracle | Probe live growth | Peak heap growth | Peak live heap | Allocation calls | Packed tables / largest read |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |
| One-hour H.264 MP4 | 5,353,480 / 3,600 s | 90,000 / 90,000 | 865,156 B | 865,164 B | 869,608 B | 47 | 864,140 B / 489,608 B |
| Fragmented ProRes MOV | 2,200,599,074 / 2,362.04 s | 59,051 / 59,051 | 954 B | 1,314 B | 5,758 B | 18,948 | 36 B / 192 B |

These first rows read through an unbuffered `FileSource`. With the bounded
1 MiB read window that the benchmark uses (`BufferedSource`, counted inside the
measurement), the rows are:

| Input | Probe live growth | Peak heap growth | Peak live heap | Allocation calls | Largest read, logical or underlying |
| --- | ---: | ---: | ---: | ---: | --- |
| One-hour H.264 MP4 | 1,354,772 B | 1,354,780 B | 1,359,701 B | 49 | 489,608 B |
| Fragmented ProRes MOV | 66,498 B | 66,858 B | 71,779 B | 18,950 | 65,536 B |

These rows were re-measured on the parser of run-04 (after the correctness
fixes); each value is 8 bytes above the run-03-era measurement (peak growth
1,354,772 B and 66,850 B), with the same allocation calls and largest reads.

Both snapshots show zero live growth after dropping each movie (and the window).
Peak growth is 0.8251 MiB / 0.0013 MiB unbuffered and 1.2920 MiB / 0.0638 MiB
with the window, below the 32 MiB bound;
metadata, tables and reads stay below their configured caps. The fragmented file
is actual media, not sparse padding. Both use default limits. Fragment iteration
allocates small temporary buffers (counted), with no retained file-sized data.
The metric counts requested live Rust heap, excluding allocator arena overhead,
usable-size padding and transient internal realloc buffers. One ignored test runs
with one test thread, using a pre-probe baseline; output is serialized afterward.
Results are **numerically within bounds**; the allocator was accepted at the
stage gate.

[Timing repetitions and pinned binary/file hashes](evidence/measurements.json)
include 15 warm runs on Linux with fixed CPU affinity, uncontrolled frequency and
host contention. Rust measures in-process metadata and descriptor iteration;
ffprobe CLI includes process startup, stream discovery, payload reads and JSON.
No throughput ratio or parity claim follows from these earlier timings; they are
kept as recorded. The in-process comparison below replaces them for the verdict.

## In-process libavformat comparison

libavformat runs in-process only as an isolated black-box benchmark baseline. The
baseline is an isolated C harness that loads pinned LGPL FFmpeg 7.0.2 shared
libraries at run time; no CoreAV crate links FFmpeg. Setup, licensing and the
allocation meter are in [baseline/README.md](baseline/README.md).
Raw paired evidence for run-04 (after
the correctness fixes), run-03 (after
the throughput fix) and run-02 (before), and each run's
status are committed.

All three runs measured all 37 fixtures, the 3 Stage 0 corpus files, the one-hour MP4
and the 2.2 GB fragmented MOV in one cloud container (gVisor, 2 CPUs, one CPU
pinned), 15 paired rounds per file and mode, engine order alternating, with the
same `paired.py`, libavformat harness, fixtures and hashes. Only the CoreAV
binary changed. Run-04 used the same derivative image chain, settings and input
hashes as run-03; its CoreAV binary was built from the current parser (HEVC/AV1
color range, edit-window presentation flag, stream duration conflict warning). Ratio = CoreAV throughput / libavformat throughput; above 1
favors CoreAV. Intervals are 95% paired bootstrap (10,000 resamples, seed 20261013).

### What changed between run-02 and run-03

- **Bounded buffered reads.** `BufferedSource` wraps any `Source` with one read
  window of at most 1 MiB (`MAX_READ`). A fill starts at 64 KiB and doubles while
  access stays forward, so a header probe after a seek reads 64 KiB, not 1 MiB.
  Requests never exceed the window; larger logical reads are split. Payload
  bytes are borrowed from the window (`slice_at`), with no extra copy. The parser
  and the payload reader each own one window, so retained read memory is at
  most 2 MiB.
- **Constant-size sample runs.** `packet_runs` delivers the same packets as
  `packets`, grouped into runs. A run is a set of consecutive samples in one chunk
  (or one `trun` without per-sample fields) that share size, duration,
  composition offset and keyframe flag, in tracks with a fixed `stsz` size and
  no `stss`. A run still describes one packet per sample: `PacketRun::packet(i)`
  returns sample `i` with its own position, size and timestamps, and `packets` is
  now this expansion. Samples are never merged into packets, so the packet model, the oracle adapter output and the C-PCM
  accounting (384,000 covered packet units) are unchanged. Every fixture's
  streams, presentation-packet and media-packet output is byte-identical to the
  previous parser. Runs only let a consumer read a run's payload in one
  slice and avoid per-packet work outside its own loop. The parser checks a run's
  byte range and its first and last timestamps before emitting it.
- **`trex` before `trak`.** ISO/IEC 14496-12 places `mvex` inside `moov`
  without ordering it after the `trak` boxes; the corpus file has `mvex` first.
  `trex` defaults are now collected while `moov` is read and resolved after it.
  An unknown or duplicate `track_ID` is still rejected.
- The timing binary visits every packet's position, size, PTS and DTS as before
  (same checksum on every file), using runs.

### What changed between run-03 and run-04

Only correctness fixes, none aimed at throughput: HEVC and AV1 color range read
from the codec configuration records, the presentation flag for samples outside
the edit list, and the stream duration conflict warning. Every file's
CoreAV and libavformat outcome, packet count, payload bytes and checksum is
identical in run-03 and run-04. Run-to-run host variance is visible in the
unchanged baseline: libavformat's warm median moved from 651.5 to 478.7 ms on the
2.2 GB file, from 79.2 to 93.7 ms on `hour.mp4` and from 1.40 to 0.87 ms on
`prores_hq`, so ratio changes between the runs are not attributed to the fixes.

### Results

| Group | Eligible files run-02 → run-03 → run-04 | Warm ratio run-02 [95%] | Warm ratio run-03 [95%] | Cold-attempt ratio run-03 [95%] | Warm ratio run-04 [95%] | Cold-attempt ratio run-04 [95%] |
| --- | ---: | --- | --- | --- | --- | --- |
| Fixtures | 33/37 → 33/37 → 33/37 | 0.013 [0.013, 0.014] | **2.056** [1.910, 2.156] | 2.131 [2.087, 2.175] | **1.990** [1.968, 2.012] | 1.975 [1.946, 2.001] |
| Fixtures without the 4 PCM files (diagnostic) | 29/37 → 29/37 → 29/37 | 0.966 [0.951, 0.983] | 2.163 [1.983, 2.290] | 2.257 [2.204, 2.311] | 2.156 [2.133, 2.178] | 2.150 [2.126, 2.176] |
| 4 PCM fixtures (diagnostic) | 4 → 4 → 4 | 0.001 [0.001, 0.001] | 1.421 [1.383, 1.461] | 1.413 [1.368, 1.459] | 1.134 [1.091, 1.179] | 1.110 [1.050, 1.158] |
| 6 ProRes fixtures (diagnostic) | 6 → 6 → 6 | 1.251 [1.199, 1.307] | 0.853 [0.674, 0.998] | 0.975 [0.945, 1.002] | 0.894 [0.873, 0.916] | 0.898 [0.881, 0.917] |
| Corpus | 2/3 → 3/3 → 3/3 | 0.364 [0.352, 0.379] | **1.866** [1.813, 1.925] | 1.839 [1.790, 1.896] | **1.849** [1.783, 1.924] | 1.922 [1.876, 1.966] |
| One-hour MP4 | 1/1 | 0.134 [0.127, 0.144] | **4.029** [3.990, 4.074] | 4.109 [4.060, 4.162] | **4.647** [4.485, 4.751] | 4.684 [4.641, 4.728] |
| 2.2 GB fragmented MOV | 1/1 | 1.163 [1.127, 1.207] | **3.830** [3.669, 3.991] | 3.925 [3.691, 4.160] | **3.132** [3.077, 3.186] | 3.206 [3.146, 3.265] |
| All | 37/42 → 38/42 → 38/42 | 0.210 [0.205, 0.215] | **2.748** [2.668, 2.830] | 2.779 [2.679, 2.874] | **2.567** [2.519, 2.613] | 2.646 [2.614, 2.677] |

Diagnostic rows are computed from the same paired rounds with the same
bootstrap; they are not part of the harness summary. Group ratios divide summed
times, so long files dominate. Excluded, never counted as speed:
`browser.webm` (WebM out of scope), `truncated.mp4` (CoreAV rejects it, "invalid
or truncated box size"; libavformat reads it), `multitrack.mp4` (payload totals
differ by 2 bytes, 296 versus 295 packets) and `nested_10000.mp4` (CoreAV rejects
by design), in all three runs except as noted. Corpus `fragmented.mp4`, excluded
in run-02 ("unknown trex track"), is eligible in run-03 and run-04.

| File | Warm median ms CoreAV / libavformat, run-02 | Same, run-03 | Warm ratio run-03 | Same as run-03, run-04 | Warm ratio run-04 | Packets CoreAV / libavformat |
| --- | --- | --- | --- | --- | --- | --- |
| `pcm_s16le.mov` | 888.5 / 1.13 | 0.78 / 1.19 | 1.52 | 0.57 / 0.74 | 1.30 | 96,000 / 94 |
| `pcm_f32le.mov` | not listed | 0.98 / 1.30 | 1.31 | 0.80 / 0.73 | 0.95 | 96,000 / 94 |
| `prores_lt.mov` | not listed | 1.09 / 1.32 | 1.21 | 0.85 / 0.86 | 0.98 | 25 / 25 |
| `prores_hq.mov` | 1.11 / 1.32 | 1.71 / 1.40 | 0.48 | 1.23 / 0.87 | 0.70 | 25 / 25 |
| corpus `small.mp4` (64.7 MB) | 377.1 / 77.7 | 33.7 / 76.1 | 2.30 | 34.8 / 58.2 | 1.69 | 42,264 / 42,264 |
| corpus `large.mp4` (276 MB) | 594.2 / 270.5 | 152.5 / 257.4 | 1.66 | 111.0 / 198.9 | 1.81 | 65,278 / 65,278 |
| corpus `fragmented.mp4` (72.8 MB) | rejected | 24.8 / 64.2 | 2.55 | 19.8 / 46.9 | 2.38 | 14,256 / 14,256 |
| `hour.mp4` | 648.4 / 82.8 | 19.7 / 79.2 | 4.03 | 19.8 / 93.7 | 4.65 | 90,000 / 90,000 |
| `large-fragmented.mov` | 635.6 / 731.0 | 162.1 / 651.5 | 3.83 | 153.5 / 478.7 | 3.13 | 59,051 / 59,051 |

Run-02's gap tracked packet count, not bytes: one positioned read per packet
(about 7 microseconds each under gVisor), and 96,000 single-sample PCM reads
against libavformat's 94 grouped packets. In run-03, 30 of 33 eligible fixtures
are at or above parity. The three large-frame ProRes fixtures (`prores_hq`
0.48x, `prores_4444` 0.85x, `prores_4444xq` 0.83x; 25 packets of about 500 KB)
regressed from run-02. For them the old direct read per packet was already
efficient, and the doubling window adds a copy into a fresh buffer plus page
faults for up to 1 MiB in a process that runs for under 2 ms. `prores_hq` also has
a wide interval (0.26 to 0.85). This is a known limitation; the read window was
not tuned before run-04. In run-04, 28 of 33 eligible fixtures are at or above
parity: the same three ProRes fixtures stay below it with intervals below 1
(`prores_hq` 0.70x [0.66, 0.74], `prores_4444` 0.76x [0.72, 0.79],
`prores_4444xq` 0.84x [0.78, 0.89]), and `prores_lt` 0.98x [0.92, 1.03] and
`pcm_f32le` 0.95x [0.89, 1.02] fall just below 1 with intervals that include
parity (1.21x and 1.31x in run-03). Median open time is lower for CoreAV on
every eligible file in both runs, for example 2.0 versus 12.0 ms on `hour.mp4`
and 0.2 versus 58.9 ms on the 2.2 GB file in run-03 (2.3 versus 13.1 ms and
0.1 versus 74.5 ms in run-04).

Boundaries that differ between the engines:

- I/O mode: run-03 CoreAV reads metadata and payload through two bounded
  `BufferedSource` windows (at most 1 MiB each) over two file handles; run-02
  used one positioned read per packet into a reused 1 MiB buffer. libavformat
  uses its default buffered file protocol via `av_read_frame`.
- Iteration order: CoreAV visits tracks one after another, so an interleaved
  file's payload is read once per track (corpus `large.mp4` reads about
  2.8 times its size from the page cache); libavformat reads in file order.
- Probing: CoreAV parses the box structure with default `Limits`; libavformat
  uses `avformat_open_input` with default format detection and no
  `avformat_find_stream_info` (which may decode). Neither forces a format.
- Packets: CoreAV still reports one packet per sample (PCM 96,000 versus
  libavformat's 94), delivered in runs; subtitle and timecode samples follow
  each engine's own rules; packet output is a checksum only.
- Timers start before open and stop after close, inside each process. Process
  start, dynamic loading, JSON output and RSS reporting are outside the timers.
- Cache: warm runs read the whole file before each engine run. Cold attempts
  call `posix_fadvise(DONTNEED)` and check with `mincore`; under gVisor all
  1,260 attempts in each of run-02, run-03 and run-04 kept every page resident, so **no cold ratio is
  verified**. CPU frequency and host contention are not controlled.

Memory, separate untimed runs with the same payload work:

| File | Peak RSS CoreAV / libavformat, run-02 | Same, run-03 | Same, run-04 | Peak requested heap CoreAV / libavformat, run-02 | Same, run-03 | Same, run-04 |
| --- | --- | --- | --- | --- | --- | --- |
| `basic.mp4` | 5.6 / 85.5 MiB | 5.0 / 85.8 MiB | 4.9 / 85.8 MiB | 1.00 / 0.47 MiB | 0.13 / 0.47 MiB | 0.13 / 0.47 MiB |
| `hour.mp4` | 6.6 / 93.5 MiB | 7.1 / 93.6 MiB | 7.2 / 93.5 MiB | 1.83 / 7.62 MiB | 2.29 / 7.62 MiB | 2.29 / 7.62 MiB |
| `large-fragmented.mov` | 5.8 / 87.6 MiB | 5.9 / 87.6 MiB | 6.0 / 87.7 MiB | 1.00 / 2.24 MiB | 1.06 / 2.24 MiB | 1.06 / 2.24 MiB |
| Maximum over all files | 6.7 / 93.5 MiB | 7.1 / 93.6 MiB | 7.2 / 93.5 MiB | 1.83 / 7.62 MiB | 2.29 / 7.62 MiB | 2.29 / 7.62 MiB |

RSS is whole-process and includes mapped shared libraries (libavformat loads
libavcodec and its dependencies), so it is not parser memory. Heap counts
requested bytes through an `LD_PRELOAD` allocation meter, whole process,
including CoreAV's read windows (run-03) or 1 MiB payload buffer (run-02); every
measurement in all three runs was accepted by its bookkeeping checks. The meter
([HEAP_METER.md](baseline/HEAP_METER.md)) was accepted at the stage
gate.

Against the pass criterion (run-04, current parser; run-03 in parentheses):

- At least on par with libavformat: **met at every group level** (all 2.57x,
  fixtures 1.99x, corpus 1.85x, one-hour MP4 4.65x, 2.2 GB file 3.13x; run-03:
  2.75x, 2.06x, 1.87x, 4.03x, 3.83x; lower 95% bounds above 1 in both runs).
  **Not met for 5 of 33 eligible fixtures** in run-04 (3 in run-03): the three
  large-frame ProRes fixtures, plus `prores_lt` and `pcm_f32le` whose intervals
  include 1. Cold-cache parity is unverified.
- 2.2 GB fragmented file peak growth below 32 MiB: **met**. Whole-process peak
  heap 1,116,118 bytes (1,116,110); Rust-heap peak growth 66,858 bytes
  (66,850) with the window.
- No read above 1 MiB: **met**. Largest CoreAV read is exactly 1,048,576 bytes
  (the window capacity) on the fragmented file in both runs; reads are capped by
  construction.
- 10,000-level nesting rejected below 100 ms: **met**. Rejected with "box depth
  limit" in all 30 runs, at most 0.28 ms (0.74 ms).

Run-01 crashed in Valgrind Massif under gVisor after the first fixture; its
partial output stays in [evidence/in-process/failed-run-01](evidence/in-process/failed-run-01).
Run-02 replaced Massif with the allocation meter and replaced GNU time with a
small fork/exec RSS wrapper, because a child spawned from Python inherits
Python's RSS high-water mark. Run-02 finished its measurement (5.2 minutes);
moving per-file heap JSON afterward failed across devices, so its evidence was
downloaded from the cloud volume. Run-03 (2.3 minutes, the corrected
experiment) completed normally. The local entrypoint now writes each
run to its own directory. Run-04 (1.7 minutes in the container, 1.9 minutes
client-side) re-ran the same experiment on the current parser and completed normally. Its
run record holds the CoreAV binary hash: the experiment source hash (the
harness files) is unchanged from run-03, so the no-retry guard now compares the
source hash together with the binary hash. Cloud settings for every run:
1,800 s timeout, 600 s startup timeout, `retries=0`, one container.

## Reproduction and checks

From the repository root, with `S` naming the spike's directory:

```sh
"$S/check.sh"
"$S/reproduce.sh" /absolute/path/to/work
```

Reproduction verifies both pinned binaries, requires all optional fixtures,
captures streams/presentation packets plus a separate media diagnostic, rejects
any reference failure/incomplete manifest, verifies raw direct counts, writes the
KD overlay, generates both large files, records timings/heap/hashes and only then
publishes evidence. Complete captures/reports/media stay ignored under `work/`.
The heap helper invokes the ignored test with `--release --test-threads=1`.

Local fmt, clippy `-D warnings`, tests and cargo-deny pass for the spike and
workspace. There are 26 Python checks (C-STTS-SHORT and C-EDIT-DISCARD
predicates included), 29 Rust tests (including the stream duration conflict warning,
4,096 regular and 4,096 fragmented deterministic mutations, run/expansion equivalence, run
range checks, `trex` ordering, buffered-read bounds, and HEVC/AV1 color range
with every truncated prefix, `colr` precedence and malformed records, and
edit-window bounds: edit end, edge-crossing and zero-duration samples, empty
edits, zero-length and unsupported edits, and runs split at the edit end), and one separately run
large-file heap test. No new dependency or CI job was required.

Open decisions: large-frame ProRes fixtures below parity with the doubling
window; file-order (interleaved) iteration across tracks; the open questions of the stream duration rule (ISO wording,
fragmented `mehd`, warning tolerance, corpus frequency); real-corpus and
frame-level evidence before any
production correctness claim. This spike is never moved into production as-is.
