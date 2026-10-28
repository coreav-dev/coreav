# The test-only counting allocator

Confined test-only `unsafe`; its contract and what it measures.

Only the ignored `tests/heap_measure.rs` integration target installs the counting
allocator. `tests/support/counting_allocator.rs` contains the unsafe surface:
one `GlobalAlloc` implementation, its four required unsafe methods, and four
forwarding blocks to `std::alloc::System`. Each has a SAFETY comment. Library and
CLI retain `forbid(unsafe_code)`; the standalone manifest uses `deny` solely to
allow this test module. No production parsing or pointer dereference is unsafe.

Contract: forward each caller's valid pointer/Layout unchanged; return
System's pointer unchanged; count only successful allocations and reallocations;
leave counters unchanged on failed realloc; account old/new requested sizes on
successful realloc. Callbacks perform atomics only, never allocate, unwind or
format. Checked counter overflow/underflow marks measurement invalid. Concurrent
counter updates and reset/peak snapshots are outside what this experiment
exercises: it runs one
ignored test with one test thread and no parser concurrency.

Metric is live **requested** Rust heap bytes, not process RSS, allocator arenas,
usable-size padding or System's transient old-plus-new realloc buffers. Baseline
includes the harness and open file source. Probe and descriptor iteration are
inside the measured window; JSON serialization happens after snapshots. Peak
minus baseline includes temporary parse buffers; allocation count includes all
successful allocator calls in that window. Post-drop growth checks retained
state. The existing default metadata/table/box/packet caps remain active.
