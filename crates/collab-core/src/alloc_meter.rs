//! A deterministic meter of the work the calling thread has done: the bytes it has requested from
//! the allocator, cumulatively.
//!
//! The setup of the isolated worker (load of the base, prewarm, base facts) runs before the metered
//! window opens, so no CPU timer bounds it. What can bound it without a clock (a clock is noisy and a
//! timer needs a signal handler) is the amount of allocation it does: every conversion the setup
//! pays for (decoding a text's style state, building a delta, a map per attribute) allocates in
//! proportion to the work, for a given library build and input, and nothing else changes it.
//! [`crate::setup_guard`] reads this meter.
//!
//! A global allocator feeds it: [`record`] is called by [`MeteredSystem`] (the allocator of the unit
//! tests and of the examples) and by `isolation::alloc::CountingAllocator` (the allocator of the
//! worker process). The count is a thread-local integer with a constant initialiser and no
//! destructor, so bumping or reading it never allocates and never registers a thread-local
//! destructor, and each thread (each test) sees only its own work. The worker is single-threaded
//! (`child_runtime::is_single_threaded` is checked at start), so for it the count is the process's.
//! Frees are not subtracted: the meter counts work, not the live size.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static REQUESTED: Cell<u64> = const { Cell::new(0) };
}

/// Bytes the calling thread has requested from the allocator so far, frees not subtracted. Zero
/// forever when no allocator calls [`record`] (see [`is_installed`]).
#[must_use]
pub fn requested_bytes() -> u64 {
    REQUESTED.with(Cell::get)
}

/// Adds `bytes` to the calling thread's count. Called by the global allocators of this crate;
/// allocates nothing.
#[inline]
pub fn record(bytes: usize) {
    // `try_with`: an allocation during thread teardown must not panic.
    let _ = REQUESTED.try_with(|total| total.set(total.get().saturating_add(bytes as u64)));
}

/// Whether the process's global allocator feeds the meter: an allocation moves the count.
///
/// The guard that reads the meter must not be silently off in a process whose allocator does not
/// feed it, so it asks this first and treats a dead meter as a reason not to build facts.
#[must_use]
pub fn is_installed() -> bool {
    let before = requested_bytes();
    let probe = std::hint::black_box(Vec::<u8>::with_capacity(64));
    let moved = requested_bytes() > before;
    drop(probe);
    moved
}

/// The system allocator with the meter: for a binary or a test target that has no allocator of its
/// own to feed [`record`] from.
pub struct MeteredSystem;

// SAFETY: every method delegates to `System` with the arguments it received, so the contract of
// `GlobalAlloc` is `System`'s own; the bookkeeping is a thread-local integer add that allocates
// nothing and so cannot re-enter this allocator.
unsafe impl GlobalAlloc for MeteredSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: `layout` is forwarded unchanged to `System::alloc`, which has the same contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` are forwarded unchanged to `System::dealloc`, which has the
        // same contract.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: `layout` is forwarded unchanged to `System::alloc_zeroed`, same contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size.saturating_sub(layout.size()));
        // SAFETY: the arguments are forwarded unchanged to `System::realloc`, same contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
