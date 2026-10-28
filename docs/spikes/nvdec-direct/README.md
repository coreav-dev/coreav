# Spike B: NVDEC driven directly from Rust (H2)

**Verdict: H2: PASS by stage-gate decision.**
The measured 100% bit-exact criterion is not met on SAODBLK_A/B; the stage gate
accepted that NVDEC divergence narrowly and requires a CPU-fallback or detection
follow-up before release. The verdict authorizes no graduation by itself.
Rust-managed H.264 and H.265 Main headers, POC, reference sets, reference lists
and output order drive NVDEC without the CUVID parser. After pipelining decode,
mapping and conversion (this round), equal-output GPU RGB replay with setup
included is **1.09–1.62× torchcodec on the four H.265 inputs** and
**1.03–2.69× on the five H.264 inputs**: warm medians of 15 alternating paired
trials, every lower interval bound above the unchanged 0.95 floor. The previous
H.265 result, **0.85–0.95×** with one picture at a time, is kept below. Exactness
is unchanged on the pipelined decoder. H.264 is **2,994/2,994**. For H.265,
**8,416/9,415** output frames match the FFmpeg 7.0.2 NV12 oracle exactly: every
frame of **127 of 129** decoded conformance streams (7,155/7,155), the corpus
file (300/300) and three fixtures (960/960). The two remaining streams,
SAODBLK_A/B (1,000 frames, 1 exact), also fail on exactly the same frames when
NVIDIA's own parser drives NVDEC, so the divergence lies in NVDEC rather than the
Rust header path; it still counts against the 100% criterion. A
[diagnostic](#diagnostic-torchcodec-on-saodblk_ab-not-part-of-the-pass-criteria)
finds torchcodec's CUDA output following NVDEC on those same frames, while its CPU
path matches the oracle exactly. Two further
conformance streams are below NVDEC's 144-pixel minimum size and fail explicitly.
Timestamps, an equal-output CUVID baseline, RGB policy, pinned-clock parity
evidence and broader corpus coverage remain open for
graduation; DCO certifies the contribution.

The spike remains confined to its own directory and outside
the Cargo workspace. Its worker frame-transfer measurements (pipe versus shared memory,
below) are recorded in the project's frame-transfer decision.

## Hypothesis and fixed criteria

H2: “NVDEC can be driven directly from Rust header parsing (no CUVID parser),
frame-exact and at throughput parity”.

Spike B's criteria:

- Pass: “frame-exact on the frame-accuracy oracle; throughput within 5% (initial).”
- Kill: “the GPU path uses NVIDIA's parser, documented as a C parser on the path,
  and GPU safety claims are narrowed accordingly.”

The exact-checksum requirement remains **100% of at least 1,000 sampled frames**. The 5% threshold is unchanged from the
stage-gate table.
The H2 verdict does not authorize either graduation or a scope pivot.
H.264 POC type 1, interlacing/MBAFF, MMCO 5, FMO, SP/SI, frame-number gaps,
custom scaling matrices, higher bit depth and chroma formats, and sequence
reconfiguration fail explicitly. H.265 is limited to Main 8-bit 4:2:0
progressive content (scope below). These exclusions are not oracle exemptions.
The 100-file real corpus is not done.

## Implementation and clean room

Throwaway, unpublished Cargo package with its own workspace and lockfile.
Default builds need neither CUDA nor the SDK. `gpu` uses bindgen against the
vendored **public NVIDIA API header**, with the original MIT notice retained,
and dynamically loads `libcuda.so.1` and `libnvcuvid.so.1`. It never loads or
calls `cuvidCreateVideoParser`/`cuvidParseVideoData` or links multimedia libraries.
The header's source URL, checksum and public-spec links are in
[evidence/provenance.json](evidence/provenance.json).

Safe Rust handles Annex B framing, emulation prevention, SPS/PPS, progressive
I/P/B slice headers, weighted-prediction syntax, POC types 0/2, sliding-window
and MMCO 1/2/3/4/6 reference marking, short/long-term references, and reference
list initialization/modification. It fills `CUVIDPICPARAMS` and its H.264 DPB
from that state. The public API has no per-slice list fields: Rust constructs
and checks the lists, while the original slice syntax and Rust-managed DPB are
passed to the decode API. NVDEC's driver and hardware remain outside the safety
claim. Absent scaling matrices infer flat 16 under H.264 8.5.6.

The decoder keeps up to four pictures in flight and maps each one before its
surface is reused, then unmaps it once its recorded work completes (pipelined
round below); exact
validation optionally copies and compacts NV12 rows on CPU. Checksum records
are sorted by IDR group and Rust POC for presentation order. Elementary input
has no container timestamps: **PTS/time-base correctness is not checked**, and
this result is not a pass of the combined stream/packet/timestamp oracle.
The saved `framemd5` checksums, byte sizes and frame counts implement the spike's
pixel-accuracy experiment. No known-divergence or tolerance exemptions exist.

Experimental limits: 512 MiB encoded input, 16 MiB per NAL, 25,000 pictures,
1,024 slices per picture, 4,096×2,304 resolution, 16 references, and bounded
reference-modification/marking loops. Streaming adds 64 MiB/1,024-picture GOP ceilings. These are spike resource ceilings, not
production defaults or a new gating threshold. Production defaults need a
follow-up decision that sets decode defaults from Stage 1 measurements.

Only the ITU H.264 (08/2024) and ITU-T H.265 (V11, 01/2026) specifications and
NVIDIA's public API header/docs were read for decoding implementation. No multimedia implementation source,
git history, old tags or archived prototype was read. FFmpeg, ffprobe, libx265 (through
FFmpeg), PyAV and torchcodec were black-box test tools. Nothing was ported. Safe parsing, DPB and process
control forbid unsafe. The driver boundary and one small shared-memory mapping
module are the spike's unsafe exceptions; generated bindings also contain
unsafe. All need fresh, scoped `unsafe` exceptions before any
production rewrite.

## H.265 Main extension: scope and implementation

This spike was extended to H.265 Main in an earlier pull request. Safe Rust
(`src/hevc.rs`, `src/hevc_dpb.rs`) implements, from ITU-T H.265 (V11, 01/2026)
only:

- NAL unit headers, SPS (profile/tier/level, sub-layer ordering, scaling lists,
  PCM, short-term RPS candidates with inter-RPS prediction, long-term candidates),
  PPS (tiles, deblocking, scaling lists, list modification) and slice segment
  headers through `ref_pic_lists_modification()`; NVDEC parses the rest of each
  slice from the unchanged NAL;
- POC (8.3.1), NoRaslOutputFlag and RASL skipping (8.1.3), RPS derivation and
  reference marking (8.3.2), reference list construction and checks (8.3.4);
- the output order DPB of C.5.2, including bumping, latency counts and
  NoOutputOfPriorPicsFlag (CRA pictures infer 1, also after an end of sequence);
- decode-surface assignment, `CUVIDHEVCPICPARAMS` (RPS index arrays,
  `NumBitsForShortTermRPSInSlice`, tiles, Tables 7-5/7-6 default scaling lists)
  and a `cuvidGetDecoderCaps` size check before session creation.

No CUVID parser symbol is loaded. Scope: Main profile (`general_profile_idc` 1 or
the Main compatibility flag), 8-bit 4:2:0, progressive source flags, at most
4,096×2,304, 16 references and 21 tile columns/rows. Higher layers are ignored as
the specification requires; reserved VCL types, other profiles, bit depths,
chroma formats, interlaced sources and PPS range/multilayer/3D/SCC extensions fail
explicitly. SPS VUI and extensions are not parsed: Main fixes the extension flags
that affect decoding, and colour metadata comes from the container in this
experiment. The validation path recreates the NVDEC session when a new coded
video sequence changes the coded size (RAP_B_Bossen_2); the RGB replay rejects
reconfiguration.

Surfaces are allocated in 16-sample units, matching the macroblock fields of
`CUVIDPICPARAMS` and the H.264 path, and the conformance window is applied through
NVDEC's display area. The first complete run allocated unaligned surfaces; the
final run aligned them before measuring (diagnostics below).

**Streaming presentation order.** NVDEC surfaces are converted to RGB as soon as
they decode, so each picture's presentation rank must be known first. Headers
are parsed ahead and run through the C.5.2 process; a picture is decoded once
bumping fixes its rank or marks it non-output or discarded. This works for open
GOPs, where RASL pictures follow a CRA picture in decoding order. The lookahead is
bounded by the existing 1,024-picture and 64 MiB ceilings, and the evidence
records peak lookahead for every replay. Every rank fits the 32-slot ring because
at most 16 pictures can be decoded ahead of the lowest pending rank; a unit test
checks this on every local H.265 input.

## H.265 inputs and method

- **Conformance:** [hevc-conformance.json](hevc-conformance.json) pins **131**
  ITU-T H.265.1 HEVC_v1 bitstreams (archive and member SHA-256). Selection rule,
  fixed before any GPU run: every HEVC_v1 archive without `MAIN10` or `TSUNEQBD`
  in its name that the spike parses and whose output-frame count matched the
  black-box FFmpeg decoder. Five archives fail that rule and are listed in the
  manifest: PICSIZE_A–D exceed the 4,096×2,304 limit, and FFmpeg reports
  missing parameter sets on VPSSPSPPS_A and outputs 2 of 6 pictures. Only
  bitstreams were extracted; no reference software was read.
- **Corpus:** [hevc-corpus.json](hevc-corpus.json) pins one public H.265 Main
  MP4, 1920×1080, 300 frames, with CRA/RASL open GOPs: Big Buck Bunny,
  **CC BY 3.0, (c) copyright 2008, Blender Foundation / www.bigbuckbunny.org**,
  from test-videos.co.uk. One file is not representative corpus coverage.
- **Fixtures:** `testsrc2` encoded in the container by FFmpeg's libx265
  (`-preset veryfast`, closed 60-frame GOPs, 3 B-frames, 3 references, one thread):
  720 frames at 320×180, 180 at 1080p and 60 at 4K. Hashes are recorded per run.

The oracle is FFmpeg 7.0.2 `framemd5` of every output frame in NV12, single
thread, `-bitexact` and `-flags unaligned`. Without that flag this FFmpeg build
skips the 2-sample left crop of CONFWIN_A_Sony_1 and outputs 414 instead of 412
columns, contrary to the conformance window; the run records both sizes. The
candidate writes records in C.5.2 output order with no sorting. Counts and sizes
must match as well as MD5s. Elementary streams carry no timestamps, so
**PTS/time-base correctness is not checked**.

The equal-output RGB comparison, its timers, retained sessions, fresh-process
cold-init rows and bootstrap intervals are exactly the H.264 retained-session
method described below, applied to the corpus file and fixtures; the corpus
requests all 300 frames. Torchcodec decodes the original MP4s on CUDA. After the
fixed measurement, the run records diagnostics for CONFWIN_A, SAODBLK_A/B and
the AMP_A control: unaligned surfaces, FFmpeg's default cropping, and NVIDIA's
own parser driving NVDEC through PyAV `hevc_cuvid`. These diagnostics are not
part of the pass criteria.

## H.265 results

Complete evidence: [raw record](evidence/hevc/attempt-4.json),
[summary](evidence/hevc/summary.json), [source snapshot](evidence/hevc/sources.json),
and the platform logs (not published).
One L4, driver 610.57.04 (CUDA user-mode driver 13.3), torch 2.14.1+cu130,
torchcodec 0.17.0+cu130; experiment time **307.7 s**.

| Inputs | Streams | Exact / compared output frames |
|---|---:|---:|
| Conformance, decoded | 129 | 7,156 / 8,155 |
| of which all except SAODBLK_A/B | 127 | 7,155 / 7,155 |
| SAODBLK_A_MainConcept_4, SAODBLK_B_MainConcept_4 | 2 | 1 / 1,000 |
| Conformance below NVDEC's minimum size (WPP_D, WPP_E) | 2 | not decoded |
| Corpus (open GOP) | 1 | 300 / 300 |
| libx265 fixtures | 3 | 960 / 960 |
| **Total** | **135** | **8,416 / 9,415** |

Every output count matched, including 23 skipped RASL pictures (NUT_A, RAP_A,
RAP_B, NoOutPrior_A), 26 pictures removed by NoOutputOfPriorPicsFlag (BUMPING_A,
NoOutPrior_A/B, RAP_B) and one coded-size change (RAP_B). The passing set covers
inter-RPS prediction, long-term references, list modification, weighted
prediction, tiles, WPP, dependent slices, scaling lists, PCM, transform skip,
lossless bypass, SAO, deblocking, temporal scalability and pic_output_flag.

**SAODBLK_A/B** (1016×760 coded, SAO and deblocking) mismatch from their first
frames. NVIDIA's parser through PyAV `hevc_cuvid` mismatches on **exactly the same
frame indices** (0/500 and 1/500), and unaligned surfaces give the same result,
while that control matches all frames of CONFWIN_A and AMP_A. The divergence
is therefore in NVDEC hardware or driver behavior on this driver, not in the Rust
header path. The correctness policy requires bit-exact decoding of conformance bitstreams, so
these 999 frames remain uncovered disagreements, not a known divergence. In the
first complete run ([attempt 2](evidence/hevc/attempt-2.json)), CONFWIN_A also
failed only because of the default-crop oracle size; SAODBLK_A/B failed the same
way with unaligned surfaces.

Equal-output GPU RGB replay (16-frame CUDA NCHW uint8 batches, presentation order,
retained sessions; ratios are torchcodec time / direct time, so values above 1
favor direct):

| Input | Frames | Direct setup / decode / total (ms) | torchcodec setup / decode / total (ms) | Decode ratio (mean interval) | Total ratio (mean interval) |
|---|---:|---:|---:|---:|---:|
| corpus (1080p, open GOP) | 300 | 1.762 / 427.180 / 428.942 | 0.016 / 378.369 / 378.385 | 0.895 (0.835–1.013) | 0.891 (0.832–1.004) |
| 320×180 fixture | 720 | 0.142 / 118.680 / 118.823 | 0.014 / 90.384 / 90.397 | 0.854 (0.673–1.261) | 0.853 (0.672–1.260) |
| 1080p fixture | 180 | 0.280 / 132.100 / 132.445 | 0.014 / 117.463 / 117.478 | 0.889 (0.879–1.492) | 0.888 (0.878–1.487) |
| 4K fixture | 60 | 0.637 / 149.301 / 149.937 | 0.019 / 142.384 / 142.401 | 0.954 (0.952–1.516) | 0.950 (0.948–1.505) |

All four total-ratio medians are below 0.95 (the 4K median is 0.9497). In each
input the first of three trials favors direct (1.00–1.50) because torchcodec's
first retrieval includes lazy initialization; the second and third trials do not.
In this round the H.265 path decoded, mapped, converted, synchronized and
unmapped one picture at a time. The pipelined round below replaces it. Attempt 2 gave
0.856–0.940 with unaligned surfaces.

| Input | Direct process cold init (ms) | torchcodec process cold init (ms) | torchcodec/direct cold time (mean interval) |
|---|---:|---:|---:|
| corpus (1080p) | 300.65 | 4455.48 | 15.035 (14.506–19.333) |
| 320×180 fixture | 230.55 | 4392.98 | 19.228 (18.179–20.424) |
| 1080p fixture | 261.49 | 4329.70 | 16.969 (16.558–18.787) |
| 4K fixture | 286.74 | 4343.26 | 15.147 (15.089–17.194) |

Cold rows are construction-ready times as defined for H.264 and include
torchcodec's interpreter and framework imports. Peak lookahead is 7 pictures /
1.33 MiB on the corpus and 6 pictures on fixtures; the corpus replay reads 30.0
MiB. Direct-versus-torchcodec RGB samples: **12/12 within 2/255**, maximum error
**1/255**, **0/12 exact**; no RGB tolerance is adopted.

**H.265 conclusion:** frame-exact driving of NVDEC without the CUVID parser holds
on every tested H.265 frame that NVDEC itself decodes exactly; the kill criterion
is not triggered. The 5% throughput criterion is not met for H.265 in this
design and measurement.

### H.265 runs

All four calls used one L4, retries=0, one container and a hard function timeout.

1. 600 s timeout. A 600-second local command timeout stopped the cloud client
   after 445 s, which stopped the app; no result exists.
2. 900 s timeout. Complete; 2 decoder mismatches and the CONFWIN oracle-crop
   failure, as above.
3. 900 s timeout. The cloud host stopped the app as "local client disconnected" during
   the run (the paused client's clock recorded 0.02 s); no result exists.
4. 480 s timeout, detached launch, function-call ID recorded. Complete;
   the results above.


## Pipelined decode (both codecs): method, recorded before the run

The H.265 replay above decoded, mapped, converted, synchronized and unmapped one
picture at a time, leaving NVDEC idle during each map and conversion. This round
changes the shared H.264/H.265 driver path, following the data flow documented in
the public `cuviddec.h` (decode picture N, then map picture N−4):

- **Pictures in flight.** Up to 4 pictures are submitted to NVDEC before the
  oldest is mapped (`NVDEC_PIPELINE_DEPTH`, default 4; 0 restores the
  one-picture-at-a-time path). Each queued picture keeps its bitstream until it
  is mapped. Both Rust planners assign free decode surfaces round-robin from 24
  surfaces (previously the lowest free of 20). A picture still waiting on a
  surface that is about to be reused is mapped first, and the evidence counts
  these early maps.
- **Separate stream.** Map post-processing (`CUVIDPROCPARAMS.output_stream`),
  RGB conversion and validation copies run on one non-blocking CUDA stream owned
  by the decoder. Up to 8 frames stay mapped. Each frame is unmapped once an
  event recorded after its conversion completes, checked without blocking after
  every decode. A decode surface is not reused until the work recorded after
  its last map has completed.
- **Batch synchronization.** One stream synchronization per 16-frame
  presentation batch replaces the per-picture context synchronization, matching
  torchcodec's per-batch `torch.cuda.synchronize()`. The final batch maps the
  pictures still in flight.

The scheduling decisions (in-flight queue, ring reservations, batch release) are
in safe Rust (`src/pipeline.rs`). A unit test replays all 131 local conformance
streams at depths 0, 1, 4 and 8, delaying each conversion until its picture
leaves the queue, and checks that every requested rank arrives exactly once, in
complete batches and without stalling.

**Exactness checks, unchanged in kind.** The FFmpeg 7.0.2 NV12 `framemd5` oracle
runs on the pipelined decoder (depth 4, round-robin surfaces, deferred maps) for
every input of both earlier rounds: the H.264 set (2,994 frames) and the H.265
set (131 conformance streams, corpus and fixtures). In addition, for each RGB
input, every requested RGB frame from the pipelined path must have the same MD5
as the depth-0 path.

**Paired measurement.** Same cloud function, pinned base image and dependency
files, one L4, FFmpeg 7.0.2 oracle, inputs and equal-output contract
(16-frame CUDA uint8 NCHW batches in presentation order, setup + decode per
trial, retained sessions) as before, for the five H.264 and four H.265 RGB
inputs. Changes, fixed before the run:

- Both backends run as retained worker processes at the same time
  (`rgb-serve`, `torch_rgb.py --serve`), and each trial is one replay request to
  each worker. There are **16 paired trials per input**. Even trials run direct
  first and odd trials run torchcodec first.
- **Trial 0** follows construction and includes torchcodec's lazy first-retrieval
  work. It is reported separately as the cold trial. **Trials 1–15** are the
  warm sample.
- Ratios are torchcodec time / direct time per trial (above 1 favors direct).
  The table reports the warm median with a seeded 5,000-draw percentile
  bootstrap 95% interval of the median, and also the mean interval used before.
- Criterion, unchanged: for each codec, the H2 throughput floor is met only if
  every input's warm setup-inclusive (total) median is at least **0.95**. The
  floor is not changed, and the cold trial does not enter it.
- Fresh-process cold init keeps its earlier method: three processes per backend
  per input in seeded random order.
- Oracle mismatches are recorded as results, not as an incomplete run. They are
  never exempted and still count against the 100% exactness criterion.

## Pipelined decode (both codecs): results

Complete evidence: [raw record](evidence/pipeline/attempt-1.json),
[summary](evidence/pipeline/summary.json), [source snapshot](evidence/pipeline/sources.json),
and the platform log (not published).
One L4, driver 610.57.04 (CUDA user-mode driver 13.3), torch 2.14.1+cu130,
torchcodec 0.17.0+cu130, FFmpeg 7.0.2 oracle; experiment time **611.6 s**. One call, complete
on the first attempt, measuring the binary built from commit `046d648`.

**Exactness is unchanged.** Running on the pipelined decoder at depth 4:

| Inputs | Exact / compared output frames | Before |
|---|---:|---:|
| H.264 (2 conformance, 2 corpus, 3 fixtures) | **2,994 / 2,994** | 2,994 / 2,994 |
| H.265 conformance, all except SAODBLK_A/B (127 streams) | **7,155 / 7,155** | 7,155 / 7,155 |
| H.265 SAODBLK_A/B | 1 / 1,000 | 1 / 1,000 |
| H.265 corpus and fixtures | **1,260 / 1,260** | 1,260 / 1,260 |
| **H.265 total** | **8,416 / 9,415** | 8,416 / 9,415 |

Every output count matches. SAODBLK_A/B mismatch on exactly the same frame
indices as before (500 and 499), which NVIDIA's own parser also mismatches.
WPP_D/E again fail NVDEC's size capability check. On all nine RGB inputs, every
requested RGB frame (4,220 frames) of the pipelined path has the same MD5 as the
depth-0 path. No picture needed an early map because of surface reuse in any
validation or replay. Direct RGB versus torchcodec: **27/27 samples within
2/255**, maximum error **1/255**, **2/27 exact**. No RGB tolerance is adopted.

**Paired replay.** Ratios are torchcodec time / direct time for the
setup-inclusive total (above 1 favors direct). Warm columns cover trials 1–15
and give the median with its 95% bootstrap interval of the median. Millisecond
columns are warm medians.

| Codec / input | Frames | Direct total (ms) | torchcodec total (ms) | Warm total ratio, median (interval) | Mean interval | Cold trial 0 | Previous (3 trials) |
|---|---:|---:|---:|---:|---:|---:|---:|
| H.265 corpus (1080p, open GOP) | 300 | 322.971 | 355.337 | **1.101 (1.095–1.108)** | 1.098–1.109 | 1.314 | 0.891 |
| H.265 320×180 fixture | 720 | 55.136 | 89.373 | **1.617 (1.609–1.657)** | 1.548–1.639 | 2.814 | 0.853 |
| H.265 1080p fixture | 180 | 100.178 | 115.255 | **1.151 (1.149–1.159)** | 1.149–1.183 | 1.846 | 0.888 |
| H.265 4K fixture | 60 | 129.208 | 141.352 | **1.094 (1.089–1.097)** | 1.091–1.096 | 1.604 | 0.950 |
| H.264 small corpus | 1,000 | 91.457 | 246.416 | **2.690 (2.496–2.739)** | 2.564–2.750 | 3.450 | 1.176 |
| H.264 large corpus (1080p) | 1,000 | 1289.561 | 1352.674 | **1.049 (1.047–1.057)** | 1.047–1.054 | 1.085 | 0.961 |
| H.264 320×180 fixture | 720 | 120.299 | 197.372 | **1.640 (1.600–1.685)** | 1.628–1.700 | 2.420 | 1.070 |
| H.264 1080p fixture | 180 | 234.746 | 257.989 | **1.098 (1.096–1.101)** | 1.097–1.101 | 1.359 | 0.999 |
| H.264 4K fixture | 60 | 312.607 | 323.461 | **1.034 (1.033–1.036)** | 1.033–1.040 | 1.280 | 1.040 |

Direct replay setup is 0.13–2.16 ms (warm medians); decode-only ratios are within
0.007 of the totals. The order effect is small: warm medians with direct first and
with torchcodec first differ by at most 0.037 (H.264 small corpus), 0.023 (H.264
320×180) and 0.013 elsewhere. Torchcodec's warm times are close to those of the
previous rounds (for example 355 ms here versus 378 ms on the H.265 corpus). The
change therefore comes mainly from the direct path: H.265 corpus decode fell from
427 ms to 321 ms and the 320×180 fixture from 119 ms to 55 ms. "Previous" is the
setup-inclusive median of three fixed-order trials from the earlier rounds,
with a different host and harness, so it is a methodological comparison rather
than a controlled before/after. Fresh-process cold init is unchanged in kind:
0.20–0.27 s direct versus 4.32–4.55 s for torchcodec.

**Pre-registered criterion.** Every warm total median is at least 0.95, and so is
every lower interval bound: the throughput floor is met for **H.265 (4/4)** and
**H.264 (5/5)** on the tested inputs.

**Run.** One call: 1,200 s timeout, retries=0, one container, launched
detached. The local client's clock recorded only 115.9 s, below the remote
experiment time, so local wall time is not used.

**Conclusion of this round.** Pipelining closes the H.265 gap. With the method
fixed in advance, the throughput half of H2 now holds on all nine tested inputs
of both codecs. H2 as a whole is still not passed: SAODBLK_A/B keep H.265 exactness
below 100%, and the open items in the verdict remain.

## Diagnostic: torchcodec on SAODBLK_A/B (not part of the pass criteria)

### Method, recorded before the run

This diagnostic asks whether torchcodec's CUDA path also disagrees with the oracle
on SAODBLK_A/B, and on which frames. It changes no criterion, oracle or result
above. Script: torchcodec_diagnostic.py, launched by
the cloud launcher script as one L4 call
(600 s timeout, retries=0) on the same pinned base
image, FFmpeg 7.0.2 oracle and GPU requirements as the rounds above. Streams:
SAODBLK_A_MainConcept_4, SAODBLK_B_MainConcept_4, and AMP_A_Samsung_7 as a
control that NVDEC decodes exactly.

- **NV12 oracle:** the H.265 round's command unchanged: FFmpeg 7.0.2 `framemd5`,
  NV12, `-threads 1 -bitexact -flags unaligned`, on the raw `.h265` stream.
- **RGB oracle:** the same FFmpeg 7.0.2 command with `-pix_fmt rgb24 -f rawvideo`,
  every output frame. Torchcodec only returns RGB, so **torchcodec is compared
  in RGB, not NV12**. A torchcodec frame is exact when its HWC RGB bytes equal
  the RGB oracle frame. This is stricter than the README's earlier RGB samples,
  which used ≤2/255 as a supplementary diagnostic.
- **Conversion caveat:** torchcodec's colour conversion is its own black box. The
  CUDA path has never matched FFmpeg's RGB exactly in the rounds above (0/12 and
  2/27 samples exact, maximum error 1/255), so RGB mismatches on CUDA do not by
  themselves show a decoding difference. To separate decoding from conversion, the
  oracle NV12 pictures and NVIDIA-parser NV12 pictures (PyAV `hevc_cuvid`, as in
  the H.265 diagnostics) are both converted to RGB by one FFmpeg 7.0.2
  `rawvideo nv12 → rgb24` command. On pixels where those two conversions differ,
  the mean absolute difference of the torchcodec frame from each is compared, and
  the frame is classed as closer to NVDEC, closer to the oracle, or tied. Frames
  where oracle and NVDEC pictures are identical have no such pixels and are not
  classed.
- **CPU control:** torchcodec with `device='cpu'` (FFmpeg's software decoder as
  bundled with the pinned PyAV wheel, which torchcodec loads; not the 7.0.2 CLI
  build), compared the same way.
- **Container caveat:** torchcodec rejects raw H.265 elementary streams (no
  timestamps). Each stream is therefore stream-copied into MP4 by PyAV, with
  decode-order DTS and output-order PTS obtained by passing decode indices as PTS
  through FFmpeg's software decoder. The MP4 is used only if FFmpeg 7.0.2 gives
  the identical `framemd5` sequence for it as for the raw stream; no sample data
  is re-encoded.
- Recorded per frame: exactness and maximum, mean and >2 counts of the RGB error
  against the RGB oracle; the attribution class; NV12 sample differences between
  oracle and NVDEC. Torchcodec's reported fallback status is recorded to confirm
  that the CUDA path did not fall back to the CPU.

**Refinement before the second call.** In the first call (attempt 1) the common
`rawvideo nv12 → rgb24` conversion did not reproduce the RGB oracle (up to 33/255
apart on the control), because FFmpeg converts NV12 input differently from the
decoder's own yuv420p output. The CPU control, RGB-exact on every frame, was then
classed "closer to NVDEC" on 4 SAODBLK_B frames, so that attribution is not used.
The second call deinterleaves NV12 losslessly to yuv420p (TV range, left chroma
siting, as the streams signal) before the same FFmpeg 7.0.2 conversion, and stops
unless that conversion equals the RGB oracle on every frame. It adds a per-pixel
test: **decisive pixels** are those where the oracle and NVDEC RGB pictures differ
by more than 4 in some channel. A decisive pixel follows NVDEC (or the oracle) when
torchcodec is within 2 in every channel, the CUDA control's maximum error in
attempt 1; it cannot be within 2 of both. A frame is classed `nvdec` or `oracle`
when all its decisive pixels follow that side, `mixed` otherwise, and unclassed
when it has no decisive pixels.

### Results

Evidence: [raw record](evidence/torchcodec-diagnostic/attempt-2.json),
[summary](evidence/torchcodec-diagnostic/summary.json) (from
summarize_torchcodec_diagnostic.py),
and the first call's
[raw record](evidence/torchcodec-diagnostic/attempt-1.json). One L4, driver
610.57.04, torch 2.14.1+cu130, torchcodec 0.17.0+cu130, PyAV 19.0.1 (bundled
libavcodec 63.1.102, which torchcodec loads), FFmpeg 7.0.2 static oracle; experiment time **215.9 s**. Every remuxed MP4
gave the identical FFmpeg 7.0.2 `framemd5` sequence; every frame count matched
(17, 500, 500); the common conversion equalled the RGB oracle on every frame;
torchcodec reported `_backend='CUDA'` with `_is_fallback=False` on CUDA.

| Stream | NVIDIA parser (`hevc_cuvid`), NV12 exact | torchcodec CUDA, RGB exact | torchcodec CPU, RGB exact | torchcodec CUDA max error vs RGB oracle |
|---|---:|---:|---:|---:|
| AMP_A (control) | 17 / 17 | 0 / 17 | **17 / 17** | 2 |
| SAODBLK_A | 0 / 500 | 0 / 500 | **500 / 500** | 15 |
| SAODBLK_B | 1 / 500 | 0 / 500 | **500 / 500** | 11 |

- **What is compared.** NVIDIA's parser and CoreAV's direct path are compared in
  NV12 against the `framemd5` oracle; torchcodec is compared in RGB24 against
  FFmpeg 7.0.2's RGB conversion of the same decode. Torchcodec CUDA is not exact on
  any frame, including the control that NVDEC decodes exactly (maximum error 2), so
  its RGB exactness count reflects colour conversion and cannot by itself locate a
  decoding difference.
- **NVIDIA parser, this run:** mismatches on exactly the same indices as CoreAV's
  direct path in the H.265 and pipelined rounds: all 500 frames of SAODBLK_A and
  frames 1–499 of SAODBLK_B. NV12 differences are small: exactly 2 samples per
  frame on SAODBLK_A and 0–1,505 samples per frame on SAODBLK_B, at most 7 code
  values.
- **Torchcodec CUDA follows NVDEC on the same frame indices.** SAODBLK_A: all 500
  frames have decisive pixels (8 per frame), and every decisive pixel follows
  NVDEC. SAODBLK_B: 50 frames have decisive pixels (3–20 per frame), and all
  follow NVDEC; no frame has a decisive pixel following the oracle, and none is
  mixed. On the other 449 mismatching frames, oracle and NVDEC RGB differ by at
  most 4 per channel, too little for the decisive test; the supplementary mean test
  (mean error on pixels where oracle and NVDEC RGB differ) puts all 499 mismatching
  frames closer to NVDEC, none closer to the oracle and none tied. Frame 0 of
  SAODBLK_B, where NVDEC equals the oracle, has a maximum error of 2, as on the
  control.
- **Torchcodec CPU** (FFmpeg's software decoder from the PyAV wheel) is RGB-exact
  on all 1,017 frames. On the same tests it follows the oracle: 500 and 50
  decisive frames, and 999 of 999 by the mean test.
- **Caveats.** The CUDA decoding path is a black box. These results establish
  that its output follows NVDEC's pictures, not that torchcodec uses NVDEC
  for these streams in a particular way. The 449 SAODBLK_B frames rest on the
  supplementary mean test, which classified the CPU control correctly on every
  frame in this run. Torchcodec read a stream-copy MP4, not the raw stream.
  Neither the threshold of 4 nor the bound of 2 is an RGB tolerance; neither is
  adopted.

**Interpretation, as facts only.** Three independent bitstream parsers driving
NVDEC on this L4 and driver (CoreAV's Rust headers, NVIDIA's parser, and
torchcodec's CUDA path) disagree with FFmpeg's software decoder on the same 999
frames, and torchcodec's CUDA output follows NVDEC there. On the same frames,
torchcodec's CPU path matches the oracle exactly.

**Runs.** Two calls, each with one L4, retries=0, a 600 s timeout and
a detached launch, both complete. Call 1 (142.6 s) used the unrefined conversion. Its counts match call 2, but its
attribution is not used; see the refinement above.

## H.264: inputs and first-measurement method

[conformance.json](conformance.json) pins the ITU AVCv1 `CABA1_SVA_B` and
`CANL1_SVA_B` archives, named bitstream members and extracted SHA-256 values.
Only bitstreams were extracted; no reference software was read. Their 17 frames
each are a small conformance subset, not certification of the AVC profiles.
No media is committed.

Corpus inputs are the complete hash-pinned Stage 0 `small` and `large` MP4s,
read from the existing baseline cloud volume **without changing its files**.
Their manifest was Stage 0's sample manifest, since removed with the Stage 0
harness; the raw reports of [measurement 0001](../../measurements/0001-baselines.md)
record the same URLs and SHA-256 values. Both are
encodings of Big Buck Bunny, **CC BY 3.0**:
**(c) copyright 2008, Blender Foundation / www.bigbuckbunny.org**.
Original corpus files remain complete and unmodified. FFmpeg's binary extracts
an elementary stream as a test adapter; CoreAV does not demux MP4 in this spike.
Rust parses the complete elementary input, decodes the first 1,100 pictures,
and compares the first 1,000 presentation frames, retaining lookahead for B-frame
reordering. Throughput uses 1,100 pictures. This is two encodings of one film,
not representative 100-file corpus coverage.

Three generated `testsrc2` H.264 fixtures have 720 frames at 320×180, 180 at
1080p and 60 at 4K, with B slices, three references and closed 60-frame GOPs.
The complete fixtures are copied into MP4 only for torchcodec's seeking API;
raw H.264 is used for Rust and CUVID. This adapter does not alter corpus files.
Generation commands and all resulting input hashes are retained in the runner
and raw evidence. Generated bytes are not assumed stable across CPU hosts.

One container ran all paths sequentially with three repetitions. Direct
NVDEC times exclude complete-file read/header parsing and context/session
construction, include map/unmap synchronization, and leave no CPU copy in the
timed GPU path. Torchcodec times use 16-frame range batches, explicit CUDA
synchronization and RGB CUDA tensors; construction is separate. FFmpeg
`h264_cuvid` runs through the pinned PyAV wheel, returning CPU NV12 frames.
CUVID codec opening is outside timing, but first-packet setup, demux and CPU
output are inside it; its session is recreated per repetition. These are
**unequal output and setup workloads**, and ratios are exploratory. The static
oracle executable has no `h264_cuvid`; the PyAV codec was available and ran.

Ratios pair repetition ordinals and matching input/count. Intervals bootstrap
the mean of three paired ratios (5,000 draws, seed 20261013); table FPS values
are medians. Tool order is fixed; first-iteration warmup and temporal drift
remain. CPU clocks and host placement are unpinned. The three observations and
one final host do not satisfy the stronger parity measurement rules in 6.5.
On the large corpus input, complete-file read/header parsing took **2,230.68 ms**
and CUDA/session construction took **288.80 ms**, both outside the reported
1,100-picture GPU repetitions. These setup costs are not zero or an end-to-end
loader speed claim.

## H.264: retained-session streaming measurement method

Each input uses one direct worker process for three replay trials, retaining
one CUDA **primary context**, loaded PTX module, RGB allocation and NVDEC session.
The torchcodec worker likewise retains one `VideoDecoder` object and its file
and exact index for all three trials. Both workers initialize once before the
replay timers. The comparison measures repeated requests for the **same input**;
it does not establish pooling across different files or sequence reconfiguration.
The initial IDR resets reference state without destroying the direct session.

Annex B is read in 64 KiB chunks. SPS/PPS and picture headers are parsed as
needed; slice entropy payload is passed unchanged to NVDEC. Presentation ranks
and the planning DPB are computed for one IDR-delimited GOP at a time, then
that GOP is submitted before reading the next. NV12 validation and decode
workers also parse incrementally. Encoded GOP data is limited to **64 MiB and
1,024 pictures**, with up to two streaming picture lookaheads, a priming picture and a NAL read buffer;
the existing 16 MiB NAL, 1,024 slices/picture and 512 MiB total-read ceilings
still apply. These bound retained storage independently of file duration;
Vec capacity and header metadata add bounded overhead to the payload ceiling.
In that round, the 32-frame RGB ring, 20 NVDEC surfaces and two output surfaces were fixed
for a given geometry. Over-limit GOPs fail explicitly. The evidence records
bytes actually read and peak retained GOP bytes/pictures for every replay.

Both sides produce the same CUDA uint8, contiguous NCHW presentation-order
RGB contract in batches of 16, including a shorter final batch. The corpus
requests 1,000 frames; fixtures request all 720/180/60. RGB conversion,
`.contiguous()` where needed, reorder lookahead and batch synchronization
are timed. Validation CPU copies, sampled RGB file writes and the exact
NV12 oracle run separately from performance measurements.

| Timer | Direct Rust worker | torchcodec worker |
|---|---|---|
| Process cold init (parent wall) | Fresh executable launch through first-picture headers, primary-context retain/set, NVDEC construction, PTX load, output allocation and synchronization, ending at ready JSON | Fresh Python launch, framework imports, input opening/exact indexing, `VideoDecoder` construction and CUDA synchronization, ending at ready JSON |
| Internal first init | Same initialization inside the executable, excluding process launch | Initialization from immediately before torch imports through CUDA synchronization, excluding interpreter launch |
| Replay setup | Synchronize/reset software DPB, reopen input, parse first picture and retain it for the next GOP | Synchronize the retained decoder; file/index/object stay open |
| Replay decode | Remaining incremental read/parse, per-GOP POC/reference planning, NVDEC submission/mapping, RGB conversion, presentation batching and synchronization | Range retrieval from frame zero, including rewind/seek, any lazy decoder initialization, decode, RGB output, contiguity and synchronization |
| Replay total | Setup + decode from the same trial | Setup + decode from the same trial |

Process cold init uses **three fresh processes per backend per input**, with
seeded randomized backend order within each trial. Its timer excludes shutdown,
validation and preparation. It measures construction-ready state rather than
time to the first output frame: lazy first-packet work stays in replay decode.
The process is cold; the GPU, driver, filesystem and JIT caches are not flushed.
Replay totals describe an already initialized worker and do not include cold
init. For a first request, initialization must be added; it is never presented
as zero or silently assigned only to the direct side. Imports and construction
are excluded from replay on **both** sides and included in their cold row.
Kernel compilation and fixture/extraction preparation are outside every timer.

Replay backend order is fixed and the three trial indices are matched for
ratios; they are not interleaved trials. Ratios are torchcodec elapsed time /
direct elapsed time for equal frame counts, so values above 1 favor direct.
Intervals are seeded 5,000-draw **95% bootstrap intervals of the mean of three
ratios**, not intervals on the reported median. Cold ratios use parent wall
readiness times. Unpinned clocks/host placement, cache state and three trials
limit inference; no Stage 1 graduation follows from these measurements alone.
The exploratory CUVID CPU-NV12 and worker transport measurements remain
separate from the equal-output RGB comparison.

## H.264: retained-session streaming measurement results

Complete evidence: [raw record](evidence/session-stream/attempt-1.json),
summary,
[source snapshot](evidence/session-stream/sources.json),
[verification](evidence/session-stream/verification.log),
and [follow-up checks](evidence/session-stream/follow-up-verification.log). Measured on one L4, driver 610.57.04.
Each setup/decode/total entry is its own median; total is computed within each
trial before taking the median, so the displayed setup and decode medians need
not sum to the displayed total. Ratio intervals are on the mean, as defined above.

| Input | Frames | Direct setup / decode / total (ms) | torchcodec setup / decode / total (ms) | Decode ratio (mean interval) | Total ratio (mean interval) |
|---|---:|---:|---:|---:|---:|
| small corpus | 1,000 | 0.125 / 183.252 / 183.308 | 0.012 / 215.262 / 215.274 | 1.176 (1.108–1.791) | 1.176 (1.108–1.791) |
| large corpus (1080p) | 1,000 | 0.122 / 1421.502 / 1421.639 | 0.014 / 1367.536 / 1367.548 | 0.961 (0.952–1.003) | 0.961 (0.952–1.003) |
| 320×180 fixture | 720 | 0.092 / 174.882 / 174.956 | 0.019 / 186.534 / 186.591 | 1.070 (1.001–1.980) | 1.070 (1.001–1.979) |
| 1080p fixture | 180 | 0.209 / 257.297 / 257.521 | 0.011 / 256.471 / 256.482 | 0.999 (0.997–1.409) | 0.999 (0.996–1.408) |
| 4K fixture | 60 | 0.695 / 323.328 / 323.941 | 0.016 / 336.326 / 336.340 | 1.042 (1.000–1.376) | 1.040 (0.998–1.372) |

Direct replay setup on the large corpus is **0.122 ms**, compared with
**1,973.64 ms** in the previous recreate/full-parse measurement. Its new
setup-inclusive ratio is **0.961 (0.952–1.003)**, previously
**0.473 (0.434–0.474)**. All five tested replay medians exceed the unchanged
0.95 floor, and all five bootstrap mean intervals have lower endpoints above
0.95. This is restricted H.264 sample evidence, not a full H2 pass. The first
replay includes lazy first-packet work, especially on torchcodec; no warm-up
trial is discarded, and the raw observations remain available. Fixed order,
three trials and unpinned clocks explain why a favorable median is insufficient
for graduation. Historical numbers remain below; session lifetime, parsing
scope and cache behavior changed, so these are methodological comparisons.

| Input | Direct process cold init (ms) | torchcodec process cold init (ms) | torchcodec/direct cold time (mean interval) |
|---|---:|---:|---:|
| small corpus | 393.98 | 3521.50 | 8.849 (6.180–15.321) |
| large corpus (1080p) | 280.07 | 3618.03 | 12.938 (10.471–14.547) |
| 320×180 fixture | 328.11 | 3570.92 | 10.883 (9.354–15.696) |
| 1080p fixture | 460.49 | 3583.93 | 7.702 (6.686–13.825) |
| 4K fixture | 327.93 | 3834.74 | 11.178 (9.844–13.607) |

These fresh-process construction-ready medians include torchcodec's interpreter
and framework imports. They exclude first-frame retrieval and do not establish
time to first RGB output or a device-cold result. Internal first-init timings
and all fresh-process observations are retained separately in the summary.
Cold construction is paid once before replay; the replay totals above describe
an already initialized worker and must not be labeled process-cold totals.

| Input | Encoded bytes read (MiB) | Peak GOP payload (MiB) | Peak GOP pictures |
|---|---:|---:|---:|
| small corpus | 3.688 | 0.060 | 12 |
| large corpus (1080p) | 14.812 | 5.108 | 250 |
| 320×180 fixture | 0.757 | 0.069 | 60 |
| 1080p fixture | 4.343 | 1.465 | 60 |
| 4K fixture | 5.296 | 5.295 | 60 |

No complete-file rank table or elementary-file allocation is retained. The large
input reads **14.812 MiB** to satisfy the requested presentation range and peaks
at **5.108 MiB / 250 pictures** for GOP payload, below the fixed ceiling. Decoded
RGB storage is the fixed 32-frame ring; NVDEC allocations depend on geometry.
Exact NV12 remains **2,994/2,994**, including 34 conformance, 2,000 corpus and
960 fixture frames. Direct RGB diagnostics remain **15/15 within 2/255**, maximum
error **1/255**, **2/15 exact**. No RGB oracle tolerance or timestamp pass is inferred.

Worker shm/pipe paired throughput is **0.955 (0.886–1.059)**
at 1080p and **1.143 (1.112–1.285)** at 4K.
Every paired parent checksum matches. The 1080p interval crosses 1.0;
4K shm/pipe sample p99 is **26.315/23.468 ms**. Streaming worker timings include
decode and parent consumption; they do not isolate IPC or certify production p99.
Prior worker numbers are preserved below.

**H2 verdict: inconclusive; the tested retained-session H.264 replay meets the
initial throughput floor, with setup included.** Restricted coverage, missing
HEVC and an equal-output CUVID baseline, unchecked timestamps, RGB oracle policy,
unpinned clocks/three trials and maintainer stage-gate
sign-off still prevent graduation. No kill/pivot or production enablement is authorized.

The run completed with one bounded call and no retries. Every build function has a 300-second timeout; the GPU function
has a 360-second timeout, 600-second startup limit, retries=0 and one container;
the runner has a 350-second timeout and the client cancels after 1,020 seconds.

## H.264: previous equal-output measurement method

The previous RGB comparison produced **CUDA-resident, contiguous uint8 NCHW RGB**
frames from both paths, in presentation order, in **16-frame batches** (the
last batch may be shorter). Direct NVDEC uses an authored CUDA kernel with
limited-range ITU-R BT.601/BT.709 inverse matrices and nearest-neighbor 4:2:0
chroma. This is RGB output equality of format, layout, device and frame order;
**it does not assume bit-exact RGB values**. NV12 checksums remain the exact
oracle; direct RGB versus torchcodec max/mean error and exact counts are
separate diagnostics, with the existing ≤2/255 bound reported informatively.
No threshold or known-divergence exemption is introduced.

In that retained measurement, each of three trials recreated its decoder. Both paths report setup and warm
decode separately, and **setup + decode** for the same requested output count.
Direct setup includes complete elementary-file read, Rust header parsing and
presentation-order calculation, CUDA context/session creation, PTX module load
and a bounded 32-frame GPU output ring. Torchcodec setup includes file opening,
exact indexing and CUDA decoder construction. NVRTC compilation of our kernel,
framework imports/global initialization, process launch, fixture/container
preparation and oracle work are harness preparation outside both timers.
There is no process-cold-start claim or subtraction of setup from the combined
ratio. Context creation is charged to the direct implementation because it
currently owns a context per decoder; torchcodec uses its framework context.

Direct output batches are released after synchronization, allowing ring reuse.
The kernel completes before NVDEC unmaps a source frame. Presentation ranks
come from Rust IDR/POC ordering, and ring occupancy/ranges are checked before
writing; extra pictures needed for reorder lookahead are counted in evidence.
The remeasurement generates fixtures directly in MP4 with encoder-assigned
timestamps, then extracts Annex B without re-encoding; this avoids the earlier
raw-stream remux losing frames in torchcodec's presentation range.
The corpus requests **1,000 presentation frames**, while fixtures request all
720/180/60 frames. Torchcodec uses the same counts and synchronizes every batch;
`.contiguous()` is inside its timed path. RGB validation copies only sampled
frames to host outside performance trials. DLPack and model consumption are not
implemented in this isolated throughput experiment.

Both paths run sequentially on one L4 host. Tool order remains fixed, CPU
frequency/host placement are unpinned, and only three repetitions are retained;
these limitations still keep it from counting as graduation evidence. FFmpeg CUVID
is remeasured as the prior **CPU NV12** baseline and remains an unequal-output
comparison. The decode-worker experiment retains identical one-slot backpressure,
120 frames per repetition and full parent checksum consumption for both transports.

Public color and compiler/driver specifications are linked in
[evidence/provenance.json](evidence/provenance.json). NVRTC and its paired
builtins are discovered from pinned distribution metadata, without reading
implementation source or adding a Cargo dependency.

## H.264: previous equal-output measurement results

Complete evidence: [attempt-3.json](evidence/equal-rgb/attempt-3.json),
[summary.json](evidence/equal-rgb/summary.json),
and [source hashes](evidence/equal-rgb/sources.json). Each ratio is the median of paired
trial ratios; intervals bootstrap the mean of three ratios as above. FPS
columns are medians and their quotient need not equal the paired median.

| Input | Frames | Direct GPU RGB FPS | torchcodec GPU RGB FPS | Warm direct/torchcodec (mean interval) | Including setup (mean interval) |
|---|---:|---:|---:|---:|---:|
| small corpus | 1,000 | 6,223 | 3,747 | 1.613 (1.552–1.758) | 0.443 (0.422–0.474) |
| large corpus (1080p) | 1,000 | 711 | 736 | 0.966 (0.955–0.980) | 0.473 (0.434–0.474) |
| 320×180 fixture | 720 | 3,978 | 3,331 | 1.194 (1.190–1.201) | 0.533 (0.506–0.549) |
| 1080p fixture | 180 | 706 | 691 | 1.021 (1.009–1.028) | 0.549 (0.497–0.620) |
| 4K fixture | 60 | 192 | 186 | 1.037 (1.025–1.058) | 0.673 (0.615–0.677) |

Previous setup/decode/total medians are retained explicitly:

| Input | Direct setup / decode / total (ms) | torchcodec setup / decode / total (ms) |
|---|---:|---:|
| small corpus | 626.071 / 160.687 / 786.759 | 84.710 / 266.874 / 351.585 |
| large corpus (1080p) | 1973.641 / 1406.032 / 3371.699 | 226.916 / 1358.633 / 1594.650 |
| 320×180 fixture | 232.736 / 180.981 / 413.717 | 4.138 / 216.167 / 220.305 |
| 1080p fixture | 240.946 / 254.810 / 495.876 | 11.171 / 260.315 / 272.285 |
| 4K fixture | 217.834 / 312.890 / 529.615 | 32.867 / 323.332 / 356.200 |

All required results completed without errors. The corpus output count is
1,000 rather than the first measurement's 1,100 decoded pictures, so the old
and new figures are methodological comparisons, not a controlled speed delta.
Direct versus torchcodec setup medians are **626.07/84.71 ms** (small) and
**1,973.64/226.92 ms** (large). Parsing the complete elementary input and owning
a fresh context/session dominate direct setup; no index/session-pool speedup
is assumed. All five warm RGB paired medians exceed 0.95, but every
setup-inclusive median is below 0.95. The initial warm throughput observation
is favorable; combined throughput is not at parity.

Exact NV12 coverage remains 2,994/2,994 against the FFmpeg oracle. For direct
RGB versus torchcodec, **15/15 samples meet ≤2**, **2/15 are exact**, and maximum
absolute error is **1**, not 2. The separate torchcodec-versus-FFmpeg RGB
comparison is retained under `rgb` in the summary and is a different diagnostic.
No RGB tolerance is adopted as a pass criterion. HEVC, full conformance/corpus
coverage and timestamps remain incomplete. The remeasured direct GPU NV12 /
FFmpeg CUVID CPU NV12 ratio on the large corpus is **0.962**; it remains
unequal output and cannot close the missing equal-output CUVID evidence.

### Worker transfer remeasurement

| Resolution / mode | FPS | GiB/s | p50 ms | p99 ms |
|---|---:|---:|---:|---:|
| 1080p pipe | 122.43 | 0.355 | 5.711 | 9.473 |
| 1080p shared memory | 136.68 | 0.396 | 4.998 | 8.670 |
| 4K pipe | 40.27 | 0.467 | 21.949 | 26.264 |
| 4K shared memory | 45.80 | 0.531 | 18.913 | 29.280 |

Every mode/trial transfers 120 frames; all parent checksums match in paired
trials. Median paired shm/pipe FPS is **1.108** at 1080p (mean interval
**0.903–1.116**) and **1.166** at 4K (**1.054–1.243**). The 1080p gain remains
uncertain. The 4K throughput gain does not imply a tail-latency win: shm/pipe
sample p99 is **29.280/26.264 ms**. Host and fixture-container preparation differ
from the first measurement, so neither absolute FPS nor gains are controlled
before/after comparisons. These remain full decode-worker exchange timings,
not isolated IPC bandwidth or production p99. The project's frame-transfer decision covers
them.

### Round environment

NVIDIA **L4**, driver **610.57.04**, CUDA user-mode driver **13.3**, torch CUDA
runtime **13.0**, with the same pinned Python/torch/torchcodec/PyAV dependencies,
2 requested/limited CPU cores, one-CPU affinity and 8,192 MiB RAM. The driver
and host differ from the first measurement. Successful runner duration **165.18 s**.
The app stopped on completion; nothing was deployed or scheduled.

Every call has retries=0, a 360-second function timeout, 600-second startup
limit and 350-second runner timeout; the client cancels after 1,020 seconds.

All three manual calls remain in the JSON evidence. Attempt 1 failed before input preparation because CUDA 13's
NVRTC wheel uses a new library layout; distribution metadata and explicit
paired-builtins loading corrected discovery, and the compiler was checked
locally before further GPU work. Attempt 2 retained successful corpus RGB
measurements but failed the fixture output-count check, and is not a complete
measurement. Attempt 3's timestamped-container fixture generation corrected
that adapter and completed every oracle, RGB and worker trial. No first-round
raw evidence was replaced. The successful call spent several minutes
waiting for L4 allocation; remote-call wall time is not all billable compute.

## H.264: first measurement (unequal output)

First-measurement raw evidence: [attempt-6.json](evidence/attempt-6.json), with every
candidate/reference checksum, timing, input hash, RGB diagnostic and environment.
Derived figures: [summary.json](evidence/summary.json). The earlier completed
candidate run is independently retained in [attempt-5.json](evidence/attempt-5.json).

| Input | Exact NV12 / compared | Direct GPU NV12 FPS | torchcodec CUDA RGB FPS | CUVID CPU NV12 FPS |
|---|---:|---:|---:|---:|
| CABA1_SVA_B | 17 / 17 | 7,361 | Not run | Not run |
| CANL1_SVA_B | 17 / 17 | 7,019 | Not run | Not run |
| small corpus | 1,000 / 1,000 | 6,697 | 4,358 | 4,617 |
| large corpus (1080p) | 1,000 / 1,000 | 713 | 745 | 760 |
| 320×180 fixture | 720 / 720 | 4,053 | 3,750 | 4,184 |
| 1080p fixture | 180 / 180 | 718 | 666 | 645 |
| 4K fixture | 60 / 60 | 195 | 195 | 164 |

No candidate/reference count or checksum mismatches on the sampled frames.
For the 1080p corpus, paired direct/torchcodec FPS ratio is **0.957**
(bootstrap mean interval **0.951–0.976**); direct/CUVID is **0.938**
(**0.937–0.965**). The latter median is 6.2% below the baseline and its interval
straddles the 0.95 floor. The small-corpus ratios are **1.537** and **1.396**,
respectively; they cannot be treated as equal-work speedups.

The explicit supplementary torchcodec RGB diagnostic uses **maximum absolute
per-channel error ≤2 on a 0–255 scale (2/255)**. **15/15 sampled
frames meet that bound; only 1/15 matches exactly**. This threshold is not a
new oracle policy, does not relax the candidate's zero-
tolerance NV12 gate, and does not certify torchcodec frame-exact. Per-frame
max/mean errors and out-of-tolerance channel counts remain in the evidence.

## H.264: first measurement, pipe versus shared memory

The worker actually parses and drives NVDEC in its own process, copies each
mapped frame to compact host NV12, and sends a descriptor and frame. Both modes
have one slot and identical acknowledgement backpressure. Shared memory is a
fixed-size mapped file; control uses `std` pipes/processes. The parent validates
**every descriptor** before allocation/view consumption: format, expected
geometry, stride, sequence, exact plane byte length, checked offset/end and
mapping extent. Descriptor JSON is limited to 1,024 bytes. It then reads every
byte for a checksum before acknowledging, so shm's parent reads are not optimized
away. Both modes produce equal parent checksums in every paired repetition.

Three repetitions transfer 120 decoded frames each at each resolution (4K
cycles the 60-frame fixture twice, resetting at IDR within one decoder session).
Latency covers worker decode, host copy, transport, descriptor validation and
parent consumption. FPS also includes startup. This measures an entire decode
worker exchange; it **does not isolate IPC-only bandwidth**, include seccomp,
model a worker pool or establish production p99.

| Resolution / mode | FPS | GiB/s | p50 ms | p99 ms |
|---|---:|---:|---:|---:|
| 1080p pipe | 146.93 | 0.426 | 4.235 | 12.769 |
| 1080p shared memory | 155.42 | 0.450 | 3.821 | 9.720 |
| 4K pipe | 43.69 | 0.506 | 19.461 | 32.582 |
| 4K shared memory | 51.36 | 0.595 | 16.220 | 28.859 |

Entries are medians of per-repetition statistics, including sample p99.
Median **paired** shm/pipe FPS ratio: **1.058 at 1080p** (mean interval
**1.000–1.200**, unrounded lower bound 0.99954) and **1.141 at 4K**
(**1.135–1.175**). The 1080p gain is not clearly outside observed noise.
No new decision was recorded in this round;
the project's frame-transfer decision now records all three rounds and the mapping
surface. A compromised worker can ignore the
acknowledgement or change/truncate its mapping: this trusted-worker timing
prototype does not prove the production hostile-worker isolation guarantee.

## H.264: first-measurement environment and failures

Final environment: **NVIDIA L4, driver 580.95.05, CUDA runtime 13.0**;
2 physical CPU cores requested/limited, 8,192 MiB RAM requested/limited,
affinity restricted to one visible CPU. CPU brand/frequency are not pinned.
Base digest is Python 3.13.5 slim bookworm:
`sha256:a7cb177dc60dfa77b223192cd0d9e6ce1aac7cf9a569023d68e7c91b6371a718`.
Derived images have the cloud host's IDs, not exported OCI digests.

Pinned test tools from an earlier pull request: **torch 2.14.1+cu130, torchcodec 0.17.0+cu130,
PyAV 19.0.1, NumPy 2.5.4**. The PyAV shared libraries are
libavcodec/libavformat **63.1.102**, distinct from the checksum-verified static
FFmpeg/ffprobe **7.0.2** oracle. The image reuses the baseline's complete pinned
GPU dependency file and binary-install helpers. No such tools are Rust runtime
dependencies. Build tools: Rust **1.85.1**, binary libclang **18.1.1**, Zig
**0.13.0**, linking to glibc 2.28 for the pinned bookworm runtime.

All six attempts remain visible:

1. Remote module-path startup failure; no measurements, app stopped explicitly.
2. ITU network HTTP 403; no measurements.
3. Blender network HTTP 403; no measurements. Later calls read existing pinned
   baseline media and staged verified conformance files instead.
4. All seven inputs prepared, but host-linked binary required glibc 2.39;
   zero candidate measurements. Corrected with the glibc 2.28 link target.
5. All 2,994 candidate hashes exact; two corpus torchcodec timings complete.
   Raw fixtures could not be sought by torchcodec, so the original measurement
   flow did not reach the worker transfers. Failures remain in that evidence.
6. Fixture container adapter corrected; candidate, torchcodec, CUVID and worker
   measurements all completed in **127.81 seconds**. No automatic retries.

The recorded evidence predates the exit-code fix: earlier attempts could exit
successfully despite download, decode, baseline, oracle or worker failures.
Their raw JSON and logs are retained unchanged. The corrected runner checks
every required result, prints the evidence before exiting nonzero on failure,
and the cloud entrypoint propagates that failure after saving the evidence.
Worker summaries now mark missing or unequal trials and frame counts as
incomplete and omit their paired statistics. These fixes were checked locally;
Those local fixes preceded the equal-output round below.

Every GPU function had a timeout (900 seconds initially, 180 for the final
bounded follow-up), a subprocess timeout, retries=0 and at most one container.
No app was deployed or left scheduled.

## Reproduction and checks

The spike's code is not part of the published tree. The
runs built the Rust extension with Rust 1.85.1, binary libclang 18.1.1 and Zig
0.13.0 (glibc 2.28 link target) and ran on a cloud host with one NVIDIA L4;
`--round-name hevc`, `h264` and `pipeline` selected the round, a separate launcher
ran the torchcodec SAODBLK diagnostic, and `--fetch-call ID` retrieved a recorded
function call's result without new compute. `inspect INPUT.h265` parses and plans a
stream on a machine without a GPU and prints its output order.

Each reproduction writes to a fresh evidence directory, so earlier evidence is
never overwritten. If network access to corpus URLs is blocked, the Stage 0
sample volume must already exist. Conformance downloads happen locally and are
staged into the image, with hashes checked again remotely.

Local verification (`verify-local.py`) passed
(first-round [verification.log](evidence/verification.log), rerun
[equal-rgb/verification.log](evidence/equal-rgb/verification.log), and
[follow-up verification](evidence/equal-rgb/follow-up-verification.log), and
[streaming verification](evidence/session-stream/verification.log), and
[H.265 verification](evidence/hevc/verification.log), and
[pipelined-round verification](evidence/pipeline/verification.log)):
workspace fmt, clippy `-D warnings`, tests and deny; excluded spike fmt, clippy
with and without `gpu`, tests with and without `gpu`, and deny on **its own
graph** with the repository allowlist; fuzz-target fmt/clippy/test/deny; Python
compilation and evidence checks. Cargo-deny emits only duplicate/unused-allowance
warnings. The initially pinned memmap2 version failed advisory checking; it was
updated to **0.9.11**, and the final graph passes. No advisory was suppressed.

The descriptor mutation target runs without extra dependencies:

```bash
cargo run --release --manifest-path "$S/fuzz/Cargo.toml" --bin descriptor -- 60
```

It completed **850,362,769 mutations in 60 seconds** without a finding
([raw record](evidence/descriptor-fuzz.json)). It exercises parent validation
and checked buffer views, plus process tests cover real pipe/mapping exchange.
It has **no coverage instrumentation or sanitizers** and is not a hardened
fuzz campaign. Instrumented descriptor/mapping fuzzing remains
required before production under the `unsafe` policy.

## Open decisions before graduation

- **GPU RGB exactness versus ≤2 tolerance:** the current exact NV12 oracle is
  unchanged. Adopting a tolerance for a production GPU RGB path requires a
  maintainer decision and a matching change to the oracle policy; diagnostic agreement does
  not authorize it.
- **Spike B into Spike A:** whether to connect this throwaway decoder to Spike
  A's MP4/index/seek/DLPack experiment remains a maintainer decision. This PR
  implements no such integration and authorizes no production graduation.
- **Throughput evidence quality:** pipelined replay meets the floor on all nine
  tested inputs with 15 alternating warm trials. Still single-host, unpinned
  clocks, one H.265 corpus file, and no equal-output CUVID baseline; the parity rules for
  performance benchmarks are not yet satisfied.
- **NVDEC divergence on SAODBLK_A/B:** NVIDIA's own parser path diverges on the
  same frames. It is accepted narrowly for the H2 verdict;
  it is still not a known divergence under the correctness policy and stays in the
  denominator. Reporting it to NVIDIA and testing other drivers or GPUs remain open.
- **Oracle cropping:** the H.265 oracle uses `-flags unaligned` to apply exact
  conformance windows. Whether to keep that setting or record a known divergence
  for FFmpeg's default left-crop behavior needs a maintainer decision.
- The retained-session H.264 sample meets the initial throughput floor including
  setup. More equal-output trials and a GPU RGB CUVID baseline are needed to assess
  parity on broader inputs and under the pinned-clock and noise rules.
- Extend corpus coverage (one H.265 file, two H.264 encodings of one film), H.264
  POC type 1/MMCO 5/interlacing/scaling, H.265 profiles beyond Main, timestamps,
  VFR, seek/session behavior, GPU-frame ownership and tensor interop.
- List the driver and mapping boundaries' `unsafe` sites; add hostile-worker containment,
  instrumented fuzzing and IPC-only measurements (open questions of the frame-transfer
  decision).
- Stage-gate verdict: H2 PASS by stage-gate decision.
  A CPU-fallback or detection follow-up for the accepted NVDEC divergence is
  required before release; it is tracked in
  an earlier change (detection
  conditions in Stage 2, CPU fallback before the first public release).
