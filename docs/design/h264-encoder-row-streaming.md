# Streaming the H.264 row wavefront

This follows [the row-wavefront design](h264-encoder-wavefront.md). Complete streams and reconstructions must remain byte-identical to the merged encoder for every positive thread count. Candidate costs, neighbour availability, motion admission, deblocking and the public API do not change.

## Ownership and interface

The scheduler calls `compute(address, current, top)` to publish one immutable candidate and `emit(row, current, top)` on the calling thread after that row completes. The emitter writes the real raster skip run and syntax, commits reconstruction tiles and retains only each macroblock's compact deblocking record. It never publishes sequence state. Deblocking still runs after complete picture assembly.

One worker uses two reusable rows without locks, channels or spawned threads. Each completed row is emitted before the next row is computed. In parallel, immutable row slots are protected by shared row borrows while producers and the caller read them. A row can be cleared only after its own emission and completion of the next row, its final prediction reader. Exclusive recycling then makes the slot available for a later row. No full candidate is cloned for transport or emission.

Let `w` and `h` be the coded macroblock dimensions and `n = min(requested_threads, h, ceil(w / 2))`. The parallel rolling window contains `min(h, n + 1)` rows; one worker retains `min(h, 2)`. A bounded retirement condition prevents a producer from entering a reused slot until the caller has cleared it. Progress within each adjacent row pair retains the existing two-macroblock lead and bounded channel. Whole rows are assigned monotonically, so a waiting row's predecessor is either finished or assigned to an earlier worker, never queued behind it.

The caller emits row `r`, then retires `r - 1`: completion of `r` proves that all prediction readers of `r - 1` have finished. Publication of row completion happens after the worker drops both row borrows. The caller cannot block an earlier producer while trying to recycle its row. This keeps the serial tail overlapping later decisions without limiting the wavefront to two computing rows.

## Workspace bound

Candidate storage is `4976 * w * window_rows` bytes on the checked 64-bit layout, rather than `4976 * w * h`. When deblocking is enabled, compact records add `136 * w * h` bytes; disabled deblocking allocates none. At the maximum picture size, those records occupy 1,109,760 bytes, so combined candidate and record storage is 2,304,000 bytes at one worker, 4,095,360 at four, 6,483,840 at eight and 11,260,800 at sixteen. The layout test checks both component sizes and the retained-storage bound.

Parallel coordination retains `h` progress channels with at most `w` messages each and `h` one-message completion channels, plus `window_rows` row locks, one retirement mutex/condition variable and at most `n` join handles. Progress payload storage is at most `w * h * sizeof(Result<usize>)`; standard-library channel metadata and allocation overhead are additional. Worker stacks and transient decision candidates remain bounded by `n`. Reconstruction and reference planes are unchanged. These formulas describe retained workspace, not total process memory.

At the maximum coded size, 1920 by 1088 (`w = 120`, `h = 68`), candidate storage is 1,194,240 bytes with one worker, 2,985,600 with four, 5,374,080 with eight and 10,151,040 with sixteen, versus 40,604,160 bytes before streaming. These are allocation bounds, not measured peak resident memory. The implementation will record the compact-record and synchronization contributions separately.

## Failure order and thread creation

Both serial and parallel decision loops catch unwind and return `Internal`. Each parallel row publishes either its successful completion or its error, and a row error wakes its dependency reader. The caller drains every assigned row and joins every started worker before returning. It chooses the first raster decision error before any emission error, matching the old decision-before-emission order even when the writer reaches a byte limit early. An emission failure suppresses later emission, but not decision draining or row recycling.

If a later thread cannot start, the already-started workers keep taking rows and their results remain usable. If none starts, the parallel workspace is released and the caller uses the two-row serial path. A per-test spawn limit exercises both cases without changing process-wide thread limits. Failed pictures never commit the reference, motion carry or counters.

## Alternatives

A global two-row parallel window would cap useful concurrency at two rows, since macroblocks within a row are sequential. Retaining a picture-wide compact prediction table would avoid recycling coordination but would introduce another representation and a second lifetime for every macroblock. A worker-scaled window keeps the existing immutable candidates and exposes the bound directly.

A persistent pool would need to own or copy borrowed picture/reference inputs and coordinate an encoder's lifetime across calls. Keep scoped workers in this change. Evaluate the work-based cap already imposed by row count and width against the measured scaling; do not add a new heuristic without evidence that it improves the current inputs.

## Verification

- Compare complete streams and reconstructions with the merged baseline at explicit commits, across 1/2/3/8 workers and scalar/dispatched builds. No output or quality change is allowed.
- Test repeated window reuse, narrow/cropped geometry, reordered completion, a blocked caller, first-raster failures, emission failure draining, one-worker unwind, partial and zero-worker startup, and successful calls after refusals.
- Round-trip textured multi-row P pictures at eight workers through the in-repository decoder. Disable top-right neighbour availability in a temporary build and require the new coverage to fail, then restore and rerun it.
- Measure seven rotated encode-only calls per input on four-vCPU Actions runners at 1/2/4 workers, using separate targets and full commit identities. Preparation, output writing and oracle decoding stay outside timing. Larger-core timing is separately routed; oversubscribed workers provide identity evidence only.
- Run both encoder test modes, formatting, strict Clippy, unsafe inventory, public-text checks and the exact-head similarity guard. Independent review covers row reuse, all wakeups and failure ordering.
