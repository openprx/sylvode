//! Test-only global allocator: the unit tests count the bytes a piece of work allocates on the
//! calling thread, which measures how much the work did without a clock (a clock is noisy on a
//! shared runner). The counter is [`crate::alloc_meter`]'s, per thread, so tests running in parallel
//! do not see each other.

#[global_allocator]
static ALLOCATOR: crate::alloc_meter::MeteredSystem = crate::alloc_meter::MeteredSystem;

/// Bytes requested from the allocator by this thread so far (frees are not subtracted).
pub fn allocated_bytes() -> usize {
    usize::try_from(crate::alloc_meter::requested_bytes()).unwrap_or(usize::MAX)
}
