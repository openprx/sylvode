//! The work guard of the worker's unmetered setup.
//!
//! The isolated worker loads the base, prewarms it and computes the base facts
//! ([`crate::facts::BaseFacts`]) before its metered window opens, so neither the CPU timer nor the
//! memory ceiling bounds that part. What bounds it is the work measure of [`crate::work`], which
//! prices what the library's conversions are known to cost (characters, delta segments, internal
//! pieces, attribute instances). It cannot price what it cannot see: the style anchors that stay in
//! a text after the text they marked is deleted, and null-valued styles (`unmark`) are read by every
//! conversion and appear in no delta, so a page can sit inside its budget while its setup runs for
//! seconds (the review's R7-1). Pricing one more shape at a time does not end that.
//!
//! The guard measures the work the setup does instead. [`crate::alloc_meter`] counts the bytes the
//! thread has requested from the allocator, cumulatively. Every conversion the setup pays for
//! allocates in proportion to what it does (a style map per range it passes, a string per piece, a
//! set insert per annotation), so the count grows with the work whatever the shape, is the same for
//! the same input and library build on every machine, needs no clock and no signal, and costs one
//! thread-local read per check.
//!
//! # What happens at the limit
//!
//! A setup that has requested more than [`SETUP_ALLOCATION_LIMIT_BYTES`], or consumed more than
//! [`SETUP_CPU_BUDGET_NANOS`] of CPU time, yields **no facts**. That
//! is an ordinary outcome, not an error: without facts every window of the page is the full
//! computation (`FallbackReason::FactsAbsent`), which is exactly what a page took before the
//! incremental window existed, bounded by the 50 ms CPU ceiling of the metered window. The worker
//! reports which guard fired (`setup_guard` on the wire, and a log line of the host); no
//! refusal, no failure, and nothing counts toward a document's cooldown.
//!
//! # What it covers
//!
//! The guard is started before the base is loaded, so its count includes the load, the prewarm and
//! the whole of the facts computation. It is **checked** at these points: before and after the
//! count of the page's items, after every text of that count (between the decode of a text's style
//! state and the conversion of its delta, and again after it), and after the projection that the
//! facts are built from. The count of the page's items converts every text the projection converts,
//! so what comes after it converts the same texts again: the work that follows a passed check is of
//! the same size as the work the check measured.
//!
//! It does **not** interrupt a single library call. One call can overshoot the limit by what it
//! costs by itself:
//!
//! * the decode of the snapshot in `LoroCollabEngine::load`, bounded by the snapshot size limit
//!   (`InputLimits::snapshot_bytes_max`, 64 MiB), which no page that was accepted through a window
//!   can approach: every update was 64 KiB at most;
//! * the decode and conversion of one text, which every window that wrote to the text paid for under
//!   the 50 ms CPU ceiling (the incremental window converts a touched text twice, the full window
//!   once);
//! * the projection that follows the count, which converts the texts the count has just measured.
//!
//! # The limit
//!
//! `window_setup_guard_tests::window_setup_guard_calibration` (release; the debug build allocates
//! within 4% of the same figures) measures the whole setup, load and prewarm included. "x3.6" is the
//! reference runner's factor:
//!
//! | page | setup allocation | setup time here (x3.6) |
//! | --- | --- | --- |
//! | flat, 9,950 blocks | 268 MiB | 87 to 150 ms (0.3 to 0.5 s) |
//! | 9,950 blocks, one bold+italic+link cluster | 664 MiB | 170 to 297 ms (0.6 to 1.1 s) |
//! | 9,950 blocks, two clusters | 991 to 1,003 MiB | 251 to 335 ms (0.9 to 1.2 s) |
//! | 9,950 blocks, six marks | 978 to 986 MiB | 241 to 319 ms (0.9 to 1.1 s) |
//! | 2,000 blocks, six marks | 155 MiB | 46 to 59 ms |
//! | 1,200 blocks, two clusters | 94 MiB | 27 to 36 ms |
//! | 9,950 blocks, 24 texts of 4,000 reverse-typed characters under 15 marks (R6-1) | 1,324 MiB | 265 to 373 ms (1.0 to 1.3 s) |
//! | 240 such blocks (the review of R6, refused by the work ceiling) | 886 MiB | 145 to 193 ms |
//! | one text with 20,000 anchors of deleted marked text (R7-1) | 26.6 MiB | 6 to 8 ms |
//! | one text, 4,000 reverse-typed pieces, 100 `unmark` keys (R7-1) | 67 MiB | 6 ms |
//! | one text with 300 nested same-key marks (R7-1) | 3 MiB | 7 ms |
//! | one text with 4,000 nested same-key marks | 307 MiB | 1.5 s (5.3 s) |
//!
//! The count of the page's items is the larger part of the setup's work (a page's count is 1 to 2
//! times the projection that follows it, 530 MiB against 236 MiB for two clusters), so the guard
//! holds the limit against the spent bytes plus the count's cost once more (the prediction of the
//! projection) before it runs the projection. The largest honest page of the table, two clusters on
//! 9,950 blocks, is 1,003 MiB spent and about 1,300 MiB with the prediction: the limit of 2 GiB is
//! twice the first and 1.58 times the second.
//!
//! At the limit, shapes whose time follows their allocation (anchors, null styles, pieces: 4 to 12
//! MiB per ms; honest pages: 2 to 4 MiB per ms) have run for 0.2 to 0.9 s here (0.7 to 3.3 s on the
//! reference runner), under the 5 s setup timeout.
//!
//! # The CPU budget
//!
//! One shape does not follow its allocation: nested styles of one key over one range decode in time
//! quadratic in their number without a matching allocation (0.2 to 0.5 MiB per ms). A window cannot
//! write more than about 350 of them into one text (its re-count converts the text under the 50 ms
//! ceiling: 12 ms here for 500), but a page can spread them over many texts:
//!
//! | page | count | projection after it |
//! | --- | --- | --- |
//! | 250 blocks, 200 texts x 300 nested marks | 444 MiB, 795 ms (2.9 s on the reference runner) | 14 MiB, 11 ms |
//! | 250 blocks, 200 texts x 350 nested marks | 580 MiB, 1.1 s (4.0 s) | 17 MiB, 13 ms |
//!
//! Both are under the allocation limit, so a second guard bounds the CPU time the setup has
//! consumed ([`SETUP_CPU_BUDGET_NANOS`], 2.5 s): the calling thread's CPU clock, which a starved host
//! does not advance. It is read at the same checkpoints as the allocation, and fires the same way
//! (no facts, the full window), reported as [`Fired::Cpu`]. The clock is a parameter of
//! [`SetupGuard::with_limits`], so tests drive it without real time. The projection after the count is
//! predicted at the count's CPU time, as it is for the allocation: measured projection to count
//! ratios are 0.77 to 0.95 for honest pages, 0.97 for the fragmented text, 0.01 for nested styles
//! (the decode, which is the cost, is paid by the count).
//!
//! Budget and honest pages: the heaviest honest page (9,950 blocks, two clusters or six marks)
//! spends 0.25 to 0.34 s in all here, about 0.9 to 1.2 s on the reference runner, of which the
//! count is about 0.14 s here (0.5 s there); with load, prewarm and the predicted projection that is
//! about 1.4 s there against 2.5 s, a margin of 1.8.
//!
//! The worst single call after the last checkpoint that passed is one of two:
//!
//! * the decode and conversion of one text. A text in a base was accepted by a window that metered
//!   its conversion under the 50 ms CPU ceiling of the reference runner (the incremental window
//!   converts a touched text twice, the full window once), so one text costs at most about 50 ms
//!   there: measured, a text with 500 nested marks (the most a window admits) costs 12 ms here,
//!   44 ms there; a text of 4,000 reverse-typed characters under 40 marks costs 8.6 ms here. A text
//!   that no window ever metered would cost more (1,500 nested marks: 162 ms here, 0.6 s there; 4,000:
//!   1.95 s here), and the documented limits alone do not exclude one: the snapshot limit of 64 MiB
//!   holds far more. Such a base cannot come out of a window of this product.
//! * `semantic_snapshot`, which the prediction rules out unless it fits the budget: after the count
//!   passed, `spent + count` is within the budget, and the projection costs at most about the count.
//!
//! So the setup ends at about the budget plus one metered text, 2.5 s plus about 50 ms of CPU on the
//! reference runner, half of the 5 s timeout. The margin is for wall time that exceeds CPU time
//! (a starved host), which the 5 s wall timeout can still cut, as it does for any setup.

use crate::alloc_meter;

/// Bytes of allocation after which the setup yields no facts.
///
/// Chosen from `window_setup_guard_tests::window_setup_guard_calibration` (see the table in the
/// module documentation).
pub const SETUP_ALLOCATION_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// CPU time, in nanoseconds, after which the setup yields no facts: 2.5 s.
///
/// The allocation limit bounds most shapes, but work that does not allocate in proportion (nested
/// styles of one key decode in quadratic time) is bounded by the CPU the setup has consumed (the
/// calling thread's CPU clock, not wall time, so a starved host cannot trip it). The heaviest honest
/// page costs 0.25 to 0.34 s here, 0.9 to 1.2 s on the reference runner; the budget is twice that
/// there, and with the single call that can still run after the last passed checkpoint (see the
/// module documentation) stays under the 5 s setup timeout. Like the 50 ms ceiling of the window it
/// is machine-dependent, and its only effect is to choose the full window.
pub const SETUP_CPU_BUDGET_NANOS: u64 = 2_500_000_000;

/// Which guard stopped the setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fired {
    /// The setup requested more than [`SETUP_ALLOCATION_LIMIT_BYTES`] from the allocator.
    Allocation,
    /// The setup consumed more than [`SETUP_CPU_BUDGET_NANOS`] of CPU time.
    Cpu,
}

impl Fired {
    /// The name the telemetry uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Allocation => "allocation",
            Self::Cpu => "cpu",
        }
    }
}

/// CPU time of the calling thread in nanoseconds. Zero when the clock cannot be read, which only
/// makes the CPU guard inert.
#[allow(unsafe_code)]
#[must_use]
pub fn thread_cpu_nanos() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `&raw mut ts` is a valid, properly aligned out-pointer for a `timespec`;
    // `CLOCK_THREAD_CPUTIME_ID` is supported on Linux and macOS.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) };
    if rc != 0 {
        return 0;
    }
    let (Ok(secs), Ok(nanos)) = (u64::try_from(ts.tv_sec), u64::try_from(ts.tv_nsec)) else {
        return 0;
    };
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// Counts the setup's allocation and CPU time from the moment it is started and says when either has
/// passed its limit.
#[derive(Debug, Clone, Copy)]
pub struct SetupGuard {
    start_bytes: u64,
    byte_limit: u64,
    start_nanos: u64,
    cpu_budget: u64,
    clock: fn() -> u64,
}

impl SetupGuard {
    /// A guard with the production limits, counting from now.
    #[must_use]
    pub fn start() -> Self {
        Self::with_limits(SETUP_ALLOCATION_LIMIT_BYTES, SETUP_CPU_BUDGET_NANOS, thread_cpu_nanos)
    }

    /// A guard with the given allocation limit and no CPU budget, counting from now.
    #[must_use]
    pub fn with_limit(limit: u64) -> Self {
        Self::with_limits(limit, u64::MAX, thread_cpu_nanos)
    }

    /// A guard with both limits and the clock that reads the CPU time in nanoseconds (injectable for
    /// the tests), counting from now. A budget of `u64::MAX` never reads the clock.
    #[must_use]
    pub fn with_limits(byte_limit: u64, cpu_budget: u64, clock: fn() -> u64) -> Self {
        Self {
            start_bytes: alloc_meter::requested_bytes(),
            byte_limit,
            start_nanos: if cpu_budget == u64::MAX { 0 } else { clock() },
            cpu_budget,
            clock,
        }
    }

    /// Bytes requested since the guard was started.
    #[must_use]
    pub fn spent(&self) -> u64 {
        alloc_meter::requested_bytes().saturating_sub(self.start_bytes)
    }

    /// CPU nanoseconds consumed since the guard was started (zero when there is no budget).
    #[must_use]
    pub fn cpu_spent(&self) -> u64 {
        if self.cpu_budget == u64::MAX {
            return 0;
        }
        (self.clock)().saturating_sub(self.start_nanos)
    }

    /// Which guard, if any, the setup has passed.
    #[must_use]
    pub fn fired(&self) -> Option<Fired> {
        self.fired_by(0, 0)
    }

    /// Which guard, if any, the setup would have passed after `extra_bytes` more bytes and
    /// `extra_nanos` more CPU time. The allocation guard is asked first.
    #[must_use]
    pub fn fired_by(&self, extra_bytes: u64, extra_nanos: u64) -> Option<Fired> {
        #[cfg(test)]
        let active = crate::window_check::fault::active();
        #[cfg(test)]
        let skip_allocation = active == Some(crate::window_check::fault::Fault::SetupGuardSkipped);
        #[cfg(not(test))]
        let skip_allocation = false;
        #[cfg(test)]
        let skip_cpu = active == Some(crate::window_check::fault::Fault::SetupCpuGuardSkipped);
        #[cfg(not(test))]
        let skip_cpu = false;
        if !skip_allocation && self.spent().saturating_add(extra_bytes) > self.byte_limit {
            return Some(Fired::Allocation);
        }
        if !skip_cpu && self.cpu_budget != u64::MAX && self.cpu_spent().saturating_add(extra_nanos) > self.cpu_budget {
            return Some(Fired::Cpu);
        }
        None
    }
}

/// A CPU clock that the tests control: every read advances it by a fixed step, so a setup that
/// reads it at the same checkpoints always sees the same time, whatever the machine.
#[cfg(test)]
pub(crate) mod stepping_clock {
    use std::cell::Cell;

    thread_local! {
        static NOW: Cell<u64> = const { Cell::new(0) };
        static STEP: Cell<u64> = const { Cell::new(0) };
        static READS: Cell<u64> = const { Cell::new(0) };
    }

    /// Starts the calling thread's clock at zero, advancing by `step_nanos` on every read.
    pub fn install(step_nanos: u64) {
        NOW.with(|now| now.set(0));
        STEP.with(|step| step.set(step_nanos));
        READS.with(|reads| reads.set(0));
    }

    /// The clock for [`super::SetupGuard::with_limits`].
    pub fn read() -> u64 {
        READS.with(|reads| reads.set(reads.get() + 1));
        NOW.with(|now| {
            now.set(now.get().saturating_add(STEP.with(Cell::get)));
            now.get()
        })
    }

    /// How many times the clock was read since [`install`].
    pub fn reads() -> u64 {
        READS.with(Cell::get)
    }
}
