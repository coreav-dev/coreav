# Bounded H.264 encoder search

Status: implemented after the design-first commit. This replaces the normal P-picture search policy, not H.264 reconstruction, the all-IDR path, or default-build admission. Encoded bytes may change. The goal is a large reduction in search work rather than another exact arithmetic optimization.

## Inputs and constraints

The specification is Rec. ITU-T H.264, version 15, August 2024. Clauses 7 and 9 define syntax and bit counting, 8.3 and 8.4 define prediction, 8.5 defines reconstruction, and Annex A defines admission. The reviewed [encoder speed note](../technique-notes/h264-encoder-speed.md) and [partition-search note](../technique-notes/h264-partition-search.md) are allowed prose inputs. No other encoder source, unreviewed study report, or new source study is needed for this design.

### Prior art

The two linked technique notes study x264 core 165 at `0480cb05fa188d37ae87e8f4fd8f1aea3711f7ee`, under GPL-2.0-or-later, and OpenH264 v2.7.0 at `1bca62311e447fd9c39fc93de2c4b96cd5ce971c`, under BSD-2-Clause. This implementation uses those notes as prose, without opening either source. The bounded proposal and its constants come from the committed independent design, not either implementation's control flow.

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

## Implementation and reference results

Both steps are adopted. The ordinary inter path uses the two cheap layout proposals and one successful full evaluation, with the other layout retained only for admission failure. The prior broad search, incremental-trial helpers, and omitted rectangular layout constructors remain only in test builds for their existing conformance and complete-evaluator checks. The P-picture intra path runs after inter and completes one cheap-selected pair, with the complete original procedure retained for IDR pictures and admission fallback. All 43 encoder tests pass, including the unchanged all-intra byte snapshot.

The following are single-run reference figures from the same two-CPU AMD EPYC machine, pinned to CPU 0, using Rust 1.99.1 optimized release builds with debug information. Both inputs are already lossy. A retains 640x480 geometry. B is resized from 414x900 to 368x800 for the level-3.0 comparison envelope. Each uses the first 60 prepared pictures after conversion to 30 pictures per second. A has 48 distinct prepared pictures and B has five, so B is a lower-motion check, not a second broad motion sample. Prepared hashes, binary hashes, source commits, and complete per-call wall and CPU times are in the [raw reference record](h264-encoder-search-reference.json).

| Clip | Arm | Pictures/s | kbit/s | PSNR, dB | SSIM |
|---|---|---:|---:|---:|---:|
| A | Main | 1.120 | 346.960 | 45.637606 | 0.998474 |
| A | Step 1, bounded inter | 10.377 | 361.972 | 44.987874 | 0.998169 |
| A | Step 2, inter-first bounded intra | 17.479 | 399.544 | 44.976752 | 0.998128 |
| A | x264 medium | 240.602 | 350.140 | 42.052419 | 0.998240 |
| B | Main | 0.748 | 126.776 | 44.152652 | 0.992678 |
| B | Step 1, bounded inter | 7.893 | 126.776 | 44.152652 | 0.992678 |
| B | Step 2, inter-first bounded intra | 14.128 | 126.776 | 44.152652 | 0.992678 |
| B | x264 medium | 268.226 | 118.308 | 40.896873 | 0.990054 |

Step 1 is 9.267x main on A and 10.548x on B. Its cost on A is 4.327% more rate, 0.649732 dB less PSNR, and 0.000305 less SSIM. B's complete stream is unchanged. Step 2 is another 1.684x on A and 1.790x on B. Compared with main, the final A result is 15.609x throughput at 15.156% more rate, 0.660854 dB less PSNR, and 0.000346 less SSIM. Final B is 18.882x main with unchanged complete output. These tradeoffs favor the requested structural speed reduction, not a matched-quality or rate-distortion claim. x264 remains much faster, and equal numeric QP does not establish equivalent quantization or quality.

Timing uses the existing `p WIDTH HEIGHT 28 60 INPUT STREAM RECON DECODED true 32` driver under `taskset -c 0`. Native timing includes its CoreAV decoder verification and file IO. The x264 arm times pinned FFmpeg with raw YUV420 input, 30 pictures per second, `-preset medium -tune psnr -profile:v baseline -level:v 3.0 -qp 28 -threads 1 -g 32 -keyint_min 32 -bf 0 -refs 1 -sc_threshold 0 -rc-lookahead 0 -weightp 0`, and `-x264-params ipratio=1:pbratio=1:chroma-qp-offset=0:sync-lookahead=0`. It signals BT.709 limited range and left chroma location. Its timing includes startup and stream IO but not decoder verification. The figures are end-to-end driver references, not isolated encoder-kernel costs, and have no repeatability interval.

Pinned FFmpeg 9.0.2 independently decodes every measured native output and matches every encoder reconstruction byte, with exactly 60 pictures and no decoder warnings. Metric work is outside timing. FFmpeg's `psnr` and `ssim` filters compare the complete decoded YUV420 output with the identical prepared source. PSNR is the complete-sequence, sample-weighted average across Y, Cb, and Cr, not the arithmetic mean of frame PSNR values. SSIM is the filter's complete-sequence All result. Each metric retains all 60 per-picture records. Bitrate counts the complete elementary stream, including repeated headers, over two seconds.
