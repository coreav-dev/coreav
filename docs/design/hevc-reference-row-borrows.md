# HEVC reference-row borrows

The interpolator checks its entire clipped reference support before writing output. It then gathers each source row through a clamped column map into a temporary buffer, even when the horizontal support lies entirely inside the coded picture.

For either supported sample depth with an interior horizontal support, borrow the consecutive source samples directly. Construct the column map and gather only for the existing boundary-replication path. The AVX2 kernels, scalar filter arithmetic, horizontal and vertical shifts, intermediate row ring, output writes and scratch accounting do not change.

H.265 clause 8.5.3.3.3 and Tables 8-8 and 8-9 define consecutive taps within a row. If `x_start >= 0` and `x_start + source_width <= reference.width`, every previously gathered index is exactly `x_start + x`. A checked source slice therefore supplies the identical samples in the identical order. The existing full-support depth check still precedes both paths. Taps with zero coefficients remain part of the checked support.

The support calculation remains shared with scheduling. This change only consumes the resulting start coordinate and filter extent inside the kernel. Picture-edge replication, extreme signed motion vectors, invalid samples and output padding preserve their existing behavior. No new unsafe site or scratch allocation is needed.

Main10 samples still enter the identical `u16` filter input, and the existing horizontal shift by two and vertical shift by six remain unchanged. Borrowing rather than copying cannot change their values, intermediate range or operation order. The full clipped support is checked against the depth's maximum before either path starts.

The preceding diagnostics compare only removal of the interior column-map and gather copies, without deleting filter arithmetic. They check both exact decoder arms against the qualified oracle before whole-process instruction counts. A Main10-only comparison keeps eight-bit input on its original path. Those counts motivate this bounded change but are not a wall-time speed claim.
