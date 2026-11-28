//! SQL of the index tables: the cache metadata, hidden sources, events, and the workspace's view
//! of what it already imported or installed.

use std::collections::{BTreeMap, BTreeSet};

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value as SqlValue};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// One cache row without its body.
#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct CacheMeta {
    pub(super) url: String,
    pub(super) etag: Option<String>,
    pub(super) generated_at: Option<Timestamp>,
    pub(super) plugin_count: i32,
    pub(super) fetched_at: Option<Timestamp>,
    pub(super) checked_at: Timestamp,
    pub(super) error: Option<String>,
    pub(super) warnings: Value,
}

pub(super) async fn cache_meta(db: &impl ConnectionTrait, source_id: &str) -> Result<Option<CacheMeta>, ApiError> {
    Ok(CacheMeta::find_by_statement(stmt(
        "SELECT url, etag, generated_at, plugin_count, fetched_at, checked_at, error, warnings \
         FROM package_index_cache WHERE source_id = $1",
        vec![source_id.into()],
    ))
    .one(db)
    .await?)
}

#[derive(FromQueryResult)]
struct BodyRow {
    body: Option<Vec<u8>>,
    fetched_at: Option<Timestamp>,
}

/// The cached body and the time it was fetched, for a cache row of this `url`.
pub(super) async fn cache_body(
    db: &impl ConnectionTrait,
    source_id: &str,
    url: &str,
) -> Result<Option<(Vec<u8>, Timestamp)>, ApiError> {
    let row = BodyRow::find_by_statement(stmt(
        "SELECT body, fetched_at FROM package_index_cache WHERE source_id = $1 AND url = $2",
        vec![source_id.into(), url.into()],
    ))
    .one(db)
    .await?;
    Ok(row.and_then(|row| row.body.zip(row.fetched_at)))
}

/// Removes the cache rows of sources that are no longer configured.
pub(super) async fn drop_unconfigured(db: &impl ConnectionTrait, configured: &[String]) -> Result<(), ApiError> {
    db.execute(stmt(
        "DELETE FROM package_index_cache WHERE source_id <> ALL($1)",
        vec![configured.to_vec().into()],
    ))
    .await?;
    Ok(())
}

/// A body that validated: replaces the cached copy and clears the error.
pub(super) struct Fetched<'a> {
    pub(super) source_id: &'a str,
    pub(super) url: &'a str,
    pub(super) etag: Option<&'a str>,
    pub(super) body: &'a [u8],
    pub(super) generated_at: Option<Timestamp>,
    pub(super) plugin_count: i32,
}

pub(super) async fn save_fetched(db: &impl ConnectionTrait, fetched: &Fetched<'_>) -> Result<(), ApiError> {
    db.execute(stmt(
        "INSERT INTO package_index_cache \
           (source_id, url, etag, body, generated_at, plugin_count, fetched_at, checked_at, error, warnings) \
         VALUES ($1, $2, $3, $4, $5, $6, now(), now(), NULL, '[]'::jsonb) \
         ON CONFLICT (source_id) DO UPDATE SET url = EXCLUDED.url, etag = EXCLUDED.etag, body = EXCLUDED.body, \
           generated_at = EXCLUDED.generated_at, plugin_count = EXCLUDED.plugin_count, fetched_at = now(), \
           checked_at = now(), error = NULL, warnings = '[]'::jsonb",
        vec![
            fetched.source_id.into(),
            fetched.url.into(),
            fetched.etag.map(str::to_owned).into(),
            fetched.body.to_vec().into(),
            fetched.generated_at.into(),
            fetched.plugin_count.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The index answered `304`: the cached copy is current.
pub(super) async fn save_unchanged(db: &impl ConnectionTrait, source_id: &str) -> Result<(), ApiError> {
    db.execute(stmt(
        "UPDATE package_index_cache SET checked_at = now(), error = NULL WHERE source_id = $1",
        vec![source_id.into()],
    ))
    .await?;
    Ok(())
}

/// A refresh failed: the last good copy (and its URL) stay, the error is recorded.
pub(super) async fn save_failure(
    db: &impl ConnectionTrait,
    source_id: &str,
    url: &str,
    error: &str,
) -> Result<(), ApiError> {
    db.execute(stmt(
        "INSERT INTO package_index_cache (source_id, url, checked_at, error) VALUES ($1, $2, now(), $3) \
         ON CONFLICT (source_id) DO UPDATE SET checked_at = now(), error = EXCLUDED.error",
        vec![source_id.into(), url.into(), error.into()],
    ))
    .await?;
    Ok(())
}

pub(super) async fn save_warnings(
    db: &impl ConnectionTrait,
    source_id: &str,
    warnings: &[String],
) -> Result<(), ApiError> {
    db.execute(stmt(
        "UPDATE package_index_cache SET warnings = $2 WHERE source_id = $1",
        vec![source_id.into(), serde_json::json!(warnings).into()],
    ))
    .await?;
    Ok(())
}

/// Appends a row to the audit log of the index.
pub(super) async fn insert_event(
    db: &impl ConnectionTrait,
    workspace_id: Option<Uuid>,
    source_id: &str,
    action: &str,
    details: Value,
) -> Result<(), ApiError> {
    db.execute(stmt(
        "INSERT INTO package_index_events (workspace_id, source_id, action, details) VALUES ($1, $2, $3, $4)",
        vec![workspace_id.into(), source_id.into(), action.into(), details.into()],
    ))
    .await?;
    Ok(())
}

/// Sources the workspace owner hid.
pub(super) async fn hidden_sources(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<BTreeSet<String>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        source_id: String,
    }
    Ok(Row::find_by_statement(stmt(
        "SELECT source_id FROM workspace_index_sources WHERE workspace_id = $1 AND hidden",
        vec![workspace_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.source_id)
    .collect())
}

/// What the workspace already holds of some plugins.
#[derive(Debug, Default)]
pub(super) struct WorkspaceState {
    /// Plugin id to the digests of the workspace's packages of it.
    pub(super) packages: BTreeMap<String, BTreeSet<String>>,
    /// Plugin ids with a live installation.
    pub(super) installed: BTreeSet<String>,
    /// Publisher to the ids of its active keys the workspace has pinned.
    pub(super) keys: BTreeMap<String, BTreeSet<String>>,
}

pub(super) async fn workspace_state(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    plugin_ids: &[String],
    publishers: &[String],
) -> Result<WorkspaceState, ApiError> {
    #[derive(FromQueryResult)]
    struct Package {
        package_key: String,
        digest: String,
    }
    #[derive(FromQueryResult)]
    struct Installed {
        package_key: String,
    }
    #[derive(FromQueryResult)]
    struct Key {
        publisher: String,
        key_id: String,
    }
    let mut state = WorkspaceState::default();
    for row in Package::find_by_statement(stmt(
        "SELECT package_key, digest FROM packages WHERE workspace_id = $1 AND package_key = ANY($2)",
        vec![workspace_id.into(), plugin_ids.to_vec().into()],
    ))
    .all(db)
    .await?
    {
        state.packages.entry(row.package_key).or_default().insert(row.digest);
    }
    state.installed = Installed::find_by_statement(stmt(
        "SELECT DISTINCT package_key FROM installations \
         WHERE workspace_id = $1 AND status <> 'revoked' AND package_key = ANY($2)",
        vec![workspace_id.into(), plugin_ids.to_vec().into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.package_key)
    .collect();
    for row in Key::find_by_statement(stmt(
        "SELECT publisher, key_id FROM publisher_keys \
         WHERE workspace_id = $1 AND status = 'active' AND publisher = ANY($2)",
        vec![workspace_id.into(), publishers.to_vec().into()],
    ))
    .all(db)
    .await?
    {
        state.keys.entry(row.publisher).or_default().insert(row.key_id);
    }
    Ok(state)
}
