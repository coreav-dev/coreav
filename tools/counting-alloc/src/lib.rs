//! A counting global allocator for the pinned limit tests
//! (`docs/design/limits.md`).
//!
//! The crate is test-only: its manifest says so, and
//! `scripts/unsafe_sites_check.py` fails if any crate names it outside its
//! `[dev-dependencies]`, so it is compiled into test binaries and into no
//! library, binary or wheel. A test binary installs the allocator and
//! measures one call:
//!
//! ```
//! use coreav_counting_alloc::{CountingAllocator, measure};
//!
//! #[global_allocator]
//! static ALLOC: CountingAllocator = CountingAllocator;
//!
//! let (value, counters) = measure(|| vec![0u8; 1 << 20]);
//! assert_eq!(value.len(), 1 << 20);
//! assert!(counters.peak_bytes >= (1 << 20));
//! ```
//!
//! Counters are per thread, so tests measuring at the same time do not mix,
//! and a region starts at zero, so what it reports is the growth it caused
//! rather than the process's total. A region that frees memory allocated
//! before it offsets its own growth by that much, as the process does;
//! [`Counters::trough_bytes`] is zero exactly when no such offsetting
//! happened. Code that allocates on threads of its own is measured with
//! [`measure_process`] instead, which counts every thread, in a test binary
//! that holds that one test. The `unsafe` surface is the four
//! [`std::alloc::GlobalAlloc`] methods of [`counting`], each forwarding to
//! [`std::alloc::System`]; it is listed in `docs/unsafe-sites.md`.

// This crate's whole `unsafe` surface: one
// `GlobalAlloc` implementation, confined to this module.
#[allow(unsafe_code)]
mod counting;

mod process;

pub use counting::{Counters, CountingAllocator, measure};
pub use process::measure_process;

/// Prints one JSON line of evidence for a measurement called `name`, so
/// that a CI run's log holds the numbers behind every bar it checked. Call
/// it outside the measured region: printing allocates.
pub fn report(name: &str, counters: &Counters) {
    println!(
        "{{\"measurement\":{:?},\"peak_bytes\":{},\"trough_bytes\":{},\
         \"bytes\":{},\"allocations\":{},\"reallocations\":{},\"live_bytes\":{}}}",
        name,
        counters.peak_bytes,
        counters.trough_bytes,
        counters.bytes,
        counters.allocations,
        counters.reallocations,
        counters.live_bytes,
    );
}
