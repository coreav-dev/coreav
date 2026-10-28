//! The measuring harness measures itself: the numbers the pinned limit
//! tests rely on have to be right before those tests mean anything
//! (`docs/design/limits.md`).
#![allow(clippy::unwrap_used, clippy::panic)]

use coreav_counting_alloc::{CountingAllocator, measure};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Allocating exactly `n` bytes shows up as `n` bytes, give or take the
/// vector's own growth, which `with_capacity` avoids.
#[test]
fn one_allocation_is_counted_once() {
    let (buffer, counters) = measure(|| Vec::<u8>::with_capacity(4096));
    assert_eq!(counters.allocations, 1);
    assert_eq!(counters.reallocations, 0);
    assert_eq!(counters.bytes, 4096);
    assert_eq!(counters.peak_bytes, 4096);
    assert_eq!(counters.live_bytes, 4096);
    drop(buffer);
}

#[test]
fn nothing_is_counted_outside_a_region() {
    let before = Vec::<u8>::with_capacity(1 << 16);
    let ((), counters) = measure(|| {});
    assert_eq!(counters, Default::default());
    assert_eq!(counters.trough_bytes, 0);
    drop(before);
}

/// The peak is the high-water mark, not the amount left at the end.
#[test]
fn freeing_lowers_the_live_count_and_not_the_peak() {
    let ((), counters) = measure(|| {
        let scratch = Vec::<u8>::with_capacity(1 << 20);
        drop(scratch);
    });
    assert_eq!(counters.peak_bytes, 1 << 20);
    assert_eq!(counters.live_bytes, 0);
    assert_eq!(counters.allocations, 1);
}

/// Memory allocated before the region and freed inside it offsets later
/// growth, as it does in the process: the region never held more than it
/// began with, so it grew by nothing. The trough is what says so, and is
/// why the pinned bars assert it is zero.
#[test]
fn freeing_older_memory_offsets_growth_and_shows_in_the_trough() {
    let before = Vec::<u8>::with_capacity(1 << 20);
    let ((), counters) = measure(|| {
        drop(before);
        let after = Vec::<u8>::with_capacity(4096);
        drop(after);
    });
    assert_eq!(counters.peak_bytes, 0);
    assert_eq!(counters.trough_bytes, -(1 << 20));
    assert_eq!(counters.live_bytes, -(1 << 20));
    assert_eq!(counters.bytes, 4096);
}

/// A region that only ever allocates has a trough of zero, so its peak is
/// growth and nothing else.
#[test]
fn a_region_that_only_allocates_has_no_trough() {
    let ((), counters) = measure(|| {
        let first = Vec::<u8>::with_capacity(4096);
        let second = Vec::<u8>::with_capacity(4096);
        drop((first, second));
    });
    assert_eq!(counters.peak_bytes, 8192);
    assert_eq!(counters.trough_bytes, 0);
    assert_eq!(counters.live_bytes, 0);
}

/// A reallocation counts only its growth, and only once, but its peak
/// covers the moment a move would hold both blocks.
#[test]
fn growing_a_vector_counts_the_growth() {
    let (buffer, counters) = measure(|| {
        // `vec![0; n]` would use `alloc_zeroed`, which counts the same;
        // this keeps the one allocation and the one reallocation apart.
        let mut buffer = Vec::<u8>::with_capacity(1024);
        buffer.extend(std::iter::repeat_n(0u8, 1024));
        buffer.reserve_exact(1024);
        buffer
    });
    assert!(buffer.capacity() >= 2048);
    assert_eq!(counters.allocations, 1);
    assert_eq!(counters.reallocations, 1);
    assert_eq!(counters.bytes, 1024 + (buffer.capacity() as u64 - 1024));
    // The old block and the new one are counted as live together, whether
    // or not the allocator grew this one in place.
    assert_eq!(counters.peak_bytes, 1024 + buffer.capacity() as u64);
}

/// An inner region reports its own total and is added to the outer one.
#[test]
fn regions_nest() {
    let ((), outer) = measure(|| {
        let first = Vec::<u8>::with_capacity(1 << 16);
        let (second, inner) = measure(|| Vec::<u8>::with_capacity(1 << 17));
        assert_eq!(inner.bytes, 1 << 17);
        assert_eq!(inner.peak_bytes, 1 << 17);
        assert_eq!(inner.allocations, 1);
        drop((first, second));
    });
    assert_eq!(outer.allocations, 2);
    assert_eq!(outer.bytes, (1 << 16) + (1 << 17));
    assert_eq!(outer.peak_bytes, (1 << 16) + (1 << 17));
    assert_eq!(outer.trough_bytes, 0);
    assert_eq!(outer.live_bytes, 0);
}

/// Counters are per thread, so a test measuring while another thread
/// allocates sees only its own work. This is what lets `cargo test` run
/// the pinned limit tests in parallel with everything else.
#[test]
fn another_thread_is_not_counted() {
    let handle = std::thread::spawn(|| {
        let (buffer, counters) = measure(|| Vec::<u8>::with_capacity(8 << 20));
        assert_eq!(counters.peak_bytes, 8 << 20);
        buffer.capacity()
    });
    let ((), counters) = measure(|| {
        let mine = Vec::<u8>::with_capacity(2048);
        drop(mine);
    });
    assert_eq!(handle.join().unwrap(), 8 << 20);
    assert_eq!(counters.bytes, 2048);
    assert_eq!(counters.peak_bytes, 2048);
}
