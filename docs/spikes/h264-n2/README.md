# Track N2: H.264 decoder performance spike

Track N, step N2 of the roadmap. It builds on the
N1 decoder (verdict: pass), whose code is copied here (spike to
spike) and changed in place. Throwaway spike code: outside the
Cargo workspace, in no build, not published. Spike-local `unsafe` is used for
the intrinsics only.

## Erratum: what P1 and P2 compared

Added after the spike closed, from a measurement in step 6 of
`docs/design/h264.md`. **The verdict below and the bars are left as they
were recorded**; this says what the numbers in them mean.

P1 and P2 timed `--framemd5` on both sides. This decoder hashes its
output on a second thread (`src/bin/h264dec.rs:169-170` spawns the sink
thread), and `ffmpeg -threads 1 -f framemd5` hashes on its only thread.
The comparison was therefore two of our threads against one of FFmpeg's,
and the hashing is not a small part of it: on
`0404af1890da.0.1080p.h264`, 300 frames, `framemd5` costs FFmpeg 0.486 s
on top of a 0.538 s decode, and costs this decoder 0.097 s on top of
1.362 s.

The same binary, the same stream, the same 300 frames, measured both
ways:

| Method | This decoder / FFmpeg |
|---|---|
| `--framemd5` against `ffmpeg -threads 1 -f framemd5` (P1's method) | **1.424** |
| Decoding only, both sides discarding their frames | **2.53** |

1.424 is consistent with the 1.476 P1 reports, so the recorded run is
reproducible; it is the method, not the run, that this erratum is about.
A single-threaded decode is 2.53 times FFmpeg's on this stream, and that
is the number step 6 works against
(`docs/measurements/h264-decoder-speed.md`).

## Verdict

Status: Accepted.

P1 (x86-64) is 1.476 and P2 (arm64) 1.499, both above the 1.15 pass bar.
K1, K2, K3, E1, S1 and S2 are met.

The stage-gate verdict: **graduate the decoder now, and
continue the performance work in the production crate.** The production
decoder is written fresh under the hardened gates in `crates/coreav-h264`
and `crates/coreav-kernels-h264`, shipped as an opt-in, separately packaged
backend that is never in a default build, with the same speed bar (pass at
most 1.15) measured again on a fresh held-out draw.
`docs/design/h264.md` holds that design and those bars.

This spike is finished: no further performance work happens here. It stays
as the record of what was measured and of the kernels still missing for
parity (the "Where the time goes now" table below).

## Hypothesis

Control code designed for Rust, plus kernels written with Rust intrinsics
(AVX2 on x86-64, NEON on arm64), brings the N1 decoder within 15% of
FFmpeg's H.264 decoder, single-threaded, on both architectures. The spike
also names the kernels still missing for parity.

## Bars

These bars were committed before any measured run.

| Bar | Set | Threshold |
|---|---|---|
| P1 speed, x86-64 | held-out set, AVX2 kernels on | median per-stream time ratio to FFmpeg at most **1.15**, overall and in each resolution class |
| P2 speed, arm64 | held-out set, NEON kernels on | median per-stream time ratio to FFmpeg at most **1.15**, overall and in each resolution class |
| K1 conformance | the 97 in-scope conformance streams of N1 (`conformance/streams.json`), SIMD kernels on, on x86-64 and on arm64 | 97 of 97 bit-exact, as N1's C1 |
| K2 corpus | N1's C2 set on x86-64, SIMD kernels on | per stream, the same outcome as N1's measured run (`docs/spikes/h264-n1/evidence/corpus.jsonl.gz` with its 4k rerun): every agreeing stream still agrees, and each of the 13 disagreeing streams disagrees with the same frame count and the same first differing frame |
| K3 held-out MD5 on arm64 | held-out set, run P2 | every stream's MD5 sequence equals FFmpeg's, as on x86-64 |
| E1 kernel equivalence | every SIMD kernel against its scalar kernel | bit-identical output in property tests on random inputs (all block sizes and fractional positions, with edge and extreme values) and in a fuzz target per kernel family, run for at least 10 minutes per target without a difference |
| S1 clean room | `src/` without the generated tables | N1's bar S1, with `similarity/check.py` run on this spike's code |
| S2 tables | `src/spec_tables.rs` | N1's bar S2 |

**The kill criterion this README first set is withdrawn.** It read: a
median ratio above 1.50 overall on either architecture is a kill, and
H.264 stays on NVDEC and the fallback. There is no kill threshold for
H.264 now; the 1.15 pass bar restated above is what applies. It is recorded here
because a pre-registration that is quietly edited is worth nothing — what
was fixed before the run is what this README said, and this is what
changed since.

**Why the speed bar is above 1.00.** N2 is a milestone on the
path to parity, not a bar against FFmpeg, and milestones keep the bar set
for them. Parity
needs hand-written assembly for the remaining hot spots, which this spike
names but does not write. A ratio above 1.15 counts as not met: it is
reported with the list of remaining kernels.

**Claims.** Each median is reported with a 95% bootstrap confidence interval
(2,000 resamples of the per-stream ratios). The spike claims that the decoder
is faster than FFmpeg only if the upper bound of that interval is below 1.00.

## Sets and method

- **Streams.** Corpus v1 `real`-set H.264 streams that are in scope and that
  agreed with FFmpeg in N1's measured run. Streams that FFmpeg decoded in
  less than 0.5 s in N1 are left out, so process start-up does not dominate
  the ratio.
- **Held-out set** ([`sets/heldout.json`](sets/heldout.json)): the streams
  of files the manifest marks `holdout`. That is 139 streams: 30 sd, 43
  720p, 48 1080p and 18 4k. It is used only for the measured runs P1, P2 and
  K3, and no held-out stream is decoded during tuning. The list's SHA-256
  is `ce5707681733bc6b1bdd0b238b97210f6eab281bfef54c0e242216b820b52e96` as fixed (the public copy, whose `key` fields hold the file name only and whose `category` fields are removed, hashes to `7affbe17f17bfd27ca10053615f566969574eb4dcff31e515317a55c0ef1a6dc`). N1's I1 had 183 held-out streams, and the
  difference is the scope rule above, not a new split. I1 counted every
  agreeing held-out stream that FFmpeg decoded in more than 0.05 s; N2
  raises that floor to 0.5 s, which removes 44 short streams.
- **Tuning set**: the remaining 313 streams. Profiling and optimisation use
  only [`sets/tuning.json`](sets/tuning.json): per resolution class, the 8
  tuning streams with the smallest SHA-256, 32 streams in all.
- **Resolution classes**, by luma samples per picture:
  - sd: at most 640x480;
  - 720p: at most 1280x720;
  - 1080p: at most 1920x1088;
  - 4k: anything larger.
- **Ratio.** Each stream is extracted as in N1's C2 (first 9,064 access
  units). Two commands run on the same machine, each timed by wall clock:
  - `ffmpeg -threads 1 -i es.h264 -map 0:v:0 -frames:v 9000 -fps_mode
    passthrough -f framemd5 -`
  - `h264dec es.h264 --framemd5 --frames 9000`

  They run interleaved, FFmpeg then this decoder, for 3 repetitions. A
  stream's ratio is the median of this decoder's 3 times over the median of
  FFmpeg's 3 times (above 1 is slower). The runs also produce the MD5
  sequences for K3.
- **Scalar row.** The same held-out run also times the scalar build
  (`--kernels scalar`), once per stream, interleaved after the first
  repetition. It is reported as a row next to P1 and P2, so the SIMD gain is
  visible. It is information, not a bar.
- **Machines.**
  - x86-64: one cloud container per stream, 1 CPU and 4 GiB, as in N1.
  - arm64: a GitHub-hosted `ubuntu-24.04-arm` runner (Neoverse-N2, 2 cores),
    which runs the streams in shards one after the other.
  - Each run records the machine's CPU model next to its results. The
    2-core GitHub runner is noisy, which is why every stream repeats 3 times.
  - The oracle on both is the same FFmpeg version and configuration:
    FFmpeg 7.0.2, built by `scripts/build-ffmpeg.sh`
    from the official release tarball `ffmpeg-7.0.2.tar.xz` (SHA-256
    `8646515b638a3ad303e23af6a3587734447cb8fc0a0c064ecdb8e95c4fd8b389`).
    The build enables the H.264 decoder with its assembly, the demuxers and
    muxers the method needs, and nothing else. Each run records the
    resulting binary's SHA-256. The johnvansickle.com static release N1
    used could not be fetched for arm64. On x86-64 the source build gives
    the same MD5 sequences as that release and is as fast or slightly
    faster, so it is not the easier oracle.
  - Clean-room note: `make install` of this build also
    copied FFmpeg's API example `.c` files into `share/ffmpeg/examples`.
    They were found later on one development VM and deleted unopened; no
    decoder source was kept. The script now deletes the installed `share`
    and `include` directories, the source tree and the tarball right after
    the install.
- **Profile first.** Profiling uses only the tuning subset. The hot-spot
  table below is committed before any optimisation. The order of work
  follows it: the profile decides, and the expected order is interpolation,
  inverse transforms, deblocking, then CABAC.
- **Kernels.** Every kernel stays behind the `Kernels` trait. The scalar
  implementation is the reference, and `--kernels scalar` selects it at run
  time. The SIMD kernels are written from the specification's equations and
  the scalar code, never from FFmpeg's or x264's assembly.
- **Limits.** Every run script enforces hard timeouts.

## Measured runs

The procedure, fixed before the runs:

- **Binaries.** One commit of this branch for both architectures. x86-64:
  the release build of `h264dec` and the oracle build (SHA-256
  `930ef113…3b9d`) go into the cloud image. arm64: the spike's ARM
  speed workflow builds both on
  the runner.
- **Inputs.** The prepare stage of the speed script (held-out subset)
  extracts each held-out stream once (FFmpeg stream copy, the first 9,064
  access units, as in N1's C2) into R2 and records each elementary
  stream's SHA-256 in `sets/es-heldout.json`. Both architectures time these
  same bytes and check the hash before each stream. Extraction does not
  decode, so no held-out stream is decoded before the measured run. One
  held-out file (`2b42d40aa328…`, a QuickTime file with a zlib-compressed
  movie header) needs zlib, which the minimal oracle build leaves out; its
  stream was extracted with N1's static FFmpeg 7.0.2 (same version, stream
  copy), as `sets/es-heldout.json` notes.
- **x86-64 (P1, K3's reference).** The speed stage of the speed script:
  one container per stream, `cpu=1.0` and 4 GiB, at most 32 containers at a
  time; `scripts/speed.py` times inside the container. The cloud host's sandbox
  (gVisor) hides the CPU model name; the runs record vendor, family, model
  and clock instead.
- **arm64 (P2, K3, K1).** `scripts/arm_run.py start` stores a manifest of
  presigned R2 URLs (12 hours) for the streams and the conformance bundle,
  passes its URL to the workflow in a file that only the run branch's last
  commit adds (the branch is deleted after the run), assigns
  the streams to 14 shards by expected time and pushes the run branch.
  It runs in a private repository only: the branch would publish the
  presigned URLs otherwise, so the script refuses a public push target and
  the workflow's jobs skip there. K1 on arm64 compares with the per-frame MD5s of the reference YUV files
  (`scripts/conformance.py --make-ref-md5s`), so the runner needs only the
  bitstreams; on x86-64 the reference files themselves are compared.
- **E1 fuzzing.** The same workflow runs each of the five fuzz targets in
  `fuzz/` (luma and chroma interpolation, inverse transforms, edge filter,
  prediction combining; SIMD against scalar under libFuzzer) for 10 minutes
  on x86-64 and on arm64. The PR workflow runs them for 30 seconds each.
- **K2.** The N1 corpus run (the corpus script as in N1) with this
  build, compared stream by stream with N1's `evidence/corpus.jsonl.gz`.

## Design notes

- **Output on a second thread.** `h264dec` decodes on the main thread and
  hashes (or writes) output frames on a second thread. FFmpeg's
  command-line tool (since 7.0) also runs demuxing, decoding and muxing in
  separate threads; `-threads 1` keeps only its decoder single-threaded.
  Both commands therefore run one single-threaded decoder plus an output
  thread. On the cloud containers both share one physical core (two
  logical CPUs).
- **Deblocking one macroblock row behind decoding.** The filter of a row
  touches only that row and the three sample rows above it, and only the
  next row's intra prediction reads its unfiltered samples, so filtering a
  row once the next row is decoded gives the same picture as filtering at
  the end, with the samples still in cache. This relies on the slices of a
  picture arriving in increasing macroblock order, which Main and High
  profile streams guarantee (7.4.3); a non-conforming stream that sends an
  earlier slice later can decode differently from N1.
- **CABAC.** The engine keeps codIOffset scaled in a 64-bit window and
  decodes a decision without branching on the bin (the LPS case is a
  mask), with the context byte indexing the rangeTabLPS and transition
  tables directly. Bins decode without error checks; running past the end
  of the slice data is detected after each block and macroblock, before
  anything decoded is used. A residual block decodes in one function that
  keeps the engine in registers and returns position and value pairs.
- **Kernels.** `src/simd/`: each algorithm once (`algo.rs`) over 16-lane
  primitives with an AVX2 (`x86.rs`) and a NEON (`neon.rs`) set. x86-64
  selects them at run time after checking AVX2; the rest of the decoder
  stays baseline x86-64 code, as FFmpeg's C code is. i32 lanes where the
  scalar arithmetic needs them (transforms, j sample, bi-prediction);
  inputs outside the lanes' range go to the scalar kernel.
- **Control code.** Single-list, unweighted predictions are written straight
  into the picture; direct macroblocks predict blocks of equal motion
  together (prediction is per sample, so the result is the same);
  neighbour addresses are derived once per macroblock; released pictures
  are reused.

## Hot spots (tuning subset, before optimisation)

Measured with `perf record -F 999` over the 32 streams of the tuning subset
(first 300 frames each, `--framemd5`), using the N1 code as copied here
(scalar kernels), on the development machine (x86-64, Intel Xeon at 2.90 GHz). Shares
are of decoder process samples, grouped by the function that holds them.

| Part (clause) | Share |
|---|---|
| CABAC engine: decision, bypass, terminate and binarizations (9.3.3.2) | 15.1% |
| Output hashing, MD5 for `--framemd5` | 15.0% |
| Luma interpolation (8.4.2.2.1) | 14.4% |
| Deblocking edge filter (8.7.2) | 10.1% |
| CABAC residual and syntax decoding (7.3.5.3, 9.3.3.1) | 9.9% |
| Inverse transforms (8.5.12, 8.5.13) | 5.4% |
| Chroma interpolation (8.4.2.2.2) | 4.9% |
| Memory copy and fill (picture allocation, copies) | 3.9% |
| Prediction combining and weighting (8.4.2.3) | 3.6% |
| Deblocking boundary strength (8.7.2.1) | 2.7% |
| Motion vector prediction and direct modes (8.4.1) | 2.3% |
| Intra prediction (8.3) | 0.9% |
| Other control code: macroblock layer, neighbours, slice and picture logic | 10.8% |

The same run gave a median time ratio of about 4.2 to 4.9 against FFmpeg per
class on the tuning subset (600 frames, scalar kernels). MD5 is part of what
both sides do under `-f framemd5`, so its cost counts.

Order of work, from this table:
1. MD5 hashing.
2. CABAC: the engine, then residual decoding.
3. Luma interpolation.
4. Deblocking.
5. Inverse transforms.
6. Chroma interpolation and prediction combining.
7. The remaining control code.

## Results

Measured runs of one source tree (`h264dec` SHA-256 `281897f0…40fa` on
x86-64, `777fa90b…594c` built on the arm64 runner), on the 139 held-out
streams (`evidence/speed-x86_64.jsonl`,
`evidence/arm64/speed-arm64-*/`, summary in `evidence/speed-summary.json`).
Ratio: this decoder's median time over FFmpeg's median time per stream, 3
interleaved repetitions (above 1 is slower); median over streams with the
95% bootstrap interval.

| Bar | Overall | sd (30) | 720p (43) | 1080p (48) | 4k (18) | Verdict |
|---|---|---|---|---|---|---|
| P1 speed, x86-64, SIMD | **1.476** [1.437, 1.506] | 1.401 | 1.490 | 1.501 | 1.463 | not met (bar 1.15) |
| P2 speed, arm64, SIMD | **1.499** [1.411, 1.605] | 1.671 | 1.489 | 1.448 | 1.097 | not met (bar 1.15) |
| Scalar row, x86-64 | 2.219 [2.068, 2.328] | 2.301 | 2.235 | 2.211 | 1.909 | information |
| Scalar row, arm64 | 2.212 [2.031, 2.407] | 2.751 | 2.242 | 2.025 | 1.538 | information |

Per-class intervals are in the summary file. Both architectures are above
the 1.15 pass bar, so N2 does not meet it, and that bar is per resolution
class as well as overall: the only class inside it anywhere is arm64 4k
at 1.097. What follows was the maintainer's to decide, with the list of
missing kernels below; the stage-gate verdict is under "Verdict" above
(graduate the decoder now; an earlier change, `b7c3b423`).
No interval lies below 1.00, so the spike claims no speed advantage
anywhere.

| Bar | Result |
|---|---|
| K1 conformance, SIMD on | x86-64: 97 of 97 bit-exact (`evidence/conformance-x86_64.json`). arm64: 97 of 97 (`evidence/arm64/k1-arm64/`), against the reference files' per-frame MD5s |
| K2 corpus, x86-64 | ran after the speed runs with the same x86-64 binary (`281897f0…40fa`; the corpus records do not carry the hash); met: all 1,125 streams of N1's run keep their outcome (933 agree, 13 disagree with the same frame count and first differing frame, 179 not decoded for the same reasons); N1's two 4k streams that needed a local rerun now finish in the batch (`evidence/corpus-k2.jsonl`, `evidence/k2-compare.json`) |
| K3 held-out MD5 on arm64 | 139 of 139 streams: this decoder's MD5 sequence equals FFmpeg's and the x86-64 sequence |
| E1 kernel equivalence | property tests (20,000 cases per kernel family, both architectures in CI) and the five fuzz targets, 10 minutes each on x86-64 and arm64 (35 to 109 million inputs per run): no difference (`evidence/arm64/fuzz-*`) |
| S1 clean room | `similarity/check.py` on this spike's code: no span of 50 or more tokens outside the allowlist |
| S2 tables | unchanged from N1; regenerated byte-identical in CI |

**Fixed after the runs.** The horizontal luma deblocking path formed an
intermediate pointer past the end of the plane before stepping back (only
the final addresses were in bounds); the kernel entry points accepted
overlapping edge geometries (which the decoder never passes, and where
gather-then-scatter differs from the scalar kernel's line-by-line
updates); and their length checks could overflow. The fix reorders the pointer arithmetic, sends overlapping
geometries to the scalar kernel and checks the arithmetic. It changes no
decoded output: K1 97 of 97 and the tuning subset's MD5s are unchanged,
and the property tests cover the new fallback. The measured binaries are
the ones above.

**Machines.** x86-64: cloud containers with one physical core (two logical
CPUs) in gVisor, which hides model names: AMD family 25 model 1 (EPYC Zen 3)
for 90 streams, family 25 model 17 (Zen 4) for 48, Intel family 6 model 85
for 1. Each stream's FFmpeg and decoder runs share one container, so the
mix does not bias a ratio. arm64: GitHub `ubuntu-24.04-arm` runners,
Neoverse-N2, two cores. Oracle binaries: x86-64 `930ef113…3b9d`, arm64
`ce780578…aa09` (both from `scripts/build-ffmpeg.sh`).

**Where the time goes now.** Final profile of the measured build, tuning
subset (as the table above), shares of the decoding thread (MD5 runs on the
output thread):

| Part (clause) | Share | Kernel or control code still missing for parity |
|---|---|---|
| CABAC residual blocks: significance map and levels (9.3.2.3, 9.3.3) | 36.9% | a hand-scheduled residual decoder; the bins are serial, so this is latency-bound scalar code (about 22 cycles per context-coded bin here) |
| Deblocking driver, bS and kernel entry (8.7.2.1) | 11.4% | a SIMD bS kernel and whole-macroblock filtering (one transpose per macroblock instead of per edge) |
| Inter prediction control: MV prediction, direct, block fetch (8.4.1) | 7.7% | control code |
| CABAC other syntax and macroblock layer | 7.3% | control code |
| Deblocking edge filter kernel (8.7.2.3, 8.7.2.4) | 6.2% | see above |
| Luma interpolation kernel (8.4.2.2.1) | 5.4% | 8- and 4-wide variants (the kernel computes 16 lanes for every block) |
| Inverse transform kernels (8.5.12, 8.5.13) | 4.5% | DC-only and fused dequantization paths |
| Chroma interpolation kernel (8.4.2.2.2) | 4.3% | both chroma planes in one call; 4- and 2-wide variants |
| Intra prediction (8.3) | 4.0% | intra prediction kernels (scalar today) |
| Memory fill and copy | 3.9% | control code |
| Residual reconstruction control (8.5) | 2.8% | control code |
| Prediction combining kernel (8.4.2.3) | 0.4% | none |
| Other macroblock layer and picture control | 5.0% | control code |

From the N1 code (tuning subset, about 4.2 to 4.9) to about 1.5: the
scalar row (about 2.2) is the N1 code plus the control-code changes, with
the scalar kernels (branchless CABAC decisions, residual blocks decoded
with the engine in registers, deblocking one row behind decoding,
direct-mode block merging, picture reuse, hashing on the output thread as
FFmpeg's command-line tool does); the SIMD kernels take it from there to
about 1.5. The tuning and held-out sets differ, so these steps are
approximate.

