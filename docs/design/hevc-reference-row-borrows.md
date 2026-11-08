# HEVC eight-bit reference-row borrows

The interpolator checks its entire clipped reference support before writing output. It then gathers each source row through a clamped column map into a temporary buffer, even when the horizontal support lies entirely inside the coded picture.

For eight-bit input with an interior horizontal support, borrow the consecutive source samples directly. Construct the column map and gather only for the existing boundary-replication path. Ten-bit reconstruction keeps that path unchanged. The AVX2 kernels, scalar filter arithmetic, horizontal and vertical shifts, intermediate row ring, output writes and scratch accounting do not change.

H.265 clause 8.5.3.3.3 and Tables 8-8 and 8-9 define consecutive taps within a row. If `x_start >= 0` and `x_start + source_width <= reference.width`, every previously gathered index is exactly `x_start + x`. A checked source slice therefore supplies the identical samples in the identical order. The existing full-support depth check still precedes both paths. Taps with zero coefficients remain part of the checked support.

The support calculation remains shared with scheduling. This change only consumes the resulting start coordinate and filter extent inside the kernel. Picture-edge replication, extreme signed motion vectors, invalid samples and output padding preserve their existing behavior. No new unsafe site or scratch allocation is needed.

The preceding diagnostic compares only removal of the interior column-map and gather copies, without deleting filter arithmetic. It checks both exact decoder arms against the qualified oracle before whole-process instruction counts. Those counts motivate this bounded change but are not a wall-time speed claim. Main10 remains a separate follow-up.
