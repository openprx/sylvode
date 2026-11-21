//! Single source of truth for the `contracts/limits-v1.md` "Isolated decode/apply" numbers this
//! isolation boundary enforces: `decode_apply_cpu_ms_max`, `decode_apply_wall_ms_max`,
//! `isolated_apply_memory_bytes_max`.
//!
//! Before this module existed, these three values (plus the CPU ceiling's microsecond form) were
//! declared independently in four places: `isolation::host` (re-exported for callers/error
//! messages), `isolation::alloc` (the actual counting-allocator threshold), `isolation::child_runtime`
//! (the actual `SIGPROF` timer arm), and `apps/api`'s `flow::collab::limits` (the `Bootstrap.limits`
//! wire report). Nothing tied those copies together, so editing the one a caller could see (the
//! wire report) silently left the ones that do the actual enforcing (the allocator threshold, the
//! timer) unchanged -- the reported ceiling and the enforced ceiling could drift apart with no
//! compiler error and no failing test. Every one of those sites now `use`/`pub use` the constants
//! defined here instead of declaring its own copy; see this module's own test for the guard
//! against that happening again.

/// `decode_apply_cpu_ms_max` (`contracts/limits-v1.md`). Consumed directly by
/// [`DECODE_APPLY_CPU_MS_MAX_MICROS`] below and re-exported (via `isolation::mod`) for
/// `IsolatedApplyError::CpuCeiling`'s reported `limit`/`observed` pair and the `Bootstrap.limits`
/// wire report.
pub const DECODE_APPLY_CPU_MS_MAX: u64 = 50;
/// `decode_apply_wall_ms_max` -- the independent wall-clock watchdog deadline `isolation::host`
/// enforces itself (see `host::read_response_two_phase`'s `metered_deadline`).
pub const DECODE_APPLY_WALL_MS_MAX: u64 = 100;
/// `isolated_apply_memory_bytes_max` (128 MiB) -- the counting-allocator threshold
/// `isolation::alloc::CountingAllocator` enforces.
pub const ISOLATED_APPLY_MEMORY_BYTES_MAX: u64 = 134_217_728;

/// The import-specific CPU budget, in milliseconds: the CPU a package import may spend decoding,
/// replaying and validating one uploaded document inside the isolated worker.
///
/// An import is one logical write of a whole document: unlike a collab write, whose base is the
/// server's own trusted state that the worker loads before its window opens, the snapshot and the
/// update tail of an upload are untrusted from the first byte, so their decode is metered too.
/// Measured on the reference runner in a release build (`tests/import_worker.rs`), with the
/// fingerprint of the current raw exporter's reader read in the window, the worker of a page at the
/// documented content limits (9,990 blocks of 100 characters, one million characters) consumes
/// 320 ms of CPU flat and 500 ms with the placement tree, and one of the same size whose history
/// holds a million operations 950 ms and between 192 and 256 MiB of counted allocation. None of
/// them fits the 50 ms, 100 ms and 128 MiB of a write window, so the import runs under its own
/// named budget: about twice the worst measurement for each of the three ceilings. Applied only
/// by the worker of the `import` operation ([`super::wire::OPERATION_IMPORT`]); no collab write,
/// diff or in-process path ever reads it.
pub const IMPORT_DECODE_CPU_MS_MAX: u64 = 2_000;
/// The import-specific wall-clock watchdog, in milliseconds (see [`IMPORT_DECODE_CPU_MS_MAX`]). It
/// also covers the unmetered export of the validated head that follows the window.
pub const IMPORT_DECODE_WALL_MS_MAX: u64 = 5_000;
/// The import-specific counting-allocator ceiling, in bytes (see [`IMPORT_DECODE_CPU_MS_MAX`]).
pub const IMPORT_ISOLATED_MEMORY_BYTES_MAX: u64 = 512 * 1024 * 1024;

/// [`DECODE_APPLY_CPU_MS_MAX`] in microseconds, for `child_runtime::arm_sigprof`'s `itimerval`
/// (`tv_usec` is `libc::suseconds_t`, `i64` on this target). Derived, not a separate frozen value,
/// so a future edit to the millisecond ceiling cannot leave the microsecond timer arm stale.
/// Linux-only, like its sole consumer `child_runtime`.
#[cfg(target_os = "linux")]
#[allow(
    clippy::cast_possible_wrap,
    reason = "DECODE_APPLY_CPU_MS_MAX * 1_000 is 50_000, far inside i64's range -- clippy is \
              warning generically about the u64->i64 cast, not this specific value"
)]
pub const DECODE_APPLY_CPU_MS_MAX_MICROS: i64 = (DECODE_APPLY_CPU_MS_MAX * 1_000) as i64;

/// [`IMPORT_DECODE_CPU_MS_MAX`] in microseconds, derived like [`DECODE_APPLY_CPU_MS_MAX_MICROS`].
#[cfg(target_os = "linux")]
#[allow(
    clippy::cast_possible_wrap,
    reason = "IMPORT_DECODE_CPU_MS_MAX * 1_000 is far inside i64's range"
)]
pub const IMPORT_DECODE_CPU_MS_MAX_MICROS: i64 = (IMPORT_DECODE_CPU_MS_MAX * 1_000) as i64;

// Compile-time guard on the derivation above: if a future edit changes the `* 1_000` factor (or
// hand-edits `DECODE_APPLY_CPU_MS_MAX_MICROS` back into a separate literal) without keeping it in
// sync with `DECODE_APPLY_CPU_MS_MAX`, this fails the build rather than silently arming a
// `SIGPROF` timer that no longer matches the millisecond value everything else reports.
#[cfg(target_os = "linux")]
#[allow(
    clippy::cast_possible_wrap,
    reason = "same justification as DECODE_APPLY_CPU_MS_MAX_MICROS's own cast above -- this is the \
              identical expression, re-checked at compile time"
)]
const _: () = assert!(DECODE_APPLY_CPU_MS_MAX_MICROS == DECODE_APPLY_CPU_MS_MAX as i64 * 1_000);

#[cfg(test)]
mod tests {
    use super::*;

    /// Documents (and pins) the frozen `contracts/limits-v1.md` values themselves, so a change to
    /// this module is a deliberate, reviewable diff to this test rather than an unnoticed edit.
    #[test]
    fn frozen_values_match_contracts_limits_v1() {
        assert_eq!(DECODE_APPLY_CPU_MS_MAX, 50);
        assert_eq!(DECODE_APPLY_WALL_MS_MAX, 100);
        assert_eq!(ISOLATED_APPLY_MEMORY_BYTES_MAX, 134_217_728);
        #[cfg(target_os = "linux")]
        assert_eq!(DECODE_APPLY_CPU_MS_MAX_MICROS, 50_000);
    }
}
