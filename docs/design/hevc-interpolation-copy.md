# HEVC Main10 interpolation row-copy ceiling

The current interpolator first checks the entire clipped reference support, then copies each needed reference row into a 71-sample temporary buffer through a precomputed clamped column map. Both sample depths already use AVX2 for eligible horizontal and vertical arithmetic.

This experiment removes only the Main10 column-map construction and source-row gather when the complete horizontal support is inside the coded reference width. The existing reference row can then supply the identical consecutive `u16` values directly. Eight-bit input keeps its existing path. Horizontal filtering, its depth-dependent shift, the eight-row intermediate ring, vertical filtering and its shift, output writes, support validation and boundary replication remain unchanged. No arithmetic is erased.

This follows H.265 clause 8.5.3.3.3 and Tables 8-8 and 8-9. An interior support already names consecutive coded samples. Borrowing that sequence instead of copying it changes neither the filter inputs nor the operation order. A support crossing an edge retains the existing gather, including taps with zero coefficients and extreme signed motion vectors.

The diagnostic branch is not a shipping proposal. A same-runner whole-process instruction comparison will establish the removable copy cost on two eight-picture Main10 prefixes, starting from current main. Both arms must match the qualified pinned oracle and produce the same work receipts before counting. The run retains exact arm SHAs, binary hashes and function-only Callgrind data. No wall-time gain or CPU-time share is inferred from instruction counts.

If this narrow comparison saves less than about two percent, do not build a larger copy-removal candidate from the whole interpolation-family share. If it clears that boundary, review a production change with the existing scalar reference and support/error invariants intact. The separate reference-support owner remains the sole source of the clipped read footprint; predictor combination is out of scope.
