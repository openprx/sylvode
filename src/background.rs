//! Work that outlives the request that started it, and how it ends on shutdown.
//!
//! A `cli` run started from a webhook delivery, and every task the tunnel accepts, runs in the
//! background. On graceful shutdown each of them is cancelled: its process group is terminated
//! (see [`crate::process`]) and it sends its terminal callback, so no issue is left
//! `in_progress` by a run that no longer exists. The service then waits, up to a bound, for those
//! callbacks before it exits.

use std::future::Future;
use std::time::Duration;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
use tokio_util::task::TaskTracker;

#[derive(Clone, Debug, Default)]
pub struct Background {
    shutdown: CancellationToken,
    tracker: TaskTracker,
}

impl Background {
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs `task` in the background; shutdown waits for it.
    pub fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        self.tracker.spawn(task);
    }

    /// Resolves once shutdown has begun.
    pub fn cancelled(&self) -> WaitForCancellationFutureOwned {
        self.shutdown.clone().cancelled_owned()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Cancels every background run.
    pub fn begin_shutdown(&self) {
        self.shutdown.cancel();
    }

    /// The number of background tasks still running.
    pub fn running(&self) -> usize {
        self.tracker.len()
    }

    /// Accepts no more tasks and waits up to `bound` for the running ones. Returns whether all of
    /// them finished in time.
    pub async fn finish(&self, bound: Duration) -> bool {
        self.tracker.close();
        tokio::time::timeout(bound, self.tracker.wait()).await.is_ok()
    }
}
