# Deterministic H.264 encoder row wavefront

## Contract

The encoder chooses the same macroblocks, writes the same stream and returns the same reconstruction for every positive thread count. The first change removes raster-order decision state; its quality cost is measured against main separately from parallel execution. There are no presets, new dependencies or unsafe code.

`Encoder::new(settings, limits)` selects the available core count, falling back to one. `Encoder::with_threads(settings, limits, threads)` selects a positive count explicitly. Workers are capped at the coded macroblock-row count and the row's two-column dependency steps. These constructors do not change the CLI or the sequence settings written to the stream.

## Decisions before concurrency

Candidate rate costs use a fixed local skip-run estimate rather than the actual pending raster skip run. The real skip run remains owned exclusively by the final serial writer. Syntax lower bounds and exact candidate counting use the same fixed estimate; changing thread count cannot change their inputs.

Motion admission retains the limit of sixteen vectors per consecutive macroblock pair. Within a row, the predecessor is the committed left macroblock. At each row boundary, both endpoint macroblocks are capped at eight vectors. The first macroblock of the picture also checks the count carried from the preceding successful picture. A one-macroblock-wide row satisfies both endpoint constraints. Intra remains available when no inter representation is admitted. No sequence state is committed on failure.

## Ownership and publication

The picture owns one bounded table of `OnceLock<Candidate>`, indexed in raster order. A candidate contains its selected samples, syntax and neighbour state. A slot is published once by its row's worker after a macroblock finishes; other workers only borrow already-published slots. Reconstruction planes and entropy output are not shared mutable state. A slot occupies 4,976 bytes on the measured 64-bit build, or 40,604,160 bytes at the maximum 1920 by 1088 coded size.

Each macroblock builds its input tile from the caller's immutable picture and its prediction border from the selected, unfiltered tiles of its left, top-left, top and top-right neighbours. These are precisely the samples and metadata the current picture loop reads. It does not read deblocked pixels or partially written picture planes.

An alternative that locks the complete picture around each macroblock would preserve the old data layout but serialize its expensive decisions. Separate mutable row planes avoid that lock but need additional ownership machinery for top-row readers. Immutable macroblock publication exposes the dependencies directly, at the cost of retaining selected candidates until picture assembly. The table is bounded by the existing coded-pixel limit; its size and the resulting peak memory are reported with the implementation.

## Scheduler

A scoped worker takes one complete row at a time from a monotonically increasing atomic row counter. Macroblocks within that row run sequentially. Macroblock `(r, x)` is ready when its left neighbour is complete and row `r - 1` has completed at least `min(x + 2, row_width)` macroblocks. The first row has no top dependency. The last-column condition uses the row end rather than an unavailable top-right neighbour.

Each adjacent row pair has a bounded standard-library channel with capacity equal to the row width. The upper row publishes its candidate before sending its completed-column count. The lower row receives until the required count is reached, then borrows the immutable slots. Channels cannot fill beyond their row's bounded message count, even when the lower row has not been assigned or has already failed. A receiver mutex is taken once by its only row reader, not per macroblock. Worker count is capped by both the row count and the number of two-column dependency steps in a row. At one thread, the same decision function runs in raster order without channels or thread creation.

A row failure sends its error to the next row instead of a completion count, so downstream rows stop before reading an unpublished dependency. Earlier rows continue to completion or their own failure. After joining the workers, outcomes are read in row order to select the first raster error. The picture is discarded on error. Worker creation, channel disconnection, poisoned coordination and worker unwind become bounded encoding errors rather than hangs. Running work may finish after another row fails, but cannot mutate encoder sequence state.

## Entropy coding and reconstruction

Exact candidate counting already derives the neighbour state. The selected candidate retains that state rather than counting its syntax again for publication. After all decisions succeed, a serial raster pass writes macroblock syntax with the real skip-run prefix and commits each selected tile to the tightly packed reconstruction planes. CAVLC neighbour contexts are derived from the same immutable completed neighbours used for candidate counting. Skip runs are never concatenated across independent entropy buffers.

Deblocking runs only after the raster assembly pass and retains its existing order and neighbour records. Visible cropping follows deblocking. The finished reference and sequence counters are committed only after the existing access-unit and total-byte limits succeed.

## Verification

- Compare the one-thread raster-independent decision build with main on five prepared inputs over the quality curve; each input must stay within a +0.5% luma BD-rate cost.
- Compare complete streams and reconstructions at one, two, three and eight threads, with both dispatched and scalar kernels. Decode every measured stream through the pinned oracle to the reconstruction.
- Exercise narrow and cropped pictures, row endpoints, cross-picture motion budgets, failure and rollback, blocked dependencies, reordered completions and repeated calls. Test that one-thread and parallel refusals agree and worker failure cannot strand the scheduler.
- Measure wall-clock encoding at one, two, four and eight threads, recording the available CPU count and identifying oversubscription. Build baseline and candidate in separate target directories at explicit full commits. Keep decoding outside encoding timing.
- Run the encoder tests, strict Clippy, workspace formatting and unsafe-site checks, public-text checks and exact-head similarity guard. Independent review covers publication order, all scheduler termination paths and the normative pair limit.

This implements the row-wavefront decision and admission scheme in [the pipeline roadmap](h264-encoder-pipeline.md). SIMD changes remain separate.

## Measured trade-offs

The fixed local skip-run estimate and row-endpoint admission change the one-thread stream. Against the prior raster-dependent encoder, luma BD-rate changes on five prepared inputs over QP 22, 27, 32, 37 and 40 range from -0.543% to +0.002%. Every input stays below the +0.5% limit. This is a reference comparison on existing lossy media, not a general compression claim.

On four-vCPU GitHub-hosted Ubuntu runners, seven alternating encode-only calls per input at QP 27 give 1.34–2.01 times the prior encoder's one-thread speed with four workers. One worker runs at 0.964–1.000 times the prior speed. The one-thread overhead retains immutable selected candidates and serially emits their syntax after all decisions, rather than immediately writing each macroblock. Two workers give 1.33–1.69 times the prior speed. These figures exclude loading input, writing files and decoder verification.

All five inputs at the five QPs produce identical streams and reconstructions at one, two, three and eight workers in both scalar and dispatched builds. The eight-worker identity check on these runners is not an eight-core speed measurement. Every stream also decodes with the pinned oracle to its reconstruction.
