# coreav-h264

An H.264 (AVC) decoder in Rust: High profile, 8-bit 4:2:0 and monochrome
(4:0:0, given as 4:2:0 with neutral chroma), progressive frames, written
from Rec. ITU-T H.264 (08/2024).

## Read this first

**This decoder is opt-in and is never part of a default CoreAV build.**

Decoding H.264 may require a patent licence depending on where you are and
what you do with it. Patents essential to H.264 are still in force in some
jurisdictions, and pool licences exist for decoders as well as encoders.
CoreAV therefore ships this decoder as a separately packaged backend that an
operator enables deliberately, behind a non-default Cargo feature
(`docs/design/h264.md`, "Packaging"). CoreAV's default H.264 path is unchanged: the
sandboxed FFmpeg fallback. An NVDEC backend is planned but not built yet
(`docs/design/decode.md`, "NVDEC").

Nothing here is legal advice. Deciding whether you may use this decoder is
yours to make.

## What it decodes, when it is complete

- Decodes the streams a High profile decoder must decode (clause A.2.4),
  with `chroma_format_idc` = 1, 8-bit samples and `frame_mbs_only_flag` = 1:
  CAVLC and CABAC, I, P and B slices, multiple slices per picture, long-term
  references, all three picture order count types, the 8x8 transform and
  scaling lists.
- Rejects what it does not decode with an `Unsupported` error rather than
  returning a wrong picture: field pictures and MBAFF, 4:2:2 and 4:4:4, bit
  depths above 8, lossless transform bypass, data partitioning, SP and SI
  slices, slice groups, and the SVC, MVC and 3D-AVC extensions.

## How it is built

- **Sans-IO.** It takes NAL units and returns pictures. It opens no files,
  makes no network requests and starts no threads unless asked to, so the
  same code runs in a WebAssembly build and inside a sandboxed worker.
- **Threads on request.** `Decoder::with_threads(n)` decodes up to `n`
  pictures at once, one thread each, and gives the same pictures, sample
  for sample, as one thread.
- **Bounded.** Every entry point takes a `Limits` value; hostile input gives
  an error, never a panic or unbounded work.
- **Safe Rust for everything that touches the bitstream.** Parsing, CAVLC,
  CABAC and the control code are safe Rust: every module but one is
  `#![forbid(unsafe_code)]`. The pixel kernels live in `coreav-kernels-h264`,
  which may use SIMD intrinsics; they are called only through views that
  have already checked every length, stride and range, and every `unsafe`
  site is listed in `docs/unsafe-sites.md`.
- **An opt-in x86-64-v3 build.** A build without flags targets the
  x86-64 baseline and picks its SIMD kernels at run time. Built with
  `RUSTFLAGS="-C target-cpu=x86-64-v3"` (AVX, AVX2, BMI1, BMI2, F16C, FMA,
  LZCNT and MOVBE, on top of x86-64-v2), it gives the same output, and
  the binary runs only on CPUs of that level. Measured on the decoder of
  `a7d8e4b` on a Xeon of the 2.90 GHz class, one thread, decoding without
  writing the pictures, it took 0.965 of the baseline build's time at the
  median over the training set (`docs/design/h264.md`, "Round 20").
- **Two `unsafe` sites of its own, in the default build.** The hot
  functions of the slice data (the CABAC and CAVLC parse of each
  macroblock, the residual blocks of both, and motion compensation's
  block prediction) are compiled twice, for the x86-64 baseline and for
  x86-64-v3, and on a CPU with the v3 features the decoder runs the v3
  instance; the pictures are the same either way. Calling a function
  compiled for features the caller is not compiled with is `unsafe` in
  Rust, so those calls are hand-written `unsafe`: one named site,
  `v3-entry`, in `src/fast/`, sound because the decoder makes
  them only after detecting every feature at run time. It is checked by a
  differential test and fuzz target against the baseline instance and by
  the conformance suite in both builds, and listed with its precondition
  in `docs/unsafe-sites.md`. Build without the default feature `fast`
  (`--no-default-features`, for a direct dependency on this crate) for a
  decoder that compiles no `unsafe` of its own and runs the baseline
  instance only; `Decoder::with_x86_64_v3(false)`
  keeps the baseline instance in the default build.
  In the v3 instance, the CABAC engine's DecodeDecision is one block of
  hand-written assembly (`asm!`), the site `decision` in
  `src/fast/decision.rs`: it reads two fixed tables at indices bounded by
  construction, writes no memory and never touches the slice data. It is
  checked against its safe twin for every context state and codIRange,
  by the same differential tests and fuzz target, and by the conformance
  suite in both builds; the build without `fast` and the baseline
  instance run the twin.

## Status

In progress, as the graduation of the native H.264 roadmap track. The
crate decodes the scope above: parameter sets, slices, CAVLC and CABAC,
prediction, transforms, deblocking and the decoded picture buffer, with
the pixel kernels in `coreav-kernels-h264`. It
decodes the 99 in-scope JVT conformance streams bit-exactly (at one, two
and eight threads), and reaches users only as an opt-in backend
(`docs/design/h264.md`, "Packaging").

The design, the scope and the correctness and speed bars this decoder is
measured against are in [`docs/design/h264.md`](../../docs/design/h264.md).
The crate is not published to crates.io.

Licensed under MIT OR Apache-2.0, like the rest of CoreAV. That covers
copyright, not patents.
