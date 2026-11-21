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
//! A setup that has requested more than [`SETUP_ALLOCATION_LIMIT_BYTES`], consumed more than its CPU
//! budget ([`SETUP_CPU_BUDGET_NANOS`] by default, `[flow] setup_cpu_budget_ms`), or spent more than
//! the per-text budget on one text ([`TEXT_CPU_BUDGET_NANOS`], [`TEXT_ALLOCATION_LIMIT_BYTES`]),
//! yields **no facts**. That is an ordinary outcome, not an error: without facts every window of
//! the page is the full computation (`FallbackReason::FactsAbsent`), which is exactly what a page
//! took before the incremental window existed, bounded by the 50 ms CPU ceiling of the metered
//! window. The worker tells the host which guard fired in its marker byte
//! (`isolation::wire::marker_byte`), so the host knows it for an attempt that is accepted, refused
//! or killed alike; no refusal, no failure, and nothing counts toward a document's cooldown.
//!
//! The setup has decoded the texts it read on the engine it holds, and the decode is memoised in the
//! document: a full window that ran on that engine would find it paid for and never meter it. After
//! a fired guard the worker therefore loads the base again and prewarms it, and the window starts from
//! that fresh engine, so the decode of every text is metered again exactly as it was for every write
//! before the incremental window existed. A text whose decode alone passes the CPU ceiling is then
//! refused with the content limit, which is the answer the page's owner can act on.
//!
//! A CPU clock that cannot be read fails closed: no clock, no facts (reported as [`Fired::Cpu`], or
//! [`Fired::Text`] for the per-text budget).
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
//! Per text, it reads the guard around the decode and the conversion of that text: before the
//! decode, after the decode (before the conversion), and after the conversion, and the per-text
//! budget is held against what the text itself cost between those reads.
//!
//! It does **not** interrupt a single library call. One call can overshoot a limit by what it costs
//! by itself:
//!
//! * the decode of the snapshot in `LoroCollabEngine::load` and the prewarm that follows it, linear
//!   in the snapshot and bounded by the snapshot size limit (`InputLimits::snapshot_bytes_max`,
//!   64 MiB). A Loro snapshot carries the whole history of the page, so a long-lived page comes
//!   near that limit after about a thousand updates of the maximal size; nothing in the setup
//!   interrupts the load of it, and the time it takes is not measured here. The 5 s setup timeout
//!   and the host's backstop bound it;
//! * the decode and conversion of one text: bounded by the per-text budget, see below;
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
//! # The per-text budget
//!
//! The page-level budgets cannot cut one call, and the setup decodes every text it reads outside any
//! window. Before the incremental window existed every write decoded and converted the page inside
//! the 50 ms CPU ceiling, so a text that alone needs more than that could never have been accepted
//! into a page. The setup is the only place where such a text can be decoded unmetered, and a text
//! can be grown to that size one accepted window at a time (nested styles of one key over one range:
//! a few marks per update, each op cheap for the window, the decode quadratic in their number). So
//! the setup measures each text's own cost, the same way it measures everything (actual work, not a
//! price list): CPU time of the calling thread, and allocated bytes as the deterministic companion.
//! A text that costs more than [`TEXT_CPU_BUDGET_NANOS`] (the window's own ceiling, 50 ms) or
//! [`TEXT_ALLOCATION_LIMIT_BYTES`] (48 MiB) yields no facts for the page, reported as
//! [`Fired::Text`]; the page is decided by the full window, where the text's decode is metered
//! again. The per-text budget follows the ceiling and is not configurable.
//!
//! `window_text_guard_tests::window_text_guard_calibration` (release, development machine; "x3.6" is
//! the reference runner) measures the cost the guard sees, the decode and the conversion of one text:
//!
//! | text | allocation | CPU here (x3.6) |
//! | --- | --- | --- |
//! | 100,000 characters (the block limit), plain | 0.5 MiB | 0.16 ms (0.6 ms) |
//! | the same after 20,000 edits by two peers | 9.0 MiB | 3.4 ms (12 ms) |
//! | the same under 500 clusters of bold, italic and a link | 7.6 MiB | 2.2 ms (8 ms) |
//! | 500 clusters after 5,000 edits (the heaviest within the item budget) | 10.3 MiB | 3.4 ms (12 ms) |
//! | 25,000 reverse-typed pieces | 8.9 MiB | 2.5 ms (9 ms) |
//! | 3,000 clusters (over the item budget: it has no facts anyway) | 44.9 MiB | 11.8 ms (42 ms) |
//! | nested same-key marks x 500 | 5.4 MiB | 12 ms (43 ms) |
//! | nested same-key marks x 830 | 14.1 MiB | 40 ms (145 ms) |
//! | nested same-key marks x 1,500 | 44.5 MiB | 178 ms (641 ms) |
//! | nested same-key marks x 4,000 | 304 MiB | 1.8 s (6.6 s) |
//!
//! Every honest text within the budget of items costs under 11.2 MiB and 12 ms of CPU on the
//! reference runner, a quarter of the CPU budget and a quarter of the allocation budget; the
//! allocation budget is the first to fire for shapes that allocate more than they compute, the CPU
//! budget for nested styles (which compute more than they allocate), on every machine.
//!
//! ## How far one window can push a text past the budget
//!
//! The guard reads a text after it has paid for it, so the first text over the budget costs its
//! whole decode once. A text that was just under the budget when a window accepted an update can be
//! pushed by that one window, which has the same CPU ceiling, and `window_text_growth_bound`
//! measures how far (a text of 830 nested marks costs 42 ms, just under 50 ms; the window of an
//! update that adds marks to it, CPU time of the window and the text's cost after it):
//!
//! | marks added | update | window | text afterwards |
//! | --- | --- | --- | --- |
//! | 100 | 0.8 KB | 15 ms | 54 ms (1.3x) |
//! | 200 | 1.5 KB | 31 ms | 72 ms (1.7x) |
//! | 400 | 3.0 KB | 62 ms | 98 ms (2.3x) |
//!
//! The window's ceiling is 50 ms of the same clock, so one accepted window adds about 320 marks and
//! the text is at most about 90 ms, 1.8 times its budget (from a text of 500 marks, 800 more cost
//! the window 82 ms and the text 91 ms; from 400, 400 more cost 27 ms and the text 33 ms). The
//! update size (64 KiB) is no bound here: 800 marks are 5.9 KB, the window's ceiling binds first.
//! The setup runs before every write, so the text is measured before the next update is imported:
//! it has no facts from then on, the full window meters its decode, and a decode over the ceiling is
//! killed there (no write can grow the text further, the full window being CPU-killed before it
//! accepts). The text is therefore never more than about twice the budget in any base this product
//! produces, and that is the most one uninterruptible call can cost the setup: about 1.8 times the
//! budget, which is 90 ms for a budget of 50 ms, not seconds. The ratio is what carries to another
//! host: the figures above were measured on the development machine against its own 50 ms (the
//! budget and the window's ceiling are the same host's clock), and the ratio has not been measured
//! on the reference runner. `window_text_guard_worst_setup` measures the page of
//! many texts each just under the budget (250 texts of 500 nested marks, 43 ms each on the
//! reference runner): the CPU budget cuts the setup at 2.51 s, the budget plus one text, where the
//! setup without a guard would take 10.7 s; that is half of the 5 s timeout. The worst total, a page that spent
//! the budget on other texts and then meets the one text that a window pushed to twice its budget,
//! is 2.5 s plus 0.1 s plus the load, about 2.7 s on the reference runner.
//!
//! A base that predates this guard can hold a text far over it (the review's growth had no bound):
//! its setup is one uninterruptible call of seconds. The host's backstop is the setup timeout: a
//! setup that times out having consumed its CPU budget is decided again with the full window, and
//! the document stays on the full window for the cooldown period (`write::decide_window`).
//!
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

/// The CPU budget of the setup in milliseconds, as [`SETUP_CPU_BUDGET_NANOS`].
pub const SETUP_CPU_BUDGET_MS_DEFAULT: u64 = SETUP_CPU_BUDGET_NANOS / 1_000_000;

/// The range of `[flow] setup_cpu_budget_ms`, in milliseconds.
///
/// The setup has a wall timeout of 5 s that no configuration moves, and the budget must leave room
/// under it for the load and for the one text that can run past it.
pub const SETUP_CPU_BUDGET_MS_RANGE: std::ops::RangeInclusive<u64> = 500..=4_000;

/// The CPU budget of the setup in nanoseconds for the host's setting.
///
/// The setting is `[flow] setup_cpu_budget_ms` in milliseconds, as the worker receives it in
/// `isolation::wire::SETUP_CPU_BUDGET_ENV`: it is taken when it is a number inside
/// [`SETUP_CPU_BUDGET_MS_RANGE`], and the default is used for an absent, unreadable or out-of-range
/// one. No setting can switch the guard off.
#[must_use]
pub fn cpu_budget_nanos_from_setting(setting: Option<&str>) -> u64 {
    setting
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|millis| SETUP_CPU_BUDGET_MS_RANGE.contains(millis))
        .map_or(SETUP_CPU_BUDGET_NANOS, |millis| millis * 1_000_000)
}

/// CPU time, in nanoseconds, that one text may cost the setup (its decode and its conversion): the
/// `decode_apply_cpu_ms` ceiling of the metered window, 50 ms.
///
/// Before the incremental window existed, every write decoded and converted the whole page inside
/// that window, so a text that alone needs more than the ceiling could never have been accepted into
/// a page. The setup pays the decode of every text outside any window, so this budget is what keeps
/// a text from being grown, update by update, to a cost that the one uninterruptible library call of
/// its decode cannot be cut off from. It is not configurable: it follows the ceiling.
pub const TEXT_CPU_BUDGET_NANOS: u64 = crate::isolation::DECODE_APPLY_CPU_MS_MAX * 1_000_000;

/// Bytes of allocation after which one text has cost the setup too much: 48 MiB.
///
/// The deterministic companion of [`TEXT_CPU_BUDGET_NANOS`]: it needs no clock, and is the same on
/// every machine. The largest honest text the documented limits allow costs 0.5 to 11.2 MiB (see the
/// calibration in the module documentation), so the budget is more than four times that; a text of
/// nested styles reaches it at about 1,500 marks.
pub const TEXT_ALLOCATION_LIMIT_BYTES: u64 = 48 * 1024 * 1024;

/// Which guard stopped the setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fired {
    /// The setup requested more than [`SETUP_ALLOCATION_LIMIT_BYTES`] from the allocator.
    Allocation,
    /// The setup consumed more than its CPU budget ([`SETUP_CPU_BUDGET_NANOS`] by default), or its
    /// CPU clock could not be read.
    Cpu,
    /// One text cost more than [`TEXT_CPU_BUDGET_NANOS`] of CPU time or
    /// [`TEXT_ALLOCATION_LIMIT_BYTES`] of allocation (or the clock could not be read).
    Text,
}

impl Fired {
    /// The name the telemetry uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Allocation => "allocation",
            Self::Cpu => "cpu",
            Self::Text => "text",
        }
    }
}

/// CPU time of the calling thread in nanoseconds, or `None` when the clock cannot be read.
///
/// A guard that cannot read its clock fails closed (no facts): a setup without a working clock is
/// not bounded by anything but the wall timeout.
#[allow(unsafe_code)]
#[must_use]
pub fn thread_cpu_nanos() -> Option<u64> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `&raw mut ts` is a valid, properly aligned out-pointer for a `timespec`;
    // `CLOCK_THREAD_CPUTIME_ID` is supported on Linux and macOS.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) };
    if rc != 0 {
        return None;
    }
    let (Ok(secs), Ok(nanos)) = (u64::try_from(ts.tv_sec), u64::try_from(ts.tv_nsec)) else {
        return None;
    };
    Some(secs.saturating_mul(1_000_000_000).saturating_add(nanos))
}

/// Where a text's own cost is measured from: the allocation and the CPU clock when its decode
/// began ([`SetupGuard::text_start`]).
#[derive(Debug, Clone, Copy)]
pub struct TextMark {
    bytes: u64,
    nanos: Option<u64>,
}

/// Counts the setup's allocation and CPU time from the moment it is started and says when either has
/// passed its limit, and when one text has cost more than its own budget.
#[derive(Debug, Clone, Copy)]
pub struct SetupGuard {
    start_bytes: u64,
    byte_limit: u64,
    /// The CPU clock when the guard started; `None` when it could not be read (the guard then
    /// fires, see [`Fired::Cpu`]).
    start_nanos: Option<u64>,
    cpu_budget: u64,
    text_byte_limit: u64,
    text_cpu_budget: u64,
    clock: fn() -> Option<u64>,
}

impl SetupGuard {
    /// A guard with the production limits, counting from now.
    #[must_use]
    pub fn start() -> Self {
        Self::start_with_cpu_budget(SETUP_CPU_BUDGET_NANOS)
    }

    /// A guard with the production limits and the given CPU budget for the whole setup in
    /// nanoseconds, counting from now.
    #[must_use]
    pub fn start_with_cpu_budget(cpu_budget: u64) -> Self {
        Self::with_limits(SETUP_ALLOCATION_LIMIT_BYTES, cpu_budget, thread_cpu_nanos)
            .with_text_limits(TEXT_ALLOCATION_LIMIT_BYTES, TEXT_CPU_BUDGET_NANOS)
    }

    /// A guard with the given allocation limit and no CPU budget and no per-text budget, counting
    /// from now.
    #[must_use]
    pub fn with_limit(limit: u64) -> Self {
        Self::with_limits(limit, u64::MAX, thread_cpu_nanos)
    }

    /// A guard with both limits and the clock that reads the CPU time in nanoseconds (injectable for
    /// the tests), counting from now. A budget of `u64::MAX` never reads the clock. No per-text
    /// budget: see [`Self::with_text_limits`].
    #[must_use]
    pub fn with_limits(byte_limit: u64, cpu_budget: u64, clock: fn() -> Option<u64>) -> Self {
        Self {
            start_bytes: alloc_meter::requested_bytes(),
            byte_limit,
            start_nanos: if cpu_budget == u64::MAX { Some(0) } else { clock() },
            cpu_budget,
            text_byte_limit: u64::MAX,
            text_cpu_budget: u64::MAX,
            clock,
        }
    }

    /// This guard with a budget for each text: at most `text_bytes` of allocation and
    /// `text_cpu_nanos` of CPU time for the decode and the conversion of one text. `u64::MAX` for
    /// the CPU budget never reads the clock for it.
    #[must_use]
    pub const fn with_text_limits(mut self, text_bytes: u64, text_cpu_nanos: u64) -> Self {
        self.text_byte_limit = text_bytes;
        self.text_cpu_budget = text_cpu_nanos;
        self
    }

    /// Bytes requested since the guard was started.
    #[must_use]
    pub fn spent(&self) -> u64 {
        alloc_meter::requested_bytes().saturating_sub(self.start_bytes)
    }

    /// CPU nanoseconds consumed since the guard was started (zero when there is no budget), or
    /// `None` when the clock cannot be read.
    #[must_use]
    pub fn cpu_spent(&self) -> Option<u64> {
        if self.cpu_budget == u64::MAX {
            return Some(0);
        }
        Some((self.clock)()?.saturating_sub(self.start_nanos?))
    }

    /// Which guard, if any, the setup has passed.
    #[must_use]
    pub fn fired(&self) -> Option<Fired> {
        self.fired_by(0, 0)
    }

    /// Which guard, if any, the setup would have passed after `extra_bytes` more bytes and
    /// `extra_nanos` more CPU time. The allocation guard is asked first. A CPU clock that cannot
    /// be read counts as the CPU guard having fired.
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
        if !skip_cpu
            && self.cpu_budget != u64::MAX
            && self
                .cpu_spent()
                .is_none_or(|spent| spent.saturating_add(extra_nanos) > self.cpu_budget)
        {
            return Some(Fired::Cpu);
        }
        None
    }

    /// Where the cost of the text whose decode begins now is measured from.
    #[must_use]
    pub fn text_start(&self) -> TextMark {
        TextMark {
            bytes: alloc_meter::requested_bytes(),
            nanos: if self.text_cpu_budget == u64::MAX {
                Some(0)
            } else {
                (self.clock)()
            },
        }
    }

    /// Whether the text that began at `mark` has cost more than its budget, in allocation or in CPU
    /// time, by now: [`Fired::Text`]. A clock that cannot be read counts as the budget passed.
    #[must_use]
    pub fn text_fired(&self, mark: TextMark) -> Option<Fired> {
        #[cfg(test)]
        if crate::window_check::fault::active() == Some(crate::window_check::fault::Fault::TextGuardSkipped) {
            return None;
        }
        let bytes = alloc_meter::requested_bytes().saturating_sub(mark.bytes);
        if bytes > self.text_byte_limit {
            return Some(Fired::Text);
        }
        if self.text_cpu_budget != u64::MAX {
            let spent = match (mark.nanos, (self.clock)()) {
                (Some(before), Some(now)) => now.saturating_sub(before),
                _ => return Some(Fired::Text),
            };
            if spent > self.text_cpu_budget {
                return Some(Fired::Text);
            }
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
        /// The read (counted from 1) that is given extra time, and how much.
        static JUMP: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    }

    /// Starts the calling thread's clock at zero, advancing by `step_nanos` on every read.
    pub fn install(step_nanos: u64) {
        NOW.with(|now| now.set(0));
        STEP.with(|step| step.set(step_nanos));
        READS.with(|reads| reads.set(0));
        JUMP.with(|jump| jump.set(None));
    }

    /// Gives the `read_index`-th read (counted from 1 after [`install`]) `extra_nanos` on top of its
    /// step: a read that finds a long stretch of CPU time passed.
    pub fn jump_at(read_index: u64, extra_nanos: u64) {
        JUMP.with(|jump| jump.set(Some((read_index, extra_nanos))));
    }

    /// The clock for [`super::SetupGuard::with_limits`].
    #[allow(clippy::unnecessary_wraps, reason = "the signature of a guard's clock")]
    pub fn read() -> Option<u64> {
        let index = READS.with(|reads| {
            reads.set(reads.get() + 1);
            reads.get()
        });
        let extra = JUMP
            .with(Cell::get)
            .filter(|(at, _)| *at == index)
            .map_or(0, |(_, extra)| extra);
        Some(NOW.with(|now| {
            now.set(now.get().saturating_add(STEP.with(Cell::get)).saturating_add(extra));
            now.get()
        }))
    }

    /// A clock that cannot be read.
    pub fn dead() -> Option<u64> {
        READS.with(|reads| reads.set(reads.get() + 1));
        None
    }

    /// How many times the clock was read since [`install`].
    pub fn reads() -> u64 {
        READS.with(Cell::get)
    }
}
