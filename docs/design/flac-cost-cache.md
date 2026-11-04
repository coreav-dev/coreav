# Byte-identical FLAC cost reuse

This plan precedes implementation. It targets a reduction in CoreAV's current encoder CPU cost without changing encoded bytes. It is an intermediate CPU milestone, not a prediction that this change alone reaches FFmpeg parity. The size and reference-speed bars in [Native FLAC output](flac-output.md) remain unchanged.

The before-change source is `cbe3f6db46668fbd910c8a3a21d92802fe759e3a`. Its `crates/coreav-audio/src/flac/encode.rs` blob is `8626b99fb83072129160cf99f35ff2805398c3a0`, with SHA-256 `a12f68790e31ce8bbb4567ee86f750b7ae99f04cdadf7da286597519ddfac665`. The registered before-change arm restores this exact encoder, not a later or selectively altered baseline.

## Evidence before choosing the change

The [sanitized diagnostic records](../measurements/flac-cpu-diagnostic.json) contain 380 timing/counter records from four already-used synthetic cases: mono 16-bit tones, stereo 24-bit tones, stereo 24-bit polynomial signals and stereo 16-bit noise, all 30 seconds at 48 kHz. They are diagnostic coverage, not held-out evidence. Do not generalize their ratios to an unseen input population.

All work ran remotely on on-demand c4-standard-8, Intel Emerald Rapids, SMT disabled, STANDARD PMU and ALL_CORE_MAX, with each process pinned to one guest CPU. Plain commands use optimized release builds. Native source profiles use an optimized build with debug information and frame pointers; its separate timing/instruction controls are retained. FFmpeg 9.0.2 is the pinned executable only. No FFmpeg or libFLAC source is an implementation reference.

The primary reference retains the registered default options. A separate diagnostic reference uses compression level 5, 4,096-sample frames, Levinson prediction, precision 15, LPC orders 1–8 with exhaustive order search, partition orders 0–4 and exact Rice parameters. These align exposed bounds and Rice calculation, not every internal encoder decision. Both references remain visible; the more expensive aligned reference does not replace the default reference or its bar.

Five alternating/reverse-order repetitions per arm record a wall invocation and a separate hardware-counter invocation. Every accepted invocation passes complete samples, CRC, MD5 and probe checks. Plain and instrumented native outputs must also be identical complete files. All hardware-counter lines were fully scheduled. Native source recordings report no lost samples. Profiles use native symbols/source lines only.

Approximate cycle buckets, in case order:

| Native symbols | Mono tones | Stereo tones | Polynomial | Noise |
| --- | ---: | ---: | ---: | ---: |
| Candidate evaluation, including Rice and residual generation | 80.00% | 84.82% | 87.51% | 87.29% |
| Planning outside candidate evaluation | 5.28% | 5.80% | 4.04% | 6.22% |
| Writer symbols | 6.88% | 4.39% | 2.64% | 2.11% |
| MD5 symbols | 1.82% | 1.45% | 1.69% | 1.17% |
| Other or unassigned | 6.02% | 3.54% | 4.12% | 3.21% |

Inlining prevents calling the first row Rice alone. Three throwaway builds therefore replace computation with recorded decisions while requiring every native output byte to remain unchanged:

| Erased work | Plain/replay range | Replay/default reference | Replay/aligned reference |
| --- | ---: | ---: | ---: |
| Rice planning | 2.715–3.057 | 2.197–4.870 | 1.126–2.040 |
| Complete subframe planning | 6.446–14.771 | 0.932–1.212 | 0.352–0.479 |
| Complete frame encoding | 14.441–25.907 | 0.422–0.575 | 0.188–0.239 |

Rice replay retains every parameter width, partition order, parameter and cost. Subframe replay additionally retains predictors, coefficients, shifts and residuals. Frame replay retains encoded frames and the same frame-count/minimum/maximum bookkeeping. Replay reads and materialization remain timed; subframe replay also reconstructs input samples. These are measured erasure experiments, not literal zero-cost bounds or usable encoders. Absolute times from different VMs are not a causal before/after comparison.

Each replay must consume exactly its complete tape. Hook-off/plain median instructions and wall time must each be at most 1.05 in every case; all controls pass. The early Rice/subframe builds compiled their variants and passed complete-output gates, but omitted library unit tests before capture. Their provenance retains that omission. The exact variants subsequently passed 56 library tests each; the final frame variant passed those tests before capture. Earlier tooling failures remain separate from these complete captures.

The observed limiter is repeated encoder planning work, not a multi-fold source/MD5 floor. Rice-only erasure still misses both reference arms in every case, so it is rejected as a parity fix. A cheap standalone writer or MD5 change also cannot remove most of the current cost. The selected work below is a byte-preserving CPU step whose reference-parity outcome must still be reported honestly.

## Selected design

`Rice::plan` remains the single owner of representation selection. Add one private block-local cost cache, discarded when that candidate's residual changes:

```text
RiceCosts<'a>: borrowed residuals, legal leaf geometry, lazy quotient sums, widths
quotient_sum(&mut self, order, partition, k) -> u64
escaped_width(&self, order, partition) -> u8
```

The bottom level is the largest legal partition order, at most four. It has at most 16 leaves. The first leaf alone excludes predictor warm-up samples. Every coarser partition is a union of complete leaves, including short final blocks. For each requested parameter `k` in 0–30, compute its at-most-16 leaf quotient sums once and reuse their integer sums for all coarser orders. Cache folded maxima/widths once per leaf as well. The quotient cache is bounded by 31 × 16 × 8 bytes, about 4 KiB, not input duration; no whole-file or per-sample prefix matrix is added. With at most 4,096 residuals, quotient sums and charged costs fit `u64`.

Compute the unrestricted five-bit Rice optimum once per partition, retaining the lowest parameter on a tie. Its clipped value at 14 is also the four-bit optimum: for `C(k) = n(k + 1) + sum(v >> k)`, the discrete difference is nondecreasing, so the constrained minimum is the unrestricted minimum clipped to the legal interval. Charge each method's own headers and compare its Rice/escape choices exactly. Keep Rice on an escape tie, lower partition order on a complete tie, and the four-bit method on a complete method tie. Width 32 remains unescapable. Keep the signed residual domain unchanged.

Two cache shapes were considered. A lazy per-sample prefix matrix also answers ranges exactly but can retain roughly a megabyte per candidate and adds unnecessary allocation/initialization. The bounded leaf cache exploits the existing power-of-two partition tree and is selected.

For LPC autocorrelation, interleave independent lag accumulations across the block rather than scanning the full signal separately for every lag. Preserve each lag's product operands, summation identity and addition order. Do not reassociate a reduction, use fused arithmetic, change the parabolic window, coefficient rounding, shift, candidate order or search bounds. Keep the existing scalar shape for short tails where necessary. Exact floating-point-bit comparisons against the former scalar definition precede complete-file tests.

## Predictions and fixed scope

These are predictions to test, not measured implementation results. They are not multiplied together:

| Candidate | Predicted component gain | Predicted whole-command effect |
| --- | --- | --- |
| Share four-/five-bit planning alone | About 2× Rice work | About 1.4–1.6×; insufficient alone for this milestone |
| Lazy leaf quotient/width reuse, including shared methods | About 4–10× Rice work when parameters repeat across partitions | About 1.9–2.4× |
| Independent-lag autocorrelation scheduling | About 2–4× autocorrelation work | About 1.02–1.05× alone; included only in the fixed bundle |
| Combined selected bundle | Not a product of the rows above | About 2.0–2.6× current CoreAV |

The estimate uses the measured Rice erasure and the much smaller outside-evaluation planning bucket. Parameter diversity, initialization and compiler scheduling can reduce realized gains. The bundle contains the leaf cache/shared-method change and exact autocorrelation scheduling; do not select arms after observing a judged result. Preserve all fixed/LPC search bounds, exact costs and ties, quantization, wasted bits, stereo assignment, framing, source admission, metadata, MD5, limits and atomic publication. No unsafe code, new dependency, encoder option, decoder change or output-byte exception is included. Residual-loop redesign, heuristic predictor rejection and output-stage changes are not part of this pull request.

## Verification and bars fixed before implementation

Correctness is mandatory: preserve every existing test, independently check cached sums and complete optimal Rice choices against scalar/exhaustive definitions, cover every legal partition/order boundary and both parameter methods, and compare autocorrelation `f64` bits with the old reduction for every lag and short-block boundary. Retain all 1,152 complete-file comparisons against `e3d1b79286c0764fed27fd9060093f881c3ebd52`, full native/independent decoding, exact CRC/MD5/probe checks, all 93 CLI comparisons and the unchanged tiny-reference rule. Keep the existing startup instruction/linkage guard. Test and clippy the actual changed build, not an uncompiled feature. No encoded-byte change is acceptable.

The additional development milestone is candidate/before-change CoreAV median elapsed time at most 0.60 overall and in every registered synthetic class. This is a CoreAV-to-CoreAV CPU milestone, not a replacement reference-speed bar. The predicted bundle should clear it; a miss remains a miss. The original total size ratio at most 1.05, each compressible-case size ratio at most 1.10, and native/default-reference speed median at most 1.00 overall and per class remain unchanged. Reference parity is not predicted from the intermediate milestone, and its miss must not be reported as a passing speed claim.

After this plan and implementation merge together, register the exact merged-main source, encoder-before/after hashes, compiler/build flags, executable hashes, host contract and tool hashes in Git before dispatch. Use the frozen 24-case generator/roster, one warm-up, nine alternating pairs, equal case weighting, seed-zero 10,000-resample intervals, the existing 0.90–1.10 A/A condition and reference-work >= five-times-null condition. For repetition `i` in 0–8, alternate the candidate/reference order as before and insert the before-change arm at position `i modulo 3` in that pair. This gives three baseline calls at each position without changing the alternating candidate/reference order. Compute both ratios from each complete triple, and resample complete triples rather than mixing calls. Build the before-change arm from the same merged source/build environment with only the encoder restored to its before-change blob, preserving all other CLI/decoder code; record and test that source variant before freezing it. Both native arms must emit identical files.

Execution uses on-demand C4 with SMT off and pinning, never the development machine or a shared-runner timing bar. The source/registration must identify merged main; the earlier Actions-only registration and driver remain byte-unchanged as historical records. Host or tooling failures are distinct from a completed bar miss. Never retry a completed miss or change the bars after results. Report every default-reference ratio, interval, faster-case share, calibration and failed invocation, whether or not the CoreAV milestone passes. These already-used synthetic inputs cannot establish a held-out faster-than-FFmpeg claim. Results are a separate pull request after the registered merged-code round.

The first commit contains this plan and its diagnostic records. No product implementation or new judged run precedes it. Plan review precedes implementation; both stay in the same pull request.
