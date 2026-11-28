//! The write quota of an installation: a sliding window over all its invocations (design M10 §3.5).
//!
//! Per process, like the other gates of the host. Only attempts of write and destructive catalogue
//! calls count; reads are bounded by the per-invocation host-call limit.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use uuid::Uuid;

use super::error::HostError;
use super::limits::{WRITE_WINDOW, WRITES_PER_WINDOW};

/// A sliding window of write attempts per installation.
pub struct WriteWindow {
    limit: usize,
    span: Duration,
    hits: Mutex<HashMap<Uuid, VecDeque<Instant>>>,
}

impl WriteWindow {
    #[must_use]
    pub fn new(limit: usize, span: Duration) -> Self {
        Self {
            limit,
            span,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one write attempt of `installation` at `now`, or refuses `plugin_write_rate_limited`
    /// with the time until the oldest counted attempt leaves the window.
    ///
    /// # Errors
    /// `plugin_write_rate_limited` (HTTP 429 at the route, `retry_after_ms` in the details).
    pub fn hit(&self, installation: Uuid, now: Instant) -> Result<(), HostError> {
        let mut hits = self.hits.lock();
        hits.retain(|_, times| times.back().is_some_and(|last| now.duration_since(*last) < self.span));
        let times = hits.entry(installation).or_default();
        while times
            .front()
            .is_some_and(|first| now.duration_since(*first) >= self.span)
        {
            times.pop_front();
        }
        if times.len() >= self.limit {
            let wait = times
                .front()
                .map_or(self.span, |first| self.span.saturating_sub(now.duration_since(*first)));
            let retry_after_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX).max(1);
            return Err(HostError::write_rate_limited(retry_after_ms));
        }
        times.push_back(now);
        drop(hits);
        Ok(())
    }
}

/// The write window of this process.
pub fn writes() -> &'static WriteWindow {
    static WRITES: LazyLock<WriteWindow> = LazyLock::new(|| WriteWindow::new(WRITES_PER_WINDOW, WRITE_WINDOW));
    &WRITES
}
