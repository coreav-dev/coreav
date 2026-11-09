# HEVC interpolation row scratch

The interpolator initializes an eight-by-64 `i32` row ring for each prediction block, then overwrites every active horizontal result before reading it. Keep the same ring and filters, but allow its backing samples to come from the decoder's existing residual workspace. No frame, worker or heap allocation is added.

## Ownership and lifetime

An additive checked kernel entry point accepts at least 512 mutable `i32` samples. The existing entry points keep their local initialized scratch and their error behavior. The new entry point uses only the first 512 samples; column indices and boundary source samples remain local. A flat ring row starts at `(logical_row % 8) * 64` and its active extent is the prediction width.

The decoder's existing residual vector has 1,024 samples. Inter prediction runs before transform reconstruction and borrows that vector only during one synchronous kernel call. Before any later residual read, coded transforms write the full active residual through `inverse_residual`, and uncoded transforms fill that active extent with zero. Interpolation therefore cannot replace a live residual value. A mutable borrow prevents simultaneous predictor or worker use of the same storage.

For every output row, the existing `next_row < y + row_count` loop writes every required horizontal row before the vertical pass reads it. A reused ring may contain arbitrary values elsewhere: filtering reads only the current prediction width, and each row is replaced before its slot is reused. Integer and horizontal-only paths keep their existing row copy. No numerical operation is deleted.

## Preserved behavior

H.265 clause 8.5.3.3.3 and Tables 8-8 and 8-9 still determine the same taps, coefficients, intermediate shifts and operation order at both supported depths. Full clipped-support depth validation still precedes scratch or output writes. Boundary replication, checked source extents, output padding, invalid-input rejection and the signed intermediate range are unchanged. No unsafe code is needed.

Keep the existing 9,224-byte kernel scratch envelope and decoder admission calculations unchanged. `Frame`, its scratch owner and worker refresh gain no fields. Only existing residual samples are borrowed, so cold and retained memory do not grow.

## Verification

Compare reused, deliberately dirty scratch against the existing forced-scalar path and literal equations across both depths, luma/chroma phases, widths and tails, interior and edge motion, and successive calls with different shapes. Reject short workspace and invalid support before output writes, and retain unused workspace and output padding. Run complete automatic and forced-scalar conformance before shipping.

Instruction evidence must count this row-only reuse in the real decoder against its exact baseline, with oracle samples and completed work checked in both arms. An earlier whole-scratch reuse probe is only motivation: it also removed column/source initialization and added fixed frame storage, neither of which this design does. Do not treat that probe's reduction as this implementation's result. Count the actual code before any timing comparison and disclose every selected timing cell.
