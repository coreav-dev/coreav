# Transcode frame handoff, phase B

## Goal and constraints

Reduce the remaining end-to-end time against FFmpeg using the same decoder source revisions and encoder settings. The target is within 1%; measurements are reference figures, not a judged speed claim. Keep the decoder sandbox, separate encoder process, deadlines, memory limits, descriptor validation and bounded slot ownership. Keep default and release builds free of GPL dependencies. Complete output files must remain byte-identical to main.

## Profile before implementation

Start from main after the decoder-owned-buffer change. Profile one substantial VP9 input, one ordinary AV1 input and the AV1 input whose x264 ratio was 1.1784. Use the existing sixteen-file roster without acquiring new media. Record executable identities and library source revisions.

Collect end-to-end elapsed time separately from profiling. Use function-only CPU samples for the parent, decoder worker and encoder, plus process CPU time, syscall counts, context switches and scheduling observations. Never request source or source-line annotations from an external encoder or decoder implementation. Distinguish time actively copying bytes from time blocked waiting for the encoder. Inspect buffer zeroing, decoder work, pipe writes, encoder reads and encoding. Check the outlier's input format, encoder arguments and thread utilization rather than assigning its excess to transport.

Commit the attribution before modifying production code. The split-process FFmpeg reference includes serialization, startup and overlap changes and cannot establish the cost of CoreAV's pipe writes. CPU shares are not additive elapsed-time savings when stages overlap. If sampling cannot distinguish a candidate cost, report the uncertainty and use a small controlled diagnostic rather than assuming the boundary is responsible.

## Implementation decision

If transport is material, evaluate shared-memory handoff to a separately installed encoder helper. The parent must validate immutable descriptors before publication; pixel bytes may change under a compromised decoder and must never become trusted control data or Rust references into concurrently writable storage. Hold a frame lease until the encoder has finished reading it. Keep bounded credits, crash cleanup and deadline cancellation. An encoder must not obtain compressed input, writable decoder control state or destination-file access.

Do not link or bundle a GPL encoder in a CoreAV artifact. A new helper or transport must respect the encoder policy and use only public interface documentation. If it requires a licensing or isolation exception, stop for a maintainer decision. If profiling identifies another material source, fix that source instead of building an unnecessary transport.

## Verification and reference capture

Run targeted ownership, hostile-process and complete-file identity tests, including the pinned FFmpeg and x264 programs. Add unsafe-site inventory rows for any new unsafe. Run the similarity guard against its main allowlist and the normal required CI checks.

Repeat the phase A method on all six AV1 and ten VP9 inputs, using three rotated calls per file and arm. Compare main and candidate with both encoder programs against FFmpeg's single-process transcode with explicit libdav1d or libvpx-vp9 input decoding. Warm source bytes, put outputs on tmpfs, and keep probing and hashing outside timing. Retain every failure, per-call time, encoder receipt and complete-file hash. Summarize medians of per-file three-call-median ratios, disclose call variation and remaining misses, and make no attribution from those ratios alone.
