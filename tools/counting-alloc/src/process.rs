//! Process-wide counters: what every thread allocates while a process-wide
//! region is open ([`measure_process`]), for code that allocates on threads
//! of its own, such as a decoder that decodes pictures on worker threads
//! (`docs/design/limits.md`, "Process-wide regions").
//!
//! No `unsafe` here: the allocator ([`crate::counting`]) calls the three
//! hooks below after each successful forwarded call, and they touch only
//! atomic integers, so they cannot allocate, lock or re-enter the
//! allocator.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use crate::counting::Counters;

/// Whether a process-wide region is open.
static ON: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicI64 = AtomicI64::new(0);
static PEAK: AtomicI64 = AtomicI64::new(0);
static TROUGH: AtomicI64 = AtomicI64::new(0);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static REALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
/// One process-wide region at a time.
static REGION: Mutex<()> = Mutex::new(());

/// A successful allocation of `bytes` (`size` as the byte total counts it).
pub(crate) fn obtained(bytes: i64, size: u64) {
    if ON.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(size, Ordering::Relaxed);
        let live = LIVE
            .fetch_add(bytes, Ordering::Relaxed)
            .saturating_add(bytes);
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
}

/// A deallocation of `bytes`.
pub(crate) fn released(bytes: i64) {
    if ON.load(Ordering::Relaxed) {
        let live = LIVE
            .fetch_sub(bytes, Ordering::Relaxed)
            .saturating_sub(bytes);
        TROUGH.fetch_min(live, Ordering::Relaxed);
    }
}

/// A successful reallocation from `old` to `new` bytes, `growth` bytes
/// more than before. As the thread-local count does, the peak counts the
/// old block and the new one as live at once.
pub(crate) fn resized(old: i64, new: i64, growth: u64) {
    if ON.load(Ordering::Relaxed) {
        REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(growth, Ordering::Relaxed);
        let before = LIVE.fetch_add(new.saturating_sub(old), Ordering::Relaxed);
        PEAK.fetch_max(before.saturating_add(new), Ordering::Relaxed);
        TROUGH.fetch_min(
            before.saturating_add(new).saturating_sub(old),
            Ordering::Relaxed,
        );
    }
}

/// Closes the open region when dropped, so a body that unwinds does not
/// leave the counters running.
struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        ON.store(false, Ordering::SeqCst);
    }
}

/// Runs `body` and returns its value together with what every thread of
/// the process allocated while it ran, the threads `body` starts as well
/// as its own.
///
/// What other threads allocate meanwhile is counted too, so a test binary
/// that measures this way holds that one test, and nothing else runs
/// beside the body: `cargo test` would run a binary's other tests at the
/// same time. The counters' meaning is [`Counters`]'s; one process-wide
/// region runs at a time (a region inside another waits for it forever),
/// and inside it [`crate::measure`] keeps counting its own thread as
/// before.
pub fn measure_process<T>(body: impl FnOnce() -> T) -> (T, Counters) {
    let _region = REGION.lock().unwrap_or_else(PoisonError::into_inner);
    for counter in [&LIVE, &PEAK, &TROUGH] {
        counter.store(0, Ordering::SeqCst);
    }
    for counter in [&ALLOCATIONS, &REALLOCATIONS, &BYTES] {
        counter.store(0, Ordering::SeqCst);
    }
    let open = Open;
    ON.store(true, Ordering::SeqCst);
    let value = body();
    drop(open);
    let peak = PEAK.load(Ordering::SeqCst);
    let counters = Counters {
        allocations: ALLOCATIONS.load(Ordering::SeqCst),
        reallocations: REALLOCATIONS.load(Ordering::SeqCst),
        bytes: BYTES.load(Ordering::SeqCst),
        // As in `Counters::of`: a negative peak cannot happen, and were it
        // to, the saturating value must fail a bar rather than pass it.
        peak_bytes: u64::try_from(peak).unwrap_or(u64::MAX),
        trough_bytes: TROUGH.load(Ordering::SeqCst),
        live_bytes: LIVE.load(Ordering::SeqCst),
    };
    (value, counters)
}
