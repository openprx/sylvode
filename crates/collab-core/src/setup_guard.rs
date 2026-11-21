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
//! A setup that has requested more than [`SETUP_ALLOCATION_LIMIT_BYTES`] yields **no facts**. That
//! is an ordinary outcome, not an error: without facts every window of the page is the full
//! computation (`FallbackReason::FactsAbsent`), which is exactly what a page took before the
//! incremental window existed, bounded by the 50 ms CPU ceiling of the metered window. The worker
//! reports that the guard fired (`setup_guard_fired` on the wire, and a log line of the host); no
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
//! reference runner), under the 5 s setup timeout. **One shape does not follow its allocation:**
//! nested styles of one key over one range decode in time quadratic in their number without a
//! matching allocation (0.2 to 0.5 MiB per ms). A window cannot write more than about 350 of them
//! into one text (its re-count converts the text under the 50 ms ceiling), which costs the setup 7
//! ms and 3 MiB, so a page that spreads them over 200 or more texts costs the setup about 1.4 s here
//! (5 s on the reference runner) for 600 to 800 MiB of allocation, below the limit.
//! This shape is not cut off long before the timeout by a limit that keeps every honest page
//! incremental, and needs a second measure.

use crate::alloc_meter;

/// Bytes of allocation after which the setup yields no facts.
///
/// Chosen from `window_setup_guard_tests::window_setup_guard_calibration` (see the table in the
/// module documentation).
pub const SETUP_ALLOCATION_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Counts the setup's allocation from the moment it is started and says when it has passed a limit.
#[derive(Debug, Clone, Copy)]
pub struct SetupGuard {
    start: u64,
    limit: u64,
}

impl SetupGuard {
    /// A guard with the production limit, counting from now.
    #[must_use]
    pub fn start() -> Self {
        Self::with_limit(SETUP_ALLOCATION_LIMIT_BYTES)
    }

    /// A guard with the given limit, counting from now.
    #[must_use]
    pub fn with_limit(limit: u64) -> Self {
        Self {
            start: alloc_meter::requested_bytes(),
            limit,
        }
    }

    /// Bytes requested since the guard was started.
    #[must_use]
    pub fn spent(&self) -> u64 {
        alloc_meter::requested_bytes().saturating_sub(self.start)
    }

    /// Whether the setup has passed the limit.
    #[must_use]
    pub fn exceeded(&self) -> bool {
        self.exceeded_by(0)
    }

    /// Whether the setup would have passed the limit after `extra` more bytes.
    #[must_use]
    pub fn exceeded_by(&self, extra: u64) -> bool {
        #[cfg(test)]
        if crate::window_check::fault::active() == Some(crate::window_check::fault::Fault::SetupGuardSkipped) {
            return false;
        }
        self.spent().saturating_add(extra) > self.limit
    }
}
