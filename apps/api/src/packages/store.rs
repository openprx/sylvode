//! SQL access to `package_blobs`, `packages` and `publisher_keys`.
//!
//! Every lookup of a package is filtered to built-in packages or the caller's workspace, so a
//! private package of another workspace cannot be found, whatever digest is asked for.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value as SqlValue};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

const PACKAGE_COLUMNS: &str =
    "id, digest, blob_digest, package_key, version, kind, manifest, custom_code, tier, created_at";

#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct PackageRow {
    pub(super) id: Uuid,
    pub(super) digest: String,
    pub(super) blob_digest: String,
    pub(super) package_key: String,
    pub(super) version: String,
    pub(super) kind: String,
    pub(super) manifest: Value,
    pub(super) custom_code: bool,
    pub(super) tier: String,
    pub(super) created_at: Timestamp,
}

/// `PackageSummary` of contract C3.
#[derive(Debug, Serialize)]
pub struct PackageSummary {
    pub id: Uuid,
    pub digest: String,
    pub package_key: String,
    pub version: String,
    pub kind: String,
    pub tier: String,
    pub publisher: Value,
    pub custom_code: bool,
    pub summary: Value,
    pub created_at: Timestamp,
}

impl From<&PackageRow> for PackageSummary {
    fn from(row: &PackageRow) -> Self {
        Self {
            id: row.id,
            digest: row.digest.clone(),
            package_key: row.package_key.clone(),
            version: row.version.clone(),
            kind: row.kind.clone(),
            tier: row.tier.clone(),
            publisher: row.manifest.get("publisher").cloned().unwrap_or(Value::Null),
            custom_code: row.custom_code,
            summary: row.manifest.get("summary").cloned().unwrap_or(Value::Null),
            created_at: row.created_at,
        }
    }
}

/// The columns a new package row takes.
pub(super) struct NewPackage<'a> {
    pub(super) workspace_id: Option<Uuid>,
    pub(super) digest: &'a str,
    pub(super) blob_digest: &'a str,
    pub(super) package_key: &'a str,
    pub(super) version: &'a str,
    pub(super) kind: &'a str,
    pub(super) publisher_key_id: Uuid,
    pub(super) manifest: &'a Value,
    pub(super) custom_code: bool,
    pub(super) tier: &'a str,
    pub(super) created_by: Option<Uuid>,
}

fn stmt(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

/// Stores the blob once; an existing blob of the same digest is left as it is.
pub(super) async fn put_blob(db: &impl ConnectionTrait, digest: &str, bytes: Vec<u8>) -> Result<(), ApiError> {
    let size = i64::try_from(bytes.len()).map_err(|_| ApiError::internal())?;
    db.execute(stmt(
        "INSERT INTO package_blobs (blob_digest, size, bytes) VALUES ($1, $2, $3) ON CONFLICT (blob_digest) DO NOTHING",
        vec![digest.into(), size.into(), bytes.into()],
    ))
    .await?;
    Ok(())
}

pub(super) async fn blob_bytes(db: &impl ConnectionTrait, digest: &str) -> Result<Option<Vec<u8>>, ApiError> {
    #[derive(FromQueryResult)]
    struct Bytes {
        bytes: Vec<u8>,
    }
    Ok(Bytes::find_by_statement(stmt(
        "SELECT bytes FROM package_blobs WHERE blob_digest = $1",
        vec![digest.into()],
    ))
    .one(db)
    .await?
    .map(|b| b.bytes))
}

/// A package by digest, visible to `workspace_id`: built-in, or private to that workspace.
pub(super) async fn find_visible(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    digest: &str,
) -> Result<Option<PackageRow>, ApiError> {
    Ok(PackageRow::find_by_statement(stmt(
        &format!(
            "SELECT {PACKAGE_COLUMNS} FROM packages WHERE digest = $1 AND (tier = 'builtin' OR workspace_id = $2) \
             ORDER BY (tier = 'builtin') DESC LIMIT 1"
        ),
        vec![digest.into(), workspace_id.into()],
    ))
    .one(db)
    .await?)
}

/// The package row of exactly this digest and owner (`None` workspace = built-in).
pub(super) async fn find_owned(
    db: &impl ConnectionTrait,
    workspace_id: Option<Uuid>,
    digest: &str,
) -> Result<Option<PackageRow>, ApiError> {
    Ok(PackageRow::find_by_statement(stmt(
        &format!("SELECT {PACKAGE_COLUMNS} FROM packages WHERE digest = $1 AND workspace_id IS NOT DISTINCT FROM $2"),
        vec![digest.into(), workspace_id.into()],
    ))
    .one(db)
    .await?)
}

/// Whether another package of the same owner already uses this key and version.
pub(super) async fn version_taken(
    db: &impl ConnectionTrait,
    workspace_id: Option<Uuid>,
    package_key: &str,
    version: &str,
) -> Result<bool, ApiError> {
    Ok(db
        .query_one(stmt(
            "SELECT 1 FROM packages WHERE package_key = $1 AND version = $2 AND workspace_id IS NOT DISTINCT FROM $3",
            vec![package_key.into(), version.into(), workspace_id.into()],
        ))
        .await?
        .is_some())
}

/// Inserts a package row; `None` when the owner already has this digest.
pub(super) async fn insert_package(
    db: &impl ConnectionTrait,
    new: &NewPackage<'_>,
) -> Result<Option<PackageRow>, ApiError> {
    Ok(PackageRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO packages (workspace_id, digest, blob_digest, package_key, version, kind, publisher_key_id, \
             manifest, custom_code, tier, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             ON CONFLICT (digest, workspace_id) DO NOTHING RETURNING {PACKAGE_COLUMNS}"
        ),
        vec![
            new.workspace_id.into(),
            new.digest.into(),
            new.blob_digest.into(),
            new.package_key.into(),
            new.version.into(),
            new.kind.into(),
            new.publisher_key_id.into(),
            new.manifest.clone().into(),
            new.custom_code.into(),
            new.tier.into(),
            new.created_by.into(),
        ],
    ))
    .one(db)
    .await?)
}

/// Escapes `%`, `_` and `\` for a `LIKE ... ESCAPE '\'` pattern.
fn like_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// One page of the packages visible to the workspace, newest first.
pub(super) async fn list_visible(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    kind: Option<&str>,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<PackageRow>, i64), ApiError> {
    let filter = "(tier = 'builtin' OR workspace_id = $1) AND ($2::text IS NULL OR kind = $2) \
                  AND ($3::text IS NULL OR package_key ILIKE $3 ESCAPE '\\' OR (manifest->'summary'->>'en') ILIKE $3 ESCAPE '\\')";
    let pattern = q.map(like_pattern);
    let args = || -> Vec<SqlValue> {
        vec![
            workspace_id.into(),
            kind.map(str::to_owned).into(),
            pattern.clone().into(),
        ]
    };
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    let total = Count::find_by_statement(stmt(
        &format!("SELECT count(*) AS n FROM packages WHERE {filter}"),
        args(),
    ))
    .one(db)
    .await?
    .map_or(0, |c| c.n);
    let mut values = args();
    values.push(limit.into());
    values.push(offset.into());
    let rows = PackageRow::find_by_statement(stmt(
        &format!(
            "SELECT {PACKAGE_COLUMNS} FROM packages WHERE {filter} ORDER BY created_at DESC, id LIMIT $4 OFFSET $5"
        ),
        values,
    ))
    .all(db)
    .await?;
    Ok((rows, total))
}

#[cfg(test)]
mod tests {
    use super::like_pattern;

    #[test]
    fn like_wildcards_in_the_query_are_escaped() {
        assert_eq!(like_pattern("a%b_c\\"), "%a\\%b\\_c\\\\%");
    }
}
