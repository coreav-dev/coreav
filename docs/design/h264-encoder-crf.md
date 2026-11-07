# H.264 encoder constant-quality rate mode

Status: implemented behind the crate's `full` feature. This is a library API, not a product command.

## Quality scale

`Settings::rate_mode` selects `RateMode::FixedQp(qp)` or `RateMode::Crf(crf)`. Both accept integers 10 through 51, matching the encoder's admitted quantizer range. `RateMode::default()` is fixed QP 26. This replaces the pre-release `Settings::qp` field without changing fixed-QP output. Smaller CRF values request finer quantization and usually more bits; CRF is neither a bitrate nor a decoder quality guarantee. CRF 10 is not lossless.

CRF N uses QP N as its neutral P-picture quantizer. A cheap source-only luma activity pass selects a correction in -1 through 2; I pictures receive an offset of -1 and P pictures receive zero. The combined adjustment has a floor of -1, so low activity and refresh preference never stack two quality boosts. The final picture QP is clamped to 10 through 51. Keeping the correction small makes CRF N comparable to fixed QP N rather than introducing a different numeric scale. It does not promise equal bits on every sequence, particularly at the endpoints or in all-intra mode. A five-input x86-64 reference run compares CRF and fixed QP at 18, 23 and 28, reporting complete emitted bits and weighted PSNR, to check this mapping rather than impose a pass threshold.

## One decision before picture encoding

Divide the visible luma into 8 by 8 blocks, including partial edge blocks. In each block, compute the integer mean, then sum absolute differences from that mean. Divide the total SAD by the visible sample count, rounding down, to get activity A in 0 through 127. This is a source-only DC-prediction SAD: it needs two bounded luma reads per sample, no motion search, allocation, reconstruction or extra full encode. It is insensitive to uniform brightness changes and independent of the chosen CRF, reference quantization and worker scheduling.

The activity correction is -1 for A < 8, zero for 8 <= A < 12, +1 for 12 <= A < 24 and +2 otherwise. These bounded, approximately logarithmic bands give smooth pictures finer quantization and textured pictures coarser quantization. The 8 through 11 band defines the QP-compatible scale. The I-picture offset gives refresh pictures a small quality preference without reserving bits. Activity is measured in source-pixel units, not a resolution-invariant perceptual model: resizing the same scene can change its band. The scale and bands are intentionally modest; this first controller does not claim perceptual equivalence across arbitrary content or compatibility with another encoder's CRF scale.

Compute one immutable `Slice::qp` before entering the existing wavefront. Prediction and coefficient search, quantization, slice syntax and deblocking consume that same QP. PPS initial QP remains 26 and macroblock QP deltas remain zero. Fixed QP bypasses the activity pass entirely. Integer accumulation and QP conversion are checked. No floating-point operations, unsafe code, retries or rate-control state are introduced. Failed calls retain the existing transactional sequence/reference contract.

## Determinism and limits

Activity reads visible source samples in a fixed order, without threads or reconstructed neighbours. All workers see the same immutable picture QP. Identical pictures and settings therefore produce identical NAL bytes and reconstructions with 1, 2 or 8 workers, including cropped dimensions, deblocking and IDR boundaries. No lookahead, B pictures or per-macroblock adaptive QP belong to this unit; motion-aware complexity and more finely calibrated perceptual allocation are later work.

The [picture-budget design](h264-encoder-rate-control.md) remains unimplemented. Its hard cap, excess-bit accumulator and transactional QP trials solve a different problem: CRF has no target size, cap, buffer, ABR, VBV or HRD promise. Existing resource and macroblock admission limits still refuse encoding instead of silently increasing QP.

## Verification

- Capture fixed-QP NAL snapshots on the base commit before implementation, then require the same bytes afterward.
- Check clean boundary refusal at 9, 52 and 255 and acceptance at 10 and 51 for both modes.
- Independently parse emitted picture QPs and check activity bands, I/P offsets and endpoint clamping. Higher CRF must never select a lower QP for the same source and picture type; sequence-level size and distortion trends use widely separated values, not strict monotonic size at every QP.
- Compare every visible reconstructed plane with CoreAV, pinned FFmpeg obtained from `test_oracle.require`, and the existing pinned JM decoder, across CRF endpoints, representative values, crops, chroma, IDR/P transitions and enabled deblocking.
- Check 1/2/8-worker NAL and reconstruction identity, including a failed call followed by retry. Retain scalar/automatic SIMD identity coverage.
- Measure the same five neutral-labelled inputs at CRF 18/23/28 and fixed QP 18/23/28 on GitHub Actions x86-64 with explicit candidate and base SHAs. Report bits, sample-weighted Y/Cb/Cr PSNR and encode-time ratio; these are reference numbers, not a speed or quality gate.

This design uses only the existing encoder and public specification concepts. No other encoder's source or behaviour was studied.
