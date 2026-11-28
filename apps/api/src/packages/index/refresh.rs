//! The refresher: one background task per API process that keeps the cache of every configured
//! source current.
//!
//! Several API processes may run. A refresh of one source runs inside a transaction that holds
//! `pg_try_advisory_xact_lock` for the source, so only one process fetches it at a time; a process
//! that does not get the lock skips the source. The transaction stays open during the fetch
//! (at most 15 s) and ends with the cache write and the revocation sync, so a crash leaves the
//! previous state.

use std::time::Duration;

use platform::{app::AppState, config::IndexSource, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::json;
use sv_index::limits::MAX_INDEX_BYTES;

use super::{
    cache,
    fetch::{self, Answer},
    store, sync,
};
use crate::error::ApiError;

/// Namespace of the advisory lock; the second key is a hash of the source id ("SIDX").
const LOCK_NAMESPACE: i32 = 0x5349_4458;
/// How often the task looks at the sources; whether one is due depends on its own interval.
const TICK: Duration = Duration::from_secs(30);
/// Wait before the first pass, so a starting process does not fetch before it serves.
const FIRST_PASS_DELAY: Duration = Duration::from_secs(5);
/// A source whose last refresh failed is retried at most this late, whatever the interval.
const RETRY_AFTER_FAILURE_SECS: u64 = 600;

/// What one call to [`refresh_source`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Not due, or another process holds the source.
    Skipped,
    /// The index has not changed since the cached copy.
    Unchanged,
    /// A new document was cached and synced.
    Updated,
    /// The fetch or the document failed; the last good copy stays.
    Failed,
}

/// Starts the refresher task. It ends when `stop` turns `true` or its sender is dropped.
pub fn spawn_refresher(state: AppState, mut stop: tokio::sync::watch::Receiver<bool>) {
    tokio::spawn(async move {
        let config = crate::config::runtime().packages.clone();
        let configured: Vec<String> = config.index_sources.iter().map(|source| source.id.clone()).collect();
        if let Err(error) = store::drop_unconfigured(&state.db, &configured).await {
            tracing::warn!(%error, "cached plugin indexes of removed sources were not dropped");
        }
        if config.index_sources.is_empty() {
            return;
        }
        tracing::info!(sources = configured.len(), "plugin index refresher started");
        tokio::select! {
            () = tokio::time::sleep(FIRST_PASS_DELAY) => {}
            _ = stop.changed() => return,
        }
        loop {
            for source in &config.index_sources {
                if let Err(error) = refresh_source(&state, source, false).await {
                    tracing::warn!(source = %source.id, %error, "plugin index refresh could not run");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(TICK) => {}
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return;
                    }
                }
            }
        }
    });
}

#[derive(FromQueryResult)]
struct Lock {
    locked: bool,
}

/// Refreshes one source: fetch, validate, cache, sync. With `force` the interval is ignored.
///
/// # Errors
/// A database failure; a failed fetch is not an error, it is recorded and returned as
/// [`RefreshOutcome::Failed`].
pub async fn refresh_source(state: &AppState, source: &IndexSource, force: bool) -> Result<RefreshOutcome, ApiError> {
    let tx = state.db.begin().await?;
    let locked = Lock::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_try_advisory_xact_lock($1, hashtext($2)) AS locked",
        vec![LOCK_NAMESPACE.into(), source.id.clone().into()],
    ))
    .one(&tx)
    .await?
    .is_some_and(|row| row.locked);
    if !locked {
        return Ok(RefreshOutcome::Skipped);
    }
    let prior = store::cache_meta(&tx, &source.id).await?;
    let interval = crate::config::runtime().packages.index_refresh_secs;
    if !force
        && !prior
            .as_ref()
            .is_none_or(|prior| due(prior, interval, Timestamp::now()))
    {
        return Ok(RefreshOutcome::Skipped);
    }
    // A conditional request only for a copy of this very URL.
    let etag = prior
        .as_ref()
        .filter(|prior| prior.url == source.url && prior.fetched_at.is_some())
        .and_then(|prior| prior.etag.clone());
    let answer = fetch::get(&source.url, MAX_INDEX_BYTES, etag.as_deref(), "application/json").await;
    let (outcome, disabled) = match answer {
        Ok(Answer::NotModified) => {
            store::save_unchanged(&tx, &source.id).await?;
            (RefreshOutcome::Unchanged, 0)
        }
        Ok(Answer::Body { bytes, etag }) => match cache::parse_body(bytes.clone()).await {
            Ok(doc) => {
                store::save_fetched(
                    &tx,
                    &store::Fetched {
                        source_id: &source.id,
                        url: &source.url,
                        etag: etag.as_deref(),
                        body: &bytes,
                        generated_at: i64::try_from(doc.generated_at)
                            .ok()
                            .and_then(|secs| Timestamp::from_unix_timestamp(secs, 0)),
                        plugin_count: i32::try_from(doc.plugins.len()).unwrap_or(i32::MAX),
                    },
                )
                .await?;
                let report = sync::apply(&tx, &source.id, &doc).await?;
                store::save_warnings(&tx, &source.id, &report.warnings).await?;
                if report.keys_revoked > 0 || report.installations_disabled > 0 {
                    tracing::warn!(
                        source = %source.id,
                        keys = report.keys_revoked,
                        installations = report.installations_disabled,
                        "the plugin index revoked keys or versions"
                    );
                }
                (RefreshOutcome::Updated, report.installations_disabled)
            }
            Err(issues) => {
                record_failure(&tx, source, prior.as_ref(), &format!("invalid_document: {issues}")).await?;
                (RefreshOutcome::Failed, 0)
            }
        },
        Err(error) => {
            record_failure(&tx, source, prior.as_ref(), &error.text()).await?;
            (RefreshOutcome::Failed, 0)
        }
    };
    tx.commit().await?;
    if disabled > 0
        && let Err(error) = crate::extensions::rules::refresh(&state.db).await
    {
        tracing::warn!(%error, "the plugin rule registry was not refreshed");
    }
    Ok(outcome)
}

/// Whether a source is due: its interval has passed since the last check, or, after a failure, the
/// shorter retry delay.
fn due(prior: &store::CacheMeta, interval_secs: u64, now: Timestamp) -> bool {
    let wait = if prior.error.is_some() {
        interval_secs.min(RETRY_AFTER_FAILURE_SECS)
    } else {
        interval_secs
    };
    let waited = now.unix_timestamp().saturating_sub(prior.checked_at.unix_timestamp());
    u64::try_from(waited).is_ok_and(|waited| waited >= wait)
}

/// Records a failed refresh. The event is written when the error is new, so an index that stays
/// down fills the log once, not once per retry.
async fn record_failure(
    db: &impl ConnectionTrait,
    source: &IndexSource,
    prior: Option<&store::CacheMeta>,
    error: &str,
) -> Result<(), ApiError> {
    let error: String = error.chars().take(500).collect();
    let is_new = prior.is_none_or(|prior| prior.error.as_deref() != Some(error.as_str()));
    store::save_failure(db, &source.id, &source.url, &error).await?;
    if is_new {
        store::insert_event(db, None, &source.id, "refresh_failed", json!({ "error": error })).await?;
    }
    tracing::warn!(source = %source.id, %error, "plugin index refresh failed");
    Ok(())
}
