//! Publisher keys: rows, registration and revocation SQL, and the boot sync of the built-in `sv`
//! keys (contract C3 section 4.2).

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value as SqlValue};
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;

const KEY_COLUMNS: &str = "id, publisher, key_id, status, created_at, revoked_at";

#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct KeyRow {
    pub(super) id: Uuid,
    pub(super) publisher: String,
    pub(super) key_id: String,
    pub(super) status: String,
    pub(super) created_at: Timestamp,
    pub(super) revoked_at: Option<Timestamp>,
}

/// A publisher key as the routes show it.
#[derive(Debug, Serialize)]
pub struct PublisherKey {
    pub id: Uuid,
    pub publisher: String,
    pub key_id: String,
    pub status: String,
    pub created_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
}

impl From<&KeyRow> for PublisherKey {
    fn from(row: &KeyRow) -> Self {
        Self {
            id: row.id,
            publisher: row.publisher.clone(),
            key_id: row.key_id.clone(),
            status: row.status.clone(),
            created_at: row.created_at,
            revoked_at: row.revoked_at,
        }
    }
}

fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

pub(super) async fn list_workspace_keys(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Vec<KeyRow>, ApiError> {
    Ok(KeyRow::find_by_statement(stmt(
        &format!("SELECT {KEY_COLUMNS} FROM publisher_keys WHERE workspace_id = $1 ORDER BY created_at, id"),
        vec![workspace_id.into()],
    ))
    .all(db)
    .await?)
}

/// Registers a key; `None` when the workspace already has a key with this `key_id`.
pub(super) async fn insert_workspace_key(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    publisher: &str,
    key_id: &str,
    public_key: &[u8],
    created_by: Uuid,
) -> Result<Option<KeyRow>, ApiError> {
    Ok(KeyRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO publisher_keys (workspace_id, publisher, key_id, public_key, created_by) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (workspace_id, key_id) DO NOTHING RETURNING {KEY_COLUMNS}"
        ),
        vec![
            workspace_id.into(),
            publisher.into(),
            key_id.into(),
            public_key.to_vec().into(),
            created_by.into(),
        ],
    ))
    .one(db)
    .await?)
}

pub(super) async fn find_workspace_key(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<Option<KeyRow>, ApiError> {
    Ok(KeyRow::find_by_statement(stmt(
        &format!("SELECT {KEY_COLUMNS} FROM publisher_keys WHERE id = $1 AND workspace_id = $2"),
        vec![id.into(), workspace_id.into()],
    ))
    .one(db)
    .await?)
}

/// Revokes a key and makes every package signed by it not installable. Returns the row as it is
/// afterwards. Installations that use such a package are disabled by the install flow (0138).
pub(super) async fn revoke_key(db: &impl ConnectionTrait, row: &KeyRow) -> Result<KeyRow, ApiError> {
    db.execute(stmt(
        "UPDATE publisher_keys SET status = 'revoked', revoked_at = COALESCE(revoked_at, now()) WHERE id = $1",
        vec![row.id.into()],
    ))
    .await?;
    db.execute(stmt(
        "UPDATE packages SET installable = false WHERE publisher_key_id = $1",
        vec![row.id.into()],
    ))
    .await?;
    KeyRow::find_by_statement(stmt(
        &format!("SELECT {KEY_COLUMNS} FROM publisher_keys WHERE id = $1"),
        vec![row.id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)
}

/// Makes the set of active `sv` keys equal to `public_key` (or empty): every other `sv` key is
/// revoked together with its packages' installability, and the configured key is inserted or
/// re-activated.
pub(super) async fn sync_builtin_key(db: &impl ConnectionTrait, key: Option<(&str, &[u8])>) -> Result<(), ApiError> {
    let keep = key.map(|(id, _)| id.to_owned());
    db.execute(stmt(
        "UPDATE packages SET installable = false WHERE publisher_key_id IN \
         (SELECT id FROM publisher_keys WHERE publisher = 'sv' AND workspace_id IS NULL AND key_id IS DISTINCT FROM $1)",
        vec![keep.clone().into()],
    ))
    .await?;
    db.execute(stmt(
        "UPDATE publisher_keys SET status = 'revoked', revoked_at = COALESCE(revoked_at, now()) \
         WHERE publisher = 'sv' AND workspace_id IS NULL AND status = 'active' AND key_id IS DISTINCT FROM $1",
        vec![keep.into()],
    ))
    .await?;
    if let Some((key_id, public)) = key {
        db.execute(stmt(
            "INSERT INTO publisher_keys (workspace_id, publisher, key_id, public_key) VALUES (NULL, 'sv', $1, $2) \
             ON CONFLICT (workspace_id, key_id) DO UPDATE SET status = 'active', revoked_at = NULL",
            vec![key_id.into(), public.to_vec().into()],
        ))
        .await?;
        db.execute(stmt(
            "UPDATE packages SET installable = true WHERE tier = 'builtin' AND publisher_key_id IN \
             (SELECT id FROM publisher_keys WHERE publisher = 'sv' AND workspace_id IS NULL AND key_id = $1)",
            vec![key_id.into()],
        ))
        .await?;
    }
    Ok(())
}

/// Takes a shared row lock on an active key, so a concurrent revocation waits for this
/// transaction; `false` when the key is no longer active.
pub(super) async fn lock_active(db: &impl ConnectionTrait, id: Uuid) -> Result<bool, ApiError> {
    Ok(db
        .query_one(stmt(
            "SELECT 1 FROM publisher_keys WHERE id = $1 AND status = 'active' FOR SHARE",
            vec![id.into()],
        ))
        .await?
        .is_some())
}
