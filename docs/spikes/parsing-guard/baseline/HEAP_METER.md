# Benchmark allocation meter

`heap.c` is compiled only into an ignored benchmark `LD_PRELOAD` library. It is
never linked into a CoreAV Cargo artifact. Timing and RSS samples run without it.
It is benchmark instrumentation only, never a production component.

The meter delegates the six public allocation/deallocation interfaces unchanged
and records requested lengths in a fixed BSS pointer table. It does not modify
allocation sizes, alignment or returned pointers. Failed reallocations leave old
accounting unchanged. Zero-sized reallocations follow the pinned GNU libc
contract. Pointer-table access uses an atomic spin lock; initialization uses a
separate atomic flag, an atomic ready flag and thread-local recursion detection.
Bookkeeping has a fixed capacity and marks saturation/overflow/underflow invalid.

A bounded BSS buffer permits allocator recursion while resolving `RTLD_NEXT`.
Bootstrap reallocations fail closed. Evidence rejects any bootstrap use or
untracked frees, as well as invalid counters. Uninstrumented allocator-internal
operations and direct mappings are outside this metric. The pinned harnesses
use no decode or worker threads. The meter rests on its forwarding calls,
initialization/reentrancy, concurrent allocation/free ordering and table bounds.

The reported whole-process peak includes library initialization, parser work,
payload buffers and final JSON output. BSS bookkeeping, allocator arenas,
usable-size padding, stack storage and allocator-internal old-plus-new realloc
transients are excluded. RSS is reported separately from uninstrumented runs.
The CoreAV whole-process peak is also a conservative upper bound on its measured
parser heap growth; original requested-Rust-heap snapshots remain separate.

Valgrind Massif crashed on the first cloud attempt under gVisor. That failed
attempt and its partial timings remain evidence. The corrected run-02, run to finish
the comparison, used this interposer; all 84 of its heap measurements passed
the bookkeeping checks.
