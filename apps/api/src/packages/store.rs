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
    "id, digest, blob_digest, package_key, version, kind, manifest, files, custom_code, tier, origin, created_at";

#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct PackageRow {
    pub(super) id: Uuid,
    pub(super) digest: String,
    pub(super) blob_digest: String,
    pub(super) package_key: String,
    pub(super) version: String,
    pub(super) kind: String,
    pub(super) manifest: Value,
    pub(super) files: Value,
    pub(super) custom_code: bool,
    pub(super) tier: String,
    /// Where the package came from, for a package imported from a public plugin index.
    pub(super) origin: Option<Value>,
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
    /// `{source, plugin_id, publisher, name, version, digest, url, sha256, imported_by, imported_at}` of a
    /// package imported from a public plugin index, else `null`.
    pub origin: Option<Value>,
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
            origin: row.origin.clone(),
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
    pub(super) files: &'a Value,
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

/// The built-in package rows signed by an active key, for the boot re-verification (a row of a
/// revoked key is not installable and is left alone).
pub(super) async fn list_builtin(db: &impl ConnectionTrait) -> Result<Vec<PackageRow>, ApiError> {
    Ok(PackageRow::find_by_statement(stmt(
        &format!(
            "SELECT {PACKAGE_COLUMNS} FROM packages WHERE tier = 'builtin' AND publisher_key_id IN \
             (SELECT id FROM publisher_keys WHERE status = 'active') ORDER BY created_at, id"
        ),
        vec![],
    ))
    .all(db)
    .await?)
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

/// What a workspace stores: its private package rows and the bytes of their distinct blobs.
#[derive(Debug, Clone, Copy, FromQueryResult)]
pub(super) struct Usage {
    pub(super) packages: i64,
    pub(super) bytes: i64,
}

/// Serialises the uploads of one workspace for the rest of the transaction, so the quota check
/// and the insert cannot race another upload of the same workspace.
pub(super) async fn lock_workspace_uploads(db: &impl ConnectionTrait, workspace_id: Uuid) -> Result<(), ApiError> {
    db.execute(stmt(
        "SELECT pg_advisory_xact_lock(hashtextextended('package_upload:' || $1::text, 0))",
        vec![workspace_id.into()],
    ))
    .await?;
    Ok(())
}

/// The workspace's stored private packages and blob bytes.
pub(super) async fn workspace_usage(db: &impl ConnectionTrait, workspace_id: Uuid) -> Result<Usage, ApiError> {
    Ok(Usage::find_by_statement(stmt(
        "SELECT (SELECT count(*) FROM packages WHERE workspace_id = $1) AS packages, \
                COALESCE((SELECT sum(b.size)::bigint FROM package_blobs b WHERE b.blob_digest IN \
                          (SELECT blob_digest FROM packages WHERE workspace_id = $1)), 0) AS bytes",
        vec![workspace_id.into()],
    ))
    .one(db)
    .await?
    .unwrap_or(Usage { packages: 0, bytes: 0 }))
}

/// Whether the workspace already holds a package that uses this blob (it adds no stored bytes).
pub(super) async fn workspace_has_blob(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    blob_digest: &str,
) -> Result<bool, ApiError> {
    Ok(db
        .query_one(stmt(
            "SELECT 1 FROM packages WHERE workspace_id = $1 AND blob_digest = $2 LIMIT 1",
            vec![workspace_id.into(), blob_digest.into()],
        ))
        .await?
        .is_some())
}

/// The blob digests the workspace's packages use. The workspace teardown reads them before the
/// workspace rows cascade away and passes them to [`delete_unreferenced_blobs`] after.
pub async fn workspace_blob_digests(db: &impl ConnectionTrait, workspace_id: Uuid) -> Result<Vec<String>, ApiError> {
    #[derive(FromQueryResult)]
    struct Digest {
        blob_digest: String,
    }
    Ok(Digest::find_by_statement(stmt(
        "SELECT DISTINCT blob_digest FROM packages WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.blob_digest)
    .collect())
}

/// Deletes the listed blobs that no package row uses any more (a blob shared with another
/// workspace or a built-in package stays).
pub async fn delete_unreferenced_blobs(db: &impl ConnectionTrait, digests: &[String]) -> Result<(), ApiError> {
    if digests.is_empty() {
        return Ok(());
    }
    db.execute(stmt(
        "DELETE FROM package_blobs b WHERE b.blob_digest = ANY($1) \
         AND NOT EXISTS (SELECT 1 FROM packages p WHERE p.blob_digest = b.blob_digest)",
        vec![digests.to_vec().into()],
    ))
    .await?;
    Ok(())
}

/// Condition on `installation_generations g` for a live generation: `pending` or `approved` of an
/// installation that is not revoked.
const LIVE_GENERATION: &str = "g.status IN ('pending','approved') \
     AND NOT EXISTS (SELECT 1 FROM installations i WHERE i.id = g.installation_id AND i.status = 'revoked')";

/// Condition on `installation_generations g` for history: `rejected` or `superseded`, or any
/// generation of a revoked installation. The delete statements carry exactly this condition, so a
/// live generation committed after the check is never removed.
const HISTORY_GENERATION: &str = "(g.status IN ('rejected','superseded') \
     OR EXISTS (SELECT 1 FROM installations i WHERE i.id = g.installation_id AND i.status = 'revoked'))";

/// Whether a live generation references the package row. Live means the generation is `pending`
/// or `approved` (the active one) and its installation is not `revoked`; `rejected` and
/// `superseded` generations, and every generation of a revoked installation, are history.
pub(super) async fn has_live_generation(db: &impl ConnectionTrait, package_row_id: Uuid) -> Result<bool, ApiError> {
    Ok(db
        .query_one(stmt(
            &format!(
                "SELECT 1 FROM installation_generations g WHERE g.package_row_id = $1 AND {LIVE_GENERATION} LIMIT 1"
            ),
            vec![package_row_id.into()],
        ))
        .await?
        .is_some())
}

/// Takes the publisher key row of the package `FOR SHARE`, with no status filter. A key revocation
/// updates the key first and then the packages and installations it signed, so a delete that takes
/// the installations before the key could deadlock with it; the shared key lock makes the two
/// finish one after the other. The package row is read through a subquery because the key id
/// never changes and every package has one. Call it first in the delete transaction, right after
/// the workspace upload lock.
pub(super) async fn lock_package_key(db: &impl ConnectionTrait, package_row_id: Uuid) -> Result<(), ApiError> {
    db.execute(stmt(
        "SELECT 1 FROM publisher_keys WHERE id = (SELECT publisher_key_id FROM packages WHERE id = $1) FOR SHARE",
        vec![package_row_id.into()],
    ))
    .await?;
    Ok(())
}

/// Deletes the non-live generations that reference the package row, with the grants and layouts
/// that hang on them (their foreign keys cascade) and the site pages those layouts serve. A
/// revoked installation may still name such a generation as its last active one; that pointer is
/// cleared first so the deferred foreign key holds. Every statement carries the history condition,
/// so a live generation that appeared after [`has_live_generation`] survives and the package delete
/// that follows fails on the `RESTRICT` key (`23503`, mapped to `package_in_use`). Call it in the
/// delete transaction.
pub(super) async fn delete_history_generations(
    db: &impl ConnectionTrait,
    package_row_id: Uuid,
) -> Result<(), ApiError> {
    // Lock every installation that has a generation of this package, in id order, so a concurrent
    // revoke either commits before the statements below (and is seen by them) or waits until this
    // transaction ends. Lock order of the package delete: publisher key ([`lock_package_key`]), then
    // installation, then the package row, the same key-first order as `install::store::lock_installation`.
    db.execute(stmt(
        "SELECT i.id FROM installations i WHERE EXISTS (SELECT 1 FROM installation_generations g \
         WHERE g.installation_id = i.id AND g.package_row_id = $1) ORDER BY i.id FOR UPDATE OF i",
        vec![package_row_id.into()],
    ))
    .await?;
    db.execute(stmt(
        "UPDATE installations i SET active_generation = NULL WHERE i.status = 'revoked' \
         AND i.active_generation IS NOT NULL AND EXISTS (SELECT 1 FROM installation_generations g \
         WHERE g.installation_id = i.id AND g.generation = i.active_generation AND g.package_row_id = $1)",
        vec![package_row_id.into()],
    ))
    .await?;
    // `site_pages.layout_id` has no ON DELETE: the pages go before the layouts cascade. Pages of
    // people (`installation_id IS NULL`) cannot show a package layout (`load_page_layout` refuses
    // layouts with an installation), so only package pages are removed.
    db.execute(stmt(
        &format!(
            "DELETE FROM site_pages p WHERE p.installation_id IS NOT NULL AND EXISTS (\
             SELECT 1 FROM installation_generations g WHERE g.package_row_id = $1 AND {HISTORY_GENERATION} \
             AND g.installation_id = p.installation_id AND g.generation = p.generation \
             UNION ALL SELECT 1 FROM layouts l JOIN installation_generations g \
             ON g.installation_id = l.installation_id AND g.generation = l.generation \
             WHERE l.id = p.layout_id AND g.package_row_id = $1 AND {HISTORY_GENERATION})"
        ),
        vec![package_row_id.into()],
    ))
    .await?;
    db.execute(stmt(
        &format!("DELETE FROM installation_generations g WHERE g.package_row_id = $1 AND {HISTORY_GENERATION}"),
        vec![package_row_id.into()],
    ))
    .await?;
    Ok(())
}

/// Deletes one package row. The foreign key from the installation generations is `RESTRICT`, so a
/// live generation created after the caller's check still refuses the delete (SQLSTATE `23503`).
pub(super) async fn delete_package(db: &impl ConnectionTrait, package_row_id: Uuid) -> Result<(), ApiError> {
    db.execute(stmt("DELETE FROM packages WHERE id = $1", vec![package_row_id.into()]))
        .await?;
    Ok(())
}

/// Inserts a package row; `None` when the owner already has this digest.
pub(super) async fn insert_package(
    db: &impl ConnectionTrait,
    new: &NewPackage<'_>,
) -> Result<Option<PackageRow>, ApiError> {
    Ok(PackageRow::find_by_statement(stmt(
        &format!(
            "INSERT INTO packages (workspace_id, digest, blob_digest, package_key, version, kind, publisher_key_id, \
             manifest, files, custom_code, tier, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
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
            new.files.clone().into(),
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
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
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

#[cfg(test)]
mod delete_condition_tests {
    use super::{HISTORY_GENERATION, LIVE_GENERATION};

    #[test]
    fn live_and_history_conditions_are_complementary() {
        for status in ["pending", "approved"] {
            assert!(LIVE_GENERATION.contains(status));
            assert!(!HISTORY_GENERATION.contains(status));
        }
        for status in ["rejected", "superseded"] {
            assert!(HISTORY_GENERATION.contains(status));
            assert!(!LIVE_GENERATION.contains(status));
        }
        assert!(LIVE_GENERATION.contains("NOT EXISTS") && LIVE_GENERATION.contains("'revoked'"));
        assert!(HISTORY_GENERATION.contains("EXISTS") && HISTORY_GENERATION.contains("'revoked'"));
    }
}
