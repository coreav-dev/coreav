# Reuse decoded frame storage

The CLI and transcode should publish the same bytes without allocating new packed H.264 planes for every frame. The CLI also clones CPU pictures and allocates a second packed output buffer for every raw frame.

## Ownership

Keep one bounded packed-picture pool per native H.264 decoder. A picture owns its writable planes exclusively. Dropping it returns its storage through a weak recycler handle, including after asynchronous encoder writes. CPU picture clones remain independent deep copies and do not inherit the return handle. A picture may outlive its decoder.

The pool reuses only matching geometry and format. Every visible row is overwritten before publication. Retained frames never block decoding; when all managed storage is in use, allocation falls back to the existing fallible path without adding an admission rule.

Use the existing decoder output-copy allowance, not another memory budget. Bound managed pixel capacities and slot count, including live leases. Cached storage plus pending frames must fit both that allowance and the pending-byte limit. Evict idle storage on geometry changes and budget reductions. Keep cumulative decoded-byte and pending-frame charges unchanged. One-IDR decoding does not retain a cache.

The CLI consumes CPU pixels rather than cloning them and writes raw picture rows directly through the existing file and pipe writers. PNG and BMP encoding remain unchanged. Shared-memory reads, sandboxing, encoder queues, decoder kernels, release allocators and build defaults stay unchanged.

## Verification

Add deterministic allocation regressions and retained-frame, clone, geometry-change, teardown and budget tests. Existing raw file, pipe and YUV4MPEG tests must preserve every sample. Check transcode packet identity against the unchanged baseline, not only decoded pixels.

Run Linux reference measurements only on GitHub Actions. Recreate the three existing lossy 480p, 720p and 1080p inputs and verify their raw hashes. Interleave baseline and candidate CLI calls for musl and glibc, with the decoder example and pinned FFmpeg as controls. Record elapsed, user and system time, page faults, and separate mmap/munmap traces. Include one inexpensive H.264-to-AV1 comparison. These are reference numbers without acceptance bars; results are recorded separately after the code merges.
