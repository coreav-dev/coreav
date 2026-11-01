# VP9 decoding

Status: Proposed. This note is the plan of CoreAV's own VP9 decoder and pre-registers its bars; no decoder code exists yet. Code, when it lands: `crates/coreav-vp9` (parsing, entropy decoding, control code), `crates/coreav-kernels-vp9` (pixel kernels) and `crates/coreav-backend-vp9` (the decode-boundary backend). Like the H.264 decoder (`docs/design/h264.md`), it reaches users only through an opt-in backend of the decode boundary (`docs/design/decode.md`), never in a default build.

## What this is

CoreAV decodes VP9 through libvpx today (`docs/design/decode.md`, "libvpx"). That path meets the thumbnail bars and misses the transcode bar B5 (1.075 and 1.074 against at most 1.05, `docs/measurements/vp9-libvpx.md`). The attribution of that miss puts about a quarter of the extra time in decoding, all of it on the 4K and 8K files, and most of the rest in copying frames out of the sandboxed worker ("Attribution of the B5 gap"). Decoding alone, CoreAV's libvpx backend in the process takes 1.28 times the CPU of FFmpeg's own VP9 decoder (median, the attribution's profiled pass). The comparison of libvpx builds in coreav-dev/coreav#182 measured the CPU of the same harness (which allocates and exports each frame) against FFmpeg 9.0.2's VP9 decoder at default threads: on the three B5 files of 4K and above, 1.213, 1.226 and 1.582 for the release's static musl libvpx 1.14.1, 1.162, 1.207 and 1.338 for Debian's libvpx 1.15.0, and 1.157, 1.209 and 1.334 for a GNU build of 1.14.1; over all ten B5 files the median is 1.580 for the release build and 1.272 for Debian's. No tested build change closes the gap.

The goal is a VP9 decoder, written from the specification in memory-safe Rust, that decodes every VP9 stream bit-exactly and at least as fast as FFmpeg's own VP9 decoder (`vp9`, its native decoder, never its libvpx wrapper), in the calling process, so that VP9 decoding stops being the slow and copied part of `coreav transcode` and `coreav thumbnail`. Reaching parity does not end the work, as with H.264.

## Specification and the clean room

- **The specification** is the VP9 Bitstream & Decoding Process Specification, version 0.7 of 22 February 2017 (`https://storage.googleapis.com/downloads.webmproject.org/docs/vp9/vp9-bitstream-specification-v0.7-20170222-draft.pdf`, SHA-256 `92a5bc772d11572c2f119680998ee202e249d075469515fe43bbb7febb51b963`). Clause numbers in this note and in the code are that version's. The container bindings are already read elsewhere: the VP codec ISOBMFF binding and Matroska's `CodecPrivate` (`crates/coreav-decode`), and the superframe index of the specification's Annex B (`crates/coreav-backend-libvpx/src/header.rs`, which the new crates do not depend on).
- **Nothing else is read.** No source of libvpx, FFmpeg or any other VP9 implementation is opened, nor any repository file of theirs, the test vector lists included. `ffmpeg`, `ffprobe` and libvpx (through the existing backend and `tools/decode-oracle`) are black boxes: their outputs are compared, their code is not read.
- **Test vectors and their MD5s are data.** The WebM project publishes VP9 test streams with a per-frame MD5 of each decoded frame beside each one (`https://storage.googleapis.com/downloads.webmproject.org/test_data/libvpx/`). The set is taken from that bucket's own listing: every stream named `vp90-2-*`, `vp91-2-*`, `vp92-2-*` or `vp93-2-*` (profiles 0 to 3) with a `.md5` file beside it, about 380 streams and 1.9 GB. Milestone 1 commits the list with each file's SHA-256 (`crates/coreav-vp9/tests/data/vectors.json`); the files themselves are downloaded and checked against it, never committed. A frame's MD5 there is the MD5 of its planes packed without padding, which is `framemd5`'s layout: FFmpeg 9.0.2's `framemd5` equals the published MD5 of every frame of the three vectors checked before this note (`vp90-2-00-quantizer-10`, `vp90-2-08-tile-4x4`, `vp91-2-04-yuv444`).
- **The similarity guard covers the new code from the first pull request that adds any.** `crates/coreav-vp9` and `crates/coreav-kernels-vp9` join the code-similarity check (`docs/design/ci-checks.md`, "Before opening a pull request"), compared with FFmpeg 9.0.2's VP9 decoding files and libvpx 1.14.1's VP9 decoder and its DSP code. The comparison sets and their calibration are added in the operations repository first, and that change merges before the first public pull request that adds decoder code, since every run reads the allowlist of that repository's `main`. The specification's tables (scan orders, default probabilities, the Pareto table, conversion tables) are transcribed from its clause 10, and each is listed as a specification item of the allowlist with the table it reproduces, as the H.264 and Opus tables are. The backend crate holds no codec logic and is not compared, as `coreav-backend-h264` is not.

## Scope

The finished decoder decodes every VP9 stream the specification defines: profiles 0 to 3; 8, 10 and 12 bits; 4:2:0, 4:2:2, 4:4:0 and 4:4:4; tiles, segmentation, lossless frames, intra-only frames, hidden frames and `show_existing_frame`, error-resilient and frame-parallel modes, reference frames of another size (scaled motion compensation), frame sizes that change within a stream, and superframes. No feature is permanently out of scope.

Until a milestone lands, what it would add is refused with `Error::Unsupported`, never decoded wrongly. Until milestone 5, every stream of profiles 1 to 3 is refused when the backend opens (the profile is in the stream description, `StreamConfig`), so selection moves on to libvpx or the fallback.

## Packaging: opt-in, separately packaged

Google's patent grant for VP9 covers "the copyrightable works that implement the WebM codecs distributed by Google as part of the WebM Project" (`https://www.webmproject.org/license/additional/`), so it covers libvpx and not a decoder of CoreAV's own, and a patent pool offers licences under patents it says relate to the VP9 specification. The decoder therefore arrives as the native H.264 decoder did (`docs/design/h264.md`, "Packaging"):

1. No default build contains it. The CLI and `tools/decode-oracle` reach it only through a non-default feature, `native-vp9`; a default build asked for it fails with `unsupported_feature`.
2. The crates are workspace members, so CI builds, lints, tests and fuzzes them like everything else.
3. The crates' README files and documentation state the patent position and that the decoder is not enabled by default.
4. Putting it in a release build (`release-full` included), publishing anything that contains it, and changing the default selection are the maintainer's decisions. The release builds keep libvpx until then.

## Architecture

```
coreav-vp9                      safe Rust, forbid(unsafe_code)
  bits        f(n) and s(n) for the uncompressed header (9.1)
  bool        the boolean decoder: init, read, exit with its padding check (9.2)
  header      the uncompressed header (6.2, 7.2) and frame size with refs
  compressed  the compressed header: tx mode, probability deltas (6.3, 7.3)
  probs       default and fixed tables (10.4, 10.5), the four saved frame
              contexts, counts and backward adaptation (8.3, 8.4)
  tile        tile layout and the partition walk (6.4.1 to 6.4.4)
  modes       intra and inter mode info, segment ids, skip, tx size (6.4.5 to 6.4.20)
  mvref       motion vector prediction (6.5)
  tokens      residual tokens, scans and their contexts (6.4.21 to 6.4.26)
  recon       dequantization and reconstruction (8.6), through the kernels
  predict     intra edges, inter block geometry and scaling (8.5), through the kernels
  filter      loop filter levels, edge masks and order (8.8), through the kernels
  refs        the eight reference slots, refresh and output (8.9, 8.10)
  superframe  the Annex B index
  limits, error, picture

coreav-kernels-vp9              deny(unsafe_code), allowed on simd modules only
  views       validated windows of samples, as in coreav-kernels-h264
  scalar      the reference kernels: inverse DCT, ADST and WHT; intra
              predictors; 8-tap and bilinear convolution, scaled and
              unscaled, and the compound average; the 4, 8 and 16 sample
              loop filters; for 8-bit and for 16-bit samples
  simd        AVX2 first, then NEON, behind the same trait (milestone 6)

coreav-backend-vp9              safe Rust, forbid(unsafe_code)
  lib         the native VP9 backend of the decode boundary
```

**Sans-IO, limits everywhere.** The decoder reads no files and starts no thread unless asked. The caller hands it one packet (a frame or a superframe) and gets back the frames shown. Every entry point takes `Limits`: the largest frame area and sides, the largest packet and tile, and a memory budget the decoder checks before it allocates a frame of a new size. The decode boundary sets them from its own policy (`max_pixels`, `max_width`, `max_height`, `max_packet_bytes`, `max_memory_bytes`), so an upload cannot make the process allocate eight 8K references of 16-bit samples. The specification's eight reference slots are fixed, so no limit counts them.

**The kernel boundary** follows the H.264 decoder's decisions (`docs/design/h264.md`, "The kernel boundary"): kernels take validated views, never a slice with an offset and a stride; the scalar kernels are the reference, shipped, and every other implementation is compared with them bit for bit; one trait, monomorphised, with `Auto` choosing per kernel at run time. Sample depth is a type parameter (8-bit samples in `u8`, 10 and 12 in `u16`), so the 8-bit path pays nothing for the others. The transforms and filters are written once from clauses 8.7 and 8.8 and instantiated for both.

**No `unsafe` outside the kernel crate's SIMD modules.** Every parser of untrusted bytes, the boolean decoder and the token reader included, is safe Rust. If a measured ceiling later shows that a hot path outside the kernels needs `unsafe` (the boolean decoder is the likely candidate, as CABAC was for H.264), that is a new policy record for the maintainer, as policy record 0038 was, and is not part of this plan.

**Reference frames carry a border.** Motion vectors may point outside a reference frame, and the specification clamps each sample position to the frame (8.5.2.3). Each reference plane is stored with a border wide enough for the farthest position a block may read, filled by edge extension once when the frame becomes a reference, so the interpolation kernels never clamp per sample. The output equals the clamped reads by construction; the vectors with motion past the edges check it.

**The decode-boundary backend.** `coreav-backend-vp9` is a backend of `coreav-decode` in the calling process, as `coreav-backend-h264` is: no worker and no shared-memory copy, which removes the part of the B5 gap the attribution puts in the copies through the worker. One packet is one frame or one superframe; a packet's frames go through in order and the shown ones come back with the packet's pts (VP9 has no reordering; a hidden frame is shown later by `show_existing_frame`, which takes the pts of its own packet). `Packet.discard` decodes without building the frame. A keyframe-only mode (`NativeOptions`) decodes key frames and skips everything else, for thumbnails at a keyframe. In a build with both, the policy's order puts the native backend ahead of libvpx; the feature exists only once bars C1 and C2 below are met for the streams it takes.

## The plan, in milestones

Each row is one or more pull requests, each reviewed independently; the rows that add `unsafe` (milestone 6) get a second independent review. "PRs" is the expected count, each of a size one reviewer reads in one sitting. A milestone's gate is checked on the pull request that completes it; a pull request inside a milestone ships with its own tests and keeps every earlier gate green.

| # | Milestone | PRs | Gate |
|---|---|---|---|
| 0 | This note | 1 | Review |
| 1 | Headers, probabilities and the boolean decoder: the crate skeleton, limits and errors; `f(n)`, the boolean decoder and its exit process; the uncompressed header, frame size with refs, the reference slots' sizes and refresh; the compressed header with every probability delta; the default tables and the four saved contexts with their reset and save rules; the superframe index; the vector manifest and the harness; a `headers` fuzz target | 2 | G1: on every vector, every frame's headers parse; the frames shown and each one's size equal the published MD5 list's entries (each entry names the frame's size); every compressed header ends inside its declared size with its padding 0 (9.2.3) |
| 2 | Intra frames: tiles and the partition walk; mode info of key and intra-only frames; segment ids, skip and transform size; tokens with their scans and contexts; dequantization; the inverse DCT and ADST of 4 to 32 samples and the WHT; intra prediction with its edge rules; reconstruction; backward adaptation after each frame | 3 | G2: every tile of every intra frame of the profile 0 vectors decodes and exits with padding 0; the MD5 of every intra frame whose loop filter level is 0 equals the published one |
| 3 | The loop filter: levels by segment, reference and mode (8.8.1); edge masks by transform and block size; the filters of 4, 8 and 16 samples; the order of 8.8.2 | 2 | G3: the MD5 of every intra frame of every profile 0 vector equals the published one, and the first frame of each 8-bit 4:2:0 development file of corpus v1 equals FFmpeg 9.0.2's |
| 4 | Inter frames: inter mode info and reference selection; motion vector reading and prediction (6.5); compound prediction; the three 8-tap filters and bilinear; scaled references; intra-only refresh, hidden frames and `show_existing_frame`; error-resilient and frame-parallel modes and context resets | 4 | G4: bar C1 on every profile 0 vector; bar C2 on the 8-bit 4:2:0 corpus files |
| B | The backend: `coreav-backend-vp9`, the `native-vp9` feature of the CLI and `tools/decode-oracle`, the keyframe-only mode, the boundary's limits, a backend fuzz target | 1 | Bars C1, C2 and C5 for the streams in scope; the decode boundary's bars 1, 2 and 4 (`docs/design/decode.md`, "Bars") for the new backend |
| 5 | Profiles 1 to 3: 4:4:4, 4:2:2 and 4:4:0, 10 and 12 bits, through the sample-depth parameter | 2 | Bar C1 on every vector; bar C2 on every corpus file |
| 6 | SIMD kernels, AVX2 on x86-64 first (CoreAV's performance focus), then NEON: the inverse transforms, the interpolation and its average, the loop filters, intra prediction; each family a round with its ceiling measured first and its own bars | 4 to 6 | Bar C3 for each kernel; C1 with the SIMD kernels on; each round's bars |
| 7 | Threading: tiles and loop-filter rows within a frame, then frames in flight with progress per superblock row; the thread count within the memory budget | 2 to 3 | T1 to T3 ("Threading (milestone 7)" below) |
| R | The gated speed run: a round README with the held-out list and method, then the run | 2 | Bars VP-1 to VP-5 |

About 20 to 25 pull requests to the gated run: 12 to 13 to a complete, correct, scalar decoder (milestones 1 to 5 and the backend), and 8 to 12 for SIMD, threading and the run. As with H.264, rounds of profiling and optimisation follow where the run misses a bar, each with its own pre-registered bars; their number is not predicted here.

**The loop filter comes before inter prediction.** The intended order was intra, then inter, then the loop filter. But no black box exposes unfiltered frames: FFmpeg 9.0.2's VP9 output with `-skip_loop_filter all` is identical to its default output on the three vectors above, and every frame of a published MD5 list is filtered. With the filter after inter prediction, milestones 2 and 4 could be checked only on the few frames whose filter level is 0. With it before, every intra frame is checked from milestone 3 on, and inter prediction is checked frame by frame against references already known to be right.

**Checkpoint after milestone 5** (information for the maintainer, not a pause): the scalar decoder's one-thread instruction count and wall-clock time against FFmpeg 9.0.2 on the training files, with FFmpeg profiled at function level only, so the size of the SIMD and threading work is known before it starts.

## Bars

Pre-registered: every threshold here is committed before the run that measures it. A round README may add method details before its run; it may not change a threshold.

### Correctness

| Bar | Set | Threshold |
|---|---|---|
| C1 vectors | the manifest's vectors (milestone 1), all profiles once milestone 5 lands | every frame's MD5 equals the published MD5 in every cell of the matrix below |
| C2 corpus | the roster's 74 VP9 files (below) by the method of the decode boundary's bar 1 (the first 120 packets, against `framemd5` of FFmpeg 9.0.2 with `-c:v vp9`); and every frame of the ten B5 files, against the same | 100% of streams and of frames equal. A difference counts as a divergence only where libvpx (the libvpx backend through `decode-oracle`) agrees with this decoder; each is recorded as a known divergence with the frame and the reason |
| C3 kernels | every SIMD kernel against the scalar reference | bit-identical in property tests over every block size, transform type, filter, sub-sample position, scale step and filter level, extreme sample values included, and in a fuzz target per kernel family run at least 10 minutes per target per architecture without a difference |
| C4 robustness | every fuzz target of the three crates | in CI on each pull request that can affect them; before the first release that contains the decoder, a campaign of at least 24 CPU-hours with no crash, hang or out-of-memory report (`docs/design/fuzz-campaign.md`); no `unwrap`, `panic!` or slice indexing (the workspace lints) |
| C5 keyframe-only | the vectors and the roster's 74 files | the keyframe-only mode returns, for every shown key frame, the frame full decoding returns for it |

C2's 74 files were measured before with libvpx (74 of 74 streams agreed with FFmpeg's `vp9` on the first 120 packets), so C2 is a bar on this decoder, not on FFmpeg.

**Where each bar runs, with which kernels and threads.** Every cell must pass; a milestone's gate names the bars, and this matrix fixes how each is run. Thread counts above 1 apply from milestone 7 on and SIMD cells from milestone 6 on; before them, only the cells that exist are run.

| Bar | Hosts | Kernels | Threads |
|---|---|---|---|
| C1 | GitHub-hosted `ubuntu-24.04` (x86-64) and `macos-14` (arm64) | scalar, and `Auto` | 1, 2 and 8 with `Auto`; 1 with scalar |
| C2 | GitHub-hosted `ubuntu-24.04` (x86-64), the corpus fetched with read-only credentials in its fetch step only | scalar, and `Auto` | 1 and 8 with `Auto`; 1 with scalar |
| C3 | GitHub-hosted `ubuntu-24.04` (x86-64) and `macos-14` (arm64) | each SIMD kernel against scalar | not applicable |
| C5 | GitHub-hosted `ubuntu-24.04` (x86-64) | `Auto` | 1 |

### Speed

Policy record 0034's rule applies: the median of per-file ratios, CoreAV over FFmpeg, at most 1.00 overall and in every class; a claim that CoreAV is faster needs the upper end of the 95% bootstrap interval below 1.00, on held-out files. The baseline is FFmpeg 9.0.2, the pinned build (`scripts/oracle-ffmpeg.env`, checked with `oracle_pin.py --verify-bin`), with `-c:v vp9`.

**Files.** The sets are fixed by `docs/measurements/vp9-native/roster.json`, committed with this note: by SHA-256, the 46 development and 28 held-out files of `docs/measurements/vp9-libvpx/thumbnails/selection.json` and the ten files of `docs/measurements/vp9-libvpx/transcode-b5/files.jsonl` (all ten held out), with the SHA-256 of both sources at commit `dfa3b897d3ca9f482d6a7a84b8a40e6944c7b107`. Every runner reads the roster and refuses to run unless its inputs equal the named set exactly. Tuning and every round's A/B use only the development files and the test vectors; the gated run uses the 28 held-out files. No held-out file is used to tune this decoder or any of its rounds. They were timed before through libvpx and the fallback (`docs/measurements/vp9-libvpx.md`), which tuned nothing here, so a "faster" claim on them is allowed by the rule above. Classes by the luma samples of a file's first frame (`ffprobe`'s width times height): below 921,600 (smaller than 1280x720), up to 2,073,600 (up to 1920x1080), and above. A class with fewer than 5 held-out files is judged together with the next larger class, or with the next smaller one for the largest class, and is also reported alone.

| Bar | Measure | Threshold |
|---|---|---|
| VP-1 one thread | decoder alone, one thread, pinned: ours decoding without output against `ffmpeg -threads 1 -c:v vp9 -f null`, and ours writing raw frames against `-f rawvideo`; the held-out files | at most **1.00**, both pairings, overall and per class |
| VP-2 eight threads | the same pairings at eight threads on both sides, on `c4-standard-16` with SMT off | at most **1.00**, both pairings, overall and per class |
| VP-3 CPU, as libvpx was measured | the CPU time of the harness of coreav-dev/coreav#182 (each frame allocated and exported), default threads, against FFmpeg's `vp9` at default threads, on the ten B5 files | median at most **1.00** over the ten, and at most **1.00** over the three of 4K and above |
| VP-4 transcode | B5 (`docs/design/transcode.md`, "Bars") with the native backend in front of libvpx, both encoder arms | at most **1.05**, the bar B5 has, overall and per class |
| VP-5 thumbnails | S1 to S5 (`docs/design/thumbnails.md`) through the native backend, on the held-out VP9 files | at most **1.00**, each |

VP-3 and VP-4 answer the measurements this plan starts from; VP-1 and VP-2 are the decoder bars. Information rows, with no bar: the scalar build, arm64, and the release build's libvpx in the VP-3 harness.

**How each is run.** These choices are part of the bars. A round README adds only what they leave open (host names, binary hashes) and may not change any of them.

- **Build.** CoreAV built with `--profile dist` and `Auto` kernels; for VP-3 to VP-5, the release recipe (`x86_64-unknown-linux-musl`, as `docs/design/release.md` builds it), so those bars measure the binary users get. FFmpeg 9.0.2 as above.
- **VP-1 and VP-2.** Each held-out file's video stream is copied without decoding into IVF (`ffmpeg -i FILE -map 0:v:0 -c:v copy -f ivf`), and both sides read the same IVF bytes (the SHA-256 of each is in the report). Both decode the first `F` frames in decode order, `F` by class: 1,800 below 921,600 samples, 600 up to 2,073,600, and 150 above, or the whole stream where it has fewer. Pairings: `discard` (ours decoding without output, against `-f null`) and `write` (ours writing raw planes to standard output, against `-f rawvideo -fps_mode passthrough`). One warm-up of every command of a file, then interleaved rounds, every command of the file once per round, in an order shuffled with a fixed seed: 5 rounds at one thread (`ffmpeg -threads 1` and ours `--threads 1`, both pinned to core 2), and 10 rounds at eight threads (`-threads 8` and `--threads 8`, unpinned). No minimum duration, and no repetition of a command within a round. A file's ratio is the median of ours over the median of FFmpeg's; the statistic is the median over files with a 95% bootstrap interval, 2,000 resamples, seed 20261101.
- **VP-3.** The method of the release-build comparison in coreav-dev/coreav#182, unchanged but for the arm: `tools/decode-oracle` with that comparison's published harness changes (no packed copy, no MD5) decodes each B5 file whole in one process, the native backend at its default thread count (one per core, 4 on the host), against `ffmpeg -c:v vp9 -i FILE -f null -` at FFmpeg's default threads. The page cache is primed before each file; 3 rounds in rotated order; a file whose FFmpeg wall time is below 2 seconds is timed as a unit of repeated invocations, of the same size for every arm; CPU is user plus system time from `wait4`, summed within a unit; a file's ratio uses the median CPU over rounds; the interval resamples the ten ratios 10,000 times, seed 518. The large class is the three files of 3840x2160 and above.
- **VP-4.** B5's method (`docs/design/transcode.md`, "Bars", B5) on the ten B5 files, with the native backend first at its default thread count, one per core.
- **VP-5.** The thumbnail method (`docs/design/thumbnails.md`, "Bars") on the 28 held-out files, through the native backend at its default thread count.

**Hosts.** VP-1 on the Compute Engine one-thread class the H.264 rounds use, calibrated (A/A and null arm) as `docs/design/h264.md` records; VP-2 on `c4-standard-16`, SMT off, one thread per core; VP-3 to VP-5 on `c4-standard-8`, SMT off, the host class of the earlier VP9 runs. Shared runner pools only count instructions; they never time a bar.

### Threading (milestone 7)

Committed now and judged on the pull request that completes milestone 7, on the development files and the vectors only:

- **T1, bit-exact.** Every vector and every one of the roster's 74 files, decoded with 1, 2, 3, 8 and 16 threads (`Auto` kernels, x86-64; the vectors also on arm64), gives the same frames as one thread, frame for frame; and a `threads` fuzz target decodes each input with 1 and 3 threads and fails on any difference, in CI with the crate's other targets.
- **T2, the cost on one thread.** One thread against the parent commit of the milestone's first pull request, by VP-1's method on the 46 development files: median ratio at most **1.01**, both pairings.
- **T3, the scaling.** On `c4-standard-16` with SMT off, both sides at 8 threads and at 1 in the same session, by VP-1's method on the 46 development files: per file `(ours_8 / ffmpeg_8) / (ours_1 / ffmpeg_1)`, median at most **1.00**, both pairings. Where VP-1 is at parity, VP-2 can pass only if the threading scales at least as well as FFmpeg's, so this bar is parity, not H.264's 1.10.

**Ceiling first.** Before any optimisation round is built, its ceiling is measured in a throwaway build that deletes the code the candidate would remove, on a few training files, by instruction count. A candidate whose ceiling cannot clear its bar is rejected before it is built. A run that misses a bar is final for its arms; it runs again only after a recorded host fault.

## Risks

| Risk | What it would cost | Mitigation |
|---|---|---|
| The specification is a draft (0.7) and may differ from what encoders produce | wrong frames on some streams | Published MD5s and FFmpeg both gate every milestone; a difference from the specification is recorded with the vector that shows it |
| The similarity guard flags specification tables and pseudocode, which every decoder shares | allowlist entries the maintainer reviews before a pull request can merge | List every table as a specification item from the start; keep the code's structure our own |
| Speed: FFmpeg's VP9 decoder is fast on its hot paths | VP-1 and VP-2 missed for several rounds, as happened with H.264 | The checkpoint after milestone 5 sizes the gap; AVX2 kernels and threading are planned, not optional |
| The boolean decoder bounds the speed and safe Rust is too slow for it | a ceiling above the bar | Measure its ceiling first; any `unsafe` there needs a new policy record |
| Memory: eight references of 8K frames, 16-bit samples from milestone 5 | an upload could exhaust a host | The memory budget is checked before each frame of a new size; the boundary sets the limits |
| Threading changes output | frames that differ between runs | Bit-exact for every thread count is a gate of milestone 7 |
| The release build's toolchain costs time on its own: the release's musl libvpx takes 1.247 times the CPU of a GNU build of the same version (coreav-dev/coreav#182) | VP-3 measured on a GNU build would not hold for users | VP-3 is measured on the build users get; frame buffers are pooled so the allocator is off the hot path |
| `coreav-cli`, `coreav-transcode` and `coreav-thumbnail` change under other work | merge conflicts at the backend milestone | The backend touches them in one pull request, merged with current `main` first |

## Decisions

- **A decoder of our own, not a faster libvpx path.** Rejected: tuning the libvpx build, since the measured build changes do not close the gap on the large files (coreav-dev/coreav#182). Rejected: an existing Rust VP9 crate, since the project's decoders are written from the specification, in its own crates under its own gates.
- **Three crates, as for H.264.** The kernels are a separate crate so that `unsafe` is confined to one crate's SIMD modules (policy record 0001's kernel-crate exception) and the decoder crate stays `forbid`. The backend is separate because it is separately packaged and the decoder stays free of the boundary's types. Rejected: sharing `coreav-kernels-h264`; the two codecs' kernels differ in every family, and the similarity guard compares each crate with its own codec's sets.
- **In the calling process, not in the worker.** The decoder is safe Rust like the H.264 decoder, which runs in the process, and the worker's copies are a large part of the B5 gap.
- **Opt-in first.** The patent position is the H.264 decoder's, and so is the packaging; the default is the maintainer's.
- **The loop filter before inter prediction**, because no oracle exposes unfiltered frames (above).
- **Test vectors from the publisher's bucket listing.** The names come from the storage listing, not from any implementation's repository, and each file is pinned by SHA-256.
