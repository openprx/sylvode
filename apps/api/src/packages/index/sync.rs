//! Revocation sync (contract M15, section 2): what a freshly fetched index revokes in the
//! workspaces that took something from it.
//!
//! - A key the index lists `revoked` and a workspace pinned from this source (`publisher_keys.origin`)
//!   is revoked through the existing path (`keys::revoke_key`, `disable_for_revoked_key`).
//! - A version the index lists `revoked` makes the workspaces' packages of that digest and source
//!   (`packages.origin`) not installable and disables their installations with `index_revoked`.
//! - A pinned key or an imported package that the fetched index no longer lists causes no action,
//!   only a warning on the source status (a forced push must not be able to revoke by omission).
//!
//! The sync is idempotent: a second run over the same index changes nothing and logs nothing.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use sv_index::{IndexDoc, KeyStatus, VersionStatus};
use uuid::Uuid;

use super::store;
use crate::{
    error::ApiError,
    extensions::install::store::{disable_for_index_revoked, disable_for_revoked_key},
    packages::keys,
};

/// What a sync did.
#[derive(Debug, Default)]
pub(super) struct SyncReport {
    pub(super) keys_revoked: usize,
    pub(super) installations_disabled: usize,
    pub(super) warnings: Vec<String>,
}

#[derive(FromQueryResult)]
struct Pinned {
    id: Uuid,
    workspace_id: Uuid,
    publisher: String,
    key_id: String,
}

#[derive(FromQueryResult)]
struct Changed {
    workspace_id: Uuid,
    digest: String,
    package_key: String,
    version: String,
}

#[derive(FromQueryResult)]
struct Count {
    count: i64,
}

fn origin(source_id: &str) -> String {
    format!("index:{source_id}")
}

pub(super) async fn apply(db: &impl ConnectionTrait, source_id: &str, doc: &IndexDoc) -> Result<SyncReport, ApiError> {
    let mut report = SyncReport::default();
    sync_keys(db, source_id, doc, &mut report).await?;
    sync_versions(db, source_id, doc, &mut report).await?;
    warn_missing(db, source_id, doc, &mut report).await?;
    Ok(report)
}

async fn sync_keys(
    db: &impl ConnectionTrait,
    source_id: &str,
    doc: &IndexDoc,
    report: &mut SyncReport,
) -> Result<(), ApiError> {
    // key_id to the publisher the index lists it under.
    let revoked: BTreeMap<&str, &str> = doc
        .publishers
        .values()
        .flat_map(|publisher| {
            publisher
                .keys
                .iter()
                .filter(|key| key.status == KeyStatus::Revoked)
                .map(|key| (key.key_id.as_str(), publisher.id.as_str()))
        })
        .collect();
    if revoked.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = revoked.keys().map(|key_id| (*key_id).to_owned()).collect();
    let pinned = Pinned::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, publisher, key_id FROM publisher_keys \
         WHERE origin = $1 AND status = 'active' AND workspace_id IS NOT NULL AND key_id = ANY($2) \
         ORDER BY created_at, id",
        vec![origin(source_id).into(), ids.into()],
    ))
    .all(db)
    .await?;
    for key in pinned {
        // The key must be revoked under the publisher it was pinned for.
        if revoked.get(key.key_id.as_str()) != Some(&key.publisher.as_str()) {
            continue;
        }
        let Some(row) = keys::find_workspace_key(db, key.workspace_id, key.id).await? else {
            continue;
        };
        keys::revoke_key(db, &row).await?;
        let stopped = disable_for_revoked_key(db, key.id).await?;
        store::insert_event(
            db,
            Some(key.workspace_id),
            source_id,
            "key_revoked_sync",
            json!({
                "publisher": key.publisher,
                "key_id": key.key_id,
                "installations_disabled": stopped.len(),
            }),
        )
        .await?;
        report.keys_revoked += 1;
        report.installations_disabled += stopped.len();
    }
    Ok(())
}

async fn sync_versions(
    db: &impl ConnectionTrait,
    source_id: &str,
    doc: &IndexDoc,
    report: &mut SyncReport,
) -> Result<(), ApiError> {
    let digests: Vec<String> = doc
        .plugins
        .iter()
        .flat_map(|plugin| plugin.versions.iter())
        .filter(|version| version.status == VersionStatus::Revoked)
        .map(|version| version.digest.clone())
        .collect();
    if digests.is_empty() {
        return Ok(());
    }
    // Installations first, then packages: the order of the package delete (installations, package),
    // so the two cannot wait for each other.
    let stopped = disable_for_index_revoked(db, source_id, &digests).await?;
    let changed = Changed::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE packages SET installable = false \
         WHERE origin ->> 'source' = $1 AND digest = ANY($2) AND installable AND workspace_id IS NOT NULL \
         RETURNING workspace_id, digest, package_key, version",
        vec![source_id.into(), digests.into()],
    ))
    .all(db)
    .await?;
    let mut events: BTreeMap<(Uuid, String), (String, String, usize)> = BTreeMap::new();
    for row in changed {
        events.insert((row.workspace_id, row.digest), (row.package_key, row.version, 0));
    }
    for stop in &stopped {
        events
            .entry((stop.workspace_id, stop.digest.clone()))
            .or_insert_with(|| (stop.package_key.clone(), stop.version.clone(), 0))
            .2 += 1;
    }
    for ((workspace_id, digest), (plugin, version, disabled)) in events {
        store::insert_event(
            db,
            Some(workspace_id),
            source_id,
            "version_revoked_sync",
            json!({
                "plugin": plugin,
                "version": version,
                "digest": digest,
                "installations_disabled": disabled,
            }),
        )
        .await?;
    }
    report.installations_disabled += stopped.len();
    Ok(())
}

async fn warn_missing(
    db: &impl ConnectionTrait,
    source_id: &str,
    doc: &IndexDoc,
    report: &mut SyncReport,
) -> Result<(), ApiError> {
    let listed_keys: BTreeSet<String> = doc
        .publishers
        .values()
        .flat_map(|publisher| publisher.keys.iter().map(|key| key.key_id.clone()))
        .collect();
    let listed_digests: BTreeSet<String> = doc
        .plugins
        .iter()
        .flat_map(|plugin| plugin.versions.iter().map(|version| version.digest.clone()))
        .collect();
    let missing_keys = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS count FROM publisher_keys WHERE origin = $1 AND key_id <> ALL($2)",
        vec![
            origin(source_id).into(),
            listed_keys.into_iter().collect::<Vec<_>>().into(),
        ],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.count);
    let missing_versions = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS count FROM packages WHERE origin ->> 'source' = $1 AND digest <> ALL($2)",
        vec![source_id.into(), listed_digests.into_iter().collect::<Vec<_>>().into()],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.count);
    if missing_keys > 0 {
        report.warnings.push(format!(
            "{missing_keys} publisher key(s) taken from this source are missing from the index"
        ));
    }
    if missing_versions > 0 {
        report.warnings.push(format!(
            "{missing_versions} imported package(s) from this source are missing from the index"
        ));
    }
    Ok(())
}
