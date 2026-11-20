//! Test-only allocation counter: the unit tests count the bytes a piece of work allocates on the
//! calling thread, which measures how much the work did without a clock (a clock is noisy on a
//! shared runner). Counters are per thread, so tests running in parallel do not see each other.
//!
//! The allocator only wraps [`System`] and adds a thread-local add; the counter is a `Cell` of a
//! plain integer with a constant initialiser, so reading or bumping it never allocates.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

/// Bytes requested from the allocator by this thread so far (frees are not subtracted).
pub fn allocated_bytes() -> usize {
    ALLOCATED.with(Cell::get)
}

fn add(bytes: usize) {
    // `try_with`: an allocation during thread teardown must not panic.
    let _ = ALLOCATED.try_with(|total| total.set(total.get().saturating_add(bytes)));
}

struct CountingAllocator;

// SAFETY: every method delegates to `System` with the arguments it received, so the contract of
// `GlobalAlloc` is `System`'s own; the bookkeeping is a thread-local integer add that allocates
// nothing and so cannot re-enter this allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(layout.size());
        // SAFETY: `layout` is forwarded unchanged to `System::alloc`, which has the same contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` are forwarded unchanged to `System::dealloc`, which has the
        // same contract.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        add(layout.size());
        // SAFETY: `layout` is forwarded unchanged to `System::alloc_zeroed`, same contract.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        add(new_size.saturating_sub(layout.size()));
        // SAFETY: the arguments are forwarded unchanged to `System::realloc`, same contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
