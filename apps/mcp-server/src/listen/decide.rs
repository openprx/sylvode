//! Deciding many subscriptions at once, within a time budget and a bound on parallel API calls.

use super::uris::{Subscribable, Verdict};
use crate::server::McpServer;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;

/// API checks in flight at once for one batch, so one request cannot flood the API.
const PARALLEL_CHECKS: usize = 8;
/// The time the acknowledgment of a listen request may spend on its subscriptions (the spec wants
/// the acknowledgment within 5 s).
pub const ACK_BUDGET: Duration = Duration::from_secs(4);
/// The time a re-check of the open subscriptions of one credential may take.
pub const RECHECK_BUDGET: Duration = Duration::from_secs(10);

/// The verdict of each item, in order; `None` for one not decided in time. The checks that are
/// still running when the budget ends are abandoned.
pub async fn decide(
    server: &Arc<McpServer>,
    items: &[(Arc<str>, Subscribable)],
    budget: Duration,
) -> Vec<Option<Verdict>> {
    let deadline = Instant::now() + budget;
    let permits = Arc::new(Semaphore::new(PARALLEL_CHECKS));
    let mut checks = JoinSet::new();
    for (index, (uri, subscribable)) in items.iter().enumerate() {
        let server = Arc::clone(server);
        let permits = Arc::clone(&permits);
        let uri = Arc::clone(uri);
        let subscribable = subscribable.clone();
        checks.spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return (index, Verdict::Unavailable);
            };
            (index, server.subscription_verdict(&uri, &subscribable).await)
        });
    }
    let mut verdicts = vec![None; items.len()];
    while let Ok(Some(finished)) = tokio::time::timeout_at(deadline, checks.join_next()).await {
        if let Ok((index, verdict)) = finished
            && let Some(slot) = verdicts.get_mut(index)
        {
            *slot = Some(verdict);
        }
    }
    verdicts
}
