//! The process-wide region counts the threads its body starts. One test,
//! alone in its binary, as [`measure_process`] asks.
#![allow(clippy::unwrap_used, clippy::panic)]

use coreav_counting_alloc::{CountingAllocator, measure_process};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

#[test]
fn another_threads_allocations_are_counted() {
    let kept = vec![0u8; 1 << 20];
    let (capacity, counters) = measure_process(|| {
        let handle = std::thread::spawn(|| {
            let buffer = Vec::<u8>::with_capacity(8 << 20);
            buffer.capacity()
        });
        handle.join().unwrap()
    });
    assert_eq!(capacity, 8 << 20);
    // The other thread's buffer, and what starting the thread took.
    assert!(counters.peak_bytes >= 8 << 20, "{counters}");
    assert!(counters.peak_bytes < 9 << 20, "{counters}");
    assert_eq!(counters.trough_bytes, 0, "{counters}");
    // Outside a region nothing is counted, and memory from before the
    // region is not this region's.
    drop(kept);
    let ((), after) = measure_process(|| {});
    assert_eq!(after.calls(), 0, "{after}");
}
