//! Child-side (worker process) primitives: single-threadedness verification, the `RLIMIT_AS`
//! backstop, and arming/reading back the `SIGPROF` CPU-ceiling timer.
//!
//! `ADR-0014` section 1's online-enforcement table and section 1.1's arm sequence, adapted from
//! `spikes/collab-shared/src/isolation/child.rs` (which arms the identical timer for its
//! calibration fixtures) to this crate's actual apply workload. See
//! `crates/collab-core/src/isolation/host.rs`'s module doc for why this runs inside a freshly
//! `exec`'d process rather than `ADR-0014`'s fork-no-exec zygote.
//!
//! Everything here runs inside `src/bin/isolated_apply_worker.rs`'s single-purpose process, which
//! owns the actual exit path (`main`'s `libc::_exit`, not `std::process::exit`, matching the
//! calibration host's own fork-safety discipline -- see that file's module doc).

#![allow(unsafe_code)]

use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::limits::{
    DECODE_APPLY_CPU_MS_MAX_MICROS, IMPORT_DECODE_CPU_MS_MAX_MICROS, IMPORT_ISOLATED_MEMORY_BYTES_MAX,
};

/// `ADR-0014` section 3's `RLIMIT_AS` backstop -- independent of, and larger than, the
/// counting-allocator-enforced `isolated_apply_memory_bytes_max` (128 MiB). Catches allocations
/// that bypass the counting allocator entirely (a raw `mmap`, an FFI allocator, a JIT reservation).
pub const ADDRESS_SPACE_BACKSTOP_BYTES: u64 = 512 * 1024 * 1024;

/// The `RLIMIT_AS` backstop of the `import` operation: four times its counting-allocator ceiling,
/// the same relation the write path has between its 128 MiB ceiling and the 512 MiB backstop.
pub const IMPORT_ADDRESS_SPACE_BACKSTOP_BYTES: u64 = 4 * IMPORT_ISOLATED_MEMORY_BYTES_MAX;

/// `ADR-0014` section 10, self-assertion 1, adapted: verifies this process is single-threaded
/// using the kernel's own bookkeeping (`/proc/self/task`), not an assumption from startup
/// ordering. A freshly `exec`'d process image starts single-threaded by construction, but this
/// checks the actual fact rather than relying on that construction never changing (a future
/// dependency upgrade could start a background thread during its own static initialization,
/// for example).
#[must_use]
pub fn is_single_threaded() -> bool {
    std::fs::read_dir("/proc/self/task").is_ok_and(|entries| entries.count() == 1)
}

/// Sets `RLIMIT_AS` to [`ADDRESS_SPACE_BACKSTOP_BYTES`] for the whole process. Called once, before
/// the request is even read, so it applies to every allocation this process ever makes (not only
/// the metered window) -- `ADR-0014` section 10's `child.rs::apply_address_space_backstop` applies
/// it "for every case, not just the fixture built specifically to probe it", for the identical
/// reason: bypassing allocations can happen at any point, not only inside the counted window.
pub fn set_address_space_backstop() {
    set_address_space_limit(ADDRESS_SPACE_BACKSTOP_BYTES);
}

/// [`set_address_space_backstop`] with [`IMPORT_ADDRESS_SPACE_BACKSTOP_BYTES`], for the worker of
/// the `import` operation only.
pub fn set_address_space_backstop_for_import() {
    set_address_space_limit(IMPORT_ADDRESS_SPACE_BACKSTOP_BYTES);
}

fn set_address_space_limit(bytes: u64) {
    let limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    // SAFETY: `&limit` is a valid, fully-initialized `rlimit`; `RLIMIT_AS` is always a valid
    // resource on Linux. This runs at process startup before any request-driven allocation, so
    // lowering the limit here cannot invalidate memory this process already depends on (its
    // startup footprint is far below 512 MiB).
    unsafe {
        libc::setrlimit(libc::RLIMIT_AS, &raw const limit);
    }
}

fn empty_sigset() -> libc::sigset_t {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `set` is a valid, properly aligned `sigset_t` out-pointer.
    unsafe {
        libc::sigemptyset(set.as_mut_ptr());
        set.assume_init()
    }
}

fn sigprof_only_set() -> libc::sigset_t {
    let mut set = empty_sigset();
    // SAFETY: `set` was just initialized by `sigemptyset` above via `empty_sigset`.
    unsafe {
        libc::sigaddset(&raw mut set, libc::SIGPROF);
    }
    set
}

/// Arms the CPU ceiling (`ADR-0014` section 1.1): resets `SIGPROF`'s disposition to `SIG_DFL`
/// (whose default action is to terminate the process -- this *is* the online CPU-ceiling
/// enforcement, no signal handler required), unblocks it, and arms a one-shot
/// [`DECODE_APPLY_CPU_MS_MAX_MICROS`] CPU timer that delivers it (see [`arm_sigprof_micros`] for
/// which timer and why).
///
/// Must be called from the single process thread (verified by [`is_single_threaded`] before this
/// is ever invoked), immediately before the metered decode/apply/shape-validate work begins.
pub fn arm_sigprof() {
    arm_sigprof_micros(DECODE_APPLY_CPU_MS_MAX_MICROS);
}

/// [`arm_sigprof`] with the import-specific CPU budget ([`IMPORT_DECODE_CPU_MS_MAX_MICROS`]), for
/// the worker of the `import` operation only.
pub fn arm_sigprof_for_import() {
    arm_sigprof_micros(IMPORT_DECODE_CPU_MS_MAX_MICROS);
}

/// The exit status of a worker that could not arm its CPU ceiling. The host sees an exit that no
/// ceiling explains and reports a host failure: running the metered window without a ceiling is
/// never an option.
///
/// Also the status of a worker that could not close its window (`timer_delete` failed): unmetered
/// work under a live timer is no better than none.
pub const EXIT_CANNOT_ARM_CPU_CEILING: libc::c_int = 71;

/// Whether a CPU timer is armed in this process. The timer id below is meaningful only while this
/// is set: `0` is a valid `timer_t` (on glibc 2.34+ and musl it is the kernel timer id, and the
/// first timer of a process has id 0), so the id cannot also say "no timer".
static CPU_TIMER_ARMED: AtomicBool = AtomicBool::new(false);

/// The armed CPU timer of this process, as a `timer_t` held in an integer.
static CPU_TIMER_ID: AtomicUsize = AtomicUsize::new(0);

/// Exits the worker with [`EXIT_CANNOT_ARM_CPU_CEILING`]; never returns.
fn exit_cannot_arm() -> ! {
    // SAFETY: `_exit` is async-signal-safe and takes no pointers; this process owns its exit.
    unsafe { libc::_exit(EXIT_CANNOT_ARM_CPU_CEILING) }
}

fn arm_sigprof_micros(budget_micros: i64) {
    let mut old_action = MaybeUninit::<libc::sigaction>::uninit();
    // SAFETY: `libc::sigaction` is a C struct of plain integers/function-pointer-sized fields with
    // no Rust-level validity invariant beyond "some bit pattern"; every field this function
    // actually relies on (`sa_sigaction`, `sa_flags`, `sa_mask`) is explicitly overwritten below
    // before this value is passed to the `sigaction(2)` syscall.
    let mut default_action: libc::sigaction = unsafe { std::mem::zeroed() };
    default_action.sa_sigaction = libc::SIG_DFL;
    default_action.sa_flags = 0;
    default_action.sa_mask = empty_sigset();
    // SAFETY: `&default_action` is a valid, fully-initialized `sigaction`; `old_action` is a valid
    // out-pointer. `SIGPROF` is a valid signal number.
    let reset = unsafe { libc::sigaction(libc::SIGPROF, &raw const default_action, old_action.as_mut_ptr()) };
    if reset != 0 {
        // An inherited `SIG_IGN` would void the ceiling silently.
        exit_cannot_arm();
    }

    let sigprof_set = sigprof_only_set();
    // SAFETY: `sigprof_set` is a fully-initialized `sigset_t` containing only `SIGPROF`; passing
    // `null` for `oldset` is explicitly allowed by POSIX when the caller does not need the
    // previous mask.
    let unblocked = unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const sigprof_set, std::ptr::null_mut()) };
    if unblocked != 0 {
        // An inherited blocked mask would void the ceiling silently.
        exit_cannot_arm();
    }
    arm_cpu_timer(budget_micros);
}

/// Arms the one-shot timer that delivers `SIGPROF` once the process has consumed `budget_micros`
/// of CPU time (user plus system).
///
/// On Linux this is a POSIX timer on `CLOCK_PROCESS_CPUTIME_ID`, the clock the host measures the
/// worker with and the one the synthetic burn polls. `setitimer(ITIMER_PROF)` is not used: it is
/// driven by the tick-sampled user/system times, which charge a whole scheduler tick to whatever
/// runs when the tick fires, so under contention they run ahead of the CPU the process really
/// consumed. Measured on a loaded 2-CPU machine the 2 s ceiling fired up to 29 ms (1.5%) early.
/// The process-clock timer is checked against the scheduler's exact runtime: it never fires
/// before the ceiling is consumed and fires at most about a tick after it.
fn arm_cpu_timer(budget_micros: i64) {
    // SAFETY: `libc::sigevent` is a C struct for which all-zero is a valid value; the fields set
    // below are the ones `timer_create` reads for `SIGEV_SIGNAL`.
    let mut event: libc::sigevent = unsafe { std::mem::zeroed() };
    event.sigev_notify = libc::SIGEV_SIGNAL;
    event.sigev_signo = libc::SIGPROF;
    let mut timer: libc::timer_t = std::ptr::null_mut();
    // SAFETY: `event` and `timer` are valid for the call; the clock id is a valid per-process CPU
    // clock.
    let created = unsafe { libc::timer_create(libc::CLOCK_PROCESS_CPUTIME_ID, &raw mut event, &raw mut timer) };
    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
        it_value: libc::timespec {
            tv_sec: budget_micros / 1_000_000,
            tv_nsec: (budget_micros % 1_000_000) * 1_000,
        },
    };
    // SAFETY: only reached with a timer `timer_create` just made; `spec` is fully initialized and
    // a null old-value pointer is allowed.
    let armed = created == 0 && unsafe { libc::timer_settime(timer, 0, &raw const spec, std::ptr::null_mut()) } == 0;
    if !armed {
        exit_cannot_arm();
    }
    CPU_TIMER_ID.store(timer as usize, Ordering::SeqCst);
    CPU_TIMER_ARMED.store(true, Ordering::SeqCst);
}

/// Disarms the CPU timer, so it cannot fire *after* the metered window has already closed
/// successfully -- e.g. while this process is busy exporting the result snapshot and writing the
/// response frame, neither of which are protected work.
pub fn disarm_sigprof() {
    if CPU_TIMER_ARMED.swap(false, Ordering::SeqCst) {
        let timer = CPU_TIMER_ID.load(Ordering::SeqCst) as libc::timer_t;
        // SAFETY: `timer` was produced by `timer_create` in `arm_cpu_timer` and is deleted once
        // (the swap above clears the armed flag); deleting also drops a pending expiry.
        if unsafe { libc::timer_delete(timer) } != 0 {
            // A window that cannot be closed must not run unmetered work under a live timer.
            exit_cannot_arm();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_single_threaded_is_true_for_this_test_process_main_thread_check() {
        // `cargo test` runs many tests concurrently across multiple OS threads within one test
        // binary process, so this process is *not* single-threaded while the suite runs -- this
        // assertion documents that fact rather than asserting `true`, so a change to test-harness
        // threading does not silently make this check meaningless.
        // `is_single_threaded` itself must still return a real, non-panicking answer either way.
        let _ = is_single_threaded();
    }

    // `set_address_space_backstop` is deliberately not exercised here: it lowers the *calling
    // process's* `RLIMIT_AS` hard limit, which cannot be raised back afterwards. Calling it from
    // a `cargo test` unit test would permanently cap this whole test binary process (which runs
    // many unrelated tests concurrently in-process) at 512 MiB of address space for the rest of
    // the run, risking spurious failures far away from this module. It is only ever actually
    // called from `src/bin/isolated_apply_worker.rs`'s `main`, a fresh single-purpose process
    // that exits immediately after one apply, where that permanence is exactly the point.
}
