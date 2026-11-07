# HEVC coefficient scan lookup

Residual syntax uses the diagonal, horizontal and vertical scan orders in
H.265 clauses 6.5.3 and 7.4.9.11. The decoder currently walks diagonals for
each coefficient coordinate and searches that order again for the last
significant coefficient.

Generate forward coordinates and inverse positions at compile time for
the four admitted subblock-grid sizes: 1, 2, 4 and 8. Select the order once
per transform block. Keep ordinary checked slice access and preserve bin
order, context selection, coefficient levels and errors on malformed input.
The tables introduce neither allocation nor unsafe code.

Check every coordinate against the existing arithmetic scan and check the
inverse permutation exhaustively. Existing residual syntax tests, complete
conformance decoding and the original-file census remain correctness gates.
Compare decoder-only release builds at explicit commits on the same runner,
with alternating calls, complete output checks and a qualified binary oracle.

CABAC arithmetic and level decoding remain unchanged in the first candidate.
Consider further restructuring or assembly only after measuring this safe
candidate. Any later unsafe site must retain a safe reference, document its
memory preconditions and compare both implementations on arbitrary bytes.
