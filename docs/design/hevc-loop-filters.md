# HEVC loop filter rows

The opt-in decoder applies vertical deblocking, horizontal deblocking, then sample adaptive offsets (SAO), as specified by ITU-T H.265 V11 clause 8.7. The scalar sample functions remain the numerical reference. No other implementation is a source.

## Remove repeated decisions before arithmetic

SAO parameters are constant over a component's coding tree block. Resolve the offset range, band position or edge direction once per block. The driver divides edge blocks into a one-sample border and an interior. Each rectangle has constant neighbour-block availability, including diagonal corners and independently restricted tile and slice boundaries. Protection remains constant over each reconstruction cell. Join adjacent unprotected cells into row spans and pass only available samples to a checked kernel.

Band filtering reads and writes the same row and needs no source copy. Edge filtering reads the original deblocked component. Keep one immutable component copy whenever any edge block exists, so neighbouring band blocks cannot change edge classification. The copy remains charged to the decoder's memory limit. A rolling row buffer would reduce memory further but changes traversal and failure recovery, so it is outside this change.

Deblocking retains vertical-before-horizontal ordering and its normative four-line decisions. Chroma components share boundary metadata, so derive their boundary strength once. Luma and chroma sample kernels keep their scalar input-validation contract.

## Bounded row interface

`SaoBand::new(depth, offsets, position)` and `SaoEdge::new(depth, offsets)` validate block parameters. Band application mutates a bounded row. Edge application takes the source row, two equal-length neighbour rows and a disjoint destination row. Both return the changed-sample count. They validate all sample values and lengths before writing. Empty rows are valid. The scalar sample interface remains available for independent differential checks.

The alternative was a single sample-class enum passed for each pixel. That repeats validation and direction dispatch and prevents a useful vector width. Separate prepared band and edge types make those choices once without exposing raw pointers.

## Vector scope and proof

AVX2 lives only in `coreav-kernels-hevc/src/simd/loop_filter.rs`, under record 0045. The existing cached runtime capability gates calls, and the `scalar` feature removes the SIMD module. This first implementation uses 8-bit samples in signed 16-bit lanes. Comparisons, offset selection, addition and clipping preserve the scalar equations without narrowing an intermediate. Main10 stays on safe scalar arithmetic and gets a separate measured change.

Loads and stores cover complete groups within checked slices. Scalar tails handle every remaining length. No padding, alignment or aliasing assumption extends beyond Rust slice borrows. Tests compare all band positions and samples, all edge comparison classes, offset extremes, clipping, malformed inputs, unaligned starts and final-byte extents. Differential fuzzing calls the safe interface against the scalar sample reference. Full conformance and census comparisons run in both feature modes on Actions before acceptance. Reference timing uses explicit main and candidate commits, alternating repeated single-threaded calls and a qualified pinned binary oracle. A change without a reproducible end-to-end benefit is not retained.
