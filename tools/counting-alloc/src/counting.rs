//! The allocator itself: this crate's only module that uses `unsafe`
//! (`docs/unsafe-sites.md`).
//!
//! Review contract. Each [`GlobalAlloc`] method forwards the caller's
//! pointer, layout and new size unchanged to the same method of [`System`]
//! and returns [`System`]'s pointer unchanged; no pointer is ever
//! dereferenced, compared except against null, or kept. Accounting runs
//! only after the forwarded call, only when it succeeded, and touches one
//! thread-local `Cell` of plain integers: it cannot allocate, format,
//! lock, unwind or otherwise re-enter the allocator; neither can the
//! process-wide counters it also feeds ([`crate::process`]), which are
//! atomic integers. A failed `realloc`
//! leaves the old allocation live and the counters unchanged.
//!
//! The metric is requested bytes, as `Layout::size` gives them: not the
//! allocator's rounding, not its own bookkeeping, and not the process's
//! resident set.

use core::cell::Cell;
use core::fmt;
use std::alloc::{GlobalAlloc, Layout, System};

/// One thread's counters, and whether it is inside a measured region.
#[derive(Clone, Copy)]
struct State {
    on: bool,
    live: i64,
    peak: i64,
    trough: i64,
    allocations: u64,
    reallocations: u64,
    bytes: u64,
}

impl State {
    /// Outside any region, with nothing counted.
    const IDLE: Self = Self {
        on: false,
        live: 0,
        peak: 0,
        trough: 0,
        allocations: 0,
        reallocations: 0,
        bytes: 0,
    };
}

thread_local! {
    /// Per thread: `cargo test` runs several tests at a time, each on its
    /// own thread, and one shared counter would mix them.
    ///
    /// Const-initialized and free of `Drop`, so on every target whose std
    /// has native thread-local storage (every host this is built for) it
    /// lowers to a plain `#[thread_local]` static: reading it allocates
    /// nothing, registers no destructor and cannot fail, which is what
    /// keeps the accounting below out of the allocator it accounts for. A
    /// target whose std falls back to OS-key storage would box the value
    /// on first use per thread, and that first allocation would re-enter
    /// this allocator; `try_with` would not save it. Such a target needs a
    /// different store, not a different comment.
    static STATE: Cell<State> = const { Cell::new(State::IDLE) };
}

fn update(change: impl FnOnce(State) -> State) {
    let _ = STATE.try_with(|state| state.set(change(state.get())));
}

fn widen(bytes: usize) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

fn count(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

/// A successful `alloc` or `alloc_zeroed` of `bytes`.
fn obtained(bytes: usize) {
    crate::process::obtained(widen(bytes), count(bytes));
    update(|mut state| {
        if state.on {
            state.allocations = state.allocations.saturating_add(1);
            state.bytes = state.bytes.saturating_add(count(bytes));
            state.live = state.live.saturating_add(widen(bytes));
            state.peak = state.peak.max(state.live);
        }
        state
    });
}

/// A `dealloc` of `bytes`.
fn released(bytes: usize) {
    crate::process::released(widen(bytes));
    update(|mut state| {
        if state.on {
            state.live = state.live.saturating_sub(widen(bytes));
            state.trough = state.trough.min(state.live);
        }
        state
    });
}

/// A successful `realloc` from `old` to `new` bytes.
///
/// A reallocation that cannot grow in place holds the old block and the
/// new one at once while it copies. Which of the two happened is not
/// observable from here, so the peak takes the larger reading: a bound
/// that is never below the truth is worth more than one that is exact
/// only when the allocator happened to grow in place.
fn resized(old: usize, new: usize) {
    crate::process::resized(widen(old), widen(new), count(new.saturating_sub(old)));
    update(|mut state| {
        if state.on {
            state.reallocations = state.reallocations.saturating_add(1);
            state.bytes = state.bytes.saturating_add(count(new.saturating_sub(old)));
            // The peak is taken before the delta, because what is live
            // during the copy is the old block and the new one. That is
            // never below what is live after, so there is nothing left to
            // raise it with afterwards.
            state.peak = state.peak.max(state.live.saturating_add(widen(new)));
            state.live = state
                .live
                .saturating_add(widen(new))
                .saturating_sub(widen(old));
            state.trough = state.trough.min(state.live);
        }
        state
    });
}

/// A global allocator that forwards to [`System`] and counts what passes
/// through it. A test binary installs it with `#[global_allocator]`.
pub struct CountingAllocator;

// SAFETY: every method forwards the caller's arguments unchanged to the
// same method of `System`, which is a correct `GlobalAlloc`, and returns
// its pointer unchanged, so the contract this implementation offers is
// `System`'s. The counting that follows reads no memory through any
// pointer and allocates nothing.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller met `System::alloc`'s contract for `layout`.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            obtained(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller met `System::alloc_zeroed`'s contract for
        // `layout`.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            obtained(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller passed a live allocation of this allocator,
        // which is `System`'s, with the layout it was allocated with.
        unsafe { System.dealloc(ptr, layout) };
        released(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller passed a live allocation of this allocator,
        // its original layout, and a valid new size.
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            resized(layout.size(), new_size);
        }
        moved
    }
}

/// What one measured region allocated, on the thread that measured it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Successful `alloc` and `alloc_zeroed` calls.
    pub allocations: u64,
    /// Successful `realloc` calls.
    pub reallocations: u64,
    /// Bytes obtained: every allocation's size, plus the growth of every
    /// reallocation.
    pub bytes: u64,
    /// High-water mark of live bytes, counted from zero at the region's
    /// start: how much more memory was held at the region's worst moment
    /// than when it began, which is the growth a bound of "peak memory
    /// growth" is written about. It never goes below zero, and it counts a
    /// reallocation as holding its old and its new block at once, so it is
    /// never below the true high-water mark.
    ///
    /// Memory that the region frees and that was allocated before it
    /// offsets later growth, exactly as it does in the process: a region
    /// that frees a mebibyte and then takes one back never grew. Check
    /// [`Counters::trough_bytes`] where that would flatter a result.
    pub peak_bytes: u64,
    /// Low-water mark of live bytes, by the same count: zero unless the
    /// region freed memory allocated before it, negative by as much as it
    /// ever had freed without replacing. Zero proves that
    /// [`Counters::peak_bytes`] is growth and nothing else.
    pub trough_bytes: i64,
    /// Bytes allocated inside the region and still live at its end;
    /// negative when the region freed more than it allocated.
    pub live_bytes: i64,
}

impl Counters {
    fn of(state: State) -> Self {
        Self {
            allocations: state.allocations,
            reallocations: state.reallocations,
            bytes: state.bytes,
            // `peak` starts at zero and is only ever raised, so it cannot
            // be negative. Were that ever to stop holding, the saturating
            // value must be the one that fails a bar, not the one that
            // passes every bar there is.
            peak_bytes: u64::try_from(state.peak).unwrap_or(u64::MAX),
            trough_bytes: state.trough,
            live_bytes: state.live,
        }
    }

    /// Every successful allocator call that obtained memory.
    pub fn calls(&self) -> u64 {
        self.allocations.saturating_add(self.reallocations)
    }
}

impl fmt::Display for Counters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "peak {} B, trough {} B, {} B in {} allocations and {} reallocations, \
             {} B live at the end",
            self.peak_bytes,
            self.trough_bytes,
            self.bytes,
            self.allocations,
            self.reallocations,
            self.live_bytes
        )
    }
}

/// Runs `body` and returns its value together with what it allocated on
/// this thread.
///
/// Memory the body allocates and keeps (the value it returns, for example)
/// counts towards the peak. A region inside another region is measured on
/// its own and then added to the enclosing one, so a measured helper can be
/// called from a measured test. A body that spawns threads is not measured
/// beyond its own thread.
pub fn measure<T>(body: impl FnOnce() -> T) -> (T, Counters) {
    let outer = STATE.with(Cell::get);
    STATE.with(|state| {
        state.set(State {
            on: true,
            ..State::IDLE
        });
    });
    let value = body();
    let inner = STATE.with(Cell::get);
    STATE.with(|state| {
        state.set(State {
            on: outer.on,
            live: outer.live.saturating_add(inner.live),
            peak: outer.peak.max(outer.live.saturating_add(inner.peak)),
            trough: outer.trough.min(outer.live.saturating_add(inner.trough)),
            allocations: outer.allocations.saturating_add(inner.allocations),
            reallocations: outer.reallocations.saturating_add(inner.reallocations),
            bytes: outer.bytes.saturating_add(inner.bytes),
        });
    });
    (value, Counters::of(inner))
}
