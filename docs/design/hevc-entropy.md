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

## Decoder-private decision engine

Keep the current safe CABAC engine as the scalar implementation and the
differential reference. The x86-64 non-scalar decision body may use a private
`fast/` module with one named assembly site. Bin admission, caller budgets,
terminal phases, bypass, raw samples and restart stay with their existing
owners. No residual-syntax or reconstruction interface changes.

Combine the specified LPS range, its normalized range and shift, and the two
context transitions in a compile-time table. Baseline x86-64 integer assembly
selects the decision and normalization without reading the byte source or
writing memory. Each table index is bounded before the assembly call. The
unsafe inventory states the entire table extent and every access.

Keep the bit position in `Bits`. A checked refill reads only the actual
normalization width, at most seven bits from two bytes. Compute prospective
arithmetic state locally first. If the complete refill is unavailable, run
the safe decision body from the untouched state, preserving partial reads,
arithmetic state and the exact truncation error. After a complete refill,
commit arithmetic and input state before checking the offset, and update the
context only after successful normalization. A zero-bit refill preserves
the last consumed bit.

Prefer this shape over a separate reservoir-backed generic engine: it keeps
one source position, avoids changing callers, and preserves the safe engine's
error-state representation. Forced scalar builds compile no assembly route.
Differential tests and fuzzing must select both bodies explicitly and compare
bins, errors, contexts, input position, range, offset, last bit, phase and
remaining budget after every operation. Check all legal arithmetic states,
short prefixes, invalid contexts, budgets, termination and restart. Complete
output checks and decoder-only one-thread comparisons remain necessary;
instruction ablations alone are not a speed result.

Keep a separate more-probable-symbol branch instead of loading both
transitions and normalization alternatives unconditionally. When the selected
normalization width is zero, only range and context state change: return the
existing MPS without visiting the refill helper or rewriting offset and the
last consumed bit. Every LPS table entry requires at least one shift, so a
zero-width result uniquely identifies this no-refill MPS path. Refilling
decisions retain the checked-window and exact safe-fallback contract above.
