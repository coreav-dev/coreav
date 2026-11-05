# Transcode frame handoff, phase B

## Goal and constraints

Reduce the remaining end-to-end time against FFmpeg using the same decoder source revisions and encoder settings. The target is within 1%; measurements are reference figures, not a judged speed claim. Keep the decoder sandbox, separate encoder process, deadlines, memory limits, descriptor validation and bounded slot ownership. Keep default and release builds free of GPL dependencies. Complete output files must remain byte-identical to main.

## Profile before implementation

Start from main after the decoder-owned-buffer change. Profile one substantial VP9 input, one ordinary AV1 input and the AV1 input whose x264 ratio was 1.1784. Use the existing sixteen-file roster without acquiring new media. Record executable identities and library source revisions.

Collect end-to-end elapsed time separately from profiling. Use function-only CPU samples for the parent, decoder worker and encoder, plus process CPU time, syscall counts, context switches and scheduling observations. Never request source or source-line annotations from an external encoder or decoder implementation. Distinguish time actively copying bytes from time blocked waiting for the encoder. Inspect buffer zeroing, decoder work, pipe writes, encoder reads and encoding. Check the outlier's input format, encoder arguments and thread utilization rather than assigning its excess to transport.

Commit the attribution before modifying production code. The split-process FFmpeg reference includes serialization, startup and overlap changes and cannot establish the cost of CoreAV's pipe writes. CPU shares are not additive elapsed-time savings when stages overlap. If sampling cannot distinguish a candidate cost, report the uncertainty and use a small controlled diagnostic rather than assuming the boundary is responsible.

## Implementation decision

If transport is material, evaluate shared-memory handoff to a separately installed encoder helper. The parent must validate immutable descriptors before publication; pixel bytes may change under a compromised decoder and must never become trusted control data or Rust references into concurrently writable storage. Hold a frame lease until the encoder has finished reading it. Keep bounded credits, crash cleanup and deadline cancellation. An encoder must not obtain compressed input, writable decoder control state or destination-file access.

Do not link or bundle a GPL encoder in a CoreAV artifact. Keep default and release builds unchanged in licence. Designs requiring GPL dependencies in those builds are excluded, not an escalation option. A new helper or transport must respect the encoder policy and use only public interface documentation. Stop if a design would weaken decoder isolation. If profiling identifies another material source, fix that source instead of building an unnecessary transport.

## Initial attribution

Six function-only profiles were collected before production changes on a standard c4-standard-8, SMT off, four available cores, Intel Xeon Platinum 8581C and Debian 13. The decoder source revisions and pinned programs match the phase A reference. Each profile samples inherited cycles at 199 Hz, with 4 KiB DWARF callchains. A separate observer samples process threads every 100 ms. These instrumented elapsed times are not benchmark results.

| Input | Encoder | Profiled elapsed seconds | Parent CPU seconds | Decoder CPU seconds | Encoder CPU seconds |
|---|---|---:|---:|---:|---:|
| `ea684713a5be` | FFmpeg | 106.299 | 4.54 | 39.43 | 353.89 |
| `ea684713a5be` | x264 | 108.353 | 4.60 | 39.98 | 355.16 |
| `effb7b78408d` | FFmpeg | 96.295 | 4.99 | 29.18 | 305.74 |
| `effb7b78408d` | x264 | 96.284 | 4.05 | 28.99 | 305.92 |
| `357fb30f4693` | FFmpeg | 116.285 | 4.30 | 55.24 | 383.76 |
| `357fb30f4693` | x264 | 133.303 | 4.26 | 55.07 | 443.57 |

CPU seconds sum the last observed per-thread user and system ticks, so short-lived work and the final sampling interval may be missed. The stages overlap and compete for four cores. The encoder accounts for most CPU work; neither the parent total nor a sleeping writer's elapsed lifetime is an independently removable wall-time cost.

The writer spends 3.78–4.66 CPU seconds in the six profiles and makes 246,896–318,849 voluntary context switches. Of its observed thread states, 79.8–90.8% are in `pipe_write`. The kernel's `_copy_from_iter` in that writer accounts for 0.53–0.79% of aggregate sampled cycles. Encoder-side `_copy_to_iter` accounts for 0.14–0.45%, and `anon_pipe_buf_release` for 0.23–0.76%. These distinguish active transfer work from blocking but do not assign the residual elapsed gap to copying alone. Run-queue waiting is substantial in both decoder and encoder threads, so scheduling and overlap need a controlled comparison.

An unresolved libc entry in the VP9 decoder accounts for 0.44% and 0.46% of aggregate cycles. Its sampled offset is 216 bytes after this host's resolved `memset` entry, consistent with buffer clearing but not a callback-specific attribution. Buffer zeroing is not removed because the external allocator contract requires cleared storage. Entries below the report's 0.1% display threshold are not a proof of zero cost.

The outlier is 3840 × 2160, 10-bit 4:2:0 AV1. With x264 its main encoder thread consumes 53.66 CPU seconds, and `get_frame` is the largest single symbol at 8.96% of aggregate sampled cycles. The encoder uses about 59.8 more CPU seconds than the FFmpeg encoder process on this same input; parent and decoder totals barely change. This supports the existing [10-bit input-conversion diagnosis](../measurements/transcode-x264-10bit.md), rather than treating the outlier as a decoder-boundary cost. A transport change alone does not remove x264's input conversion.

Before adding a helper, test a bounded larger pipe with an external diagnostic wrapper that changes only `F_SETPIPE_SZ`, then execs the unchanged pinned encoder. Compare three rotated calls on these same inputs and require complete-file identity. This tests whether fewer pipe wakeups reduce the residual without a new encoder interface, licensing dependency or shared-memory trust boundary. No production implementation is selected until that diagnostic returns.

The [compressed profile observations](../measurements/transcode-frame-handoff/phase-b-profile.json.gz) retain executable hashes, per-thread observations and function-level sample rows. No encoder or decoder source lines were requested or retained.

### Pipe capacity diagnostic

The wrapper confirms 65,536-byte input pipes and enlarges them to 1,048,576 bytes before execing the unchanged encoder. All eighteen completed calls succeed and all nine paired output files match. The median enlarged/default ratios are 1.0058 for VP9 with FFmpeg, 0.9954 for ordinary AV1 with FFmpeg and 1.0109 for the 10-bit AV1 x264 outlier. Within-cell ranges span 0.73–2.37% of their medians. There is no consistent improvement, so do not implement the larger-pipe change. The [diagnostic capture](../measurements/transcode-frame-handoff/phase-b-pipe-capacity.json.gz) retains all completed calls.

One earlier attempt stopped because the harness expected successful encoder stderr in CLI output, where it is retained internally instead. The corrected wrapper asserts pipe sizes before exec. That tooling failure supplies no speed result.

### Rejected page loans

Linux `vmsplice` can loan shared-memory pages into the existing encoder pipe without another binary or a GPL dependency. Waiting for `FIONREAD` to report an empty pipe does not preserve the current frame-lease boundary against a hostile encoder. The reader can `tee` the data into another pipe and consume the original pipe. The parent would acknowledge the frame while the second pipe still refers to the decoder-writable pages.

A 4,096-byte local counterexample confirms this. After `vmsplice`, `tee` and a complete read of the original pipe, `FIONREAD` reports zero. Overwriting the source mapping changes all 4,096 bytes subsequently read from the second pipe. No page ownership was gifted. The [observation](../measurements/transcode-frame-handoff/phase-b-page-loan.json) records those counts. Reader-close checks and a shorter polling interval cannot revoke forwarded page references.

Reject loans of decoder-writable pages. A shared-memory encoder helper instead needs an immutable snapshot and an explicit acknowledgement protocol. The full sixteen-input main/candidate comparison has not run yet.

## Selected helper design

The [operator-only exception in record 0042](encoder-backends.md) permits a separate executable outside every CoreAV artifact and build graph. The pipe-fed paths remain unchanged. The operator explicitly supplies `COREAV_X264_SHM_HELPER` and selects the x264 encoder. An explicit `--x264` still names the pipe program and takes precedence.

For each frame, the parent copies checked packed rows into a fresh anonymous file using the same raw `writev` copy boundary as the pipe path. It adds write, grow, shrink and final seals before sending a descriptor on the helper's control pipe, then releases the decoder's frame. The helper opens only the snapshot read-only, verifies its seals and exact size, maps it read-only and calls libx264. Its acknowledgement releases the parent's file handle. Neither process ever makes the snapshot writable again or reuses it for another frame.

A reusable ring was considered but rejected because immutable per-frame files make early or dishonest acknowledgements harmless to the contents of retained mappings and forwarded pipe references. Only one snapshot is in flight from the writer. The existing bounded input queue, process limits, keeper and deadline remain. The helper's bounded stdout framing separates acknowledgements from encoded bytes; the latter still pass through the existing Annex B validator and muxer. Unknown messages, wrong sequences, short frames or invalid seals fail the job.

This removes the pipe's pixel payload and encoder-side raw input read. It keeps one trusted copy out of decoder-writable storage rather than allowing those pages to cross the next boundary. Test sealed-snapshot immutability after decoder reuse, hostile protocol messages and process exits, then require complete-file identity before reporting speed. [The helper protocol](../../tools/x264-shm-helper/README.md) defines the wire fields.

## Verification and reference capture

Run targeted ownership, hostile-process and complete-file identity tests, including the pinned FFmpeg and x264 programs. Add unsafe-site inventory rows for any new unsafe. Run the similarity guard against its main allowlist and the normal required CI checks.

Repeat the phase A method on all six AV1 and ten VP9 inputs, using three rotated calls per file and arm. Compare main and candidate with both encoder programs against FFmpeg's single-process transcode with explicit libdav1d or libvpx-vp9 input decoding. Warm source bytes, put outputs on tmpfs, and keep probing and hashing outside timing. Retain every failure, per-call time, encoder receipt and complete-file hash. Summarize medians of per-file three-call-median ratios, disclose call variation and remaining misses, and make no attribution from those ratios alone.
