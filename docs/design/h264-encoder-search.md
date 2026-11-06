# Bounded H.264 encoder search

Status: design committed before implementation. This replaces the normal P-picture search policy, not H.264 reconstruction, the all-IDR path, or default-build admission. Encoded bytes may change. The goal is a large reduction in search work rather than another exact arithmetic optimization.

## Inputs and constraints

The specification is Rec. ITU-T H.264, version 15, August 2024. Clauses 7 and 9 define syntax and bit counting, 8.3 and 8.4 define prediction, 8.5 defines reconstruction, and Annex A defines admission. The reviewed [encoder speed note](../technique-notes/h264-encoder-speed.md) and [partition-search note](../technique-notes/h264-partition-search.md) are allowed prose inputs. No other encoder source, unreviewed study report, or new source study is needed for this design.

The previous [profile and exact-search round](h264-encoder-p.md#partition-search-cost-design) assigns about 89% of sampled process cycles to partition search. Cheap probes, full-RDO finalists, and preparation all scale with the number of searched partitions. Removing only arithmetic cannot remove those multiplicative factors. The remaining intra work also prevents a tenfold improvement even if inter search disappeared, so this design addresses both normal P-picture paths.

Keep the opt-in feature, safe scalar Rust, fixed QP, GOP, colour signalling, filtering, shared syntax traversal, and transactional sequence state. A lower-cost search decision never bypasses vector, coefficient, consecutive motion-count, macroblock-bit, or output-budget admission. Preserve all existing legal prediction and syntax support for conformance fixtures.

## Step 1: propose complete inter layouts cheaply

Search only P_L0_16x16 and P_8x8 with four 8x8 partitions in normal mode decision. Do not search 16x8, 8x16, or smaller subpartitions. Those tools remain representable and independently tested, but no longer multiply the ordinary search tree.

For each partition, score zero, the normative predictor, and its rounded integer position. Starting at the best integer seed, test four axial neighbours at integer steps of four, two, and one luma samples, with one fixed-centre pass per step. Then test four axial half-sample neighbours and four axial quarter-sample neighbours. Keep the best result after each stage. This is at most 23 probe calls per partition, or 115 across both layouts, including seeds and duplicates. Predictors outside the local range remain legal seeds. Reject, never clip, invalid vectors or differences. Retain deterministic cost, motion-length, and vector ties.

Build each layout greedily in syntax order with a private motion map. Rank complete layouts by luma SAD plus the existing SAD-scale penalty times complete inter-prediction syntax length. Count that syntax after deriving all motion differences. Do not double-count motion lengths already present in partition scores. This is a cheap proxy, not full RDO or a lower bound on it.

Run complete reconstruction and exact syntax counting only for the best cheap layout. If normative coefficient or macroblock admission rejects it, try the other admitted layout. Always evaluate the normative skip candidate independently. An exact zero-distortion skip can terminate normal inter search. Otherwise compare the completed regular inter candidate with skip using the existing full candidate objective. At most two regular complete inter evaluations are needed, not several macroblock reconstructions for every partition of every layout.

The alternative of ranking every old layout cheaply still performs many redundant searches. Retaining only whole-block motion would be cheaper, but gives opposing local motion no representation. Whole-block plus four 8x8 regions is the initial compromise to measure rather than a claim of optimal quality.

## Step 2: postpone and bound P-picture intra work

Evaluate inter before intra in P pictures. An admitted exact skip can finish the macroblock without constructing discarded intra candidates. This is a deliberate mode policy, not a proof that intra cannot use fewer bits.

Otherwise rank legal I_16x16 luma modes and legal chroma modes by prediction SAD, without residual transforms or reconstruction. Complete only the best luma/chroma pair and compare it with the admitted inter candidate using exact full RDO. Normal P-picture selection omits I_4x4 analysis. IDR pictures keep the current complete intra search, including I_4x4, so the unchanged all-intra byte snapshot remains valid.

If the selected cheap intra pair is unrepresentable, use the existing complete intra procedure as a bounded admission fallback. This preserves the zero-motion-count escape when an inter layout is forbidden. The ordinary path has one skip evaluation, at most one admitted regular inter evaluation, and one complete intra pair. Refusal paths can additionally try the second inter layout and the existing finite intra fallback. No partial candidate may change committed neighbours, reconstruction, or sequence state.

The competing design is a content threshold that omits intra whenever inter error looks small. It avoids more work but introduces a poorly justified threshold. Cheaply choosing one intra pair keeps a complete intra comparison without that threshold.

## Reference comparisons and verification

Commit this design first on a fresh branch from main. Measure step 1 against main, then step 2 against step 1 and main. Keep only steps whose speed benefit repays their observed quality and rate cost. These are reference comparisons without acceptance bars, not claims at matched quality.

Use the existing 640x480 clip and at least one additional existing R2 clip. Prepare 60 YUV420 pictures at 30 pictures per second with the pinned FFmpeg. Preserve native geometry where admitted and record any conversion, prepared hashes, and lossy input status. Use QP 28, GOP 32, one reference, enabled zero-offset deblocking, and one pinned CPU. Run x264 medium with the same previous constrained-baseline, single-thread, fixed-GOP, no-B, no-lookahead, unweighted settings and fixed QP offsets. Report each arm's fps, stream bitrate, PSNR, and SSIM against the identical prepared source. Keep timing and metric work separate. Record complete run failures rather than averaging incomplete output.

The existing raw-picture driver verifies CoreAV reconstruction while encoding. Independently decode every measured stream with pinned FFmpeg and compare every visible Y, Cb, and Cr byte to encoder reconstruction, with exact frame counts and no decoder warnings. Rate and quality may differ from main. Encoder reconstruction and the normative decoder must not differ.

Add tests for probe and full-evaluation bounds, small-range and outlying seeds, deterministic ties, layout admission, cheap mode legality, refusal fallback, and transactional retry. Keep existing tests intact. The existing forced-layout fixtures and three-decoder CI still cover every supported syntax form, including forms omitted by normal search. Run the exact-head encoder similarity guard using the current main allowlist before review.
