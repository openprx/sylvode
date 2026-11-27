//! Sliding-window limits of the installation routes (contract C1 §5): a request, an upgrade or a
//! preview compiles the package's components, so each workspace gets a few per hour.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use uuid::Uuid;

use crate::catalogue::{Refusal, RefusalCode};
use crate::error::ApiError;

/// Installation requests and upgrades per workspace per hour.
pub const REQUESTS_PER_HOUR: usize = 10;
/// Previews per workspace per hour (a preview compiles too, but only to show the result).
pub const PREVIEWS_PER_HOUR: usize = 60;
const WINDOW: Duration = Duration::from_secs(3600);

/// A per-workspace window of hits.
pub struct Window {
    limit: usize,
    hits: Mutex<HashMap<Uuid, VecDeque<Instant>>>,
}

impl Window {
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Counts one hit of `workspace` at `now`, or refuses.
    ///
    /// # Errors
    /// `installation_rate_limited` (HTTP 429).
    pub fn hit(&self, workspace: Uuid, now: Instant) -> Result<(), ApiError> {
        let mut hits = self.hits.lock();
        hits.retain(|_, times| times.back().is_some_and(|last| now.duration_since(*last) < WINDOW));
        let times = hits.entry(workspace).or_default();
        while times.front().is_some_and(|first| now.duration_since(*first) >= WINDOW) {
            times.pop_front();
        }
        if times.len() >= self.limit {
            return Err(Refusal::new(RefusalCode::InstallationRateLimited, None).into());
        }
        times.push_back(now);
        Ok(())
    }
}

/// The window of requests and upgrades.
pub fn requests() -> &'static Window {
    static WINDOW_OF_REQUESTS: LazyLock<Window> = LazyLock::new(|| Window::new(REQUESTS_PER_HOUR));
    &WINDOW_OF_REQUESTS
}

/// The window of previews.
pub fn previews() -> &'static Window {
    static WINDOW_OF_PREVIEWS: LazyLock<Window> = LazyLock::new(|| Window::new(PREVIEWS_PER_HOUR));
    &WINDOW_OF_PREVIEWS
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{REQUESTS_PER_HOUR, Window};

    #[test]
    fn a_workspace_gets_its_hits_per_hour() {
        let window = Window::new(REQUESTS_PER_HOUR);
        let (workspace, other) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let start = Instant::now();
        for _ in 0..REQUESTS_PER_HOUR {
            window.hit(workspace, start).expect("within the limit");
        }
        assert!(window.hit(workspace, start).is_err());
        assert!(window.hit(other, start).is_ok());
        window
            .hit(workspace, start + Duration::from_secs(3601))
            .expect("the hour passed");
    }
}
